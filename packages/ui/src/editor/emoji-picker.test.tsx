// Emoji in text: the `:` picker (opens after two characters, only after whitespace or at the
// start of a block, never in code), choosing an emoji (plain text, the query replaced), the
// closing colon of an exact shortcode, Escape, the `/emoji` command, the limit of the list and
// what a choice does to the Loro document.

import type { JsonValue } from "@atomik-kb/core-client";
import { loroToPortable } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it } from "vitest";

import { t } from "../i18n";
import { paragraphs, pid, tick } from "../test/editor";
import { STAND_IN_EMOJI } from "../test/emoji";
import { openPage, renderPages } from "../test/page";
import { setEmojiData } from "./emoji-data";
import type { EmojiRecord } from "./emoji-data";
import { loroSyncPluginKey } from "./loro-binding";
import { MAX_EMOJI_RESULTS } from "./emoji-picker";

afterEach(() => {
  setEmojiData(STAND_IN_EMOJI); // some tests swap the data
});

/** Opens a page of one empty paragraph and puts the caret in it. */
async function openEmpty(): Promise<Editor> {
  const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
  const editor = await openPage(user, "Notes");
  placeCursor(editor, 1);
  return editor;
}

function placeCursor(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** Types `text` the way the browser does (input rules only react to typed text). */
function typeText(editor: Editor, text: string): void {
  for (const character of text) {
    const { from, to } = editor.state.selection;
    const handled = editor.view.someProp("handleTextInput", (handle) =>
      handle(editor.view, from, to, character, () => editor.state.tr.insertText(character, from, to)),
    );
    if (handled !== true) {
      editor.view.dispatch(editor.state.tr.insertText(character, from, to));
    }
  }
}

/** Presses `key` as the browser delivers it to the editor; whether something took it. */
function press(editor: Editor, key: string): boolean {
  return (
    editor.view.someProp("handleKeyDown", (handle) => handle(editor.view, new KeyboardEvent("keydown", { key }))) === true
  );
}

/** What the paragraph of the page holds. */
function text(editor: Editor): string {
  return editor.state.doc.child(0).textContent;
}

/** A paragraph `a  b` of inline code with the caret between the spaces (typed text there is code too). */
function insideCode(editor: Editor): void {
  editor.commands.setContent({
    type: "doc",
    content: [{ type: "paragraph", content: [{ type: "text", text: "a  b", marks: [{ type: "code" }] }] }],
  });
  placeCursor(editor, 3);
}

function menuNow(): HTMLElement | null {
  return screen.queryByRole("listbox", { name: t("emoji.label") });
}

async function findMenu(): Promise<HTMLElement> {
  return screen.findByRole("listbox", { name: t("emoji.label") });
}

/** Lets what is under way (the search, the menu) finish. */
async function settle(): Promise<void> {
  await act(async () => {
    await tick(20);
  });
}

/** The shortcodes the open menu lists, in order. */
function shown(menu: HTMLElement): (string | null | undefined)[] {
  return within(menu)
    .getAllByRole("option")
    .map((option) => option.querySelector(".suggestion-title")?.textContent);
}

/** Waits until the open menu lists exactly `expected` (it is looked up again each time: an old one may still be leaving). */
async function waitForShown(expected: string[]): Promise<void> {
  await waitFor(() => {
    const menu = menuNow();
    expect(menu).not.toBeNull();
    expect(shown(menu as HTMLElement)).toEqual(expected);
  });
}

describe("the trigger", () => {
  it("opens after two characters with the emoji that match, best first", async () => {
    const editor = await openEmpty();
    typeText(editor, ":");
    await settle();
    expect(menuNow()).toBeNull();
    typeText(editor, "s");
    await settle();
    expect(menuNow()).toBeNull(); // a colon and one letter: prose
    typeText(editor, "m");
    await waitForShown([":smile:", ":smiley:", ":smiling_face_with_smiling_eyes:", ":slightly_smiling_face:"]);
    expect(text(editor)).toBe(":sm"); // nothing is inserted until a choice is made
  });

  it("shows each emoji as its glyph, named, with its shortcode", async () => {
    const editor = await openEmpty();
    typeText(editor, ":tad");
    const menu = await findMenu();
    await waitForShown([":tada:"]);
    const option = within(menu).getByRole("option");
    expect(within(option).getByRole("img", { name: "party popper" })).toHaveTextContent("🎉");
    expect(menu.closest(".suggestion-menu")).toHaveClass("emoji-menu");
  });

  it("finds :+1, with its plus", async () => {
    const editor = await openEmpty();
    typeText(editor, ":+1");
    await waitForShown([":+1:"]);
  });

  it("finds :tad", async () => {
    const editor = await openEmpty();
    typeText(editor, ":tad");
    await waitForShown([":tada:"]);
  });

  it("opens only at the start of a block or after whitespace", async () => {
    const editor = await openEmpty();
    typeText(editor, "foo:smi");
    await settle();
    expect(menuNow()).toBeNull();

    for (const prose of ["see https://example.com/smi", "at 10:30", "a:b:smi"]) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      placeCursor(editor, 1);
      typeText(editor, prose);
      await settle();
      expect(menuNow(), prose).toBeNull();
    }

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    typeText(editor, "foo :smi");
    expect(await findMenu()).toBeInTheDocument(); // after a space
    expect(text(editor)).toBe("foo :smi");
  });

  it("opens after a line break, and not right after a mark's end", async () => {
    const editor = await openEmpty();
    typeText(editor, "one");
    expect(editor.commands.setHardBreak()).toBe(true);
    typeText(editor, ":smi");
    expect(await findMenu()).toBeInTheDocument();

    editor.commands.setContent({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "bold", marks: [{ type: "bold" }] }] }],
    });
    placeCursor(editor, 5);
    typeText(editor, ":smi"); // the text node of `:smi` starts where the bold ends, but a letter stands before it
    await settle();
    expect(menuNow()).toBeNull();
  });

  it("closes at the space that ends the shortcode, leaving the text", async () => {
    const editor = await openEmpty();
    typeText(editor, ":smi");
    expect(await findMenu()).toBeInTheDocument();
    typeText(editor, " ");
    await waitFor(() => {
      expect(menuNow()).toBeNull();
    });
    expect(text(editor)).toBe(":smi ");
  });

  it("never opens in a code block, the math block or inline code", async () => {
    const editor = await openEmpty();
    expect(editor.commands.setCodeBlock()).toBe(true);
    typeText(editor, ":smi");
    await settle();
    expect(menuNow()).toBeNull();
    expect(text(editor)).toBe(":smi");

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    expect(editor.commands.setMathBlock()).toBe(true);
    typeText(editor, ":smi");
    await settle();
    expect(menuNow()).toBeNull();
    expect(editor.state.doc.child(0).type.name).toBe("mathBlock");
    expect(text(editor)).toBe(":smi");

    insideCode(editor);
    typeText(editor, ":smi");
    await settle();
    expect(menuNow()).toBeNull();
    expect(editor.state.doc.child(0).childCount).toBe(1); // all of it is one span of code
    expect(text(editor)).toBe("a :smi b");
  });

  it("lists at most eight emoji", async () => {
    const many: Record<string, EmojiRecord> = {};
    for (let index = 0; index < 30; index += 1) {
      many[`face-${String(index)}`] = {
        name: `face ${String(index)}`,
        slug: `face_${String(index).padStart(2, "0")}`,
        group: "Smileys & Emotion",
      };
    }
    setEmojiData(many);
    expect(MAX_EMOJI_RESULTS).toBe(8);
    const editor = await openEmpty();
    typeText(editor, ":fa");
    await waitFor(() => {
      expect(shown(menuNow() as HTMLElement)).toHaveLength(8);
    });
    expect(shown(menuNow() as HTMLElement)[0]).toBe(":face_00:");
  });

  it("stays shut while a page is searched for with [[, whose title may hold a colon", async () => {
    const editor = await openEmpty();
    typeText(editor, "[[Plan :ta");
    expect(await screen.findByText(t("picker.empty"))).toBeInTheDocument(); // the page picker is open
    await settle();
    expect(menuNow()).toBeNull(); // `:ta` would list the tada otherwise
    expect(text(editor)).toBe("[[Plan :ta");
  });

  it("leaves the arrow keys to the editor while no emoji is listed, and takes them when some are", async () => {
    const editor = await openEmpty();
    typeText(editor, ":zzq"); // hidden: nothing matches
    await settle();
    for (const key of ["ArrowUp", "ArrowDown", "PageUp", "PageDown"]) {
      expect(press(editor, key), key).toBe(false);
    }

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    typeText(editor, ":smi");
    await waitForShown([":smile:", ":smiley:", ":smiling_face_with_smiling_eyes:", ":slightly_smiling_face:"]);
    expect(press(editor, "ArrowDown")).toBe(true);
    expect(press(editor, "ArrowUp")).toBe(true);
  });

  it("shows no menu, not even a No results row, when nothing matches", async () => {
    const editor = await openEmpty();
    typeText(editor, ":zzq");
    await settle();
    expect(menuNow()).toBeNull();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.queryByText(t("slash.empty"))).not.toBeInTheDocument();
    expect(text(editor)).toBe(":zzq");
  });
});

