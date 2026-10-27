// How a move to another space carries property values and saved views (ADR-0038,
// `docs/architecture/space-move.md`, 4 steps 2 and 5), as pure functions: the plan of what the
// target's schema needs and which value goes where, and the remapping of a view's attributes. The
// mirror (`./memory-move`) reads the state, plans with these, then writes.
//
// The plan works on a model of the target's schema so that the counts of the plan request and of the
// move agree: properties are matched by name (Unicode case folding, the built-in Tags by id), reused
// when the kind is the same or both are select-like (options by name, missing ones added), reused
// with the value coerced through its text when the kinds differ (`valuesDropped` when it does not fit),
// else created with the same kind, options and colours; the 100-property and 500-option limits turn
// values into `valuesDropped`. Relation values keep the pages that move along (the rest count as
// removed links); a two-way pair is created as a pair when both halves are used, else one-way; a
// rollup is created when its relation is. A formula (ADR-0055) a saved view or a dashboard widget names is created in
// the target too, after the properties it reads (which are created when missing, and so on through formulas that read
// formulas), with its expression's ids made the target's; one the target has by name and kind is used. A formula with
// an id that has no counterpart is created as written and stays broken (`MemoryProperties.createFormulaUnchecked`).

import type { AttachmentId } from "./generated/AttachmentId";
import type { OptionColor } from "./generated/OptionColor";
import type { PageId } from "./generated/PageId";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RollupFunction } from "./generated/RollupFunction";
import type { StatusGroup } from "./generated/StatusGroup";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { formulaReads } from "./memory-formula";
import { parseBool, parseDate, parseNumber } from "./memory-import-text";
import type { Attrs } from "./page-doc";
import {
  MAX_OPTIONS,
  MAX_PROPERTIES,
  TAGS_PROPERTY_ID,
  isComputedPropertyKind,
  propertyKindHasOptions,
  propertyValueProblem,
  propertyValueText,
} from "./properties";
import { ROLLUP_TITLE_TARGET, isRollupTargetKind, rollupFunctionAccepts } from "./rollup";

/** A property of the target: the id of an existing one, or a temporary `new:<n>` for one the plan creates. */
export type Ref = string;

/** An option to create. */
export interface PlannedOption {
  name: string;
  color: OptionColor;
  group?: StatusGroup;
}

/** A change of the target's schema, in the order to apply them. */
export type PropertyOp =
  | {
      op: "create";
      ref: Ref;
      name: string;
      kind: PropertyKind;
      /** The options it starts with (a status property replaces its three defaults with these). */
      options: PlannedOption[];
      /** A relation made together with its back property (a two-way pair). */
      pair?: { ref: Ref; name: string };
      /** A rollup: its relation, its target (`"title"` or a property) and its function. */
      rollup?: { relation: Ref; target: Ref; function: RollupFunction };
      /** A formula: the source's stored expression, and the target property of each source property it reads. */
      formula?: { expression: string; reads: ReadonlyMap<PropertyId, Ref> };
    }
  | { op: "option"; property: Ref; option: PlannedOption };

/** A value to store: the kinds with options name them (the ids exist once the options do). */
export type PlannedValue =
  | { named: "select" | "status"; name: string }
  | { named: "multi_select"; names: string[] }
  | { stored: PropertyValue };

/** What the move does to the target's schema and to each page's values. */
export interface PropertyPlan {
  ops: PropertyOp[];
  /** Each source property that has a counterpart: where its values and view entries go. */
  mapping: Map<PropertyId, Ref>;
  values: { pageId: PageId; property: Ref; value: PlannedValue }[];
  /** Properties created (a pair counts two). */
  created: number;
  /** Values that cannot follow. */
  dropped: number;
  /** Relation entries of the moved pages that pointed at pages that stay. */
  linksRemoved: number;
}

