// Offline LaTeX typesetting for the math block (math-block.ts) and the inline equation
// (math-inline.ts, MathInlineView.tsx).
//
// KaTeX is loaded with a dynamic `import()` the first time an equation is drawn, together
// with its stylesheet: the bundler makes the library and the stylesheet lazily loaded
// chunks of the app (never part of the main bundle) and emits the stylesheet's fonts as
// local assets, so nothing is fetched from anywhere else at runtime (the desktop CSP allows
// only local resources, `font-src 'self'` included). The source is never trusted:
// `trust: false` keeps raw HTML, links and other URLs out of the output, and a source that
// does not parse throws (KaTeX never draws its own error). This module typesets; it never
// touches the document, so nothing about an equation is stored beyond the block's text.

import type { KatexOptions } from "katex";
import type Katex from "katex";

/** How an equation is typeset: on a line of its own (the block, the default) or inside a line of text. */
export interface MathRenderOptions {
  readonly inline?: boolean;
}

/** Typesets `latex` as markup for `innerHTML`, or throws the reason it cannot. */
export type MathRenderer = (latex: string, options?: MathRenderOptions) => string;

type KatexApi = typeof Katex;

/** An equation on a line of its own (the block), without KaTeX's own error markup. */
const OPTIONS: KatexOptions = {
  displayMode: true,
  throwOnError: true,
  output: "htmlAndMathml",
  trust: false,
  strict: "ignore",
};

/** An equation inside a line of text (the inline equation): the same, without display mode. */
const INLINE_OPTIONS: KatexOptions = { ...OPTIONS, displayMode: false };

/** A KaTeX message quotes the source around the error: the start tells enough. */
const MAX_ERROR_LENGTH = 300;

let override: MathRenderer | null = null;

/** A test seam: typesets with `renderer` (`null` brings KaTeX back), so tests never load it. */
export function setMathRenderer(renderer: MathRenderer | null): void {
  override = renderer;
}

let katex: KatexApi | null = null;
let loading: Promise<void> | null = null;

/** Whether `renderMath` works now: KaTeX is loaded, or a test's renderer is set. */
export function mathIsLoaded(): boolean {
  return override !== null || katex !== null;
}

/**
 * Loads KaTeX and its stylesheet once; `renderMath` works after this resolves. A failed
 * load (a missing chunk) is tried again by the next equation. With a renderer set by
 * `setMathRenderer` there is nothing to load.
 */
export function loadMath(): Promise<void> {
  if (override !== null || katex !== null) {
    return Promise.resolve();
  }
  loading ??= Promise.all([import("katex"), import("katex/dist/katex.min.css")]).then(
    ([module]) => {
      katex = module.default;
    },
    (error: unknown) => {
      loading = null;
      throw error;
    },
  );
  return loading;
}

/**
 * Typesets `latex` (display mode, or inline with `{ inline: true }`) as the markup KaTeX
 * produces, which is meant for `innerHTML` and nothing else: HTML and MathML, no scripts, no
 * links. Throws KaTeX's own `ParseError` for a source it cannot parse, and an `Error` when
 * `loadMath()` has not resolved yet.
 */
export function renderMath(latex: string, options: MathRenderOptions = {}): string {
  if (override !== null) {
    return override(latex, options);
  }
  if (katex === null) {
    throw new Error("KaTeX is not loaded");
  }
  return katex.renderToString(latex, options.inline === true ? INLINE_OPTIONS : OPTIONS);
}

/** `renderMath` for an equation inside a line of text (not display mode). */
export function renderMathInline(latex: string): string {
  return renderMath(latex, { inline: true });
}

/** KaTeX's reason for an equation it cannot typeset, trimmed to a short message. */
export function describeMathError(error: unknown): string {
  const raw =
    typeof error === "object" && error !== null && "message" in error && typeof error.message === "string"
      ? error.message
      : String(error);
  const message = raw.trim();
  return message.length > MAX_ERROR_LENGTH ? `${message.slice(0, MAX_ERROR_LENGTH).trimEnd()}...` : message;
}
