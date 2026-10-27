// Print mode (ADR-0036 section 4, print.ts): Ctrl+P and the page menu turn on the store's flag, the light
// theme goes on <html> for the duration, the app waits for fonts, pictures and diagrams, calls
// `window.print()` and turns the mode off on `afterprint` (or when the page goes); and while it is on the lazy
// node views draw in full: every row of a table view, every picture of a gallery, a PDF as its card, the
// properties as read-only values.

import { demoImagePng } from "@atomik-kb/core-client";
import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setDiagramRenderer } from "./editor/mermaid";
import type { DiagramTheme } from "./editor/mermaid";
import { openPdf } from "./editor/pdf-engine";
import type * as Engine from "./editor/pdf-engine";
import type { PdfDocument } from "./editor/pdf-engine";
import { setExcalidrawEngine } from "./editor/excalidraw-engine";
import type { ExcalidrawEngine } from "./editor/excalidraw-engine";
import { WhiteboardPreview } from "./editor/WhiteboardPreview";
import { t } from "./i18n";
import { PRINT_WAIT_MS, isPagePending, isPrintShortcut, waitForPrintReady } from "./print";
import { stubObjectUrls } from "./test/attachments";
import { paragraphs, pid } from "./test/editor";
import { exportResult, renderInterop } from "./test/interop";
import { idsOf, openPage } from "./test/page";
import { testPdf } from "./test/pdf";

vi.mock("./editor/pdf-engine", async (importOriginal) => ({
  ...(await importOriginal<typeof Engine>()),
  openPdf: vi.fn(),
}));

const STAND_IN_DIAGRAM = '<svg xmlns="http://www.w3.org/2000/svg" data-stand-in-diagram=""></svg>';

/** `window.print`, which jsdom cannot do: a spy the tests look at. */
const print = vi.fn<() => void>();

beforeEach(() => {
  print.mockReset();
  vi.spyOn(window, "print").mockImplementation(print);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.accent;
  setDiagramRenderer(() => Promise.resolve(STAND_IN_DIAGRAM));
  setExcalidrawEngine(null);
});

function ctrlP(target: EventTarget = document.body, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { key: "p", code: "KeyP", ctrlKey: true, bubbles: true, cancelable: true, ...init });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

function afterPrint(): void {
  act(() => {
    window.dispatchEvent(new Event("afterprint"));
  });
}

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: paragraphs("Hello") },
      { title: "Other", content: paragraphs("Elsewhere") },
    ],
  },
];

describe("the shortcut", () => {
  it("is Ctrl+P or Cmd+P, nothing else", () => {
    const key = { key: "p", code: "KeyP", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, isComposing: false };
    expect(isPrintShortcut({ ...key, ctrlKey: true })).toBe(true);
    expect(isPrintShortcut({ ...key, metaKey: true })).toBe(true);
    expect(isPrintShortcut({ ...key, ctrlKey: true, key: "P" })).toBe(true);
    // On a layout where the key types another letter the physical key still counts.
    expect(isPrintShortcut({ ...key, ctrlKey: true, key: "з", code: "KeyP" })).toBe(true);
    expect(isPrintShortcut(key)).toBe(false);
    expect(isPrintShortcut({ ...key, ctrlKey: true, shiftKey: true })).toBe(false);
    expect(isPrintShortcut({ ...key, ctrlKey: true, altKey: true })).toBe(false);
    expect(isPrintShortcut({ ...key, ctrlKey: true, isComposing: true })).toBe(false);
    expect(isPrintShortcut({ ...key, ctrlKey: true, key: "o", code: "KeyO" })).toBe(false);
  });
});

