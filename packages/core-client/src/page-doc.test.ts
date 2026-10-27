import { LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import {
  PAGE_MARKS,
  attachmentRefsOf,
  configurePageDoc,
  dateMentionText,
  editPageListAttrs,
  extractAttachmentRefs,
  extractBlocks,
  extractLinks,
  inlineText,
  isIsoDate,
  isIsoTime,
  loroToPortable,
  mathInlineSource,
  normalizePortable,
  portableToLoro,
} from "./page-doc";
import {
  JS_SNAPSHOT,
  RUST_SNAPSHOT,
  UNKNOWN_JSON,
  UNKNOWN_SNAPSHOT,
  buildSampleDoc,
  fixtureStatus,
  loroPayload,
  readFixtureBytes,
  readFixtureJson,
  samplePage,
} from "./testing/fixtures";

const PARAGRAPH = "1a9e4c3f-8d2b-4f1c-8b6e-2c3d4e5f6071";
const TARGET_PAGE = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
const TARGET_BLOCK = "8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7";
const IMAGE_BLOCK = "a9b8c7d6-e5f4-4a3b-9c2d-1e0f2a3b4c5d";
const IMAGE_ATTACHMENT = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";
/** The caption of the sample's image: it holds a quote and a backslash (kb-model's golden has the same). */
const SAMPLE_CAPTION = 'Figure 1: the "sync" flow, C:\\sync';
const FILE_BLOCK = "b0c9d8e7-f6a5-4b4c-8d3e-2f1a3b4c5d6e";
const FILE_ATTACHMENT = "6a1d2e3f-4b5c-4d6e-9f70-8192a3b4c5d6";
const GALLERY_BLOCK = "e2f3a4b5-8071-4b92-8ca3-1d2e3f4a5b6c";
const WHITEBOARD_BLOCK = "f3a4b5c6-9182-4ca3-9db4-2e3f4a5b6c7d";
const GALLERY_SECOND = "7b2e3f4a-5c6d-4e7f-8091-a2b3c4d5e6f7";
const GALLERY_THIRD = "8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708";
const TABLE_BLOCK = "7b1e0c01-5d2a-4e01-9a01-3c4d5e6f7a01";
const COLUMN_LIST_BLOCK = "c01a5d01-6e2b-4f01-8b01-9d3c4e5f6a01";
const PAGE_LINK_BLOCK = "d1b2c3a4-7f60-4a81-9b92-0c1d2e3f4a5b";
const TABLE_OF_CONTENTS_BLOCK = "d01e7a01-7f3c-4a01-9c01-0e4d5f6a7b01";
const BREADCRUMB_BLOCK = "d01e7a02-7f3c-4a02-9c02-0e4d5f6a7b02";
const PAGE_LIST_BLOCK = "d01e7a03-7f3c-4a03-9c03-0e4d5f6a7b03";
const LIST_TAG_OPTION = "3c9d1e52-6a47-4b8e-9f20-5d1c7a8b4e61";
const LIST_PROPERTY = "5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74";
const TAGS_PROPERTY = "0000000a-0000-4000-8000-000000000001";

/** Builds `json` into a fresh document and reads it back. */
function roundTrip(json: JsonValue): JsonValue {
  const doc = new LoroDoc();
  portableToLoro(doc, json);
  return loroToPortable(doc);
}

function refusal(run: () => unknown): CoreError {
  try {
    run();
  } catch (error) {
    expect(error).toBeInstanceOf(CoreError);
    return error as CoreError;
  }
  throw new Error("expected the call to throw");
}

function child(list: unknown, index: number): unknown {
  expect(list).toBeInstanceOf(LoroList);
  return (list as LoroList).get(index);
}

function asMap(value: unknown): LoroMap {
  expect(value).toBeInstanceOf(LoroMap);
  return value as LoroMap;
}

function asText(value: unknown): LoroText {
  expect(value).toBeInstanceOf(LoroText);
  return value as LoroText;
}

/** One paragraph with the given inline content. */
function paragraph(content: JsonValue[], id = "p1"): JsonValue {
  return { type: "doc", content: [{ type: "paragraph", attrs: { id }, content }] };
}

describe("portable JSON <-> Loro on the page-v1 fixture", () => {
  it("round-trips page-v1.portable.json exactly", () => {
    const sample = samplePage();
    expect(roundTrip(sample)).toEqual(sample);
    expect(loroToPortable(buildSampleDoc())).toEqual(sample);
  });

  it("is already normalised (normalizePortable is the identity on it)", () => {
    // The `whiteboards` value (ADR-0033) is not part of the tree that normalizePortable reads.
    const { whiteboards, ...tree } = samplePage() as { [key: string]: JsonValue };
    expect(whiteboards).toBeDefined();
    expect(normalizePortable(tree)).toEqual(tree);
  });

  it("writes the binding's shape: name, attributes map, children list, merged text runs", () => {
    const doc = buildSampleDoc();
    const root = doc.getMap("doc");
    expect(root.get("nodeName")).toBe("doc");
    expect(asMap(root.get("attributes")).size).toBe(0);
    const blocks = root.get("children");
    expect((blocks as LoroList).length).toBe(23);

    const heading = asMap(child(blocks, 0));
    expect(heading.get("nodeName")).toBe("heading");
    expect(asMap(heading.get("attributes")).toJSON()).toEqual({ id: "0f8d3b2e-7c1a-4e0b-9a5d-1b2c3d4e5f60", level: 1 });
    expect(asText(child(heading.get("children"), 0)).toDelta()).toEqual([
      { insert: "Welcome to " },
      { insert: "atomik-kb", attributes: { bold: {} } },
    ]);

    // Text, hardBreak, text, pageRef, text, pageRef, text: consecutive text nodes share one LoroText.
    const para = asMap(child(blocks, 1));
    const inline = para.get("children") as LoroList;
    expect(inline.toArray().map((item) => (item instanceof LoroText ? "text" : asMap(item).get("nodeName")))).toEqual([
      "text",
      "hardBreak",
      "text",
      "pageRef",
      "text",
      "pageRef",
      "text",
    ]);
    const first = asText(inline.get(0));
    expect(first.toString()).toBe("Rich text with italic, underline, strike, code and a link.");
    expect(first.toDelta().find((run) => run.insert === "link")).toEqual({
      insert: "link",
      attributes: { link: { href: "https://example.com/docs" } },
    });
    const ref = asMap(inline.get(5));
    expect(asMap(ref.get("attributes")).toJSON()).toEqual({
      id: "4dc1f7c2-b05e-4c4f-9e91-5f607182a3b4",
      pageId: TARGET_PAGE,
      blockId: TARGET_BLOCK,
    });
    expect((ref.get("children") as LoroList).length).toBe(0);

    // The table (additive nodes, 2026-10-05): table > tableRow > cell > paragraph, every node
    // an element triple with its `id`; the cells also hold the span attributes the editor writes.
    const table = asMap(child(blocks, 11));
    expect(table.get("nodeName")).toBe("table");
    expect(asMap(table.get("attributes")).toJSON()).toEqual({ id: TABLE_BLOCK });
    const rows = table.get("children") as LoroList;
    expect(rows.length).toBe(3);
    const headerRow = asMap(rows.get(0));
    expect(headerRow.get("nodeName")).toBe("tableRow");
    const headerCell = asMap(child(headerRow.get("children"), 0));
    expect(headerCell.get("nodeName")).toBe("tableHeader");
    expect(asMap(headerCell.get("attributes")).toJSON()).toEqual({
      id: "7b1e0c03-5d2a-4e03-9a03-3c4d5e6f7a03",
      colspan: 1,
      rowspan: 1,
    });
    const bodyRow = asMap(rows.get(1));
    expect(asMap(child(bodyRow.get("children"), 0)).get("nodeName")).toBe("tableCell");
    const twoParagraphs = asMap(child(bodyRow.get("children"), 2));
    expect((twoParagraphs.get("children") as LoroList).length).toBe(2);
    const bold = asMap(child(asMap(child(bodyRow.get("children"), 1)).get("children"), 0));
    expect(asText(child(bold.get("children"), 0)).toDelta()).toEqual([{ insert: "Done", attributes: { bold: {} } }]);

    // Leaf blocks of Milestone 5: attributes only, an empty children list.
    const image = asMap(child(blocks, 12));
    expect(image.get("nodeName")).toBe("image");
    expect(asMap(image.get("attributes")).toJSON()).toEqual({
      id: IMAGE_BLOCK,
      attachmentId: IMAGE_ATTACHMENT,
      alt: "Diagram of the sync flow",
      width: 640,
      widthPercent: 60,
      align: "right",
      caption: SAMPLE_CAPTION,
    });
    expect((image.get("children") as LoroList).length).toBe(0);
    const file = asMap(child(blocks, 13));
    expect(file.get("nodeName")).toBe("file");
    expect(asMap(file.get("attributes")).toJSON()).toEqual({ id: FILE_BLOCK, attachmentId: FILE_ATTACHMENT, view: "preview" });

    // The gallery (additive node, 2026-10-09): its pictures are one plain value of the attributes map,
    // a list of plain maps set whole, and the columns a number; no child.
    const photos = asMap(child(blocks, 14));
    expect(photos.get("nodeName")).toBe("gallery");
    expect(asMap(photos.get("attributes")).toJSON()).toEqual({
      id: GALLERY_BLOCK,
      columns: 3,
      items: [
        { attachmentId: IMAGE_ATTACHMENT, caption: 'Opening night, "north" wing' },
        { attachmentId: GALLERY_SECOND },
        { attachmentId: GALLERY_THIRD, caption: "Backstage" },
      ],
    });
    expect(Array.isArray(asMap(photos.get("attributes")).get("items"))).toBe(true);
    expect((photos.get("children") as LoroList).length).toBe(0);

    // The whiteboard (additive node, 2026-10-10): a leaf with its `id` and the height of its view; the
    // elements of the drawing are in the root map `whiteboards`, not in the node.
    const board = asMap(child(blocks, 15));
    expect(board.get("nodeName")).toBe("whiteboard");
    expect(asMap(board.get("attributes")).toJSON()).toEqual({ id: WHITEBOARD_BLOCK, height: 360 });
    expect((board.get("children") as LoroList).length).toBe(0);
    expect(Object.keys(doc.getMap("whiteboards").toJSON() as object).sort()).toEqual([
      `${WHITEBOARD_BLOCK}/frame1`,
      `${WHITEBOARD_BLOCK}/gone1`,
      `${WHITEBOARD_BLOCK}/label1`,
    ]);

    // The columns (additive nodes, 2026-10-06): columnList > column > blocks, every node an
    // element triple holding its `id` and nothing else.
    const layout = asMap(child(blocks, 16));
    expect(layout.get("nodeName")).toBe("columnList");
    expect(asMap(layout.get("attributes")).toJSON()).toEqual({ id: COLUMN_LIST_BLOCK });
    const columns = layout.get("children") as LoroList;
    expect(columns.length).toBe(2);
    const left = asMap(columns.get(0));
    expect(left.get("nodeName")).toBe("column");
    expect(asMap(left.get("attributes")).toJSON()).toEqual({ id: "c01a5d02-6e2b-4f02-8b02-9d3c4e5f6a02" });
    expect((left.get("children") as LoroList).length).toBe(1);
    expect((asMap(columns.get(1)).get("children") as LoroList).length).toBe(2);
    expect(asText(child(asMap(child(left.get("children"), 0)).get("children"), 0)).toString()).toBe("Left column");

    // The page link card (additive node, 2026-10-06): a leaf block with `id`, `pageId` and `blockId`.
    const card = asMap(child(blocks, 17));
    expect(card.get("nodeName")).toBe("pageLink");
    expect(asMap(card.get("attributes")).toJSON()).toEqual({
      id: PAGE_LINK_BLOCK,
      pageId: TARGET_PAGE,
      blockId: TARGET_BLOCK,
    });
    expect((card.get("children") as LoroList).length).toBe(0);

    // The template button (additive node, ADR-0056): labelled, placing its copy above, holding one paragraph with a person
    // mention and a date mention that wait to be filled in (the attributes `relative`).
    const templateButton = asMap(child(blocks, 19));
    expect(templateButton.get("nodeName")).toBe("templateButton");
    expect(asMap(templateButton.get("attributes")).toJSON()).toEqual({
      id: "d01e7a04-7f3c-4a04-9c04-0e4d5f6a7b04",
      label: "Add task",
      position: "above",
    });
    const buttonContent = asMap(child(templateButton.get("children"), 0));
    expect(buttonContent.get("nodeName")).toBe("paragraph");
    const mentions = (buttonContent.get("children") as LoroList).toArray().filter((entry) => entry instanceof LoroMap) as LoroMap[];
    expect(mentions.map((mention) => mention.get("nodeName"))).toEqual(["personMention", "dateMention"]);
    expect(asMap(mentions[0]?.get("attributes")).toJSON()).toEqual({
      accountId: "8e2f1a3b-5c4d-4e6f-9a7b-1c2d3e4f5a6b",
      label: "Ada",
      relative: "me",
    });
    expect(asMap(mentions[1]?.get("attributes")).toJSON()).toMatchObject({ date: "2026-10-18", reminder: 30, relative: "today" });

    // The derived blocks (additive nodes, 2026-10-06), the three blocks before the last: leaves
    // holding their `id` and nothing else, an empty children list.
    const toc = asMap(child(blocks, 20));
    expect(toc.get("nodeName")).toBe("tableOfContents");
    expect(asMap(toc.get("attributes")).toJSON()).toEqual({ id: TABLE_OF_CONTENTS_BLOCK });
    expect((toc.get("children") as LoroList).length).toBe(0);
    const crumbs = asMap(child(blocks, 21));
    expect(crumbs.get("nodeName")).toBe("breadcrumb");
    expect(asMap(crumbs.get("attributes")).toJSON()).toEqual({ id: BREADCRUMB_BLOCK });
    expect((crumbs.get("children") as LoroList).length).toBe(0);

    // The list view (additive node, 2026-10-07), the last block: a leaf holding the configuration
    // of its view, an object for the source and the sort and a list of ids, an empty children list.
    const list = asMap(child(blocks, 22));
    expect(list.get("nodeName")).toBe("pageList");
    expect(asMap(list.get("attributes")).toJSON()).toEqual({
      id: PAGE_LIST_BLOCK,
      source: { kind: "tag", optionId: LIST_TAG_OPTION },
      layout: "list",
      properties: [TAGS_PROPERTY, LIST_PROPERTY],
      sort: { by: LIST_PROPERTY, dir: "desc" },
    });
    expect((list.get("children") as LoroList).length).toBe(0);
  });

  it("configures the mark table: bold, italic, underline, strike and the colour marks grow, code and link do not", () => {
    expect(PAGE_MARKS).toEqual([
      ["bold", "after"],
      ["italic", "after"],
      ["underline", "after"],
      ["strike", "after"],
      ["code", "none"],
      ["link", "none"],
      ["textColor", "after"],
      ["highlight", "after"],
    ]);
    const doc = new LoroDoc();
    portableToLoro(
      doc,
      paragraph(
        PAGE_MARKS.flatMap(([mark]): JsonValue[] => [
          {
            type: "text",
            text: mark,
            marks: [mark === "link" ? { type: mark, attrs: { href: "h" } } : { type: mark }],
          },
          { type: "text", text: "|" },
        ]),
      ),
    );
    const text = asText(child(asMap(child(doc.getMap("doc").get("children"), 0)).get("children"), 0));
    // Type one character right after every marked run, from the end so positions stay valid.
    const ends: number[] = [];
    let offset = 0;
    for (const [mark] of PAGE_MARKS) {
      offset += mark.length;
      ends.push(offset);
      offset += 1;
    }
    for (const end of ends.reverse()) {
      text.insert(end, "+");
    }
    const grown: Record<string, string> = {};
    for (const run of text.toDelta()) {
      const [mark] = Object.keys(run.attributes ?? {});
      if (mark !== undefined && typeof run.insert === "string") {
        grown[mark] = run.insert;
      }
    }
    expect(grown).toEqual({
      bold: "bold+",
      italic: "italic+",
      underline: "underline+",
      strike: "strike+",
      code: "code",
      link: "link",
      textColor: "textColor+",
      highlight: "highlight+",
    });
  });

  const rust = fixtureStatus(RUST_SNAPSHOT, "kb-model (cargo run -p kb-model --example make_golden)");
  it.skipIf(rust.missing)(`reads the Rust-written ${RUST_SNAPSHOT} as the same portable JSON${rust.note}`, () => {
    const doc = new LoroDoc();
    doc.import(loroPayload(readFixtureBytes(RUST_SNAPSHOT)));
    expect(loroToPortable(doc)).toEqual(samplePage());
  });

  const js = fixtureStatus(JS_SNAPSHOT, "pnpm --filter @atomik-kb/core-client fixtures");
  it.skipIf(js.missing)(`has a JS-written snapshot that is exactly what the code writes today${js.note}`, () => {
    const bytes = readFixtureBytes(JS_SNAPSHOT);
    const doc = new LoroDoc();
    doc.import(bytes);
    expect(loroToPortable(doc)).toEqual(samplePage());
    expect(bytes).toEqual(buildSampleDoc().export({ mode: "snapshot" }));
  });
});

describe("unknown node and mark names (page-v1-unknown fixture)", () => {
  const json = fixtureStatus(UNKNOWN_JSON, "kb-model (cargo run -p kb-model --example make_golden)");
  const rust = fixtureStatus(UNKNOWN_SNAPSHOT, "kb-model (cargo run -p kb-model --example make_golden)");

  it.skipIf(json.missing)(`pass through the TypeScript conversion untouched${json.note}`, () => {
    const page = readFixtureJson(UNKNOWN_JSON);
    expect(normalizePortable(page)).toEqual(page);
    expect(roundTrip(page)).toEqual(page);
  });

  const either = json.missing || rust.missing;
  it.skipIf(either)(`read the same from the Rust-written snapshot${json.note || rust.note}`, () => {
    const doc = new LoroDoc();
    doc.import(loroPayload(readFixtureBytes(UNKNOWN_SNAPSHOT)));
    expect(loroToPortable(doc)).toEqual(readFixtureJson(UNKNOWN_JSON));
    // A reference inside an unknown block belongs to the nearest node with an id, as anywhere else.
    expect(extractLinks(doc)).toEqual([
      { fromBlock: "3d4e5f60-7182-4d9e-bfa0-b1c2d3e4f506", toPage: "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6" },
    ]);
    expect(extractBlocks(doc).find((block) => block.kind === "fancyWidget")?.inlines).toEqual([
      { type: "text", text: "Loose widget text" },
    ]);
  });
});

describe("portableToLoro", () => {
  it("makes the root's attributes and children mergeable, so concurrent creations of a page keep both", () => {
    const [first, second] = ["aaaaaaaa-0000-4000-8000-000000000001", "aaaaaaaa-0000-4000-8000-000000000002"];
    const create = (peer: number, id: string): LoroDoc => {
      const doc = new LoroDoc();
      doc.setPeerId(peer);
      portableToLoro(doc, { type: "doc", content: [{ type: "paragraph", attrs: { id } }] });
      return doc;
    };
    const a = create(1, first);
    const b = create(2, second);
    // Mergeable containers have ids derived from the root and the key, not from an operation ("n@peer").
    const root = a.getMap("doc");
    expect(asMap(root.get("attributes")).id).not.toContain("@");
    expect((root.get("children") as LoroList).id).not.toContain("@");
    const paragraph = asMap(child(root.get("children"), 0));
    expect(paragraph.id).toContain("@");
    expect(asMap(paragraph.get("attributes")).id).toContain("@");
    expect((paragraph.get("children") as LoroList).id).toContain("@");

    a.import(b.export({ mode: "snapshot" }));
    b.import(a.export({ mode: "snapshot" }));
    const ids = (doc: LoroDoc): string[] => extractBlocks(doc).map((block) => block.id).sort();
    expect(ids(a)).toEqual([first, second]);
    expect(loroToPortable(a)).toEqual(loroToPortable(b));
  });

  it("reads a root with regular containers, as the vendored binding would create it", () => {
    const doc = new LoroDoc();
    configurePageDoc(doc);
    const root = doc.getMap("doc");
    root.set("nodeName", "doc");
    root.setContainer("attributes", new LoroMap());
    const blocks = root.setContainer("children", new LoroList());
    const paragraph = blocks.insertContainer(0, new LoroMap());
    paragraph.set("nodeName", "paragraph");
    paragraph.setContainer("attributes", new LoroMap()).set("id", PARAGRAPH);
    const inline = paragraph.setContainer("children", new LoroList());
    inline.insertContainer(0, new LoroText()).insert(0, "See ");
    const ref = inline.insertContainer(1, new LoroMap());
    ref.set("nodeName", "pageRef");
    ref.setContainer("attributes", new LoroMap()).set("pageId", TARGET_PAGE);
    ref.setContainer("children", new LoroList());
    doc.commit();
    expect(asMap(root.get("attributes")).id).toContain("@");

    const expected: JsonValue = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: PARAGRAPH },
          content: [
            { type: "text", text: "See " },
            { type: "pageRef", attrs: { pageId: TARGET_PAGE } },
          ],
        },
      ],
    };
    expect(loroToPortable(doc)).toEqual(expected);
    expect(extractBlocks(doc)).toEqual([
      {
        id: PARAGRAPH,
        kind: "paragraph",
        inlines: [
          { type: "text", text: "See " },
          { type: "pageRef", pageId: TARGET_PAGE },
        ],
      },
    ]);
    expect(extractLinks(doc)).toEqual([{ fromBlock: PARAGRAPH, toPage: TARGET_PAGE }]);
    expect(renderMarkdown(loroToPortable(doc), () => "Target")).toBe("See [[Target]]\n");
    // Converting it back writes the canonical shape, with a mergeable root.
    expect(roundTrip(loroToPortable(doc))).toEqual(expected);
  });

  it("accepts raw ProseMirror JSON: null attributes are not stored, mark attributes keep non-null values", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, {
      type: "doc",
      attrs: null,
      content: [
        {
          type: "orderedList",
          attrs: { id: "l1", start: 1, type: null },
          content: [
            {
              type: "listItem",
              attrs: { id: "i1" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: "p1", textAlign: null },
                  content: [
                    {
                      type: "text",
                      text: "site",
                      marks: [
                        { type: "link", attrs: { href: "https://a.example", target: null, rel: null, class: null } },
                      ],
                    },
                    { type: "hardBreak", marks: [{ type: "bold", attrs: {} }] },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });
    const list = asMap(child(doc.getMap("doc").get("children"), 0));
    expect(asMap(list.get("attributes")).toJSON()).toEqual({ id: "l1", start: 1 });
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        {
          type: "orderedList",
          attrs: { id: "l1", start: 1 },
          content: [
            {
              type: "listItem",
              attrs: { id: "i1" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: "p1" },
                  content: [
                    { type: "text", marks: [{ type: "link", attrs: { href: "https://a.example" } }], text: "site" },
                    // Marks on non-text nodes are dropped, as the binding drops them.
                    { type: "hardBreak" },
                  ],
                },
              ],
            },
          ],
        },
      ],
    });
  });

  it("stores marks without attributes as an empty object", () => {
    const doc = new LoroDoc();
    const marks: JsonValue[] = [{ type: "bold" }, { type: "italic", attrs: null }];
    portableToLoro(doc, paragraph([{ type: "text", text: "x", marks }]));
    const text = asText(child(asMap(child(doc.getMap("doc").get("children"), 0)).get("children"), 0));
    expect(text.toDelta()).toEqual([{ insert: "x", attributes: { bold: {}, italic: {} } }]);
  });

  it("passes unknown nodes, attributes and marks through", () => {
    const json: JsonValue = {
      type: "doc",
      content: [
        {
          type: "mermaidDiagram",
          attrs: { id: "m1", config: { theme: "dark", sizes: [1, 2.5] }, visible: true },
          content: [{ type: "text", marks: [{ type: "highlight", attrs: { color: "#ff0" } }], text: "graph" }],
        },
      ],
    };
    expect(roundTrip(json)).toEqual(json);
  });

  it("creates the attributes and children containers for leaves too", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, { type: "doc", content: [{ type: "horizontalRule", attrs: { id: "hr" } }] });
    const rule = asMap(child(doc.getMap("doc").get("children"), 0));
    expect(rule.get("attributes")).toBeInstanceOf(LoroMap);
    expect(rule.get("children")).toBeInstanceOf(LoroList);
    expect(loroToPortable(doc)).toEqual({ type: "doc", content: [{ type: "horizontalRule", attrs: { id: "hr" } }] });
  });

  it("refuses anything that is not a node tree with a doc root, naming the place but not the content", () => {
    const cases: [JsonValue, string][] = [
      ["doc", "doc is not a node object"],
      [[], "doc is not a node object"],
      [{ content: [] }, "doc.type must be a non-empty string"],
      [{ type: "paragraph" }, "the root node must be a doc"],
      [{ type: "doc", content: {} }, "doc.content must be an array"],
      [{ type: "doc", content: [{ type: "paragraph", attrs: [] }] }, "doc.content[0].attrs must be an object"],
      [{ type: "doc", content: [{ type: "text", text: 7 }] }, "doc.content[0].text must be a string"],
      [
        { type: "doc", content: [{ type: "text", text: "secret", marks: {} }] },
        "doc.content[0].marks must be an array",
      ],
      [
        { type: "doc", content: [{ type: "text", text: "secret", marks: [{ attrs: {} }] }] },
        "doc.content[0].marks[0] must be a mark object with a type",
      ],
    ];
    for (const [json, message] of cases) {
      const error = refusal(() => {
        portableToLoro(new LoroDoc(), json);
      });
      expect(error.toApiError(), message).toEqual({ code: "InvalidInput", message, details: { field: "content" } });
      expect(error.message).not.toContain("secret");
    }
  });

  it("refuses a document that already has content and leaves it untouched", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, paragraph([{ type: "text", text: "first" }]));
    const before = loroToPortable(doc);
    expect(refusal(() => portableToLoro(doc, paragraph([]))).message).toBe("the document is not empty");
    expect(loroToPortable(doc)).toEqual(before);
  });

  it("validates everything before writing anything", () => {
    const doc = new LoroDoc();
    refusal(() => {
      portableToLoro(doc, { type: "doc", content: [{ type: "paragraph" }, { type: "text" }] });
    });
    expect(doc.getMap("doc").size).toBe(0);
  });
});

