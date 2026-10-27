// The PDF preview of a file block (ADR-0031, ADR-0016 amendment of 2026-10-09): the pure rules, the
// viewer (pages drawn lazily, page n / N, zoom, fit width, full screen), the bytes read in ranges
// through the store, the card and the toggle, errors, read-only editors, and `/pdf`. Whole-app tests
// over the in-memory core with pdf.js replaced by a fake engine (pdf-engine.test.ts runs the real
// one).

import type { JsonValue, MemoryTransportOptions } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { MIB, textFile } from "../test/attachments";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { testPdf } from "../test/pdf";
import { isPdfFile, isPdfMime, parseFileView } from "./attachment-nodes";
import type * as Engine from "./pdf-engine";
import { openPdf, PdfOpenError } from "./pdf-engine";
import type { PdfDocument, PdfPageView, PdfRenderTarget } from "./pdf-engine";
import { MAX_SCALE, MIN_SCALE, pdfStopsEvent } from "./PdfViewer";

vi.mock("./pdf-engine", async (importOriginal) => ({
  ...(await importOriginal<typeof Engine>()),
  openPdf: vi.fn(),
}));

const PDF = "a0000000-0000-4000-8000-0000000000f1";
const BIG = "a0000000-0000-4000-8000-0000000000f2";
const TEXT = "a0000000-0000-4000-8000-0000000000f3";
const GHOST = "a0000000-0000-4000-8000-0000000000fe";

const PDF_BYTES = testPdf({ pages: 2 });
const BIG_SIZE = 40 * MIB;
const BIG_MARK = 36 * MIB + 3;

function seed(): MemoryTransportOptions["attachments"] {
  const big = new Uint8Array(BIG_SIZE);
  big[BIG_MARK] = 7;
  return [
    { attachmentId: PDF, name: "report.pdf", mime: "application/pdf", bytes: PDF_BYTES },
    { attachmentId: BIG, name: "book.pdf", mime: "application/pdf", bytes: big },
    { attachmentId: TEXT, name: "notes.txt", mime: "text/plain", bytes: new TextEncoder().encode("hello") },
  ];
}

interface Draw {
  number: number;
  target: PdfRenderTarget;
  cancel: ReturnType<typeof vi.fn>;
}

/** A document of `pages` pages of 200 x 300 that records what is drawn. */
function fakeDocument(pages = 5) {
  const draws: Draw[] = [];
  const destroy = vi.fn(() => Promise.resolve());
  const doc: PdfDocument = {
    pages,
    page: vi.fn((number: number) => {
      const view: PdfPageView = {
        width: 200,
        height: 300,
        render: (target) => {
          const cancel = vi.fn();
          draws.push({ number, target, cancel });
          return { done: Promise.resolve(), cancel };
        },
      };
      return Promise.resolve(view);
    }),
    destroy,
  };
  return { doc, draws, destroy };
}

/** The pages drawn and not cancelled since. */
function drawn(draws: Draw[]): number[] {
  return draws.filter((draw) => draw.cancel.mock.calls.length === 0).map((draw) => draw.number);
}

function opens(doc: PdfDocument) {
  vi.mocked(openPdf).mockImplementation(() => Promise.resolve(doc));
}

/** A page with a paragraph and one file block (`pid(1)`). */
function pageWith(attachmentId: string, view?: string): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Read:" }] },
      { type: "file", attrs: { id: pid(1), attachmentId, ...(view === undefined ? {} : { view }) } },
    ],
  };
}

const space = (content: JsonValue) => [{ name: "Personal", pages: [{ title: "Notes", content }] }];

async function openFile(attachmentId: string, view?: string) {
  const app = await renderPages(space(pageWith(attachmentId, view)), { attachments: seed() });
  const editor = await openPage(app.user, "Notes");
  return { ...app, editor };
}

function fileAttrs(editor: Editor): Record<string, unknown> {
  return editor.state.doc.child(1).attrs;
}