describe("waiting for the page", () => {
  const FAST = { timeoutMs: 400, pollMs: 5 };

  function pageWith(html: string): HTMLElement {
    const root = document.createElement("div");
    root.innerHTML = html;
    document.body.append(root);
    return root;
  }

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("is ready at once for a page with nothing to wait for", async () => {
    const root = pageWith('<p>text</p><img alt="no source"><img src="" alt="empty">');
    expect(isPagePending(root)).toBe(false);
    await expect(waitForPrintReady({ ...FAST, root })).resolves.toBe("ready");
  });

  it("waits for what is loading or drawing again, and for a picture whose bytes are on their way", async () => {
    const root = pageWith('<div data-state="loading"></div><div data-busy=""></div><div class="attachment-card is-loading"></div>');
    expect(isPagePending(root)).toBe(true);
    const done = waitForPrintReady({ ...FAST, root });
    setTimeout(() => {
      root.replaceChildren();
    }, 150);
    await expect(done).resolves.toBe("ready");

    const image = document.createElement("img");
    image.setAttribute("src", "blob:test/1");
    root.append(image);
    expect(isPagePending(root)).toBe(true); // jsdom never loads it
    Object.defineProperty(image, "complete", { configurable: true, value: true });
    expect(isPagePending(root)).toBe(false);
  });

  it("waits for a page whose document is still opening", () => {
    expect(isPagePending(pageWith('<p class="editor-status" role="status">Opening</p>'))).toBe(true);
  });

  it("looks at the main area, not at the sidebar around it", async () => {
    pageWith('<nav><div data-state="loading"></div></nav><main><div class="content-body"><p>Page</p></div></main>');
    await expect(waitForPrintReady({ timeoutMs: 300, pollMs: 5 })).resolves.toBe("ready");
    document.body.replaceChildren();
    pageWith('<nav></nav><main><div class="content-body"><div data-busy=""></div></div></main>');
    await expect(waitForPrintReady({ timeoutMs: 150, pollMs: 5 })).resolves.toBe("timeout");
  });

  it("gives up after the timeout and prints what is there", async () => {
    const root = pageWith('<div data-state="loading"></div>');
    const started = Date.now();
    await expect(waitForPrintReady({ root, timeoutMs: 250, pollMs: 5 })).resolves.toBe("timeout");
    expect(Date.now() - started).toBeGreaterThanOrEqual(240);
    // The real limit is ten seconds.
    expect(PRINT_WAIT_MS).toBe(10_000);
  });

  it("stops when it is cancelled", async () => {
    const root = pageWith('<div data-state="loading"></div>');
    const controller = new AbortController();
    const done = waitForPrintReady({ root, timeoutMs: 5000, pollMs: 5, signal: controller.signal });
    setTimeout(() => {
      controller.abort();
    }, 150);
    await expect(done).resolves.toBe("cancelled");
  });

  it("waits for the fonts", async () => {
    let loaded: () => void = () => undefined;
    const ready = new Promise<void>((resolve) => {
      loaded = resolve;
    });
    Object.defineProperty(document, "fonts", { configurable: true, value: { ready } });
    try {
      const outcome = vi.fn();
      void waitForPrintReady({ root: pageWith("<p>text</p>"), ...FAST }).then(outcome);
      await new Promise((resolve) => setTimeout(resolve, 80));
      expect(outcome).not.toHaveBeenCalled();
      loaded();
      await vi.waitFor(() => {
        expect(outcome).toHaveBeenCalledWith("ready");
      });
    } finally {
      Reflect.deleteProperty(document, "fonts");
    }
  });
});

