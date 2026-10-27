// The in-memory twin of the core's bulk edit of database rows (ADR-0058, `kb-core`'s `bulk.rs`,
// `docs/architecture/database-power-ups.md` section 5.4): `PageBulk` over the mirror's structure, properties and
// duplication, with the core's rules.
//
// One action on a set of pages of one space, all or nothing: the pages are checked first (1 to 1,000, none twice, each a live
// page of the space: `InvalidInput` for `pageIds`), then the action's own arguments, and nothing is written until all of it
// holds. The role gate (Editor in a shared space) is the transport's `#authorise`.
//
// - `SetValue` and `SetValues` write every page's value through the value path of `PagePropertySet`
//   (`MemoryProperties.pageSet`); every value is checked first. Property values are not in structural undo.
// - `Move`, `Duplicate` and `Trash` first drop the pages whose ancestor is in the list (they go with it), then write all the
//   pages as ONE structural undo entry (`MemoryStructure.moveMany`, `MemoryDuplicate.duplicateMany`, `trashMany`).
//
// The answer is the top copies for a duplicate, in order, and the pages acted on otherwise. Events come once each, as the core
// announces them: `StructureChanged` for every action, `SpacesChanged` too for a duplicate and a trash (the page count changed;
// the twin's single `PageTrash` leaves it out), `BacklinksChanged` for what the trashed pages referenced, and for a duplicate
// the events of the new documents.

import type { BulkAction } from "./generated/BulkAction";
import type { PageId } from "./generated/PageId";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { MemoryContent } from "./content";
import type { MemoryDuplicate } from "./memory-duplicate";
import type { MemoryProperties } from "./memory-properties";
import type { MemoryStructure } from "./structure";
import { invalid } from "./validate";
import { CoreError } from "./errors";

/** Most pages one `PageBulk` request acts on, and most pages one bulk duplicate copies (ADR-0058). */
export const MAX_BULK_PAGES = 1_000;

/** What the bulk edit needs from its owner. */
export interface BulkDeps {
  /** Runs `run`, then delivers the events it caused once each (in order of first occurrence). */
  collect: <T>(run: () => T) => T;
  structure: MemoryStructure;
  properties: MemoryProperties;
  duplicate: MemoryDuplicate;
  content: MemoryContent;
}

/** The bulk edits of one core. */
export class MemoryBulk {
  readonly #deps: BulkDeps;

  constructor(deps: BulkDeps) {
    this.#deps = deps;
  }

  /**
   * `PageBulk`: applies `action` to `pageIds` of `spaceId`. Answers the top copies for a duplicate, else the pages acted on.
   * `NotFound` for an unknown space; `InvalidInput` for `pageIds`, `values`, `parentId`, or a value the property refuses (as
   * `PagePropertySet`); a refused request writes nothing.
   */
  bulk(spaceId: SpaceId, pageIds: readonly PageId[], action: BulkAction): PageId[] {
    const { structure } = this.#deps;
    if (structure.spacePageIds(spaceId) === undefined) {
      throw new CoreError({ code: "NotFound", message: "space not found" });
    }
    this.#checkPages(spaceId, pageIds);
    switch (action.type) {
      case "SetValue": {
        const { propertyId, value } = action.data;
        this.#setValues(spaceId, pageIds, propertyId, pageIds.map(() => value));
        return [...pageIds];
      }
      case "SetValues": {
        const { propertyId, values } = action.data;
        if (values.length !== pageIds.length) {
          throw invalid("values", "there must be one value for each page");
        }
        this.#setValues(spaceId, pageIds, propertyId, values);
        return [...pageIds];
      }
      case "Move": {
        const tops = structure.withoutDescendants(spaceId, pageIds);
        structure.moveMany(spaceId, tops, action.data.parentId);
        return tops;
      }
      case "Duplicate":
        return this.#deps.duplicate.duplicateMany(spaceId, structure.withoutDescendants(spaceId, pageIds));
      case "Trash": {
        const tops = structure.withoutDescendants(spaceId, pageIds);
        this.#deps.collect(() => {
          structure.trashMany(spaceId, tops);
          // Backlinks list live sources only: what the trashed pages referenced changes.
          this.#deps.content.sourcesMoved(tops.flatMap((top) => structure.subtreeIds(top)));
        });
        return tops;
      }
      default:
        // An action this build does not know (a newer client): refused, nothing written.
        throw invalid("action", "unknown bulk action");
    }
  }

  /** The pages of a request: 1 to {@link MAX_BULK_PAGES}, none twice, each a live page of the space. */
  #checkPages(spaceId: SpaceId, pageIds: readonly PageId[]): void {
    if (pageIds.length === 0) {
      throw invalid("pageIds", "name at least one page");
    }
    if (pageIds.length > MAX_BULK_PAGES) {
      throw invalid("pageIds", `at most ${String(MAX_BULK_PAGES)} pages at a time`);
    }
    const seen = new Set<PageId>();
    for (const pageId of pageIds) {
      if (seen.has(pageId)) {
        throw invalid("pageIds", "a page is named twice");
      }
      seen.add(pageId);
      const page = this.#deps.structure.pageMeta(pageId);
      if (page === undefined || page.spaceId !== spaceId || page.trashed) {
        throw invalid("pageIds", "a page is not a live page of this space");
      }
    }
  }

  /** One property of many pages, every value checked before any is written; the events come once each. */
  #setValues(
    spaceId: SpaceId,
    pageIds: readonly PageId[],
    propertyId: PropertyId,
    values: readonly (PropertyValue | null)[],
  ): void {
    const { properties } = this.#deps;
    for (const value of values) {
      properties.checkValue(spaceId, propertyId, value);
    }
    this.#deps.collect(() => {
      pageIds.forEach((pageId, at) => {
        properties.pageSet(pageId, propertyId, values[at] ?? null);
      });
    });
  }
}