describe("loroToPortable", () => {
  it("reads an empty document as a bare doc", () => {
    expect(loroToPortable(new LoroDoc())).toEqual({ type: "doc" });
  });

  it("orders keys type, attrs, content, marks, text and sorts attributes", () => {
    const portable = roundTrip({
      type: "doc",
      content: [
        {
          type: "pageRef",
          attrs: { pageId: "p", id: "r", blockId: "b" },
        },
        { type: "paragraph", attrs: { id: "x" }, content: [{ type: "text", text: "t", marks: [{ type: "bold" }] }] },
      ],
    }) as { content: { [key: string]: JsonValue }[] };
    expect(Object.keys(portable)).toEqual(["type", "content"]);
    expect(Object.keys(portable.content[0] ?? {})).toEqual(["type", "attrs"]);
    expect(Object.keys((portable.content[0]?.attrs ?? {}) as object)).toEqual(["blockId", "id", "pageId"]);
    expect(Object.keys(portable.content[1] ?? {})).toEqual(["type", "attrs", "content"]);
    const text = (portable.content[1]?.content as { [key: string]: JsonValue }[])[0] ?? {};
    expect(Object.keys(text)).toEqual(["type", "marks", "text"]);
  });

  it("joins adjacent LoroTexts and splits text per distinct mark set", () => {
    const doc = new LoroDoc();
    configurePageDoc(doc);
    const root = doc.getMap("doc");
    root.set("nodeName", "doc");
    root.setContainer("attributes", new LoroMap());
    const blocks = root.setContainer("children", new LoroList());
    const para = blocks.insertContainer(0, new LoroMap());
    para.set("nodeName", "paragraph");
    para.setContainer("attributes", new LoroMap()).set("id", "p1");
    const inline = para.setContainer("children", new LoroList());
    // What concurrent edits can leave behind: two LoroTexts side by side.
    const a = inline.insertContainer(0, new LoroText());
    a.insert(0, "plain bold");
    a.mark({ start: 6, end: 10 }, "bold", {});
    const b = inline.insertContainer(1, new LoroText());
    b.insert(0, "bolder plain");
    b.mark({ start: 0, end: 6 }, "bold", {});
    b.mark({ start: 0, end: 3 }, "italic", {});
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "p1" },
          content: [
            { type: "text", text: "plain " },
            { type: "text", marks: [{ type: "bold" }], text: "bold" },
            { type: "text", marks: [{ type: "bold" }, { type: "italic" }], text: "bol" },
            { type: "text", marks: [{ type: "bold" }], text: "der" },
            { type: "text", text: " plain" },
          ],
        },
      ],
    });
  });

  it("orders marks by the v1 table, then other marks by name, whatever order Loro reports", () => {
    const portable = roundTrip(
      paragraph([
        {
          type: "text",
          text: "all",
          marks: [
            { type: "zebra" },
            { type: "link", attrs: { href: "h" } },
            { type: "code" },
            { type: "alpha" },
            { type: "strike" },
            { type: "underline" },
            { type: "italic" },
            { type: "bold" },
          ],
        },
      ]),
    );
    expect(portable).toEqual(
      paragraph([
        {
          type: "text",
          marks: [
            { type: "bold" },
            { type: "italic" },
            { type: "underline" },
            { type: "strike" },
            { type: "code" },
            { type: "link", attrs: { href: "h" } },
            { type: "alpha" },
            { type: "zebra" },
          ],
          text: "all",
        },
      ]),
    );
  });

  it("treats non-object mark values as marks without attributes and ignores removed marks", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, paragraph([{ type: "text", text: "abcdef" }]));
    const text = asText(child(asMap(child(doc.getMap("doc").get("children"), 0)).get("children"), 0));
    text.mark({ start: 0, end: 2 }, "bold", true);
    text.mark({ start: 2, end: 4 }, "italic", {});
    text.unmark({ start: 2, end: 4 }, "italic");
    expect(loroToPortable(doc)).toEqual(
      paragraph([
        { type: "text", marks: [{ type: "bold" }], text: "ab" },
        { type: "text", text: "cdef" },
      ]),
    );
  });

  it("skips malformed element maps and plain values in children", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, paragraph([{ type: "text", text: "a" }]));
    const inline = asMap(child(doc.getMap("doc").get("children"), 0)).get("children") as LoroList;
    inline.insertContainer(1, new LoroMap()).set("nodeName", 42);
    inline.insert(2, "a plain value");
    const tail = inline.insertContainer(3, new LoroText());
    tail.insert(0, "b");
    expect(loroToPortable(doc)).toEqual(paragraph([{ type: "text", text: "ab" }]));
  });
});

