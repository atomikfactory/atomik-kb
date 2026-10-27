// The properties slice of the store (store-properties.ts) and the tag filter of the search
// (store-content.ts), over the in-memory core: the schema is read lazily and follows
// `PropertySchemaChanged`, a saved value is in the tree at once, and a search can be limited to a tag.

import { CoreClient, MemoryTransport, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { MemoryTransportOptions, PageNode, Request, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import type { AppState } from "./store";
import { EMPTY_SEARCH } from "./store-content";
import { withPageProperties } from "./store-properties";
import { paragraphs } from "./test/editor";

const PERSONAL = {
  name: "Personal",
  tagOptions: [{ name: "idea" }, { name: "project" }],
  properties: [
    { name: "Priority", kind: "number" as const },
    { name: "Stage", kind: "select" as const, options: [{ name: "Draft" }] },
  ],
  pages: [
    { title: "Notes", tags: ["idea"], content: paragraphs("Plans for the garden"), properties: { Priority: 2 } },
    { title: "Garden", tags: ["idea", "project"], content: paragraphs("Plans for roses") },
    { title: "Journal", content: paragraphs("Plans for tomorrow") },
  ],
};
const WORK = {
  name: "Work",
  tagOptions: [{ name: "client" }],
  pages: [{ title: "Meetings", tags: ["client"], content: paragraphs("Plans") }],
};

async function setup(options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PERSONAL, WORK], ...options });
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return memory.dispatch(request);
    },
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
  };
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  const client = new CoreClient(memory);
  const spaces = new Map<string, string>();
  const pages = new Map<string, string>();
  for (const space of await client.spaceList()) {
    spaces.set(space.name, space.id);
    for (const node of (await client.treeGet(space.id)).roots) {
      pages.set(node.page.title, node.page.id);
    }
  }
  return {
    memory,
    client,
    store,
    stop,
    requests,
    state: (): AppState => store.getState(),
    space: (name: string): string => spaces.get(name) ?? "",
    page: (title: string): string => pages.get(title) ?? "",
    count: (type: Request["type"]): number => requests.filter((request) => request.type === type).length,
  };
}

describe("withPageProperties", () => {
  const page = (id: string, properties?: [string, { kind: "number"; value: number }][]): PageNode => ({
    page: { id, spaceId: "s", title: id, createdAt: 0, updatedAt: 0, ...(properties === undefined ? {} : { properties }) },
    children: [],
  });

  it("puts the values on the page wherever it is in the tree and shares what it does not change", () => {
    const leaf = page("leaf");
    const other = page("other");
    const roots: PageNode[] = [other, { ...page("parent"), children: [leaf] }];
    const next = withPageProperties(roots, "leaf", [["p", { kind: "number", value: 1 }]]);
    expect(next[0]).toBe(other);
    expect(next[1]?.children[0]?.page.properties).toEqual([["p", { kind: "number", value: 1 }]]);
    expect(leaf.page.properties).toBeUndefined();
  });

  it("leaves the field out when there are no values, and returns the same array for an unknown page", () => {
    const roots: PageNode[] = [page("a", [["p", { kind: "number", value: 1 }]])];
    const cleared = withPageProperties(roots, "a", []);
    expect(cleared[0]?.page).not.toHaveProperty("properties");
    expect(withPageProperties(roots, "missing", [])).toBe(roots);
  });
});

