// Find in page and replace: what the search finds (across marks, in every kind of block, not across
// an atom or a block boundary), the options, the active match and how it moves, how a change of the
// document finds the matches again, the replacing (marks kept, code stays plain, one update and one
// undo step for "Replace all", nothing in a read-only editor), and the find bar in the page editor.

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { loroDocFrom, paragraphs, pid, sync, tick } from "../test/editor";
import { appendReference, openPage, renderPages } from "../test/page";
import { row } from "../test/sidebar";
import { LoroBinding } from "./extensions";
import { selectedSearchText } from "./FindBar";
import { FIND_ACTIVE_CLASS, FIND_MATCH_CLASS, findMatches, findStateOf, FindReplace } from "./find-replace";
import { undo } from "./loro-binding";
import { createEditorSchema, createSchemaExtensions } from "./schema";

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/** A Tiptap editor over the schema, the Loro binding and find in page, on `content`. */
async function mount(content: JsonValue, { editable = true }: { editable?: boolean } = {}): Promise<{ editor: Editor; doc: LoroDoc }> {
  const doc = loroDocFrom(content);
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({
    element,
    extensions: [...createSchemaExtensions(), LoroBinding.configure({ doc, readOnly: !editable }), FindReplace],
    content: content as JSONContent,
    editable,
  });
  mounted.push({ editor, element });
  await tick(); // the binding shows the document in a timeout
  return { editor, doc };
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
  vi.restoreAllMocks();
});

// ----- documents ---------------------------------------------------------------------------

let nextId = 1000;

function attrs(extra: Record<string, JsonValue> = {}): Record<string, JsonValue> {
  return { id: pid(nextId++), ...extra };
}

function text(value: string, ...marks: string[]): JsonValue {
  return marks.length === 0 ? { type: "text", text: value } : { type: "text", text: value, marks: marks.map((type) => ({ type })) };
}

function node(type: string, content: JsonValue[], extra: Record<string, JsonValue> = {}): JsonValue {
  return { type, attrs: attrs(extra), ...(content.length === 0 ? {} : { content }) };
}

function p(...content: JsonValue[]): JsonValue {
  return node("paragraph", content);
}

function page(...blocks: JsonValue[]): JsonValue {
  return { type: "doc", content: blocks };
}

// ----- looking at an editor --------------------------------------------------------------------

function search(editor: Editor, query: string, options: { matchCase?: boolean; wholeWord?: boolean } = {}): void {
  editor.commands.setFind({ query, ...options });
}

/** The text of every match, in order. */
function found(editor: Editor): string[] {
  const state = findStateOf(editor.state);
  return (state?.matches ?? []).map((match) => editor.state.doc.textBetween(match.from, match.to, "\n", "\n"));
}

function activeIndex(editor: Editor): number {
  return findStateOf(editor.state)?.active ?? -1;
}

/** The text of what the page draws as a match (one entry for each stretch of one mark). */
function drawn(editor: Editor): string[] {
  return [...editor.view.dom.querySelectorAll(`.${FIND_MATCH_CLASS}`)].map((element) => element.textContent ?? "");
}

function drawnActive(editor: Editor): string[] {
  return [...editor.view.dom.querySelectorAll(`.${FIND_ACTIVE_CLASS}`)].map((element) => element.textContent ?? "");
}

/** The first paragraph as `[text, marks]` runs. */
function runs(editor: Editor, index = 0): [string, string[]][] {
  const result: [string, string[]][] = [];
  editor.state.doc.child(index).forEach((child) => {
    result.push([child.text ?? `<${child.type.name}>`, child.marks.map((mark) => mark.type.name)]);
  });
  return result;
}

function select(editor: Editor, from: number, to: number = from): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, from, to)));
  });
}

// ----- what is found ------------------------------------------------------------------------