describe("normalizePortable", () => {
  it("normalises Tiptap's getJSON output the way loroToPortable reads the document", () => {
    const fromEditor: JsonValue = {
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { id: "h1", level: 2, textAlign: null },
          content: [
            { type: "text", text: "a", marks: [{ type: "italic" }, { type: "bold" }] },
            { type: "text", text: "b", marks: [{ type: "bold" }, { type: "italic", attrs: {} }] },
            { type: "text", text: "" },
            { type: "text", text: "c" },
          ],
        },
        { type: "paragraph", attrs: { id: "p2" }, content: [] },
        { type: "codeBlock", attrs: { id: "c1", language: null }, content: [{ type: "text", text: "x" }] },
      ],
    };
    const normalised = normalizePortable(fromEditor);
    expect(normalised).toEqual({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { id: "h1", level: 2 },
          content: [
            { type: "text", marks: [{ type: "bold" }, { type: "italic" }], text: "ab" },
            { type: "text", text: "c" },
          ],
        },
        { type: "paragraph", attrs: { id: "p2" } },
        { type: "codeBlock", attrs: { id: "c1" }, content: [{ type: "text", text: "x" }] },
      ],
    });
    expect(normalizePortable(normalised)).toEqual(normalised);
    expect(roundTrip(fromEditor)).toEqual(normalised);
  });

  it("does not share objects with its input", () => {
    const input = { type: "doc", content: [{ type: "callout", attrs: { id: "c", meta: { a: 1 } } }] };
    const output = normalizePortable(input) as { content: { attrs: { meta: { a: number } } }[] };
    const first = output.content[0];
    if (first === undefined) {
      throw new Error("expected a node");
    }
    first.attrs.meta.a = 2;
    expect(input.content[0]?.attrs.meta.a).toBe(1);
  });
});

