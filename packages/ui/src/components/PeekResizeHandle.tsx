import { useRef, useState } from "react";

import { t } from "../i18n";
import { useAppStore, useStoreApi } from "../store-context";
import { PEEK_DEFAULT_WIDTH, PEEK_KEY_STEP, PEEK_MAX_WIDTH, PEEK_MIN_WIDTH, clampPeekWidth, selectPeekWidth } from "../store-peek";

/** The CSS variable on the main area that holds the side peek's width (`database-peek.css` reads it). */
const WIDTH_VARIABLE = "--peek-width";

interface Drag {
  pointerId: number;
  startX: number;
  startWidth: number;
  last: number;
}

/**
 * The 6 px handle on the left edge of the side peek (ADR-0054), a copy of the sidebar's (`SidebarResizeHandle`, ADR-0029)
 * for the other side of a panel: a drag to the left widens the peek. It changes the width live through a CSS variable on
 * the main area (nothing else re-renders) and writes the setting once, on release; Escape or a cancelled pointer puts the
 * old width back. Arrow Left widens and Arrow Right narrows it by 16 px, a double click resets it. It is a child of the
 * peek, which is a child of the main area.
 */
export function PeekResizeHandle() {
  const store = useStoreApi();
  const width = useAppStore(selectPeekWidth);
  const setPeekWidth = useAppStore((state) => state.setPeekWidth);
  // The width while a drag is going on (the setting is only written on release).
  const [live, setLive] = useState<number | null>(null);
  const drag = useRef<Drag | null>(null);

  function show(handle: HTMLElement, pixels: number): void {
    handle.closest<HTMLElement>(".content")?.style.setProperty(WIDTH_VARIABLE, `${String(pixels)}px`);
  }

  function finish(handle: HTMLElement, commit: boolean): void {
    const state = drag.current;
    if (state === null) {
      return;
    }
    drag.current = null;
    setLive(null);
    try {
      handle.releasePointerCapture(state.pointerId);
    } catch {
      // Already released, or a synthetic pointer.
    }
    if (commit && state.last !== state.startWidth) {
      setPeekWidth(state.last);
      show(handle, state.last);
    } else {
      show(handle, selectPeekWidth(store.getState()));
    }
  }

  return (
    <div
      className={live === null ? "peek-resize" : "peek-resize is-dragging"}
      role="separator"
      aria-orientation="vertical"
      aria-label={t("peek.resize")}
      aria-valuemin={PEEK_MIN_WIDTH}
      aria-valuemax={PEEK_MAX_WIDTH}
      aria-valuenow={live ?? width}
      tabIndex={0}
      onMouseDown={(event) => {
        // No text selection while dragging; focus the handle by hand.
        event.preventDefault();
        event.currentTarget.focus({ preventScroll: true });
      }}
      onPointerDown={(event) => {
        if (event.button !== 0) {
          return;
        }
        const start = selectPeekWidth(store.getState());
        drag.current = { pointerId: event.pointerId, startX: event.clientX, startWidth: start, last: start };
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
        // The panel is at the right: its left edge moving left makes it wider.
        const next = clampPeekWidth(state.startWidth + state.startX - event.clientX);
        if (next !== state.last) {
          state.last = next;
          setLive(next);
          show(event.currentTarget, next);
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
      onDoubleClick={() => {
        setPeekWidth(PEEK_DEFAULT_WIDTH);
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape" && drag.current !== null) {
          // Puts the old width back and keeps the key from closing the peek.
          event.preventDefault();
          finish(event.currentTarget, false);
          return;
        }
        if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
          return;
        }
        const current = selectPeekWidth(store.getState());
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          setPeekWidth(current + PEEK_KEY_STEP);
        } else if (event.key === "ArrowRight") {
          event.preventDefault();
          setPeekWidth(current - PEEK_KEY_STEP);
        }
      }}
    />
  );
}