describe("what the search finds", () => {
  it("finds every occurrence in the text, any case by default, and matching the case on request", async () => {
    const { editor } = await mount(page(p(text("The cat sat. "), text("Cat", "bold"), text("s purr, the CAT"))));
    search(editor, "cat");
    expect(found(editor)).toEqual(["cat", "Cat", "CAT"]);
    expect(drawn(editor)).toEqual(["cat", "Cat", "CAT"]);

    search(editor, "cat", { matchCase: true });
    expect(found(editor)).toEqual(["cat"]);
    expect(drawn(editor)).toEqual(["cat"]);

    search(editor, "CAT", { matchCase: false });
    expect(found(editor)).toEqual(["cat", "Cat", "CAT"]);
  });

  it("runs a match across the marks of a sentence", async () => {
    const { editor } = await mount(page(p(text("a bro"), text("wn", "bold", "italic"), text("ie, brown"))));
    search(editor, "brownie");
    expect(found(editor)).toEqual(["brownie"]);
    // One stretch for each text node it touches.
    expect(drawn(editor)).toEqual(["bro", "wn", "ie"]);
    expect(drawnActive(editor)).toEqual(["bro", "wn", "ie"]);
  });

  it("finds text in code blocks, math blocks, table cells, columns, toggles, callouts, lists and quotes", async () => {
    const { editor } = await mount(
      page(
        node("codeBlock", [text("let needle = 1;\nneedle += 1;")], { language: null }),
        node("mathBlock", [text("needle^2")]),
        node("table", [
          node("tableRow", [
            node("tableHeader", [p(text("needle"))], { colspan: 1, rowspan: 1 }),
            node("tableCell", [p(text("one "), text("needle", "bold"))], { colspan: 1, rowspan: 1 }),
          ]),
        ]),
        node("columnList", [node("column", [p(text("a needle"))]), node("column", [p(text("another"), text(" needle", "italic"))])]),
        node("details", [node("detailsSummary", [text("needle")]), node("detailsContent", [p(text("needle inside"))])], { open: true }),
        node("callout", [p(text("a needle in a callout"))], { icon: "💡", color: null }),
        node("bulletList", [node("listItem", [p(text("needle item"))])]),
        node("blockquote", [p(text("a needle quoted"))]),
      ),
    );
    search(editor, "needle");
    const tops = (findStateOf(editor.state)?.matches ?? []).map((match) => editor.state.doc.resolve(match.from).node(1).type.name);
    expect(tops).toEqual([
      "codeBlock",
      "codeBlock",
      "mathBlock",
      "table",
      "table",
      "columnList",
      "columnList",
      "details",
      "details",
      "callout",
      "bulletList",
      "blockquote",
    ]);
    expect(drawn(editor)).toHaveLength(12);
    expect(editor.view.dom.querySelectorAll(`pre .${FIND_MATCH_CLASS}`)).toHaveLength(2);
  });

  it("does not run a match across an atom, a line break or a block boundary", async () => {
    const { editor } = await mount(
      page(
        p(text("need"), { type: "dateMention", attrs: attrs({ date: "2026-10-07" }) }, text("le")),
        p(text("need"), { type: "pageRef", attrs: attrs({ pageId: "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6" }) }, text("le")),
        p(text("need"), { type: "hardBreak" }, text("le")),
        p(text("need")),
        p(text("le")),
      ),
    );
    search(editor, "needle");
    expect(found(editor)).toEqual([]);
    expect(drawn(editor)).toEqual([]);
    search(editor, "need");
    expect(found(editor)).toHaveLength(4); // each half is found on its own
    search(editor, "le");
    expect(found(editor)).toHaveLength(4);
  });

  it("finds whole words only on request: no letter, digit or underscore next to the match", async () => {
    const sentence = "cat concat cat's cats Cat_x cat, 9cat café-cat caté";
    const { editor } = await mount(page(p(text(sentence))));
    search(editor, "cat", { wholeWord: true });
    // The first, the one before an apostrophe, the one before a comma and the one after a hyphen.
    const starts = (findStateOf(editor.state)?.matches ?? []).map((match) => match.from - 1);
    expect(starts).toEqual([0, sentence.indexOf("cat's"), sentence.indexOf("cat,"), sentence.indexOf("-cat") + 1]);
    search(editor, "cat", { wholeWord: false });
    expect(found(editor)).toHaveLength(9); // all but `café`, which has no `cat` in it
    search(editor, "caf", { wholeWord: true });
    expect(found(editor)).toEqual([]); // `café` goes on with an accented letter
  });

  it("checks a word boundary only on the side of the query that is a word character", () => {
    const schema = createEditorSchema();
    const doc = schema.nodeFromJSON(page(p(text("c++ and c++x and xc++ c+"))));
    const words = (query: string): number => findMatches(doc, query, { matchCase: false, wholeWord: true }).length;
    expect(words("c++")).toBe(2); // `c++x` counts, the query ends in a sign; `xc++` does not, it starts inside a word
    expect(words("+")).toBe(7); // a sign has no word side: every one is a match
    expect(words("c")).toBe(3); // `c` next to a sign stands alone, in `xc++` it does not
  });

  it("takes the query as plain text, never as a pattern", async () => {
    const { editor } = await mount(page(p(text("abc a.c a*c (x) [y] \\d $1 ^ ?"))));
    for (const [query, count] of [
      ["a.c", 1],
      ["a*c", 1],
      ["(x)", 1],
      ["[y]", 1],
      ["\\d", 1],
      ["$1", 1],
      ["^", 1],
      ["?", 1],
      ["(", 1],
      ["[", 1],
    ] as const) {
      search(editor, query);
      expect(found(editor), query).toHaveLength(count);
    }
  });

  it("finds nothing for an empty query, and draws nothing", async () => {
    const { editor } = await mount(page(p(text("cat cat"))));
    search(editor, "cat");
    expect(drawn(editor)).toHaveLength(2);
    search(editor, "");
    expect(found(editor)).toEqual([]);
    expect(activeIndex(editor)).toBe(-1);
    expect(drawn(editor)).toEqual([]);
    expect(editor.commands.findNext()).toBe(false);
    expect(editor.commands.findPrevious()).toBe(false);
  });

  it("keeps the options when the query is cleared, and the query when an option changes", async () => {
    const { editor } = await mount(page(p(text("Cat cat"))));
    search(editor, "cat", { matchCase: true });
    editor.commands.setFind({ wholeWord: true });
    expect(findStateOf(editor.state)).toMatchObject({ query: "cat", matchCase: true, wholeWord: true });
    search(editor, "");
    expect(findStateOf(editor.state)).toMatchObject({ query: "", matchCase: true, wholeWord: true });
  });
});