/** What {@link planProperties} reads. */
export interface PropertyPlanInput {
  source: readonly PropertyDef[];
  target: readonly PropertyDef[];
  /** The moved pages' readable stored values (`MemoryProperties.movingValues`). */
  values: ReadonlyMap<PageId, readonly (readonly [PropertyId, PropertyValue])[]>;
  moved: ReadonlySet<PageId>;
  /**
   * The properties the saved views of the moved pages name, widgets of a dashboard included ({@link viewPropertyIds}):
   * a formula among them is carried, with what it reads. Default: none.
   */
  viewProperties?: ReadonlySet<PropertyId>;
  /**
   * The properties the placeholders of the moved pages sit on (ADR-0056), those whose kind takes the placeholder: needs
   * like a property with a value, so the target's property of that name is matched or created. Default: none.
   */
  placeholderProperties?: ReadonlySet<PropertyId>;
  /** The title of a page, for a relation coerced to text. */
  titleOf: (pageId: PageId) => string | undefined;
  /** The name of an attachment, for a file value coerced to text. */
  fileName: (attachmentId: AttachmentId) => string | undefined;
}

const SELECT_LIKE: readonly PropertyKind[] = ["select", "multi_select", "status"];

/** A name as properties and options are matched: Unicode lower case, trimmed. */
function fold(name: string): string {
  return name.trim().toLowerCase();
}

interface ModelProperty {
  ref: Ref;
  name: string;
  kind: PropertyKind;
  options: PlannedOption[];
  fresh: boolean;
}

/** A value's text form, for a target of another kind (`undefined`: nothing to carry). */
function textForm(
  value: PropertyValue,
  optionName: (id: string) => string | undefined,
  input: PropertyPlanInput,
): string | undefined {
  switch (value.kind) {
    case "number":
      return String(value.value);
    case "checkbox":
      return value.value ? "Yes" : "No";
    case "date":
      return value.value.time === undefined ? value.value.date : `${value.value.date} ${value.value.time}`;
    default:
      return propertyValueText(value, optionName, input.fileName, input.titleOf);
  }
}

/** The kinds that a value of another kind can be coerced into through its text (`kb_import::values`). */
function coerceText(kind: PropertyKind, text: string): PlannedValue | undefined {
  const done = (value: PropertyValue): PlannedValue | undefined =>
    propertyValueProblem(value) === undefined ? { stored: value } : undefined;
  switch (kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
      return done({ kind, value: text });
    case "number": {
      const number = parseNumber(text);
      return number === undefined ? undefined : done({ kind, value: number });
    }
    case "checkbox": {
      const checked = parseBool(text);
      return checked === undefined ? undefined : done({ kind, value: checked });
    }
    case "date": {
      const date = parseDate(text);
      return date === undefined ? undefined : done({ kind, value: date });
    }
    case "select":
    case "status":
      return { named: kind, name: text.trim() };
    case "multi_select":
      return { named: kind, names: text.split(", ").map((name) => name.trim()).filter((name) => name !== "") };
    default:
      return undefined;
  }
}

