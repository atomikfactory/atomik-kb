// The inline PDF viewer (ADR-0031): what a `file` block with `view: "preview"` shows for a PDF. A
// toolbar (page n / N, previous and next page, zoom out and in, fit width, full screen, and the
// block's menu) over a scrolling column of pages. Only the pages near the window are drawn (a canvas
// and a transparent text layer for selecting and copying), the others are empty boxes of the right
// size, and a page that leaves the window is dropped again, so a long PDF costs what it shows.
//
// The bytes come from `usePdfDocument`: the attachment read in ranges (pdf-source.ts), the engine
// (pdf-engine.ts) loaded on first use. The same opened document backs the inline viewer and the full
// screen one, each with a scroll position and a zoom of its own. Nothing here is written to the page.

import { toCoreError } from "@atomik-kb/core-client";
import type { AttachmentInfo } from "@atomik-kb/core-client";
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode, RefObject } from "react";

import { ChevronIcon, ExpandIcon, FitWidthIcon, MinusIcon, PlusIcon } from "../components/icons";
import { Modal } from "../components/Modal";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { openPdf, PdfOpenError } from "./pdf-engine";
import type { PdfDocument, PdfFailure, PdfPageView } from "./pdf-engine";
import { createPdfSource } from "./pdf-source";

/** CSS pixels per PDF unit at "100 %": a PDF unit is 1/72 inch and a CSS pixel 1/96. */
const CSS_UNITS = 96 / 72;
export const MIN_SCALE = 0.25;
export const MAX_SCALE = 6;
export const ZOOM_STEP = 1.25;
/** Space around the pages, in CSS pixels. */
const PAGE_PADDING = 12;
/** How far from the window a page is drawn: one and a half windows above and below. */
const NEAR_MARGIN = "150% 0px";
/** Where nothing can tell what is near (no IntersectionObserver), this many pages are drawn. */
const PAGES_WITHOUT_OBSERVER = 3;

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

/** The state of the opening of a PDF. */
export type PdfLoad =
  | { status: "loading" }
  | { status: "ready"; doc: PdfDocument }
  | { status: "error"; failure: PdfFailure; message: string };

/** The line that says why a PDF is not shown. */
export function describePdfFailure(failure: PdfFailure, message: string): string {
  switch (failure) {
    case "encrypted":
      return t("pdf.error.encrypted");
    case "invalid":
      return t("pdf.error.invalid");
    case "unreadable":
      return t("pdf.error.unreadable", { message });
    case "failed":
      return t("pdf.error.failed", { message });
  }
}

/**
 * Opens the PDF of `info` (a complete attachment) once per attachment: its bytes are read in ranges
 * (`readAttachmentRange`, each at most `MAX_PREVIEW_BYTES`) as the engine asks for them, the engine
 * is loaded on first use, and the document is destroyed when the view goes. `null` for no `info`.
 */
export function usePdfDocument(info: AttachmentInfo | null): PdfLoad | null {
  const readRange = useAppStore((state) => state.readAttachmentRange);
  const id = info?.attachmentId ?? null;
  const size = info === null ? 0 : Number(info.size);
  const key = id === null ? null : `${id} ${String(size)}`;
  const [loaded, setLoaded] = useState<{ key: string; load: PdfLoad } | null>(null);

  useEffect(() => {
    if (id === null || key === null) {
      return;
    }
    let active = true;
    let opened: PdfDocument | null = null;
    const source = createPdfSource(size, (range) => readRange(id, range));
    openPdf(source, {
      onReadError: (error) => {
        if (active) {
          setLoaded({ key, load: { status: "error", failure: "unreadable", message: toCoreError(error).message } });
        }
      },
    }).then(
      (doc) => {
        if (!active) {
          void doc.destroy();
          return;
        }
        opened = doc;
        setLoaded({ key, load: { status: "ready", doc } });
      },
      (error: unknown) => {
        if (!active) {
          return;
        }
        const failure = error instanceof PdfOpenError ? error.failure : "failed";
        setLoaded({ key, load: { status: "error", failure, message: toCoreError(error).message } });
      },
    );
    return () => {
      active = false;
      if (opened !== null) {
        void opened.destroy();
      }
    };
  }, [id, key, size, readRange]);

  if (key === null) {
    return null;
  }
  return loaded?.key === key ? loaded.load : { status: "loading" };
}

