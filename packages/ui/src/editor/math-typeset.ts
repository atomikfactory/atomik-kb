// What an inline equation shows for its source (`MathInlineView.tsx`, `MathInlineEditor.tsx`): nothing to
// draw, drawing, drawn, or KaTeX's message. The typesetting is `renderMathInline` (`./math`), which needs
// KaTeX loaded: the first equation of a session starts the load and shows its source meanwhile, the next
// ones are drawn at once.

import { useEffect, useMemo, useState } from "react";

import { describeMathError, loadMath, mathIsLoaded, renderMathInline } from "./math";

/** The state of one equation's drawing. */
export type Typeset =
  | { readonly state: "empty" }
  | { readonly state: "loading" }
  | { readonly state: "ready"; readonly markup: string }
  | { readonly state: "error"; readonly message: string };

/**
 * The typeset form of `latex` (inline mode), kept up to date as the source changes. The markup is
 * KaTeX's own output (`trust: false`: no raw HTML, no links), meant for `innerHTML` and nothing else.
 * A source that is blank draws nothing (`empty`); one that does not parse gives KaTeX's message.
 */
export function useTypeset(latex: string): Typeset {
  const [loaded, setLoaded] = useState(mathIsLoaded);
  const [loadFailure, setLoadFailure] = useState<string | null>(null);
  const blank = latex.trim() === "";

  useEffect(() => {
    if (loaded || blank) {
      return;
    }
    let current = true;
    loadMath().then(
      () => {
        if (current) {
          setLoaded(true);
        }
      },
      (error: unknown) => {
        if (current) {
          setLoadFailure(describeMathError(error));
        }
      },
    );
    return () => {
      current = false;
    };
  }, [loaded, blank]);

  return useMemo((): Typeset => {
    if (blank) {
      return { state: "empty" };
    }
    if (loadFailure !== null) {
      return { state: "error", message: loadFailure };
    }
    if (!loaded) {
      return { state: "loading" };
    }
    try {
      return { state: "ready", markup: renderMathInline(latex) };
    } catch (error) {
      return { state: "error", message: describeMathError(error) };
    }
  }, [latex, blank, loaded, loadFailure]);
}
