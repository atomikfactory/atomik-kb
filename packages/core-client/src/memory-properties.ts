// The in-memory twin of typed page properties (ADR-0026), used by `MemoryTransport`: the
// handlers of `kb-core`'s `properties.rs` over the schema and the values `kb-model` keeps in
// each space's structure document (`properties.rs`, `properties/value.rs`). Nothing here knows
// about requests or responses; `MemoryTransport` maps one onto the other. The rules of a value
// are in `./properties`; the page tree, the trash and the undo history stay in `./structure`.
//
// - A space has a schema: Tags first (a virtual entry until something writes it: its first
//   option, a rename), then the other properties in the order they were created or moved. A
//   page has at most one value per property, kept here by page id (not in the page records),
//   so structural undo never touches the schema or a value, exactly like the core, whose
//   writes carry an origin that structural undo excludes. A purged page keeps its values
//   (hidden: no read, no query, no backup reaches a page that does not exist), so that an undone
//   purge brings them back, as it does in the core; `SpaceDelete` drops the schema and every value.
// - Reads show what is readable: the property exists, the value is of its kind and follows its
//   rules, and options the property no longer lists are dropped (a select or status value that
//   names one reads as nothing, a multi-select keeps the known options and reads as nothing
//   when none is left).
// - Deleting a property hides its values at once; they stay stored until the next real write of
//   that page's values removes them (the core's lazy clean-up). Deleting an option removes it from
//   every value of the property in the space (live and trashed pages) in one change and sets those
//   pages' `updatedAt`.
// - Events: every accepted request emits `StructureChanged { spaceId }`, also when nothing
//   changed; a change of the schema (create, rename, delete, move, option add, update and delete)
//   emits `PropertySchemaChanged { spaceId }` first. Refused requests emit nothing.
// - `pagesBy` answers `PagesByProperty`: the live pages of the space whose value matches, ordered
//   by title (ASCII-only case folding, then the exact title, then the page id, compared by code
//   point), with the core's filters and their errors.
// - `searchTexts` is what the text index holds for a page's properties: one text per property that
//   has some (option names, a text, URL, email or phone value), which `MemoryContent.search`
//   matches like a block.
// - Backups carry the written schema and every stored value (`exportState`, `prepareRestore`).
// - Files (amendment of 2026-10-07): a `files` value is an ordered list of attachment ids. The
//   attachments it lists count as references of the attachment (`attachmentReferenceCounts`, one
//   per page and property that lists it, live and trashed pages, never a purged one and never a
//   property that was deleted), and every change of a value or of the schema that may change them
//   tells the attachments through `attachmentsChanged` first (the core's `attachment_refs_changed`),
//   so that the retention bookkeeping and `AttachmentsChanged` follow. Their text is the file names
//   of the manifests (`attachmentName`); a checklist's is its item texts.
// - Relations and rollups (ADR-0027): a `relation` is a stored ordered list of page ids. A two-way
//   relation is two properties naming each other; a write of one side also writes the other side of
//   every page it adds or removes, and a read shows the union of what the page stores and what the
//   other pages store about it (`#effective`), so the view is symmetric whatever the merge left.
//   Pages that are unknown, purged or of another space read as nothing; trashed ones stay listed.
//   A `rollup` is computed from the related pages (`#rollupValue`, `./rollup`), ignoring trashed
//   pages, and is read-only; `config` of a record holds the relation's pairing or the rollup's
//   configuration.
// - Place and the computed kinds (amendment of 2026-10-08): a `place` is a stored value whose text
//   is its name. `created_time`, `last_edited_time`, `created_by` and `last_edited_by` are never
//   stored: `valuesOf` and the filters read them from the page (`PageMeta`: `createdAt`,
//   `updatedAt`, `createdBy`, `editedBy`; a device is unknown for pages made before it was
//   recorded), `pageSet` refuses them with the field `propertyId`, and they add no search text.
// - Person (M13 slice 2, ADR-0049): a stored list of account ids (at most 20, no duplicates). `pageSet` refuses a person who is
//   not a member of the space when the value is written (`InvalidInput` for `value`; a personal space: this account alone), a
//   reader keeps an account that left; the filters are `HasValue`, `ContainsPerson` and `ContainsMe` (the latter reads
//   `deps.me`), the search text is the members' names.
// - Formulas (ADR-0055, `./formula`, `./memory-formula`): a `formula` record keeps its stored expression (`config`); no
//   page stores a value. `valuesOf` computes it with the engine over a host that reads the space (stored values, rollups,
//   relations as the rollup reads them (live pages), titles, creation and edit times, file names, member names), with the
//   time of the read (`deps.now`, the device's offset: `deps.localOffsetSeconds`, else the JavaScript date's). Creating a
//   formula or setting its configuration parses and checks the expression against the schema (the property's own id is the
//   `selfId`: a loop is a `cycle`) and refuses it with `InvalidInput` for `config`; the schema reports the derived
//   `resultType`, absent when the formula no longer checks (then no page has a value). `pagesBy` refuses a formula.

import { CoreError, internalError } from "./errors";
import { checkFormula, evaluateFormula, formulaToStored, isFormulaError } from "./formula";
import type { FormulaEnv, FormulaHost, FormulaSchema } from "./formula";
import type { AccountId } from "./generated/AccountId";
import type { AttachmentId } from "./generated/AttachmentId";
import type { Event } from "./generated/Event";
import type { FormulaType } from "./generated/FormulaType";
import type { OptionColor } from "./generated/OptionColor";
import type { OptionId } from "./generated/OptionId";
import type { PageId } from "./generated/PageId";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyFilter } from "./generated/PropertyFilter";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyOption } from "./generated/PropertyOption";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { StatusGroup } from "./generated/StatusGroup";
import { canonicalId, isIsoDate } from "./page-doc";
import {
  DEFAULT_STATUS_OPTIONS,
  MAX_FILTER_CHARS,
  MAX_NAME_CHARS,
  MAX_OPTIONS,
  MAX_PROPERTIES,
  OPTION_COLORS,
  PROPERTY_KINDS,
  STATUS_GROUPS,
  TAGS_NAME,
  TAGS_PROPERTY_ID,
  COMPUTED_PROPERTY_MESSAGE,
  codePointLength,
  copyPropertyValue,
  isComputedPropertyKind,
  isEmptyPropertyValue,
  normalisePropertyValue,
  checklistProgress,
  propertyKindHasOptions,
  propertyValueFiles,
  propertyValueKind,
  propertyValueOptions,
  propertyValueProblem,
  propertyValueText,
  propertyValuesEqual,
} from "./properties";
import type { SeedOption, SeedProperty, SeedValue } from "./properties";
import { FORMULA_TITLE_LABEL, formulaRefusal, formulaSchemaOf } from "./memory-formula";
import { ROLLUP_FUNCTIONS, ROLLUP_TITLE_TARGET, evaluateRollup, isRollupTargetKind, rollupFunctionAccepts } from "./rollup";
import { placeholderTakes } from "./structure";
import type { MemoryStructure, PageMeta, SeedPage, SeedSpace, SpaceState } from "./structure";
import { displayName, invalid } from "./validate";

/** What the properties mirror needs from its owner. */
export interface PropertiesDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Id generator (UUID text), for properties and options. */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** The page tree: which spaces and pages exist, titles, trash state, `updatedAt`. */
  structure: MemoryStructure;
  /** The file name of an attachment (its manifest's), `undefined` when there is none; default: none. */
  attachmentName?: (attachmentId: AttachmentId) => string | undefined;
  /**
   * The attachments whose references may have changed (a `files` value was set, cleared or hidden):
   * called before the event of the change, as the core runs its bookkeeping before it announces it.
   */
  attachmentsChanged?: (attachmentIds: readonly AttachmentId[]) => void;
  /**
   * The people a Person value may list in a space (M13 slice 2, ADR-0049): its current members, in a personal space this
   * account alone. `PagePropertySet` refuses a value that lists anyone else (`InvalidInput` for `value`). Without it every
   * account is accepted.
   */
  people?: (spaceId: SpaceId) => readonly AccountId[];
  /** This account (the one `ContainsMe` stands for); without it the filter matches nothing. */
  me?: () => AccountId | undefined;
  /** The current name of a member of a space; `undefined` for a former member (no text of theirs is indexed). */
  personName?: (spaceId: SpaceId, accountId: AccountId) => string | undefined;
  /** The account a seeded name stands for in a space (a member, this account, or a made-up person). */
  account?: (spaceId: SpaceId, name: string) => AccountId;
  /**
   * How far the device's wall clock is ahead of UTC, in seconds (east positive, daylight saving included), at the time
   * given (Unix ms): what the local day of a formula's `today()` and of the creation time it reads is made with
   * (`Clock::local_offset_seconds`). Default: the JavaScript date's offset.
   */
  localOffsetSeconds?: (atMs: number) => number;
}

/** A space the seed created, with its pages in creation order (what `MemoryProperties.seed` takes). */
export interface SeededSpace {
  spaceId: SpaceId;
  seed: SeedSpace;
  pages: { pageId: PageId; seed: SeedPage }[];
}

interface PropertyRecord {
  id: PropertyId;
  name: string;
  kind: PropertyKind;
  createdAt: number;
  /** In display order; empty for the kinds without a list. */
  options: PropertyOption[];
  /** A relation's pairing (always present for one) or a rollup's configuration. */
  config?: PropertyConfig;
}

