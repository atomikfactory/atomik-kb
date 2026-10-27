// `Search` in `MemoryTransport` (ADR-0040, item 9), mirroring `kb-core`'s `tests_search.rs`: a page
// matches when every word is somewhere in it, the fuzzy title hits after the full-text ones, and the
// `editedAfter` filter. The same cases as the core's, on the same data.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { SearchHit } from "./generated/SearchHit";
import { MemoryTransport } from "./memory";
import type { SeedPage, SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function paragraph(blockId: number, text: string): JsonValue {
  return { type: "paragraph", attrs: { id: id(blockId) }, content: [{ type: "text", text }] };
}

/** A seeded page with one paragraph per text, their ids `id(base)`, `id(base + 1)`, ... */
function page(title: string, base: number, ...texts: string[]): SeedPage {
  return { title, content: { type: "doc", content: texts.map((text, at) => paragraph(base + at, text)) } };
}

function setup(spaces: SeedSpace[]) {
  let counter = 0x1000;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    workspaceName: "Home",
    spaces,
  });
  return { transport, client: new CoreClient(transport), clock };
}

function titles(hits: readonly SearchHit[]): string[] {
  return hits.map((hit) => hit.title);
}

/** The highlighted parts of a hit's snippet. */
function highlighted(hit: SearchHit | undefined): string[] {
  return (hit?.highlights ?? []).map((range) => hit?.snippet.slice(range.start, range.end) ?? "");
}

describe("a page matches when every word is somewhere in it", () => {
  const spaces = () => [
    {
      name: "Personal",
      pages: [
        page("Roadmap", 100, "Quarter planning notes"),
        page("Other", 200, "the roadmap for the quarter"),
      ],
    },
  ];

  it("gives the rows that hold every word, then one hit for each page whose words are spread", async () => {
    const { client } = setup(spaces());
    const hits = await client.search("roadmap quarter");
    expect(titles(hits)).toEqual(["Other", "Roadmap"]);
    expect(highlighted(hits[0])).toEqual(["roadmap", "quarter"]);
    expect(hits[1]?.blockId === undefined || hits[1].blockId === id(100)).toBe(true);

    // The row that holds the most of the words is the page's hit, highlighted for the words it holds.
    const three = await client.search("roadmap quarter planning");
    expect(titles(three)).toEqual(["Roadmap"]);
    expect(three[0]).toMatchObject({ blockId: id(100), snippet: "Quarter planning notes" });
    expect(highlighted(three[0])).toEqual(["Quarter", "planning"]);
  });

  it("needs every word, matches prefixes and has no spread hit in the titles scope", async () => {
    const { client } = setup(spaces());
    await expect(client.search("roadmap quarter zebra")).resolves.toEqual([]);
    expect(await client.search("road quart")).toHaveLength(2);
    await expect(client.search("roadmap quarter", { scope: "Titles" })).resolves.toEqual([]);
  });

  it("leaves trashed pages out and follows the space filter", async () => {
    const { client } = setup([
      ...spaces(),
      { name: "Work", pages: [page("Budget again", 300, "quarterly review"), page("Budget", 400, "other")] },
    ]);
    const [personal, work] = await client.spaceList();
    expect(await client.search("budget review", { spaceId: work?.id })).toHaveLength(1);
    expect(await client.search("budget review", { spaceId: personal?.id })).toEqual([]);
    const tree = await client.treeGet(personal?.id ?? "");
    const roadmap = tree.roots.find((node) => node.page.title === "Roadmap")?.page.id ?? "";
    await client.pageTrash(roadmap);
    expect(titles(await client.search("roadmap quarter"))).toEqual(["Other"]);
  });

  it("counts a property and a block as rows of the page", async () => {
    const { client } = setup([
      {
        name: "Personal",
        properties: [{ name: "Where", kind: "text" }],
        pages: [{ ...page("Cafe", 100, "Summer brunch spot"), properties: { Where: "Café de Flore, Paris" } }],
      },
    ]);
    const hits = await client.search("flore brunch");
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({ title: "Cafe" });
    // The block is first on a tie, as the title is.
    expect(highlighted(hits[0])).toEqual(["brunch"]);
  });
});

