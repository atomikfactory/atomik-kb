// What a recording or a transcription adds to the page: the audio block and the "Transcript" toggle in
// one transaction (one Loro change, one undo step), the summary under the toggle in a second one, found
// by block id however the page changed meanwhile. Over a plain Tiptap editor bound to a Loro document.

import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it } from "vitest";

import { t } from "../../i18n";
import { loroDocFrom, paragraphs, pid, tick } from "../../test/editor";
import { LoroBinding } from "../extensions";
import { undo } from "../loro-binding";
import { createSchemaExtensions } from "../schema";
import { applyMarkdown, nodesOfMarkdown } from "./apply";
import { insertRecording, insertSummary, insertTranscriptAfter } from "./insert-transcript";
import { insertMarkdownAtEnd, setActiveEditor } from "./active-editor";

const mounted: { editor: Editor; element: HTMLElement }[] = [];

function mount(texts: string[], doc: LoroDoc = loroDocFrom(paragraphs(...texts))): { editor: Editor; doc: LoroDoc } {
  const element = document.createElement("div");
  document.body.append(element);
  const content: JSONContent = {
    type: "doc",
    content: texts.map((text, index) => ({
      type: "paragraph",
      attrs: { id: pid(index) },
      ...(text === "" ? {} : { content: [{ type: "text", text }] }),
    })),
  };
  const editor = new Editor({
    element,
    extensions: [...createSchemaExtensions(), LoroBinding.configure({ doc, readOnly: false })],
    content,
    editable: true,
  });
  mounted.push({ editor, element });
  return { editor, doc };
}

afterEach(() => {
  setActiveEditor(null);
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
});

const ATTACHMENT = "a0000000-0000-4000-8000-0000000000a1";
const SEGMENTS = [
  { startMs: 0, text: "Hello everyone." },
  { startMs: 95_000, text: "Second **point**." },
];

function outline(editor: Editor): string[] {
  const lines: string[] = [];
  editor.state.doc.forEach((node) => {
    lines.push(`${node.type.name}:${node.textContent}`);
  });
  return lines;
}

describe("insertRecording", () => {
  it("replaces the empty line the recording started on with the audio block and the toggle, in one transaction", async () => {
    const { editor, doc } = mount(["Before", "", "After"]);
    await tick();
    const before = doc.oplogVersion();
    const inserted = insertRecording(editor, { anchorId: pid(1), attachmentId: ATTACHMENT, segments: SEGMENTS });

    expect(inserted?.toggleId).toEqual(expect.any(String));
    expect(outline(editor)).toEqual([
      "paragraph:Before",
      "file:",
      `details:${t("transcribe.toggleTitle")}[00:00] Hello everyone.[01:35] Second **point**.`,
      "paragraph:After",
    ]);
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);

    const audio = editor.state.doc.child(1);
    expect(audio.attrs).toMatchObject({ id: inserted?.audioId, attachmentId: ATTACHMENT, view: "preview" });
    const toggle = editor.state.doc.child(2);
    expect(toggle.attrs.id).toBe(inserted?.toggleId);
    expect(toggle.attrs.open ?? false).toBe(false);
    expect(toggle.child(0).type.name).toBe("detailsSummary");
    expect(toggle.child(1).type.name).toBe("detailsContent");
    expect(toggle.child(1).childCount).toBe(2);

    undo(editor.state, editor.view.dispatch);
    expect(outline(editor)).toEqual(["paragraph:Before", "paragraph:", "paragraph:After"]);
  });

  it("keeps what was said as text: a bold timestamp, nothing else formatted", async () => {
    const { editor } = mount([""]);
    await tick();
    insertRecording(editor, { anchorId: pid(0), attachmentId: ATTACHMENT, segments: SEGMENTS });
    const paragraph = editor.state.doc.child(1).child(1).child(1);
    expect(paragraph.child(0).marks.map((mark) => mark.type.name)).toEqual(["bold"]);
    expect(paragraph.child(0).text).toBe("[01:35]");
    expect(paragraph.child(1).marks).toHaveLength(0);
    expect(paragraph.child(1).text).toBe(" Second **point**.");
  });

  it("inserts after the block when it holds text, and at the end of the page when the block is gone", async () => {
    const first = mount(["Before", "Text", "After"]);
    await tick();
    insertRecording(first.editor, { anchorId: pid(1), attachmentId: ATTACHMENT, segments: [] });
    expect(outline(first.editor)).toEqual(["paragraph:Before", "paragraph:Text", "file:", "paragraph:After"]);

    const second = mount(["Before", "After"]);
    await tick();
    insertRecording(second.editor, { anchorId: pid(9), attachmentId: ATTACHMENT, segments: [] });
    expect(outline(second.editor)).toEqual(["paragraph:Before", "paragraph:After", "file:"]);
    insertRecording(second.editor, { anchorId: null, attachmentId: ATTACHMENT, segments: [] });
    expect(outline(second.editor)).toEqual(["paragraph:Before", "paragraph:After", "file:", "file:"]);
  });

  it("makes no toggle when nobody spoke", async () => {
    const { editor } = mount([""]);
    await tick();
    const inserted = insertRecording(editor, {
      anchorId: pid(0),
      attachmentId: ATTACHMENT,
      segments: [{ startMs: 0, text: "  " }, { startMs: 30_000, text: "" }],
    });
    expect(inserted?.toggleId).toBeNull();
    expect(outline(editor)).toEqual(["file:"]);
  });

  it("does nothing in a read-only editor", async () => {
    const { editor } = mount(["x"]);
    await tick();
    editor.setEditable(false);
    expect(insertRecording(editor, { anchorId: pid(0), attachmentId: ATTACHMENT, segments: SEGMENTS })).toBeNull();
    expect(outline(editor)).toEqual(["paragraph:x"]);
  });
});

