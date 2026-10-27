// The handle at the left of the top-level block under the pointer: "+" adds a block below
// (and opens the `/` menu), the grip opens the block menu (turn into, duplicate, copy link to
// block, delete, move up and down) and drags the block. A click on the grip selects the block
// (Shift+click extends the selection to it, `block-selection.ts`); when the block is in a
// selected range, the menu and the drag act on the whole range (`block-range-commands.ts`). A
// drag is ProseMirror's own: the block (or the range) becomes the selection and `view.dragging`
// carries it, so a drop moves it (delete + insert; the blocks keep their ids, "moved blocks
// win", ADR-0011).

import type { Editor } from "@tiptap/core";
import type { Slice } from "@tiptap/pm/model";
import { NodeSelection } from "@tiptap/pm/state";
import type { Transaction } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { useEditorState } from "@tiptap/react";
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";

import { writeClipboard } from "../clipboard";
import { anchorBelow } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { GripIcon, PlusIcon } from "../components/icons";
import { Menu } from "../components/Menu";
import type { MenuItem } from "../components/Menu";
import { t, tp } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAiCapabilities } from "./ai/availability";
import { useEditorAi } from "./ai/EditorAi";
import { blockLinkText } from "./block-link";
import {
  canMoveBlocks,
  canTurnBlocksInto,
  deleteBlocks,
  duplicateBlocks,
  moveBlocks,
  turnBlocksInto,
  turnIntoKindsOf,
} from "./block-range-commands";
import {
  BlockRangeSelection,
  blockCount,
  blockElementAt,
  blockPositionOf,
  blockRangeOf,
  extendBlockSelectionTo,
  rangeHas,
  selectBlocks,
  singleBlockRange,
} from "./block-selection";
import type { BlockRange } from "./block-selection";
import type { BlockKind } from "./blocks";
import { isListNode, sortListTransaction } from "./list-sort";

const KIND_LABELS: Record<BlockKind, MessageKey> = {
  paragraph: "slash.text",
  heading1: "slash.heading1",
  heading2: "slash.heading2",
  heading3: "slash.heading3",
  bulletList: "slash.bulletList",
  orderedList: "slash.orderedList",
  taskList: "slash.taskList",
  blockquote: "slash.quote",
  callout: "slash.callout",
  codeBlock: "slash.codeBlock",
  mathBlock: "slash.math",
  toggle: "slash.toggle",
  toggleHeading1: "slash.toggleHeading1",
  toggleHeading2: "slash.toggleHeading2",
  toggleHeading3: "slash.toggleHeading3",
  image: "slash.image",
  file: "slash.file",
};

/** How long the "link copied" message stays on screen (a link to copy by hand stays longer). */
const NOTICE_MS = 3000;
const NOTICE_BY_HAND_MS = 12_000;

interface Hovered {
  /** Start of the top-level block. */
  pos: number;
  /** Its top edge, relative to the editor's box. */
  top: number;
}

interface OpenMenu {
  pos: number;
  anchor: Anchor;
  trigger: Element | null;
  level: "main" | "turnInto";
}

/** "1 block" or "3 blocks": what a drag of that many blocks is called. */
export function blocksLabel(count: number): string {
  return tp("blockSelection", count);
}

/** "1 block selected" or "3 blocks selected", for the status message. */
export function blockSelectionStatus(count: number): string {
  return tp("blockSelection.status", count);
}

/** Puts `slice` into the drag's data (HTML and text, as a copy would) and in `view.dragging`, as a move (ids stay). */
function carry(view: EditorView, slice: Slice, data: DataTransfer): void {
  const { dom, text } = view.serializeForClipboard(slice);
  data.clearData();
  data.setData("text/html", dom.innerHTML);
  data.setData("text/plain", text);
  // "move" keeps ids (UniqueID only renews the ids of copies and of drops from elsewhere).
  data.effectAllowed = "move";
  view.dragging = { slice, move: true };
}

/** A small label for a drag of several blocks ("3 blocks"), off screen: the browser draws it as the drag image. */
function dragImage(label: string): HTMLElement {
  const image = document.createElement("div");
  image.className = "block-drag-image";
  image.textContent = label;
  document.body.append(image);
  setTimeout(() => {
    image.remove();
  }, 0);
  return image;
}

/**
 * Starts ProseMirror's own drag of the top-level block at `pos`: the block becomes the
 * selection and `view.dragging` carries it, so a drop in the editor moves it. When the block is
 * in a selected range, the drag carries the whole range as one slice (the selection stays as it
 * is, so that the drop deletes exactly that range).
 */
export function startBlockDrag(view: EditorView, pos: number, data: DataTransfer): void {
  const block = view.state.doc.nodeAt(pos);
  if (block === null) {
    return;
  }
  const { selection } = view.state;
  const range = selection instanceof BlockRangeSelection ? blockRangeOf(selection) : null;
  if (range !== null && rangeHas(range, pos)) {
    carry(view, selection.content(), data);
    data.setDragImage(dragImage(blocksLabel(blockCount(range))), 0, 0);
    return;
  }
  view.dispatch(view.state.tr.setSelection(NodeSelection.create(view.state.doc, pos)));
  carry(view, view.state.selection.content(), data);
  const element = view.nodeDOM(pos);
  if (element instanceof HTMLElement) {
    data.setDragImage(element, 0, 0);
  }
}