describe("Ctrl+P with a page open", () => {
  it("keeps the webview from printing the window, turns the mode on, prints, and ends on afterprint", async () => {
    document.documentElement.dataset.theme = "dark";
    const { user, store } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");

    const event = ctrlP();
    expect(event.defaultPrevented).toBe(true);
    expect(store.getState().printing).toBe(true);
    // The light palette for the duration, and a line that says what is going on.
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(screen.getByText(t("print.preparing"))).toBeInTheDocument();

    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    // The mode stays on while the print dialog is up.
    expect(store.getState().printing).toBe(true);

    afterPrint();
    expect(store.getState().printing).toBe(false);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(screen.queryByText(t("print.preparing"))).not.toBeInTheDocument();
    expect(print).toHaveBeenCalledTimes(1);
  });

  it("titles the document after the page while it prints (the PDF's name and the sheets' header)", async () => {
    document.title = "atomik-kb";
    const { user } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    ctrlP();
    expect(document.title).toBe("Notes");
    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    afterPrint();
    expect(document.title).toBe("atomik-kb");
  });

  it("puts the theme back as it was, also when the page had none", async () => {
    const { user, store } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    expect(document.documentElement.dataset.theme).toBeUndefined();
    ctrlP();
    expect(document.documentElement.dataset.theme).toBe("light");
    afterPrint();
    expect(store.getState().printing).toBe(false);
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });

  it("prints in the default accent whatever the chosen one, and gives it back afterwards (ADR-0040)", async () => {
    document.documentElement.dataset.theme = "dark";
    document.documentElement.dataset.accent = "green";
    const { user, store } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");

    ctrlP();
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(document.documentElement.dataset.accent).toBeUndefined();
    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    // Still the default while the print dialog is up.
    expect(document.documentElement.dataset.accent).toBeUndefined();

    afterPrint();
    expect(store.getState().printing).toBe(false);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(document.documentElement.dataset.accent).toBe("green");
  });

  it("does not invent an accent when the page had none", async () => {
    const { user } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    expect(document.documentElement.dataset.accent).toBeUndefined();
    ctrlP();
    expect(document.documentElement.dataset.accent).toBeUndefined();
    afterPrint();
    expect(document.documentElement.dataset.accent).toBeUndefined();
    expect("accent" in document.documentElement.dataset).toBe(false);
  });

  it("gives the accent back when the page goes while printing", async () => {
    document.documentElement.dataset.accent = "purple";
    const { user, store, memory } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    ctrlP();
    expect(document.documentElement.dataset.accent).toBeUndefined();
    const ids = await idsOf(memory);
    act(() => {
      store.getState().selectPage(ids.page("Other"));
    });
    await screen.findByRole("heading", { level: 1, name: "Other" });
    expect(store.getState().printing).toBe(false);
    expect(document.documentElement.dataset.accent).toBe("purple");
  });

  it("works from the editor, and with Cmd+P", async () => {
    const { user, store } = await renderInterop({ spaces: SPACES });
    const editor = await openPage(user, "Notes");
    expect(ctrlP(editor.view.dom).defaultPrevented).toBe(true);
    expect(store.getState().printing).toBe(true);
    afterPrint();
    expect(ctrlP(editor.view.dom, { ctrlKey: false, metaKey: true }).defaultPrevented).toBe(true);
    expect(store.getState().printing).toBe(true);
  });

  it("is cancelled for the webview but does not print under a dialog", async () => {
    const { user, store } = await renderInterop({ spaces: SPACES }, { methods: { exportRun: () => Promise.resolve(exportResult()) } });
    await openPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("interop.menu.export") }));
    const dialog = await screen.findByRole("dialog");
    const event = ctrlP(within(dialog).getByRole("radio", { name: t("export.format.markdown") }));
    expect(event.defaultPrevented).toBe(true);
    expect(store.getState().printing).toBe(false);
    expect(print).not.toHaveBeenCalled();
  });

  it("leaves the keys alone where no page is open", async () => {
    const { store } = await renderInterop({ spaces: SPACES });
    const event = ctrlP();
    expect(event.defaultPrevented).toBe(false);
    expect(store.getState().printing).toBe(false);
  });

  it("ends when the page goes, and puts the theme back", async () => {
    document.documentElement.dataset.theme = "dark";
    const { user, store, memory } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    ctrlP();
    expect(store.getState().printing).toBe(true);
    const ids = await idsOf(memory);
    act(() => {
      store.getState().selectPage(ids.page("Other"));
    });
    await screen.findByRole("heading", { level: 1, name: "Other" });
    expect(store.getState().printing).toBe(false);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(print).not.toHaveBeenCalled();
  });

  it("ends on the next press on the page when the webview prints nothing and says nothing", async () => {
    const { user, store } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    ctrlP();
    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    expect(store.getState().printing).toBe(true);
    // A key held down is not a new press.
    act(() => {
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "p", ctrlKey: true, repeat: true, bubbles: true }));
    });
    expect(store.getState().printing).toBe(true);
    await user.click(screen.getByRole("heading", { level: 1 }));
    expect(store.getState().printing).toBe(false);
  });

  it("ends at once for a webview whose print call throws", async () => {
    print.mockImplementation(() => {
      throw new Error("not supported");
    });
    const { user, store } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    ctrlP();
    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(store.getState().printing).toBe(false);
    });
  });

  it("starts from the page menu's Print... entry", async () => {
    const { user, store } = await renderInterop({ spaces: SPACES });
    await openPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("interop.menu.print") }));
    expect(store.getState().printing).toBe(true);
    await waitFor(() => {
      expect(print).toHaveBeenCalledTimes(1);
    });
    afterPrint();
  });
});