describe("extractBlocks and extractLinks", () => {
  it("lists every node with an id in document order with its direct inlines", () => {
    const blocks = extractBlocks(buildSampleDoc());
    // `hardBreak` carries no id in schema v1: it is only an inline break of the paragraph.
    expect(blocks.map((block) => block.kind)).toEqual([
      "heading",
      "paragraph",
      "pageRef",
      "pageRef",
      "bulletList",
      "listItem",
      "paragraph",
      "bulletList",
      "listItem",
      "paragraph",
      "listItem",
      "paragraph",
      // The two date mentions of the second item (inline atoms with an id, like the page references).
      "dateMention",
      "dateMention",
      "orderedList",
      "listItem",
      "paragraph",
      "taskList",
      "taskItem",
      "paragraph",
      "taskItem",
      "paragraph",
      "blockquote",
      "paragraph",
      // The inline equation of the quote's paragraph (an inline atom with an id, like the chips).
      "mathInline",
      "codeBlock",
      "mathBlock",
      "horizontalRule",
      "details",
      "detailsSummary",
      "detailsContent",
      "paragraph",
      "callout",
      "paragraph",
      // The table: header row, then a row with a two-paragraph cell, then a row with a `|`.
      "table",
      "tableRow",
      "tableHeader",
      "paragraph",
      "tableHeader",
      "paragraph",
      "tableHeader",
      "paragraph",
      "tableRow",
      "tableCell",
      "paragraph",
      "tableCell",
      "paragraph",
      "tableCell",
      "paragraph",
      "paragraph",
      "tableRow",
      "tableCell",
      "paragraph",
      "tableCell",
      "paragraph",
      "tableCell",
      "paragraph",
      "image",
      "file",
      // The gallery (2026-10-09): a leaf with the captions of its pictures as its text.
      "gallery",
      // The whiteboard (2026-10-10): a leaf; its elements are in the root map `whiteboards`.
      "whiteboard",
      // The columns: a list of two columns, holding one paragraph and then two.
      "columnList",
      "column",
      "paragraph",
      "column",
      "paragraph",
      "paragraph",
      // The card that links to a page, before the last paragraph.
      "pageLink",
      "paragraph",
      // The template button (ADR-0056) with its one paragraph and the date mention in it: listed, with no text.
      "templateButton",
      "paragraph",
      "dateMention",
      // The derived blocks, after it: a table of contents, a breadcrumb and a list view.
      "tableOfContents",
      "breadcrumb",
      "pageList",
    ]);
    // Every block id of the fixture is distinct, so ids identify blocks.
    expect(blocks).toHaveLength(75);
    expect(new Set(blocks.map((block) => block.id)).size).toBe(75);
    expect(blocks[0]).toEqual({
      id: "0f8d3b2e-7c1a-4e0b-9a5d-1b2c3d4e5f60",
      kind: "heading",
      inlines: [{ type: "text", text: "Welcome to atomik-kb" }],
    });
    expect(blocks[1]).toEqual({
      id: PARAGRAPH,
      kind: "paragraph",
      inlines: [
        { type: "text", text: "Rich text with italic, underline, strike, code and a link." },
        { type: "break" },
        { type: "text", text: "See also " },
        { type: "pageRef", pageId: TARGET_PAGE },
        { type: "text", text: " and " },
        { type: "pageRef", pageId: TARGET_PAGE, blockId: TARGET_BLOCK },
        { type: "text", text: "." },
      ],
    });
    const containers = blocks.filter((block) => block.inlines.length === 0).map((block) => block.kind);
    expect(containers).toEqual([
      "pageRef",
      "pageRef",
      "bulletList",
      "listItem",
      "bulletList",
      "listItem",
      "listItem",
      "dateMention",
      "dateMention",
      "orderedList",
      "listItem",
      "taskList",
      "taskItem",
      "taskItem",
      "blockquote",
      "mathInline",
      "horizontalRule",
      "details",
      "detailsContent",
      "callout",
      "table",
      "tableRow",
      "tableHeader",
      "tableHeader",
      "tableHeader",
      "tableRow",
      "tableCell",
      "tableCell",
      "tableCell",
      "tableRow",
      "tableCell",
      "tableCell",
      "tableCell",
      "file",
      "columnList",
      "column",
      "column",
      // The template button, its paragraph (template material: no text) and the date mention in it.
      "templateButton",
      "paragraph",
      "dateMention",
      "tableOfContents",
      "breadcrumb",
      "pageList",
    ]);
    // A table is no text of its own: the text of its cells is that of their paragraphs.
    const inTable = blocks.slice(
      blocks.findIndex((block) => block.id === TABLE_BLOCK),
      blocks.findIndex((block) => block.kind === "image"),
    );
    expect(
      inTable
        .filter((block) => block.kind === "paragraph")
        .map((block) => block.inlines.map((inline) => (inline.type === "text" ? inline.text : "?")).join("")),
    ).toEqual([
      "Feature",
      "Status",
      "Notes",
      "Tables",
      "Done",
      "Header row",
      "Plain cells",
      "Pipes",
      "a | b",
      "Escaped in Markdown",
    ]);
    // A column list is no text of its own either: the text of its columns is that of the
    // blocks inside them, the left column first.
    const inColumns = blocks.slice(
      blocks.findIndex((block) => block.id === COLUMN_LIST_BLOCK),
      blocks.findIndex((block) => block.id === PAGE_LINK_BLOCK), // the page link card follows the columns
    );
    expect(inColumns.map((block) => block.kind)).toEqual([
      "columnList",
      "column",
      "paragraph",
      "column",
      "paragraph",
      "paragraph",
    ]);
    expect(
      inColumns
        .filter((block) => block.kind === "paragraph")
        .map((block) => block.inlines.map((inline) => (inline.type === "text" ? inline.text : "?")).join("")),
    ).toEqual(["Left column", "Right column", "Two blocks in this one"]);
    // An image reads as its caption then its alt text, a file as nothing (kb-model's `blocks`); the
    // block also reports the three attributes of the amendment of 2026-10-09.
    expect(blocks.filter((block) => block.attachmentId !== undefined)).toEqual([
      {
        id: IMAGE_BLOCK,
        kind: "image",
        inlines: [{ type: "text", text: `${SAMPLE_CAPTION} Diagram of the sync flow` }],
        attachmentId: IMAGE_ATTACHMENT,
        alt: "Diagram of the sync flow",
        width: 640,
        widthPercent: 60,
        align: "right",
        caption: SAMPLE_CAPTION,
      },
      // The file block of the sample is shown as a preview (ADR-0031).
      { id: FILE_BLOCK, kind: "file", inlines: [], attachmentId: FILE_ATTACHMENT, view: "preview" },
    ]);
    expect(extractAttachmentRefs(buildSampleDoc())).toEqual([
      { blockId: IMAGE_BLOCK, attachmentId: IMAGE_ATTACHMENT },
      { blockId: FILE_BLOCK, attachmentId: FILE_ATTACHMENT },
      // The gallery references each of its three pictures (ADR-0030); the first is the image's.
      { blockId: GALLERY_BLOCK, attachmentId: IMAGE_ATTACHMENT },
      { blockId: GALLERY_BLOCK, attachmentId: GALLERY_SECOND },
      { blockId: GALLERY_BLOCK, attachmentId: GALLERY_THIRD },
    ]);
    // The gallery reads as its captions, one space apart.
    expect(blocks.find((block) => block.id === GALLERY_BLOCK)).toEqual({
      id: GALLERY_BLOCK,
      kind: "gallery",
      inlines: [{ type: "text", text: 'Opening night, "north" wing Backstage' }],
      items: [
        { attachmentId: IMAGE_ATTACHMENT, caption: 'Opening night, "north" wing' },
        { attachmentId: GALLERY_SECOND },
        { attachmentId: GALLERY_THIRD, caption: "Backstage" },
      ],
      columns: 3,
    });
    expect(blocks.find((block) => block.kind === "codeBlock")?.inlines).toEqual([
      { type: "text", text: 'fn main() {\n    println!("hi");\n}' },
    ]);
    // The sample's last paragraph with text (the template button and the derived blocks, which hold none, come after it).
    expect(blocks.filter((block) => block.kind === "paragraph" && block.templateContent !== true).at(-1)?.inlines).toEqual([
      { type: "text", text: "Unicode: café, 日本語, 👩‍💻 and 𝔘𝔫𝔦𝔠𝔬𝔡𝔢." },
    ]);
    // The button's paragraph is listed with no text and marked as template material (ADR-0056); so are the mention in it.
    expect(blocks.filter((block) => block.templateContent === true).map((block) => block.kind)).toEqual(["paragraph", "dateMention"]);
    // The derived blocks have no text and no reference of their own (the list view's configuration
    // is attributes, not text, and names a tag and properties, not pages).
    expect(blocks.slice(-3)).toEqual([
      { id: TABLE_OF_CONTENTS_BLOCK, kind: "tableOfContents", inlines: [] },
      { id: BREADCRUMB_BLOCK, kind: "breadcrumb", inlines: [] },
      { id: PAGE_LIST_BLOCK, kind: "pageList", inlines: [] },
    ]);
  });

  it("attributes both page references of the sample to their paragraph, and its card to itself", () => {
    expect(extractLinks(buildSampleDoc())).toEqual([
      { fromBlock: PARAGRAPH, toPage: TARGET_PAGE },
      { fromBlock: PARAGRAPH, toPage: TARGET_PAGE, toBlock: TARGET_BLOCK },
      { fromBlock: PAGE_LINK_BLOCK, toPage: TARGET_PAGE, toBlock: TARGET_BLOCK },
    ]);
  });

  it("reads a page link block as the reference it stands for, so the text index treats it like a chip", () => {
    const card = extractBlocks(buildSampleDoc()).find((block) => block.id === PAGE_LINK_BLOCK);
    expect(card).toEqual({
      id: PAGE_LINK_BLOCK,
      kind: "pageLink",
      inlines: [{ type: "pageRef", pageId: TARGET_PAGE, blockId: TARGET_BLOCK }],
    });
    expect(inlineText(card?.inlines ?? [], (id) => (id === TARGET_PAGE ? "Roadmap" : undefined))).toBe("Roadmap");
    expect(inlineText(card?.inlines ?? [], () => undefined)).toBe("");
  });

  it("counts a page link block as a link of its own block, wherever it sits", () => {
    const [column, own, callout, plain, damaged, malformed] = [
      "44444444-0000-4000-8000-000000000001",
      "44444444-0000-4000-8000-000000000002",
      "44444444-0000-4000-8000-000000000003",
      "44444444-0000-4000-8000-000000000004",
      "44444444-0000-4000-8000-000000000005",
      "44444444-0000-4000-8000-000000000006",
    ];
    const [pageA, pageB, pageC, pageD, pageE] = [
      "55555555-0000-4000-8000-00000000000a",
      "55555555-0000-4000-8000-00000000000b",
      "55555555-0000-4000-8000-00000000000c",
      "55555555-0000-4000-8000-00000000000d",
      "55555555-0000-4000-8000-00000000000e",
    ];
    const block = "66666666-0000-4000-8000-0000000000b1";
    const doc = new LoroDoc();
    portableToLoro(doc, {
      type: "doc",
      content: [
        {
          type: "columnList",
          attrs: { id: "44444444-0000-4000-8000-0000000000c0" },
          content: [
            {
              type: "column",
              attrs: { id: column },
              content: [{ type: "pageLink", attrs: { id: own, pageId: pageA, blockId: block } }],
            },
            { type: "column", attrs: { id: "44444444-0000-4000-8000-0000000000c1" }, content: [{ type: "paragraph", attrs: { id: "p9" } }] },
          ],
        },
        // No id of its own: it belongs to the callout around it, like a chip.
        { type: "callout", attrs: { id: callout }, content: [{ type: "pageLink", attrs: { pageId: pageB } }] },
        // No id and nothing around it: nobody to belong to.
        { type: "pageLink", attrs: { pageId: pageC } },
        // Not a valid page: no link.
        { type: "pageLink", attrs: { id: damaged, pageId: "not-a-page" } },
        // A malformed blockId reads as none.
        { type: "pageLink", attrs: { id: malformed, pageId: pageD, blockId: "nope" } },
        { type: "pageLink", attrs: { id: plain, pageId: pageE } },
      ],
    });
    expect(extractLinks(doc)).toEqual([
      { fromBlock: own, toPage: pageA, toBlock: block },
      { fromBlock: callout, toPage: pageB },
      { fromBlock: malformed, toPage: pageD },
      { fromBlock: plain, toPage: pageE },
    ]);
    const blocks = extractBlocks(doc);
    expect(blocks.find((entry) => entry.id === damaged)?.inlines).toEqual([]);
    expect(blocks.find((entry) => entry.id === malformed)?.inlines).toEqual([{ type: "pageRef", pageId: pageD }]);
  });

  it("attributes a reference to the nearest enclosing node with an id and skips orphans", () => {
    const [list, item, inner, broken, pageA, pageB, blockB, pageC] = [
      "11111111-0000-4000-8000-000000000001",
      "11111111-0000-4000-8000-000000000002",
      "11111111-0000-4000-8000-000000000003",
      "11111111-0000-4000-8000-000000000004",
      "22222222-0000-4000-8000-00000000000a",
      "22222222-0000-4000-8000-00000000000b",
      "33333333-0000-4000-8000-0000000000b1",
      "22222222-0000-4000-8000-00000000000c",
    ];
    const doc = new LoroDoc();
    portableToLoro(doc, {
      type: "doc",
      content: [
        {
          type: "bulletList",
          attrs: { id: list },
          content: [
            {
              type: "listItem",
              attrs: { id: item },
              content: [
                // A paragraph without an id (never written by the editor, but legal): the item owns its references.
                { type: "paragraph", content: [{ type: "pageRef", attrs: { pageId: pageA } }] },
                {
                  type: "paragraph",
                  attrs: { id: inner },
                  content: [{ type: "pageRef", attrs: { pageId: pageB, blockId: blockB } }],
                },
              ],
            },
          ],
        },
        { type: "pageRef", attrs: { pageId: pageC } },
        { type: "paragraph", attrs: { id: broken }, content: [{ type: "pageRef", attrs: { id: "r3" } }] },
      ],
    });
    expect(extractLinks(doc)).toEqual([
      { fromBlock: item, toPage: pageA },
      { fromBlock: inner, toPage: pageB, toBlock: blockB },
    ]);
    expect(extractBlocks(doc).find((block) => block.id === broken)?.inlines).toEqual([]);
  });

  it("reads ids like kb-types: 32 hex digits, hyphens ignored, canonical lowercase; others are no id", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA" },
          content: [
            { type: "pageRef", attrs: { pageId: "bbbbbbbbbbbb4bbb8bbbbbbbbbbbbbbb", blockId: "not-a-block" } },
            { type: "pageRef", attrs: { pageId: "p1" } },
          ],
        },
        // Not a valid id: neither a block nor an owner of references.
        { type: "paragraph", attrs: { id: "b1" }, content: [{ type: "text", text: "x" }] },
      ],
    });
    const owner = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const target = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    expect(extractBlocks(doc)).toEqual([
      { id: owner, kind: "paragraph", inlines: [{ type: "pageRef", pageId: target }] },
    ]);
    expect(extractLinks(doc)).toEqual([{ fromBlock: owner, toPage: target }]);
  });

  it("ends a text run at any child node, not at what is not a node", () => {
    const id = "cccccccc-0000-4000-8000-000000000001";
    const doc = new LoroDoc();
    portableToLoro(doc, {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id },
          content: [
            { type: "text", text: "a" },
            { type: "mention", attrs: { label: "x" } },
            { type: "text", text: "b" },
          ],
        },
      ],
    });
    const inline = asMap(child(doc.getMap("doc").get("children"), 0)).get("children") as LoroList;
    inline.insert(3, "a plain value");
    inline.insertContainer(4, new LoroMap()).set("nodeName", 7);
    inline.insertContainer(5, new LoroText()).insert(0, "c");
    expect(extractBlocks(doc)).toEqual([
      {
        id,
        kind: "paragraph",
        inlines: [
          { type: "text", text: "a" },
          { type: "text", text: "bc" },
        ],
      },
    ]);
  });

  it("is empty for a document without content", () => {
    expect(extractBlocks(new LoroDoc())).toEqual([]);
    expect(extractLinks(new LoroDoc())).toEqual([]);
  });
});