function viewer(editor: Editor): HTMLElement {
  const element = editor.view.dom.querySelector(".pdf-preview");
  if (!(element instanceof HTMLElement)) {
    throw new Error("no viewer");
  }
  return element;
}

async function openViewer(attachmentId = PDF, view = "preview") {
  const fake = fakeDocument();
  opens(fake.doc);
  const app = await openFile(attachmentId, view);
  await waitFor(() => {
    expect(app.editor.view.dom.querySelector(".pdf-page")).not.toBeNull();
  });
  return { ...app, ...fake };
}

/** An IntersectionObserver that the test drives. */
class FakeObserver {
  static all: FakeObserver[] = [];
  readonly targets = new Set<Element>();

  constructor(
    readonly callback: IntersectionObserverCallback,
    readonly init: IntersectionObserverInit = {},
  ) {
    FakeObserver.all.push(this);
  }

  observe(target: Element): void {
    this.targets.add(target);
  }

  unobserve(target: Element): void {
    this.targets.delete(target);
  }

  disconnect(): void {
    this.targets.clear();
  }

  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }

  /** Tells every observer with this root margin (`near`) or thresholds (`seen`) about a page. */
  static report(page: number, kind: "near" | "seen", isIntersecting: boolean, ratio = isIntersecting ? 1 : 0): void {
    for (const observer of FakeObserver.all) {
      const isNear = observer.init.rootMargin !== undefined;
      if (isNear !== (kind === "near")) {
        continue;
      }
      for (const target of observer.targets) {
        if (target.getAttribute("data-page") === String(page)) {
          act(() => {
            observer.callback(
              [{ target, isIntersecting, intersectionRatio: ratio } as unknown as IntersectionObserverEntry],
              observer as unknown as IntersectionObserver,
            );
          });
        }
      }
    }
  }
}

beforeEach(() => {
  FakeObserver.all = [];
  vi.mocked(openPdf).mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the rules", () => {
  it("read the view leniently and know a PDF by its type, or by its name when the type is generic", () => {
    expect(["preview", "Preview", "card", "", null, undefined, 1].map(parseFileView)).toEqual(["preview", null, null, null, null, null, null]);
    expect(isPdfMime("application/pdf")).toBe(true);
    expect(isPdfMime("application/x-pdf")).toBe(false);
    expect(isPdfFile("a.pdf", "application/pdf")).toBe(true);
    expect(isPdfFile("scan.PDF", "application/octet-stream")).toBe(true);
    expect(isPdfFile("scan.pdf", "")).toBe(true);
    expect(isPdfFile("scan.pdf", "text/plain")).toBe(false);
    expect(isPdfFile("notes.txt", "application/octet-stream")).toBe(false);
  });

  it("leave every event inside the viewer to it, and the card's to the editor as before", () => {
    const inside = document.createElement("div");
    inside.className = "pdf-preview";
    const text = document.createElement("span");
    inside.append(text);
    for (const type of ["mousedown", "click", "keydown", "wheel", "copy", "dragstart"]) {
      const event = new Event(type);
      Object.defineProperty(event, "target", { value: text });
      expect(pdfStopsEvent(event), type).toBe(true);
    }
    const card = document.createElement("div");
    const cardButton = document.createElement("button");
    card.append(cardButton);
    const stops = (type: string) => {
      const event = new Event(type);
      Object.defineProperty(event, "target", { value: cardButton });
      return pdfStopsEvent(event);
    };
    expect(["mousedown", "dragstart", "drop", "dragover", "copy", "cut", "paste"].map(stops)).toEqual([false, false, false, false, false, false, false]);
    expect(["click", "keydown", "mouseover"].map(stops)).toEqual([true, true, true]);
  });
});

