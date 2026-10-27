// The in-memory twin of the core's move of a page and its subpages to another space (ADR-0038,
// `docs/architecture/space-move.md`): `PageMoveToSpacePlan` and `PageMoveToSpace` over the mirror's
// structure, properties, documents and attachments, with reduced fidelity: no keys, no residue nodes
// and no reconciliation of concurrent moves (there is one replica).
//
// What a move does, as the core does it: the page and its live subtree keep their ids and arrive in the
// target space under the parent at the index (`MemoryStructure.moveToSpace`: titles, icons and times
// kept, favourites follow, not an undo step); their property values follow by property name (missing
// properties and options are created, a value that cannot follow is counted, relation values keep the
// pages that move along: `./memory-move-values`); the relation entries of the pages that stay which list a
// moved page are removed; the saved views (`pageList` blocks) of the moved pages have their schema ids
// remapped; the attachments that only moved pages use change space (the others stay and are counted as
// left). The plan request answers the same counts and writes nothing.
//
// Events, once each in this order: `PagesMovedSpace`, `StructureChanged` for the source then the target,
// `SpacesChanged`, `PropertySchemaChanged` and the writes of the target's schema, `AttachmentsChanged` for
// the source then the target (when an attachment moved), `PageContentChanged { source: "Core" }` for each
// document whose views changed, then `BacklinksChanged` for the pages the moved ones reference (their
// backlink entries name another space now).

import type { MemoryAttachments } from "./attachment-mirror";
import type { MemoryContent } from "./content";
import { CoreError } from "./errors";
import { remapFormulaIds } from "./formula";
import type { AttachmentId } from "./generated/AttachmentId";
import type { PageId } from "./generated/PageId";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { SpaceMoveSummary } from "./generated/SpaceMoveSummary";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import { planProperties, remapView, viewPropertyIds } from "./memory-move-values";
import type { PlannedValue, PropertyPlan, Ref, ViewMap } from "./memory-move-values";
import type { MemoryProperties } from "./memory-properties";
import { TAGS_PROPERTY_ID } from "./properties";
import { ROLLUP_TITLE_TARGET } from "./rollup";
import { placeholderTakes } from "./structure";
import type { MemoryStructure } from "./structure";
import { position } from "./validate";

/** What the move mirror needs from its owner. */
export interface MoveDeps {
  /** Runs `run`, then delivers the events it caused once each (in order of first occurrence). */
  collect: <T>(run: () => T) => T;
  structure: MemoryStructure;
  properties: MemoryProperties;
  content: MemoryContent;
  attachments: MemoryAttachments;
}

/** Everything a move needs, read before anything is written. */
interface MovePlan {
  fromSpaceId: SpaceId;
  pageIds: PageId[];
  /** The source schema as it was (the views' ids are read against it). */
  sourceSchema: PropertyDef[];
  properties: PropertyPlan;
  /** The attachments that move with the pages. */
  attachments: AttachmentId[];
  summary: SpaceMoveSummary;
}

/** A name as options are matched. */
function fold(name: string): string {
  return name.trim().toLowerCase();
}

/** The moves of one core. */
export class MemoryMove {
  readonly #deps: MoveDeps;

  constructor(deps: MoveDeps) {
    this.#deps = deps;
  }

  /**
   * `PageMoveToSpacePlan`: the checks of the move and its counts, nothing written. `InvalidInput` for
   * `pageId` (in the trash, or more than 10,000 pages), `spaceId` (the page's own space) or `parentId`
   * (not a live page of the target); `NotFound` for an unknown page or space.
   */
  plan(pageId: PageId, spaceId: SpaceId, parentId: PageId | null | undefined): SpaceMoveSummary {
    return this.#plan(pageId, spaceId, parentId).summary;
  }