/**
 * The node view's `stopEvent` (what ProseMirror must leave alone): every event inside the viewer is
 * the viewer's own, and the player's of a video or audio block (ADR-0032), (clicks, keys, the wheel, selecting and copying the page text). Elsewhere in the
 * block (the card it falls back to) it is Tiptap's own default for a selectable block, which a
 * custom `stopEvent` replaces: a click that selects the block, drags, drops, copy, cut and paste
 * are the editor's, the rest (the menu button's events included) is not.
 */
export function pdfStopsEvent(event: Event): boolean {
  const target = event.target;
  if (target instanceof Element && target.closest(".pdf-preview, .media-preview") !== null) {
    return true;
  }
  return !(
    event.type.startsWith("drag") ||
    event.type === "drop" ||
    event.type === "mousedown" ||
    event.type === "copy" ||
    event.type === "cut" ||
    event.type === "paste"
  );
}

interface PageSize {
  width: number;
  height: number;
}

interface PdfPageSlotProps {
  doc: PdfDocument;
  number: number;
  scale: number;
  /** The size of the page until its own is known. */
  fallback: PageSize;
  scrollRoot: RefObject<HTMLDivElement | null>;
  /** How much of the page shows in the window, from 0 to 1. */
  onVisible: (number: number, ratio: number) => void;
}

/** One page: a box of its size that draws the page while it is near the window. */
function PdfPageSlot({ doc, number, scale, fallback, scrollRoot, onVisible }: PdfPageSlotProps) {
  const boxRef = useRef<HTMLDivElement>(null);
  const paintRef = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(typeof IntersectionObserver === "undefined" && number <= PAGES_WITHOUT_OBSERVER);
  const [view, setView] = useState<PdfPageView | null>(null);
  const [failedLoading, setFailedLoading] = useState(false);
  const [failedAt, setFailedAt] = useState<number | null>(null);

  useEffect(() => {
    const box = boxRef.current;
    if (box === null || typeof IntersectionObserver === "undefined") {
      return;
    }
    const root = scrollRoot.current;
    const nearby = new IntersectionObserver(
      (entries) => {
        const last = entries.at(-1);
        if (last !== undefined) {
          setNear(last.isIntersecting);
        }
      },
      { root, rootMargin: NEAR_MARGIN },
    );
    const seen = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          onVisible(number, entry.isIntersecting ? entry.intersectionRatio : 0);
        }
      },
      { root, threshold: [0, 0.25, 0.5, 0.75, 1] },
    );
    nearby.observe(box);
    seen.observe(box);
    return () => {
      nearby.disconnect();
      seen.disconnect();
      onVisible(number, 0);
    };
  }, [number, onVisible, scrollRoot]);

  // The page's own size and drawing instructions, once it is near for the first time.
  useEffect(() => {
    if (!near || view !== null) {
      return;
    }
    let active = true;
    doc.page(number).then(
      (loaded) => {
        if (active) {
          setView(loaded);
        }
      },
      () => {
        if (active) {
          setFailedLoading(true);
        }
      },
    );
    return () => {
      active = false;
    };
  }, [near, view, doc, number]);

  // The drawing: made when the page is near, dropped when it leaves or the zoom changes.
  useEffect(() => {
    const host = paintRef.current;
    if (!near || view === null || host === null) {
      return;
    }
    const canvas = document.createElement("canvas");
    canvas.className = "pdf-canvas";
    canvas.setAttribute("aria-hidden", "true");
    const text = document.createElement("div");
    text.className = "pdf-text-layer";
    text.style.setProperty("--total-scale-factor", String(scale));
    host.append(canvas, text);
    let live = true;
    const job = view.render({ canvas, textLayer: text, scale, pixelRatio: window.devicePixelRatio || 1 });
    job.done.catch(() => {
      if (live) {
        setFailedAt(scale);
      }
    });
    return () => {
      live = false;
      job.cancel();
      canvas.width = 0;
      canvas.height = 0;
      canvas.remove();
      text.remove();
    };
  }, [near, view, scale]);

  const size = view ?? fallback;
  const failed = failedLoading || failedAt === scale;
  return (
    <div
      ref={boxRef}
      className="pdf-page"
      role="group"
      aria-label={t("pdf.page", { n: number })}
      data-page={number}
      style={{ width: Math.floor(size.width * scale), height: Math.floor(size.height * scale) }}
    >
      <div ref={paintRef} className="pdf-paint" />
      {failed && (
        <p className="pdf-page-error" role="alert">
          {t("pdf.pageError")}
        </p>
      )}
    </div>
  );
}