describe("inlineText", () => {
  it("resolves references to titles, unresolved ones to nothing, and breaks to spaces (kb-core's index text)", () => {
    const titles: Record<string, string> = { A: "Alpha" };
    expect(
      inlineText(
        [
          { type: "text", text: "See " },
          { type: "pageRef", pageId: "A" },
          { type: "break" },
          { type: "pageRef", pageId: "gone" },
          { type: "text", text: "end" },
        ],
        (id) => titles[id],
      ),
    ).toBe("See Alpha end");
  });
});

describe("image and file blocks (Milestone 5)", () => {
  const BLOCK = "c1c1c1c1-0000-4000-8000-000000000001";
  const OTHER = "c1c1c1c1-0000-4000-8000-000000000002";

  function build(content: JsonValue[]): LoroDoc {
    const doc = new LoroDoc();
    portableToLoro(doc, { type: "doc", content });
    return doc;
  }

  it("round-trip through Loro as leaf nodes with their attributes", () => {
    const page: JsonValue = {
      type: "doc",
      content: [
        { type: "image", attrs: { alt: "Sky", attachmentId: IMAGE_ATTACHMENT, id: BLOCK, width: 480 } },
        { type: "file", attrs: { attachmentId: FILE_ATTACHMENT, id: OTHER } },
      ],
    };
    expect(roundTrip(page)).toEqual(page);
  });

  it("extract the attachment id (canonical form), the alt text and an integer width", () => {
    const doc = build([
      { type: "image", attrs: { id: BLOCK, attachmentId: IMAGE_ATTACHMENT.toUpperCase(), alt: "", width: 200 } },
      { type: "file", attrs: { id: OTHER, attachmentId: FILE_ATTACHMENT, alt: "ignored", width: 3 } },
    ]);
    expect(extractBlocks(doc)).toEqual([
      { id: BLOCK, kind: "image", inlines: [], attachmentId: IMAGE_ATTACHMENT, alt: "", width: 200 },
      { id: OTHER, kind: "file", inlines: [], attachmentId: FILE_ATTACHMENT },
    ]);
    const described = build([{ type: "image", attrs: { id: BLOCK, attachmentId: IMAGE_ATTACHMENT, alt: "Sky" } }]);
    expect(extractBlocks(described)[0]?.inlines).toEqual([{ type: "text", text: "Sky" }]);
    expect(extractAttachmentRefs(doc)).toEqual([
      { blockId: BLOCK, attachmentId: IMAGE_ATTACHMENT },
      { blockId: OTHER, attachmentId: FILE_ATTACHMENT },
    ]);
  });

  it("leave out malformed attributes; blocks without an id or an attachment reference nothing", () => {
    const doc = build([
      { type: "image", attrs: { id: BLOCK, attachmentId: "photo.png", alt: 3, width: 12.5 } },
      { type: "image", attrs: { attachmentId: IMAGE_ATTACHMENT } },
      { type: "file", attrs: { id: OTHER } },
      { type: "paragraph", attrs: { id: PARAGRAPH, attachmentId: FILE_ATTACHMENT } },
    ]);
    const blocks = extractBlocks(doc);
    expect(blocks).toEqual([
      { id: BLOCK, kind: "image", inlines: [] },
      { id: OTHER, kind: "file", inlines: [] },
      { id: PARAGRAPH, kind: "paragraph", inlines: [] },
    ]);
    expect(attachmentRefsOf(blocks)).toEqual([]);
  });

  it("round-trip the width in percent, the alignment and the caption (2026-10-09)", () => {
    const page: JsonValue = {
      type: "doc",
      content: [
        {
          type: "image",
          attrs: {
            id: BLOCK,
            attachmentId: IMAGE_ATTACHMENT,
            alt: "Sky",
            widthPercent: 60,
            align: "right",
            caption: SAMPLE_CAPTION,
          },
        },
      ],
    };
    expect(roundTrip(page)).toEqual(page);
  });

  it("read an image as its caption, then its alt text, one space apart (kb-model's `blocks`)", () => {
    const image = (n: number, attrs: Record<string, JsonValue>): JsonValue => ({
      type: "image",
      attrs: { id: `c1c1c1c1-0000-4000-8000-00000000010${n}`, attachmentId: IMAGE_ATTACHMENT, ...attrs },
    });
    const blocks = extractBlocks(
      build([
        image(1, { alt: "A cat", caption: "Our cat, asleep" }),
        image(2, { alt: "", caption: "Only a caption" }),
        image(3, { alt: "Only alt text" }),
        image(4, { alt: "Alt text", caption: "" }),
        image(5, { alt: "", caption: "" }),
        image(6, {}),
        // Layout is not text, and a damaged alt or caption reads as absent.
        image(7, { alt: "A", widthPercent: 50, align: "right", width: 640 }),
        image(8, { alt: "Kept", caption: 7 }),
        image(9, { alt: 7, caption: "Kept too" }),
      ]),
    );
    expect(blocks.map((block) => block.inlines)).toEqual([
      [{ type: "text", text: "Our cat, asleep A cat" }],
      [{ type: "text", text: "Only a caption" }],
      [{ type: "text", text: "Only alt text" }],
      [{ type: "text", text: "Alt text" }],
      [],
      [],
      [{ type: "text", text: "A" }],
      [{ type: "text", text: "Kept" }],
      [{ type: "text", text: "Kept too" }],
    ]);
    // `caption` is reported when it is a string, like `alt` (an empty one included).
    expect(blocks.map((block) => block.caption)).toEqual([
      "Our cat, asleep",
      "Only a caption",
      undefined,
      "",
      "",
      undefined,
      undefined,
      undefined,
      "Kept too",
    ]);
  });

  it("report widthPercent only as an integer from 10 to 100, align only as one of the three sides", () => {
    const image = (n: number, attrs: Record<string, JsonValue>): JsonValue => ({
      type: "image",
      attrs: { id: `c1c1c1c1-0000-4000-8000-00000000020${n}`, attachmentId: IMAGE_ATTACHMENT, ...attrs },
    });
    const blocks = extractBlocks(
      build([
        image(1, { widthPercent: 10 }),
        image(2, { widthPercent: 100 }),
        image(3, { widthPercent: 50.0 }),
        image(4, { widthPercent: 9 }),
        image(5, { widthPercent: 101 }),
        image(6, { widthPercent: "x" }),
        image(7, { widthPercent: 1.5 }),
        image(8, { widthPercent: -50 }),
        image(9, { widthPercent: true }),
      ]),
    );
    expect(blocks.map((block) => block.widthPercent)).toEqual([10, 100, 50, undefined, undefined, undefined, undefined, undefined, undefined]);

    const aligned = extractBlocks(
      build([
        image(1, { align: "left" }),
        image(2, { align: "center" }),
        image(3, { align: "right" }),
        image(4, { align: "middle" }),
        image(5, { align: "Left" }),
        image(6, { align: 1 }),
        image(7, {}),
      ]),
    );
    expect(aligned.map((block) => block.align)).toEqual(["left", "center", "right", undefined, undefined, undefined, undefined]);
  });

  it("keep duplicates, in document order, nested ones included", () => {
    const doc = build([
      { type: "file", attrs: { id: BLOCK, attachmentId: FILE_ATTACHMENT } },
      {
        type: "callout",
        attrs: { id: PARAGRAPH },
        content: [{ type: "image", attrs: { id: OTHER, attachmentId: FILE_ATTACHMENT } }],
      },
    ]);
    expect(extractAttachmentRefs(doc)).toEqual([
      { blockId: BLOCK, attachmentId: FILE_ATTACHMENT },
      { blockId: OTHER, attachmentId: FILE_ATTACHMENT },
    ]);
  });
});