// ----- the active match -----------------------------------------------------------------------

describe("the active match", () => {
  it("starts at the first match from the selection on, and is the only one drawn as active", async () => {
    const { editor } = await mount(page(p(text("one cat, two cat, three cat"))));
    search(editor, "cat");
    expect(activeIndex(editor)).toBe(0);
    expect(drawn(editor)).toHaveLength(3);
    expect(drawnActive(editor)).toEqual(["cat"]);
    const [first, second] = findStateOf(editor.state)?.matches ?? [];
    expect(first?.from).toBeLessThan(second?.from ?? 0);

    search(editor, "");
    select(editor, (second?.from ?? 0) + 1);
    search(editor, "cat");
    expect(activeIndex(editor)).toBe(2); // the match the caret is in is passed, the next one counts
  });

  it("goes to the next and the previous match, and wraps around at both ends", async () => {
    const { editor } = await mount(page(p(text("a cat")), p(text("b cat")), p(text("c cat"))));
    search(editor, "cat");
    const classes = (): number[] =>
      [...editor.view.dom.querySelectorAll(`.${FIND_MATCH_CLASS}`)].flatMap((element, index) => (element.classList.contains(FIND_ACTIVE_CLASS) ? [index] : []));
    expect([activeIndex(editor), classes()]).toEqual([0, [0]]);

    expect(editor.commands.findNext()).toBe(true);
    expect([activeIndex(editor), classes()]).toEqual([1, [1]]);
    editor.commands.findNext();
    expect([activeIndex(editor), classes()]).toEqual([2, [2]]);
    editor.commands.findNext();
    expect([activeIndex(editor), classes()]).toEqual([0, [0]]); // wrapped

    expect(editor.commands.findPrevious()).toBe(true);
    expect([activeIndex(editor), classes()]).toEqual([2, [2]]); // wrapped backwards
    editor.commands.findPrevious();
    expect([activeIndex(editor), classes()]).toEqual([1, [1]]);
  });

  it("stays on the one match there is", async () => {
    const { editor } = await mount(page(p(text("one cat only"))));
    search(editor, "cat");
    editor.commands.findNext();
    editor.commands.findPrevious();
    expect([activeIndex(editor), drawnActive(editor)]).toEqual([0, ["cat"]]);
  });

  it("brings the active match into view when the search or the active match changes, and not when the document does", async () => {
    const { editor } = await mount(page(p(text("one cat, two cat"))));
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    search(editor, "cat");
    expect(scroll).toHaveBeenCalledTimes(1);
    expect(scroll.mock.contexts[0]).toBeInstanceOf(HTMLElement);
    expect((scroll.mock.contexts[0] as HTMLElement).classList.contains(FIND_ACTIVE_CLASS)).toBe(true);

    editor.commands.findNext();
    expect(scroll).toHaveBeenCalledTimes(2);
    expect((scroll.mock.contexts[1] as HTMLElement).classList.contains(FIND_ACTIVE_CLASS)).toBe(true);
    expect(scroll.mock.contexts[1]).not.toBe(scroll.mock.contexts[0]);

    editor.commands.insertContentAt(1, "x ");
    expect(scroll).toHaveBeenCalledTimes(2); // typing is not followed
  });

  it("the match a replacement or a change passed is not shown again from the start", async () => {
    const { editor } = await mount(page(p(text("cat one cat two cat"))));
    search(editor, "cat");
    editor.commands.findNext();
    expect(activeIndex(editor)).toBe(1);
    editor.commands.insertContentAt(1, "a cat and ");
    expect(found(editor)).toHaveLength(4);
    expect(activeIndex(editor)).toBe(2); // still the same cat, now the third
    expect(drawnActive(editor)).toEqual(["cat"]);
    const active = findStateOf(editor.state)?.matches[2];
    expect(editor.state.doc.textBetween((active?.from ?? 0) - 4, active?.from ?? 0)).toBe("one ");
  });
});

