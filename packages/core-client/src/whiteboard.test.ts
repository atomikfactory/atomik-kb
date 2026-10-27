// The elements of whiteboard blocks in the page document (ADR-0033): the TypeScript mirror of
// `kb_model::page::whiteboard`, the portable form with its `whiteboards` value, the Markdown, and
// the merge of two replicas that edit the same drawing.

import { LoroDoc } from "loro-crdt";
import { describe, expect, it } from "vitest";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import { extractBlocks, loroToPortable, portableToLoro } from "./page-doc";
import {
  WHITEBOARDS,
  WHITEBOARD_ORIGIN,
  importWhiteboards,
  orphanWhiteboardKeys,
  readWhiteboard,
  splitWhiteboardKey,
  whiteboardKey,
  writeWhiteboardElements,
} from "./whiteboard";
import type { WhiteboardValue } from "./whiteboard";

const PARA = "0a0a0a0a-0a0a-4a0a-8a0a-0a0a0a0a0a0a";
const BOARD = "0b0b0b0b-0b0b-4b0b-8b0b-0b0b0b0b0b0b";
const OTHER = "0c0c0c0c-0c0c-4c0c-8c0c-0c0c0c0c0c0c";

function element(id: string, type: string, index: string, version = 1, extra: WhiteboardValue = {}): WhiteboardValue {
  return { id, type, index, version, versionNonce: 7, isDeleted: false, x: 1.5, y: 2, frameId: null, groupIds: [], ...extra };
}

function textElement(id: string, index: string, text: string): WhiteboardValue {
  return element(id, "text", index, 1, { text, originalText: text });
}

function page(): LoroDoc {
  const doc = new LoroDoc();
  portableToLoro(doc, {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: PARA }, content: [{ type: "text", text: "intro" }] },
      { type: "whiteboard", attrs: { id: BOARD, height: 400 } },
    ],
  });
  return doc;
}

function replicas(): [LoroDoc, LoroDoc] {
  const a = page();
  writeWhiteboardElements(a, BOARD, [element("r1", "rectangle", "a0"), element("r2", "ellipse", "a1")]);
  const b = new LoroDoc();
  b.import(a.export({ mode: "snapshot" }));
  return [a, b];
}

function exchange(a: LoroDoc, b: LoroDoc): void {
  const toB = a.export({ mode: "update", from: b.version() });
  const toA = b.export({ mode: "update", from: a.version() });
  a.import(toA);
  b.import(toB);
}

function versionOf(doc: LoroDoc, id: string): unknown {
  return readWhiteboard(doc, BOARD).find((e) => e.id === id)?.value.version;
}

describe("keys", () => {
  it("are <block id>/<element id>", () => {
    expect(whiteboardKey(BOARD, "abc_-1")).toBe(`${BOARD}/abc_-1`);
    expect(splitWhiteboardKey(`${BOARD}/abc_-1`)).toEqual({ blockId: BOARD, elementId: "abc_-1" });
    expect(splitWhiteboardKey(`${BOARD}/`)).toBeUndefined();
    expect(splitWhiteboardKey("nope/abc")).toBeUndefined();
    expect(splitWhiteboardKey(`${BOARD}/${"x".repeat(257)}`)).toBeUndefined();
  });
});