/** Plans what the move does to the schema and the values (see the module comment). */
export function planProperties(input: PropertyPlanInput): PropertyPlan {
  const model: ModelProperty[] = input.target.map((def) => ({
    ref: def.id,
    name: def.name,
    kind: def.kind,
    options: def.options.map((option) => ({
      name: option.name,
      color: option.color,
      ...(option.group === undefined ? {} : { group: option.group }),
    })),
    fresh: false,
  }));
  const byName = new Map<string, ModelProperty>();
  for (const property of model) {
    if (!byName.has(fold(property.name))) {
      byName.set(fold(property.name), property);
    }
  }
  const ops: PropertyOp[] = [];
  const mapping = new Map<PropertyId, Ref>();
  const planned: PropertyPlan["values"] = [];
  let created = 0;
  let dropped = 0;
  let linksRemoved = 0;
  let nextRef = 0;
  const newRef = (): Ref => {
    nextRef += 1;
    return `new:${String(nextRef)}`;
  };

  // Values by source property; a relation keeps the pages that move along.
  const used = new Map<PropertyId, { pageId: PageId; value: PropertyValue }[]>();
  for (const [pageId, entries] of input.values) {
    for (const [propertyId, value] of entries) {
      let kept: PropertyValue = value;
      if (value.kind === "relation") {
        const inside = value.value.filter((id) => input.moved.has(id));
        linksRemoved += value.value.length - inside.length;
        if (inside.length === 0) {
          continue;
        }
        kept = { kind: "relation", value: inside };
      }
      const list = used.get(propertyId) ?? [];
      list.push({ pageId, value: kept });
      used.set(propertyId, list);
    }
  }

  const sourceById = new Map(input.source.map((def) => [def.id, def]));
  const sourceIds: ReadonlySet<string> = new Set(sourceById.keys());
  /** What a source formula reads (the source properties its `prop` calls name). */
  const readsOf = (def: PropertyDef): string[] =>
    def.config?.kind === "formula" ? formulaReads(def.config.expression, sourceIds) : [];
  // What the target must have for the formulas the views name: those formulas, what they read, and so on through the
  // formulas that read formulas (a property a view merely names, without a value, is not carried here).
  const needed = new Set<PropertyId>();
  const queue = [...(input.viewProperties ?? [])].filter((id) => sourceById.get(id)?.kind === "formula");
  for (let id = queue.pop(); id !== undefined; id = queue.pop()) {
    if (needed.has(id)) {
      continue;
    }
    needed.add(id);
    const def = sourceById.get(id);
    if (def !== undefined) {
      queue.push(...readsOf(def).filter((read) => !needed.has(read)));
    }
  }
  const counterpart = (def: PropertyDef): ModelProperty | undefined =>
    def.id === TAGS_PROPERTY_ID ? model.find((property) => property.ref === TAGS_PROPERTY_ID) : byName.get(fold(def.name));

  const addProperty = (property: ModelProperty): void => {
    model.push(property);
    if (!byName.has(fold(property.name))) {
      byName.set(fold(property.name), property);
    }
  };
  /** Room for `count` more properties (Tags counts). */
  const room = (count: number): boolean => model.length + count <= MAX_PROPERTIES;

  /** The option of a model property called `name`, added (an op) when it has none; `undefined` at the limit. */
  const optionOf = (target: ModelProperty, name: string, color: OptionColor, group: StatusGroup | undefined): string | undefined => {
    const wanted = name.trim();
    if (wanted === "") {
      return undefined;
    }
    const found = target.options.find((option) => fold(option.name) === fold(wanted));
    if (found !== undefined) {
      return found.name;
    }
    if (target.options.length >= MAX_OPTIONS) {
      return undefined;
    }
    const option: PlannedOption = { name: wanted, color };
    if (target.kind === "status") {
      option.group = group ?? "todo";
    }
    target.options.push(option);
    ops.push({ op: "option", property: target.ref, option });
    return option.name;
  };

  /** One value carried to `target`: `undefined` when it cannot follow. */
  const carry = (source: PropertyDef, target: ModelProperty, value: PropertyValue): PlannedValue | undefined => {
    const sourceOption = (id: string): PropertyDef["options"][number] | undefined =>
      source.options.find((option) => option.id === id);
    if (SELECT_LIKE.includes(target.kind) && SELECT_LIKE.includes(source.kind)) {
      const names = (value.kind === "multi_select" ? value.value : value.kind === "select" || value.kind === "status" ? [value.value] : [])
        .map(sourceOption)
        .filter((option): option is NonNullable<typeof option> => option !== undefined);
      const wanted = target.kind === "multi_select" ? names : names.slice(0, 1);
      const result: string[] = [];
      for (const option of wanted) {
        const name = optionOf(target, option.name, option.color, option.group);
        if (name === undefined) {
          return undefined;
        }
        if (!result.includes(name)) {
          result.push(name);
        }
      }
      if (result.length === 0) {
        return undefined;
      }
      return target.kind === "multi_select"
        ? { named: "multi_select", names: result }
        : { named: target.kind === "status" ? "status" : "select", name: result[0] ?? "" };
    }
    if (source.kind === target.kind) {
      return { stored: value };
    }
    const text = textForm(value, (id) => sourceOption(id)?.name, input);
    if (text === undefined || text.trim() === "") {
      return undefined;
    }
    const coerced = coerceText(target.kind, text);
    if (coerced === undefined) {
      return undefined;
    }
    if ("named" in coerced) {
      // A select-like target gets the options its text names, coloured as the next default.
      const names = coerced.named === "multi_select" ? coerced.names : [coerced.name];
      const made: string[] = [];
      for (const name of names) {
        const option = optionOf(target, name, "gray", undefined);
        if (option === undefined) {
          return undefined;
        }
        made.push(option);
      }
      return coerced.named === "multi_select" ? { named: "multi_select", names: made } : { named: coerced.named, name: made[0] ?? "" };
    }
    return coerced;
  };

  const place = (source: PropertyDef, target: ModelProperty): void => {
    mapping.set(source.id, target.ref);
    for (const { pageId, value } of used.get(source.id) ?? []) {
      const carried = carry(source, target, value);
      if (carried === undefined || isComputedPropertyKind(target.kind)) {
        dropped += 1;
      } else {
        planned.push({ pageId, property: target.ref, value: carried });
      }
    }
  };

  const optionsOf = (def: PropertyDef): PlannedOption[] =>
    def.options.map((option) => ({
      name: option.name,
      color: option.color,
      ...(option.group === undefined ? {} : { group: option.group }),
    }));

  /** A relation's back property in the source: the mutual partner. */
  const partnerOf = (def: PropertyDef): PropertyDef | undefined => {
    const config = def.config;
    if (def.kind !== "relation" || config?.kind !== "relation" || !config.twoWay || config.pairedPropertyId === undefined) {
      return undefined;
    }
    const other = sourceById.get(config.pairedPropertyId);
    const back = other?.config;
    return other !== undefined && other.id !== def.id && back?.kind === "relation" && back.twoWay && back.pairedPropertyId === def.id
      ? other
      : undefined;
  };

  const dropAll = (def: PropertyDef): void => {
    dropped += used.get(def.id)?.length ?? 0;
  };

  for (const def of input.source) {
    if (mapping.has(def.id) || def.kind === "rollup" || def.kind === "formula") {
      continue;
    }
    const found = counterpart(def);
    if (isComputedPropertyKind(def.kind)) {
      // No stored value: only a property of the same kind and name is a counterpart (for the views); one a formula
      // reads is created when the target has none.
      if (found?.kind === def.kind) {
        mapping.set(def.id, found.ref);
      } else if (found === undefined && needed.has(def.id) && room(1)) {
        const ref = newRef();
        ops.push({ op: "create", ref, name: def.name, kind: def.kind, options: [] });
        addProperty({ ref, name: def.name, kind: def.kind, options: [], fresh: true });
        mapping.set(def.id, ref);
        created += 1;
      }
      continue;
    }
    if (found !== undefined) {
      place(def, found);
      continue;
    }
    if ((used.get(def.id)?.length ?? 0) === 0 && !needed.has(def.id) && input.placeholderProperties?.has(def.id) !== true) {
      continue;
    }
    // Missing: created with the same kind, options and colours; relations as a pair when both halves are used.
    const partner = partnerOf(def);
    const pairWithPartner =
      partner !== undefined &&
      !mapping.has(partner.id) &&
      counterpart(partner) === undefined &&
      (used.get(partner.id)?.length ?? 0) > 0;
    if (!room(pairWithPartner ? 2 : 1)) {
      dropAll(def);
      continue;
    }
    const ref = newRef();
    const options = propertyKindHasOptions(def.kind) ? optionsOf(def) : [];
    if (pairWithPartner) {
      const backRef = newRef();
      ops.push({ op: "create", ref, name: def.name, kind: "relation", options: [], pair: { ref: backRef, name: partner.name } });
      addProperty({ ref, name: def.name, kind: "relation", options: [], fresh: true });
      addProperty({ ref: backRef, name: partner.name, kind: "relation", options: [], fresh: true });
      created += 2;
      const forward = model.find((property) => property.ref === ref);
      const back = model.find((property) => property.ref === backRef);
      if (forward !== undefined && back !== undefined) {
        place(def, forward);
        place(partner, back);
      }
      continue;
    }
    ops.push({ op: "create", ref, name: def.name, kind: def.kind, options });
    const property: ModelProperty = { ref, name: def.name, kind: def.kind, options, fresh: true };
    addProperty(property);
    created += 1;
    place(def, property);
  }

  // Rollups: carried to a property of the same name, or created when their relation was.
  for (const def of input.source) {
    const config = def.config;
    if (def.kind !== "rollup" || config?.kind !== "rollup") {
      continue;
    }
    const found = counterpart(def);
    if (found !== undefined) {
      if (found.kind === "rollup") {
        mapping.set(def.id, found.ref);
      }
      continue;
    }
    const relation = mapping.get(config.relationPropertyId);
    const relationModel = relation === undefined ? undefined : model.find((property) => property.ref === relation);
    // Created when its relation was, or when a formula that a view names reads it.
    if (
      relation === undefined ||
      relationModel === undefined ||
      relationModel.kind !== "relation" ||
      (!relationModel.fresh && !needed.has(def.id))
    ) {
      continue;
    }
    let target: Ref | undefined = ROLLUP_TITLE_TARGET;
    let targetKind: PropertyKind | undefined;
    if (config.target !== ROLLUP_TITLE_TARGET) {
      target = mapping.get(config.target);
      targetKind = model.find((property) => property.ref === target)?.kind;
      if (target === undefined || targetKind === undefined || !isRollupTargetKind(targetKind)) {
        continue;
      }
    }
    if (!rollupFunctionAccepts(config.function, targetKind) || !room(1)) {
      continue;
    }
    const ref = newRef();
    ops.push({ op: "create", ref, name: def.name, kind: "rollup", options: [], rollup: { relation, target, function: config.function } });
    addProperty({ ref, name: def.name, kind: "rollup", options: [], fresh: true });
    mapping.set(def.id, ref);
    created += 1;
  }

  // Formulas the views name: the target's formula of that name, else a new one made after the formulas it reads (a
  // loop, or a formula that reads itself, is made last and stays broken).
  const makeFormula = (def: PropertyDef): void => {
    const config = def.config;
    if (config?.kind !== "formula") {
      return;
    }
    const found = counterpart(def);
    if (found !== undefined) {
      if (found.kind === "formula") {
        mapping.set(def.id, found.ref);
      }
      return;
    }
    if (!room(1)) {
      return;
    }
    const reads = new Map<PropertyId, Ref>();
    for (const read of readsOf(def)) {
      const ref = mapping.get(read);
      if (ref !== undefined) {
        reads.set(read, ref);
      }
    }
    const ref = newRef();
    ops.push({ op: "create", ref, name: def.name, kind: "formula", options: [], formula: { expression: config.expression, reads } });
    addProperty({ ref, name: def.name, kind: "formula", options: [], fresh: true });
    mapping.set(def.id, ref);
    created += 1;
  };
  let pending = input.source.filter((def) => def.kind === "formula" && needed.has(def.id));
  while (pending.length > 0) {
    const waiting = new Set(pending.map((def) => def.id));
    const ready = pending.filter((def) => readsOf(def).every((read) => read === def.id || !waiting.has(read)));
    if (ready.length === 0) {
      pending.forEach(makeFormula);
      break;
    }
    ready.forEach(makeFormula);
    pending = pending.filter((def) => !ready.includes(def));
  }

  return { ops, mapping, values: planned, created, dropped, linksRemoved };
}

