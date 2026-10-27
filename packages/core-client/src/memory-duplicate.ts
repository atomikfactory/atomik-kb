// The in-memory twin of the core's duplication of a page with its sub-pages (ADR-0040, `kb-core`'s
// `duplicate.rs`) and of its making a page from a template (ADR-0056, `template_create.rs`): `PageDuplicate` and
// `PageCreateFromTemplate` over the mirror's structure, properties and documents, with the core's semantics and a
// reduced fidelity (no keys, no sync entries).
//
// What a duplication does, as the core does it: the live subtree of the page is copied (a trashed sub-page is not
// in it) with new page ids, new block ids and the same icons; the top copy goes right after the page among its
// siblings and is titled with " (copy)" appended (English only, as in the core), the sub-pages keep their titles; the
// template flag and the placeholders are copied (the duplicate of a template is a template); the property values are
// copied through the normal value path (a relation value that lists a page of the copied
// subtree lists its copy, any other related page is kept, a two-way relation keeps both sides paired); the content
// is the portable JSON of the original with the ids replaced (`./page-copy`: links into the subtree follow the
// copies, drawings follow their blocks) built into a new document, the copy of a page without a document being an
// empty page; attachments are shared by id (the copies' blocks, icons and values reference them). Favourites are not
// copied. The creation of the pages is one undo step (`MemoryStructure.duplicate`); values and documents are not.
//
// What a creation from a template does: the same copy of the template's subtree, placed under the chosen parent at the
// chosen index, with no flag and no placeholders on the copies, the tokens `{{date}}` and `{{time}}` of every title
// resolved (no " (copy)"), the placeholders of each page resolved into values (`today` the local day, `now` the day
// and time, `me` the account), the request's values written on the top copy (they win over a placeholder of the same
// property), and in the documents the relative date and person mentions resolved (`./page-copy`,
// `fillTemplateContent`); a template button's content is copied as it is. Every check comes first: a refused request
// writes nothing.
//
// Events, once each in this order: `StructureChanged`, `SpacesChanged`, then the writes of the values and the
// documents (`PageContentChanged { source: "Core" }` for every new page, `BacklinksChanged` for the pages the copies
// link to, the attachment events).