// ----- the document changes ---------------------------------------------------------------------

describe("when the document changes", () => {
  it("finds the matches again after typing", async () => {
    const { editor } = await mount(page(p(text("cat and dog"))));
    search(editor, "cat");
    expect(drawn(editor)).toHaveLength(1);

    editor.commands.insertContentAt(editor.state.doc.content.size - 1, " cat");
    expect(found(editor)).toEqual(["cat", "cat"]);
    expect(drawn(editor)).toEqual(["cat", "cat"]);

    // Typing inside a match takes it away.
    const [first] = findStateOf(editor.state)?.matches ?? [];
    editor.commands.insertContentAt((first?.from ?? 0) + 1, "x");
    expect(found(editor)).toEqual(["cat"]);
    expect(drawn(editor)).toEqual(["cat"]);
    expect(activeIndex(editor)).toBe(0);
  });

  it("finds them again after a remote change, which the binding shows as a replaced page", async () => {
    const { editor, doc } = await mount(page(p(text("cat one cat"))));
    search(editor, "cat");
    editor.commands.findNext();
    expect(activeIndex(editor)).toBe(1);

    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    appendReference(remote, "another cat", "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6", pid(900));
    sync(remote, doc);
    await waitFor(() => {
      expect(found(editor)).toHaveLength(3);
    });
    expect(drawn(editor)).toHaveLength(3);
    expect(activeIndex(editor)).toBe(1); // where it was, not back at the start
    expect(drawnActive(editor)).toHaveLength(1);
  });

  it("clears the matches when the search is closed, and keeps the selection unless it is asked to select the match", async () => {
    const { editor } = await mount(page(p(text("cat one cat"))));
    select(editor, 1);
    search(editor, "cat");
    editor.commands.findNext();
    editor.commands.closeFind();
    expect(findStateOf(editor.state)).toMatchObject({ query: "", matches: [], active: -1 });
    expect(drawn(editor)).toEqual([]);
    expect(editor.state.selection.empty).toBe(true);

    search(editor, "cat");
    editor.commands.findNext();
    const { from, to } = findStateOf(editor.state)?.matches[1] ?? { from: -1, to: -1 };
    editor.commands.closeFind({ select: true });
    expect([editor.state.selection.from, editor.state.selection.to]).toEqual([from, to]);
    expect(drawn(editor)).toEqual([]);
  });
});

// ----- replacing ------------------------------------------------------------------------------------

