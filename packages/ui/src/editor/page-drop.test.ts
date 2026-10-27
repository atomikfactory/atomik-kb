// Dropping a page row of the sidebar tree into the text (ADR-0016, 2026-10-24): a page mention at the drop position.

import { Editor } from "@tiptap/core";
import { Slice } from "@tiptap/pm/model";
import StarterKit from "@tiptap/starter-kit";
import { afterEach, describe, expect, it } from "vitest";

import { PageRef } from "./page-ref";
import { attachPageAreaDrop, insertDroppedPage, PAGE_DRAG_SPACE_TYPE, PAGE_DRAG_TYPE, PageDrop } from "./page-drop";

let editor: Editor | null = null;

afterEach(() => {
  editor?.destroy();
  editor = null;
});

function make(content = "<p>Hello</p>"): Editor {
  editor = new Editor({
    extensions: [StarterKit, PageRef, PageDrop.configure({ spaceId: "s1", pageId: "open" })],
    content,
  });
  return editor;
}

/** Runs the editor's drop handlers for a drop at document position `pos` carrying `data`; the result is "handled". */
function drop(ed: Editor, data: Record<string, string>, pos: number | null = 3): boolean {
  ed.view.posAtCoords = () => (pos === null ? null : { pos, inside: -1 });
  const event = new Event("drop", { cancelable: true }) as DragEvent;
  Object.defineProperty(event, "dataTransfer", {
    value: { types: Object.keys(data), getData: (type: string) => data[type] ?? "" },
  });
  return ed.view.someProp("handleDrop", (handler) => handler(ed.view, event, Slice.empty, false)) === true;
}

function pageIds(ed: Editor): string[] {
  const ids: string[] = [];
  ed.state.doc.descendants((node) => {
    if (node.type.name === "pageRef") {
      ids.push(String(node.attrs.pageId));
    }
    return true;
  });
  return ids;
}

describe("PageDrop", () => {
  it("inserts a page mention at the drop position", () => {
    const ed = make();
    expect(drop(ed, { [PAGE_DRAG_TYPE]: "p2", [PAGE_DRAG_SPACE_TYPE]: "s1" })).toBe(true);
    expect(pageIds(ed)).toEqual(["p2"]);
    expect(ed.state.doc.textContent).toBe("He llo");
    const first = ed.state.doc.firstChild?.child(1);
    expect(first?.type.name).toBe("pageRef");
  });

  it("does not handle a drop without the page type", () => {
    const ed = make();
    expect(drop(ed, { "text/plain": "x" })).toBe(false);
    expect(drop(ed, { Files: "" })).toBe(false);
    expect(pageIds(ed)).toEqual([]);
  });

  it("ignores the open page and a page of another space", () => {
    const ed = make();
    expect(drop(ed, { [PAGE_DRAG_TYPE]: "open", [PAGE_DRAG_SPACE_TYPE]: "s1" })).toBe(true);
    expect(drop(ed, { [PAGE_DRAG_TYPE]: "p2", [PAGE_DRAG_SPACE_TYPE]: "other" })).toBe(true);
    expect(drop(ed, { [PAGE_DRAG_TYPE]: "p2" }, null)).toBe(true);
    expect(pageIds(ed)).toEqual([]);
  });
});

describe("insertDroppedPage", () => {
  it("inserts inside inline content at a position", () => {
    const ed = make();
    expect(insertDroppedPage(ed.view, "p2", 3)).toBe(true);
    expect(ed.state.doc.firstChild?.child(1).type.name).toBe("pageRef");
    expect(insertDroppedPage(ed.view, "p3", 0)).toBe(false);
  });

  it("appends in an empty last paragraph, else in a new one", () => {
    const empty = make("<p>Hello</p><p></p>");
    expect(insertDroppedPage(empty.view, "p2", "end")).toBe(true);
    expect(empty.state.doc.childCount).toBe(2);
    expect(empty.state.doc.lastChild?.firstChild?.type.name).toBe("pageRef");
    const full = make();
    expect(insertDroppedPage(full.view, "p2", "end")).toBe(true);
    expect(full.state.doc.childCount).toBe(2);
    expect(full.state.doc.firstChild?.textContent).toBe("Hello");
    expect(full.state.doc.lastChild?.firstChild?.type.name).toBe("pageRef");
  });

  it("puts it at the start of the document", () => {
    const ed = make();
    expect(insertDroppedPage(ed.view, "p2", "start")).toBe(true);
    expect(ed.state.doc.childCount).toBe(2);
    expect(ed.state.doc.firstChild?.firstChild?.type.name).toBe("pageRef");
    const empty = make("<p></p><p>Hello</p>");
    expect(insertDroppedPage(empty.view, "p2", "start")).toBe(true);
    expect(empty.state.doc.childCount).toBe(2);
    expect(empty.state.doc.firstChild?.firstChild?.type.name).toBe("pageRef");
  });
});

describe("attachPageAreaDrop", () => {
  function areaDrop(ed: Editor, area: HTMLElement, data: Record<string, string>, y: number): DragEvent {
    ed.view.dom.getBoundingClientRect = () => ({ top: 100, bottom: 200, left: 100, right: 300 }) as DOMRect;
    ed.view.posAtCoords = () => ({ pos: 3, inside: -1 });
    const event = new Event("drop", { cancelable: true, bubbles: true }) as DragEvent;
    Object.defineProperty(event, "clientX", { value: 50 });
    Object.defineProperty(event, "clientY", { value: y });
    Object.defineProperty(event, "dataTransfer", {
      value: { types: Object.keys(data), getData: (type: string) => data[type] ?? "" },
    });
    area.dispatchEvent(event);
    return event;
  }

  it("accepts the drag over the area and inserts beside, below and above the text", () => {
    const ed = make();
    const area = document.createElement("div");
    const off = attachPageAreaDrop(area, ed.view, { spaceId: "s1", pageId: "open" });
    const over = new Event("dragover", { cancelable: true, bubbles: true }) as DragEvent;
    const dt = { types: [PAGE_DRAG_TYPE], dropEffect: "none" };
    Object.defineProperty(over, "dataTransfer", { value: dt });
    area.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    expect(dt.dropEffect).toBe("copy");

    const data = { [PAGE_DRAG_TYPE]: "p2", [PAGE_DRAG_SPACE_TYPE]: "s1" };
    expect(areaDrop(ed, area, data, 150).defaultPrevented).toBe(true);
    expect(ed.state.doc.childCount).toBe(1);
    areaDrop(ed, area, data, 300);
    expect(ed.state.doc.childCount).toBe(2);
    areaDrop(ed, area, data, 10);
    expect(ed.state.doc.childCount).toBe(3);
    expect(pageIds(ed)).toEqual(["p2", "p2", "p2"]);

    const before: unknown = ed.state.doc.toJSON();
    expect(areaDrop(ed, area, { "text/plain": "x" }, 300).defaultPrevented).toBe(false);
    areaDrop(ed, area, { [PAGE_DRAG_TYPE]: "open" }, 300);
    expect(ed.state.doc.toJSON()).toEqual(before);
    off();
  });
});
