// A page dragged from the sidebar tree and dropped into the text of an open page inserts a `pageRef` chip at the drop
// position, the same node the `@` menu inserts (ADR-0016, 2026-10-24 note). Only drops that carry the tree's page type are
// handled (files, block handles and text drags are left to their own handlers). A page of another space is ignored (a
// `pageRef` names a page of its own space only) and so is the open page itself (a link to itself says nothing). A read-only
// editor is not editable, so ProseMirror never calls the handler. The drop is accepted anywhere on the page area too (the
// margins beside the text, the space below the last block, the header above the text): `attachPageAreaDrop`.

import { Extension } from "@tiptap/core";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";

/** The page row of the sidebar tree: carries the page id (the tree's own drop reads it too). */
export const PAGE_DRAG_TYPE = "application/x-atomik-page";
/** Also set by the tree on drag start: the space the dragged page belongs to. */
export const PAGE_DRAG_SPACE_TYPE = "application/x-atomik-page-space";

export interface PageDropOptions {
  /** The space of the open page; `null`: the space of a dragged page is not checked. */
  spaceId: string | null;
  /** The open page: dropping it on itself does nothing. */
  pageId: string | null;
}

/** The page id of a sidebar page drag that may be dropped here, `null` when the drag is not one or is to be ignored. */
export function droppedPage(data: DataTransfer | null, options: PageDropOptions): string | null {
  if (data === null || !Array.from(data.types).includes(PAGE_DRAG_TYPE)) {
    return null;
  }
  const dragged = data.getData(PAGE_DRAG_TYPE);
  const draggedSpace = data.getData(PAGE_DRAG_SPACE_TYPE);
  if (
    dragged === "" ||
    dragged === options.pageId ||
    (options.spaceId !== null && draggedSpace !== "" && draggedSpace !== options.spaceId)
  ) {
    return null;
  }
  return dragged;
}

/**
 * Puts a mention of `pageId` (and a space) at a document position inside inline content, or at the start or the end of the
 * document: in the first/last block if that is an empty paragraph, else in a new paragraph. False when nothing was inserted.
 */
export function insertDroppedPage(view: EditorView, pageId: string, where: number | "start" | "end"): boolean {
  const { schema, doc } = view.state;
  const ref = schema.nodes.pageRef;
  const paragraph = schema.nodes.paragraph;
  if (ref === undefined || paragraph === undefined) {
    return false;
  }
  const content = [ref.create({ pageId }), schema.text(" ")];
  const tr = view.state.tr;
  if (typeof where === "number") {
    if (!doc.resolve(where).parent.inlineContent) {
      return false;
    }
    tr.insert(where, content);
  } else {
    const block = where === "start" ? doc.firstChild : doc.lastChild;
    if (block?.type === paragraph && block.content.size === 0) {
      tr.insert(where === "start" ? 1 : doc.content.size - 1, content);
    } else {
      tr.insert(where === "start" ? 0 : doc.content.size, paragraph.create(null, content));
    }
  }
  view.dispatch(tr.scrollIntoView());
  view.focus();
  return true;
}

/**
 * Accepts a sidebar page drag over the whole page area `container` (not only over the text, where ProseMirror does it) and
 * inserts the mention on drop: beside the text at that line's start or end, below the text at the end, above it at the start.
 * Drops on the ProseMirror element itself, or already handled, are left to the plugin. Returns the detach function.
 */
export function attachPageAreaDrop(
  container: HTMLElement,
  view: EditorView,
  options: PageDropOptions,
): () => void {
  const mine = (event: DragEvent): boolean =>
    view.editable &&
    !event.defaultPrevented &&
    !(event.target instanceof Node && view.dom.contains(event.target)) &&
    event.dataTransfer !== null &&
    Array.from(event.dataTransfer.types).includes(PAGE_DRAG_TYPE);
  const onDragOver = (event: DragEvent): void => {
    if (mine(event)) {
      event.preventDefault();
      if (event.dataTransfer !== null) {
        event.dataTransfer.dropEffect = "copy";
      }
    }
  };
  const onDrop = (event: DragEvent): void => {
    if (!mine(event)) {
      return;
    }
    event.preventDefault();
    const dragged = droppedPage(event.dataTransfer, options);
    if (dragged === null) {
      return;
    }
    const rect = view.dom.getBoundingClientRect();
    if (event.clientY < rect.top) {
      insertDroppedPage(view, dragged, "start");
      return;
    }
    if (event.clientY <= rect.bottom) {
      const left = Math.min(Math.max(event.clientX, rect.left + 2), rect.right - 2);
      const at = view.posAtCoords({ left, top: event.clientY });
      if (at !== null && insertDroppedPage(view, dragged, at.pos)) {
        return;
      }
    }
    insertDroppedPage(view, dragged, "end");
  };
  container.addEventListener("dragover", onDragOver);
  container.addEventListener("drop", onDrop);
  return () => {
    container.removeEventListener("dragover", onDragOver);
    container.removeEventListener("drop", onDrop);
  };
}

export const PageDrop = Extension.create<PageDropOptions>({
  name: "pageDrop",

  addOptions() {
    return { spaceId: null, pageId: null };
  },

  addProseMirrorPlugins() {
    const options = this.options;
    return [
      new Plugin({
        key: new PluginKey("pageDrop"),
        props: {
          handleDrop: (view, event) => {
            const data = event.dataTransfer;
            if (data === null || !Array.from(data.types).includes(PAGE_DRAG_TYPE)) {
              return false;
            }
            const dragged = droppedPage(data, options);
            const target = view.posAtCoords({ left: event.clientX, top: event.clientY });
            if (dragged !== null && target !== null && insertDroppedPage(view, dragged, target.pos)) {
              event.preventDefault();
            }
            return true; // ours either way
          },
        },
      }),
    ];
  },
});
