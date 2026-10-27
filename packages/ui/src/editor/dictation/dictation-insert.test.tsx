// Where and how a recognised phrase goes into the page (ADR-0053, `docs/architecture/dictation.md` section 6, "Insertion
// rules"): one transaction at the end of the selection, never over the selected text, joined with `joinPhrase`
// to the text before it in the same block (an inline atom counts as a character), the marks of the cursor, the
// last selection of a blurred editor, nothing on a read-only page, and one undo step per phrase of the real
// editor, whatever the binding's merge interval.

import type { JSONContent } from "@tiptap/core";
import { Editor } from "@tiptap/core";
import { NodeSelection } from "@tiptap/pm/state";
import { act, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { removeMicrophone } from "../ai/recorder-kit";
import { createSchemaExtensions } from "../schema";
import { setPageLock } from "../page-lock";
import { redo, undo } from "../loro-binding";
import { bareEditor, blocksOf, controllerOf, openNotes, placeCursor, selectRange, speak, startAndListen } from "./dictation-test-kit";
import { insertPhrase, textBefore } from "./insert";

afterEach(() => {
  removeMicrophone();
  vi.restoreAllMocks();
});

/** A schema-only editor holding `content` (portable JSON, as the page document has it). */
function editorOf(...blocks: JSONContent[]): Editor {
  const element = document.createElement("div");
  document.body.append(element);
  return new Editor({ element, extensions: createSchemaExtensions(), content: { type: "doc", content: blocks } });
}

const paragraph = (...content: JSONContent[]): JSONContent => ({ type: "paragraph", ...(content.length === 0 ? {} : { content }) });
const text = (value: string, ...marks: string[]): JSONContent => ({ type: "text", text: value, ...(marks.length === 0 ? {} : { marks: marks.map((type) => ({ type })) }) });

describe("insertPhrase at the end of the selection", () => {
  it("puts the phrase after the cursor, with a space between words, and leaves the cursor after it", () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    expect(insertPhrase(editor, "world.")).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello world."]);
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.from).toBe("Hello world.".length + 1);
    // The next phrase joins what is there now: after a full stop, a space.
    expect(insertPhrase(editor, "and more")).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello world. and more"]);
  });

  it("does not add a space at the start of a block, after white space or an opening mark, nor before a closing mark", () => {
    const empty = bareEditor("");
    expect(insertPhrase(empty, "Hello")).toBe(true);
    expect(blocksOf(empty)).toEqual(["Hello"]);

    const spaced = bareEditor("Hello ");
    placeCursor(spaced, 7);
    insertPhrase(spaced, "there");
    expect(blocksOf(spaced)).toEqual(["Hello there"]);

    const opening = bareEditor("say (");
    placeCursor(opening, 6);
    insertPhrase(opening, "hello");
    expect(blocksOf(opening)).toEqual(["say (hello"]);

    const closing = bareEditor("Hello");
    placeCursor(closing, 6);
    insertPhrase(closing, ", world");
    expect(blocksOf(closing)).toEqual(["Hello, world"]);
  });

  it("collapses the white space inside a phrase and ignores a phrase of nothing", () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    expect(insertPhrase(editor, "  a   b\n c  ")).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello a b c"]);
    const before = JSON.stringify(editor.getJSON());
    expect(insertPhrase(editor, "  \n ")).toBe(false);
    expect(JSON.stringify(editor.getJSON())).toBe(before);
  });

  it("never deletes or replaces the selected text: the phrase goes after the selection", () => {
    const editor = bareEditor("Hello world");
    selectRange(editor, 1, 6); // "Hello"
    expect(insertPhrase(editor, "there.")).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello there. world"]);
    // Afterwards a cursor after the new text, and nothing is selected.
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.from).toBe("Hello there.".length + 1);
  });

  it("joins to the last character of the same text block, never the one before", () => {
    // The cursor at the start of the second block: the first block's last letter does not count.
    const editor = bareEditor("One", "");
    placeCursor(editor, editor.state.doc.content.size - 1);
    insertPhrase(editor, "Two");
    expect(blocksOf(editor)).toEqual(["One", "Two"]);
  });

  it("counts an inline atom as a character that is not a space, so a space follows it", () => {
    const editor = editorOf(paragraph(text("on "), { type: "dateMention", attrs: { date: "2026-10-16" } }));
    placeCursor(editor, editor.state.doc.content.size - 1);
    expect(textBefore(editor.state.doc.child(0), editor.state.doc.child(0).content.size)).toBe("on ￼");
    insertPhrase(editor, "we meet");
    expect(editor.state.doc.child(0).lastChild?.text).toBe(" we meet");
    expect(editor.state.doc.textContent).toContain(" we meet");
  });

  it("keeps the capital of a phrase after an inline atom, which does not leave a sentence open", () => {
    const editor = editorOf(paragraph(text("on "), { type: "dateMention", attrs: { date: "2026-10-16" } }));
    placeCursor(editor, editor.state.doc.content.size - 1);
    insertPhrase(editor, "We meet");
    expect(editor.state.doc.child(0).lastChild?.text).toBe(" We meet");
  });

  it("reads a line break as white space", () => {
    const editor = editorOf(paragraph(text("first"), { type: "hardBreak" }));
    placeCursor(editor, editor.state.doc.content.size - 1);
    insertPhrase(editor, "second");
    expect(editor.state.doc.child(0).lastChild?.text).toBe("second");
  });

  it("lowers the capital of a phrase that continues the sentence, and keeps it where a sentence starts", () => {
    // The engine capitalises every phrase; the cursor follows words that are not the end of a sentence.
    const open = bareEditor("I went to the");
    placeCursor(open, open.state.doc.content.size - 1);
    insertPhrase(open, "Store and bought milk.");
    expect(blocksOf(open)).toEqual(["I went to the store and bought milk."]);
    // Its full stop stays (a limit), so the next phrase follows the end of a sentence: the capital stays.
    insertPhrase(open, "Then we left.");
    expect(blocksOf(open)).toEqual(["I went to the store and bought milk. Then we left."]);

    // After a comma, with a space typed before, a number, and with the cursor in the middle of the text.
    const comma = bareEditor("Well, ");
    placeCursor(comma, 7);
    insertPhrase(comma, "Then we left");
    expect(blocksOf(comma)).toEqual(["Well, then we left"]);

    // The start of a block and the start of a line after a line break keep the capital.
    const empty = bareEditor("One", "");
    placeCursor(empty, empty.state.doc.content.size - 1);
    insertPhrase(empty, "The store");
    expect(blocksOf(empty)).toEqual(["One", "The store"]);
    const broken = editorOf(paragraph(text("first"), { type: "hardBreak" }));
    placeCursor(broken, broken.state.doc.content.size - 1);
    insertPhrase(broken, "Second");
    expect(broken.state.doc.child(0).lastChild?.text).toBe("Second");

    // "I", acronyms and words with a later capital are not lowered.
    const names = bareEditor("we saw");
    placeCursor(names, names.state.doc.content.size - 1);
    insertPhrase(names, "NASA and I");
    expect(blocksOf(names)).toEqual(["we saw NASA and I"]);
  });

  it("applies the marks of the cursor, as typing does", () => {
    const bold = editorOf(paragraph(text("Bold", "bold")));
    placeCursor(bold, 5);
    insertPhrase(bold, "again");
    // One text node, as the same marks make it: the new words are bold too.
    const last = bold.state.doc.child(0).lastChild;
    expect(bold.state.doc.child(0).childCount).toBe(1);
    expect(last?.text).toBe("Bold again");
    expect(last?.marks.map((mark) => mark.type.name)).toEqual(["bold"]);

    // A link does not continue past its end, as in typing.
    const linked = editorOf(paragraph({ type: "text", text: "site", marks: [{ type: "link", attrs: { href: "https://example.com" } }] }));
    placeCursor(linked, 5);
    insertPhrase(linked, "is good");
    expect(linked.state.doc.child(0).lastChild?.marks).toHaveLength(0);
  });

  it("goes in a new paragraph after a block that is selected as a whole (a divider)", () => {
    const editor = editorOf(paragraph(text("Above")), { type: "horizontalRule" });
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize)));
    });
    expect(insertPhrase(editor, "Below")).toBe(true);
    expect(editor.state.doc.childCount).toBe(3);
    expect(editor.state.doc.child(2).type.name).toBe("paragraph");
    expect(editor.state.doc.child(2).textContent).toBe("Below");
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));
  });

  it("writes nothing on a read-only page or a destroyed editor", () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    editor.setEditable(false);
    expect(insertPhrase(editor, "world")).toBe(false);
    expect(blocksOf(editor)).toEqual(["Hello"]);
    editor.setEditable(true);
    editor.destroy();
    expect(insertPhrase(editor, "world")).toBe(false);
  });

  it("is one transaction per phrase", () => {
    const editor = bareEditor("Hello");
    placeCursor(editor, 6);
    const changes: number[] = [];
    editor.on("transaction", ({ transaction }) => {
      changes.push(transaction.docChanged ? 1 : 0);
    });
    insertPhrase(editor, "one two three");
    expect(changes).toEqual([1]);
  });
});

