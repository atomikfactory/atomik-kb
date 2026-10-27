// `PageExcerpts` in `MemoryTransport` (ADR-0058): the twin of the core's `page_excerpts`. The rule of the text (one line per
// block with its whitespace collapsed, lines joined with a line break, cut at 280 Unicode scalar values at a word with `…`),
// the order, and the pages left out (not live pages of the space, templates, pages without text).

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { EXCERPT_CHARS, cutExcerpt, excerptRow } from "./content";
import { MemoryTransport } from "./memory";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Portable content: one paragraph for each text, the block ids from `first` on. */
function rows(first: number, ...texts: string[]): JsonValue {
  return {
    type: "doc",
    content: texts.map((value, at): JsonValue => ({
      type: "paragraph",
      attrs: { id: blockId(first + at) },
      content: [{ type: "text", text: value }],
    })),
  };
}

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    pages: [
      { title: "Rows", content: rows(1, "  first   row \t", "second 　row", "   ", "third") },
      { title: "Words", content: rows(10, Array.from({ length: 100 }, () => "word").join(" ")) },
      { title: "Long word", content: rows(20, "x".repeat(400)) },
      { title: "Exact", content: rows(30, "y".repeat(EXCERPT_CHARS)) },
      { title: "Over", content: rows(40, "y".repeat(EXCERPT_CHARS + 1)) },
      { title: "Emoji", content: rows(50, "😀".repeat(300)) },
      { title: "Many rows", content: rows(60, "a".repeat(100), Array.from({ length: 100 }, () => "bb").join(" ")) },
      { title: "Marks", content: rows(70, "ab  c�d") },
      { title: "Empty" },
      {
        title: "Template",
        template: true,
        content: rows(80, "template text"),
        children: [{ title: "Inside", content: rows(90, "inside text") }],
      },
      { title: "Gone", content: rows(100, "gone text") },
      { title: "One", content: rows(110, "one text") },
      { title: "Two", content: rows(120, "two text") },
    ],
  },
  { name: "Other", pages: [{ title: "Foreign", content: rows(130, "foreign text") }] },
];

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

async function setup() {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0,
    workspaceName: "Workspace",
    spaces: SPACES,
  });
  const client = new CoreClient(transport);
  const spaces = await client.spaceList();
  const home = spaces.find((space) => space.name === "Home")?.id ?? "";
  const other = spaces.find((space) => space.name === "Other")?.id ?? "";
  const pages = new Map<string, string>();
  const walk = (nodes: Awaited<ReturnType<typeof client.treeGet>>["roots"]): void => {
    for (const node of nodes) {
      pages.set(node.page.title, node.page.id);
      walk(node.children);
    }
  };
  walk((await client.treeGet(home)).roots);
  walk((await client.treeGet(other)).roots);
  const pageId = (title: string): string => pages.get(title) ?? "";
  /** The excerpt text of one page, `undefined` when the answer leaves it out. */
  const excerpt = async (title: string, space = home): Promise<string | undefined> =>
    (await client.pageExcerpts(space, [pageId(title)]))[0]?.text;
  return { client, home, other, pageId, excerpt };
}

describe("PageExcerpts", () => {
  it("joins the lines of the page in document order, one space for each run of whitespace", async () => {
    const { excerpt } = await setup();
    expect(await excerpt("Rows")).toBe("first row\nsecond row\nthird");
    // The marks the index puts for its highlight markers and inline atoms are not text.
    expect(await excerpt("Marks")).toBe("ab cd");
  });

  it("cuts at 280 characters at a word with an ellipsis, counting Unicode scalar values", async () => {
    const { excerpt } = await setup();
    // 56 words are 279 characters; the 280th is the space after them.
    expect(await excerpt("Words")).toBe(`${Array.from({ length: 56 }, () => "word").join(" ")}…`);
    expect(await excerpt("Long word")).toBe(`${"x".repeat(280)}…`);
    expect(await excerpt("Exact")).toBe("y".repeat(280));
    expect(await excerpt("Over")).toBe(`${"y".repeat(280)}…`);
    expect(await excerpt("Emoji")).toBe(`${"😀".repeat(280)}…`);
    const lines = (await excerpt("Many rows")) ?? "";
    expect(lines.startsWith(`${"a".repeat(100)}\nbb bb`)).toBe(true);
    expect(lines.endsWith("bb…")).toBe(true);
    expect(Array.from(lines).length).toBeLessThanOrEqual(281);
  });

  it("answers in the order asked, each page once, and leaves out what has no excerpt", async () => {
    const { client, home, other, pageId, excerpt } = await setup();
    await client.pageTrash(pageId("Gone"));
    const asked = ["Two", "Empty", "One", "Template", "Inside", "Gone", "Foreign", "Two"].map(pageId);
    asked.splice(5, 0, id(999));
    const answer = await client.pageExcerpts(home, asked);
    expect(answer.map(({ pageId: page, text }) => [page, text])).toEqual([
      [pageId("Two"), "two text"],
      [pageId("One"), "one text"],
    ]);
    expect(await client.pageExcerpts(home, [])).toEqual([]);
    // Another space answers its own pages.
    expect(await excerpt("Foreign", other)).toBe("foreign text");
    expect(await excerpt("Foreign")).toBeUndefined();
  });

  it("asks for at most 500 pages and a known space", async () => {
    const { client, home } = await setup();
    const ids = Array.from({ length: 501 }, (_, n) => id(10_000 + n));
    expect(await failure(client.pageExcerpts(home, ids))).toMatchObject({ code: "InvalidInput", details: { field: "pageIds" } });
    expect(await client.pageExcerpts(home, ids.slice(0, 500))).toEqual([]);
    expect(await failure(client.pageExcerpts(id(777), ids.slice(0, 1)))).toMatchObject({ code: "NotFound" });
  });

  it("reads a page's text as the editor leaves it (a page nobody gave text has no excerpt)", async () => {
    const { client, home, pageId } = await setup();
    const fresh = await client.pageCreate({ spaceId: home, title: "Fresh" });
    expect(await client.pageExcerpts(home, [fresh.id, pageId("One")])).toEqual([{ pageId: pageId("One"), text: "one text" }]);
  });
});

describe("the text rule", () => {
  it("cleans one line", () => {
    expect(excerptRow("  a \t b\n\nc  ")).toBe("a b c");
    expect(excerptRow("a�b")).toBe("ab");
    expect(excerptRow("a � b")).toBe("a b");
    expect(excerptRow(" �\t　 � ")).toBeUndefined();
    expect(excerptRow("")).toBeUndefined();
    expect(excerptRow("é 😀")).toBe("é 😀");
    // The byte order mark is no whitespace for the core, so it stays.
    expect(excerptRow("﻿a")).toBe("﻿a");
  });

  it("cuts the joined lines", () => {
    expect(cutExcerpt("short")).toBe("short");
    expect(cutExcerpt("a".repeat(280))).toBe("a".repeat(280));
    expect(cutExcerpt("a".repeat(281))).toBe(`${"a".repeat(280)}…`);
    expect(cutExcerpt(`${"a".repeat(100)} ${"b".repeat(300)}`)).toBe(`${"a".repeat(100)}…`);
    // The line break is whitespace.
    expect(cutExcerpt(`${"a".repeat(100)}\n${"b".repeat(300)}`)).toBe(`${"a".repeat(100)}…`);
    // A cut at the very start keeps the limit.
    expect(cutExcerpt(` ${"b".repeat(300)}`)).toBe(` ${"b".repeat(279)}…`);
  });
});