describe("the schema", () => {
  it("is read when asked for, once, and not before", async () => {
    const ctx = await setup();
    expect(ctx.state().propertySchemas).toEqual({});
    expect(ctx.count("PropertySchemaGet")).toBe(0);
    ctx.state().loadPropertySchema(ctx.space("Personal"));
    ctx.state().loadPropertySchema(ctx.space("Personal"));
    await vi.waitFor(() => {
      expect(ctx.state().propertySchemas[ctx.space("Personal")]).toBeDefined();
    });
    ctx.state().loadPropertySchema(ctx.space("Personal"));
    expect(ctx.count("PropertySchemaGet")).toBe(1);
    expect(ctx.state().propertySchemas[ctx.space("Personal")]?.map((def) => def.name)).toEqual(["Tags", "Priority", "Stage"]);
    ctx.stop();
  });

  it("follows PropertySchemaChanged for the spaces that are known, and only those", async () => {
    const ctx = await setup();
    ctx.state().loadPropertySchema(ctx.space("Personal"));
    await vi.waitFor(() => {
      expect(ctx.state().propertySchemas[ctx.space("Personal")]).toBeDefined();
    });
    await ctx.client.propertyCreate(ctx.space("Personal"), "Mood", "text");
    await ctx.client.propertyCreate(ctx.space("Work"), "Client", "text");
    await vi.waitFor(() => {
      expect(ctx.state().propertySchemas[ctx.space("Personal")]?.map((def) => def.name)).toContain("Mood");
    });
    expect(ctx.state().propertySchemas[ctx.space("Work")]).toBeUndefined();
    ctx.stop();
  });

  it("has the new schema in the store when a schema action resolves", async () => {
    const ctx = await setup();
    const personal = ctx.space("Personal");
    ctx.state().loadPropertySchema(personal);
    await vi.waitFor(() => {
      expect(ctx.state().propertySchemas[personal]).toBeDefined();
    });
    const names = (): string[] => ctx.state().propertySchemas[personal]?.map((def) => def.name) ?? [];
    const created = await ctx.state().createProperty(personal, "Mood", "status");
    expect(names()).toEqual(["Tags", "Priority", "Stage", "Mood"]);
    await ctx.state().renameProperty(personal, created.id, "Feeling");
    expect(names()).toContain("Feeling");
    await ctx.state().moveProperty(personal, created.id, ctx.state().propertySchemas[personal]?.[1]?.id);
    expect(names()).toEqual(["Tags", "Feeling", "Priority", "Stage"]);
    const option = await ctx.state().addPropertyOption(personal, created.id, "Blocked", "red", "in_progress");
    expect(ctx.state().propertySchemas[personal]?.[1]?.options.map((entry) => entry.name)).toEqual([
      "Not started",
      "In progress",
      "Done",
      "Blocked",
    ]);
    await ctx.state().updatePropertyOption(personal, created.id, option.id, { name: "Stuck" });
    expect(ctx.state().propertySchemas[personal]?.[1]?.options.at(-1)?.name).toBe("Stuck");
    await ctx.state().deletePropertyOption(personal, created.id, option.id);
    expect(ctx.state().propertySchemas[personal]?.[1]?.options).toHaveLength(3);
    await ctx.state().deleteProperty(personal, created.id);
    expect(names()).toEqual(["Tags", "Priority", "Stage"]);
    ctx.stop();
  });

  it("rejects with the core's error and leaves the schema alone", async () => {
    const ctx = await setup();
    const personal = ctx.space("Personal");
    ctx.state().loadPropertySchema(personal);
    await vi.waitFor(() => {
      expect(ctx.state().propertySchemas[personal]).toBeDefined();
    });
    await expect(ctx.state().deleteProperty(personal, TAGS_PROPERTY_ID)).rejects.toMatchObject({ code: "InvalidInput" });
    await expect(ctx.state().createProperty(personal, "  ", "text")).rejects.toMatchObject({ code: "InvalidInput" });
    expect(ctx.state().propertySchemas[personal]).toHaveLength(3);
    ctx.stop();
  });

  it("counts the pages that have a value or an option", async () => {
    const ctx = await setup();
    const personal = ctx.space("Personal");
    expect(await ctx.state().countPagesWithProperty(personal, TAGS_PROPERTY_ID)).toBe(2);
    const idea = (await ctx.client.propertySchemaGet(personal))[0]?.options[0]?.id ?? "";
    expect(await ctx.state().countPagesWithOption(personal, TAGS_PROPERTY_ID, idea)).toBe(2);
    ctx.stop();
  });
});