describe("in the page", () => {
  it("uses the last selection when the editor is blurred (the person clicked the bar or another window)", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello world"] });
    placeCursor(editor, 6); // after "Hello"
    await startAndListen(editor);
    act(() => {
      editor.commands.blur();
    });
    expect(editor.isFocused).toBe(false);
    speak(microphone);
    expect(blocksOf(editor)).toEqual(["Hello first phrase. world"]);
    expect(editor.state.selection.from).toBe("Hello first phrase.".length + 1);
    expect(editor.isFocused).toBe(false); // nothing took the focus
    // The next phrase follows the first, at the cursor it left.
    speak(microphone);
    expect(blocksOf(editor)).toEqual(["Hello first phrase. and second world"]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("does not delete a selection that is there when a phrase comes", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello world"] });
    await startAndListen(editor);
    selectRange(editor, 1, 6);
    speak(microphone);
    expect(blocksOf(editor)).toEqual(["Hello first phrase. world"]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("writes nothing into a page that became read-only while a phrase was on its way", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    const controller = await startAndListen(editor);
    speak(microphone, 0.1); // half a phrase heard: the engine still owes one when it is stopped
    act(() => {
      setPageLock(editor, true);
    });
    expect(editor.isEditable).toBe(false);
    // The lock stopped the session; the phrase it finished with has nowhere to go.
    await waitFor(() => {
      expect(controller.state.get().phase).toBe("idle");
    });
    expect(blocksOf(editor)).toEqual(["Hello"]);
  });
});

describe("one undo step per phrase", () => {
  const undoOnce = (editor: Editor): boolean => {
    let done = false;
    act(() => {
      done = undo(editor.state, editor.view.dispatch);
    });
    return done;
  };

  it("takes away the last phrase and only it, for phrases closer together than the merge interval", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    await startAndListen(editor);
    // Three phrases within a few milliseconds: the binding's interval (500 ms) alone would make them one step.
    speak(microphone, 0.8);
    expect(blocksOf(editor)).toEqual(["Hello first phrase. and second third."]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello first phrase. and second"]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello first phrase."]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello"]);
    // And the session goes on: an undo does not stop it.
    expect(controllerOf(editor).state.get().phase).toBe("listening");
    act(() => {
      redo(editor.state, editor.view.dispatch);
    });
    expect(blocksOf(editor)).toEqual(["Hello first phrase."]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("holds for two phrases 600 ms apart", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    await startAndListen(editor);
    speak(microphone);
    await new Promise((resolve) => setTimeout(resolve, 600));
    speak(microphone);
    expect(blocksOf(editor)).toEqual(["Hello first phrase. and second"]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello first phrase."]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello"]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("keeps a phrase apart from what was typed just before it", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    await startAndListen(editor);
    act(() => {
      editor.commands.insertContent(" typed");
    });
    speak(microphone); // a few milliseconds after the keystroke
    expect(blocksOf(editor)).toEqual(["Hello typed first phrase."]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello typed"]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("keeps a phrase apart from what is typed right after it", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    await startAndListen(editor);
    speak(microphone);
    act(() => {
      editor.commands.insertContent(" typed");
    });
    expect(blocksOf(editor)).toEqual(["Hello first phrase. typed"]);
    // The keystroke is its own step: the first undo takes it, the second the phrase.
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello first phrase."]);
    expect(undoOnce(editor)).toBe(true);
    expect(blocksOf(editor)).toEqual(["Hello"]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });

  it("goes back to the merge interval for typing once the phrase is behind: fast typing is still one step", async () => {
    const { editor, microphone } = await openNotes({ texts: ["Hello"] });
    await startAndListen(editor);
    speak(microphone);
    act(() => {
      editor.commands.insertContent(" a");
      editor.commands.insertContent("b");
      editor.commands.insertContent("c");
    });
    expect(blocksOf(editor)).toEqual(["Hello first phrase. abc"]);
    expect(undoOnce(editor)).toBe(true); // "a", "b" and "c" came within the interval: one step
    expect(blocksOf(editor)).toEqual(["Hello first phrase."]);
    await act(async () => {
      await controllerOf(editor).stop();
    });
  });
});
