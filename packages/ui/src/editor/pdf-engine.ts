// The PDF engine of the inline viewer (ADR-0031): pdf.js, loaded with a dynamic `import()` the first
// time a PDF preview is shown, so it is never part of the main bundle (as Mermaid in mermaid.ts and
// KaTeX in math.ts). The bundler makes the library and its worker lazily loaded files of the app and
// the worker is a local asset (`?url`): nothing is fetched from anywhere else, and the desktop CSP
// stays as it is (`script-src 'self'` covers the module worker).
//
// A PDF is untrusted input, so the engine is held to the least it needs:
// - the bytes come through a `PDFDataRangeTransport` over a `PdfByteSource` (pdf-source.ts), never
//   a URL: no network request is made by the library;
// - no `cMapUrl`, `standardFontDataUrl`, `iccUrl` or `wasmUrl`: the library fetches none of them
//   (the CSP's `connect-src` has no 'self' either). The price is a degraded rendering of the PDFs
//   that need them: fonts that are not embedded fall back to the system's, a PDF with a
//   non-embedded CJK font that relies on a predefined CMap shows no text for it, ICC colour
//   profiles fall back to their device colour space, and JPEG 2000 and JBIG2 images are skipped;
// - `isEvalSupported: false` (v6 has no eval path left; the option is passed for the day it has),
//   no XFA, no scripting (the library only scripts through an annotation layer, which is not drawn);
// - only a canvas and a text layer are drawn: there is no annotation layer, so a link in a PDF is
//   not a link here (nothing can open a URL, and no form can be filled);
// - no auto-fetch and no streaming: only the byte ranges the pages in view need are asked for, and
//   each is a ranged `attachment_read` (a chunk is decrypted once, see pdf-source.ts).
//
// The views talk to the small `PdfDocument` interface below, not to pdf.js, so that tests replace
// this module and the engine stays swappable.

import type * as Pdfjs from "pdfjs-dist";

import type { PdfByteSource } from "./pdf-source";

/** Why a PDF could not be opened. */
export type PdfFailure =
  /** It is password protected (no password is asked for). */
  | "encrypted"
  /** The bytes are not a PDF, or are too damaged to read. */
  | "invalid"
  /** The bytes could not be read from the attachment. */
  | "unreadable"
  /** Anything else (the library failed to start, ...). */
  | "failed";

export class PdfOpenError extends Error {
  readonly failure: PdfFailure;

  constructor(failure: PdfFailure, message: string) {
    super(message);
    this.name = "PdfOpenError";
    this.failure = failure;
  }
}

/** Where and how one page is drawn. */
export interface PdfRenderTarget {
  /** The canvas that gets the page; its size is set by the render. */
  canvas: HTMLCanvasElement;
  /** Receives the selectable text (transparent spans); `null` draws no text layer. */
  textLayer: HTMLElement | null;
  /** CSS pixels per PDF unit (1 PDF unit is 1/72 inch, so 1.333 is "100 %" on a 96 dpi screen). */
  scale: number;
  /** Device pixels per CSS pixel. */
  pixelRatio: number;
}

export interface PdfRenderJob {
  /** Settles when the page is drawn; rejects with the reason it could not be (a cancelled job rejects too). */
  done: Promise<void>;
  /** Stops the job; its canvas and text layer must then be discarded. */
  cancel: () => void;
}

export interface PdfPageView {
  /** The page's size at scale 1, after its rotation. */
  readonly width: number;
  readonly height: number;
  render: (target: PdfRenderTarget) => PdfRenderJob;
}

export interface PdfDocument {
  readonly pages: number;
  /** Page `number` (1-based). */
  page: (number: number) => Promise<PdfPageView>;
  /** Frees the document and stops its worker. */
  destroy: () => Promise<void>;
}

export interface OpenPdfOptions {
  /**
   * Called once when a read of the bytes fails after the document was opened (a page then never
   * draws): the viewer says so instead of waiting for ever.
   */
  onReadError?: (error: unknown) => void;
}

/** Opens a PDF; rejects with a {@link PdfOpenError}. */
export type PdfOpener = (source: PdfByteSource, options?: OpenPdfOptions) => Promise<PdfDocument>;

type PdfjsApi = typeof Pdfjs;

/** The most pixels a page's canvas may have (a page at high zoom must not take the webview's memory). */
export const MAX_CANVAS_PIXELS = 16_000_000;

/** The size of a range request of the library, and of the first piece handed to it. */
const RANGE_CHUNK = 256 * 1024;

/** Images above this many pixels are not decoded (a decompression bomb draws nothing). */
const MAX_IMAGE_PIXELS = 100_000_000;

let loading: Promise<PdfjsApi> | null = null;

