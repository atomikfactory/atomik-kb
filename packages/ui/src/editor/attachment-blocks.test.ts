// The attachment blocks in the schema and in the block commands: their shape, where a new
// one goes, the Markdown of one block, and turn-into rules that never drop an attachment.

import { EditorState } from "@tiptap/pm/state";
import { DOMParser as PmDOMParser, DOMSerializer } from "@tiptap/pm/model";
import type { Node as PmNode } from "@tiptap/pm/model";
import { describe, expect, it } from "vitest";

import { pid } from "../test/editor";
import {
  CAPTION_MAX_LENGTH,
  attachmentMarkdown,
  normalizeCaption,
  parseAlign,
  parseCaption,
  parseFileView,
  isPdfMime,
  parseWidth,
  parseWidthPercent,
} from "./attachment-nodes";
import { kindForMime, placementFor } from "./attachments";
import { ATTACHMENT_KINDS, TEXT_KINDS, TOGGLE_KINDS, duplicateBlock, turnInto, turnIntoKinds } from "./blocks";
import { ID_BEARING_NODES, createEditorSchema } from "./schema";

const schema = createEditorSchema();
const A = "a0000000-0000-4000-8000-000000000001";

function doc(json: unknown): PmNode {
  return schema.nodeFromJSON({ type: "doc", content: json });
}

const image = (id: number, alt = "") => ({ type: "image", attrs: { id: pid(id), attachmentId: A, alt } });
const paragraph = (id: number, text = "") => ({
  type: "paragraph",
  attrs: { id: pid(id) },
  ...(text === "" ? {} : { content: [{ type: "text", text }] }),
});

/** The document as compact `type(id)` lines with nesting. */
function outline(node: PmNode): string {
  const parts: string[] = [];
  node.forEach((child) => {
    const inner = child.isTextblock ? JSON.stringify(child.textContent) : child.childCount > 0 ? `[${outline(child)}]` : "";
    parts.push(`${child.type.name}${inner}`);
  });
  return parts.join(" ");
}

