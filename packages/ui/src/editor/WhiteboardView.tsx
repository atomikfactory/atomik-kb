// The whiteboard block (ADR-0033): the drawing as a picture at the block's height, a button that opens
// the full-screen editor (a double click on the picture does too), and, on a page that can be edited,
// two buttons that make the view shorter or taller (the `height` attribute, 160 to 1200 pixels).
// Read-only editors show the picture and a button that opens the drawing to look at. The elements
// are read from, and written to, the page document's root map through the store the extension holds
// (whiteboard-store.ts); the node only holds the place and the height.

import { NodeViewWrapper, useEditorState } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useEffect, useMemo, useRef, useState } from "react";

import { MinusIcon, PlusIcon, WhiteboardIcon } from "../components/icons";
import { t } from "../i18n";
import { BLOCK_ID_ATTRIBUTE } from "./schema";
import {
  MAX_WHITEBOARD_HEIGHT,
  MIN_WHITEBOARD_HEIGHT,
  readWhiteboardHeight,
} from "./whiteboard-nodes";
import type { WhiteboardOptions } from "./whiteboard-nodes";
import type { WhiteboardStore } from "./whiteboard-store";
import { WhiteboardEditor } from "./WhiteboardEditor";
import { WhiteboardPreview } from "./WhiteboardPreview";

/** How much the height buttons change the view's height. */
export const HEIGHT_STEP = 80;

/**
 * The node view's `stopEvent` (what ProseMirror must leave alone): the events of the buttons in the
 * block, as Tiptap's own default does for a selectable block. A click on the picture selects the block.
 */
export function whiteboardStopsEvent(event: Event): boolean {
  const target = event.target;
  return target instanceof Element && target.closest(".whiteboard-block") !== null && target.closest("button") !== null;
}

/** The elements of a drawing, read again whenever the store says they changed. */
export function useWhiteboardElements(store: WhiteboardStore | null, blockId: string) {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    if (store === null || blockId === "") {
      return;
    }
    return store.subscribe(blockId, () => {
      setVersion((current) => current + 1);
    });
  }, [store, blockId]);
  return useMemo(
    () => (store === null || blockId === "" ? [] : store.read(blockId)),
    // `version` is what makes the elements be read again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [store, blockId, version],
  );
}

export function WhiteboardView(props: ReactNodeViewProps) {
  const { node, editor, extension, updateAttributes } = props;
  const store = (extension.options as Partial<WhiteboardOptions>).store ?? null;
  const blockId = typeof node.attrs[BLOCK_ID_ATTRIBUTE] === "string" ? node.attrs[BLOCK_ID_ATTRIBUTE] : "";
  const height = readWhiteboardHeight(node.attrs.height);
  const editable = useEditorState({ editor, selector: ({ editor: current }) => current.isEditable });
  const elements = useWhiteboardElements(store, blockId);
  const [open, setOpen] = useState(false);
  const opener = useRef<HTMLElement | null>(null);
  const canOpen = store !== null && blockId !== "";

  function openEditor(from: HTMLElement | null): void {
    if (!canOpen) {
      return;
    }
    opener.current = from ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    setOpen(true);
  }

  return (
    <NodeViewWrapper className="whiteboard-block" data-whiteboard-block={blockId}>
      <div
        className="whiteboard-frame"
        contentEditable={false}
        onDoubleClick={(event) => {
          event.preventDefault();
          openEditor(null);
        }}
      >
        <div className="whiteboard-toolbar">
          <button
            type="button"
            className="button whiteboard-open"
            disabled={!canOpen}
            onClick={(event) => {
              openEditor(event.currentTarget);
            }}
          >
            <WhiteboardIcon />
            <span>{editable ? t("whiteboard.edit") : t("whiteboard.open")}</span>
          </button>
          {editable && (
            <>
              <button
                type="button"
                className="icon-button"
                aria-label={t("whiteboard.shorter")}
                title={t("whiteboard.shorter")}
                disabled={height <= MIN_WHITEBOARD_HEIGHT}
                onClick={() => {
                  updateAttributes({ height: Math.max(MIN_WHITEBOARD_HEIGHT, height - HEIGHT_STEP) });
                }}
              >
                <MinusIcon />
              </button>
              <button
                type="button"
                className="icon-button"
                aria-label={t("whiteboard.taller")}
                title={t("whiteboard.taller")}
                disabled={height >= MAX_WHITEBOARD_HEIGHT}
                onClick={() => {
                  updateAttributes({ height: Math.min(MAX_WHITEBOARD_HEIGHT, height + HEIGHT_STEP) });
                }}
              >
                <PlusIcon />
              </button>
            </>
          )}
        </div>
        <WhiteboardPreview elements={elements} height={height} />
      </div>
      {open && store !== null && (
        <WhiteboardEditor
          store={store}
          blockId={blockId}
          editable={editable}
          returnFocus={opener}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </NodeViewWrapper>
  );
}