describe("date mentions (additive inline node, 2026-10-07)", () => {
  const mention = (id: string, date: JsonValue, time?: JsonValue): JsonValue => ({
    type: "dateMention",
    attrs: time === undefined ? { id, date } : { id, date, time },
  });
  const M1 = "d9a1c0e1-0000-4000-8000-000000000001";
  const M2 = "d9a1c0e1-0000-4000-8000-000000000002";
  const M3 = "d9a1c0e1-0000-4000-8000-000000000003";

  function build(content: JsonValue[]): LoroDoc {
    const doc = new LoroDoc();
    portableToLoro(doc, paragraph(content, PARAGRAPH));
    return doc;
  }

  it("accepts only real days as dates: YYYY-MM-DD, leap days in leap years, years 0001 to 9999", () => {
    for (const date of ["2026-10-07", "2026-01-31", "2026-12-31", "2024-02-29", "2000-02-29", "0001-01-01", "9999-12-31", "2026-04-30"]) {
      expect(isIsoDate(date), date).toBe(true);
    }
    for (const date of [
      "",
      "2026-10-7",
      "26-10-07",
      "2026/10/07",
      "20261007",
      "2026-10-07 ",
      " 2026-10-07",
      "2026-10-07\n",
      "2026-10-07T00:00",
      "2026-13-01",
      "2026-00-10",
      "2026-10-00",
      "2026-10-32",
      "2026-04-31",
      "2026-02-29",
      "1900-02-29",
      "0000-01-01",
      "+026-10-07",
      "２０２６-10-07",
      "tomorrow",
    ]) {
      expect(isIsoDate(date), JSON.stringify(date)).toBe(false);
    }
  });

  it("accepts only HH:mm as a time: 00:00 to 23:59", () => {
    for (const time of ["00:00", "09:05", "12:00", "23:59"]) {
      expect(isIsoTime(time), time).toBe(true);
    }
    for (const time of ["", "9:05", "24:00", "12:60", "12:5", "1205", "12-05", "12:05:00", " 12:05", "12:05\n", "ab:cd", "１２:05"]) {
      expect(isIsoTime(time), JSON.stringify(time)).toBe(false);
    }
  });

  it("reads as its ISO text; a bad time is left out and a bad date reads as nothing", () => {
    expect(dateMentionText("2026-10-07", undefined)).toBe("2026-10-07");
    expect(dateMentionText("2026-10-07", null)).toBe("2026-10-07");
    expect(dateMentionText("2026-10-07", "14:30")).toBe("2026-10-07 14:30");
    expect(dateMentionText("2026-10-07", "25:00")).toBe("2026-10-07");
    expect(dateMentionText("2026-10-07", 1430)).toBe("2026-10-07");
    expect(dateMentionText("2026-02-30", "14:30")).toBeUndefined();
    expect(dateMentionText(undefined, "14:30")).toBeUndefined();
    expect(dateMentionText(20261007, undefined)).toBeUndefined();
  });

  it("round-trips through Loro with its attributes, and stores no time when there is none", () => {
    const page = paragraph(
      [{ type: "text", text: "a " }, mention(M1, "2026-10-07"), { type: "text", text: " " }, mention(M2, "2026-10-08", "14:30")],
      PARAGRAPH,
    );
    expect(roundTrip(page)).toEqual(page);
    const stored = JSON.stringify(roundTrip(page));
    expect(stored.match(/"time"/gu)).toHaveLength(1);
  });

  it("is a block of its own with no inlines, and its ISO text joins the text run around it", () => {
    const doc = build([
      { type: "text", text: "Meet on " },
      mention(M1, "2026-10-07"),
      { type: "text", text: " and again at " },
      mention(M2, "2026-10-08", "14:30"),
      { type: "text", text: " sharp" },
    ]);
    expect(extractBlocks(doc)).toEqual([
      {
        id: PARAGRAPH,
        kind: "paragraph",
        inlines: [{ type: "text", text: "Meet on 2026-10-07 and again at 2026-10-08 14:30 sharp" }],
      },
      { id: M1, kind: "dateMention", inlines: [] },
      { id: M2, kind: "dateMention", inlines: [] },
    ]);
    // A date is no link, so backlinks have nothing to list.
    expect(extractLinks(doc)).toEqual([]);
    // The backlink text and the index text are that run.
    expect(inlineText(extractBlocks(doc)[0]?.inlines ?? [], () => undefined)).toBe(
      "Meet on 2026-10-07 and again at 2026-10-08 14:30 sharp",
    );
  });

  it("splits the text only at a break or a reference, as the Rust reading does", () => {
    const doc = build([
      { type: "text", text: "a " },
      mention(M1, "2026-10-07"),
      { type: "hardBreak" },
      mention(M2, "2026-10-08", "09:05"),
      { type: "text", text: " b " },
      { type: "pageRef", attrs: { id: M3, pageId: TARGET_PAGE } },
      mention("d9a1c0e1-0000-4000-8000-000000000004", "2026-10-09"),
    ]);
    expect(extractBlocks(doc)[0]?.inlines).toEqual([
      { type: "text", text: "a 2026-10-07" },
      { type: "break" },
      { type: "text", text: "2026-10-08 09:05 b " },
      { type: "pageRef", pageId: TARGET_PAGE },
      { type: "text", text: "2026-10-09" },
    ]);
  });

  it("reads a mention without a valid date as nothing, and keeps the date of one with a bad time", () => {
    const doc = build([
      mention(M1, ""),
      mention(M2, 20261007),
      mention(M3, "2026-02-30", "10:00"),
      mention("d9a1c0e1-0000-4000-8000-000000000004", "2026-10-07", 930),
      mention("d9a1c0e1-0000-4000-8000-000000000005", "2026-10-08", "24:00"),
    ]);
    expect(extractBlocks(doc)[0]?.inlines).toEqual([{ type: "text", text: "2026-10-072026-10-08" }]);
  });

  it("renders in Markdown exactly like the Rust export", () => {
    const doc = build([
      { type: "text", text: "Due " },
      mention(M1, "2026-10-07"),
      { type: "text", text: ", review " },
      mention(M2, "2026-10-08", "14:30"),
      { type: "text", text: "." },
    ]);
    expect(renderMarkdown(loroToPortable(doc), () => undefined)).toBe("Due 2026-10-07, review 2026-10-08 14:30.\n");
  });
});

