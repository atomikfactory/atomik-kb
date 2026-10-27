// The mouse gestures that open a page (ADR-0034 section 2), shared by every link to a page: a click
// follows `dispositionFromEvent` (plain: the active tab, Ctrl/Cmd: a background tab, Ctrl/Cmd +
// Shift: a tab in front) and a middle click opens a background tab. A caller gives what to do with
// the disposition (usually `navigateTo({ ..., disposition })`) and spreads the handlers on the element.

import type { MouseEvent } from "react";

import { dispositionFromEvent } from "./open-disposition";
import type { DispositionGesture } from "./open-disposition";
import type { OpenDisposition } from "./store-tabs";

/** The controls inside a link whose own gestures are not "open this page" (a rename field, a toggle, a menu button). */
const CONTROLS = "button, input, textarea, select, a[href], [contenteditable='true']";

/** True when the event comes from a control inside `event.currentTarget` (not from the element itself). */
function fromInnerControl(event: MouseEvent<HTMLElement>): boolean {
  const { target, currentTarget } = event;
  if (!(target instanceof Element)) {
    return false;
  }
  const control = target.closest(CONTROLS);
  return control !== null && control !== currentTarget && currentTarget.contains(control);
}

/** True for a press that asks for another tab: the middle button, or the primary one with Ctrl or Cmd. */
function asksForTab(event: MouseEvent<HTMLElement>): boolean {
  return event.button === 1 || (event.button === 0 && (event.ctrlKey || event.metaKey));
}

export interface OpenGestureOptions {
  /**
   * Keeps a press that asks for another tab from the page editor around the link (it would select
   * the chip or move the caret there): for the links the editor draws itself. The editor's own
   * handlers sit above the link's element, so stopping the press here is enough.
   */
  fromEditor?: boolean;
}

export interface OpenGestureHandlers<E extends HTMLElement> {
  onClick: (event: MouseEvent<E>) => void;
  onAuxClick: (event: MouseEvent<E>) => void;
  onMouseDown: (event: MouseEvent<E>) => void;
}

/**
 * The handlers of an element that opens a page, calling `open` with how:
 *
 * - `onClick`: the disposition of the click's modifiers. A click that comes from a control inside the
 *   element (a rename field) opens nothing in another tab: the control keeps its own gestures.
 * - `onAuxClick`: a middle click opens a background tab (Ctrl/Cmd + Shift keeps its meaning); the
 *   other buttons do nothing. Chromium sends the middle button as `auxclick` and no `click`, so the
 *   plain-click path does not run as well.
 * - `onMouseDown`: stops the middle button from starting the browser's autoscroll.
 */
export function openGestures<E extends HTMLElement>(
  open: (disposition: OpenDisposition) => void,
  { fromEditor = false }: OpenGestureOptions = {},
): OpenGestureHandlers<E> {
  return {
    onClick: (event) => {
      if (event.button !== 0) {
        return; // a middle click is the `auxclick`'s
      }
      const disposition = dispositionFromEvent(event);
      if (disposition !== "current" && fromInnerControl(event)) {
        return;
      }
      open(disposition);
    },
    onAuxClick: (event) => {
      if (event.button !== 1 || fromInnerControl(event)) {
        return;
      }
      event.preventDefault();
      open(dispositionFromEvent(event));
    },
    onMouseDown: (event) => {
      if (fromInnerControl(event)) {
        return;
      }
      if (event.button === 1) {
        event.preventDefault(); // no autoscroll
      }
      if (fromEditor && asksForTab(event)) {
        event.preventDefault(); // no caret or selection change in the editor
        event.stopPropagation();
      }
    },
  };
}

/** The disposition of a keyboard confirmation (Ctrl+K's Enter): Ctrl or Cmd opens a tab in front. */
export function dispositionFromKey(event: Pick<DispositionGesture, "ctrlKey" | "metaKey">): OpenDisposition {
  return event.ctrlKey || event.metaKey ? "foreground" : "current";
}