describe("replace", () => {
  it("replaces the active match, keeps the marks of its start and goes on to the next", async () => {
    const { editor } = await mount(page(p(text("a "), text("fo", "bold"), text("o b foo"))));
    search(editor, "foo");
    expect(found(editor)).toEqual(["foo", "foo"]);
    expect(editor.commands.replaceActive("bar")).toBe(true);
    expect(runs(editor)).toEqual([
      ["a ", []],
      ["bar", ["bold"]], // the marks of the first letter
      [" b foo", []],
    ]);
    expect(found(editor)).toEqual(["foo"]);
    expect(activeIndex(editor)).toBe(0);
    expect(drawnActive(editor)).toEqual(["foo"]); // the next one, in the text that was not replaced

    editor.commands.replaceActive("baz");
    expect(editor.state.doc.textContent).toBe("a bar b baz");
    expect(findStateOf(editor.state)).toMatchObject({ matches: [], active: -1 });
    expect(editor.commands.replaceActive("x")).toBe(false); // nothing left
  });

  it("does not look for the next match inside the text it just put in", async () => {
    const { editor } = await mount(page(p(text("foo foo"))));
    search(editor, "foo");
    editor.commands.replaceActive("foofoo");
    expect(editor.state.doc.textContent).toBe("foofoo foo");
    expect(found(editor)).toHaveLength(3);
    expect(activeIndex(editor)).toBe(2); // the one that was not replaced, not the ones in the new text
    editor.commands.replaceActive("foo"); // replacing a match with itself still moves on
    expect(activeIndex(editor)).toBe(0); // wrapped
  });

  it("replaces with nothing", async () => {
    const { editor } = await mount(page(p(text("a foo b foo c"))));
    search(editor, "foo ");
    editor.commands.replaceAll("");
    expect(editor.state.doc.textContent).toBe("a b c");
  });

  it("keeps the text of a code block plain, and in the block", async () => {
    const { editor } = await mount(
      page(node("codeBlock", [text("let foo = foo;")], { language: null }), p(text("foo", "bold"))),
    );
    search(editor, "foo");
    editor.commands.findNext();
    editor.commands.findPrevious(); // the first one, in the code block
    editor.commands.replaceActive("bar");
    expect(editor.state.doc.child(0).textContent).toBe("let bar = foo;");
    expect(editor.state.doc.child(0).type.name).toBe("codeBlock");
    expect(runs(editor, 0).every(([, marks]) => marks.length === 0)).toBe(true);
    editor.commands.replaceAll("qux");
    expect(editor.state.doc.child(0).textContent).toBe("let bar = qux;");
    expect(runs(editor, 1)).toEqual([["qux", ["bold"]]]);
  });

  it("replaces all in one transaction: one update, one undo step", async () => {
    const original = page(
      p(text("foo and "), text("foo", "bold"), text(" and foo")),
      node("table", [node("tableRow", [node("tableCell", [p(text("a foo"))], { colspan: 1, rowspan: 1 })])]),
      node("codeBlock", [text("foo()")], { language: null }),
    );
    const { editor, doc } = await mount(original);
    const before = loroToPortable(doc);
    const updates: Uint8Array[] = [];
    const unsubscribe = doc.subscribeLocalUpdates((bytes) => {
      updates.push(bytes);
    });
    const changes: number[] = [];
    editor.on("transaction", ({ transaction }) => {
      if (transaction.docChanged) {
        changes.push(transaction.steps.length);
      }
    });
    try {
      search(editor, "foo");
      expect(found(editor)).toHaveLength(5);
      expect(editor.commands.replaceAll("bar")).toBe(true);

      expect(editor.state.doc.textContent).toBe("bar and bar and bara barbar()");
      expect(found(editor)).toEqual([]);
      expect(drawn(editor)).toEqual([]);
      expect(changes.filter((steps) => steps > 1)).toHaveLength(1); // the replacements are one transaction
      expect(updates).toHaveLength(1); // and the binding wrote one update
      expect(runs(editor).map(([value]) => value)).toEqual(["bar and ", "bar", " and bar"]);
      expect(loroToPortable(doc)).not.toEqual(before);

      expect(undo(editor.state, editor.view.dispatch)).toBe(true);
      expect(loroToPortable(doc)).toEqual(before); // one undo brings every one back
      expect(editor.state.doc.textContent).toBe("foo and foo and foo" + "a foo" + "foo()");
    } finally {
      unsubscribe();
    }
  });

  it("changes nothing in a read-only editor", async () => {
    const { editor, doc } = await mount(page(p(text("foo foo"))), { editable: false });
    const before = loroToPortable(doc);
    search(editor, "foo");
    expect(found(editor)).toEqual(["foo", "foo"]); // finding works
    expect(drawn(editor)).toEqual(["foo", "foo"]);
    expect(editor.can().replaceAll("bar")).toBe(false);
    expect(editor.can().replaceActive("bar")).toBe(false);
    expect(editor.commands.replaceAll("bar")).toBe(false);
    expect(editor.commands.replaceActive("bar")).toBe(false);
    expect(editor.state.doc.textContent).toBe("foo foo");
    expect(loroToPortable(doc)).toEqual(before);
  });
});

