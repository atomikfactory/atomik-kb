// The controls an image block draws around its picture (ADR-0028): the two resize handles,
// the toolbar of alignment, caption and viewer buttons, and the caption's text field. They are
// presentational: `ImageView` (AttachmentViews.tsx) owns the state and writes the attributes.

import { useRef } from "react";
import type { ReactNode, RefObject } from "react";

import { AlignCenterIcon, AlignLeftIcon, AlignRightIcon, CaptionIcon, ExpandIcon } from "../components/icons";
import { t } from "../i18n";
import { CAPTION_MAX_LENGTH, MAX_WIDTH_PERCENT, MIN_WIDTH_PERCENT } from "./attachment-nodes";
import type { ImageAlign } from "./attachment-nodes";

/** A width kept inside 10..100 percent. */
export function clampPercent(percent: number): number {
  return Math.min(MAX_WIDTH_PERCENT, Math.max(MIN_WIDTH_PERCENT, percent));
}

/** Points a key press moves the width by: 5, or 1 with Shift. */
const KEY_STEP = 5;
const KEY_STEP_FINE = 1;

interface DragState {
  pointerId: number;
  startX: number;
  /** The width when the drag began, in percent of the column. */
  start: number;
  /** The column's width in pixels when the drag began. */
  column: number;
  /** The width the pointer is at now, in percent. */
  last: number;
}

export interface ResizeHandleProps {
  /** The picture edge this handle sits on. */
  side: "left" | "right";
  /** The width the slider reports (`aria-valuenow`), in percent of the column. */
  value: number;
  /** A centred picture grows on both sides, so the pointer's move counts twice. */
  centered: boolean;
  /** The column's width in pixels, 0 when it cannot be measured. */
  column: () => number;
  /** The picture's width now, in percent of the column (for one with no width of its own). */
  current: () => number;
  /** Shows a width while the pointer drags (`null`: back to the stored one); writes nothing. */
  onPreview: (percent: number | null) => void;
  /** One width to write: the release of a drag, or one key press. */
  onCommit: (percent: number) => void;
  /** Back to the picture's own size (double click). */
  onReset: () => void;
  /** The handle took focus: a moment to measure what the slider reports. */
  onFocus: () => void;
}

/**
 * A resize handle on the left or right edge of the picture: drag it (pointer capture, so the
 * pointer may leave it), or use the slider keys. A drag only previews the width and writes it
 * once, on release. The handle never starts the block's drag (the node view's drag handle) nor
 * a text selection, so the events that would are stopped here.
 */
export function ResizeHandle({ side, value, centered, column, current, onPreview, onCommit, onReset, onFocus }: ResizeHandleProps) {
  const drag = useRef<DragState | null>(null);

  function release(element: HTMLElement, pointerId: number): void {
    try {
      if (element.hasPointerCapture(pointerId)) {
        element.releasePointerCapture(pointerId);
      }
    } catch {
      // Not captured (a synthetic event, or the capture is gone): nothing to release.
    }
  }

  /** Ends the drag: `commit` writes the width the pointer reached, else the picture goes back. */
  function finish(element: HTMLElement, commit: boolean): void {
    const state = drag.current;
    if (state === null) {
      return;
    }
    drag.current = null;
    release(element, state.pointerId);
    if (commit && state.last !== state.start) {
      onCommit(state.last);
    }
    onPreview(null);
  }

  return (
    <div
      className="image-handle"
      data-side={side}
      role="slider"
      tabIndex={0}
      aria-label={t("attachment.image.width")}
      aria-orientation="horizontal"
      aria-valuemin={MIN_WIDTH_PERCENT}
      aria-valuemax={MAX_WIDTH_PERCENT}
      aria-valuenow={value}
      aria-valuetext={`${String(value)}%`}
      onFocus={onFocus}
      onMouseDown={(event) => {
        // No text selection, no native drag of the block; focus the handle by hand.
        event.preventDefault();
        event.stopPropagation();
        event.currentTarget.focus({ preventScroll: true });
      }}
      onDragStart={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
      onPointerDown={(event) => {
        if (event.button !== 0) {
          return;
        }
        event.stopPropagation();
        const width = column();
        if (width <= 0) {
          return;
        }
        const start = clampPercent(Math.round(current()));
        drag.current = { pointerId: event.pointerId, startX: event.clientX, start, column: width, last: start };
        try {
          event.currentTarget.setPointerCapture(event.pointerId);
        } catch {
          // A synthetic pointer cannot be captured; its events are sent to the handle anyway.
        }
      }}
      onPointerMove={(event) => {
        const state = drag.current;
        if (state === null || event.pointerId !== state.pointerId) {
          return;
        }
        const move = ((event.clientX - state.startX) * 100) / state.column;
        const sign = side === "right" ? 1 : -1;
        const next = clampPercent(Math.round(state.start + sign * (centered ? 2 : 1) * move));
        if (next !== state.last) {
          state.last = next;
          onPreview(next);
        }
      }}
      onPointerUp={(event) => {
        if (drag.current?.pointerId === event.pointerId) {
          finish(event.currentTarget, true);
        }
      }}
      onPointerCancel={(event) => {
        finish(event.currentTarget, false);
      }}
      onLostPointerCapture={(event) => {
        finish(event.currentTarget, false);
      }}
      onDoubleClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onReset();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape" && drag.current !== null) {
          event.preventDefault();
          event.stopPropagation();
          finish(event.currentTarget, false);
          return;
        }
        if (event.altKey || event.ctrlKey || event.metaKey) {
          return;
        }
        const step = event.shiftKey ? KEY_STEP_FINE : KEY_STEP;
        let next: number;
        switch (event.key) {
          case "ArrowLeft":
          case "ArrowDown":
            next = current() - step;
            break;
          case "ArrowRight":
          case "ArrowUp":
            next = current() + step;
            break;
          case "Home":
            next = MIN_WIDTH_PERCENT;
            break;
          case "End":
            next = MAX_WIDTH_PERCENT;
            break;
          default:
            return;
        }
        event.preventDefault();
        event.stopPropagation();
        const width = clampPercent(Math.round(next));
        if (width !== Math.round(current())) {
          onCommit(width);
        }
      }}
    />
  );
}