interface SpaceProps {
  /** Tags once written; `undefined` while it is only listed. */
  tags: PropertyRecord | undefined;
  /** Every other property, in schema order. */
  others: PropertyRecord[];
  /** Stored values by page, then property: what was written, including what is no longer read. */
  values: Map<PageId, Map<PropertyId, PropertyValue>>;
  /** The schema as the formula engine sees it, built on first use and dropped by every schema change. */
  formulaSchema?: FormulaSchema | undefined;
  /** The result type of each formula that checks (`undefined`: it does not), same life as `formulaSchema`. */
  formulaTypes?: Map<PropertyId, FormulaType | undefined> | undefined;
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** Tags as it is listed before anything writes it. */
function virtualTags(): PropertyRecord {
  return { id: TAGS_PROPERTY_ID, name: TAGS_NAME, kind: "multi_select", createdAt: 0, options: [] };
}

/** `null` (from an untyped caller) reads as "left out", like the core's `Option`. */
function optional<T>(value: T | null | undefined): T | undefined {
  return value ?? undefined;
}

function oneOf<T extends string>(field: string, all: readonly T[], raw: unknown): T {
  if (typeof raw !== "string" || !all.includes(raw as T)) {
    throw invalid(field, `must be one of: ${all.join(", ")}`);
  }
  return raw as T;
}

/** A copy of an option as the API shows it (`group` only when set). */
function optionOf(option: PropertyOption): PropertyOption {
  const copy: PropertyOption = { id: option.id, name: option.name, color: option.color };
  if (option.group !== undefined) {
    copy.group = option.group;
  }
  return copy;
}

/** A copy of a configuration as the API shows it. */
function configOf(config: PropertyConfig): PropertyConfig {
  if (config.kind === "relation") {
    return config.pairedPropertyId === undefined
      ? { kind: "relation", twoWay: config.twoWay }
      : { kind: "relation", twoWay: config.twoWay, pairedPropertyId: config.pairedPropertyId };
  }
  if (config.kind === "formula") {
    return config.resultType === undefined
      ? { kind: "formula", expression: config.expression }
      : {
          kind: "formula",
          expression: config.expression,
          resultType: { base: config.resultType.base, list: config.resultType.list },
        };
  }
  return {
    kind: "rollup",
    relationPropertyId: config.relationPropertyId,
    target: config.target,
    function: config.function,
  };
}

function oneWay(): PropertyConfig {
  return { kind: "relation", twoWay: false };
}

/** The record as the API shows it, with the configuration given (settled by the caller). */
function defOf(record: PropertyRecord, config: PropertyConfig | undefined = record.config): PropertyDef {
  const def: PropertyDef = {
    id: record.id,
    name: record.name,
    kind: record.kind,
    options: record.options.map(optionOf),
    createdAt: record.createdAt,
  };
  if (config !== undefined) {
    def.config = configOf(config);
  }
  return def;
}

/** `str::chars().take(n)`-style cut by code points. */
function takeChars(text: string, count: number): string {
  return Array.from(text).slice(0, count).join("");
}

/** The name of the back property of a new two-way relation: "<name> (back)", the name cut short to fit. */
function backPropertyName(name: string): string {
  const suffix = " (back)";
  const room = Math.max(0, MAX_NAME_CHARS - suffix.length);
  return `${takeChars(name, room).trimEnd()}${suffix}`;
}

/** Compares by code point (what UTF-8 byte order is), not by UTF-16 unit. */
function compareCodePoints(a: string, b: string): number {
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const p = a.codePointAt(i) ?? 0;
    const q = b.codePointAt(j) ?? 0;
    if (p !== q) {
      return p < q ? -1 : 1;
    }
    i += p > 0xffff ? 2 : 1;
    j += q > 0xffff ? 2 : 1;
  }
  if (i < a.length) {
    return 1;
  }
  return j < b.length ? -1 : 0;
}

/** Folds only `A`-`Z` (SQLite's `NOCASE`). */
function asciiLower(text: string): string {
  return text.replace(/[A-Z]/gu, (char) => char.toLowerCase());
}

/** The order of the pages of a query: by title (ASCII case-insensitive, then exact), then page id. */
function comparePages(a: PageMeta, b: PageMeta): number {
  return (
    compareCodePoints(asciiLower(a.title), asciiLower(b.title)) ||
    compareCodePoints(a.title, b.title) ||
    compareCodePoints(a.id, b.id)
  );
}

/**
 * A stored value as the property reads it: `undefined` unless it is of the property's kind and
 * follows its rules (an empty one is no value); options the property does not list are dropped
 * (`kb-model`'s `read_value`). Returns a copy.
 */
function readValue(entry: PropertyRecord, stored: PropertyValue | undefined): PropertyValue | undefined {
  if (
    stored === undefined ||
    stored.kind !== entry.kind ||
    propertyValueProblem(stored) !== undefined ||
    isEmptyPropertyValue(stored)
  ) {
    return undefined;
  }
  const listed = (id: OptionId): boolean => entry.options.some((option) => option.id === id);
  switch (stored.kind) {
    case "select":
      return listed(stored.value) ? { kind: "select", value: stored.value } : undefined;
    case "status":
      return listed(stored.value) ? { kind: "status", value: stored.value } : undefined;
    case "multi_select": {
      const known = stored.value.filter(listed);
      return known.length > 0 ? { kind: "multi_select", value: known } : undefined;
    }
    default:
      return copyPropertyValue(stored);
  }
}

/**
 * The value of a computed property (`kb-model`'s `computed_value`): read from the page itself.
 * `undefined` when the page did not record the device.
 */
function computedValue(kind: PropertyKind, page: PageMeta): PropertyValue | undefined {
  switch (kind) {
    case "created_time":
      return { kind: "created_time", value: page.createdAt };
    case "last_edited_time":
      return { kind: "last_edited_time", value: page.updatedAt };
    case "created_by":
      return page.createdBy === undefined ? undefined : { kind: "created_by", value: page.createdBy };
    case "last_edited_by":
      return page.editedBy === undefined ? undefined : { kind: "last_edited_by", value: page.editedBy };
    default:
      return undefined;
  }
}

/** The page's value as a filter sees it. */
type Matcher = (value: PropertyValue | undefined) => boolean;

/**
 * The properties of every space: schema, values and the queries over them. Methods throw
 * `CoreError`s where the core answers with an `ApiError` (`NotFound` for unknown spaces, pages,
 * properties and options; `InvalidInput` with `details.field` for everything else) and emit
 * events in the core's order, after the change is applied.
 */
export class MemoryProperties {
  readonly #deps: PropertiesDeps;
  readonly #spaces = new Map<SpaceId, SpaceProps>();
  /** Page ids by title, while seeding a space. */
  #seedTitles = new Map<string, PageId>();

  constructor(deps: PropertiesDeps) {
    this.#deps = deps;
  }

  // ----- schema -----------------------------------------------------------------------

  /** `PropertySchemaGet`: Tags first, then the others in order. */
  schemaGet(spaceId: SpaceId): PropertyDef[] {
    const space = this.#space(spaceId);
    return this.#schema(space).map((record) => this.#def(space, record));
  }

  /**
   * `PropertyCreate`: appended last; a status property starts with the three default options. A
   * relation takes a configuration (a two-way one makes the paired property too, in the same
   * change; the answer is the forward property) and a rollup requires one.
   */
  create(
    spaceId: SpaceId,
    rawName: string,
    rawKind: PropertyKind,
    rawConfig?: PropertyConfig | null,
  ): PropertyDef {
    const kind = oneOf("kind", PROPERTY_KINDS, rawKind);
    const name = displayName(rawName, MAX_NAME_CHARS, "name");
    const space = this.#space(spaceId);
    const config = optional(rawConfig);
    if (kind === "relation" && config?.kind === "relation" && config.twoWay && config.pairedPropertyId === undefined) {
      return this.#createPair(spaceId, space, name);
    }
    // A formula is checked as the new property itself (a formula that reads it would be a loop), so its id comes
    // first; the other kinds draw theirs after the checks, as they always did.
    const formulaId = kind === "formula" ? this.#deps.newId() : undefined;
    const stored = this.#newConfig(space, kind, config, formulaId);
    // Tags counts, written or not.
    if (space.others.length + 1 >= MAX_PROPERTIES) {
      throw invalid("spaceId", `a space holds at most ${String(MAX_PROPERTIES)} properties`);
    }
    const record: PropertyRecord = {
      id: formulaId ?? this.#deps.newId(),
      name,
      kind,
      createdAt: this.#deps.now(),
      options: [],
    };
    if (stored !== undefined) {
      record.config = stored;
    }
    if (kind === "status") {
      record.options = this.#defaultStatusOptions();
    }
    space.others.push(record);
    this.#changed(spaceId, true);
    return this.#def(space, record);
  }

  /** The two properties of a new two-way relation, in one change. */
  #createPair(spaceId: SpaceId, space: SpaceProps, name: string): PropertyDef {
    // Tags counts, written or not; both properties need room.
    if (space.others.length + 1 + 2 > MAX_PROPERTIES) {
      throw invalid("spaceId", `a space holds at most ${String(MAX_PROPERTIES)} properties`);
    }
    const now = this.#deps.now();
    const forward: PropertyRecord = { id: this.#deps.newId(), name, kind: "relation", createdAt: now, options: [] };
    const back: PropertyRecord = {
      id: this.#deps.newId(),
      name: backPropertyName(name),
      kind: "relation",
      createdAt: now,
      options: [],
    };
    forward.config = { kind: "relation", twoWay: true, pairedPropertyId: back.id };
    back.config = { kind: "relation", twoWay: true, pairedPropertyId: forward.id };
    space.others.push(forward, back);
    this.#changed(spaceId, true);
    return this.#def(space, forward);
  }