// ----- the properties the views name ------------------------------------------------------------

/**
 * The properties the saved views of a page name (`kb-core`'s `Needs::add_view`): the block's attributes and each
 * entry of `views`: columns, sort and the other sort keys of `sorts`, filters (the flat list and the tree), the calendar's dates, the grouping, the column
 * widths and calculations, a tag source (Tags), and the widgets of a dashboard (their own filters, the column of a
 * number, the grouping and the measure of a chart). Only the ids that look like property ids: `title`, `created` and
 * `updated` name no property.
 */
export function viewPropertyIds(attrs: Attrs): Set<string> {
  const found = new Set<string>();
  const take = (value: JsonValue | undefined): void => {
    if (typeof value === "string" && ID_TEXT.test(value)) {
      found.add(value);
    }
  };
  const takeGroup = (group: JsonValue | undefined, depth: number): void => {
    if (!isGroup(group) || depth > MAX_GROUP_DEPTH) {
      return;
    }
    for (const item of group.items) {
      if (isGroup(item)) {
        takeGroup(item, depth + 1);
      } else if (isRecord(item)) {
        take(item.propertyId);
      }
    }
  };
  const read = (one: Attrs, isBlock: boolean): void => {
    if (Array.isArray(one.properties)) {
      one.properties.forEach(take);
    }
    if (isRecord(one.sort)) {
      take(one.sort.by);
    }
    // The other keys of a view's `sorts` (ADR-0058).
    if (Array.isArray(one.sorts)) {
      for (const key of one.sorts) {
        if (isRecord(key)) {
          take(key.by);
        }
      }
    }
    if (Array.isArray(one.filters)) {
      for (const filter of one.filters) {
        if (isRecord(filter)) {
          take(filter.propertyId);
        }
      }
    }
    takeGroup(one.filter, 0);
    take(one.dateBy);
    take(one.endBy);
    take(one.groupBy);
    for (const key of ["columnWidths", "calculations"] as const) {
      const keyed = one[key];
      if (isRecord(keyed)) {
        Object.keys(keyed).forEach(take);
      }
    }
    if (Array.isArray(one.widgets)) {
      for (const widget of one.widgets) {
        if (!isRecord(widget)) {
          continue;
        }
        takeGroup(widget.filter, 0);
        if (isRecord(widget.calc)) {
          take(widget.calc.by);
        }
        if (isRecord(widget.chart)) {
          take(widget.chart.groupBy);
          if (isRecord(widget.chart.measure)) {
            take(widget.chart.measure.propertyId);
          }
        }
      }
    }
    if (isBlock && isRecord(one.source) && one.source.kind === "tag") {
      found.add(TAGS_PROPERTY_ID);
    }
  };
  read(attrs, true);
  if (Array.isArray(attrs.views)) {
    for (const view of attrs.views) {
      if (isRecord(view)) {
        read(view, false);
      }
    }
  }
  return found;
}