function ToolButton({
  label,
  pressed,
  buttonRef,
  onClick,
  children,
}: {
  label: string;
  pressed?: boolean;
  buttonRef?: RefObject<HTMLButtonElement | null>;
  onClick: (button: HTMLButtonElement) => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      ref={buttonRef}
      className="icon-button image-tool"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      onClick={(event) => {
        onClick(event.currentTarget);
      }}
    >
      {children}
    </button>
  );
}

export interface ImageToolbarProps {
  /** Alignment and caption are for editable pages; View full size and the menu are for all. */
  editable: boolean;
  /** `null` is centered. */
  align: ImageAlign | null;
  captionButton: RefObject<HTMLButtonElement | null>;
  onAlign: (align: ImageAlign) => void;
  onCaption: () => void;
  onView: (button: HTMLButtonElement) => void;
  /** The "⋯" menu button. */
  menuButton: ReactNode;
}

/** The buttons at the picture's top right corner: alignment, caption, full size, the menu. */
export function ImageToolbar({ editable, align, captionButton, onAlign, onCaption, onView, menuButton }: ImageToolbarProps) {
  const current = align ?? "center";
  return (
    <div className="image-toolbar">
      {editable && (
        <>
          <ToolButton
            label={t("attachment.image.alignLeft")}
            pressed={current === "left"}
            onClick={() => {
              onAlign("left");
            }}
          >
            <AlignLeftIcon />
          </ToolButton>
          <ToolButton
            label={t("attachment.image.alignCenter")}
            pressed={current === "center"}
            onClick={() => {
              onAlign("center");
            }}
          >
            <AlignCenterIcon />
          </ToolButton>
          <ToolButton
            label={t("attachment.image.alignRight")}
            pressed={current === "right"}
            onClick={() => {
              onAlign("right");
            }}
          >
            <AlignRightIcon />
          </ToolButton>
          <ToolButton label={t("attachment.image.caption")} buttonRef={captionButton} onClick={onCaption}>
            <CaptionIcon />
          </ToolButton>
        </>
      )}
      <ToolButton label={t("attachment.image.viewFull")} onClick={onView}>
        <ExpandIcon />
      </ToolButton>
      {menuButton}
    </div>
  );
}

export interface CaptionEditProps {
  value: string;
  /** The picture's alignment, which the text follows. */
  align: ImageAlign | null;
  /** Saves the text, on Enter or when the field loses focus. */
  onCommit: (text: string) => void;
  /** The edit is over: `blur` when focus went elsewhere on purpose, so that it stays there. */
  onDone: (reason: "enter" | "escape" | "blur") => void;
}

/**
 * The caption's text field: one line, Enter or leaving it commits, Escape cancels. It takes
 * focus with the text selected when it appears.
 */
export function CaptionEdit({ value, align, onCommit, onDone }: CaptionEditProps) {
  const finished = useRef(false);
  const draft = useRef(value);

  // Enter, Escape and blur can follow each other (the field blurs as it disappears); only the first counts.
  function end(reason: "enter" | "escape" | "blur", commit: boolean): void {
    if (finished.current) {
      return;
    }
    finished.current = true;
    if (commit) {
      onCommit(draft.current);
    }
    onDone(reason);
  }

  return (
    <input
      className="inline-edit image-caption-edit"
      data-align={align ?? "center"}
      type="text"
      defaultValue={value}
      maxLength={CAPTION_MAX_LENGTH}
      aria-label={t("attachment.image.captionLabel")}
      placeholder={t("attachment.image.captionPlaceholder")}
      autoComplete="off"
      autoFocus
      onFocus={(event) => {
        event.currentTarget.select();
      }}
      onChange={(event) => {
        draft.current = event.target.value;
      }}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) {
          return; // Enter or Escape confirms or cancels the input method's composition, not the edit
        }
        if (event.key === "Enter") {
          event.preventDefault();
          end("enter", true);
        } else if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          end("escape", false);
        }
      }}
      onBlur={() => {
        end("blur", true);
      }}
      onClick={(event) => {
        event.stopPropagation();
      }}
      onDoubleClick={(event) => {
        event.stopPropagation();
      }}
    />
  );
}