describe("the page in print mode", () => {
  it("shows the properties as read-only values", async () => {
    const { user } = await renderInterop({
      spaces: [
        {
          name: "Personal",
          properties: [{ name: "Cost", kind: "number" }],
          pages: [{ title: "Notes", content: paragraphs("x"), properties: { Cost: 3 } }],
        },
      ],
    });
    await openPage(user, "Notes");
    expect(await screen.findByRole("button", { name: /Cost/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("properties.add") })).toBeInTheDocument();

    ctrlP();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: t("properties.add") })).not.toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: /Cost/ })).not.toBeInTheDocument();
    afterPrint();
    expect(await screen.findByRole("button", { name: t("properties.add") })).toBeInTheDocument();
  });

  it("draws every row of a long table view, not only the ones in view", async () => {
    const children = Array.from({ length: 130 }, (_, index) => ({ title: `Row ${String(index + 1).padStart(3, "0")}` }));
    const content: JsonValue = {
      type: "doc",
      content: [
        {
          type: "pageList",
          attrs: { id: pid(1), source: { kind: "children" }, layout: "table", properties: [], sort: { by: "title", dir: "asc" } },
        },
        { type: "paragraph", attrs: { id: pid(2) } },
      ],
    };
    const { user } = await renderInterop({ spaces: [{ name: "Personal", pages: [{ title: "Hub", content, children }] }] });
    const editor = await openPage(user, "Hub");
    const rows = (): number => editor.view.dom.querySelectorAll(".page-table-body [role='row']").length;
    await waitFor(() => {
      expect(rows()).toBeGreaterThan(0);
    });
    expect(rows()).toBeLessThan(130);

    ctrlP();
    await waitFor(() => {
      expect(rows()).toBe(130);
    });
    expect(editor.view.dom.querySelector(".page-table-scroll")).not.toHaveAttribute("data-windowed");
    afterPrint();
    await waitFor(() => {
      expect(rows()).toBeLessThan(130);
    });
  });

  it("reads every picture of a gallery, near the window or not", async () => {
    stubObjectUrls();
    // An observer that never reports anything: no picture is ever "near".
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        observe(): void {}
        unobserve(): void {}
        disconnect(): void {}
        takeRecords(): [] {
          return [];
        }
      },
    );
    const cat = "a0000000-0000-4000-8000-0000000000ca";
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) } },
        { type: "gallery", attrs: { id: pid(1), columns: 2, items: [{ attachmentId: cat }] } },
      ],
    };
    const { user } = await renderInterop(
      { spaces: [{ name: "Personal", pages: [{ title: "Trip", content }] }], attachments: [{ attachmentId: cat, name: "cat.png", mime: "image/png", bytes: demoImagePng() }] },
    );
    const editor = await openPage(user, "Trip");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".gallery-thumb")).not.toBeNull();
    });
    expect(editor.view.dom.querySelectorAll("img")).toHaveLength(0);

    ctrlP();
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll("img")).toHaveLength(1);
    });
    // A picture still on its way is what the wait is for.
    expect(isPagePending(editor.view.dom)).toBe(true);
    afterPrint();
  });

  it("prints a PDF as its card, with the name and size, and gets the viewer back afterwards", async () => {
    const fake: PdfDocument = {
      pages: 1,
      page: () => Promise.resolve({ width: 200, height: 300, render: () => ({ done: Promise.resolve(), cancel: () => undefined }) }),
      destroy: () => Promise.resolve(),
    };
    vi.mocked(openPdf).mockImplementation(() => Promise.resolve(fake));
    const pdf = "a0000000-0000-4000-8000-0000000000f1";
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Read:" }] },
        { type: "file", attrs: { id: pid(1), attachmentId: pdf, view: "preview" } },
      ],
    };
    const { user } = await renderInterop({
      spaces: [{ name: "Personal", pages: [{ title: "Notes", content }] }],
      attachments: [{ attachmentId: pdf, name: "report.pdf", mime: "application/pdf", bytes: testPdf({ pages: 1 }) }],
    });
    const editor = await openPage(user, "Notes");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-preview")).not.toBeNull();
    });

    ctrlP();
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
    });
    const card = editor.view.dom.querySelector(".file-card");
    expect(card).toHaveTextContent("report.pdf");
    afterPrint();
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-preview")).not.toBeNull();
    });
  });
});

