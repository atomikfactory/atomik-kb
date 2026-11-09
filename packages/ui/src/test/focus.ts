// Getting the keyboard into a field that opened next to the editor. The editor takes the focus back late: tiptap's `focus()` is deferred by a
// frame, and a loaded machine runs that frame long after the field was shown, so keys typed "when the field has the focus" can still land in the
// editor. Let the pending frames run first, then take the focus.

import { waitFor } from "@testing-library/react";
import type userEvent from "@testing-library/user-event";
import { expect } from "vitest";

/** Resolves after `count` animation frames, so that every deferred `focus()` queued before the call has run. */
function frames(count: number): Promise<void> {
  return new Promise((resolve) => {
    const step = (left: number): void => {
      if (left <= 0) {
        resolve();
        return;
      }
      window.requestAnimationFrame(() => {
        step(left - 1);
      });
    };
    step(count);
  });
}

/** Gives `field` the keyboard: lets the editor's deferred focus run, then clicks until the field holds the focus. */
export async function focusField(user: ReturnType<typeof userEvent.setup>, field: HTMLElement): Promise<void> {
  await frames(3);
  await waitFor(async () => {
    await user.click(field);
    expect(field).toHaveFocus();
  });
}