describe("inline equations (additive inline node, 2026-10-07)", () => {
  const equation = (id: string, latex: JsonValue): JsonValue => ({ type: "mathInline", attrs: { id, latex } });
  const M1 = "e51c0a01-0000-4000-8000-000000000001";
  const M2 = "e51c0a01-0000-4000-8000-000000000002";
  const M3 = "e51c0a01-0000-4000-8000-000000000003";
  const M4 = "e51c0a01-0000-4000-8000-000000000004";

  function build(content: JsonValue[]): LoroDoc {
    const doc = new LoroDoc();
    portableToLoro(doc, paragraph(content, PARAGRAPH));
    return doc;
  }

  it("reads as its latex when that is a string (empty while being written), as nothing otherwise", () => {
    expect(mathInlineSource("E=mc^2")).toBe("E=mc^2");
    expect(mathInlineSource("")).toBe("");
    expect(mathInlineSource(" x ")).toBe(" x ");
    for (const latex of [undefined, null, 5, true, ["x"], { latex: "x" }]) {
      expect(mathInlineSource(latex), JSON.stringify(latex)).toBeUndefined();
    }
  });

  it("round-trips through Loro with its latex, an empty source and odd characters included", () => {
    const page = paragraph(
      [
        { type: "text", text: "a " },
        equation(M1, "E=mc^2"),
        { type: "text", text: " " },
        equation(M2, ""),
        { type: "text", text: " " },
        equation(M3, "a$b \\ c"),
      ],
      PARAGRAPH,
    );
    expect(roundTrip(page)).toEqual(page);
    // The empty source is stored, not dropped like a null attribute.
    expect(JSON.stringify(roundTrip(page))).toContain('"latex":""');
  });

  it("is a block of its own with no inlines, and its latex joins the text run around it", () => {
    const doc = build([
      { type: "text", text: "Energy " },
      equation(M1, "E=mc^2"),
      { type: "text", text: " is " },
      equation(M2, "\\frac{a}{b}"),
      { type: "text", text: " of mass" },
    ]);
    expect(extractBlocks(doc)).toEqual([
      {
        id: PARAGRAPH,
        kind: "paragraph",
        inlines: [{ type: "text", text: "Energy E=mc^2 is \\frac{a}{b} of mass" }],
      },
      { id: M1, kind: "mathInline", inlines: [] },
      { id: M2, kind: "mathInline", inlines: [] },
    ]);
    // An equation is no link, so backlinks have nothing to list.
    expect(extractLinks(doc)).toEqual([]);
    expect(inlineText(extractBlocks(doc)[0]?.inlines ?? [], () => undefined)).toBe(
      "Energy E=mc^2 is \\frac{a}{b} of mass",
    );
  });

  it("splits the text only at a break or a reference, as the Rust reading does", () => {
    const doc = build([
      { type: "text", text: "a " },
      equation(M1, "x"),
      { type: "hardBreak" },
      equation(M2, "y"),
      { type: "text", text: " b " },
      { type: "pageRef", attrs: { id: M3, pageId: TARGET_PAGE } },
      equation(M4, "z"),
    ]);
    expect(extractBlocks(doc)[0]?.inlines).toEqual([
      { type: "text", text: "a x" },
      { type: "break" },
      { type: "text", text: "y b " },
      { type: "pageRef", pageId: TARGET_PAGE },
      { type: "text", text: "z" },
    ]);
  });

  it("reads an equation whose latex is not a string as nothing, and an empty one as nothing", () => {
    const doc = build([
      { type: "text", text: "a" },
      equation(M1, ""),
      equation(M2, 5),
      equation(M3, null),
      equation(M4, "b"),
    ]);
    expect(extractBlocks(doc)[0]?.inlines).toEqual([{ type: "text", text: "ab" }]);
  });

  it("renders in Markdown exactly like the Rust export", () => {
    const doc = build([
      { type: "text", text: "Energy " },
      equation(M1, "E=mc^2"),
      { type: "text", text: " is " },
      equation(M2, ""),
      { type: "text", text: "." },
    ]);
    expect(renderMarkdown(loroToPortable(doc), () => undefined)).toBe("Energy $E=mc^2$ is .\n");
  });
});