describe("choosing", () => {
  it("puts the emoji in the text with Enter, in place of the query, as plain text", async () => {
    const editor = await openEmpty();
    typeText(editor, "Well done :smi");
    await waitForShown([":smile:", ":smiley:", ":smiling_face_with_smiling_eyes:", ":slightly_smiling_face:"]);
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(text(editor)).toBe("Well done 😄");
    expect(editor.getJSON().content?.[0]?.content).toEqual([{ type: "text", text: "Well done 😄" }]); // text: no node
    expect(menuNow()).toBeNull();
    expect(editor.state.selection.from).toBe(1 + "Well done 😄".length); // the caret is after it
    expect(editor.state.doc.childCount).toBe(1); // Enter did not split the block

    typeText(editor, "!"); // typing goes on, the menu stays shut
    expect(text(editor)).toBe("Well done 😄!");
    await settle();
    expect(menuNow()).toBeNull();
  });

  it("chooses with Tab, with the arrow keys and with a click", async () => {
    const editor = await openEmpty();
    typeText(editor, ":smi");
    await waitForShown([":smile:", ":smiley:", ":smiling_face_with_smiling_eyes:", ":slightly_smiling_face:"]);
    fireEvent.keyDown(editor.view.dom, { key: "Tab" });
    expect(text(editor)).toBe("😄");

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    typeText(editor, ":smi");
    await waitForShown([":smile:", ":smiley:", ":smiling_face_with_smiling_eyes:", ":slightly_smiling_face:"]);
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(text(editor)).toBe("😃"); // the second one

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    typeText(editor, ":tad");
    const option = within(await findMenu()).getByRole("option");
    fireEvent.click(option);
    expect(text(editor)).toBe("🎉");
    await waitFor(() => {
      expect(menuNow()).toBeNull();
    });
  });

  it("takes the formatting of the words around it", async () => {
    const editor = await openEmpty();
    editor.commands.setContent({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "Hi", marks: [{ type: "bold" }] }] }],
    });
    placeCursor(editor, 3);
    typeText(editor, " :tad");
    await waitForShown([":tada:"]);
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(text(editor)).toBe("Hi 🎉");
    const last = editor.state.doc.child(0).lastChild;
    expect(last?.text).toContain("🎉");
    expect(last?.marks.map((mark) => mark.type.name)).toEqual(["bold"]);
  });

  it("leaves the text alone on Escape, and stays shut while the same shortcode is edited", async () => {
    const editor = await openEmpty();
    typeText(editor, ":smi");
    await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(menuNow()).toBeNull();
    });
    expect(text(editor)).toBe(":smi"); // nothing inserted

    typeText(editor, "l");
    await settle();
    expect(menuNow()).toBeNull();
    expect(text(editor)).toBe(":smil");

    // A new colon after a space is a new shortcode.
    typeText(editor, " :tad");
    expect(await findMenu()).toBeInTheDocument();
  });

  it("takes Enter for a new line when no menu is open", async () => {
    const editor = await openEmpty();
    typeText(editor, ":s");
    await settle();
    expect(menuNow()).toBeNull();
    expect(press(editor, "Enter")).toBe(true);
    expect(editor.state.doc.childCount).toBe(2);
    expect(editor.state.doc.child(0).textContent).toBe(":s");
  });
});