  /** `PageMoveToSpace`: the checks of {@link MemoryMove.plan}, then the move; answers what it did. */
  move(
    pageId: PageId,
    spaceId: SpaceId,
    parentId: PageId | null | undefined,
    index: number | null | undefined,
  ): SpaceMoveSummary {
    const at = index === null || index === undefined ? undefined : position(index);
    const plan = this.#plan(pageId, spaceId, parentId);
    return this.#deps.collect(() => {
      const { structure, properties, content, attachments } = this.#deps;
      const moved = new Set(plan.pageIds);
      structure.moveToSpace(pageId, spaceId, parentId, at);
      const resolve = this.#applySchema(plan, spaceId);
      this.#applyValues(plan, spaceId, resolve);
      this.#applyTemplates(plan, spaceId, resolve);
      properties.unlinkPages(plan.fromSpaceId, moved, true);
      attachments.moveToSpace(plan.attachments, plan.fromSpaceId, spaceId);
      const map = this.#viewMap(plan, spaceId, resolve);
      content.remapViews(plan.pageIds, (attrs) => remapView(attrs, map));
      content.sourcesMoved(plan.pageIds);
      return plan.summary;
    });
  }

  // ----- the plan ---------------------------------------------------------------------------

  #plan(pageId: PageId, spaceId: SpaceId, parentId: PageId | null | undefined): MovePlan {
    const { structure, properties, attachments } = this.#deps;
    const { fromSpaceId, pageIds } = structure.moveCheck(pageId, spaceId, parentId);
    const moved = new Set(pageIds);
    const sourceSchema = properties.schemaGet(fromSpaceId);
    const plan = planProperties({
      source: sourceSchema,
      target: properties.schemaGet(spaceId),
      values: properties.movingValues(fromSpaceId, pageIds),
      moved,
      viewProperties: this.#viewProperties(pageIds),
      placeholderProperties: this.#placeholderProperties(sourceSchema, pageIds),
      titleOf: (id) => structure.pageMeta(id)?.title,
      fileName: (id) => attachments.nameOf(id),
    });
    const stay = properties.unlinkPages(fromSpaceId, moved, false);
    const carried = this.#attachmentsOf(fromSpaceId, moved);
    return {
      fromSpaceId,
      pageIds,
      sourceSchema,
      properties: plan,
      attachments: carried.moving,
      summary: {
        pageIds: [...pageIds],
        attachmentsMoved: carried.moving.length,
        attachmentsLeft: carried.left,
        propertiesCreated: plan.created,
        valuesDropped: plan.dropped,
        relationLinksRemoved: plan.linksRemoved + stay.removed,
      },
    };
  }

  /**
   * The source properties the placeholders of the moved pages sit on (ADR-0056), when the property takes the placeholder:
   * the plan needs them like a property that has a value, so the target gets one of that name when it has none.
   */
  #placeholderProperties(sourceSchema: readonly PropertyDef[], pageIds: readonly PageId[]): Set<PropertyId> {
    const kinds = new Map(sourceSchema.map((def) => [def.id, def.kind]));
    const needed = new Set<PropertyId>();
    for (const pageId of pageIds) {
      for (const [propertyId, placeholder] of this.#deps.structure.templateOf(pageId)?.values ?? []) {
        const kind = kinds.get(propertyId);
        if (kind !== undefined && placeholderTakes(placeholder, kind)) {
          needed.add(propertyId);
        }
      }
    }
    return needed;
  }

  /**
   * The properties the saved views of the moved pages name (`Needs::add_view`), widgets of a dashboard included:
   * read through `remapViews` with a function that changes nothing, which writes nothing.
   */
  #viewProperties(pageIds: readonly PageId[]): Set<PropertyId> {
    const named = new Set<PropertyId>();
    this.#deps.content.remapViews(pageIds, (attrs) => {
      viewPropertyIds(attrs).forEach((id) => named.add(id));
      return attrs;
    });
    return named;
  }

  /**
   * The attachments of the source space that the moved pages use (in blocks and galleries, `files`
   * values and icons): those whose every reference is a moved page move, the others stay.
   */
  #attachmentsOf(fromSpaceId: SpaceId, moved: ReadonlySet<PageId>): { moving: AttachmentId[]; left: number } {
    const { structure, properties, content, attachments } = this.#deps;
    const referrers = new Map<AttachmentId, Set<PageId>>();
    for (const source of [content.attachmentReferrers(), properties.attachmentReferrers(), structure.iconReferrers()]) {
      for (const [attachmentId, pages] of source) {
        const into = referrers.get(attachmentId) ?? new Set<PageId>();
        pages.forEach((id) => into.add(id));
        referrers.set(attachmentId, into);
      }
    }
    const moving: AttachmentId[] = [];
    let left = 0;
    for (const [attachmentId, pages] of referrers) {
      if (attachments.manifestOf(attachmentId)?.spaceId !== fromSpaceId || ![...pages].some((id) => moved.has(id))) {
        continue;
      }
      if ([...pages].every((id) => moved.has(id))) {
        moving.push(attachmentId);
      } else {
        left += 1;
      }
    }
    return { moving, left };
  }

  // ----- the writes -------------------------------------------------------------------------

  /** Applies the schema changes of the plan to the target; answers how a planned property is named in it. */
  #applySchema(plan: MovePlan, spaceId: SpaceId): (ref: Ref) => PropertyId {
    const { properties } = this.#deps;
    const real = new Map<Ref, PropertyId>();
    const resolve = (ref: Ref): PropertyId => real.get(ref) ?? ref;
    for (const op of plan.properties.ops) {
      if (op.op === "option") {
        properties.optionAdd(spaceId, resolve(op.property), op.option.name, op.option.color, op.option.group);
        continue;
      }
      let config: PropertyConfig | undefined;
      if (op.rollup !== undefined) {
        config = {
          kind: "rollup",
          relationPropertyId: resolve(op.rollup.relation),
          target: op.rollup.target === ROLLUP_TITLE_TARGET ? ROLLUP_TITLE_TARGET : resolve(op.rollup.target),
          function: op.rollup.function,
        };
      } else if (op.kind === "relation") {
        config = { kind: "relation", twoWay: op.pair !== undefined };
      } else if (op.formula !== undefined) {
        const reads = op.formula.reads;
        config = {
          kind: "formula",
          expression: remapFormulaIds(op.formula.expression, (id) => {
            const ref = reads.get(id);
            return ref === undefined ? undefined : resolve(ref);
          }),
        };
      }
      let def: PropertyDef;
      try {
        def = properties.create(spaceId, op.name, op.kind, config);
      } catch (error) {
        // A formula that does not check in the target (something it reads has no counterpart there) moves as written
        // and stays broken: no value, and its settings show the error (ADR-0055).
        if (config?.kind !== "formula" || !(error instanceof CoreError) || error.code !== "InvalidInput") {
          throw error;
        }
        def = properties.createFormulaUnchecked(spaceId, op.name, config.expression);
      }
      real.set(op.ref, def.id);
      if (op.pair !== undefined && def.config?.kind === "relation" && def.config.pairedPropertyId !== undefined) {
        const backId = def.config.pairedPropertyId;
        real.set(op.pair.ref, backId);
        const back = properties.schemaGet(spaceId).find((candidate) => candidate.id === backId);
        if (back !== undefined && back.name !== op.pair.name) {
          properties.rename(spaceId, backId, op.pair.name);
        }
      }
      if (op.kind === "status") {
        // The three default options give way to the source's.
        for (const option of op.options) {
          const found = def.options.find((candidate) => fold(candidate.name) === fold(option.name));
          if (found === undefined) {
            properties.optionAdd(spaceId, def.id, option.name, option.color, option.group);
          } else {
            properties.optionUpdate(spaceId, def.id, found.id, { color: option.color, group: option.group });
          }
        }
        for (const given of def.options) {
          if (!op.options.some((option) => fold(option.name) === fold(given.name))) {
            properties.optionDelete(spaceId, def.id, given.id);
          }
        }
      } else {
        for (const option of op.options) {
          properties.optionAdd(spaceId, def.id, option.name, option.color, undefined);
        }
      }
    }
    return resolve;
  }

  /** The values of the moved pages leave the source and are stored in the target. */
  #applyValues(plan: MovePlan, spaceId: SpaceId, resolve: (ref: Ref) => PropertyId): void {
    const { properties } = this.#deps;
    properties.takeValues(plan.fromSpaceId, plan.pageIds);
    const schema = new Map(properties.schemaGet(spaceId).map((def) => [def.id, def]));
    for (const { pageId, property, value } of plan.properties.values) {
      const propertyId = resolve(property);
      const stored = this.#stored(value, schema.get(propertyId));
      if (stored !== undefined) {
        properties.putValue(spaceId, pageId, propertyId, stored);
      }
    }
  }

  /**
   * The placeholders of the moved pages follow the property mapping like the values (ADR-0056; the flag travels with the
   * page by itself; the plan needs the properties they sit on, so the target has them by now): a placeholder whose
   * property has no counterpart in the target (its schema is full), or whose counterpart's kind does not take it, is
   * dropped.
   */
  #applyTemplates(plan: MovePlan, spaceId: SpaceId, resolve: (ref: Ref) => PropertyId): void {
    const { structure, properties } = this.#deps;
    const kinds = new Map(properties.schemaGet(spaceId).map((def) => [def.id, def.kind]));
    for (const pageId of plan.pageIds) {
      const stored = structure.templateOf(pageId);
      if (stored === undefined || stored.values.size === 0) {
        continue;
      }
      // In the source's schema order; two source properties that map to one target keep the first.
      const mapped = new Map<PropertyId, TemplatePlaceholder>();
      for (const { id: sourceId } of plan.sourceSchema) {
        const placeholder = stored.values.get(sourceId);
        const ref = plan.properties.mapping.get(sourceId);
        const targetId = ref === undefined ? undefined : resolve(ref);
        const kind = targetId === undefined ? undefined : kinds.get(targetId);
        if (placeholder !== undefined && targetId !== undefined && kind !== undefined && placeholderTakes(placeholder, kind) && !mapped.has(targetId)) {
          mapped.set(targetId, placeholder);
        }
      }
      structure.templateSet(pageId, stored.template, mapped);
    }
  }

  /** A planned value with its option names turned into the target's option ids. */
  #stored(value: PlannedValue, def: PropertyDef | undefined): PropertyValue | undefined {
    if ("stored" in value) {
      return value.stored;
    }
    const idOf = (name: string): string | undefined => def?.options.find((option) => fold(option.name) === fold(name))?.id;
    if (value.named === "multi_select") {
      const ids = value.names.flatMap((name) => idOf(name) ?? []);
      return ids.length === 0 ? undefined : { kind: "multi_select", value: ids };
    }
    const id = idOf(value.name);
    return id === undefined ? undefined : { kind: value.named, value: id };
  }

  /** The ids of the source's schema as the target has them, for the views of the moved pages. */
  #viewMap(plan: MovePlan, spaceId: SpaceId, resolve: (ref: Ref) => PropertyId): ViewMap {
    const source = new Map(plan.sourceSchema.map((def) => [def.id, def]));
    const target = new Map(this.#deps.properties.schemaGet(spaceId).map((def) => [def.id, def]));
    const property = (sourceId: string): string | undefined => {
      const ref = plan.properties.mapping.get(sourceId);
      return ref === undefined ? undefined : resolve(ref);
    };
    return {
      property,
      option: (sourcePropertyId, optionId) => {
        const name = source.get(sourcePropertyId)?.options.find((option) => option.id === optionId)?.name;
        const targetId = sourcePropertyId === TAGS_PROPERTY_ID ? TAGS_PROPERTY_ID : property(sourcePropertyId);
        if (name === undefined || targetId === undefined) {
          return undefined;
        }
        return target.get(targetId)?.options.find((option) => fold(option.name) === fold(name))?.id;
      },
    };
  }
}