// ----- saved views ----------------------------------------------------------------------------

/** What remaps a view's ids once the target's schema exists. */
export interface ViewMap {
  /** The target property of a source property, when it has one. */
  property: (sourceId: string) => string | undefined;
  /** The target option of a source property's option, when it has one. */
  option: (sourcePropertyId: string, optionId: string) => string | undefined;
}

function isRecord(value: JsonValue | undefined): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Sort keys that are not properties. */
const BUILT_IN_SORTS = new Set(["title", "updated", "created"]);

/** The deepest nesting of filter groups followed (reading takes one level; deeper is left as stored). */
const MAX_GROUP_DEPTH = 8;

const ID_TEXT = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu;

/** Whether a stored filter tree node is a group (it has an `items` list) and not a filter. */
function isGroup(node: JsonValue | undefined): node is { [key: string]: JsonValue } & { items: JsonValue[] } {
  return isRecord(node) && Array.isArray(node.items);
}

/**
 * One stored filter with its property and, for a `hasOption` or `lacksOption` (ADR-0058), its option remapped (one id
 * or a list of them: the ones with no counterpart go); `undefined` when it has no counterpart in the target or is no
 * filter. Keys this version does not know are kept, and so is the value of every other operator (the page ids of
 * `relatesTo` and `notRelatesTo`, a day, a relative value such as `{ relative: "today" }`).
 */
