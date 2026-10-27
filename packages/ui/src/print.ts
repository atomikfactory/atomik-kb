// Print mode (ADR-0036, `docs/architecture/import-export.md` section 4): "Print...", Ctrl+P / Cmd+P with
// a page open and the Export dialog's PDF choice all turn on the store's `printing` flag. While it is on:
//
// - the node views that draw lazily draw everything (every row of a table view, every picture of a
//   gallery; a PDF or a media file shows its card instead of the viewer, since paper has no player);
// - the page shows in the light palette whatever the theme (`data-theme="light"` on <html> for the
//   duration, so diagrams and whiteboards, which follow it, draw again in their light look) and in the
//   default accent whatever the chosen one (no `data-accent` on <html> for the duration, ADR-0040);
// - the app waits for fonts, pictures and diagrams (at most `PRINT_WAIT_MS`), then calls
//   `window.print()`;
// - `afterprint` turns the mode off (so does leaving the page, and, for a webview whose print call does
//   nothing and says nothing, the first press on the page after it).
//
// `@media print` (app.css) then shows only the page. The webview's print dialog offers "Save as PDF".

import { useEffect } from "react";

import { insideOverlay } from "./shortcuts";
import { useAppStore, useStoreApi } from "./store-context";

/** The longest the app waits for fonts and pictures before it prints anyway, in milliseconds. */
export const PRINT_WAIT_MS = 10_000;

/** How often the page is looked at while it waits, in milliseconds. */
const POLL_MS = 50;

/**
 * A moment for the effects of the light theme (diagrams, whiteboards) to start before the page is
 * looked at: until then nothing says they are drawing again.
 */
const SETTLE_MS = 100;

/**
 * What says something on the page is still being drawn: a diagram, an equation or a whiteboard that
 * is loading or drawing again (`data-state="loading"`, `data-busy`), a picture whose bytes are still
 * being read (`is-loading`), a page whose document is still opening (`editor-status`).
 */
const PENDING = '[data-state="loading"], [data-busy], .attachment-card.is-loading, .editor-status';

/** True for Ctrl+P (Cmd+P on macOS), the key that prints the page. */
export function isPrintShortcut(event: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "isComposing">): boolean {
  if (event.isComposing || event.altKey || event.shiftKey || !(event.ctrlKey || event.metaKey)) {
    return false;
  }
  return event.key.toLowerCase() === "p" || event.code === "KeyP";
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

/** Whether a picture of the page has not loaded yet (one without a source never will). */
function pictureLoading(image: HTMLImageElement): boolean {
  return image.getAttribute("src") !== null && image.getAttribute("src") !== "" && !image.complete;
}

/** Whether something in `root` is still being drawn or loaded. */
export function isPagePending(root: ParentNode): boolean {
  return root.querySelector(PENDING) !== null || Array.from(root.querySelectorAll("img")).some(pictureLoading);
}

export interface PrintWaitOptions {
  /** Where to look; the main area (the page and the banners above it) by default, else the whole document. */
  root?: ParentNode;
  timeoutMs?: number;
  pollMs?: number;
  /** Stops the wait (print mode ended meanwhile): it resolves `"cancelled"` at once. */
  signal?: AbortSignal;
}

/**
 * Resolves when the fonts are loaded and nothing on the page is still being drawn or loaded
 * (`"ready"`), when `timeoutMs` ran out (`"timeout"`: print what is there), or when `signal` aborted it.
 */
export async function waitForPrintReady(options: PrintWaitOptions = {}): Promise<"ready" | "timeout" | "cancelled"> {
  const { timeoutMs = PRINT_WAIT_MS, pollMs = POLL_MS, signal } = options;
  const root = options.root ?? document.querySelector(".content-body") ?? document;
  const deadline = Date.now() + timeoutMs;
  const left = (): number => Math.max(0, deadline - Date.now());
  // jsdom and old webviews have no font loading API.
  const fonts = (document as { fonts?: { ready: Promise<unknown> } }).fonts;
  if (fonts !== undefined) {
    await Promise.race([fonts.ready.then(() => undefined), sleep(left())]);
  }
  await sleep(Math.min(SETTLE_MS, left()));
  while (signal?.aborted !== true && isPagePending(root)) {
    if (left() === 0) {
      return "timeout";
    }
    await sleep(Math.min(pollMs, left()));
  }
  return signal?.aborted === true ? "cancelled" : "ready";
}

/**
 * Runs print mode until the returned function is called: the light theme and the default accent, `title`
 * as the document's title (when given), the wait, then
 * `window.print()`; `onAfterPrint` is called when the print dialog is gone (the `afterprint` event). A
 * webview whose `window.print()` does nothing never says so: the first press on the page after the call
 * returned ends the mode as well (the print dialog of a webview that has one keeps the keyboard and the
 * pointer while it is up).
 */
export function runPrintMode(onAfterPrint: () => void, options: PrintWaitOptions = {}, title?: string): () => void {
  const root = document.documentElement;
  const theme = root.dataset.theme;
  const accent = root.dataset.accent;
  root.dataset.theme = "light";
  delete root.dataset.accent;
  // The print dialog names the PDF and heads every sheet with the document's title.
  const appTitle = document.title;
  if (title !== undefined) {
    document.title = title;
  }
  const wait = new AbortController();
  let disarm: () => void = () => undefined;
  window.addEventListener("afterprint", onAfterPrint);
  void waitForPrintReady({ ...options, signal: wait.signal }).then((outcome) => {
    if (outcome === "cancelled") {
      return;
    }
    try {
      window.print();
    } catch {
      onAfterPrint(); // a webview that cannot print: nothing will say the dialog is gone
      return;
    }
    const pressed = (event: Event): void => {
      if (!(event instanceof KeyboardEvent && event.repeat)) {
        onAfterPrint();
      }
    };
    window.addEventListener("pointerdown", pressed, true);
    window.addEventListener("keydown", pressed, true);
    disarm = () => {
      window.removeEventListener("pointerdown", pressed, true);
      window.removeEventListener("keydown", pressed, true);
    };
  });
  return () => {
    wait.abort();
    disarm();
    window.removeEventListener("afterprint", onAfterPrint);
    document.title = appTitle;
    if (theme === undefined) {
      delete root.dataset.theme;
    } else {
      root.dataset.theme = theme;
    }
    if (accent !== undefined) {
      root.dataset.accent = accent;
    }
  };
}

/**
 * Print mode for the open page (mount it once, in the page view, with the title the printed document
 * takes: the PDF's name and the sheets' header): Ctrl+P / Cmd+P turns it on and
 * keeps the webview from printing the whole window, the `printing` flag runs it, and leaving the
 * page ends it. The shortcut does nothing under a dialog or a menu (which own the keyboard), but the
 * webview's own printing is still cancelled there.
 */
export function usePrintMode(title: string): void {
  const store = useStoreApi();
  const printing = useAppStore((state) => state.printing);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent): void {
      if (event.defaultPrevented || !isPrintShortcut(event)) {
        return;
      }
      event.preventDefault();
      if (!insideOverlay(event.target)) {
        store.getState().startPrint();
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [store]);

  useEffect(() => {
    if (!printing) {
      return;
    }
    return runPrintMode(
      () => {
        store.getState().endPrint();
      },
      {},
      title,
    );
  }, [printing, store, title]);

  // The page goes (another page opens, the app locks): the mode goes with it.
  useEffect(
    () => () => {
      store.getState().endPrint();
    },
    [store],
  );
}
