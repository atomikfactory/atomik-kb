// The "Type '/' for commands" hint: one at most, on the empty block that holds the cursor, only
// while the editor is focused (a completely empty page keeps one on its first line).

import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { paragraphs } from "../test/editor";
import { openPage, renderPages } from "../test/page";

function hints(editor: Editor): string[] {
  return Array.from(editor.view.dom.querySelectorAll(".is-empty[data-placeholder]")).map((el) => el.getAttribute("data-placeholder") ?? "");
}

function startOf(editor: Editor, index: number): number {
  let pos = 1;
  for (let i = 0; i < index; i += 1) {
    pos += editor.state.doc.child(i).nodeSize;
  }
  return pos;
}

function place(editor: Editor, index: number): void {
  act(() => {
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, startOf(editor, index))));
  });
}

describe("the empty-block placeholder", () => {
  it("shows once, on the focused empty block that holds the cursor, and not when the editor is blurred", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "", "", "More", "") }] }]);
    const editor = await openPage(user, "Notes");

    act(() => {
      fireEvent.focus(editor.view.dom);
    });
    place(editor, 1);
    expect(hints(editor)).toEqual([t("editor.placeholder")]);
    expect(editor.view.dom.querySelectorAll(".is-empty")).toHaveLength(1);

    place(editor, 2);
    expect(hints(editor)).toHaveLength(1);
    expect(editor.view.dom.children[2]?.classList.contains("is-empty")).toBe(true);

    place(editor, 0);
    expect(hints(editor)).toHaveLength(0);

    place(editor, 4);
    expect(hints(editor)).toHaveLength(1);

    act(() => {
      fireEvent.blur(editor.view.dom);
    });
    expect(hints(editor)).toHaveLength(0);
  });

  it("keeps one hint on the first line of a page with nothing on it, focused or not", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Blank", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Blank");
    expect(hints(editor)).toEqual([t("editor.placeholder")]);
    act(() => {
      fireEvent.blur(editor.view.dom);
    });
    expect(hints(editor)).toEqual([t("editor.placeholder")]);
  });
});