describe("a whiteboard in a page document", () => {
  it("reads as its live text elements in z-order, whitespace collapsed", () => {
    const doc = page();
    writeWhiteboardElements(doc, BOARD, [
      textElement("t2", "a2", "second\n  line"),
      element("r1", "rectangle", "a0"),
      textElement("t1", "a1", "first"),
      textElement("t3", "a3", "   "),
      { ...textElement("t4", "a4", "deleted text"), isDeleted: true },
    ]);
    expect(readWhiteboard(doc, BOARD).map((e) => e.id)).toEqual(["r1", "t1", "t2", "t3", "t4"]);
    const blocks = extractBlocks(doc);
    const board = blocks.find((block) => block.id === BOARD);
    expect(board).toEqual({
      id: BOARD,
      kind: "whiteboard",
      inlines: [{ type: "text", text: "first second line" }],
      height: 400,
    });
  });

  it("keeps its elements in the root map, outside the tree of the page", () => {
    const doc = page();
    writeWhiteboardElements(doc, BOARD, [element("r1", "rectangle", "a0")]);
    expect(JSON.stringify(doc.getMap("doc").toJSON())).not.toContain("rectangle");
    expect(Object.keys(doc.getMap(WHITEBOARDS).toJSON() as object)).toEqual([`${BOARD}/r1`]);
  });

  it("has the portable value of the blocks that exist, and rebuilds from it", () => {
    const doc = page();
    writeWhiteboardElements(doc, BOARD, [element("r1", "rectangle", "a0", 3)]);
    writeWhiteboardElements(doc, OTHER, [element("o1", "ellipse", "a0")]); // an orphan
    const portable = loroToPortable(doc) as { [key: string]: JsonValue };
    const boards = portable.whiteboards as { [block: string]: { [id: string]: { [key: string]: JsonValue } } };
    expect(Object.keys(boards)).toEqual([BOARD]);
    expect(boards[BOARD]?.r1).toMatchObject({ x: 1.5, y: 2, frameId: null, version: 3 });
    expect(orphanWhiteboardKeys(doc, new Set([BOARD]))).toEqual([`${OTHER}/o1`]);

    const copy = new LoroDoc();
    portableToLoro(copy, portable);
    expect(loroToPortable(copy)).toEqual(portable);
    expect(readWhiteboard(copy, BOARD)).toHaveLength(1);
  });

  it("has no whiteboards value on a page without a drawing", () => {
    expect(loroToPortable(page())).not.toHaveProperty("whiteboards");
  });

  it("refuses damaged whiteboards on import and writes nothing", () => {
    for (const bad of [
      "x",
      { "not a uuid": {} },
      { [BOARD]: [] },
      { [BOARD]: { r1: { id: "other" } } },
      { [BOARD]: { r1: 5 } },
    ] as JsonValue[]) {
      const doc = new LoroDoc();
      expect(() => {
        importWhiteboards(doc, bad);
      }, JSON.stringify(bad)).toThrow();
      expect(Object.keys(doc.getMap(WHITEBOARDS).toJSON() as object)).toEqual([]);
    }
  });

  it("is written as a quote of the texts in Markdown", () => {
    const doc = page();
    writeWhiteboardElements(doc, BOARD, [
      textElement("t2", "a1", "Beta | two\nlines"),
      textElement("t1", "a0", "Alpha"),
      element("r", "rectangle", "a2"),
    ]);
    expect(renderMarkdown(loroToPortable(doc), () => undefined)).toBe(
      "intro\n\n> **Whiteboard**\n>\n> - Alpha\n> - Beta | two lines\n",
    );
  });

  it("commits its writes with an origin the page undo manager excludes", () => {
    const doc = page();
    const origins: string[] = [];
    doc.subscribe((event) => {
      origins.push(event.origin ?? "");
    });
    writeWhiteboardElements(doc, BOARD, [element("r1", "rectangle", "a0")]);
    return Promise.resolve().then(() => {
      expect(origins).toContain(WHITEBOARD_ORIGIN);
    });
  });
});

describe("two replicas that edit the same drawing", () => {
  it("keep both edits when they edit different elements", () => {
    const [a, b] = replicas();
    writeWhiteboardElements(a, BOARD, [element("r1", "rectangle", "a0", 2), element("n-a", "diamond", "a2")]);
    writeWhiteboardElements(b, BOARD, [element("r2", "ellipse", "a1", 2), textElement("n-b", "a3", "from b")]);
    exchange(a, b);
    for (const doc of [a, b]) {
      expect(versionOf(doc, "r1")).toBe(2);
      expect(versionOf(doc, "r2")).toBe(2);
      expect(readWhiteboard(doc, BOARD)).toHaveLength(4);
    }
    expect(loroToPortable(a)).toEqual(loroToPortable(b));
  });

  it("converge on one value when they edit the same element", () => {
    const [a, b] = replicas();
    writeWhiteboardElements(a, BOARD, [element("r1", "rectangle", "a0", 2)]);
    writeWhiteboardElements(b, BOARD, [element("r1", "rectangle", "a0", 3)]);
    exchange(a, b);
    expect(versionOf(a, "r1")).toBe(versionOf(b, "r1"));
    expect([2, 3]).toContain(versionOf(a, "r1"));
    expect(versionOf(a, "r2")).toBe(1);
  });

  it("converge when one deletes an element the other edits, and the tombstone stays in the map", () => {
    const [a, b] = replicas();
    writeWhiteboardElements(a, BOARD, [element("r1", "rectangle", "a0", 5, { isDeleted: true })]);
    writeWhiteboardElements(b, BOARD, [element("r1", "rectangle", "a0", 4)]);
    exchange(a, b);
    expect(loroToPortable(a)).toEqual(loroToPortable(b));
    const kept = readWhiteboard(a, BOARD).find((e) => e.id === "r1");
    expect(kept).toBeDefined();
    expect([
      [true, 5],
      [false, 4],
    ]).toContainEqual([kept?.value.isDeleted, kept?.value.version]);
  });

  it("keep both first elements of a new drawing", () => {
    const a = page();
    const b = new LoroDoc();
    b.import(a.export({ mode: "snapshot" }));
    writeWhiteboardElements(a, BOARD, [element("x", "rectangle", "a0")]);
    writeWhiteboardElements(b, BOARD, [element("y", "ellipse", "a1")]);
    exchange(a, b);
    expect(readWhiteboard(a, BOARD).map((e) => e.id)).toEqual(["x", "y"]);
    expect(loroToPortable(a)).toEqual(loroToPortable(b));
  });
});