describe("preview view of a file block (additive attribute, 2026-10-09, ADR-0031)", () => {
  const file = (n: number, attrs: Record<string, JsonValue>): JsonValue => ({
    type: "file",
    attrs: { id: `c1c1c1c1-0000-4000-8000-00000000030${n}`, attachmentId: FILE_ATTACHMENT, ...attrs },
  });
  const docOf = (...nodes: JsonValue[]): LoroDoc => {
    const doc = new LoroDoc();
    portableToLoro(doc, { type: "doc", content: nodes });
    return doc;
  };

  it("reports view only when it is exactly \"preview\", and as no text", () => {
    const blocks = extractBlocks(
      docOf(
        file(1, {}),
        file(2, { view: "preview" }),
        file(3, { view: "Preview" }),
        file(4, { view: "" }),
        file(5, { view: 1 }),
        file(6, { view: "card" }),
      ),
    );
    expect(blocks.map((block) => block.view)).toEqual([undefined, "preview", undefined, undefined, undefined, undefined]);
    expect(blocks.every((block) => block.inlines.length === 0)).toBe(true);
  });

  it("references the attachment as the card does", () => {
    const refs = extractAttachmentRefs(docOf(file(1, {}), file(2, { view: "preview" })));
    expect(refs.map((ref) => ref.attachmentId)).toEqual([FILE_ATTACHMENT, FILE_ATTACHMENT]);
  });

  it("round-trips the attribute through portable JSON", () => {
    const page: JsonValue = { type: "doc", content: [file(1, { view: "preview" })] };
    expect(roundTrip(page)).toEqual(page);
  });
});

describe("toggle headings (additive attribute `level` of the summary, 2026-10-14, ADR-0040)", () => {
  const summary = (n: number, attrs: Record<string, JsonValue>, text: string): JsonValue => ({
    type: "details",
    attrs: { id: `c2c2c2c2-0000-4000-8000-00000000040${n}`, open: false },
    content: [
      {
        type: "detailsSummary",
        attrs: { id: `c2c2c2c2-0000-4000-8000-00000000041${n}`, ...attrs },
        content: [{ type: "text", text }],
      },
      {
        type: "detailsContent",
        attrs: { id: `c2c2c2c2-0000-4000-8000-00000000042${n}` },
        content: [{ type: "paragraph", attrs: { id: `c2c2c2c2-0000-4000-8000-00000000043${n}` }, content: [{ type: "text", text: "body" }] }],
      },
    ],
  });
  const docOf = (...nodes: JsonValue[]): LoroDoc => {
    const doc = new LoroDoc();
    portableToLoro(doc, { type: "doc", content: nodes });
    return doc;
  };

  it("keeps the level in the summary's attributes map and round-trips it through portable JSON", () => {
    const page: JsonValue = { type: "doc", content: [summary(1, { level: 2 }, "Title")] };
    expect(roundTrip(page)).toEqual(page);
    const doc = docOf(summary(1, { level: 3 }, "Title"));
    const details = asMap(child(doc.getMap("doc").get("children"), 0));
    const node = asMap(child(details.get("children"), 0));
    expect(node.get("nodeName")).toBe("detailsSummary");
    expect(asMap(node.get("attributes")).toJSON()).toEqual({ id: "c2c2c2c2-0000-4000-8000-000000000411", level: 3 });
  });

  it("is layout only: the summary reads as the same text with or without a level", () => {
    const plain = extractBlocks(docOf(summary(1, {}, "Title")));
    const heading = extractBlocks(docOf(summary(1, { level: 2 }, "Title")));
    expect(heading).toEqual(plain);
    expect(heading.map((block) => block.kind)).toEqual(["details", "detailsSummary", "detailsContent", "paragraph"]);
    expect(heading[1]?.inlines).toEqual([{ type: "text", text: "Title" }]);
  });

  it("is written by the Markdown renderer as a heading inside the summary, in a damaged form as a plain toggle", () => {
    const md = (level: JsonValue | undefined): string =>
      renderMarkdown(loroToPortable(docOf(summary(1, level === undefined ? {} : { level }, "Title"))), () => undefined);
    expect(md(2)).toBe("<details>\n<summary><h2>Title</h2></summary>\n\nbody\n\n</details>\n");
    expect(md(undefined)).toBe("<details>\n<summary>Title</summary>\n\nbody\n\n</details>\n");
    expect(md(7)).toBe("<details>\n<summary>Title</summary>\n\nbody\n\n</details>\n");
  });
});

describe("photo gallery (additive node, 2026-10-09, ADR-0030)", () => {
  const A = "a1010908-0706-4504-8302-010001020301";
  const B = "a2020908-0706-4504-8302-010001020302";
  const C = "a3030908-0706-4504-8302-010001020303";
  const gallery = (n: number, attrs: Record<string, JsonValue>): JsonValue => ({
    type: "gallery",
    attrs: { id: `c1c1c1c1-0000-4000-8000-00000000030${n}`, ...attrs },
  });
  const entry = (attachmentId: string, caption?: string): JsonValue =>
    caption === undefined ? { attachmentId } : { attachmentId, caption };
  const docOf = (...nodes: JsonValue[]): LoroDoc => {
    const doc = new LoroDoc();
    portableToLoro(doc, { type: "doc", content: nodes });
    return doc;
  };

  it("round-trips the list of items and the columns", () => {
    const page: JsonValue = {
      type: "doc",
      content: [gallery(1, { columns: 4, items: [entry(A, "One"), entry(B), entry(A, 'Say "hi"')] })],
    };
    expect(roundTrip(page)).toEqual(page);
  });

  it("reads as its captions and references each valid picture, in order and duplicates kept", () => {
    const doc = docOf(
      gallery(1, { columns: 2, items: [entry(A, "Sunrise"), entry(B), entry(C, ""), entry(A, "Again")] }),
      // No list, or only damaged entries: no text, no reference, and a malformed columns reads as absent.
      gallery(2, { columns: 5 }),
      gallery(3, { items: "x" }),
      gallery(4, { items: [5, { caption: "no picture" }, { attachmentId: "nope" }, { attachmentId: C, caption: 7 }] }),
    );
    const blocks = extractBlocks(doc).filter((block) => block.kind === "gallery");
    expect(blocks.map((block) => block.inlines)).toEqual([
      [{ type: "text", text: "Sunrise Again" }],
      [],
      [],
      [],
    ]);
    expect(blocks.map((block) => block.columns)).toEqual([2, undefined, undefined, undefined]);
    expect(blocks.map((block) => block.items?.length)).toEqual([4, 0, 0, 1]);
    expect(extractAttachmentRefs(doc).map((ref) => [ref.blockId.slice(-2), ref.attachmentId])).toEqual([
      ["01", A],
      ["01", B],
      ["01", C],
      ["01", A],
      ["04", C],
    ]);
  });

  it("counts a picture listed twice by one gallery once", () => {
    const blocks = extractBlocks(docOf(gallery(1, { items: [entry(A), entry(A)] })));
    expect(attachmentRefsOf(blocks)).toHaveLength(2);
  });
});

describe("the table view's optional attributes (ADR-0016 amendments of 2026-10-08 and 2026-10-14)", () => {
  const VIEW = "a1b2c3d4-0001-4000-8000-0000000000a1";
  const COST = "a1b2c3d4-0001-4000-8000-0000000000c1";
  const DUE = "a1b2c3d4-0001-4000-8000-0000000000c2";

  /** A table view with filters, widths and the footer's calculations (one function of a newer version). */
  function table(): JsonValue {
    return {
      type: "doc",
      content: [
        {
          type: "pageList",
          attrs: {
            id: VIEW,
            source: { kind: "children" },
            layout: "table",
            properties: [COST, DUE],
            sort: { by: COST, dir: "desc" },
            filters: [{ propertyId: COST, op: "an-operator-of-a-newer-version", value: [1, "two"] }],
            columnWidths: { title: 320, [COST]: 120.5 },
            calculations: { title: "count_all", [COST]: "sum", [DUE]: "a-function-of-a-newer-version" },
          },
        },
      ],
    };
  }

  const viewAttrs = (doc: LoroDoc): unknown => {
    const root = loroToPortable(doc) as { content: { attrs: unknown }[] };
    return root.content[0]?.attrs;
  };

  it("round-trips through Loro exactly, the calculations whole and as plain values, a function nobody knows included", () => {
    expect(roundTrip(table())).toEqual(table());
    const doc = new LoroDoc();
    portableToLoro(doc, table());
    expect(normalizePortable(table())).toEqual(table());
    const attributes = asMap(asMap(child(doc.getMap("doc").get("children"), 0)).get("attributes"));
    // A plain value (one register), not a container the editor and the core would have to merge key by key.
    expect(attributes.get("calculations")).toEqual({ title: "count_all", [COST]: "sum", [DUE]: "a-function-of-a-newer-version" });
  });

  it("is left out of a view that has none: the default of the attribute is not written", () => {
    const plain: JsonValue = { type: "doc", content: [{ type: "pageList", attrs: { id: VIEW, layout: "table", calculations: null } }] };
    expect(roundTrip(plain)).toEqual({ type: "doc", content: [{ type: "pageList", attrs: { id: VIEW, layout: "table" } }] });
  });

  it("is rewritten by editPageListAttrs like the other attributes: set whole, removed, and untouched when equal", () => {
    const doc = new LoroDoc();
    portableToLoro(doc, table());
    expect(editPageListAttrs(doc, (attrs) => ({ ...attrs }))).toBe(false);
    expect(
      editPageListAttrs(doc, (attrs) => ({ ...attrs, calculations: { title: "count_not_empty", [DUE]: "date_range" } })),
    ).toBe(true);
    expect((viewAttrs(doc) as { calculations: unknown }).calculations).toEqual({ title: "count_not_empty", [DUE]: "date_range" });
    expect((viewAttrs(doc) as { columnWidths: unknown }).columnWidths).toEqual({ title: 320, [COST]: 120.5 });
    expect(
      editPageListAttrs(doc, (attrs) => {
        const { calculations: removed, ...rest } = attrs;
        void removed;
        return rest;
      }),
    ).toBe(true);
    expect(viewAttrs(doc)).not.toHaveProperty("calculations");
    expect(viewAttrs(doc)).toHaveProperty("columnWidths");
  });
});
