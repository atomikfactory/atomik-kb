// Regular expressions (section 3.4.1): the subset JavaScript and Rust's `regex` share, with JavaScript's meaning (the
// corpus follows it): `.` stops at the four line terminators, `\d \w \b` are ASCII, `[]` is empty and `[^]` is anything,
// matches are found as `matchAll` finds them. The pattern is checked and rewritten before `new RegExp(p, "u")` sees it:
// what is outside the subset is refused (look-around, back-references, named groups, flags, escapes of letters and
// digits other than the classes), `\s` is the White_Space of Unicode (as in Rust), the escape of punctuation that is not
// a regex character (`\#`, `\-` outside a class: Rust accepts them, the `u` mode does not) is written without its
// backslash, and a lone `]` or `}` is a literal (the `u` mode refuses them). `$0..$9` and `$$` in a replacement are
// expanded here, not by JavaScript, whose rules differ.

import { MAX_FORMULA_LIST_ITEMS } from "./limits";

/** What may follow a backslash in the `u` mode of JavaScript as a plain regex character. */
const SYNTAX_CHARACTERS = "^$\\.*+?()[]{}|/";

/** The characters `\s` stands for (Unicode White_Space), as the inside of a class. */
const WHITE_SPACE = "\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";

function isAsciiPunctuation(c: string): boolean {
  return c.length === 1 && c >= "!" && c <= "~" && !/[0-9A-Za-z]/u.test(c);
}

/** The pattern as the `u` mode of JavaScript reads it with the meaning of the shared subset; `null` when it is not in it. */
export function normalizePattern(pattern: string): string | null {
  let out = "";
  let inClass = false;
  for (let i = 0; i < pattern.length; i += 1) {
    const c = pattern.charAt(i);
    if (c === "\\") {
      const next = pattern.charAt(i + 1);
      if (next === "") {
        return null;
      }
      i += 1;
      if ("dDwWnrtfvb".includes(next)) {
        out += `\\${next}`;
      } else if (next === "B") {
        if (inClass) {
          return null;
        }
        out += "\\B";
      } else if (next === "s") {
        out += inClass ? WHITE_SPACE : `[${WHITE_SPACE}]`;
      } else if (next === "S") {
        out += inClass ? "\\S" : `[^${WHITE_SPACE}]`;
      } else if (isAsciiPunctuation(next)) {
        out += SYNTAX_CHARACTERS.includes(next) || (inClass && next === "-") ? `\\${next}` : next;
      } else {
        return null;
      }
      continue;
    }
    if (inClass) {
      if (c === "]") {
        inClass = false;
      }
      out += c;
      continue;
    }
    switch (c) {
      case "[":
        inClass = true;
        out += c;
        break;
      case "(":
        if (pattern.charAt(i + 1) === "?") {
          if (pattern.charAt(i + 2) !== ":") {
            return null;
          }
          out += "(?:";
          i += 2;
        } else {
          out += c;
        }
        break;
      case "{": {
        const quantifier = /^\{\d+(?:,\d*)?\}/u.exec(pattern.slice(i));
        if (quantifier === null) {
          out += c;
        } else {
          out += quantifier[0];
          i += quantifier[0].length - 1;
        }
        break;
      }
      case "}":
      case "]":
        out += `\\${c}`;
        break;
      default:
        out += c;
    }
  }
  return inClass ? null : out;
}

/** The pattern compiled with the `u` flag (and `g`, for iteration), or `null` when it is not accepted or does not compile. */
export function compilePattern(pattern: string): RegExp | null {
  const normalized = normalizePattern(pattern);
  if (normalized === null) {
    return null;
  }
  try {
    return new RegExp(normalized, "gu");
  } catch {
    return null;
  }
}

/** The width of the character at `index` in UTF-16 units (a code point), at least 1. */
function step(text: string, index: number): number {
  const c = text.codePointAt(index);
  return c !== undefined && c > 0xffff ? 2 : 1;
}

/**
 * Every non-overlapping match, left to right (at most `limit`), as JavaScript's `matchAll` yields them: after an empty
 * match the search goes on one code point further, and an empty match right after another match counts.
 */
export function allMatches(re: RegExp, text: string, limit = MAX_FORMULA_LIST_ITEMS + 1): RegExpExecArray[] {
  const found: RegExpExecArray[] = [];
  let from = 0;
  while (from <= text.length && found.length < limit) {
    re.lastIndex = from;
    const m = re.exec(text);
    if (m === null) {
      break;
    }
    found.push(m);
    const end = m.index + m[0].length;
    from = m[0].length === 0 ? end + step(text, end) : end;
  }
  return found;
}

/** The first match, if any. */
export function firstMatch(re: RegExp, text: string): RegExpExecArray | null {
  re.lastIndex = 0;
  return re.exec(text);
}

/** The replacement with `$0`..`$9` (an unmatched group is `""`) and `$$` expanded; any other `$` is literal. */
export function expandReplacement(replacement: string, m: RegExpExecArray): string {
  let out = "";
  for (let i = 0; i < replacement.length; i += 1) {
    const c = replacement.charAt(i);
    if (c === "$") {
      const next = replacement.charAt(i + 1);
      if (next === "$") {
        out += "$";
        i += 1;
        continue;
      }
      if (next >= "0" && next <= "9") {
        out += m[Number(next)] ?? "";
        i += 1;
        continue;
      }
    }
    out += c;
  }
  return out;
}

/** `text` with the given matches replaced. */
export function replaceMatches(text: string, matches: readonly RegExpExecArray[], replacement: string): string {
  let out = "";
  let last = 0;
  for (const m of matches) {
    out += text.slice(last, m.index) + expandReplacement(replacement, m);
    last = m.index + m[0].length;
  }
  return out + text.slice(last);
}