  /**
   * What the configuration given at creation stores for a property of `kind` (or the refusal); `selfId` is the id a new
   * formula will have.
   */
  #newConfig(
    space: SpaceProps,
    kind: PropertyKind,
    config: PropertyConfig | undefined,
    selfId: PropertyId | undefined,
  ): PropertyConfig | undefined {
    if (kind === "rollup") {
      if (config === undefined) {
        throw invalid("config", "a rollup needs a configuration");
      }
      this.#checkRollup(space, config);
      return configOf(config);
    }
    if (kind === "formula") {
      if (config === undefined) {
        throw invalid("config", "a formula needs a configuration");
      }
      this.#checkFormula(space, selfId, config);
      return { kind: "formula", expression: config.kind === "formula" ? config.expression : "" };
    }
    if (kind === "relation") {
      if (config === undefined) {
        return oneWay();
      }
      if (config.kind !== "relation") {
        throw invalid("config", "the configuration does not match the kind");
      }
      if (!config.twoWay && config.pairedPropertyId === undefined) {
        return oneWay();
      }
      throw invalid("config", "a new two-way relation makes its own paired property");
    }
    if (config !== undefined) {
      throw invalid("config", "only a relation, a rollup or a formula takes a configuration");
    }
    return undefined;
  }

  /**
   * `PropertyConfigSet`: the configuration of a rollup or a formula (checked against the schema; a formula's
   * expression must parse, check and not depend on itself, and its `resultType` is ignored). A relation's is fixed at
   * creation.
   */
  setConfig(spaceId: SpaceId, propertyId: PropertyId, config: PropertyConfig): PropertyDef {
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    if (entry.kind === "rollup") {
      this.#checkRollup(space, config);
      entry.config = configOf(config);
    } else if (entry.kind === "formula") {
      this.#checkFormula(space, propertyId, config);
      if (config.kind === "formula" && !(entry.config?.kind === "formula" && entry.config.expression === config.expression)) {
        entry.config = { kind: "formula", expression: config.expression };
      }
    } else if (entry.kind === "relation") {
      throw invalid("config", "the configuration of a relation cannot be changed");
    } else {
      throw invalid("config", "only a relation, a rollup or a formula takes a configuration");
    }
    this.#changed(spaceId, true);
    return this.#def(space, entry);
  }

  /**
   * Creates a formula property whose expression is stored as given, without checking it: for the import of a file whose
   * expression does not resolve in this space, and a move that carries a formula whose inputs have no counterpart
   * (`kb-model`'s `create_formula_unchecked`). It has no result type and no value until a configuration that checks is
   * set. The name and the size of the schema are checked as for any property.
   */
  createFormulaUnchecked(spaceId: SpaceId, rawName: string, expression: string): PropertyDef {
    const name = displayName(rawName, MAX_NAME_CHARS, "name");
    const space = this.#space(spaceId);
    if (space.others.length + 1 >= MAX_PROPERTIES) {
      throw invalid("spaceId", `a space holds at most ${String(MAX_PROPERTIES)} properties`);
    }
    const record: PropertyRecord = {
      id: this.#deps.newId(),
      name,
      kind: "formula",
      createdAt: this.#deps.now(),
      options: [],
      config: { kind: "formula", expression },
    };
    space.others.push(record);
    this.#changed(spaceId, true);
    return this.#def(space, record);
  }

  /** `PropertyRename`: Tags can be renamed too (and is written by it). */
  rename(spaceId: SpaceId, propertyId: PropertyId, rawName: string): PropertyDef {
    const name = displayName(rawName, MAX_NAME_CHARS, "name");
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    const target = propertyId === TAGS_PROPERTY_ID ? this.#storedTags(space) : entry;
    target.name = name;
    this.#changed(spaceId, true);
    return this.#def(space, target);
  }

  /**
   * `PropertyDelete`: the schema entry goes; the values stay stored, unread, until the next
   * write of a page's values removes them. Tags cannot be deleted.
   */
  delete(spaceId: SpaceId, propertyId: PropertyId): void {
    const space = this.#space(spaceId);
    if (propertyId === TAGS_PROPERTY_ID) {
      throw invalid("propertyId", "the Tags property cannot be deleted");
    }
    const at = space.others.findIndex((record) => record.id === propertyId);
    if (at < 0) {
      throw notFound("property not found");
    }
    const released = this.#filesOf(space, propertyId);
    // The other half of a two-way relation becomes a one-way relation; it keeps its links.
    const partner = this.#partner(space, space.others[at] as PropertyRecord);
    if (partner !== undefined) {
      partner.config = oneWay();
    }
    space.others.splice(at, 1);
    // Its values are hidden at once, so the files they listed lose those references now.
    this.#deps.attachmentsChanged?.(released);
    this.#changed(spaceId, true);
  }

  /**
   * `PropertyMove`: in front of `before` (`undefined` = last; Tags = first of the others).
   * Tags itself is not movable. A move that changes nothing is accepted (and announced).
   */
  move(spaceId: SpaceId, propertyId: PropertyId, rawBefore: PropertyId | null | undefined): void {
    const before = optional(rawBefore);
    const space = this.#space(spaceId);
    if (propertyId === TAGS_PROPERTY_ID) {
      throw invalid("propertyId", "the Tags property always comes first");
    }
    const moving = space.others.find((record) => record.id === propertyId);
    if (moving === undefined) {
      throw notFound("property not found");
    }
    if (before !== undefined && this.#entry(space, before) === undefined) {
      throw notFound("property not found");
    }
    if (before !== propertyId) {
      const rest = space.others.filter((record) => record !== moving);
      let at = rest.length;
      if (before === TAGS_PROPERTY_ID) {
        at = 0;
      } else if (before !== undefined) {
        at = rest.findIndex((record) => record.id === before);
      }
      rest.splice(at, 0, moving);
      space.others = rest;
    }
    this.#changed(spaceId, true);
  }

  /** `PropertyOptionAdd`: appended last; Tags is written on first need. */
  optionAdd(
    spaceId: SpaceId,
    propertyId: PropertyId,
    rawName: string,
    rawColor: OptionColor,
    rawGroup: StatusGroup | null | undefined,
  ): PropertyOption {
    const name = displayName(rawName, MAX_NAME_CHARS, "name");
    const color = oneOf("color", OPTION_COLORS, rawColor);
    const group = optional(rawGroup);
    if (group !== undefined) {
      oneOf("group", STATUS_GROUPS, group);
    }
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    const optionGroup = this.#checkNewOption(entry, group);
    // Nothing was refused: now Tags may be written.
    const target = propertyId === TAGS_PROPERTY_ID ? this.#storedTags(space) : entry;
    const option = this.#pushOption(target, name, color, optionGroup, undefined);
    this.#changed(spaceId, true);
    return optionOf(option);
  }

  /** `PropertyOptionUpdate`: what is left out stays; the group is for status options only. */
  optionUpdate(
    spaceId: SpaceId,
    propertyId: PropertyId,
    optionId: OptionId,
    patch: { name?: string | null; color?: OptionColor | null; group?: StatusGroup | null },
  ): PropertyOption {
    const name = patch.name === undefined || patch.name === null ? undefined : displayName(patch.name, MAX_NAME_CHARS, "name");
    const rawColor = optional(patch.color);
    const color = rawColor === undefined ? undefined : oneOf("color", OPTION_COLORS, rawColor);
    const rawGroup = optional(patch.group);
    const group = rawGroup === undefined ? undefined : oneOf("group", STATUS_GROUPS, rawGroup);
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    const option = entry.options.find((candidate) => candidate.id === optionId);
    if (option === undefined) {
      throw notFound("option not found");
    }
    if (group !== undefined && entry.kind !== "status") {
      throw invalid("group", "only status options have a group");
    }
    if (name !== undefined) {
      option.name = name;
    }
    if (color !== undefined) {
      option.color = color;
    }
    if (group !== undefined) {
      option.group = group;
    }
    this.#changed(spaceId, true);
    return optionOf(option);
  }

  /**
   * `PropertyOptionDelete`: takes the option out of every page's value of the property in the
   * space, in one change (a select or status value that named it is cleared, a multi-select value
   * loses it and is cleared when it was the last); those pages' `updatedAt` becomes now.
   */
  optionDelete(spaceId: SpaceId, propertyId: PropertyId, optionId: OptionId): void {
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    const at = entry.options.findIndex((candidate) => candidate.id === optionId);
    if (at < 0) {
      throw notFound("option not found");
    }
    entry.options.splice(at, 1);
    const existing = new Set(this.#deps.structure.spacePageIds(spaceId));
    const touched: PageId[] = [];
    for (const [pageId, map] of space.values) {
      // A purged page's values are not part of the document any more: left alone.
      const stored = existing.has(pageId) ? map.get(propertyId) : undefined;
      if (stored === undefined) {
        continue;
      }
      if ((stored.kind === "select" || stored.kind === "status") && stored.value === optionId) {
        map.delete(propertyId);
        touched.push(pageId);
      } else if (stored.kind === "multi_select" && stored.value.includes(optionId)) {
        const kept = stored.value.filter((id) => id !== optionId);
        if (kept.length > 0) {
          map.set(propertyId, { kind: "multi_select", value: kept });
        } else {
          map.delete(propertyId);
        }
        touched.push(pageId);
      }
      if (map.size === 0) {
        space.values.delete(pageId);
      }
    }
    if (touched.length > 0) {
      const now = this.#deps.now();
      for (const pageId of touched) {
        this.#deps.structure.touchPage(pageId, now);
      }
    }
    this.#changed(spaceId, true);
  }

  // ----- values -----------------------------------------------------------------------

  /**
   * `PagePropertySet`: sets or (with `null`, or an empty text, URL or list) clears a page's value,
   * live or trashed; answers the page's values after the change. The same value again changes
   * nothing (no `updatedAt`); a real change sets the page's `updatedAt`, and removes the values
   * of deleted properties from that page. `StructureChanged` follows either way.
   */
  pageSet(pageId: PageId, propertyId: PropertyId, value: PropertyValue | null | undefined): [PropertyId, PropertyValue][] {
    const page = this.#page(pageId);
    const space = this.#space(page.spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    if (isComputedPropertyKind(entry.kind)) {
      throw invalid("propertyId", COMPUTED_PROPERTY_MESSAGE);
    }
    if (entry.kind === "relation") {
      return this.#setRelation(page, space, entry, value);
    }
    const stored = this.#checkedValue(page.spaceId, entry, value);
    // A value written (or cleared) by hand replaces the template placeholder of the property (ADR-0056), same value or not.
    this.#deps.structure.templatePlaceholderDrop(pageId, propertyId);
    const map = space.values.get(pageId);
    const current = map?.get(propertyId);
    const same = current === undefined ? stored === null : stored !== null && propertyValuesEqual(current, stored);
    // The files the page's value listed before and lists after: their references may change.
    const touched: AttachmentId[] = same
      ? []
      : [...(current === undefined ? [] : propertyValueFiles(current)), ...(stored === null ? [] : propertyValueFiles(stored))];
    if (!same) {
      const target = map ?? new Map<PropertyId, PropertyValue>();
      if (stored === null) {
        target.delete(propertyId);
      } else {
        target.set(propertyId, copyPropertyValue(stored));
      }
      // Lazy clean-up: values of deleted properties go with the next write.
      for (const id of [...target.keys()]) {
        if (this.#entry(space, id) === undefined) {
          target.delete(id);
        }
      }
      if (target.size > 0) {
        space.values.set(pageId, target);
      } else {
        space.values.delete(pageId);
      }
      this.#deps.structure.touchPage(pageId, this.#deps.now());
    }
    if (touched.length > 0) {
      this.#deps.attachmentsChanged?.(touched);
    }
    this.#changed(page.spaceId, false);
    return this.valuesOf(page.spaceId, pageId);
  }

  /**
   * Whether `pageSet` would accept `value` (`null` or `undefined`: a clear) for `propertyId` on a page of the space, checked
   * without a page and writing nothing: `NotFound` for an unknown property, `InvalidInput` for `propertyId` (computed) or
   * `value` (another kind, refused by its own rules, an unknown option, a person who is not a member). The bulk edit
   * (ADR-0058) checks every page's value with it before it writes any.
   */
  checkValue(spaceId: SpaceId, propertyId: PropertyId, value: PropertyValue | null | undefined): void {
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    if (isComputedPropertyKind(entry.kind)) {
      throw invalid("propertyId", COMPUTED_PROPERTY_MESSAGE);
    }
    this.#checkedValue(spaceId, entry, value);
  }

  /**
   * The value as it is stored (`null`: nothing, for a clear and for an empty text, URL or list) after the checks of
   * `PagePropertySet` on a property that is not computed: the kind, the value's own rules, the options, the people.
   */
  #checkedValue(spaceId: SpaceId, entry: PropertyRecord, value: PropertyValue | null | undefined): PropertyValue | null {
    if (value === null || value === undefined) {
      return null;
    }
    if (propertyValueKind(value) !== entry.kind) {
      throw invalid("value", "the value does not match the kind of the property");
    }
    const stored = normalisePropertyValue(value);
    if (stored !== null && !propertyValueOptions(stored).every((id) => entry.options.some((option) => option.id === id))) {
      throw invalid("value", "the value names an unknown option");
    }
    // The people of a Person value are members of the space when it is written (ADR-0049); a reader keeps one who left.
    if (stored?.kind === "person") {
      const members = this.#deps.people?.(spaceId);
      if (members !== undefined && !stored.value.every((id) => members.includes(id))) {
        throw invalid("value", "a person is not a member of the space");
      }
    }
    return stored;
  }

  /**
   * `PagePropertySet` for a relation: writes the value and, for a two-way relation, the paired value
   * of every page it adds or removes, in one change. Pages that were already on one side only (a
   * merge can leave that) are removed or kept by what the reader sees, not by what is stored.
   */
  #setRelation(
    page: PageMeta,
    space: SpaceProps,
    entry: PropertyRecord,
    value: PropertyValue | null | undefined,
  ): [PropertyId, PropertyValue][] {
    let wanted: PageId[] = [];
    if (value !== null && value !== undefined) {
      if (propertyValueKind(value) !== "relation") {
        throw invalid("value", "the value does not match the kind of the property");
      }
      const stored = normalisePropertyValue(value);
      wanted = stored?.kind === "relation" ? [...stored.value] : [];
    }
    const spaceId = page.spaceId;
    const current = this.#effective(space, spaceId, entry, page.id);
    const added = wanted.filter((id) => this.#exists(spaceId, id) && !current.includes(id));
    const removed = current.filter((id) => !wanted.includes(id));
    const stored: PropertyValue | null = wanted.length > 0 ? { kind: "relation", value: wanted } : null;
    const map = space.values.get(page.id);
    const before = map?.get(entry.id);
    const ownChange = before === undefined ? stored !== null : stored === null || !propertyValuesEqual(before, stored);

    // The paired value of each affected page after the change: add or remove this page.
    const others: { id: PageId; next: PageId[] }[] = [];
    const partner = this.#partner(space, entry);
    if (partner !== undefined) {
      const changes: [PageId, boolean][] = [...added.map((id): [PageId, boolean] => [id, true]), ...removed.map((id): [PageId, boolean] => [id, false])];
      for (const [other, add] of changes) {
        if (!this.#exists(spaceId, other)) {
          continue;
        }
        const raw = space.values.get(other)?.get(partner.id);
        const listed = raw?.kind === "relation" && propertyValueProblem(raw) === undefined ? [...raw.value] : [];
        let next = listed;
        if (add && !listed.includes(page.id)) {
          next = [...listed, page.id];
        } else if (!add) {
          next = listed.filter((id) => id !== page.id);
        }
        if (next.length === listed.length && next.every((id, at) => id === listed[at])) {
          continue;
        }
        others.push({ id: other, next });
      }
    }

    if (ownChange || others.length > 0) {
      const now = this.#deps.now();
      if (ownChange) {
        const target = map ?? new Map<PropertyId, PropertyValue>();
        if (stored === null) {
          target.delete(entry.id);
        } else {
          target.set(entry.id, copyPropertyValue(stored));
        }
        // Lazy clean-up: values of deleted properties go with the next write.
        for (const id of [...target.keys()]) {
          if (this.#entry(space, id) === undefined) {
            target.delete(id);
          }
        }
        if (target.size > 0) {
          space.values.set(page.id, target);
        } else {
          space.values.delete(page.id);
        }
        this.#deps.structure.touchPage(page.id, now);
      }
      if (partner !== undefined) {
        for (const { id, next } of others) {
          const target = space.values.get(id) ?? new Map<PropertyId, PropertyValue>();
          if (next.length > 0) {
            target.set(partner.id, { kind: "relation", value: next });
          } else {
            target.delete(partner.id);
          }
          if (target.size > 0) {
            space.values.set(id, target);
          } else {
            space.values.delete(id);
          }
          this.#deps.structure.touchPage(id, now);
        }
      }
    }
    this.#changed(spaceId, false);
    return this.valuesOf(spaceId, page.id);
  }

  /**
   * The properties of a space in schema order (Tags first) with their kinds; none for a space this knows nothing about.
   * Never throws: what `PageInfo.templatePlaceholders` lists its entries by (ADR-0056).
   */
  kindsOf(spaceId: SpaceId): { id: PropertyId; kind: PropertyKind }[] {
    const space = this.#spaces.get(spaceId);
    return space === undefined ? [] : this.#schema(space).map(({ id, kind }) => ({ id, kind }));
  }

  /**
   * What a page stores for a property, as stored (a copy; the property's kind and options are not checked), `undefined` when
   * it has nothing (ADR-0056: the value that setting a placeholder removes). Never throws.
   */
  storedValue(spaceId: SpaceId, pageId: PageId, propertyId: PropertyId): PropertyValue | undefined {
    const stored = this.#spaces.get(spaceId)?.values.get(pageId)?.get(propertyId);
    return stored === undefined ? undefined : copyPropertyValue(stored);
  }

  /**
   * Stores a value for a page's property, or removes it (`undefined`): no event, no stamp, no checks (what an undo or redo
   * of a placeholder does, ADR-0056). A page that is not in the space is left alone.
   */
  restoreValue(spaceId: SpaceId, pageId: PageId, propertyId: PropertyId, value: PropertyValue | undefined): void {
    const space = this.#spaces.get(spaceId);
    if (space === undefined || !this.#exists(spaceId, pageId)) {
      return;
    }
    const map = space.values.get(pageId) ?? new Map<PropertyId, PropertyValue>();
    if (value === undefined) {
      map.delete(propertyId);
    } else {
      map.set(propertyId, copyPropertyValue(value));
    }
    if (map.size > 0) {
      space.values.set(pageId, map);
    } else {
      space.values.delete(pageId);
    }
  }

  /** `PagePropertiesGet`: the page's readable values in schema order. */
  pageGet(pageId: PageId): [PropertyId, PropertyValue][] {
    const page = this.#page(pageId);
    return this.valuesOf(page.spaceId, pageId);
  }

  /**
   * The readable values of a page in schema order (Tags first); what `PageInfo.properties` lists.
   * Never throws: a page or space this knows nothing about has none.
   */
  valuesOf(spaceId: SpaceId, pageId: PageId): [PropertyId, PropertyValue][] {
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      return [];
    }
    // One time for the whole read, taken (and the clock read) only when a formula needs it.
    let env: FormulaEnv | undefined;
    const timeOfRead = (): FormulaEnv => (env ??= this.#env());
    const found: [PropertyId, PropertyValue][] = [];
    for (const entry of this.#schema(space)) {
      const value = this.#value(space, spaceId, entry, pageId, timeOfRead);
      if (value !== undefined) {
        found.push([entry.id, value]);
      }
    }
    return found;
  }

  /** One property of a page as it reads: stored, computed from the page, or from its relations. */
  #value(
    space: SpaceProps,
    spaceId: SpaceId,
    entry: PropertyRecord,
    pageId: PageId,
    timeOfRead: () => FormulaEnv = () => this.#env(),
  ): PropertyValue | undefined {
    switch (entry.kind) {
      case "rollup":
        return this.#rollupValue(space, spaceId, entry, pageId);
      case "formula":
        return this.#formulaValue(space, spaceId, entry, pageId, timeOfRead);
      case "relation": {
        const pages = this.#effective(space, spaceId, entry, pageId);
        return pages.length > 0 ? { kind: "relation", value: pages } : undefined;
      }
      default: {
        if (isComputedPropertyKind(entry.kind)) {
          const page = this.#deps.structure.pageMeta(pageId);
          return page === undefined ? undefined : computedValue(entry.kind, page);
        }
        return readValue(entry, space.values.get(pageId)?.get(entry.id));
      }
    }
  }

  // ----- queries ----------------------------------------------------------------------

  /**
   * `PagesByProperty`: the live pages of the space whose value of the property matches `filter`,
   * ordered by title (ASCII-only case folding, then the exact title, then the page id, all by code
   * point). The property must exist first; then the filter is checked against its kind.
   */
  pagesBy(spaceId: SpaceId, propertyId: PropertyId, filter: PropertyFilter): PageId[] {
    const space = this.#space(spaceId);
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw notFound("property not found");
    }
    if (entry.kind === "formula") {
      // Computed on every read, nothing indexed: the views filter a formula in the client like a rollup (ADR-0055).
      throw invalid("filter", "a formula cannot be searched; its pages are filtered in the views");
    }
    const matches = this.#matcher(entry, filter);
    if (matches === undefined) {
      return [];
    }
    return this.#deps.structure
      .livePages(spaceId)
      .filter((page) => matches(this.#value(space, spaceId, entry, page.id)))
      .sort(comparePages)
      .map((page) => page.id);
  }

  /**
   * What the text index holds for the properties of a page: one text per property that has some,
   * in schema order (option names, or a text, URL, email or phone value). Number, checkbox and
   * date values add nothing.
   */
  searchTexts(spaceId: SpaceId, pageId: PageId): string[] {
    const space = this.#spaces.get(spaceId);
    const map = space?.values.get(pageId);
    if (space === undefined || map === undefined || map.size === 0) {
      return [];
    }
    const texts: string[] = [];
    for (const entry of this.#schema(space)) {
      const value = isComputedPropertyKind(entry.kind) ? undefined : readValue(entry, map.get(entry.id));
      if (value === undefined) {
        continue;
      }
      const text = propertyValueText(
        value,
        (id) => entry.options.find((option) => option.id === id)?.name,
        (id) => this.#deps.attachmentName?.(id),
        (id) => (this.#exists(spaceId, id) ? this.#deps.structure.pageMeta(id)?.title : undefined),
        (id) => this.#deps.personName?.(spaceId, id),
      );
      if (text !== undefined) {
        texts.push(text);
      }
    }
    return texts;
  }

  /**
   * How many `files` values list each attachment: one per page and property, over the pages that
   * exist (live and trashed; a purged page's values are not part of the document any more) and the
   * properties the schema has. What the core's `attachment_reference_count` adds to the blocks.
   */
  attachmentReferenceCounts(): Map<AttachmentId, number> {
    const counts = new Map<AttachmentId, number>();
    for (const space of this.#spaces.values()) {
      const schema = this.#schema(space);
      for (const [pageId, map] of space.values) {
        if (this.#deps.structure.pageMeta(pageId) === undefined) {
          continue;
        }
        for (const entry of schema) {
          if (entry.kind !== "files") {
            continue;
          }
          const value = readValue(entry, map.get(entry.id));
          for (const id of value === undefined ? [] : propertyValueFiles(value)) {
            counts.set(id, (counts.get(id) ?? 0) + 1);
          }
        }
      }
    }
    return counts;
  }

  /** The pages whose readable `files` values list each attachment: `attachmentReferenceCounts` by page. */
  attachmentReferrers(): Map<AttachmentId, Set<PageId>> {
    const found = new Map<AttachmentId, Set<PageId>>();
    for (const space of this.#spaces.values()) {
      const schema = this.#schema(space);
      for (const [pageId, map] of space.values) {
        if (this.#deps.structure.pageMeta(pageId) === undefined) {
          continue;
        }
        for (const entry of schema) {
          const value = entry.kind === "files" ? readValue(entry, map.get(entry.id)) : undefined;
          for (const id of value === undefined ? [] : propertyValueFiles(value)) {
            const pages = found.get(id) ?? new Set<PageId>();
            pages.add(pageId);
            found.set(id, pages);
          }
        }
      }
    }
    return found;
  }

  // ----- moving pages to another space (ADR-0038) ---------------------------------------

  /**
   * What a move reads of the pages that leave a space: each page's stored values that are readable
   * and not computed, as `[property id, value]` in schema order (a relation lists the pages of the space
   * that exist, trashed ones included). Call before the pages leave the space's structure.
   */
  movingValues(spaceId: SpaceId, pageIds: readonly PageId[]): Map<PageId, [PropertyId, PropertyValue][]> {
    const space = this.#space(spaceId);
    const schema = this.#schema(space).filter((entry) => !isComputedPropertyKind(entry.kind));
    const found = new Map<PageId, [PropertyId, PropertyValue][]>();
    for (const pageId of pageIds) {
      const stored = space.values.get(pageId);
      const entries: [PropertyId, PropertyValue][] = [];
      for (const entry of schema) {
        const value =
          entry.kind === "relation"
            ? this.#storedRelationValue(space, spaceId, entry, pageId)
            : readValue(entry, stored?.get(entry.id));
        if (value !== undefined) {
          entries.push([entry.id, value]);
        }
      }
      found.set(pageId, entries);
    }
    return found;
  }

  #storedRelationValue(space: SpaceProps, spaceId: SpaceId, entry: PropertyRecord, pageId: PageId): PropertyValue | undefined {
    const pages = this.#storedRelation(space, spaceId, entry, pageId);
    return pages.length > 0 ? { kind: "relation", value: pages } : undefined;
  }

  /** The pages left the space: their stored values go (the move wrote them into the target's). */
  takeValues(spaceId: SpaceId, pageIds: readonly PageId[]): void {
    const space = this.#space(spaceId);
    for (const pageId of pageIds) {
      space.values.delete(pageId);
    }
  }

  /**
   * Stores a value the move mapped (checked by the move, no event): the page must already be in the
   * space. A relation is stored on this page's side only; the read shows the symmetric union.
   */
  putValue(spaceId: SpaceId, pageId: PageId, propertyId: PropertyId, value: PropertyValue): void {
    const space = this.#space(spaceId);
    const map = space.values.get(pageId) ?? new Map<PropertyId, PropertyValue>();
    map.set(propertyId, copyPropertyValue(value));
    space.values.set(pageId, map);
  }

  /**
   * The relation entries of the pages that stay in a space which list a moved page, counted (and, with
   * `write`, removed: an emptied value is cleared and the page's `updatedAt` becomes now): both sides of a
   * two-way relation count, each stored entry once. Answers the count and the pages written.
   */
  unlinkPages(spaceId: SpaceId, moving: ReadonlySet<PageId>, write: boolean): { removed: number; touched: PageId[] } {
    const space = this.#space(spaceId);
    const relations = new Set(this.#schema(space).filter((entry) => entry.kind === "relation").map((entry) => entry.id));
    let removed = 0;
    const touched: PageId[] = [];
    for (const [pageId, map] of [...space.values]) {
      if (moving.has(pageId)) {
        continue;
      }
      let changed = false;
      for (const [propertyId, value] of [...map]) {
        if (value.kind !== "relation" || !relations.has(propertyId)) {
          continue;
        }
        const kept = value.value.filter((id) => !moving.has(id));
        if (kept.length === value.value.length) {
          continue;
        }
        removed += value.value.length - kept.length;
        changed = true;
        if (write) {
          if (kept.length > 0) {
            map.set(propertyId, { kind: "relation", value: kept });
          } else {
            map.delete(propertyId);
          }
        }
      }
      if (changed && write) {
        if (map.size === 0) {
          space.values.delete(pageId);
        }
        if (this.#deps.structure.pageMeta(pageId) !== undefined) {
          this.#deps.structure.touchPage(pageId, this.#deps.now());
          touched.push(pageId);
        }
      }
    }
    return { removed, touched };
  }

  // ----- space lifecycle, seeding, backups ----------------------------------------------

  /** The space was deleted: its schema and every value go. */
  spaceDeleted(spaceId: SpaceId): void {
    this.#spaces.delete(spaceId);
  }

  /**
   * Creates the schemas and values the seeds describe (`SeedSpace.tagOptions`, `.properties`,
   * `SeedPage.tags`, `.properties`), without events and without touching any page. Meant to run
   * after everything else seeded, so that the ids it draws shift nobody else's; the order of the
   * draws is documented on `SeedSpace.properties`.
   */
  seed(seeded: readonly SeededSpace[]): void {
    for (const { spaceId, seed, pages } of seeded) {
      const space = this.#space(spaceId);
      // Relations are seeded by the titles of the space's pages (the first page with each).
      this.#seedTitles = new Map();
      for (const { pageId, seed: page } of pages) {
        if (!this.#seedTitles.has(page.title)) {
          this.#seedTitles.set(page.title, pageId);
        }
      }
      for (const option of seed.tagOptions ?? []) {
        this.#seedOption(this.#storedTags(space), option);
      }
      for (const def of seed.properties ?? []) {
        this.#seedProperty(space, def);
      }
      space.formulaSchema = undefined;
      space.formulaTypes = undefined;
      for (const { pageId, seed: page } of pages) {
        if (page.tags !== undefined && page.tags.length > 0) {
          const ids = page.tags.map((name) => this.#seedTagOption(space, name));
          this.#seedStore(space, pageId, this.#schemaEntry(space, TAGS_PROPERTY_ID), { kind: "multi_select", value: ids });
        }
        for (const [name, raw] of Object.entries(page.properties ?? {})) {
          const entry = this.#schema(space).find((record) => record.name === name);
          if (entry === undefined) {
            throw internalError(`MemoryTransport seed: the space has no property named "${name}"`);
          }
          this.#seedStore(space, pageId, entry, this.#seedValue(spaceId, space, entry, name, raw));
        }
        // The placeholders of a seeded template page, by property name (ADR-0056): `today` and `now` on a date property,
        // `me` on a person property.
        if (page.templatePlaceholders !== undefined) {
          const { structure } = this.#deps;
          const mapped = new Map(structure.templateOf(pageId)?.values);
          for (const [name, placeholder] of Object.entries(page.templatePlaceholders)) {
            const entry = this.#schema(space).find((record) => record.name === name);
            if (entry === undefined) {
              throw internalError(`MemoryTransport seed: the space has no property named "${name}"`);
            }
            if (!placeholderTakes(placeholder, entry.kind)) {
              throw internalError(`MemoryTransport seed: the ${entry.kind} property "${name}" takes no placeholder "${placeholder}"`);
            }
            mapped.set(entry.id, placeholder);
          }
          structure.templateSet(pageId, structure.templateOf(pageId)?.template ?? false, mapped);
        }
      }
    }
  }

  /**
   * `spaces` (as `MemoryStructure.exportState` made them) with the written schema of each space
   * and the stored values of each of its pages added, for a backup.
   */
  exportState(spaces: SpaceState[]): SpaceState[] {
    return spaces.map((state) => {
      const space = this.#spaces.get(state.id);
      if (space === undefined) {
        return state;
      }
      const out: SpaceState = {
        ...state,
        pages: state.pages.map((page) => {
          const map = space.values.get(page.id);
          if (map === undefined || map.size === 0) {
            return page;
          }
          return {
            ...page,
            props: [...map].map(([id, value]): [PropertyId, PropertyValue] => [id, copyPropertyValue(value)]),
          };
        }),
      };
      // A formula is written as stored: its result type is derived on read.
      const written = [...(space.tags === undefined ? [] : [space.tags]), ...space.others].map((record) =>
        record.kind === "formula" ? defOf(record) : this.#def(space, record),
      );
      if (written.length > 0) {
        out.properties = written;
      }
      return out;
    });
  }

  /**
   * Checks the schemas and values `spaces` carry (a backup's) and returns the function that
   * replaces every schema and value with them, without events. Nothing changes until it is called;
   * damaged records throw first.
   */
  prepareRestore(spaces: readonly SpaceState[]): () => void {
    const restored = new Map<SpaceId, SpaceProps>();
    for (const state of spaces) {
      const props: SpaceProps = { tags: undefined, others: [], values: new Map() };
      const seen = new Set<PropertyId>();
      for (const def of state.properties ?? []) {
        if (seen.has(def.id)) {
          throw internalError("a property appears twice");
        }
        seen.add(def.id);
        const record = this.#restoredRecord(def);
        if (def.id === TAGS_PROPERTY_ID) {
          if (record.kind !== "multi_select") {
            throw internalError("the Tags property is not a multi-select");
          }
          props.tags = record;
        } else {
          props.others.push(record);
        }
      }
      for (const page of state.pages) {
        const map = new Map<PropertyId, PropertyValue>();
        for (const [id, value] of page.props ?? []) {
          if (propertyValueProblem(value) !== undefined) {
            throw internalError("a property value is damaged");
          }
          map.set(id, copyPropertyValue(value));
        }
        if (map.size > 0) {
          props.values.set(page.id, map);
        }
      }
      restored.set(state.id, props);
    }
    return () => {
      this.#spaces.clear();
      for (const [id, props] of restored) {
        this.#spaces.set(id, props);
      }
    };
  }

  // ----- plumbing ---------------------------------------------------------------------

  /**
   * The configuration of a record as the API shows it: a relation whose partner is gone reads as one-way, a formula
   * carries its derived result type (absent when it does not check).
   */
  #settled(space: SpaceProps, record: PropertyRecord): PropertyConfig | undefined {
    if (record.kind === "formula" && record.config?.kind === "formula") {
      const resultType = this.#formulaType(space, record);
      return resultType === undefined
        ? { kind: "formula", expression: record.config.expression }
        : { kind: "formula", expression: record.config.expression, resultType };
    }
    if (record.kind !== "relation") {
      return record.config;
    }
    return this.#partner(space, record) === undefined ? oneWay() : record.config;
  }

  #def(space: SpaceProps, record: PropertyRecord): PropertyDef {
    return defOf(record, this.#settled(space, record));
  }

  /** The paired property of a two-way relation, when the pair points both ways. */
  #partner(space: SpaceProps, record: PropertyRecord): PropertyRecord | undefined {
    const config = record.config;
    if (record.kind !== "relation" || config?.kind !== "relation" || !config.twoWay || config.pairedPropertyId === undefined) {
      return undefined;
    }
    const other = this.#entry(space, config.pairedPropertyId);
    const back = other?.config;
    const mutual =
      other !== undefined &&
      other.kind === "relation" &&
      other.id !== record.id &&
      back?.kind === "relation" &&
      back.twoWay &&
      back.pairedPropertyId === record.id;
    return mutual ? other : undefined;
  }

  /** Whether the space has this page (live or trashed; not purged, not another space's). */
  #exists(spaceId: SpaceId, pageId: PageId): boolean {
    return this.#deps.structure.pageMeta(pageId)?.spaceId === spaceId;
  }

  /** What a page stores in a relation property, over the pages that exist, in order. */
  #storedRelation(space: SpaceProps, spaceId: SpaceId, entry: PropertyRecord, pageId: PageId): PageId[] {
    if (!this.#exists(spaceId, pageId)) {
      return [];
    }
    const value = readValue(entry, space.values.get(pageId)?.get(entry.id));
    return value?.kind === "relation" ? value.value.filter((id) => this.#exists(spaceId, id)) : [];
  }

  /**
   * The relation a page shows: what it stores plus, for a two-way relation, the pages that store
   * it on the paired side (oldest first, then by id), over the pages that exist.
   */
  #effective(space: SpaceProps, spaceId: SpaceId, entry: PropertyRecord, pageId: PageId): PageId[] {
    if (!this.#exists(spaceId, pageId)) {
      return [];
    }
    const pages = this.#storedRelation(space, spaceId, entry, pageId);
    const partner = this.#partner(space, entry);
    if (partner !== undefined) {
      const incoming: PageMeta[] = [];
      for (const other of space.values.keys()) {
        const meta = this.#deps.structure.pageMeta(other);
        if (meta?.spaceId === spaceId && this.#storedRelation(space, spaceId, partner, other).includes(pageId)) {
          incoming.push(meta);
        }
      }
      incoming.sort((a, b) => a.createdAt - b.createdAt || compareCodePoints(a.id, b.id));
      for (const meta of incoming) {
        if (!pages.includes(meta.id)) {
          pages.push(meta.id);
        }
      }
    }
    return pages;
  }

  /**
   * The refusal a rollup configuration deserves, or the kind of its target (`undefined`: the
   * title) when it is valid (`check_rollup_config`).
   */
  #rollupCheck(space: SpaceProps, config: PropertyConfig): { problem: string } | { kind: PropertyKind | undefined } {
    if (config.kind !== "rollup") {
      return { problem: "the configuration does not match the kind" };
    }
    const relation = this.#entry(space, config.relationPropertyId);
    if (relation === undefined || relation.kind !== "relation") {
      return { problem: "the rollup needs a relation property" };
    }
    let kind: PropertyKind | undefined;
    if (config.target !== ROLLUP_TITLE_TARGET) {
      const target = this.#entry(space, config.target);
      if (target === undefined) {
        return { problem: "the rollup target does not exist" };
      }
      if (!isRollupTargetKind(target.kind)) {
        return { problem: "this kind of property cannot be rolled up" };
      }
      kind = target.kind;
    }
    if (!ROLLUP_FUNCTIONS.includes(config.function) || !rollupFunctionAccepts(config.function, kind)) {
      return { problem: "this function does not apply to the target" };
    }
    return { kind };
  }

  #checkRollup(space: SpaceProps, config: PropertyConfig): void {
    const checked = this.#rollupCheck(space, config);
    if ("problem" in checked) {
      throw invalid("config", checked.problem);
    }
  }

  /** The value of a rollup for a page: `undefined` when its configuration is not valid (any more) or it has no result. */
  #rollupValue(space: SpaceProps, spaceId: SpaceId, entry: PropertyRecord, pageId: PageId): PropertyValue | undefined {
    const config = entry.config;
    if (config?.kind !== "rollup" || "problem" in this.#rollupCheck(space, config)) {
      return undefined;
    }
    const relation = this.#entry(space, config.relationPropertyId);
    if (relation === undefined) {
      return undefined;
    }
    const target = config.target === ROLLUP_TITLE_TARGET ? undefined : this.#entry(space, config.target);
    const values: (PropertyValue | undefined)[] = [];
    for (const related of this.#effective(space, spaceId, relation, pageId)) {
      const meta = this.#deps.structure.pageMeta(related);
      if (meta === undefined || meta.trashed) {
        continue;
      }
      if (config.target === ROLLUP_TITLE_TARGET) {
        values.push(/^\p{White_Space}*$/u.test(meta.title) ? undefined : { kind: "text", value: meta.title });
      } else {
        values.push(target === undefined ? undefined : readValue(target, space.values.get(related)?.get(target.id)));
      }
    }
    const result = evaluateRollup(config.function, values);
    return result === undefined ? undefined : { kind: "rollup", value: { function: config.function, result } };
  }

  // ----- formulas (ADR-0055) -----------------------------------------------------------------

  /** The schema changed: what was derived from it (the engine's schema, the result types) is made again on use. */
  #schemaChanged(spaceId: SpaceId): void {
    const space = this.#spaces.get(spaceId);
    if (space !== undefined) {
      space.formulaSchema = undefined;
      space.formulaTypes = undefined;
    }
  }

  /** The time of a read: now, and the device's offset from UTC at that time. */
  #env(): FormulaEnv {
    const nowMs = this.#deps.now();
    const offsetSeconds = this.#deps.localOffsetSeconds?.(nowMs) ?? 0 - new Date(nowMs).getTimezoneOffset() * 60;
    return { nowMs, offsetSeconds };
  }

  /** The schema as the engine sees it, Tags first (built again after a schema change). */
  #formulaSchema(space: SpaceProps): FormulaSchema {
    space.formulaSchema ??= this.#buildFormulaSchema(space);
    return space.formulaSchema;
  }

  #buildFormulaSchema(space: SpaceProps): FormulaSchema {
    return formulaSchemaOf(this.#schema(space));
  }

  /** The result type of a formula: `undefined` when its expression does not parse or check against the schema now. */
  #formulaType(space: SpaceProps, entry: PropertyRecord): FormulaType | undefined {
    const types = (space.formulaTypes ??= new Map<PropertyId, FormulaType | undefined>());
    if (!types.has(entry.id)) {
      const config = entry.config;
      const checked = config?.kind === "formula" ? checkFormula(config.expression, this.#formulaSchema(space), entry.id) : undefined;
      types.set(entry.id, checked === undefined || isFormulaError(checked) ? undefined : checked);
    }
    return types.get(entry.id);
  }

  /**
   * The refusal a formula configuration deserves (`InvalidInput` for `config`, with the error's code, position and
   * message): the configuration is one of a formula, its expression parses and checks against the schema, and `selfId`
   * (the property itself) is not reached through the properties it reads.
   */
  #checkFormula(space: SpaceProps, selfId: PropertyId | undefined, config: PropertyConfig): void {
    if (config.kind !== "formula") {
      throw invalid("config", "the configuration does not match the kind");
    }
    const checked = checkFormula(config.expression, this.#formulaSchema(space), selfId);
    if (isFormulaError(checked)) {
      throw formulaRefusal(checked);
    }
  }

  /** The value of a formula for a page: `undefined` when it does not check (any more) or its result is empty. */
  #formulaValue(
    space: SpaceProps,
    spaceId: SpaceId,
    entry: PropertyRecord,
    pageId: PageId,
    timeOfRead: () => FormulaEnv,
  ): PropertyValue | undefined {
    const config = entry.config;
    if (config?.kind !== "formula" || this.#formulaType(space, entry) === undefined || !this.#exists(spaceId, pageId)) {
      return undefined;
    }
    const result = evaluateFormula(config.expression, pageId, this.#formulaHost(space, spaceId), timeOfRead());
    return result === null ? undefined : { kind: "formula", value: result };
  }

  /**
   * What a formula reads of a space (`kb-model`'s `ModelHost`): stored values and rollups as the page reads them, a
   * relation as the rollup reads it (both directions, live pages only), titles, creation and last edit times, file names
   * and the members' names (their e-mail is not known here).
   */
  #formulaHost(space: SpaceProps, spaceId: SpaceId): FormulaHost {
    const schema = this.#formulaSchema(space);
    const structure = this.#deps.structure;
    const related = (page: PageId, relation: PropertyId): PageId[] => {
      const entry = this.#entry(space, relation);
      if (entry?.kind !== "relation") {
        return [];
      }
      return this.#effective(space, spaceId, entry, page).filter((id) => {
        const meta = structure.pageMeta(id);
        return meta !== undefined && meta.spaceId === spaceId && !meta.trashed;
      });
    };
    return {
      property: (id) => schema.property(id),
      properties: () => schema.properties(),
      title: (page) => (this.#exists(spaceId, page) ? structure.pageMeta(page)?.title : undefined),
      value: (page, property) => {
        const entry = this.#entry(space, property);
        if (entry === undefined || !this.#exists(spaceId, page)) {
          return undefined;
        }
        switch (entry.kind) {
          case "rollup":
            return this.#rollupValue(space, spaceId, entry, page);
          case "relation": {
            const pages = related(page, property);
            return pages.length > 0 ? { kind: "relation", value: pages } : undefined;
          }
          default:
            // A formula is the engine's to evaluate; the times are `createdMs` and `editedMs`; the devices are not
            // readable in a formula.
            return isComputedPropertyKind(entry.kind) ? undefined : readValue(entry, space.values.get(page)?.get(entry.id));
        }
      },
      related,
      createdMs: (page) => (this.#exists(spaceId, page) ? structure.pageMeta(page)?.createdAt : undefined),
      editedMs: (page) => (this.#exists(spaceId, page) ? structure.pageMeta(page)?.updatedAt : undefined),
      person: (account) => {
        const name = this.#deps.personName?.(spaceId, account);
        return name === undefined ? undefined : { name, email: "" };
      },
      fileName: (attachment) => this.#deps.attachmentName?.(attachment),
    };
  }

  /** The attachments the readable values of a property list, over the pages that exist. */
  #filesOf(space: SpaceProps, propertyId: PropertyId): AttachmentId[] {
    const entry = this.#entry(space, propertyId);
    if (entry === undefined || entry.kind !== "files") {
      return [];
    }
    const found: AttachmentId[] = [];
    for (const [pageId, map] of space.values) {
      if (this.#deps.structure.pageMeta(pageId) === undefined) {
        continue;
      }
      const value = readValue(entry, map.get(propertyId));
      found.push(...(value === undefined ? [] : propertyValueFiles(value)));
    }
    return found;
  }

  /** A space's properties; `NotFound` for a space the structure does not know. */
  #space(spaceId: SpaceId): SpaceProps {
    if (!this.#deps.structure.hasSpace(spaceId)) {
      throw notFound("space not found");
    }
    let space = this.#spaces.get(spaceId);
    if (space === undefined) {
      space = { tags: undefined, others: [], values: new Map() };
      this.#spaces.set(spaceId, space);
    }
    return space;
  }

  /** A live or trashed page; `NotFound` once purged. */
  #page(pageId: PageId): PageMeta {
    const page = this.#deps.structure.pageMeta(pageId);
    if (page === undefined) {
      throw notFound("page not found");
    }
    return page;
  }

  /** Tags first (virtual until written), then the others. Do not mutate a virtual entry. */
  #schema(space: SpaceProps): PropertyRecord[] {
    return [space.tags ?? virtualTags(), ...space.others];
  }

  #entry(space: SpaceProps, propertyId: PropertyId): PropertyRecord | undefined {
    if (propertyId === TAGS_PROPERTY_ID) {
      return space.tags ?? virtualTags();
    }
    return space.others.find((record) => record.id === propertyId);
  }

  /** An entry that must exist (seeding: Tags always does). */
  #schemaEntry(space: SpaceProps, propertyId: PropertyId): PropertyRecord {
    const entry = this.#entry(space, propertyId);
    if (entry === undefined) {
      throw internalError("MemoryTransport seed: a property is missing");
    }
    return entry;
  }

  /** Tags in the schema: written now when it was only listed (its creation time is now). */
  #storedTags(space: SpaceProps): PropertyRecord {
    if (space.tags === undefined) {
      space.tags = { ...virtualTags(), createdAt: this.#deps.now() };
    }
    return space.tags;
  }

  #changed(spaceId: SpaceId, schema: boolean): void {
    if (schema) {
      this.#schemaChanged(spaceId);
      this.#deps.emit({ type: "PropertySchemaChanged", data: { spaceId } });
    }
    this.#deps.emit({ type: "StructureChanged", data: { spaceId } });
  }

  #defaultStatusOptions(): PropertyOption[] {
    return DEFAULT_STATUS_OPTIONS.map(({ name, group, color }) => ({ id: this.#deps.newId(), name, color, group }));
  }

  /**
   * What an option added to `entry` needs checked before anything is written: a kind with a list,
   * a group only for status options (default `todo`) and room for one more. Returns the group.
   */
  #checkNewOption(entry: PropertyRecord, group: StatusGroup | undefined): StatusGroup | undefined {
    if (!propertyKindHasOptions(entry.kind)) {
      throw invalid("propertyId", "this kind of property has no options");
    }
    let optionGroup: StatusGroup | undefined;
    if (entry.kind === "status") {
      optionGroup = group ?? "todo";
    } else if (group !== undefined) {
      throw invalid("group", "only status options have a group");
    }
    if (entry.options.length >= MAX_OPTIONS) {
      throw invalid("propertyId", `a property holds at most ${String(MAX_OPTIONS)} options`);
    }
    return optionGroup;
  }

  #pushOption(
    target: PropertyRecord,
    name: string,
    color: OptionColor,
    group: StatusGroup | undefined,
    id: OptionId | undefined,
  ): PropertyOption {
    if (id !== undefined && target.options.some((option) => option.id === id)) {
      throw invalid("optionId", "an option with this id exists");
    }
    const option: PropertyOption = { id: id ?? this.#deps.newId(), name, color };
    if (group !== undefined) {
      option.group = group;
    }
    target.options.push(option);
    return option;
  }

  /** A property of a backup, checked. */
  #restoredRecord(def: PropertyDef): PropertyRecord {
    const kind = oneOf("kind", PROPERTY_KINDS, def.kind);
    if (typeof def.name !== "string" || typeof def.createdAt !== "number") {
      throw internalError("a property is damaged");
    }
    const options = def.options.map((option): PropertyOption => {
      if (typeof option.name !== "string") {
        throw internalError("an option is damaged");
      }
      const copy: PropertyOption = { id: option.id, name: option.name, color: oneOf("color", OPTION_COLORS, option.color) };
      if ((kind === "status") !== (option.group !== undefined)) {
        throw internalError("an option's group does not fit its property");
      }
      if (option.group !== undefined) {
        copy.group = oneOf("group", STATUS_GROUPS, option.group);
      }
      return copy;
    });
    if (options.length > 0 && !propertyKindHasOptions(kind)) {
      throw internalError("a property has options but its kind takes none");
    }
    if (new Set(options.map((option) => option.id)).size !== options.length) {
      throw internalError("an option appears twice");
    }
    const record: PropertyRecord = { id: def.id, name: def.name, kind, createdAt: def.createdAt, options };
    if (kind === "relation") {
      record.config = def.config?.kind === "relation" ? configOf(def.config) : oneWay();
    } else if (kind === "rollup" && def.config?.kind === "rollup") {
      record.config = configOf(def.config);
    } else if (kind === "formula" && def.config?.kind === "formula" && typeof def.config.expression === "string") {
      // Stored as given, checked on read: a backup of a space whose formula no longer checks restores as it was.
      record.config = { kind: "formula", expression: def.config.expression };
    }
    return record;
  }

  /** What a filter asks of one page's value; `undefined` for a filter that can match nothing. */
  #matcher(entry: PropertyRecord, filter: PropertyFilter): Matcher | undefined {
    if (entry.kind === "relation") {
      return this.#relationMatcher(filter);
    }
    if (entry.kind === "person") {
      return this.#personMatcher(filter);
    }
    switch (filter.type) {
      case "ContainsPerson":
      case "ContainsMe":
        throw invalid("filter", "people can only be searched in a person property");
      case "HasValue":
        return (value) => value !== undefined;
      case "Equals": {
        const target = filter.data;
        if (propertyValueKind(target) !== entry.kind) {
          throw invalid("filter", "the value does not match the kind of the property");
        }
        if (isComputedPropertyKind(entry.kind)) {
          // A computed value is compared as it is read (no rule applies to it).
          return (value) => value !== undefined && propertyValuesEqual(target, value);
        }
        const problem = propertyValueProblem(target);
        if (problem !== undefined) {
          throw invalid("filter", problem);
        }
        // An empty text, URL or list is no value: nothing equals it.
        if (isEmptyPropertyValue(target)) {
          return undefined;
        }
        if (target.kind === "files" || target.kind === "checklist") {
          throw invalid(
            "filter",
            "a list of files or items cannot be compared; use HasValue (or ChecklistComplete for a checklist)",
          );
        }
        if (target.kind === "place") {
          throw invalid("filter", "a place cannot be compared; use TextContains or HasValue");
        }
        if (target.kind === "multi_select") {
          const wanted = new Set(target.value);
          return (value) =>
            value?.kind === "multi_select" &&
            value.value.length === wanted.size &&
            value.value.every((id) => wanted.has(id));
        }
        return (value) => value !== undefined && propertyValuesEqual(target, value);
      }
      case "ContainsOption": {
        if (!propertyKindHasOptions(entry.kind)) {
          throw invalid("filter", "this kind of property has no options");
        }
        const optionId = filter.data;
        return (value) => {
          if (value?.kind === "select" || value?.kind === "status") {
            return value.value === optionId;
          }
          return value?.kind === "multi_select" && value.value.includes(optionId);
        };
      }
      case "TextContains": {
        if (
          entry.kind !== "text" &&
          entry.kind !== "url" &&
          entry.kind !== "email" &&
          entry.kind !== "phone" &&
          entry.kind !== "place"
        ) {
          throw invalid("filter", "text can only be searched in a text, URL, email, phone or place property");
        }
        const text = filter.data;
        if (text === "") {
          throw invalid("filter", "must not be empty");
        }
        if (codePointLength(text) > MAX_FILTER_CHARS) {
          throw invalid("filter", `must be at most ${String(MAX_FILTER_CHARS)} characters`);
        }
        if (/\p{Cc}/u.test(text)) {
          throw invalid("filter", "must not contain control characters");
        }
        // Unicode lower case on both sides, a substring.
        const needle = text.toLowerCase();
        return (value) => {
          if (value === undefined) {
            return false;
          }
          if (value.kind === "place") {
            return value.value.name.toLowerCase().includes(needle);
          }
          return (
            (value.kind === "text" || value.kind === "url" || value.kind === "email" || value.kind === "phone") &&
            value.value.toLowerCase().includes(needle)
          );
        };
      }
      case "DateBetween": {
        if (entry.kind !== "date") {
          throw invalid("filter", "dates can only be searched in a date property");
        }
        const from = optional(filter.data.from);
        const to = optional(filter.data.to);
        for (const end of [from, to]) {
          if (end !== undefined && !isIsoDate(end)) {
            throw invalid("filter", "a date must be a real day as YYYY-MM-DD");
          }
        }
        if (from !== undefined && to !== undefined && from > to) {
          throw invalid("filter", "from must not be after to");
        }
        // `YYYY-MM-DD` compares as text; the time of day is ignored.
        return (value) =>
          value?.kind === "date" &&
          (from === undefined || value.value.date >= from) &&
          (to === undefined || value.value.date <= to);
      }
      case "Checked": {
        if (entry.kind !== "checkbox") {
          throw invalid("filter", "a checked state can only be searched in a checkbox property");
        }
        const checked = filter.data;
        // `false` lists every live page that is not on: off or unset.
        return (value) => (value?.kind === "checkbox" && value.value) === checked;
      }
      case "ChecklistComplete": {
        if (entry.kind !== "checklist") {
          throw invalid("filter", "completion can only be searched in a checklist property");
        }
        const complete = filter.data;
        // Every item done, or an item not done; a page without a checklist is in neither answer.
        return (value) => {
          if (value?.kind !== "checklist") {
            return false;
          }
          const { done, total } = checklistProgress(value.value);
          return complete ? done === total : done < total;
        };
      }
      case "InstantBetween": {
        if (entry.kind !== "created_time" && entry.kind !== "last_edited_time") {
          throw invalid("filter", "instants can only be searched in a created or last edited time property");
        }
        const from = optional(filter.data.from);
        const to = optional(filter.data.to);
        if (from !== undefined && to !== undefined && from > to) {
          throw invalid("filter", "from must not be after to");
        }
        return (value) =>
          (value?.kind === "created_time" || value?.kind === "last_edited_time") &&
          (from === undefined || value.value >= from) &&
          (to === undefined || value.value <= to);
      }
      case "RelatesTo":
        throw invalid("filter", "related pages can only be searched in a relation property");
      default:
        throw invalid("filter", "unknown filter");
    }
  }

  /**
   * The filters of a Person property (ADR-0049): has a person, lists this account (`ContainsMe`: a saved view is shared and "me"
   * differs per viewer, so it is its own filter) or lists an account. Every other filter is `InvalidInput` for `filter`.
   */
  #personMatcher(filter: PropertyFilter): Matcher {
    switch (filter.type) {
      case "HasValue":
        return (value) => value !== undefined;
      case "ContainsPerson": {
        const wanted = canonicalId(filter.data) ?? filter.data;
        return (value) => value?.kind === "person" && value.value.includes(wanted);
      }
      case "ContainsMe": {
        const me = this.#deps.me?.();
        return (value) => me !== undefined && value?.kind === "person" && value.value.includes(me);
      }
      case "Equals":
        throw invalid("filter", "a list of people cannot be compared; use HasValue, ContainsPerson or ContainsMe");
      case "ContainsOption":
        throw invalid("filter", "this kind of property has no options");
      case "TextContains":
        throw invalid("filter", "text can only be searched in a text, URL, email, phone or place property");
      case "DateBetween":
        throw invalid("filter", "dates can only be searched in a date property");
      case "Checked":
        throw invalid("filter", "a checked state can only be searched in a checkbox property");
      case "ChecklistComplete":
        throw invalid("filter", "completion can only be searched in a checklist property");
      case "RelatesTo":
        throw invalid("filter", "related pages can only be searched in a relation property");
      case "InstantBetween":
        throw invalid("filter", "instants can only be searched in a created or last edited time property");
      default:
        throw invalid("filter", "unknown filter");
    }
  }

  /** The filters of a relation: has a related page, or relates to a page (both directions of a two-way relation). */
  #relationMatcher(filter: PropertyFilter): Matcher {
    switch (filter.type) {
      case "HasValue":
        return (value) => value !== undefined;
      case "RelatesTo": {
        const wanted = canonicalId(filter.data) ?? filter.data;
        return (value) => value?.kind === "relation" && value.value.includes(wanted);
      }
      case "Equals":
        throw invalid("filter", "a list of pages cannot be compared; use HasValue or RelatesTo");
      case "ContainsOption":
        throw invalid("filter", "this kind of property has no options");
      case "TextContains":
        throw invalid("filter", "text can only be searched in a text, URL, email, phone or place property");
      case "DateBetween":
        throw invalid("filter", "dates can only be searched in a date property");
      case "Checked":
        throw invalid("filter", "a checked state can only be searched in a checkbox property");
      case "ChecklistComplete":
        throw invalid("filter", "completion can only be searched in a checklist property");
      case "InstantBetween":
        throw invalid("filter", "instants can only be searched in a created or last edited time property");
      case "ContainsPerson":
      case "ContainsMe":
        throw invalid("filter", "people can only be searched in a person property");
      default:
        throw invalid("filter", "unknown filter");
    }
  }

  // ----- seeding ------------------------------------------------------------------------

  #seedOption(target: PropertyRecord, seed: SeedOption): void {
    const name = displayName(seed.name, MAX_NAME_CHARS, "name");
    const color = seed.color === undefined ? "gray" : oneOf("color", OPTION_COLORS, seed.color);
    const group = seed.group === undefined ? undefined : oneOf("group", STATUS_GROUPS, seed.group);
    const optionGroup = this.#checkNewOption(target, group);
    this.#pushOption(target, name, color, optionGroup, seed.id);
  }

  #seedProperty(space: SpaceProps, def: SeedProperty): void {
    // Seeding changes the schema without a request: what a formula derived from it is made again on use.
    space.formulaSchema = undefined;
    space.formulaTypes = undefined;
    const kind = oneOf("kind", PROPERTY_KINDS, def.kind);
    const name = displayName(def.name, MAX_NAME_CHARS, "name");
    if (space.others.length + 1 >= MAX_PROPERTIES) {
      throw invalid("spaceId", `a space holds at most ${String(MAX_PROPERTIES)} properties`);
    }
    if (def.id !== undefined && this.#entry(space, def.id) !== undefined) {
      throw invalid("propertyId", "a property with this id exists");
    }
    const record: PropertyRecord = {
      id: def.id ?? this.#deps.newId(),
      name,
      kind,
      createdAt: this.#deps.now(),
      options: [],
    };
    if (kind === "rollup") {
      if (def.rollup === undefined) {
        throw internalError(`MemoryTransport seed: the rollup "${name}" needs \`rollup\``);
      }
      const relation = space.others.find((candidate) => candidate.name === def.rollup?.relation);
      const target =
        def.rollup.target === ROLLUP_TITLE_TARGET
          ? ROLLUP_TITLE_TARGET
          : space.others.find((candidate) => candidate.name === def.rollup?.target)?.id;
      if (relation === undefined || target === undefined) {
        throw internalError(`MemoryTransport seed: the rollup "${name}" names a property the space does not have`);
      }
      const config: PropertyConfig = {
        kind: "rollup",
        relationPropertyId: relation.id,
        target,
        function: def.rollup.function,
      };
      this.#checkRollup(space, config);
      record.config = config;
    }
    if (kind === "formula") {
      if (def.formula === undefined) {
        throw internalError(`MemoryTransport seed: the formula "${name}" needs \`formula\``);
      }
      // The expression names the properties seeded before it; it is stored with their ids.
      const stored = formulaToStored(def.formula, this.#buildFormulaSchema(space), FORMULA_TITLE_LABEL);
      if (typeof stored !== "string") {
        throw internalError(`MemoryTransport seed: the formula "${name}" is refused: ${stored.code} at ${String(stored.start)}: ${stored.message}`);
      }
      const config: PropertyConfig = { kind: "formula", expression: stored };
      try {
        this.#checkFormula(space, record.id, config);
      } catch (error) {
        throw internalError(`MemoryTransport seed: the formula "${name}" is refused: ${error instanceof Error ? error.message : String(error)}`);
      }
      record.config = config;
    }
    if (kind === "relation") {
      record.config = oneWay();
      if (def.twoWay === true) {
        if (space.others.length + 2 + 1 > MAX_PROPERTIES) {
          throw invalid("spaceId", `a space holds at most ${String(MAX_PROPERTIES)} properties`);
        }
        const back: PropertyRecord = {
          id: this.#deps.newId(),
          name: def.backName === undefined ? backPropertyName(name) : displayName(def.backName, MAX_NAME_CHARS, "name"),
          kind: "relation",
          createdAt: this.#deps.now(),
          options: [],
          config: { kind: "relation", twoWay: true, pairedPropertyId: record.id },
        };
        record.config = { kind: "relation", twoWay: true, pairedPropertyId: back.id };
        space.others.push(record, back);
        return;
      }
    }
    if (def.options !== undefined) {
      if (!propertyKindHasOptions(kind)) {
        throw invalid("options", "this kind of property has no options");
      }
      for (const option of def.options) {
        this.#seedOption(record, option);
      }
    } else if (kind === "status") {
      record.options = this.#defaultStatusOptions();
    }
    space.others.push(record);
  }

  /** The id of the Tags option with this name, created (gray) when Tags has none. */
  #seedTagOption(space: SpaceProps, name: string): OptionId {
    const found = (space.tags ?? virtualTags()).options.find((option) => option.name === name);
    if (found !== undefined) {
      return found.id;
    }
    const option = displayName(name, MAX_NAME_CHARS, "name");
    return this.#pushOption(this.#storedTags(space), option, "gray", undefined, undefined).id;
  }

  /** The id of an option of `entry` by name; Tags gets a missing one, any other property refuses. */
  #seedOptionId(space: SpaceProps, entry: PropertyRecord, property: string, name: string): OptionId {
    const found = entry.options.find((option) => option.name === name);
    if (found !== undefined) {
      return found.id;
    }
    if (entry.id === TAGS_PROPERTY_ID) {
      return this.#seedTagOption(space, name);
    }
    throw internalError(`MemoryTransport seed: the property "${property}" has no option named "${name}"`);
  }

  /** A seeded value as the property's kind wants it (see `SeedValue`). */
  #seedValue(spaceId: SpaceId, space: SpaceProps, entry: PropertyRecord, property: string, raw: SeedValue): PropertyValue {
    const wrong = (): CoreError =>
      internalError(`MemoryTransport seed: the value of "${property}" does not fit its kind (${entry.kind})`);
    switch (entry.kind) {
      case "text":
      case "url":
      case "email":
      case "phone":
        if (typeof raw !== "string") {
          throw wrong();
        }
        return { kind: entry.kind, value: raw };
      case "number":
        if (typeof raw !== "number") {
          throw wrong();
        }
        return { kind: "number", value: raw };
      case "checkbox":
        if (typeof raw !== "boolean") {
          throw wrong();
        }
        return { kind: "checkbox", value: raw };
      case "date": {
        if (typeof raw === "string") {
          return { kind: "date", value: { date: raw } };
        }
        if (typeof raw !== "object" || Array.isArray(raw) || typeof (raw as { date?: unknown }).date !== "string") {
          throw wrong();
        }
        const { date, time } = raw as { date: string; time?: string };
        return { kind: "date", value: time === undefined ? { date } : { date, time } };
      }
      case "select":
      case "status":
        if (typeof raw !== "string") {
          throw wrong();
        }
        return { kind: entry.kind, value: this.#seedOptionId(space, entry, property, raw) };
      case "multi_select": {
        const names: unknown = raw;
        if (!Array.isArray(names) || !names.every((name) => typeof name === "string")) {
          throw wrong();
        }
        return {
          kind: "multi_select",
          value: names.map((name) => this.#seedOptionId(space, entry, property, name)),
        };
      }
      case "files": {
        const ids: unknown = raw;
        if (!Array.isArray(ids) || !ids.every((id) => typeof id === "string" && canonicalId(id) !== undefined)) {
          throw wrong();
        }
        return { kind: "files", value: ids.map((id: string) => canonicalId(id) ?? id) };
      }
      case "place": {
        if (typeof raw !== "object" || Array.isArray(raw) || typeof (raw as { name?: unknown }).name !== "string") {
          throw wrong();
        }
        const { name, lat, lon } = raw as { name: string; lat?: number; lon?: number };
        return { kind: "place", value: lat === undefined || lon === undefined ? { name } : { name, lat, lon } };
      }
      case "person": {
        // The NAMES of members of the space (this account too: "me", or its profile name; a name that is nobody's
        // is a made-up person's), as `{ kind: "person", value: [account ids] }`.
        const names: unknown = raw;
        if (!Array.isArray(names) || !names.every((name) => typeof name === "string")) {
          throw wrong();
        }
        const account = this.#deps.account;
        if (account === undefined) {
          throw internalError(`MemoryTransport seed: the person property "${property}" needs the sharing mirror`);
        }
        return { kind: "person", value: names.map((name: string) => account(spaceId, name)) };
      }
      case "relation": {
        const titles: unknown = raw;
        if (!Array.isArray(titles) || !titles.every((title) => typeof title === "string")) {
          throw wrong();
        }
        return {
          kind: "relation",
          value: titles.map((title: string) => {
            const found = this.#seedTitles.get(title);
            if (found === undefined) {
              throw internalError(`MemoryTransport seed: the value of "${property}" names no page titled "${title}"`);
            }
            return found;
          }),
        };
      }
      case "rollup":
      case "formula":
      case "created_time":
      case "last_edited_time":
      case "created_by":
      case "last_edited_by":
        throw internalError(`MemoryTransport seed: "${property}" is computed from the page and takes no value`);
      case "checklist": {
        const items: unknown = raw;
        if (!Array.isArray(items)) {
          throw wrong();
        }
        return {
          kind: "checklist",
          value: items.map((item: unknown) => {
            if (typeof item !== "object" || item === null || typeof (item as { text?: unknown }).text !== "string") {
              throw wrong();
            }
            const { id, text, done } = item as { id?: string; text: string; done?: boolean };
            return { id: id ?? this.#deps.newId(), text, done: done ?? false };
          }),
        };
      }
    }
  }

  /** Stores a seeded value after checking it like `PagePropertySet`; an empty one is no value. */
  #seedStore(space: SpaceProps, pageId: PageId, entry: PropertyRecord, value: PropertyValue): void {
    const stored = normalisePropertyValue(value);
    if (stored === null) {
      return;
    }
    const map = space.values.get(pageId) ?? new Map<PropertyId, PropertyValue>();
    map.set(entry.id, copyPropertyValue(stored));
    space.values.set(pageId, map);
    // The other side of a two-way relation stores the link too, as a write would.
    const partner = stored.kind === "relation" ? this.#partner(space, entry) : undefined;
    if (stored.kind === "relation" && partner !== undefined) {
      for (const other of stored.value) {
        const otherMap = space.values.get(other) ?? new Map<PropertyId, PropertyValue>();
        const raw = otherMap.get(partner.id);
        const listed = raw?.kind === "relation" ? raw.value : [];
        if (!listed.includes(pageId)) {
          otherMap.set(partner.id, { kind: "relation", value: [...listed, pageId] });
          space.values.set(other, otherMap);
        }
      }
    }
  }
}