function remapFilter(filter: JsonValue, map: ViewMap): JsonValue | undefined {
  if (!isRecord(filter) || typeof filter.propertyId !== "string") {
    return undefined;
  }
  const propertyId = map.property(filter.propertyId);
  if (propertyId === undefined) {
    return undefined;
  }
  const next: { [key: string]: JsonValue } = { ...filter, propertyId };
  if (filter.op === "hasOption" || filter.op === "lacksOption") {
    if (typeof filter.value === "string") {
      const option = map.option(filter.propertyId, filter.value);
      if (option === undefined) {
        return undefined;
      }
      next.value = option;
    } else if (Array.isArray(filter.value)) {
      const options: JsonValue[] = [];
      for (const entry of filter.value) {
        const option = typeof entry === "string" ? map.option(filter.propertyId, entry) : undefined;
        if (option !== undefined) {
          options.push(option);
        }
      }
      if (options.length === 0) {
        return undefined;
      }
      next.value = options;
    }
  }
  return next;
}

/**
 * A stored filter group with its filters remapped, nested groups too. A filter with no counterpart goes; a
 * group that is left with no item after it had some goes with them (`undefined`), one that was empty stays.
 */
function remapGroup(group: JsonValue, map: ViewMap, depth: number): JsonValue | undefined {
  if (!isGroup(group) || depth > MAX_GROUP_DEPTH) {
    return group;
  }
  const items: JsonValue[] = [];
  for (const item of group.items) {
    const next = isGroup(item) ? remapGroup(item, map, depth + 1) : remapFilter(item, map);
    if (next !== undefined) {
      items.push(next);
    }
  }
  return items.length === 0 && group.items.length > 0 ? undefined : { ...group, items };
}