describe("the closing colon", () => {
  it("completes a shortcode that names an emoji exactly, at once, and Backspace brings the text back", async () => {
    const editor = await openEmpty();
    typeText(editor, "Done :tada");
    await findMenu(); // the data is loaded by now, the menu is open
    typeText(editor, ":");
    expect(text(editor)).toBe("Done 🎉");
    await waitFor(() => {
      expect(menuNow()).toBeNull();
    });
    expect(editor.getJSON().content?.[0]?.content).toEqual([{ type: "text", text: "Done 🎉" }]);

    expect(press(editor, "Backspace")).toBe(true);
    expect(text(editor)).toBe("Done :tada:");
  });

  it("is done with the emoji: a colon typed after it is only a colon", async () => {
    const editor = await openEmpty();
    typeText(editor, ":tada::");
    expect(text(editor)).toBe("🎉:");
  });

  it("takes slugs, aliases and capitals, and the plus and minus of :+1: and :-1:", async () => {
    const editor = await openEmpty();
    for (const [typed, emoji] of [
      [":party_popper:", "🎉"],
      [":TADA:", "🎉"],
      [":smile:", "😄"],
      [":+1:", "👍"],
      [":-1:", "👎"],
      [":100:", "💯"],
      [":x:", "❌"],
      [":fire:", "🔥"],
    ] as const) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      placeCursor(editor, 1);
      typeText(editor, typed.slice(0, -1));
      if (typed.length > 4) {
        await findMenu();
      }
      typeText(editor, ":");
      expect(text(editor), typed).toBe(emoji);
    }
  });

  it("leaves what is not an exact shortcode, or not after whitespace, or in code, as typed", async () => {
    const editor = await openEmpty();
    for (const typed of [":smi:", ":nope:", "foo:tada:", "a:tada:", "::"]) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      placeCursor(editor, 1);
      typeText(editor, typed);
      expect(text(editor), typed).toBe(typed);
    }

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    editor.commands.setCodeBlock();
    typeText(editor, ":tada:");
    expect(text(editor)).toBe(":tada:");

    insideCode(editor);
    typeText(editor, ":tada:");
    expect(text(editor)).toBe("a :tada: b");
  });
});