// ----- what the selection gives the bar ----------------------------------------------------------------

describe("the selected text a search starts with", () => {
  it("is a short selection on one line, and nothing else", async () => {
    const long = "x".repeat(150);
    const { editor } = await mount(
      page(p(text("hello brave world")), p(text("second line")), node("codeBlock", [text("line one\nline two")], { language: null }), p(text(long)), p(text("  "))),
    );
    select(editor, 7, 12);
    expect(selectedSearchText(editor)).toBe("brave");

    select(editor, 3); // a caret
    expect(selectedSearchText(editor)).toBeNull();

    select(editor, 3, 25); // two blocks
    expect(selectedSearchText(editor)).toBeNull();

    let codeStart = 0;
    editor.state.doc.descendants((child, pos) => {
      if (child.type.name === "codeBlock") {
        codeStart = pos + 1;
      }
      return true;
    });
    select(editor, codeStart, codeStart + 17); // two lines of one block
    expect(selectedSearchText(editor)).toBeNull();
    select(editor, codeStart, codeStart + 8);
    expect(selectedSearchText(editor)).toBe("line one");

    let longStart = 0;
    editor.state.doc.forEach((child, offset, index) => {
      if (index === 3) {
        longStart = offset + 1;
      }
    });
    select(editor, longStart, longStart + 150); // too long
    expect(selectedSearchText(editor)).toBeNull();
    select(editor, longStart, longStart + 20);
    expect(selectedSearchText(editor)).toBe("x".repeat(20));
  });
});

// ----- the find bar in the page editor ---------------------------------------------------------------------

