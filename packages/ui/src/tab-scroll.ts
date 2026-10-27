// The scroll position of every entry of the tabs (ADR-0034 invariant 4). Only the front tab's page
// is mounted, so the position of an entry is remembered for the session (`saveScroll`, outside the
// reactive state) when another entry takes the main area, and put back when the entry shows again.

import type { RefObject } from "react";
import { createContext, useContext, useEffect } from "react";

import { useStoreApi } from "./store-context";
import type { AppState } from "./store";
import type { TabEntry } from "./store-tabs";

/** The element that scrolls the screen in the main area (the body under the tab strip). */
export const ScrollContainerContext = createContext<RefObject<HTMLElement | null> | null>(null);

/**
 * How many animation frames a remembered position waits for the page to be tall enough: the page
 * opens asynchronously (the editor shows the document some moments after it mounts), and a
 * position below the end of what is laid out so far would be cut to it. About two seconds.
 */
export const RESTORE_FRAMES = 120;

/** What the reader does to take over from a position that is being put back. */
const INPUTS = ["wheel", "touchstart", "pointerdown", "keydown"] as const;

/** Scrolls an element to a position (a helper: the position is the element's own business, not the context's). */
function setScrollTop(element: HTMLElement, top: number): void {
  element.scrollTop = top;
}

/** True while the entry is in a tab (or among the closed tabs, which can come back with their history). */
function isKnown(state: AppState, entryId: string): boolean {
  return [...state.tabs, ...state.closedTabs].some((tab) => tab.entries.some((entry) => entry.id === entryId));
}

/**
 * Keeps the scroll position of the entry that shows in the main area.
 *
 * While the entry shows, its position is followed in a plain variable (a store write on every
 * scroll event would re-render nothing but still cost). It is saved when the entry stops showing (a
 * tab switch, a navigation, back or forward, the screen unmounting) and put back when it shows
 * again, once the page is laid out; a new entry starts at the top. A block that a search result
 * or a link points at (`scrollTarget` for the entry's page) wins over a remembered position. A
 * wheel, touch, pointer or key press while the position is being put back leaves the reader where
 * they are.
 *
 * `shown` says that what the entry stands for is on screen (its page is in the tree, or it is a
 * Home); it starts the restore again when the page replaces the Home that showed in its place
 * while the tree was loading.
 */
export function useEntryScroll(entry: TabEntry | null, shown: boolean): void {
  const containerRef = useContext(ScrollContainerContext);
  const store = useStoreApi();
  const entryId = entry?.id ?? null;
  const pageId = entry?.pageId ?? null;

  useEffect(() => {
    const found = containerRef?.current ?? null;
    if (found === null || entryId === null) {
      return;
    }
    const container: HTMLElement = found;
    if (!shown) {
      setScrollTop(container, 0); // a stand-in is up until the page is: its position is not the entry's
      return;
    }
    const state = store.getState();
    const saved = state.savedScroll(entryId);
    const targeted = pageId !== null && state.scrollTarget?.pageId === pageId;
    const wanted = targeted || saved === undefined || saved <= 0 ? null : saved;

    const track = { top: wanted ?? 0, restoring: wanted !== null };
    setScrollTop(container, 0);

    function onScroll(): void {
      if (!track.restoring) {
        track.top = container.scrollTop;
      }
    }
    container.addEventListener("scroll", onScroll, { passive: true });

    let frame = 0;
    let handle = 0;
    function stopRestoring(): void {
      track.restoring = false;
      track.top = container.scrollTop;
      cancelAnimationFrame(handle);
      for (const type of INPUTS) {
        container.removeEventListener(type, stopRestoring);
      }
    }
    function restore(target: number): void {
      handle = 0;
      frame += 1;
      if (!track.restoring) {
        return;
      }
      if (container.scrollHeight - container.clientHeight >= target || frame >= RESTORE_FRAMES) {
        setScrollTop(container, target);
        stopRestoring();
        return;
      }
      handle = requestAnimationFrame(() => {
        restore(target);
      });
    }
    if (wanted !== null) {
      for (const type of INPUTS) {
        container.addEventListener(type, stopRestoring, { passive: true });
      }
      restore(wanted); // at once when the page is tall enough already, else frame by frame
    }

    return () => {
      container.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(handle);
      for (const type of INPUTS) {
        container.removeEventListener(type, stopRestoring);
      }
      // A position still being waited for is not the reader's: the one remembered stays.
      if (!track.restoring && isKnown(store.getState(), entryId)) {
        store.getState().saveScroll(entryId, track.top);
      }
    };
  }, [containerRef, store, entryId, pageId, shown]);
}
