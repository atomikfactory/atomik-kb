// How a gesture on a page link opens the page (ADR-0034 section 2): a plain click navigates the
// active tab, Ctrl/Cmd + click and a middle click open a background tab, Ctrl/Cmd + Shift + click
// opens a tab in front.

import type { OpenDisposition } from "./store-tabs";

/** The parts of a mouse or keyboard event that choose the disposition. */
export interface DispositionGesture {
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  /** Mouse button (1 is the middle button); absent for a key. */
  button?: number;
}

export function dispositionFromEvent(event: DispositionGesture): OpenDisposition {
  const modifier = event.ctrlKey || event.metaKey;
  if (modifier && event.shiftKey) {
    return "foreground";
  }
  if (modifier || event.button === 1) {
    return "background";
  }
  return "current";
}