describe("the preview", () => {
  it("shows the pages as boxes of the page's size, with page n / N and the toolbar", async () => {
    const { editor } = await openViewer();
    const element = viewer(editor);
    const pages = element.querySelectorAll<HTMLElement>(".pdf-page");
    expect(pages).toHaveLength(5);
    // No window to fit (jsdom has no layout): 100 %, 200 x 300 PDF units at 96/72.
    expect(pages[0]?.style.width).toBe("266px");
    expect(pages[0]?.style.height).toBe("400px");
    expect(within(element).getByText(t("pdf.position", { index: 1, total: 5 }))).toBeInTheDocument();
    expect(within(element).getByText(t("pdf.zoom", { percent: 100 }))).toBeInTheDocument();
    expect(within(element).getByRole("toolbar", { name: t("pdf.toolbar") })).toBeInTheDocument();
    expect(within(element).getByText("report.pdf")).toBeInTheDocument();
    // The card is gone while the viewer shows.
    expect(editor.view.dom.querySelector(".file-card")).toBeNull();
  });

  it("opens the PDF over ranged reads of the attachment, so a PDF beyond the 32 MiB preview limit opens", async () => {
    const { editor } = await openViewer(BIG);
    expect(viewer(editor)).toBeInTheDocument();
    const [source] = vi.mocked(openPdf).mock.calls[0] ?? [];
    expect(source?.size).toBe(BIG_SIZE);
    // Beyond MAX_PREVIEW_BYTES, across the chunk border at 36 MiB, the end of the file.
    const piece = await source?.read(BIG_MARK - 3, BIG_MARK + 5);
    expect(piece).toEqual(Uint8Array.from([0, 0, 0, 7, 0, 0, 0, 0]));
    expect(await source?.read(BIG_SIZE - 4, BIG_SIZE + 100)).toEqual(new Uint8Array(4));
    expect(screen.queryByText(/too large/u)).toBeNull();
  });

  it("reads the bytes of a small PDF the same way", async () => {
    await openViewer();
    const [source] = vi.mocked(openPdf).mock.calls[0] ?? [];
    expect(source?.size).toBe(PDF_BYTES.length);
    expect(await source?.read(0, 8)).toEqual(PDF_BYTES.slice(0, 8));
    expect(await source?.read(0, PDF_BYTES.length)).toEqual(PDF_BYTES);
  });

  it("draws only the first pages while nothing says where the window is, and not the others", async () => {
    const { draws } = await openViewer();
    await waitFor(() => {
      expect(drawn(draws)).toEqual([1, 2, 3]);
    });
  });

  it("draws the pages near the window and drops the ones that leave it", async () => {
    vi.stubGlobal("IntersectionObserver", FakeObserver);
    const { editor, draws } = await openViewer();
    expect(drawn(draws)).toEqual([]);
    FakeObserver.report(2, "near", true);
    await waitFor(() => {
      expect(drawn(draws)).toEqual([2]);
    });
    const first = draws[0];
    expect(first?.target.canvas).toBeInstanceOf(HTMLCanvasElement);
    expect(first?.target.canvas.isConnected).toBe(true);
    expect(first?.target.textLayer?.classList.contains("pdf-text-layer")).toBe(true);
    FakeObserver.report(4, "near", true);
    await waitFor(() => {
      expect(drawn(draws)).toEqual([2, 4]);
    });
    FakeObserver.report(2, "near", false);
    expect(first?.cancel).toHaveBeenCalledTimes(1);
    expect(first?.target.canvas.isConnected).toBe(false);
    expect(editor.view.dom.querySelectorAll(".pdf-canvas")).toHaveLength(1);
  });

  it("follows the page in the window and goes to the previous and next page", async () => {
    vi.stubGlobal("IntersectionObserver", FakeObserver);
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const { editor, user } = await openViewer();
    const element = viewer(editor);
    const previous = within(element).getByRole("button", { name: t("pdf.previousPage") });
    const next = within(element).getByRole("button", { name: t("pdf.nextPage") });
    expect(previous).toBeDisabled();

    FakeObserver.report(1, "seen", true, 0.3);
    FakeObserver.report(2, "seen", true, 0.7);
    await waitFor(() => {
      expect(within(element).getByText(t("pdf.position", { index: 2, total: 5 }))).toBeInTheDocument();
    });
    FakeObserver.report(2, "seen", false);
    await waitFor(() => {
      expect(within(element).getByText(t("pdf.position", { index: 1, total: 5 }))).toBeInTheDocument();
    });

    await user.click(next);
    expect(scroll.mock.contexts.at(-1)).toBe(element.querySelector('[data-page="2"]'));
    expect(within(element).getByText(t("pdf.position", { index: 2, total: 5 }))).toBeInTheDocument();
    await user.click(previous);
    expect(scroll.mock.contexts.at(-1)).toBe(element.querySelector('[data-page="1"]'));
    expect(previous).toBeDisabled();
    for (let page = 2; page <= 5; page += 1) {
      await user.click(next);
    }
    expect(next).toBeDisabled();
    expect(within(element).getByText(t("pdf.position", { index: 5, total: 5 }))).toBeInTheDocument();
  });

  it("zooms in and out by steps within limits, and fits the width again", async () => {
    const { editor, user, draws } = await openViewer();
    const element = viewer(editor);
    const zoomIn = within(element).getByRole("button", { name: t("pdf.zoomIn") });
    const zoomOut = within(element).getByRole("button", { name: t("pdf.zoomOut") });
    const fit = within(element).getByRole("button", { name: t("pdf.fitWidth") });
    const page = (): HTMLElement => element.querySelector<HTMLElement>('[data-page="1"]') as HTMLElement;
    expect(fit).toHaveAttribute("aria-pressed", "true");

    await user.click(zoomIn);
    expect(within(element).getByText(t("pdf.zoom", { percent: 125 }))).toBeInTheDocument();
    expect(page().style.width).toBe("333px");
    expect(fit).toHaveAttribute("aria-pressed", "false");
    // The pages were drawn again at the new scale.
    await waitFor(() => {
      expect(draws.some((draw) => Math.abs(draw.target.scale - (96 / 72) * 1.25) < 1e-9)).toBe(true);
    });
    await user.click(zoomOut);
    await user.click(zoomOut);
    expect(within(element).getByText(t("pdf.zoom", { percent: 80 }))).toBeInTheDocument();
    await user.click(fit);
    expect(within(element).getByText(t("pdf.zoom", { percent: 100 }))).toBeInTheDocument();
    expect(fit).toHaveAttribute("aria-pressed", "true");

    for (let step = 0; step < 20; step += 1) {
      await user.click(zoomIn);
    }
    expect(zoomIn).toBeDisabled();
    expect(within(element).getByText(t("pdf.zoom", { percent: Math.round((MAX_SCALE / (96 / 72)) * 100) }))).toBeInTheDocument();
    await user.click(fit);
    for (let step = 0; step < 20; step += 1) {
      await user.click(zoomOut);
    }
    expect(zoomOut).toBeDisabled();
    expect(within(element).getByText(t("pdf.zoom", { percent: Math.round((MIN_SCALE / (96 / 72)) * 100) }))).toBeInTheDocument();
  });

  it("fits the width of the window", async () => {
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(readonly callback: ResizeObserverCallback) {}
        observe(target: Element): void {
          act(() => {
            this.callback([{ target, contentRect: { width: 424 } } as unknown as ResizeObserverEntry], this);
          });
        }
        disconnect(): void {}
        unobserve(): void {}
      },
    );
    const { editor } = await openViewer();
    // (424 - 2 x 12) / 200 = 2: 150 % of the 96/72 scale... shown as 150 %.
    await waitFor(() => {
      expect(within(viewer(editor)).getByText(t("pdf.zoom", { percent: 150 }))).toBeInTheDocument();
    });
    expect(editor.view.dom.querySelector<HTMLElement>('[data-page="1"]')?.style.width).toBe("400px");
  });

  it("opens the same document full screen in a dialog, with a viewer of its own, and closes it", async () => {
    const { editor, user, doc } = await openViewer();
    await user.click(within(viewer(editor)).getByRole("button", { name: t("pdf.fullscreen") }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveAccessibleName("report.pdf");
    expect(within(dialog).getAllByText(/./u).length).toBeGreaterThan(0);
    expect(dialog.querySelectorAll(".pdf-page")).toHaveLength(5);
    expect(dialog.querySelector(".pdf-preview")).toHaveClass("is-fullscreen");
    // One document, opened once; the dialog zooms on its own.
    expect(vi.mocked(openPdf)).toHaveBeenCalledTimes(1);
    await user.click(within(dialog).getByRole("button", { name: t("pdf.zoomIn") }));
    expect(within(dialog).getByText(t("pdf.zoom", { percent: 125 }))).toBeInTheDocument();
    expect(within(viewer(editor)).getByText(t("pdf.zoom", { percent: 100 }))).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(doc.destroy).not.toHaveBeenCalled();

    await user.click(within(viewer(editor)).getByRole("button", { name: t("pdf.fullscreen") }));
    const again = await screen.findByRole("dialog");
    await user.click(within(again).getByRole("button", { name: t("pdf.close") }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("destroys the document when the block goes", async () => {
    const { editor, destroy } = await openViewer();
    expect(destroy).not.toHaveBeenCalled();
    act(() => {
      editor.commands.deleteRange({ from: 0, to: editor.state.doc.content.size });
    });
    await waitFor(() => {
      expect(destroy).toHaveBeenCalled();
    });
  });

  it("is a card again with 'Show as card', and a preview again with 'Show preview'", async () => {
    const { editor, user, memory } = await openViewer();
    const menu = async () => {
      await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
      return screen.findByRole("menu");
    };
    await user.click(within(await menu()).getByRole("menuitem", { name: t("attachment.pdf.showCard") }));
    expect(fileAttrs(editor).view).toBeNull();
    expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
    expect(within(editor.view.dom).getByText("report.pdf")).toBeInTheDocument();
    expect(editor.view.dom.querySelector(".file-card")).not.toBeNull();
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      const content = (await contentOf(memory, page("Notes"))) as { content: { type: string; attrs?: Record<string, unknown> }[] };
      expect(content.content.find((node) => node.type === "file")?.attrs).not.toHaveProperty("view");
    });

    await user.click(within(await menu()).getByRole("menuitem", { name: t("attachment.pdf.showPreview") }));
    expect(fileAttrs(editor).view).toBe("preview");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-preview")).not.toBeNull();
    });
    await waitFor(async () => {
      const content = (await contentOf(memory, page("Notes"))) as { content: { type: string; attrs?: Record<string, unknown> }[] };
      expect(content.content.find((node) => node.type === "file")?.attrs?.view).toBe("preview");
    });
  });

  it("keeps showing the card, with a toggle, for a PDF file block without a view", async () => {
    opens(fakeDocument().doc);
    const { editor, user } = await openFile(PDF);
    expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
    expect(editor.view.dom.querySelector(".file-card")).not.toBeNull();
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("attachment.pdf.showPreview") }));
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-preview")).not.toBeNull();
    });
    expect(fileAttrs(editor).view).toBe("preview");
    expect(vi.mocked(openPdf)).toHaveBeenCalledTimes(1);
  });

  it("offers no toggle and no viewer for a file that is not a PDF, even with a view", async () => {
    const { editor, user } = await openFile(TEXT, "preview");
    expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
    expect(within(editor.view.dom).getByText("notes.txt")).toBeInTheDocument();
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.pdf.showPreview") })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.pdf.showCard") })).toBeNull();
    expect(vi.mocked(openPdf)).not.toHaveBeenCalled();
  });

  it("still copies the file link as Markdown from a preview", async () => {
    const { editor, user } = await openViewer();
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });
    await user.click(await screen.findByRole("menuitem", { name: t("attachment.copyMarkdown") }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByDisplayValue(`[report.pdf](attachment://${PDF})`)).toBeInTheDocument();
  });
});