describe("/emoji", () => {
  it("replaces the command with a colon and opens the picker on a short default list", async () => {
    const editor = await openEmpty();
    editor.commands.insertContent("/emoji");
    const slash = await screen.findByRole("listbox", { name: t("slash.label") });
    expect(within(slash).getAllByRole("option")[0]).toHaveTextContent("Emoji");
    expect(within(slash).getAllByRole("option")[0]).toHaveTextContent(t("slash.emoji.hint"));
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    expect(text(editor)).toBe(":");
    await waitForShown([":smile:", ":+1:", ":heart:", ":tada:", ":fire:", ":eyes:", ":rocket:", ":sparkles:"]);
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();

    typeText(editor, "roc"); // narrows the list as usual
    await waitForShown([":rocket:"]);
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(text(editor)).toBe("🚀");
    expect(editor.getJSON().content?.[0]?.content).toEqual([{ type: "text", text: "🚀" }]);
  });

  it("finds Emoji by its keywords", async () => {
    const editor = await openEmpty();
    for (const typed of ["/emo", "/smiley", "/emoticon", "/icon"]) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      placeCursor(editor, 1);
      editor.commands.insertContent(typed);
      const slash = await screen.findByRole("listbox", { name: t("slash.label") });
      await waitFor(() => {
        expect(
          within(slash)
            .getAllByRole("option")
            .map((option) => option.querySelector(".suggestion-title")?.textContent),
        ).toContain("Emoji");
      }, { timeout: 4000 });
    }
  });

  it("closes with the colon removed, and the next colon then waits for its characters again", async () => {
    const editor = await openEmpty();
    editor.commands.insertContent("/emoji");
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await findMenu();
    editor.commands.deleteRange({ from: 1, to: 2 }); // the colon
    await waitFor(() => {
      expect(menuNow()).toBeNull();
    });

    typeText(editor, ":s");
    await settle();
    expect(menuNow()).toBeNull();
  });

  it("opens where the colon does not follow whitespace, because it was asked for", async () => {
    const editor = await openEmpty();
    editor.commands.setContent({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "ab", marks: [{ type: "bold" }] },
            { type: "text", text: "/emoji" },
          ],
        },
      ],
    });
    placeCursor(editor, 1 + "ab/emoji".length);
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(text(editor)).toBe("ab:");
    await waitForShown([":smile:", ":+1:", ":heart:", ":tada:", ":fire:", ":eyes:", ":rocket:", ":sparkles:"]);
  });

  it("leaves the colon as plain text in inline code, where no picker opens", async () => {
    const editor = await openEmpty();
    editor.commands.setContent({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "a /emoji b", marks: [{ type: "code" }] }] }],
    });
    placeCursor(editor, 1 + "a /emoji".length);
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(text(editor)).toBe("a : b");
    await settle();
    expect(menuNow()).toBeNull();

    typeText(editor, "sm"); // the colon is plain text and stays so: no menu waits behind it
    await settle();
    expect(menuNow()).toBeNull();
  });
});

describe("the Loro document", () => {
  /** Every op `doc` recorded since `since`, grouped by change. */
  function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
    return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
  }

  function docOf(editor: Editor): LoroDoc {
    const doc = loroSyncPluginKey.getState(editor.state)?.doc;
    if (doc === undefined) {
      throw new Error("the editor is not bound to a document");
    }
    return doc;
  }

  it("sees one text insert for a choice: the emoji replaces the query, and no node is added", async () => {
    const editor = await openEmpty();
    const doc = docOf(editor);
    typeText(editor, ":smi");
    await findMenu();
    await waitFor(() => {
      expect(JSON.stringify(loroToPortable(doc))).toContain(":smi");
    });

    const before = doc.oplogVersion();
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1); // one local change
    const ops = changes[0] ?? [];
    expect(ops.filter((op) => op.type === "insert")).toEqual([expect.objectContaining({ text: "😄" })]); // one insert, the character
    expect(ops.filter((op) => op.type === "delete")).toHaveLength(1); // the typed `:smi` goes in one delete
    expect(ops.every((op) => op.type === "insert" || op.type === "delete")).toBe(true);

    const portable = loroToPortable(doc) as { content: JsonValue[] };
    expect(portable).toEqual({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "😄" }] }],
    });
  });
});