describe("the attachment blocks of the schema", () => {
  it("are id-bearing leaf blocks with the attributes of ADR-0020", () => {
    expect(ID_BEARING_NODES).toEqual(expect.arrayContaining(["image", "file"]));
    expect(Object.keys(schema.nodes.image?.spec.attrs ?? {})).toEqual([
      "id",
      "attachmentId",
      "alt",
      "width",
      "widthPercent",
      "align",
      "caption",
    ]);
    expect(Object.keys(schema.nodes.file?.spec.attrs ?? {})).toEqual(["id", "attachmentId", "view"]);
    expect(schema.nodes.image?.isLeaf).toBe(true);
    expect(schema.nodes.image?.spec.draggable).toBe(true);
    expect(schema.nodes.file?.spec.group).toBe("block");
  });

  it("render as Markdown the way the core exports them", () => {
    const [picture, file] = [
      schema.nodeFromJSON({ type: "image", attrs: { attachmentId: A, alt: "A cat" } }),
      schema.nodeFromJSON({ type: "file", attrs: { attachmentId: A } }),
    ];
    expect(attachmentMarkdown(picture, "cat.png")).toBe(`![A cat](attachment://${A})`);
    expect(attachmentMarkdown(file, "notes.txt")).toBe(`[notes.txt](attachment://${A})`);
    expect(attachmentMarkdown(file, null)).toBe(`[${A}](attachment://${A})`);
  });

  it("render an image's caption as the Markdown link title, escaped the way the core escapes it", () => {
    const image = (caption: unknown) =>
      schema.nodeFromJSON({ type: "image", attrs: { attachmentId: A, alt: "A cat", widthPercent: 50, align: "left", caption } });
    expect(attachmentMarkdown(image("The cat"), null)).toBe(`![A cat](attachment://${A} "The cat")`);
    // A backslash is doubled, a double quote gets a backslash, any line break is one space (CRLF too).
    expect(attachmentMarkdown(image('Say "hi" at C:\\sync'), null)).toBe(
      `![A cat](attachment://${A} "Say \\"hi\\" at C:\\\\sync")`,
    );
    expect(attachmentMarkdown(image("one\ntwo\r\nthree\rfour"), null)).toBe(`![A cat](attachment://${A} "one two three four")`);
    // No caption (absent, empty, damaged): the line is as before; width and alignment are not written.
    for (const none of [null, "", 7]) {
      expect(attachmentMarkdown(image(none), null)).toBe(`![A cat](attachment://${A})`);
    }
  });

  it("round-trip an image's width in percent, alignment and caption through HTML", () => {
    const serializer = DOMSerializer.fromSchema(schema);
    const parser = PmDOMParser.fromSchema(schema);
    const roundTrip = (attrs: Record<string, unknown>): { html: HTMLElement; back: PmNode | null | undefined } => {
      const html = serializer.serializeNode(schema.nodeFromJSON({ type: "image", attrs: { attachmentId: A, ...attrs } })) as HTMLElement;
      const holder = document.createElement("div");
      holder.append(html);
      return { html, back: parser.parse(holder).firstChild };
    };

    const full = roundTrip({ alt: "A cat", width: 640, widthPercent: 60, align: "right", caption: 'The "cat"' });
    expect(full.html.tagName).toBe("FIGURE");
    expect(
      ["data-attachment-id", "data-alt", "data-width", "data-width-percent", "data-align", "data-caption"].map((name) =>
        full.html.getAttribute(name),
      ),
    ).toEqual([A, "A cat", "640", "60", "right", 'The "cat"']);
    // The figcaption is the caption, else the alt text.
    expect(full.html.querySelector("figcaption")?.textContent).toBe('The "cat"');
    expect(full.back?.attrs).toMatchObject({ attachmentId: A, alt: "A cat", width: 640, widthPercent: 60, align: "right", caption: 'The "cat"' });

    const plain = roundTrip({ alt: "A cat" });
    for (const name of ["data-width", "data-width-percent", "data-align", "data-caption"]) {
      expect(plain.html.hasAttribute(name), name).toBe(false);
    }
    expect(plain.html.querySelector("figcaption")?.textContent).toBe("A cat");
    expect(plain.back?.attrs).toMatchObject({ alt: "A cat", width: null, widthPercent: null, align: null, caption: null });

    // A damaged value reads as absent.
    const damaged = document.createElement("div");
    damaged.innerHTML = `<figure data-type="image" data-attachment-id="${A}" data-width-percent="5" data-align="middle" data-caption=""></figure>`;
    expect(parser.parse(damaged).firstChild?.attrs).toMatchObject({ widthPercent: null, align: null, caption: null });
  });

  it("round-trip a file's view through HTML and read it leniently (ADR-0031)", () => {
    const serializer = DOMSerializer.fromSchema(schema);
    const parser = PmDOMParser.fromSchema(schema);
    const roundTrip = (attrs: Record<string, unknown>): { html: HTMLElement; back: PmNode | null | undefined } => {
      const html = serializer.serializeNode(schema.nodeFromJSON({ type: "file", attrs: { attachmentId: A, ...attrs } })) as HTMLElement;
      const holder = document.createElement("div");
      holder.append(html);
      return { html, back: parser.parse(holder).firstChild };
    };
    const preview = roundTrip({ view: "preview" });
    expect(preview.html.getAttribute("data-view")).toBe("preview");
    expect(preview.back?.attrs).toMatchObject({ attachmentId: A, view: "preview" });
    const card = roundTrip({});
    expect(card.html.hasAttribute("data-view")).toBe(false);
    expect(card.back?.attrs).toMatchObject({ attachmentId: A, view: null });
    // The Markdown is the file link either way.
    expect(attachmentMarkdown(schema.nodeFromJSON({ type: "file", attrs: { attachmentId: A, view: "preview" } }), "a.pdf")).toBe(
      `[a.pdf](attachment://${A})`,
    );
    expect(["preview", "Preview", "card", "", null, 1].map(parseFileView)).toEqual(["preview", null, null, null, null, null]);
    expect(["application/pdf", " Application/PDF ", "application/x-pdf", "image/png", ""].map(isPdfMime)).toEqual([true, true, false, false, false]);
  });

  it("read the layout attributes leniently: a damaged value is absent", () => {
    expect([10, "10", 50, "75", 100, 9, 101, 0, 50.5, "abc", null, undefined].map(parseWidthPercent)).toEqual([
      10, 10, 50, 75, 100, null, null, null, null, null, null, null,
    ]);
    expect(["left", "center", "right", "Left", "middle", "", null, 1].map(parseAlign)).toEqual([
      "left",
      "center",
      "right",
      null,
      null,
      null,
      null,
      null,
    ]);
    expect(["A cat", "", null, 3, "  "].map(parseCaption)).toEqual(["A cat", null, null, null, "  "]);
  });

  it("make an edited caption one trimmed line of at most 1000 characters, or none", () => {
    expect(normalizeCaption("  A cat ")).toBe("A cat");
    expect(normalizeCaption("one\ntwo\r\nthree\rfour")).toBe("one two three four");
    expect(normalizeCaption("")).toBeNull();
    expect(normalizeCaption(" \n \t ")).toBeNull();
    expect(normalizeCaption("x".repeat(CAPTION_MAX_LENGTH + 20))).toBe("x".repeat(CAPTION_MAX_LENGTH));
  });

  it("take whole positive pixel widths only, and image types become images", () => {
    expect([parseWidth(320), parseWidth("640"), parseWidth(0), parseWidth(1.5), parseWidth(null)]).toEqual([
      320,
      640,
      null,
      null,
      null,
    ]);
    expect([kindForMime("image/png"), kindForMime("IMAGE/SVG+XML"), kindForMime("application/pdf")]).toEqual([
      "image",
      "image",
      "file",
    ]);
  });
});