export interface PdfReaderProps {
  doc: PdfDocument;
  name: string;
  fullscreen: boolean;
  /** Opens the full screen viewer, or closes it, from the toolbar. */
  onFullscreen: () => void;
  /** More toolbar content at its end (the block's menu button). */
  trailing?: ReactNode;
  /** Called with the reason when the first page cannot be read at all. */
  onFailure?: (message: string) => void;
}

/** The toolbar and the pages of an opened PDF. */
export function PdfReader({ doc, name, fullscreen, onFullscreen, trailing, onFailure }: PdfReaderProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const ratios = useRef(new Map<number, number>());
  const keepScroll = useRef<number | null>(null);
  const [first, setFirst] = useState<PageSize | null>(null);
  const [width, setWidth] = useState(0);
  const [zoom, setZoom] = useState<number | null>(null);
  const [current, setCurrent] = useState(1);

  // The first page's size stands in for every page that has not been drawn yet.
  useEffect(() => {
    let active = true;
    doc.page(1).then(
      (page) => {
        if (active) {
          setFirst({ width: page.width, height: page.height });
        }
      },
      (error: unknown) => {
        if (active) {
          onFailure?.(describePdfFailure("failed", toCoreError(error).message));
        }
      },
    );
    return () => {
      active = false;
    };
  }, [doc, onFailure]);

  useEffect(() => {
    const element = scrollRef.current;
    if (element === null || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const entry = entries.at(-1);
      if (entry !== undefined) {
        setWidth(entry.contentRect.width);
      }
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, []);

  const page = first ?? { width: 612, height: 792 };
  const fitScale = width > 0 ? clampScale((width - 2 * PAGE_PADDING) / page.width) : CSS_UNITS;
  const scale = zoom ?? fitScale;

  // A change of zoom keeps the reader at the same place of the document.
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (element !== null && keepScroll.current !== null) {
      element.scrollTop = keepScroll.current * element.scrollHeight;
    }
    keepScroll.current = null;
  }, [scale]);

  function rememberPlace(): void {
    const element = scrollRef.current;
    keepScroll.current = element === null || element.scrollHeight === 0 ? null : element.scrollTop / element.scrollHeight;
  }

  function zoomTo(next: number | null): void {
    rememberPlace();
    setZoom(next === null ? null : clampScale(next));
  }

  const onVisible = useCallback((number: number, ratio: number) => {
    if (ratio > 0) {
      ratios.current.set(number, ratio);
    } else {
      ratios.current.delete(number);
    }
    let best = 0;
    let bestRatio = 0;
    for (const [candidate, value] of ratios.current) {
      if (value > bestRatio || (value === bestRatio && candidate < best)) {
        best = candidate;
        bestRatio = value;
      }
    }
    if (best !== 0) {
      setCurrent(best);
    }
  }, []);

  function goTo(number: number): void {
    const target = Math.min(doc.pages, Math.max(1, number));
    setCurrent(target);
    scrollRef.current?.querySelector(`[data-page="${String(target)}"]`)?.scrollIntoView({ block: "start" });
  }

  const pages = Array.from({ length: doc.pages }, (_, index) => index + 1);
  const percent = Math.round((scale / CSS_UNITS) * 100);
  return (
    <div className={fullscreen ? "pdf-preview is-fullscreen" : "pdf-preview"}>
      <div className="pdf-toolbar" role="toolbar" aria-label={t("pdf.toolbar")}>
        <span className="pdf-title" title={name}>
          {name}
        </span>
        <span className="pdf-tools">
          <button
            type="button"
            className="icon-button pdf-previous"
            aria-label={t("pdf.previousPage")}
            title={t("pdf.previousPage")}
            disabled={current <= 1}
            onClick={() => {
              goTo(current - 1);
            }}
          >
            <ChevronIcon />
          </button>
          <span className="pdf-position" aria-live="polite">
            {t("pdf.position", { index: current, total: doc.pages })}
          </span>
          <button
            type="button"
            className="icon-button pdf-next"
            aria-label={t("pdf.nextPage")}
            title={t("pdf.nextPage")}
            disabled={current >= doc.pages}
            onClick={() => {
              goTo(current + 1);
            }}
          >
            <ChevronIcon />
          </button>
          <span className="pdf-divider" aria-hidden="true" />
          <button
            type="button"
            className="icon-button"
            aria-label={t("pdf.zoomOut")}
            title={t("pdf.zoomOut")}
            disabled={scale <= MIN_SCALE}
            onClick={() => {
              zoomTo(scale / ZOOM_STEP);
            }}
          >
            <MinusIcon />
          </button>
          <span className="pdf-zoom">{t("pdf.zoom", { percent })}</span>
          <button
            type="button"
            className="icon-button"
            aria-label={t("pdf.zoomIn")}
            title={t("pdf.zoomIn")}
            disabled={scale >= MAX_SCALE}
            onClick={() => {
              zoomTo(scale * ZOOM_STEP);
            }}
          >
            <PlusIcon />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label={t("pdf.fitWidth")}
            title={t("pdf.fitWidth")}
            aria-pressed={zoom === null}
            onClick={() => {
              zoomTo(null);
            }}
          >
            <FitWidthIcon />
          </button>
          <span className="pdf-divider" aria-hidden="true" />
          <button
            type="button"
            className="icon-button"
            aria-label={fullscreen ? t("pdf.close") : t("pdf.fullscreen")}
            title={fullscreen ? t("pdf.close") : t("pdf.fullscreen")}
            onClick={onFullscreen}
          >
            <ExpandIcon />
          </button>
          {trailing}
        </span>
      </div>
      <div ref={scrollRef} className="pdf-scroll" tabIndex={0} aria-label={t("pdf.label", { name })}>
        <div className="pdf-pages" style={{ padding: PAGE_PADDING }}>
          {pages.map((number) => (
            <PdfPageSlot
              key={number}
              doc={doc}
              number={number}
              scale={scale}
              fallback={page}
              scrollRoot={scrollRef}
              onVisible={onVisible}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

export interface PdfPreviewProps {
  info: AttachmentInfo;
  /** The block's menu button, shown in the toolbar. */
  menuButton: ReactNode;
  /** The PDF cannot be shown: the block falls back to its card with this line. */
  onFailure: (message: string) => void;
}

/**
 * The preview of a PDF attachment: opens it, shows {@link PdfReader} (and the full screen one on
 * request), or reports through `onFailure` why it cannot (encrypted, damaged, unreadable).
 */
export function PdfPreview({ info, menuButton, onFailure }: PdfPreviewProps) {
  const load = usePdfDocument(info);
  const [fullscreen, setFullscreen] = useState(false);
  const labelId = useId();
  const opener = useRef<HTMLElement | null>(null);
  const failure = load?.status === "error" ? describePdfFailure(load.failure, load.message) : null;

  useEffect(() => {
    if (failure !== null) {
      onFailure(failure);
    }
  }, [failure, onFailure]);

  if (load === null || load.status === "loading") {
    return (
      <div className="pdf-preview is-loading" role="status">
        <span className="pdf-title" title={info.name}>
          {info.name}
        </span>
        <span className="pdf-message">{t("pdf.loading")}</span>
        {menuButton}
      </div>
    );
  }
  if (load.status === "error") {
    return null;
  }
  return (
    <>
      <PdfReader
        doc={load.doc}
        name={info.name}
        fullscreen={false}
        trailing={menuButton}
        onFailure={onFailure}
        onFullscreen={() => {
          opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
          setFullscreen(true);
        }}
      />
      {fullscreen && (
        <Modal
          labelledBy={labelId}
          onDismiss={() => {
            setFullscreen(false);
          }}
          className="pdf-viewer"
          returnFocus={opener}
        >
          <h2 id={labelId} className="visually-hidden">
            {info.name}
          </h2>
          <PdfReader
            doc={load.doc}
            name={info.name}
            fullscreen
            onFailure={onFailure}
            onFullscreen={() => {
              setFullscreen(false);
            }}
          />
        </Modal>
      )}
    </>
  );
}
