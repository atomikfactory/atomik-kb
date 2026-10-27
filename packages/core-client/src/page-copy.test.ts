// The content of a page copy (ADR-0040) on the TypeScript side: the twin of `kb-model`'s `page/remap.rs`.

import { describe, expect, it } from "vitest";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { portableBlockIds, remapPortable } from "./page-copy";
import type { CopyIds } from "./page-copy";

const uuid = (prefix: string, n: number): string => `${prefix}-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
const bid = (n: number): string => uuid("bbbbbbbb", n);
const copyBid = (n: number): string => bid(n + 100);
const pid = (n: number): string => uuid("70000000", n);
const copyPid = (n: number): string => uuid("71000000", n);

/** The ids of blocks 1 to 30 and of pages 1 and 2 are copied. */
function ids(): CopyIds {
  const blocks = new Map<string, string>();
  for (let n = 1; n <= 30; n += 1) {
    blocks.set(bid(n), copyBid(n));
  }
  return {
    pages: new Map([
      [pid(1), copyPid(1)],
      [pid(2), copyPid(2)],
    ]),
    blocks,
  };
}

const para = (id: string, content: JsonValue[] = []): JsonValue => ({ type: "paragraph", attrs: { id }, content });
const docOf = (content: JsonValue[]): { [key: string]: JsonValue } => ({ type: "doc", content });
const pageRef = (id: string, pageId: string, blockId?: string): JsonValue => ({
  type: "pageRef",
  attrs: blockId === undefined ? { id, pageId } : { id, pageId, blockId },
});

describe("remapPortable", () => {
  it("gives every node with an id its copy and changes nothing else", () => {
    const json = docOf([
      para(bid(1), [
        { type: "text", text: "Hello ", marks: [{ type: "bold" }] },
        { type: "dateMention", attrs: { id: bid(2), date: "2026-10-13" } },
        { type: "mathInline", attrs: { id: bid(3), latex: "E=mc^2" } },
      ]),
      {
        type: "bulletList",
        attrs: { id: bid(4) },
        content: [{ type: "listItem", attrs: { id: bid(5) }, content: [para(bid(6), [{ type: "text", text: "item" }])] }],
      },
      { type: "heading", attrs: { id: bid(7), level: 2 }, content: [{ type: "text", text: "Title" }] },
      { type: "horizontalRule" },
      { type: "paragraph", attrs: { id: "not-an-id" } },
      { type: "paragraph", attrs: { id: bid(99) } },
    ]);
    const before = JSON.stringify(json);
    const copy = remapPortable(json, ids()) as { content: { [key: string]: JsonValue }[] };

    expect(copy.content[0]?.attrs).toEqual({ id: copyBid(1) });
    expect((copy.content[0]?.content as { attrs: JsonValue }[])[1]?.attrs).toEqual({ id: copyBid(2), date: "2026-10-13" });
    expect((copy.content[0]?.content as JsonValue[])[0]).toEqual({ type: "text", text: "Hello ", marks: [{ type: "bold" }] });
    expect(JSON.stringify(copy)).toContain(copyBid(6));
    expect(copy.content[2]?.attrs).toEqual({ id: copyBid(7), level: 2 });
    expect(copy.content[3]).toEqual({ type: "horizontalRule" });
    expect(copy.content[4]?.attrs).toEqual({ id: "not-an-id" });
    expect(copy.content[5]?.attrs).toEqual({ id: bid(99) });
    // The original is not modified, and the copy shares no mapped id with it.
    expect(JSON.stringify(json)).toBe(before);
    const old = new Set(portableBlockIds(json));
    const next = new Set(portableBlockIds(copy));
    expect(old.has(bid(1)) && !next.has(bid(1))).toBe(true);
    expect(next.has(copyBid(1)) && next.has(bid(99))).toBe(true);
  });

  it("points links to the copied pages at the copies and keeps the other links", () => {
    const outside = pid(9);
    const json = docOf([
      para(bid(1), [
        pageRef(bid(10), pid(1)),
        pageRef(bid(11), pid(2), bid(5)),
        // A block that was not copied keeps its id.
        pageRef(bid(12), pid(2), bid(77)),
        // A link to a page outside the subtree is kept (only the reference's own id changes).
        pageRef(bid(13), outside, bid(5)),
      ]),
      { type: "pageLink", attrs: { id: bid(14), pageId: pid(1), blockId: bid(6) } },
      { type: "pageLink", attrs: { id: bid(15), pageId: outside } },
    ]);
    const copy = remapPortable(json, ids()) as { content: { content?: { attrs: JsonValue }[]; attrs: JsonValue }[] };
    const refs = copy.content[0]?.content ?? [];
    expect(refs[0]?.attrs).toEqual({ id: copyBid(10), pageId: copyPid(1) });
    expect(refs[1]?.attrs).toEqual({ id: copyBid(11), pageId: copyPid(2), blockId: copyBid(5) });
    expect(refs[2]?.attrs).toEqual({ id: copyBid(12), pageId: copyPid(2), blockId: bid(77) });
    expect(refs[3]?.attrs).toEqual({ id: copyBid(13), pageId: outside, blockId: bid(5) });
    expect(copy.content[1]?.attrs).toEqual({ id: copyBid(14), pageId: copyPid(1), blockId: copyBid(6) });
    expect(copy.content[2]?.attrs).toEqual({ id: copyBid(15), pageId: outside });
  });

  it("moves a whiteboard's elements under the copy of its block", () => {
    const element = (id: string): JsonValue => ({ id, type: "rectangle", index: "a0", version: 1, isDeleted: false });
    const json = docOf([
      para(bid(1), [{ type: "text", text: "x" }]),
      { type: "whiteboard", attrs: { id: bid(2), height: 400 } },
      { type: "whiteboard", attrs: { id: bid(3) } },
    ]);
    json.whiteboards = {
      [bid(2)]: { r1: element("r1"), r2: element("r2") },
      [bid(3)]: { r1: element("r1") },
    };
    const copy = remapPortable(json, ids()) as { whiteboards: { [key: string]: JsonValue } };
    expect(Object.keys(copy.whiteboards).sort()).toEqual([copyBid(2), copyBid(3)]);
    expect(copy.whiteboards[copyBid(2)]).toEqual({ r1: element("r1"), r2: element("r2") });
  });

  it("points a page source inside the copied subtree at the copy and keeps every other source", () => {
    const view: JsonValue = {
      id: "default",
      layout: "list",
      sorts: [
        { by: "title", dir: "asc" },
        { by: "created", dir: "desc" },
      ],
      filter: { op: "and", items: [{ propertyId: "p", op: "within", value: { relative: "thisWeek" } }] },
    };
    const list = (n: number, source: JsonValue): JsonValue => ({ type: "pageList", attrs: { id: bid(n), source, views: [view] } });
    const json = docOf([
      list(1, { kind: "page", pageId: pid(1) }),
      list(2, { kind: "page", pageId: pid(9) }),
      list(3, { kind: "children" }),
      list(4, { kind: "space" }),
      list(5, { kind: "tag", optionId: pid(1) }),
      list(6, { kind: "page", pageId: "not an id" }),
      list(7, { kind: "page" }),
      { type: "callout", attrs: { id: bid(8), source: { kind: "page", pageId: pid(1) } } },
    ]);
    const before = JSON.stringify(json);
    const copy = remapPortable(json, ids()) as { content: { attrs: { [key: string]: JsonValue } }[] };
    const source = (n: number): JsonValue | undefined => copy.content[n]?.attrs.source;

    expect(source(0)).toEqual({ kind: "page", pageId: copyPid(1) });
    expect(source(1)).toEqual({ kind: "page", pageId: pid(9) });
    expect(source(2)).toEqual({ kind: "children" });
    expect(source(3)).toEqual({ kind: "space" });
    expect(source(4)).toEqual({ kind: "tag", optionId: pid(1) });
    expect(source(5)).toEqual({ kind: "page", pageId: "not an id" });
    expect(source(6)).toEqual({ kind: "page" });
    expect(source(7)).toEqual({ kind: "page", pageId: pid(1) });
    // The views, their sorts and their relative values are untouched; the block id still follows.
    expect(copy.content[0]?.attrs.views).toEqual([view]);
    expect(copy.content[0]?.attrs.id).toBe(copyBid(1));
    expect(JSON.stringify(json)).toBe(before);
  });

  it("keeps a duplicated id one id", () => {
    const copy = remapPortable(docOf([para(bid(3)), para(bid(3))]), ids()) as { content: { attrs: JsonValue }[] };
    expect(copy.content.map((node) => node.attrs)).toEqual([{ id: copyBid(3) }, { id: copyBid(3) }]);
  });

  it("reads ids written without hyphens or in capitals as the same ids", () => {
    const loud = bid(1).toUpperCase().replaceAll("-", "");
    const copy = remapPortable(docOf([para(loud)]), ids()) as { content: { attrs: JsonValue }[] };
    expect(copy.content[0]?.attrs).toEqual({ id: copyBid(1) });
  });
});

describe("portableBlockIds", () => {
  it("lists each id once in document order", () => {
    const json = docOf([
      para(bid(3), [pageRef(bid(4), pid(1))]),
      { type: "horizontalRule" },
      para(bid(3)),
      para(bid(1)),
    ]);
    expect(portableBlockIds(json)).toEqual([bid(3), bid(4), bid(1)]);
    expect(portableBlockIds({ type: "doc" })).toEqual([]);
  });
});