describe("a page's values", () => {
  it("are in the tree as soon as the save resolves", async () => {
    const ctx = await setup();
    const personal = ctx.space("Personal");
    await vi.waitFor(() => {
      expect(ctx.state().tree?.spaceId).toBe(personal);
    });
    const priority = (await ctx.client.propertySchemaGet(personal)).find((def) => def.name === "Priority");
    const valuesOf = (title: string) => ctx.state().tree?.roots.find((node) => node.page.title === title)?.page.properties;

    await ctx.state().setPageProperty(ctx.page("Journal"), priority?.id ?? "", { kind: "number", value: 7 });
    expect(valuesOf("Journal")).toEqual([[priority?.id, { kind: "number", value: 7 }]]);
    // Clearing the only value takes the field out.
    await ctx.state().setPageProperty(ctx.page("Journal"), priority?.id ?? "", null);
    expect(valuesOf("Journal")).toBeUndefined();
    // The refresh the core asks for brings the same.
    await vi.waitFor(() => {
      expect(valuesOf("Notes")?.map(([, value]) => value.kind)).toEqual(["multi_select", "number"]);
    });
    ctx.stop();
  });

  it("rejects a value the core refuses and leaves the tree alone", async () => {
    const ctx = await setup();
    const personal = ctx.space("Personal");
    await vi.waitFor(() => {
      expect(ctx.state().tree?.spaceId).toBe(personal);
    });
    const priority = (await ctx.client.propertySchemaGet(personal)).find((def) => def.name === "Priority");
    await expect(
      ctx.state().setPageProperty(ctx.page("Journal"), priority?.id ?? "", { kind: "text", value: "x" }),
    ).rejects.toMatchObject({ code: "InvalidInput" });
    expect(ctx.state().tree?.roots.find((node) => node.page.title === "Journal")?.page.properties).toBeUndefined();
    ctx.stop();
  });

  it("follow a change another writer made", async () => {
    const ctx = await setup();
    const personal = ctx.space("Personal");
    await vi.waitFor(() => {
      expect(ctx.state().tree?.spaceId).toBe(personal);
    });
    const priority = (await ctx.client.propertySchemaGet(personal)).find((def) => def.name === "Priority");
    await ctx.client.pagePropertySet(ctx.page("Journal"), priority?.id ?? "", { kind: "number", value: 5 });
    await vi.waitFor(() => {
      expect(ctx.state().tree?.roots.find((node) => node.page.title === "Journal")?.page.properties).toEqual([
        [priority?.id, { kind: "number", value: 5 }],
      ]);
    });
    ctx.stop();
  });
});

describe("search limited to a tag", () => {
  async function tagOf(ctx: Awaited<ReturnType<typeof setup>>, spaceName: string, name: string) {
    const spaceId = ctx.space(spaceName);
    const tags = (await ctx.client.propertySchemaGet(spaceId)).find((def) => def.id === TAGS_PROPERTY_ID);
    return { spaceId, propertyId: TAGS_PROPERTY_ID, optionId: tags?.options.find((option) => option.name === name)?.id ?? "" };
  }

  it("lists the pages that have the tag when nothing is typed, as title hits in the core's order", async () => {
    const ctx = await setup();
    ctx.state().openSearchWithTag(await tagOf(ctx, "Personal", "idea"));
    expect(ctx.state().search).toMatchObject({ open: true, query: "" });
    await ctx.state().runSearch("");
    expect(ctx.state().search.status).toBe("ready");
    expect(ctx.state().search.hits.map((hit) => [hit.title, hit.snippet, hit.blockId])).toEqual([
      ["Garden", "Garden", undefined],
      ["Notes", "Notes", undefined],
    ]);
    ctx.stop();
  });

  it("searches the text among the pages that have it, in that space only", async () => {
    const ctx = await setup();
    ctx.state().openSearchWithTag(await tagOf(ctx, "Personal", "project"));
    await ctx.state().runSearch("plans");
    expect(ctx.state().search.hits.map((hit) => hit.title)).toEqual(["Garden"]);
    ctx.state().setSearchTag(await tagOf(ctx, "Personal", "idea"));
    expect(ctx.state().search).toMatchObject({ status: "loading", hits: [] });
    await ctx.state().runSearch("plans");
    expect(ctx.state().search.hits.map((hit) => hit.title).sort()).toEqual(["Garden", "Notes"]);
    // Pages of another space are not looked at.
    expect(ctx.requests.filter((request) => request.type === "Search").at(-1)).toMatchObject({
      data: { query: "plans", spaceId: ctx.space("Personal") },
    });
    ctx.stop();
  });

  it("without a tag searches everywhere again, and closing the search drops the tag", async () => {
    const ctx = await setup();
    ctx.state().openSearchWithTag(await tagOf(ctx, "Personal", "idea"));
    ctx.state().setSearchTag(null);
    expect(ctx.state().search).toMatchObject({ tag: null, status: "idle", hits: [] });
    await ctx.state().runSearch("plans");
    expect(ctx.state().search.hits.map((hit) => hit.title)).toContain("Meetings");

    ctx.state().setSearchTag(await tagOf(ctx, "Work", "client"));
    ctx.state().closeSearch();
    expect(ctx.state().search).toEqual(EMPTY_SEARCH);
    ctx.stop();
  });

  it("keeps the answer to the latest request", async () => {
    const ctx = await setup();
    ctx.state().openSearch();
    const tag = await tagOf(ctx, "Personal", "idea");
    ctx.state().setSearchTag(tag);
    const first = ctx.state().runSearch("");
    const second = ctx.state().runSearch("roses");
    await Promise.all([first, second]);
    expect(ctx.state().search).toMatchObject({ query: "roses", status: "ready" });
    expect(ctx.state().search.hits.map((hit) => hit.title)).toEqual(["Garden"]);
    ctx.stop();
  });
});