import type { MemoryAttachments } from "./attachment-mirror";
import type { MemoryContent } from "./content";
import { CoreError, internalError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { PageId } from "./generated/PageId";
import type { PageInfo } from "./generated/PageInfo";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { MemoryProperties } from "./memory-properties";
import {
  type TemplateFill,
  fillTemplateContent,
  localMoment,
  portableBlockIds,
  remapPortable,
} from "./page-copy";
import {
  isComputedPropertyKind,
  normalisePropertyValue,
  propertyValueKind,
  propertyValueOptions,
} from "./properties";
import { MAX_DUPLICATED_PAGES, type MemoryStructure, placeholderTakes } from "./structure";
import { coverAttachment, iconAttachment, invalid, position } from "./validate";

/** What the copy of a page's title gets appended (English only: the core has no translations). */
const COPY_SUFFIX = " (copy)";
/** The longest page title, in characters. */
const MAX_TITLE_CHARS = 500;

/**
 * The title of the copy of a page: the title and " (copy)", the title cut to fit the limit of a page title when it
 * would not. An untitled page's copy is titled "(copy)". The core's `copy_title`.
 */
export function copyTitle(title: string): string {
  const room = MAX_TITLE_CHARS - Array.from(COPY_SUFFIX).length;
  const kept = Array.from(title).slice(0, room).join("").trimEnd();
  return `${kept}${COPY_SUFFIX}`.trimStart();
}

/** What the duplication mirror needs from its owner. */
export interface DuplicateDeps {
  /** Runs `run`, then delivers the events it caused once each (in order of first occurrence). */
  collect: <T>(run: () => T) => T;
  /** Id generator (UUID text). */
  newId: () => string;
  /** Clock in Unix milliseconds: the moment a page made from a template resolves its placeholders to. */
  now: () => number;
  /**
   * How far the device's wall clock is ahead of UTC at a time given in Unix ms (east positive): the local day and time of
   * a page made from a template. Default: the JavaScript date's own offset.
   */
  localOffsetSeconds?: (atMs: number) => number;
  /** This account, which a `me` placeholder stands for; none leaves `me` unresolved (the core before its kit exists). */
  me?: () => AccountId | undefined;
  /** The name an account goes by in a space (`undefined` for a former member): the label of a `me` mention. */
  personName?: (spaceId: SpaceId, accountId: AccountId) => string | undefined;
  /** The accounts a Person value of a space may list (its members); without it any account is accepted. */
  people?: (spaceId: SpaceId) => readonly AccountId[];
  structure: MemoryStructure;
  properties: MemoryProperties;
  content: MemoryContent;
  attachments: MemoryAttachments;
}

/** The copy of `key`, or an internal error: every page of the subtree has one. */
function copyOf(ids: ReadonlyMap<PageId, PageId>, key: PageId): PageId {
  const copy = ids.get(key);
  if (copy === undefined) {
    throw internalError("a page of the subtree has no copy");
  }
  return copy;
}

/** The duplications and the creations from a template of one core. */
export class MemoryDuplicate {
  readonly #deps: DuplicateDeps;

  constructor(deps: DuplicateDeps) {
    this.#deps = deps;
  }

  /**
   * `PageDuplicate`: copies a live page with its live sub-pages and answers the copy of the page. `NotFound` for an
   * unknown page; `InvalidInput` for `pageId` when it is in the trash or its subtree has more than 1,000 pages. A
   * refused request writes nothing.
   */
  duplicate(pageId: PageId): PageInfo {
    const { structure } = this.#deps;
    const { spaceId, pageIds } = structure.duplicateCheck(pageId);
    const title = structure.pageMeta(pageId)?.title ?? "";
    const { ids, documents } = this.#prepare(pageIds, undefined);
    return this.#deps.collect(() => {
      // 1. The pages: one undo step.
      const copies = structure.duplicate(pageId, ids, copyTitle(title));
      this.#writeCopies(spaceId, pageIds, ids, copies, documents, () => undefined);
      return structure.pageGet(copyOf(ids, pageId));
    });
  }

  /**
   * `PageBulk` `Duplicate` (ADR-0058): copies each of the live pages `tops` of a space (none an ancestor of another) with its
   * live sub-pages as {@link MemoryDuplicate.duplicate} copies one: the top copy right after its page and titled with
   * " (copy)", values, relations, content and attachment references as for one page. The pages of all the copies are ONE undo
   * entry. Answers the copy of each top, in the order given. `InvalidInput` for `pageIds` when more than 1,000 pages would be
   * copied in all (checked before anything is written; the caller has checked that the pages are live pages of the space).
   */
  duplicateMany(spaceId: SpaceId, tops: readonly PageId[]): PageId[] {
    const { structure } = this.#deps;
    const groups = tops.map((top) => ({
      top,
      pageIds: structure.subtreeIds(top),
      topTitle: copyTitle(structure.pageMeta(top)?.title ?? ""),
    }));
    const pageIds = groups.flatMap((group) => group.pageIds);
    if (pageIds.length > MAX_DUPLICATED_PAGES) {
      throw invalid("pageIds", `at most ${String(MAX_DUPLICATED_PAGES)} pages are duplicated at a time`);
    }
    const { ids, documents } = this.#prepare(pageIds, undefined);
    return this.#deps.collect(() => {
      const copies = structure.duplicateMany(spaceId, groups, ids);
      this.#writeCopies(spaceId, pageIds, ids, copies, documents, () => undefined);
      return groups.map((group) => copyOf(ids, group.top));
    });
  }

  /**
   * `PageCreateFromTemplate` (ADR-0056): a new page (and sub-pages) from a template, placed under `parentId` (the
   * space's top level when absent) at `index` (last when absent, clamped), placeholders resolved, `values` written on the
   * top copy. `NotFound` for an unknown page; `InvalidInput` for `templateId` (not a template, in the trash, more than
   * 1,000 pages), `parentId` (not a live page of the template's space, or inside the template) and `values` (a value
   * `PagePropertySet` would refuse). A refused request writes nothing.
   */
  createFromTemplate(
    templateId: PageId,
    parentId: PageId | null | undefined,
    index: number | undefined,
    values: readonly (readonly [PropertyId, PropertyValue])[],
  ): PageInfo {
    const { structure } = this.#deps;
    const { spaceId, pageIds } = structure.templateCheck(templateId, parentId);
    const place = index === undefined ? undefined : position(index);
    this.#checkValues(spaceId, values);

    // The moment and the person the placeholders stand for.
    const nowMs = this.#deps.now();
    const offset = this.#deps.localOffsetSeconds?.(nowMs) ?? -new Date(nowMs).getTimezoneOffset() * 60;
    const moment = localMoment(nowMs, offset);
    const me = this.#deps.me?.();
    const fill: TemplateFill = {
      moment,
      me: me === undefined ? undefined : { account: me, label: this.#deps.personName?.(spaceId, me) },
    };
    const { ids, documents } = this.#prepare(pageIds, fill);
    const kinds = new Map(this.#deps.properties.kindsOf(spaceId).map((def) => [def.id, def.kind]));

    return this.#deps.collect(() => {
      // 1. The pages: one undo step.
      const copies = structure.createFromTemplate(templateId, ids, { parentId, index: place, moment });
      this.#writeCopies(spaceId, pageIds, ids, copies, documents, () => {
        // 2b. The placeholders of every page, then the request's values on the top copy.
        for (const source of pageIds) {
          for (const [propertyId, placeholder] of structure.templateOf(source)?.values ?? []) {
            const kind = kinds.get(propertyId);
            if (kind === undefined || !placeholderTakes(placeholder, kind)) {
              continue;
            }
            const value: PropertyValue | undefined =
              placeholder === "today"
                ? { kind: "date", value: { date: moment.date } }
                : placeholder === "now"
                  ? { kind: "date", value: { date: moment.date, time: moment.time } }
                  : me === undefined
                    ? undefined
                    : { kind: "person", value: [me] };
            if (value !== undefined) {
              this.#setValue(copyOf(ids, source), propertyId, value);
            }
          }
        }
        for (const [propertyId, value] of values) {
          this.#deps.properties.pageSet(copyOf(ids, templateId), propertyId, value);
        }
      });
      return structure.pageGet(copyOf(ids, templateId));
    });
  }

  /**
   * Everything that can fail is read before anything is written: the new ids, the content of every page rebuilt with them
   * (and with its relative mentions resolved by `fill`, for a page made from a template).
   */
  #prepare(
    pageIds: readonly PageId[],
    fill: TemplateFill | undefined,
  ): { ids: Map<PageId, PageId>; documents: JsonValue[] } {
    const { content, newId } = this.#deps;
    const ids = new Map<PageId, PageId>(pageIds.map((source) => [source, newId()]));
    const stored = pageIds.map((source) => content.storedContent(source));
    const blocks = new Map<string, string>();
    for (const json of stored) {
      for (const block of json === undefined ? [] : portableBlockIds(json)) {
        if (!blocks.has(block)) {
          blocks.set(block, newId());
        }
      }
    }
    const documents: JsonValue[] = stored.map((json): JsonValue => {
      if (json === undefined) {
        return { type: "doc", content: [{ type: "paragraph", attrs: { id: newId() } }] };
      }
      const remapped = remapPortable(json, { pages: ids, blocks });
      return fill === undefined ? remapped : fillTemplateContent(remapped, fill);
    });
    return { ids, documents };
  }

  /**
   * What follows the creation of the pages (`copies`, parents first): the attachment references of icons and covers, the
   * values copied through the value path, `afterValues` (the placeholders and the request's values of a page made from a
   * template), then the documents, announced as the core's own change.
   */
  #writeCopies(
    spaceId: SpaceId,
    pageIds: readonly PageId[],
    ids: ReadonlyMap<PageId, PageId>,
    copies: readonly PageId[],
    documents: readonly JsonValue[],
    afterValues: () => void,
  ): void {
    const { structure, properties, content, attachments } = this.#deps;
    attachments.refresh(
      copies.flatMap((copy) => {
        const meta = structure.pageMeta(copy);
        return [iconAttachment(meta?.icon), coverAttachment(meta?.cover)].filter((id): id is string => id !== undefined);
      }),
    );

    // 2. The values, through the value path.
    for (const source of pageIds) {
      const copy = copyOf(ids, source);
      for (const [propertyId, value] of properties.valuesOf(spaceId, source)) {
        if (isComputedPropertyKind(propertyValueKind(value))) {
          continue;
        }
        const mapped: PropertyValue =
          value.kind === "relation" ? { kind: "relation", value: value.value.map((page) => ids.get(page) ?? page) } : value;
        this.#setValue(copy, propertyId, mapped);
      }
    }
    afterValues();

    // 3. The documents, announced as the core's own change.
    pageIds.forEach((source, at) => {
      content.createDocument(copyOf(ids, source), documents[at] ?? { type: "doc" });
    });
  }

  /** Writes a value of a copy through the value path; one its own rules refuse now is left out rather than failing half way. */
  #setValue(copy: PageId, propertyId: PropertyId, value: PropertyValue): void {
    try {
      this.#deps.properties.pageSet(copy, propertyId, value);
    } catch (error) {
      if (!(error instanceof CoreError) || error.code !== "InvalidInput") {
        throw error;
      }
    }
  }

  /**
   * Every value of `values` would be accepted by `PagePropertySet` on a page of the space (`InvalidInput` for `values`
   * otherwise): the property exists and is not computed, the kind is the property's, the value passes its own rules, the
   * options it names are listed, the people it lists are members of the space.
   */
  #checkValues(spaceId: SpaceId, values: readonly (readonly [PropertyId, PropertyValue])[]): void {
    if (values.length === 0) {
      return;
    }
    const defs = this.#deps.properties.schemaGet(spaceId);
    for (const [propertyId, value] of values) {
      const def = defs.find((candidate) => candidate.id === propertyId);
      if (def === undefined) {
        throw invalid("values", "the property does not exist");
      }
      if (isComputedPropertyKind(def.kind)) {
        throw invalid("values", "this property is computed from the page and cannot be set");
      }
      if (propertyValueKind(value) !== def.kind) {
        throw invalid("values", "the value does not match the kind of the property");
      }
      let stored: PropertyValue | null;
      try {
        stored = normalisePropertyValue(value);
      } catch (error) {
        throw error instanceof CoreError ? invalid("values", error.message) : error;
      }
      if (stored === null) {
        continue;
      }
      if (!propertyValueOptions(stored).every((id) => def.options.some((option) => option.id === id))) {
        throw invalid("values", "the value names an unknown option");
      }
      if (stored.kind === "person") {
        const members = this.#deps.people?.(spaceId);
        if (members !== undefined && !stored.value.every((id) => members.includes(id))) {
          throw invalid("values", "a person of the list is not a member of this space");
        }
      }
    }
  }
}