/** A calendar date property as the target has it; `undefined` when it has no counterpart (the calendar then asks to choose). */
function remapDateProperty(value: JsonValue, map: ViewMap): JsonValue | undefined {
  if (typeof value !== "string" || !ID_TEXT.test(value)) {
    return value;
  }
  return map.property(value);
}

/**
 * The attributes of a saved view (`pageList`) with the schema ids of the source space replaced by the
 * target's (`space-move.md` 4 step 5): `properties`, `sort.by`, `sorts` (each key's `by`), `filters` (property and, for
 * `hasOption` and `lacksOption`, the option), `groupBy` with its `hiddenGroups`, `columnWidths`, `calculations` (the table
 * footer's functions, keyed like the widths) and a tag `source`; and, for every view of `views` (ADR-0054, one
 * view being the same attributes with a `filter` tree beside `filters`, and `dateBy` and `endBy`), the same
 * through the same code, so that the mirror of the first view and the view stay equal.
 * An id with no counterpart is removed with what hangs on it (a filter, a group left with no filter, the
 * grouping, a calendar's date, a sort key); the `page` and `space` sources (ADR-0058) are kept as they are (page ids are kept
 * by a move); attributes that name no id are left alone; so is everything a view holds that
 * this version does not understand, and an entry of `views` that is no object.
 */
export function remapView(attrs: Attrs, map: ViewMap): Attrs {
  const out = remapAttrs(attrs, map, true);
  if (Array.isArray(attrs.views)) {
    out.views = attrs.views.map((view) => (isRecord(view) ? remapAttrs(view, map, false) : view));
  }
  return out;
}

/**
 * One widget of a dashboard with its property ids remapped (`dashboards.md` section 5, `kb-core`'s `remap_widget`): its
 * own filter (a filter tree like a view's; one emptied by what has no counterpart goes), the column of a number (`title`
 * stays; with no counterpart the number counts all pages again), the grouping of a chart (`created` and `updated` stay;
 * with no counterpart it asks to choose: `null`) and the measure of a chart (with no counterpart it counts). `id`,
 * `kind`, `title`, `w`, `h`, `viewId` and every key this version does not know are untouched; an entry that is no object
 * is kept as stored.
 */
function remapWidget(widget: JsonValue, map: ViewMap): JsonValue {
  if (!isRecord(widget)) {
    return widget;
  }
  const next: { [key: string]: JsonValue } = { ...widget };
  if (isGroup(widget.filter)) {
    const tree = remapGroup(widget.filter, map, 0);
    if (tree === undefined) {
      delete next.filter;
    } else {
      next.filter = tree;
    }
  }
  const calc = widget.calc;
  if (isRecord(calc) && typeof calc.by === "string" && ID_TEXT.test(calc.by)) {
    const by = map.property(calc.by);
    next.calc = by === undefined ? { ...calc, by: "title", fn: "count_all" } : { ...calc, by };
  }
  const chart = widget.chart;
  if (isRecord(chart)) {
    const after: { [key: string]: JsonValue } = { ...chart };
    if (typeof chart.groupBy === "string" && ID_TEXT.test(chart.groupBy)) {
      after.groupBy = map.property(chart.groupBy) ?? null;
    }
    const measure = chart.measure;
    if (isRecord(measure) && typeof measure.propertyId === "string") {
      const propertyId = map.property(measure.propertyId);
      after.measure = propertyId === undefined ? { fn: "count" } : { ...measure, propertyId };
    }
    next.chart = after;
  }
  return next;
}

