// Names and ids (section 6): the four rewrites of the `prop("…")` and `.prop("…")` calls of a text. They work on tokens,
// so they apply to the calls they can see in a text that does not fully parse, and keep everything else exactly as written.

import { makeError } from "./errors";
import { scan } from "./lexer";
import { FORMULA_TITLE_ID } from "./limits";
import type { FormulaError, FormulaSchema } from "./types";

/** The text argument of a `prop(…)` call: its decoded content and its range (the quotes included). */
interface PropRef {
  readonly name: string;
  readonly start: number;
  readonly end: number;
}

/** A rewritten argument: where it was (`from`) and where it is now (`to`), both as code point ranges. */
export interface Edit {
  readonly from: { readonly start: number; readonly end: number };
  readonly to: { readonly start: number; readonly end: number };
}

/** Every `prop ( "text" )` of the text (a call may follow a dot: `x.prop("…")`), comments and spaces skipped. */
function propRefs(chars: readonly string[]): PropRef[] {
  const tokens = scan(chars).filter((t) => t.kind !== "comment");
  const refs: PropRef[] = [];
  for (let i = 0; i + 3 < tokens.length; i += 1) {
    const [name, open, argument, close] = [tokens[i], tokens[i + 1], tokens[i + 2], tokens[i + 3]];
    if (
      name?.kind === "ident" &&
      name.text === "prop" &&
      open?.kind === "punct" &&
      open.text === "(" &&
      argument?.kind === "str" &&
      argument.err === undefined &&
      typeof argument.value === "string" &&
      close?.kind === "punct" &&
      close.text === ")"
    ) {
      refs.push({ name: argument.value, start: argument.start, end: argument.end });
    }
  }
  return refs;
}

function quote(text: string): string {
  return `"${text.replace(/\\/gu, "\\\\").replace(/"/gu, '\\"')}"`;
}

/** Rewrites the text argument of every `prop` call for which `replace` answers a text. */
function rewrite(text: string, replace: (ref: PropRef) => string | undefined): { text: string; edits: Edit[] } {
  const chars = Array.from(text);
  const edits: Edit[] = [];
  let out = "";
  let outLength = 0;
  let position = 0;
  for (const ref of propRefs(chars)) {
    const next = replace(ref);
    if (next === undefined) {
      continue;
    }
    const before = chars.slice(position, ref.start).join("");
    const literal = quote(next);
    outLength += ref.start - position;
    edits.push({
      from: { start: ref.start, end: ref.end },
      to: { start: outLength, end: outLength + Array.from(literal).length },
    });
    outLength += Array.from(literal).length;
    out += before + literal;
    position = ref.end;
  }
  return { text: out + chars.slice(position).join(""), edits };
}

function storedRewrite(
  display: string,
  schema: FormulaSchema,
  titleLabel: string,
): { text: string; edits: Edit[] } | FormulaError {
  let error: FormulaError | undefined;
  const properties = schema.properties();
  const result = rewrite(display, (ref) => {
    if (error !== undefined) {
      return undefined;
    }
    const found = properties.find((p) => p.name === ref.name);
    if (found !== undefined) {
      return found.id;
    }
    if (ref.name === FORMULA_TITLE_ID || ref.name === titleLabel) {
      return FORMULA_TITLE_ID;
    }
    error = makeError("unknown_property", `no property named "${ref.name}"`, ref.start, ref.end, ref.name);
    return undefined;
  });
  return error ?? result;
}

/**
 * Display to stored: names to ids. A name is looked up among the schema's properties in schema order (the first wins),
 * then the title's names (`title` and `titleLabel`); a name that matches nothing is `unknown_property` at its argument.
 */
export function toStored(display: string, schema: FormulaSchema, titleLabel: string): string | FormulaError {
  const result = storedRewrite(display, schema, titleLabel);
  return "code" in result ? result : result.text;
}

/** `toStored` and a function that maps a position of the stored text back to the display text. */
export function toStoredMapped(
  display: string,
  schema: FormulaSchema,
  titleLabel: string,
): { stored: string; toDisplay(position: number): number } | FormulaError {
  const result = storedRewrite(display, schema, titleLabel);
  if ("code" in result) {
    return result;
  }
  const { edits } = result;
  return {
    stored: result.text,
    toDisplay(position: number): number {
      let shift = 0;
      for (const edit of edits) {
        if (position <= edit.to.start) {
          break;
        }
        if (position < edit.to.end) {
          // Inside a rewritten argument: the nearest edge of the argument as the person wrote it.
          return position - edit.to.start < edit.to.end - position ? edit.from.start : edit.from.end;
        }
        shift += edit.to.end - edit.to.start - (edit.from.end - edit.from.start);
      }
      return position - shift;
    },
  };
}

/**
 * Stored to display: ids to names; an id with no property stays as it is. The title (`title`) shows as `titleLabel` when
 * the caller gives one and no property of the schema has that name (so that the text converts back to the same id).
 */
export function toDisplay(stored: string, schema: FormulaSchema, titleLabel?: string): string {
  const properties = schema.properties();
  const labelFree = titleLabel !== undefined && !properties.some((p) => p.name === titleLabel);
  return rewrite(stored, (ref) => {
    const found = schema.property(ref.name);
    if (found !== undefined) {
      return found.name;
    }
    return ref.name === FORMULA_TITLE_ID && labelFree ? titleLabel : undefined;
  }).text;
}

/** Rewrites the ids of the `prop` calls through `map` (an id it does not map stays). */
export function remapIds(stored: string, map: (id: string) => string | undefined): string {
  return rewrite(stored, (ref) => map(ref.name)).text;
}

/** The ids the `prop` calls name, in order, without repeats. */
export function referencedIds(stored: string): string[] {
  const seen = new Set<string>();
  for (const ref of propRefs(Array.from(stored))) {
    seen.add(ref.name);
  }
  return [...seen];
}