export function BlockHandle({ editor, pageId }: { editor: Editor; pageId: string }) {
  const [hovered, setHovered] = useState<Hovered | null>(null);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [notice, setNotice] = useState<{ text: string; byHand: boolean } | null>(null);
  // The handle lives next to the editor's root, inside the element that holds it: a drag
  // that starts there counts as a drag inside the editor (UniqueID keeps the block's ids).
  // It gets an element of its own, because `EditorContent` moves its children around.
  const [host] = useState(() => {
    const element = document.createElement("div");
    element.className = "block-handle-host";
    return element;
  });
  const view = editor.view;
  const ai = useEditorAi();
  const askAi = useAiCapabilities().writing && ai !== null;
  // How many blocks a block range selects (0 for any other selection), for the status message.
  const selectedBlocks = useEditorState({
    editor,
    selector: ({ editor: current }) =>
      current.state.selection instanceof BlockRangeSelection ? current.state.selection.count : 0,
  });

  useEffect(() => {
    if (notice === null) {
      return;
    }
    const timer = setTimeout(
      () => {
        setNotice(null);
      },
      notice.byHand ? NOTICE_BY_HAND_MS : NOTICE_MS,
    );
    return () => {
      clearTimeout(timer);
    };
  }, [notice]);

  useEffect(() => {
    view.dom.parentElement?.append(host);
    return () => {
      host.remove();
    };
  }, [host, view]);

  useEffect(() => {
    const root = view.dom;
    const onMove = (event: MouseEvent): void => {
      const element = blockElementAt(root, event.target);
      if (element === null) {
        return;
      }
      const pos = blockPositionOf(view, element);
      if (pos === null) {
        return;
      }
      const base = (root.parentElement ?? root).getBoundingClientRect();
      const top = element.getBoundingClientRect().top - base.top;
      setHovered((current) => (current?.pos === pos && current.top === top ? current : { pos, top }));
    };
    const onTransaction = ({ transaction }: { transaction: Transaction }): void => {
      if (!transaction.docChanged) {
        return;
      }
      setHovered(null); // hidden while typing; the pointer brings it back
      setMenu((current) => (current === null ? null : { ...current, pos: transaction.mapping.map(current.pos) }));
    };
    root.addEventListener("mousemove", onMove);
    editor.on("transaction", onTransaction);
    return () => {
      root.removeEventListener("mousemove", onMove);
      editor.off("transaction", onTransaction);
    };
  }, [editor, view]);

  function run(tr: Transaction | null): void {
    if (tr !== null) {
      view.dispatch(tr);
      view.focus();
    }
  }

  /** The blocks an action of the handle at `pos` applies to: the selected range when the block is in it, else the block. */
  function rangeAt(pos: number): BlockRange | null {
    const selected = blockRangeOf(view.state.selection);
    return selected !== null && rangeHas(selected, pos) ? selected : singleBlockRange(view.state.doc, pos);
  }

  /** Selects the block at `pos` unless it is in the selected range already (the menu then acts on the range). */
  function selectForMenu(pos: number): void {
    const range = singleBlockRange(view.state.doc, pos);
    const selected = blockRangeOf(view.state.selection);
    if (range !== null && (selected === null || !rangeHas(selected, pos))) {
      const tr = selectBlocks(view.state, range.first);
      if (tr !== null) {
        view.dispatch(tr);
      }
    }
    // Focus on the editor, so that the menu gives it back when it closes (Escape, Backspace
    // and Ctrl+C then act on the selected blocks).
    view.focus();
  }

  function addBelow(pos: number): void {
    const block = view.state.doc.nodeAt(pos);
    if (block === null) {
      return;
    }
    const after = pos + block.nodeSize;
    editor
      .chain()
      .insertContentAt(after, { type: "paragraph" })
      .setTextSelection(after + 1)
      .insertContent("/")
      .focus()
      .run();
  }


  /**
   * Writes the link to the block at `pos` (`atomik-kb://page/<pageId>#<blockId>`, block-link.ts)
   * to the clipboard and says so; where the clipboard cannot be written, shows the link to copy by hand.
   */
  async function copyLink(pos: number): Promise<void> {
    const id: unknown = view.state.doc.nodeAt(pos)?.attrs.id;
    if (typeof id !== "string") {
      return;
    }
    const link = blockLinkText(pageId, id);
    try {
      await writeClipboard(link);
      setNotice({ text: t("block.linkCopied"), byHand: false });
    } catch {
      setNotice({ text: t("block.linkNoClipboard", { link }), byHand: true });
    }
  }

  function mainItems(pos: number): MenuItem[] {
    const range = rangeAt(pos);
    if (range === null) {
      return [];
    }
    const items: MenuItem[] = [
      ...(askAi ? [{ id: "askAi", label: t("aiWriter.askBlock") }] : []),
      { id: "turnInto", label: t("block.turnInto") },
      { id: "duplicate", label: t("block.duplicate") },
    ];
    if (blockCount(range) === 1) {
      // A link points at one block.
      items.push({
        id: "copyLink",
        label: t("block.copyLink"),
        disabled: typeof view.state.doc.nodeAt(range.from)?.attrs.id !== "string",
      });
      // A list block (ADR-0052): its items, A to Z, once; nothing to do (fewer than two items, or in order) is disabled.
      if (isListNode(view.state.doc.nodeAt(range.from))) {
        items.push({ id: "sortList", label: t("block.sortList"), disabled: sortListTransaction(view.state, range.from) === null });
      }
    }
    items.push(
      { id: "up", label: t("block.moveUp"), disabled: !canMoveBlocks(view.state, range, -1) },
      { id: "down", label: t("block.moveDown"), disabled: !canMoveBlocks(view.state, range, 1) },
      { id: "delete", label: t("block.delete"), danger: true },
    );
    return items;
  }

  function kindItems(pos: number): MenuItem[] {
    const range = rangeAt(pos);
    if (range === null) {
      return [];
    }
    return turnIntoKindsOf(view.state, range).map((kind) => ({
      id: kind,
      label: t(KIND_LABELS[kind]),
      disabled: !canTurnBlocksInto(view.state, range, kind),
    }));
  }

  function choose(open: OpenMenu, id: string): void {
    const range = rangeAt(open.pos);
    if (range === null) {
      return;
    }
    if (open.level === "turnInto") {
      run(turnBlocksInto(view.state, range, id as BlockKind));
      return;
    }
    switch (id) {
      case "askAi":
        // The blocks the menu acts on, as text: the writing panel opens under them.
        ai?.writer.open({ kind: "blocks", from: range.from, to: range.to });
        return;
      case "turnInto":
        setMenu({ ...open, level: "turnInto" });
        return;
      case "duplicate":
        run(duplicateBlocks(view.state, range));
        return;
      case "copyLink":
        void copyLink(range.from);
        return;
      case "sortList":
        run(sortListTransaction(view.state, range.from));
        return;
      case "up":
        run(moveBlocks(view.state, range, -1));
        return;
      case "down":
        run(moveBlocks(view.state, range, 1));
        return;
      case "delete":
        run(deleteBlocks(view.state, range));
        return;
    }
  }

  const handle =
    hovered !== null && editor.isEditable
      ? createPortal(
          <div className="block-handle" style={{ top: hovered.top }} contentEditable={false}>
            <button
              type="button"
              tabIndex={-1}
              className="block-handle-button"
              aria-label={t("block.add")}
              title={t("block.add")}
              onMouseDown={(event) => {
                event.preventDefault();
              }}
              onClick={() => {
                addBelow(hovered.pos);
              }}
            >
              <PlusIcon />
            </button>
            <button
              type="button"
              tabIndex={-1}
              className="block-handle-button block-handle-grip"
              aria-label={t("block.actions")}
              title={t("block.actionsHint")}
              aria-haspopup="menu"
              draggable
              onDragStart={(event) => {
                startBlockDrag(view, hovered.pos, event.dataTransfer);
              }}
              onDragEnd={() => {
                // ProseMirror clears `view.dragging` on its own `dragend`, which never comes here (the
                // handle is outside the editor's root): a cancelled drag must not stay a move.
                const dragging = view.dragging;
                setTimeout(() => {
                  if (view.dragging === dragging) {
                    view.dragging = null;
                  }
                }, 50);
              }}
              onClick={(event) => {
                const trigger = event.currentTarget;
                if (event.shiftKey) {
                  // Shift+click grows the selected range (or the block the caret is in) to this block.
                  setMenu(null);
                  run(extendBlockSelectionTo(view.state, view.state.doc.resolve(hovered.pos).index(0)));
                  return;
                }
                if (menu?.trigger === trigger) {
                  setMenu(null);
                  return;
                }
                selectForMenu(hovered.pos);
                setMenu({ pos: hovered.pos, anchor: anchorBelow(trigger), trigger, level: "main" });
              }}
            >
              <GripIcon />
            </button>
          </div>,
          host,
        )
      : null;

  return (
    <>
      {handle}
      {/* A live region that is always there (a message put into a new one is not reliably announced), empty and invisible without a selection. It has no `status` role: the link notice below is the one. */}
      <p className="block-selection-status" aria-live="polite">
        {selectedBlocks > 0 ? blockSelectionStatus(selectedBlocks) : ""}
      </p>
      {notice !== null && (
        <p className="block-link-notice" role="status">
          {notice.text}
        </p>
      )}
      {menu !== null && (
        <Menu
          key={menu.level}
          label={menu.level === "main" ? t("block.actions") : t("block.turnInto")}
          items={menu.level === "main" ? mainItems(menu.pos) : kindItems(menu.pos)}
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          onSelect={(id) => {
            choose(menu, id);
          }}
          onClose={() => {
            setMenu((current) => (current?.level === "turnInto" && menu.level === "main" ? current : null));
          }}
        />
      )}
    </>
  );
}
