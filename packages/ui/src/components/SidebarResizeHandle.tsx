import { useRef, useState } from "react";

import { t } from "../i18n";
import { useAppStore, useStoreApi } from "../store-context";
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_KEY_STEP,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  clampSidebarWidth,
  selectSidebarWidth,
} from "../store-layout";

/** The CSS variable on the layout that holds the sidebar's width (`.layout` in app.css reads it). */
const WIDTH_VARIABLE = "--sidebar-width";

interface Drag {
  pointerId: number;
  startX: number;
  startWidth: number;
  last: number;
}

/**
 * The 6 px handle on the right edge of the sidebar (ADR-0029). A drag changes the width live,
 * through a CSS variable on the layout (nothing else re-renders), and writes the setting once,
 * on release; Escape or a cancelled pointer puts the old width back. Arrow Left and Right change
 * it by 16 px, a double click resets it. It is a child of the layout, next to the sidebar.
 */
export function SidebarResizeHandle() {
  const store = useStoreApi();
  const width = useAppStore(selectSidebarWidth);
  const setSidebarWidth = useAppStore((state) => state.setSidebarWidth);
  // The width while a drag is going on (the setting is only written on release).
  const [live, setLive] = useState<number | null>(null);
  const drag = useRef<Drag | null>(null);

  function show(handle: HTMLElement, pixels: number): void {
    handle.parentElement?.style.setProperty(WIDTH_VARIABLE, `${String(pixels)}px`);
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
      setSidebarWidth(state.last);
      show(handle, state.last);
    } else {
      show(handle, selectSidebarWidth(store.getState()));
    }
  }

  return (
    <div
      className={live === null ? "sidebar-resize" : "sidebar-resize is-dragging"}
      role="separator"
      aria-orientation="vertical"
      aria-label={t("sidebar.resize")}
      aria-valuemin={SIDEBAR_MIN_WIDTH}
      aria-valuemax={SIDEBAR_MAX_WIDTH}
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
        const start = selectSidebarWidth(store.getState());
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
        const next = clampSidebarWidth(state.startWidth + event.clientX - state.startX);
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
        setSidebarWidth(SIDEBAR_DEFAULT_WIDTH);
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape" && drag.current !== null) {
          event.preventDefault();
          finish(event.currentTarget, false);
          return;
        }
        if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) {
          return;
        }
        const current = selectSidebarWidth(store.getState());
        if (event.key === "ArrowLeft") {
          event.preventDefault();
          setSidebarWidth(current - SIDEBAR_KEY_STEP);
        } else if (event.key === "ArrowRight") {
          event.preventDefault();
          setSidebarWidth(current + SIDEBAR_KEY_STEP);
        }
      }}
    />
  );
}