describe("fuzzy titles", () => {
  const spaces = () => [
    {
      name: "Personal",
      pages: [
        { title: "Plan" },
        page("Zone", 100, "plan b for the weekend"),
        { title: "Pelican" },
        { title: "Unrelated" },
      ],
    },
  ];

  it("come after the full-text hits and never repeat a page", async () => {
    const { client } = setup(spaces());
    const hits = await client.search("plan");
    expect(titles(hits)).toEqual(["Plan", "Zone", "Pelican"]);
    expect(hits[2]?.blockId).toBeUndefined();
    expect(hits[2]?.snippet).toBe("Pelican");
    expect(highlighted(hits[2])).toEqual(["P", "l", "an"]);
    // They only fill what the limit leaves.
    expect(titles(await client.search("plan", { limit: 2 }))).toEqual(["Plan", "Zone"]);
    // The titles scope is fuzzy too.
    expect(titles(await client.search("plan", { scope: "Titles", limit: 8 }))).toEqual(["Plan", "Pelican"]);
  });

  it("find a title by a typo or by its initials, and need two letters", async () => {
    const { client } = setup(spaces());
    expect(titles(await client.search("pelcian"))).toEqual(["Pelican"]);
    expect(titles(await client.search("unrltd"))).toEqual(["Unrelated"]);
    await expect(client.search("e")).resolves.toEqual([]);
    await expect(client.search("...")).resolves.toEqual([]);
  });

  it("leave out trashed pages and follow the space filter", async () => {
    const { client } = setup([
      { name: "Personal", pages: [{ title: "Recipes" }, { title: "Pelican" }] },
      { name: "Work", pages: [{ title: "Rice cooking plan" }] },
    ]);
    const [personal, work] = await client.spaceList();
    // The letters at word starts score more: "Rice cooking plan" before "Recipes".
    expect(titles(await client.search("rcp"))).toEqual(["Rice cooking plan", "Recipes"]);
    expect(titles(await client.search("rcp", { spaceId: personal?.id }))).toEqual(["Recipes"]);
    expect(titles(await client.search("rcp", { spaceId: work?.id }))).toEqual(["Rice cooking plan"]);
    const tree = await client.treeGet(personal?.id ?? "");
    const pelican = tree.roots.find((node) => node.page.title === "Pelican")?.page.id ?? "";
    await client.pageTrash(pelican);
    expect(await client.search("pelcian")).toEqual([]);
  });
});

describe("editedAfter", () => {
  it("keeps the pages last edited at or after it, in every pass", async () => {
    const { client, clock } = setup([{ name: "Personal", pages: [{ title: "Old plan" }] }]);
    const space = (await client.spaceList())[0]?.id ?? "";
    clock.now = T0 + 4_000;
    await client.pageCreate({ spaceId: space, title: "New plan" });
    await client.pageCreate({ spaceId: space, title: "Pelican" });

    const after = async (query: string, editedAfter?: number): Promise<string[]> =>
      titles(await client.search(query, editedAfter === undefined ? {} : { editedAfter })).sort();
    expect(await after("plan")).toEqual(["New plan", "Old plan", "Pelican"]);
    // The fuzzy pass follows the filter too, and it is inclusive.
    expect(await after("plan", T0 + 2_000)).toEqual(["New plan", "Pelican"]);
    expect(await after("plan", T0 + 4_000)).toEqual(["New plan", "Pelican"]);
    expect(await after("plan", T0 + 4_001)).toEqual([]);

    // Editing a page moves it into the range.
    const later = T0 + 10_000;
    clock.now = later;
    const old = (await client.treeGet(space)).roots.find((node) => node.page.title === "Old plan")?.page.id ?? "";
    await client.pageRename(old, "Old plan b");
    expect(await after("plan", later)).toEqual(["Old plan b"]);
  });

  it("refuses anything but a number from untyped callers", async () => {
    const { transport } = setup([{ name: "Personal", pages: [{ title: "A page" }] }]);
    const request = { type: "Search", data: { query: "page", editedAfter: "yesterday" } } as unknown as Parameters<
      MemoryTransport["dispatch"]
    >[0];
    const error: unknown = await transport.dispatch(request).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({ details: { field: "editedAfter" } });
  });
});