describe("insertSummary", () => {
  it("puts the summary right under the toggle in a second transaction, found by id after the page changed", async () => {
    const { editor, doc } = mount(["Before", ""]);
    await tick();
    const inserted = insertRecording(editor, { anchorId: pid(1), attachmentId: ATTACHMENT, segments: SEGMENTS });
    // The person keeps writing above while the summary is made.
    editor.chain().insertContentAt(0, { type: "paragraph", content: [{ type: "text", text: "Typed meanwhile" }] }).run();

    const before = doc.oplogVersion();
    const summary = "### Summary\nWe met.\n\n### Key points\n- One\n\n### Action items\n- [ ] Follow up";
    expect(insertSummary(editor, inserted?.toggleId ?? "", summary)).toBe(true);
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);

    const names = editor.state.doc.children.map((node) => node.type.name);
    expect(names).toEqual(["paragraph", "paragraph", "file", "details", "heading", "paragraph", "heading", "bulletList", "heading", "taskList"]);
    // (The summary is one Loro change of its own, as counted above; Loro's undo manager merges steps that
    // are less than a second apart, so a test that runs them back to back cannot tell them apart by undo.)
  });

  it("gives up quietly when the toggle was deleted", async () => {
    const { editor } = mount(["x"]);
    await tick();
    expect(insertSummary(editor, "00000000-0000-4000-8000-00000000dead", "text")).toBe(false);
    expect(outline(editor)).toEqual(["paragraph:x"]);
  });
});

describe("insertTranscriptAfter", () => {
  it("puts the toggle right after the file block", async () => {
    const { editor } = mount(["Intro", "Outro"]);
    await tick();
    insertRecording(editor, { anchorId: pid(0), attachmentId: ATTACHMENT, segments: [] });
    const fileId = editor.state.doc.child(1).attrs.id as string;
    const inserted = insertTranscriptAfter(editor, fileId, SEGMENTS);
    expect(inserted?.toggleId).toEqual(expect.any(String));
    expect(editor.state.doc.children.map((node) => node.type.name)).toEqual(["paragraph", "file", "details", "paragraph"]);
    expect(insertTranscriptAfter(editor, fileId, [])).toBeNull();
    expect(insertTranscriptAfter(editor, "00000000-0000-4000-8000-00000000dead", SEGMENTS)).toBeNull();
  });
});

describe("applyMarkdown and the active editor", () => {
  it("builds blocks of the live schema from Markdown and writes nothing for text that holds none", async () => {
    const { editor } = mount(["x"]);
    await tick();
    expect(nodesOfMarkdown(editor, "# T\n\n- a").map((node) => node.type.name)).toEqual(["heading", "bulletList"]);
    expect(nodesOfMarkdown(editor, "   ")).toEqual([]);
    expect(applyMarkdown(editor, "", { mode: "below", pos: 1, replaceEmpty: false })).toBe(false);
  });

  it("replaces whole blocks in one Loro change", async () => {
    const { editor, doc } = mount(["one", "two", "three"]);
    await tick();
    const first = editor.state.doc.child(0).nodeSize;
    const second = first + editor.state.doc.child(1).nodeSize;
    const before = doc.oplogVersion();
    expect(applyMarkdown(editor, "# New\n\ntext", { mode: "replaceBlocks", from: 0, to: second })).toBe(true);
    expect(outline(editor)).toEqual(["heading:New", "paragraph:text", "paragraph:three"]);
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    undo(editor.state, editor.view.dispatch);
    expect(outline(editor)).toEqual(["paragraph:one", "paragraph:two", "paragraph:three"]);
  });

  it("appends Markdown at the end of the front page for the chat panel, in one undo step", async () => {
    const { editor, doc } = mount(["one", ""]);
    await tick();
    expect(insertMarkdownAtEnd("# Answer")).toBe(false); // no page registered yet
    setActiveEditor({ pageId: "00000000-0000-4000-8000-00000000f001", editor });
    const before = doc.oplogVersion();
    expect(insertMarkdownAtEnd("# Answer\n\n- [1] source")).toBe(true);
    // The empty line the page ended with is where the blocks go.
    expect(outline(editor)).toEqual(["paragraph:one", "heading:Answer", "bulletList:[1] source"]);
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    undo(editor.state, editor.view.dispatch);
    expect(outline(editor)).toEqual(["paragraph:one", "paragraph:"]);

    expect(insertMarkdownAtEnd("more")).toBe(true);
    expect(outline(editor).at(-1)).toBe("paragraph:more");
    expect(insertMarkdownAtEnd("   ")).toBe(false);
    editor.setEditable(false);
    expect(insertMarkdownAtEnd("nope")).toBe(false);
  });
});