describe("a PDF that cannot be previewed", () => {
  it.each([
    ["encrypted", "pdf.error.encrypted"],
    ["invalid", "pdf.error.invalid"],
  ] as const)("falls back to the card with the reason when it is %s", async (failure, message) => {
    vi.mocked(openPdf).mockRejectedValue(new PdfOpenError(failure, "boom"));
    const { editor } = await openFile(PDF, "preview");
    await waitFor(() => {
      expect(within(editor.view.dom).getByText(t(message))).toBeInTheDocument();
    });
    expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
    expect(within(editor.view.dom).getByText("report.pdf")).toBeInTheDocument();
    // The attribute is not touched: another version may open it.
    expect(fileAttrs(editor).view).toBe("preview");
  });

  it("says why when the bytes cannot be read, and when the library does not start", async () => {
    vi.mocked(openPdf).mockRejectedValue(new PdfOpenError("unreadable", "disk is gone"));
    const first = await openFile(PDF, "preview");
    await waitFor(() => {
      expect(within(first.editor.view.dom).getByText(t("pdf.error.unreadable", { message: "disk is gone" }))).toBeInTheDocument();
    });
  });

  it("goes back to the card when a read fails after the document opened", async () => {
    const fake = fakeDocument();
    vi.mocked(openPdf).mockImplementation((_source, options) => {
      setTimeout(() => {
        options?.onReadError?.(new Error("gone"));
      }, 5);
      return Promise.resolve(fake.doc);
    });
    const { editor } = await openFile(PDF, "preview");
    await waitFor(() => {
      expect(within(editor.view.dom).getByText(t("pdf.error.unreadable", { message: "gone" }))).toBeInTheDocument();
    });
    expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
  });

  it("tries again with 'Show preview' after a failure, the attribute staying as it was", async () => {
    vi.mocked(openPdf).mockRejectedValueOnce(new PdfOpenError("invalid", "bad"));
    const { editor, user } = await openFile(PDF, "preview");
    await waitFor(() => {
      expect(within(editor.view.dom).getByText(t("pdf.error.invalid"))).toBeInTheDocument();
    });
    opens(fakeDocument().doc);
    await user.click(within(editor.view.dom).getByRole("button", { name: t("attachment.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("attachment.pdf.showPreview") }));
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-page")).not.toBeNull();
    });
    expect(fileAttrs(editor).view).toBe("preview");
    expect(vi.mocked(openPdf)).toHaveBeenCalledTimes(2);
  });

  it("does not open the PDF of a file that is gone", async () => {
    const { editor } = await openFile(GHOST, "preview");
    await waitFor(() => {
      expect(within(editor.view.dom).getByText(t("attachment.deleted.file"))).toBeInTheDocument();
    });
    expect(vi.mocked(openPdf)).not.toHaveBeenCalled();
  });
});

describe("a preview in a read-only editor", () => {
  it("shows the viewer and its toolbar, without the toggle or the remove entry", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    opens(fakeDocument().doc);
    const content = pageWith(PDF, "preview") as { content: JsonValue[] };
    const withUnknownBlock: JsonValue = { type: "doc", content: [...content.content, { type: "kanban", attrs: { id: pid(9) } }] };
    const app = await renderPages(space(withUnknownBlock), { attachments: seed() });
    const editor = await openPage(app.user, "Notes");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".pdf-page")).not.toBeNull();
    });
    expect(within(viewer(editor)).getByRole("button", { name: t("pdf.zoomIn") })).toBeInTheDocument();
    await app.user.click(within(viewer(editor)).getByRole("button", { name: t("attachment.actions") }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.pdf.showCard") })).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: t("attachment.remove") })).toBeNull();
    expect(within(menu).getByRole("menuitem", { name: t("attachment.copyMarkdown") })).toBeInTheDocument();
  });
});

