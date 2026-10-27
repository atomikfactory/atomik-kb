// The tokenizer of the editor (section 7.2): colours and completion. It covers all the text but white space, never throws,
// and counts positions in code points.

import { scan } from "./lexer";
import { isFunctionName } from "./names";
import type { Tok } from "./lexer";
import type { FormulaToken } from "./types";

function publicKind(t: Tok): FormulaToken["kind"] {
  switch (t.kind) {
    case "num":
      return "number";
    case "str":
      return "string";
    case "kw":
      return "keyword";
    case "op":
      return "operator";
    case "punct":
      return "punct";
    case "comment":
      return "comment";
    case "ident":
      return "ident";
    default:
      return "error";
  }
}

/** Splits any text into tokens covering it entirely, white space excepted; never throws. */
export function tokenize(text: string): FormulaToken[] {
  const toks = scan(Array.from(text));
  // The tokens that matter for the shape of calls: comments are skipped when looking for a `(`.
  const significant = toks.filter((t) => t.kind !== "comment");
  const kinds = new Map<Tok, FormulaToken["kind"]>();
  significant.forEach((t, i) => {
    const next = significant[i + 1];
    if (t.kind === "ident" && next?.kind === "punct" && next.text === "(" && isFunctionName(t.text)) {
      kinds.set(t, "function");
    }
    if (t.kind === "str") {
      const open = significant[i - 1];
      const callee = significant[i - 2];
      if (open?.kind === "punct" && open.text === "(" && callee?.kind === "ident" && callee.text === "prop") {
        kinds.set(t, "property");
      }
    }
  });
  return toks.map((t) => ({ kind: kinds.get(t) ?? publicKind(t), start: t.start, end: t.end, text: t.text }));
}