describe("diagrams and whiteboards", () => {
  const mermaidPage: JsonValue = {
    type: "doc",
    content: [
      { type: "codeBlock", attrs: { id: pid(0), language: "mermaid" }, content: [{ type: "text", text: "graph TD; A-->B" }] },
      { type: "paragraph", attrs: { id: pid(1) } },
    ],
  };

  it("are drawn again in the light look, and the page waits for them before it prints", async () => {
    document.documentElement.dataset.theme = "dark";
    const draws: { theme: DiagramTheme; finish: () => void }[] = [];
    let hold = false;
    setDiagramRenderer(
      (_source, theme) =>
        new Promise((resolve) => {
          const svg = `<svg xmlns="http://www.w3.org/2000/svg" data-theme-drawn="${theme}"></svg>`;
          draws.push({ theme, finish: () => resolve(svg) });
          if (!hold) {
            resolve(svg);
          }
        }),
    );
    const { user } = await renderInterop({ spaces: [{ name: "Personal", pages: [{ title: "Chart", content: mermaidPage }] }] });
    const editor = await openPage(user, "Chart");
    const preview = (): Element | null => editor.view.dom.querySelector(".code-block-preview");
    await vi.waitFor(() => {
      expect(preview()?.querySelector("svg")).toHaveAttribute("data-theme-drawn", "dark");
    });
    expect(draws.every((draw) => draw.theme === "dark")).toBe(true);
    expect(preview()).not.toHaveAttribute("data-busy");

    hold = true;
    ctrlP();
    await vi.waitFor(() => {
      expect(draws.some((draw) => draw.theme === "light")).toBe(true);
    });
    // The dark picture stays on show, but it is not ready: the print waits for the light one.
    expect(preview()?.querySelector("svg")).toHaveAttribute("data-theme-drawn", "dark");
    expect(preview()).toHaveAttribute("data-busy");
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(print).not.toHaveBeenCalled();

    await vi.waitFor(() => {
      for (const draw of draws) {
        draw.finish();
      }
      expect(print).toHaveBeenCalledTimes(1);
    });
    expect(preview()?.querySelector("svg")).toHaveAttribute("data-theme-drawn", "light");
    expect(preview()).not.toHaveAttribute("data-busy");
    afterPrint();
  });

  function fakeEngine(exports: { finish: () => void; dark: boolean }[]): ExcalidrawEngine {
    const engine: Pick<ExcalidrawEngine, "exportToSvg" | "restoreElements"> = {
      exportToSvg: (options) =>
        new Promise((resolve) => {
          exports.push({
            dark: options.appState.exportWithDarkMode,
            finish: () => {
              resolve(document.createElementNS("http://www.w3.org/2000/svg", "svg"));
            },
          });
        }),
      restoreElements: (elements) => elements,
    };
    return engine as ExcalidrawEngine;
  }

  it("mark a whiteboard drawn again for a new theme as busy until it is", async () => {
    const exports: { finish: () => void; dark: boolean }[] = [];
    setExcalidrawEngine(fakeEngine(exports));
    document.documentElement.dataset.theme = "dark";
    const elements = [{ id: "a", type: "rectangle", x: 0, y: 0, width: 10, height: 10 }];
    const { container } = render(<WhiteboardPreview elements={elements} height={200} />);
    const stage = (): Element | null => container.querySelector(".whiteboard-stage");

    await vi.waitFor(() => {
      expect(exports).toHaveLength(1);
    });
    expect(exports[0]?.dark).toBe(true);
    expect(stage()).toHaveAttribute("data-state", "loading");
    await act(async () => {
      exports[0]?.finish();
      await Promise.resolve();
    });
    await vi.waitFor(() => {
      expect(stage()).toHaveAttribute("data-state", "drawn");
    });
    expect(stage()).not.toHaveAttribute("data-busy");

    // The theme changes (print mode puts the light one on): the dark picture stays while the light one is made.
    act(() => {
      document.documentElement.dataset.theme = "light";
    });
    await vi.waitFor(() => {
      expect(exports).toHaveLength(2);
    });
    expect(exports[1]?.dark).toBe(false);
    expect(stage()).toHaveAttribute("data-state", "drawn");
    expect(stage()).toHaveAttribute("data-busy");
    expect(isPagePending(container)).toBe(true);

    await act(async () => {
      exports[1]?.finish();
      await Promise.resolve();
    });
    await vi.waitFor(() => {
      expect(stage()).not.toHaveAttribute("data-busy");
    });
    expect(isPagePending(container)).toBe(false);
  });
});

describe("the print stylesheet", () => {
  it("has a print block that hides the app around the page", () => {
    const css = readFileSync("src/styles/app.css", "utf8");
    const block = css.slice(css.lastIndexOf("@media print"));
    for (const selector of [".sidebar", ".tab-strip", ".backlinks", ".find-bar", ".page-header-actions", ".page-path", ".toasts"]) {
      expect(block, selector).toContain(selector);
    }
    expect(block).toContain("break-inside: avoid");
    expect(block).toContain("overflow: visible");
    // The properties print as values, and no selection or focus decoration prints.
    for (const selector of [".file-chip-actions", ".icon-button", ".ProseMirror-selectednode", ".is-block-selected", ":focus-visible"]) {
      expect(block, selector).toContain(selector);
    }
  });
});