describe("where a new attachment block goes", () => {
  const type = schema.nodes.image;
  if (type === undefined) {
    throw new Error("no image node");
  }
  const page = doc([paragraph(0, "Hello"), paragraph(1), { type: "bulletList", attrs: { id: pid(2) }, content: [{ type: "listItem", attrs: { id: pid(3) }, content: [paragraph(4, "Item")] }] }]);

  it("goes after the paragraph it lands in, before it at its start, and replaces an empty one", () => {
    expect(placementFor(page, 4, type)).toEqual({ pos: 7, replace: null }); // middle of "Hello"
    expect(placementFor(page, 1, type)).toEqual({ pos: 0, replace: null }); // start of "Hello"
    expect(placementFor(page, 8, type)).toEqual({ pos: 7, replace: pid(1) }); // the empty paragraph
    expect(placementFor(page, 0, type)).toEqual({ pos: 0, replace: null }); // between blocks
  });

  it("stays inside a list item, after the item's paragraph", () => {
    // doc: p(0..7) p(7..9) ul(9) > li(10) > p(11) "Item"
    expect(placementFor(page, 14, type)).toEqual({ pos: 17, replace: null });
  });

  it("leaves a toggle's summary for the toggle's surroundings", () => {
    const toggle = doc([
      {
        type: "details",
        attrs: { id: pid(0) },
        content: [
          { type: "detailsSummary", attrs: { id: pid(1) }, content: [{ type: "text", text: "Sum" }] },
          { type: "detailsContent", attrs: { id: pid(2) }, content: [paragraph(3)] },
        ],
      },
    ]);
    const placed = placementFor(toggle, 3, type); // inside "Sum"
    const $pos = toggle.resolve(placed.pos);
    expect($pos.parent.canReplaceWith($pos.index(), $pos.index(), type)).toBe(true);
    expect($pos.parent.type.name).not.toBe("details");
  });
});

describe("turning attachment blocks into other kinds", () => {
  function state(content: unknown): EditorState {
    return EditorState.create({ schema, doc: doc(content) });
  }

  it("offers the other attachment kind and the wrappers, and keeps the block's identity", () => {
    const start = state([image(0, "A cat")]);
    const block = start.doc.child(0);
    expect(turnIntoKinds(block)).toEqual(ATTACHMENT_KINDS);
    // A paragraph can also become a toggle or a toggle heading (ADR-0040).
    expect(turnIntoKinds(start.schema.nodeFromJSON(paragraph(9)))).toEqual([...TEXT_KINDS, ...TOGGLE_KINDS]);

    const asFile = start.apply(turnInto(start, 0, "file") ?? start.tr).doc.child(0);
    expect(asFile.type.name).toBe("file");
    expect(asFile.attrs).toEqual({ id: pid(0), attachmentId: A, view: null });

    const quoted = start.apply(turnInto(start, 0, "callout") ?? start.tr).doc;
    expect(outline(quoted)).toBe("callout[image]");
    expect(quoted.child(0).child(0).attrs.id).toBe(pid(0));

    expect(turnInto(start, 0, "paragraph")).toBeNull(); // text would drop the image
    expect(turnInto(state([paragraph(0, "x")]), 0, "image")).toBeNull(); // text has no attachment
  });

  it("keeps the attachments of a container turned into text or a list", () => {
    const callout = [{ type: "callout", attrs: { id: pid(0) }, content: [paragraph(1, "One"), image(2), paragraph(3, "Two")] }];
    const asText = state(callout);
    expect(outline(asText.apply(turnInto(asText, 0, "paragraph") ?? asText.tr).doc)).toBe('paragraph"One" image paragraph"Two"');

    const asList = state([{ type: "callout", attrs: { id: pid(0) }, content: [image(2), paragraph(1, "One"), image(4)] }]);
    expect(outline(asList.apply(turnInto(asList, 0, "bulletList") ?? asList.tr).doc)).toBe(
      'bulletList[listItem[paragraph"" image] listItem[paragraph"One" image]]',
    );
  });

  it("duplicates an attachment block with a new block id and the same attachment", () => {
    const start = state([image(0)]);
    const copy = start.apply(duplicateBlock(start, 0) ?? start.tr).doc.child(1);
    expect(copy.attrs).toMatchObject({ id: null, attachmentId: A });
  });
});