describe("the find bar", () => {
  async function open(...texts: string[]) {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs(...texts) }, { title: "Other", content: paragraphs("cat elsewhere") }] }]);
    const editor = await openPage(app.user, "Notes");
    return { ...app, editor };
  }

  const barOf = (): HTMLElement => screen.getByRole("search", { name: t("find.label") });
  const fieldOf = (): HTMLInputElement => within(barOf()).getByRole("textbox", { name: t("find.field") });
  const counterOf = (): string => within(barOf()).getByRole("status").textContent;
  const button = (name: string): HTMLElement => within(barOf()).getByRole("button", { name });

  function pressFind(editor: Editor, init: KeyboardEventInit = { ctrlKey: true }): boolean {
    return fireEvent.keyDown(editor.view.dom, { key: "f", ...init });
  }

  it("is not there until it is asked for", async () => {
    await open("one cat");
    expect(screen.queryByRole("search", { name: t("find.label") })).not.toBeInTheDocument();
    expect(document.querySelector(`.${FIND_MATCH_CLASS}`)).toBeNull();
  });

  it("opens on Ctrl+F with the field focused and the browser's own find kept away", async () => {
    const { editor } = await open("one cat");
    const free = pressFind(editor);
    expect(free).toBe(false); // the default was prevented: no find bar of the webview
    expect(barOf()).toBeInTheDocument();
    await waitFor(() => {
      expect(fieldOf()).toHaveFocus();
    });
    expect(fieldOf()).toHaveValue("");
    expect(counterOf()).toBe(""); // nothing typed, nothing counted
  });

  it("opens on Cmd+F too, from outside the editor, and Ctrl+F again moves focus back to the field", async () => {
    const { editor, user } = await open("one cat");
    expect(fireEvent.keyDown(document.body, { key: "f", metaKey: true })).toBe(false);
    await waitFor(() => {
      expect(fieldOf()).toHaveFocus();
    });
    await user.click(screen.getByRole("textbox", { name: t("editor.label") }));
    expect(fieldOf()).not.toHaveFocus();
    pressFind(editor, { metaKey: true });
    await waitFor(() => {
      expect(fieldOf()).toHaveFocus();
    });
    expect(screen.getAllByRole("search")).toHaveLength(1);
  });

  it("starts with the selected text, counts, and marks the match under the selection as active", async () => {
    const { editor } = await open("The word here", "another word", "one more word");
    select(editor, 5, 9); // "word" in the first block
    pressFind(editor);
    await waitFor(() => {
      expect(fieldOf()).toHaveValue("word");
    });
    expect(counterOf()).toBe(t("find.count", { current: 1, total: 3 }));
    expect(drawn(editor)).toEqual(["word", "word", "word"]);
    expect(drawnActive(editor)).toEqual(["word"]);
    expect(activeIndex(editor)).toBe(0);
  });

  it("is not started by a selection in the editor while the keys come from somewhere else", async () => {
    const { editor } = await open("The word here");
    select(editor, 5, 9);
    fireEvent.keyDown(document.body, { key: "f", ctrlKey: true });
    await waitFor(() => {
      expect(fieldOf()).toHaveFocus();
    });
    expect(fieldOf()).toHaveValue("");
  });

  it("finds as you type, Enter goes to the next match and Shift+Enter to the previous", async () => {
    const { editor, user } = await open("one cat", "two cat", "three cat");
    pressFind(editor);
    await waitFor(() => {
      expect(fieldOf()).toHaveFocus();
    });
    await user.keyboard("cat");
    expect(fieldOf()).toHaveValue("cat");
    expect(counterOf()).toBe("1 of 3");
    expect(drawn(editor)).toHaveLength(3);

    await user.keyboard("{Enter}");
    expect(counterOf()).toBe("2 of 3");
    await user.keyboard("{Enter}{Enter}");
    expect(counterOf()).toBe("1 of 3"); // wrapped
    await user.keyboard("{Shift>}{Enter}{/Shift}");
    expect(counterOf()).toBe("3 of 3");
    expect(fieldOf()).toHaveFocus(); // the keys stay in the field

    await user.click(button(t("find.previous")));
    expect(counterOf()).toBe("2 of 3");
    await user.click(button(t("find.next")));
    expect(counterOf()).toBe("3 of 3");
    expect(drawnActive(editor)).toEqual(["cat"]);

    await user.type(fieldOf(), "x");
    expect(counterOf()).toBe(t("find.noMatches"));
    expect(button(t("find.next"))).toBeDisabled();
    expect(button(t("find.previous"))).toBeDisabled();
    expect(drawn(editor)).toEqual([]);
  });

  it("has toggles for match case and whole word", async () => {
    const { editor, user } = await open("Cat cat concat");
    pressFind(editor);
    await user.keyboard("cat");
    expect(counterOf()).toBe("1 of 3");

    const matchCase = button(t("find.matchCase"));
    const wholeWord = button(t("find.wholeWord"));
    expect(matchCase).toHaveAttribute("aria-pressed", "false");
    expect(wholeWord).toHaveAttribute("aria-pressed", "false");

    await user.click(matchCase);
    expect(matchCase).toHaveAttribute("aria-pressed", "true");
    expect(counterOf()).toBe("1 of 2");
    await user.click(wholeWord);
    expect(wholeWord).toHaveAttribute("aria-pressed", "true");
    expect(counterOf()).toBe("1 of 1");
    await user.click(matchCase);
    expect(matchCase).toHaveAttribute("aria-pressed", "false");
    expect(counterOf()).toBe("2 of 2"); // `Cat` joins, and the active match stays where it was: the second now
  });

  it("replaces the active match or all of them, from the Replace disclosure", async () => {
    const { editor, user } = await open("one cat", "two cat", "three cat");
    pressFind(editor);
    await user.keyboard("cat");
    expect(screen.queryByRole("textbox", { name: t("find.replaceField") })).not.toBeInTheDocument();
    const toggle = button(t("find.toggleReplace"));
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    const replacement = within(barOf()).getByRole("textbox", { name: t("find.replaceField") });
    await user.type(replacement, "dog");
    await user.click(button(t("find.replace")));
    expect(editor.state.doc.textContent).toBe("one dogtwo cat" + "three cat");
    expect(counterOf()).toBe("1 of 2");
    expect(drawnActive(editor)).toEqual(["cat"]);

    await user.type(replacement, "{Enter}"); // Enter in the field replaces too
    expect(editor.state.doc.textContent).toBe("one dogtwo dogthree cat");
    expect(counterOf()).toBe("1 of 1");

    await user.click(button(t("find.replaceAll")));
    expect(editor.state.doc.textContent).toBe("one dogtwo dogthree dog");
    expect(counterOf()).toBe(t("find.noMatches"));
    expect(button(t("find.replace"))).toBeDisabled();
    expect(button(t("find.replaceAll"))).toBeDisabled();

    await user.click(toggle);
    expect(within(barOf()).queryByRole("textbox", { name: t("find.replaceField") })).not.toBeInTheDocument();
  });

  it("has no replacing in a read-only page, but still finds", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Kept as it is, cat cat" }] },
        { type: "kanban", attrs: { id: pid(1), columns: 3 } },
      ],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Board", content }] }]);
    const editor = await openPage(user, "Board");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    pressFind(editor);
    await user.keyboard("cat");
    expect(counterOf()).toBe("1 of 2");
    expect(within(barOf()).queryByRole("button", { name: t("find.toggleReplace") })).not.toBeInTheDocument();
    expect(within(barOf()).queryByRole("button", { name: t("find.replaceAll") })).not.toBeInTheDocument();
    expect(editor.commands.replaceAll("dog")).toBe(false);
    expect(editor.state.doc.firstChild?.textContent).toBe("Kept as it is, cat cat");
  });

  it("closes on Escape, selects the active match and gives the editor the keyboard", async () => {
    const { editor, user } = await open("one cat", "two cat", "three cat");
    pressFind(editor);
    await user.keyboard("cat{Enter}");
    expect(counterOf()).toBe("2 of 3");
    const { from, to } = findStateOf(editor.state)?.matches[1] ?? { from: -1, to: -1 };

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("search", { name: t("find.label") })).not.toBeInTheDocument();
    expect(drawn(editor)).toEqual([]);
    expect(findStateOf(editor.state)?.query).toBe("");
    expect([editor.state.selection.from, editor.state.selection.to]).toEqual([from, to]);
    expect(editor.state.doc.textBetween(from, to)).toBe("cat");
    expect(editor.view.hasFocus()).toBe(true);
  });

  it("closes with its button the same way, and keeps the selection when there was no match", async () => {
    const { editor, user } = await open("one cat");
    select(editor, 2);
    pressFind(editor);
    await user.keyboard("zebra");
    expect(counterOf()).toBe(t("find.noMatches"));
    await user.click(button(t("find.close")));
    expect(screen.queryByRole("search", { name: t("find.label") })).not.toBeInTheDocument();
    expect([editor.state.selection.from, editor.state.selection.to]).toEqual([2, 2]);
    expect(editor.view.hasFocus()).toBe(true);
  });

  it("closes when another page is opened, and the new page has no search", async () => {
    const { editor, user } = await open("one cat");
    pressFind(editor);
    await user.keyboard("cat");
    expect(counterOf()).toBe("1 of 1");

    await user.click(row("Other"));
    await waitFor(() => {
      expect(screen.queryByRole("search", { name: t("find.label") })).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(screen.getByRole("textbox", { name: t("editor.label") })).toHaveTextContent("cat elsewhere");
    });
    expect(document.querySelector(`.${FIND_MATCH_CLASS}`)).toBeNull();
  });

  it("opens from the page menu", async () => {
    const { user } = await open("one cat");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.find") }));
    expect(barOf()).toBeInTheDocument();
    await waitFor(() => {
      expect(fieldOf()).toHaveFocus();
    });
  });

  it("leaves Ctrl+F alone while a dialog is open", async () => {
    await open("one cat");
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true }); // the global search dialog
    await screen.findByRole("dialog", { name: t("search.label") });
    expect(fireEvent.keyDown(document.body, { key: "f", ctrlKey: true })).toBe(true); // not prevented
    expect(screen.queryByRole("search", { name: t("find.label") })).not.toBeInTheDocument();
  });

  it("is not opened by Ctrl+F when no page is open", async () => {
    await renderPages([{ name: "Personal", pages: [{ title: "Notes" }] }]);
    expect(fireEvent.keyDown(document.body, { key: "f", ctrlKey: true })).toBe(true);
    expect(screen.queryByRole("search", { name: t("find.label") })).not.toBeInTheDocument();
  });
});