describe("/pdf", () => {
  /** Stands in for the webview's file chooser: answers with `files`, or cancels. */
  function chooser(files: File[] | "cancel") {
    const inputs: HTMLInputElement[] = [];
    vi.spyOn(HTMLInputElement.prototype, "click").mockImplementation(function click(this: HTMLInputElement) {
      if (this.type !== "file") {
        return;
      }
      inputs.push(this);
      setTimeout(() => {
        if (files === "cancel") {
          this.dispatchEvent(new Event("cancel"));
        } else {
          Object.defineProperty(this, "files", { configurable: true, value: files });
          this.dispatchEvent(new Event("change"));
        }
      }, 0);
    });
    return inputs;
  }

  async function runSlash() {
    opens(fakeDocument().doc);
    const app = await renderPages(space({ type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] }));
    const editor = await openPage(app.user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
    editor.commands.insertContent("/pdf");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option")[0]?.querySelector(".suggestion-title")?.textContent).toBe("PDF");
    });
    return { ...app, editor, menu };
  }

  const pdfFile = (name = "plan.pdf") => new File([PDF_BYTES], name, { type: "application/pdf" });

  it("is found by pdf and sits before File", async () => {
    const { filterSlashCommands } = await import("./slash-commands");
    expect(filterSlashCommands("pdf")[0]?.id).toBe("pdf");
    expect(filterSlashCommands("pdf").map((command) => command.id)).not.toContain("file");
    expect(filterSlashCommands("file")[0]?.id).toBe("file");
    for (const word of ["document", "read", "acrobat"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("pdf");
    }
  });

  it("asks for a PDF with the chooser, uploads it and inserts a file block that shows as a preview", async () => {
    const inputs = chooser([pdfFile()]);
    const { editor, user, menu, memory } = await runSlash();
    await user.click(within(menu).getByText("PDF"));
    await waitFor(() => {
      expect(editor.state.doc.firstChild?.type.name).toBe("file");
    });
    expect(inputs).toHaveLength(1);
    expect(inputs[0]?.accept).toBe("application/pdf,.pdf");
    expect(inputs[0]?.multiple).toBe(false);
    const block = editor.state.doc.firstChild;
    expect(block?.attrs.view).toBe("preview");
    expect(typeof block?.attrs.attachmentId).toBe("string");
    // The command text is gone, and the preview opens the new attachment.
    expect(editor.state.doc.textContent).not.toContain("/pdf");
    await waitFor(() => {
      expect(vi.mocked(openPdf)).toHaveBeenCalledTimes(1);
    });
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      const content = (await contentOf(memory, page("Notes"))) as { content: { type: string; attrs?: Record<string, unknown> }[] };
      expect(content.content.find((node) => node.type === "file")?.attrs?.view).toBe("preview");
    });
  });

  it("leaves nothing behind when the chooser is cancelled", async () => {
    chooser("cancel");
    const { editor, user, menu } = await runSlash();
    await user.click(within(menu).getByText("PDF"));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    expect(editor.state.doc.childCount).toBe(1);
    expect(editor.state.doc.firstChild?.type.name).toBe("paragraph");
    expect(vi.mocked(openPdf)).not.toHaveBeenCalled();
  });

  it("inserts the usual file card for a file that is not a PDF", async () => {
    chooser([textFile("notes.txt")]);
    const { editor, user, menu } = await runSlash();
    await user.click(within(menu).getByText("PDF"));
    await waitFor(() => {
      expect(editor.state.doc.firstChild?.type.name).toBe("file");
    });
    expect(editor.state.doc.firstChild?.attrs.view).toBeNull();
    expect(editor.view.dom.querySelector(".pdf-preview")).toBeNull();
  });

  it("does not add a preview for a PDF dropped or pasted into the page", async () => {
    const { dropFiles } = await import("../test/attachments");
    opens(fakeDocument().doc);
    const app = await renderPages(space({ type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] }));
    const editor = await openPage(app.user, "Notes");
    dropFiles(editor.view.dom, [pdfFile("dropped.pdf")]);
    await waitFor(() => {
      expect(editor.state.doc.firstChild?.type.name).toBe("file");
    });
    expect(editor.state.doc.firstChild?.attrs.view).toBeNull();
  });
});
