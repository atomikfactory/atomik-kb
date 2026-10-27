// The properties half of the app store (ADR-0026): the property schema of each space the UI has
// looked at, and the actions on that schema and on the values of a page.
//
// The schema is read lazily, one space at a time (`loadPropertySchema`, called by the panel, the
// dialog and the search filter, so a space nobody asks about costs nothing) and re-read when
// `PropertySchemaChanged` names a space whose schema is known. Reads are "latest request wins".
// The actions that change the schema also wait for that re-read, so that what they return (a
// property just created, an option just added) is already in the store when the caller continues.
//
// The values of a page come with the tree (`PageInfo.properties`, refreshed by `StructureChanged`
// like every other page field). `setPageProperty` puts the values the core answered with into the
// tree at once, so an editor that just saved never shows the old value while the refresh is on
// its way.

import type {
  CoreClient,
  Event as CoreEvent,
  OptionColor,
  OptionId,
  PageId,
  PageInfo,
  PageNode,
  PropertyConfig,
  PropertyDef,
  PropertyId,
  PropertyKind,
  PropertyOption,
  PropertyOptionPatch,
  PropertyValue,
  SpaceId,
  StatusGroup,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import type { AppState } from "./store";

export interface PropertiesData {
  /** The property schema (Tags first) of each space read so far. */
  propertySchemas: Readonly<Record<SpaceId, readonly PropertyDef[]>>;
}

/**
 * Every action returns a promise that rejects with a `CoreError` when the core refuses. The
 * schema ones resolve once the store holds the new schema.
 */
export interface PropertiesActions {
  /** Reads the schema of a space unless it is known or already on its way. */
  loadPropertySchema: (spaceId: SpaceId) => void;
  /**
   * A relation takes a configuration (a two-way one creates the paired property too, and the answer
   * is the forward one), a rollup needs one (ADR-0027), and so does a formula, with its stored expression (ADR-0055).
   */
  createProperty: (spaceId: SpaceId, name: string, kind: PropertyKind, config?: PropertyConfig) => Promise<PropertyDef>;
  /** Changes the configuration of a rollup, or the expression of a formula (stored form, ADR-0055). */
  setPropertyConfig: (spaceId: SpaceId, propertyId: PropertyId, config: PropertyConfig) => Promise<PropertyDef>;
  renameProperty: (spaceId: SpaceId, propertyId: PropertyId, name: string) => Promise<void>;
  /** Not for Tags (the core refuses). */
  deleteProperty: (spaceId: SpaceId, propertyId: PropertyId) => Promise<void>;
  /** In front of `before`, at the end without it (`before` may be Tags). */
  moveProperty: (spaceId: SpaceId, propertyId: PropertyId, before?: PropertyId) => Promise<void>;
  addPropertyOption: (
    spaceId: SpaceId,
    propertyId: PropertyId,
    name: string,
    color: OptionColor,
    group?: StatusGroup,
  ) => Promise<PropertyOption>;
  updatePropertyOption: (
    spaceId: SpaceId,
    propertyId: PropertyId,
    optionId: OptionId,
    patch: PropertyOptionPatch,
  ) => Promise<PropertyOption>;
  /** Also takes the option out of every page's value. */
  deletePropertyOption: (spaceId: SpaceId, propertyId: PropertyId, optionId: OptionId) => Promise<void>;
  /** Sets one value of a page, or clears it with `null`; the page in the tree shows it at once. */
  setPageProperty: (pageId: PageId, propertyId: PropertyId, value: PropertyValue | null) => Promise<void>;
  /** How many live pages of the space have a value of the property. */
  countPagesWithProperty: (spaceId: SpaceId, propertyId: PropertyId) => Promise<number>;
  /** How many live pages of the space have the option selected. */
  countPagesWithOption: (spaceId: SpaceId, propertyId: PropertyId, optionId: OptionId) => Promise<number>;
  /** The live pages of the space that have the option selected (the list view's tag source), in no set order. */
  pagesWithOption: (spaceId: SpaceId, propertyId: PropertyId, optionId: OptionId) => Promise<PageId[]>;
}

export type PropertiesState = PropertiesData & PropertiesActions;

export const EMPTY_PROPERTIES: PropertiesData = { propertySchemas: {} };

export interface PropertiesSlice {
  initial: PropertiesState;
  onEvent: (event: CoreEvent) => void;
}

/**
 * `roots` with the page `pageId` showing `values` as its properties (none: the field is left
 * out, as the core leaves it out). The same array when the page is not in the tree.
 */
export function withPageProperties(
  roots: readonly PageNode[],
  pageId: PageId,
  values: NonNullable<PageInfo["properties"]>,
): readonly PageNode[] {
  let changed = false;
  const next = roots.map((node): PageNode => {
    if (node.page.id === pageId) {
      changed = true;
      const page: PageInfo = { ...node.page };
      if (values.length === 0) {
        delete page.properties;
      } else {
        page.properties = values;
      }
      return { ...node, page };
    }
    const children = withPageProperties(node.children, pageId, values);
    if (children === node.children) {
      return node;
    }
    changed = true;
    return { ...node, children: children as PageNode[] };
  });
  return changed ? next : roots;
}

export function createPropertiesSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): PropertiesSlice {
  // Bumped by every read of a space's schema; only the answer to the latest one is applied.
  const schemaTickets = new Map<SpaceId, number>();
  const loading = new Set<SpaceId>();
  // Bumped by every write of a page's values; only the latest answer is put into the tree.
  const writeTickets = new Map<PageId, number>();

  async function fetchSchema(spaceId: SpaceId): Promise<void> {
    const ticket = (schemaTickets.get(spaceId) ?? 0) + 1;
    schemaTickets.set(spaceId, ticket);
    const properties = await client.propertySchemaGet(spaceId);
    if (schemaTickets.get(spaceId) === ticket) {
      set((state) => ({ propertySchemas: { ...state.propertySchemas, [spaceId]: properties } }));
    }
  }

  /** Re-reads the schema after a change the core accepted; a failed read leaves the old one (the event retries). */
  async function settle(spaceId: SpaceId): Promise<void> {
    try {
      await fetchSchema(spaceId);
    } catch (error) {
      console.error("could not read the property schema", error);
    }
  }

  const initial: PropertiesState = {
    ...EMPTY_PROPERTIES,

    loadPropertySchema: (spaceId) => {
      if (get().propertySchemas[spaceId] !== undefined || loading.has(spaceId)) {
        return;
      }
      loading.add(spaceId);
      refreshQuietly(async () => {
        try {
          await fetchSchema(spaceId);
        } finally {
          loading.delete(spaceId);
        }
      });
    },

    createProperty: async (spaceId, name, kind, config) => {
      const property = await client.propertyCreate(spaceId, name, kind, config);
      await settle(spaceId);
      return property;
    },
    setPropertyConfig: async (spaceId, propertyId, config) => {
      const property = await client.propertyConfigSet(spaceId, propertyId, config);
      await settle(spaceId);
      return property;
    },
    renameProperty: async (spaceId, propertyId, name) => {
      await client.propertyRename(spaceId, propertyId, name);
      await settle(spaceId);
    },
    deleteProperty: async (spaceId, propertyId) => {
      await client.propertyDelete(spaceId, propertyId);
      await settle(spaceId);
    },
    moveProperty: async (spaceId, propertyId, before) => {
      await client.propertyMove(spaceId, propertyId, before);
      await settle(spaceId);
    },
    addPropertyOption: async (spaceId, propertyId, name, color, group) => {
      const option = await client.propertyOptionAdd(spaceId, propertyId, name, color, group);
      await settle(spaceId);
      return option;
    },
    updatePropertyOption: async (spaceId, propertyId, optionId, patch) => {
      const option = await client.propertyOptionUpdate(spaceId, propertyId, optionId, patch);
      await settle(spaceId);
      return option;
    },
    deletePropertyOption: async (spaceId, propertyId, optionId) => {
      await client.propertyOptionDelete(spaceId, propertyId, optionId);
      await settle(spaceId);
    },

    setPageProperty: async (pageId, propertyId, value) => {
      const ticket = (writeTickets.get(pageId) ?? 0) + 1;
      writeTickets.set(pageId, ticket);
      const values = await client.pagePropertySet(pageId, propertyId, value);
      if (writeTickets.get(pageId) !== ticket) {
        return;
      }
      set((state) => {
        if (state.tree === null) {
          return {};
        }
        const roots = withPageProperties(state.tree.roots, pageId, values);
        return roots === state.tree.roots ? {} : { tree: { ...state.tree, roots: roots as PageNode[] } };
      });
    },

    countPagesWithProperty: async (spaceId, propertyId) =>
      (await client.pagesByProperty(spaceId, propertyId, { type: "HasValue" })).length,
    countPagesWithOption: async (spaceId, propertyId, optionId) =>
      (await client.pagesByProperty(spaceId, propertyId, { type: "ContainsOption", data: optionId })).length,
    pagesWithOption: (spaceId, propertyId, optionId) =>
      client.pagesByProperty(spaceId, propertyId, { type: "ContainsOption", data: optionId }),
  };

  return {
    initial,
    onEvent: (event) => {
      if (event.type === "PropertySchemaChanged") {
        const { spaceId } = event.data;
        if (get().propertySchemas[spaceId] !== undefined) {
          refreshQuietly(() => fetchSchema(spaceId));
        }
      }
    },
  };
}