/** One block's attributes (`isBlock`) or one view's: a view has no `source` of its own. */
function remapAttrs(attrs: Attrs, map: ViewMap, isBlock: boolean): Attrs {
  const out: Attrs = { ...attrs };
  if (Array.isArray(attrs.properties)) {
    const ids: JsonValue[] = [];
    for (const entry of attrs.properties) {
      const id = typeof entry === "string" ? map.property(entry) : undefined;
      if (id !== undefined && !ids.includes(id)) {
        ids.push(id);
      }
    }
    out.properties = ids;
  }
  const sort = attrs.sort;
  if (isRecord(sort)) {
    const by = typeof sort.by === "string" ? sort.by : "";
    const id = BUILT_IN_SORTS.has(by) ? by : map.property(by);
    if (id === undefined) {
      delete out.sort;
    } else {
      out.sort = { ...sort, by: id };
    }
  }
  // The sort keys of a view (ADR-0058): each key follows like `sort.by`; a key with no counterpart goes, and `sort`,
  // which mirrors the first key, then follows the first key left (both go when none is left). An entry that is no
  // object or names no `by` is kept as stored, like `kb-core`'s `remap_sort_key`.
  const sorts = attrs.sorts;
  if (Array.isArray(sorts)) {
    const kept: JsonValue[] = [];
    for (const key of sorts) {
      if (!isRecord(key) || key.by === undefined) {
        kept.push(key);
        continue;
      }
      const by = typeof key.by === "string" ? key.by : "";
      const id = BUILT_IN_SORTS.has(by) ? by : map.property(by);
      if (id !== undefined) {
        kept.push({ ...key, by: id });
      }
    }
    if (kept.length === 0 && sorts.length > 0) {
      delete out.sorts;
      delete out.sort;
    } else {
      out.sorts = kept;
      const first = kept[0];
      if (kept.length < sorts.length && isRecord(first) && attrs.sort !== undefined) {
        out.sort = first;
      }
    }
  }
  if (Array.isArray(attrs.filters)) {
    const filters: JsonValue[] = [];
    for (const filter of attrs.filters) {
      const next = remapFilter(filter, map);
      if (next !== undefined) {
        filters.push(next);
      }
    }
    if (filters.length > 0) {
      out.filters = filters;
    } else {
      delete out.filters;
    }
  }
  // The filter tree of a view; one that is emptied by what has no counterpart goes (an absent one is the
  // empty AND group, which says nothing).
  if (isGroup(attrs.filter)) {
    const tree = remapGroup(attrs.filter, map, 0);
    if (tree === undefined) {
      delete out.filter;
    } else {
      out.filter = tree;
    }
  }
  // The calendar's date properties; one with no counterpart goes (`created` and `updated` are no property).
  for (const key of ["dateBy", "endBy"] as const) {
    const stored = attrs[key];
    if (stored !== undefined) {
      const next = remapDateProperty(stored, map);
      if (next === undefined) {
        delete out[key];
      } else {
        out[key] = next;
      }
    }
  }
  if (typeof attrs.groupBy === "string") {
    const groupBy = map.property(attrs.groupBy);
    if (groupBy === undefined) {
      delete out.groupBy;
      delete out.hiddenGroups;
    } else {
      out.groupBy = groupBy;
      if (Array.isArray(attrs.hiddenGroups)) {
        const hidden: JsonValue[] = [];
        for (const entry of attrs.hiddenGroups) {
          const id = entry === "none" ? "none" : typeof entry === "string" ? map.option(String(attrs.groupBy), entry) : undefined;
          if (id !== undefined) {
            hidden.push(id);
          }
        }
        if (hidden.length > 0) {
          out.hiddenGroups = hidden;
        } else {
          delete out.hiddenGroups;
        }
      }
    }
  }
  if (isRecord(attrs.columnWidths)) {
    const widths: { [key: string]: JsonValue } = {};
    for (const [key, width] of Object.entries(attrs.columnWidths)) {
      const id = key === "title" ? "title" : map.property(key);
      if (id !== undefined) {
        widths[id] = width;
      }
    }
    if (Object.keys(widths).length > 0) {
      out.columnWidths = widths;
    } else {
      delete out.columnWidths;
    }
  }
  if (isRecord(attrs.calculations)) {
    const calculations: { [key: string]: JsonValue } = {};
    for (const [key, fn] of Object.entries(attrs.calculations)) {
      const id = key === "title" ? "title" : map.property(key);
      if (id !== undefined) {
        calculations[id] = fn;
      }
    }
    if (Object.keys(calculations).length > 0) {
      out.calculations = calculations;
    } else {
      delete out.calculations;
    }
  }
  // The widgets of a dashboard (ADR-0055): each widget's ids follow like a view's.
  if (Array.isArray(attrs.widgets)) {
    out.widgets = attrs.widgets.map((widget) => remapWidget(widget, map));
  }
  const source = attrs.source;
  if (isBlock && isRecord(source) && source.kind === "tag" && typeof source.optionId === "string") {
    const option = map.option(TAGS_PROPERTY_ID, source.optionId);
    if (option === undefined) {
      delete out.source;
    } else {
      out.source = { ...source, optionId: option };
    }
  }
  return out;
}