/** The library, loaded once; `workerSrc` is set to the bundled worker. */
function loadLibrary(): Promise<PdfjsApi> {
  loading ??= Promise.all([
    import("pdfjs-dist/legacy/build/pdf.mjs"),
    import("pdfjs-dist/legacy/build/pdf.worker.min.mjs?url"),
  ]).then(
    ([library, worker]) => {
      library.GlobalWorkerOptions.workerSrc = worker.default;
      return library;
    },
    (error: unknown) => {
      loading = null; // the next preview tries again
      throw new PdfOpenError("failed", messageOf(error));
    },
  );
  return loading;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The failure a pdf.js error stands for. */
function failureOf(error: unknown): PdfFailure {
  switch (error instanceof Error ? error.name : "") {
    case "PasswordException":
      return "encrypted";
    case "InvalidPDFException":
    case "FormatError":
    case "MissingPDFException":
      return "invalid";
    default:
      return "failed";
  }
}

/** A canvas scale that keeps the page's canvas within {@link MAX_CANVAS_PIXELS}. */
export function limitedPixelRatio(width: number, height: number, pixelRatio: number): number {
  const wanted = Math.max(1, pixelRatio);
  const area = width * height * wanted * wanted;
  return area <= MAX_CANVAS_PIXELS ? wanted : Math.max(0.1, Math.sqrt(MAX_CANVAS_PIXELS / (width * height)));
}

/** The settings of the library for an untrusted file read through `range`; see the header. */
function documentParameters(library: PdfjsApi, range: Pdfjs.PDFDataRangeTransport) {
  return {
    range,
    rangeChunkSize: RANGE_CHUNK,
    disableAutoFetch: true,
    disableStream: true,
    isEvalSupported: false as const,
    enableXfa: false,
    useWorkerFetch: false,
    useSystemFonts: true,
    stopAtErrors: false,
    maxImageSize: MAX_IMAGE_PIXELS,
    verbosity: library.VerbosityLevel.ERRORS,
  };
}

/** Wraps a pdf.js page in the interface of the views. */
function pageView(library: PdfjsApi, page: Pdfjs.PDFPageProxy): PdfPageView {
  const natural = page.getViewport({ scale: 1 });
  return {
    width: natural.width,
    height: natural.height,
    render({ canvas, textLayer, scale, pixelRatio }) {
      const viewport = page.getViewport({ scale });
      const ratio = limitedPixelRatio(viewport.width, viewport.height, pixelRatio);
      canvas.width = Math.max(1, Math.floor(viewport.width * ratio));
      canvas.height = Math.max(1, Math.floor(viewport.height * ratio));
      canvas.style.width = `${String(Math.floor(viewport.width))}px`;
      canvas.style.height = `${String(Math.floor(viewport.height))}px`;
      const task = page.render({
        canvas,
        viewport,
        ...(ratio === 1 ? {} : { transform: [ratio, 0, 0, ratio, 0, 0] }),
      });
      let layer: Pdfjs.TextLayer | null = null;
      let text: Promise<unknown> = Promise.resolve();
      if (textLayer !== null) {
        layer = new library.TextLayer({ textContentSource: page.streamTextContent(), container: textLayer, viewport });
        text = layer.render();
      }
      return {
        done: Promise.all([task.promise, text]).then(() => undefined),
        cancel: () => {
          task.cancel();
          layer?.cancel();
        },
      };
    },
  };
}

/** Opens `source` with an already loaded library (the engine's tests call this with the real one). */
export async function openWithLibrary(
  library: PdfjsApi,
  source: PdfByteSource,
  options: OpenPdfOptions = {},
): Promise<PdfDocument> {
  if (source.size <= 0) {
    throw new PdfOpenError("invalid", "the file is empty");
  }
  let opened = false;
  let readFailure: unknown;
  let rejectOpening: (error: PdfOpenError) => void = () => undefined;
  const failedOpening = new Promise<never>((_, reject: (error: PdfOpenError) => void) => {
    rejectOpening = reject;
  });

  let first: Uint8Array;
  try {
    first = await source.read(0, Math.min(source.size, RANGE_CHUNK));
  } catch (error) {
    throw new PdfOpenError("unreadable", messageOf(error));
  }

  const range = new library.PDFDataRangeTransport(source.size, first);
  range.requestDataRange = (begin, end) => {
    source.read(begin, end).then(
      (chunk) => {
        range.onDataRange(begin, chunk);
      },
      (error: unknown) => {
        readFailure ??= error;
        if (opened) {
          options.onReadError?.(error);
        } else {
          rejectOpening(new PdfOpenError("unreadable", messageOf(error)));
        }
      },
    );
  };

  const task = library.getDocument(documentParameters(library, range));
  let document: Pdfjs.PDFDocumentProxy;
  try {
    document = await Promise.race([task.promise, failedOpening]);
  } catch (error) {
    void task.destroy();
    if (error instanceof PdfOpenError) {
      throw error;
    }
    throw new PdfOpenError(readFailure === undefined ? failureOf(error) : "unreadable", messageOf(error));
  }
  opened = true;
  if (readFailure !== undefined) {
    options.onReadError?.(readFailure);
  }
  return {
    pages: document.numPages,
    async page(number) {
      return pageView(library, await document.getPage(number));
    },
    async destroy() {
      await task.destroy();
    },
  };
}

/** Opens a PDF with pdf.js, loading the library first. */
export const openPdf: PdfOpener = async (source, options) => {
  const library = await loadLibrary();
  return openWithLibrary(library, source, options);
};
