// The differences between a version of a page and the page now (ADR-0057, `docs/architecture/page-history.md` section 8):
// blocks aligned by id (recursively), text blocks diffed by words, answered as one merged document whose parts are marked.
//
// **Direction.** The version is the base and the page now is what it is compared with, from the point of view of a
// restore: a part only in the version is `added` (restoring puts it back), a part only on the page now is `removed`
// (restoring takes it away). The merged document is the version's blocks in the version's order with the page's own
// parts inserted where they stand.
//
// **Alignment.** At every level of the tree (the page, a list, a toggle, a callout, a column, a table, a row, a template
// button) the children are aligned by block id (`attrs.id`): the longest common subsequence of the two id sequences, which
// is exact and `O(n log n)` because an id is unique in a sequence (a repeated one is numbered, so that it stays unique).
// A block in both sequences but out of the common order (it moved) is a removed block where it stands now and an added
// block where the version had it. What the ids leave unmatched between two aligned blocks is paired a second time, but only
// blocks that lack an id on one side or both (a page written before ids, a block the core made): first the ones with the
// same content, then the ones of the same type in order. A block with an id on both sides and different ids never pairs.
// Two aligned blocks that hold blocks are merged by the same rules, level by level; two that hold text are merged by words.
//
// **Words.** The plain text of the two blocks is cut into words, spaces, single punctuation marks and (for scripts written
// without spaces) single characters; a line break and an inline atom (a page reference, a date, an equation, a person) are one
// word of their own. The longest common subsequence of the words is kept, the rest is a removed run (the page's, with the
// page's marks) and an added run (the version's, with the version's marks), the removed one first; a run of spaces that
// sits between two changes joins them, so that "a b c" against "x y z" reads as one replacement. Marks are ignored by the
// diff (the version's marks stay on the version's words), so a block whose words are the same but whose marks differ is
// `changed` with no marked runs.
//
// **Limits.** Words and unmatched id-less blocks are compared with a table of at most `LCS_CELL_LIMIT` cells (after the
// common start and end are cut off, which is most of the work of a typical edit): beyond it the whole middle is a
// replacement (a paragraph of tens of thousands of words, or hundreds of unmatched blocks without ids). Block ids need no
// table, so a page of thousands of blocks costs a pass over its nodes.
//
// **The merged document.** Every block (every node that is not inline) carries `attrs.historyDiff`: `same`, `added`,
// `removed` or `changed` (a block of both whose own attributes or type differ, or whose words or marks differ). A block that
// is wholly added or removed has the status on itself and on every block inside it, and its text carries no run marks (the
// status says it all); the summary counts it once. Inline runs inside a `changed` block carry a `historyDiff` mark
// (`attrs.status`: `added` or `removed`).
// Not compared: the id (it aligns), whether a toggle is open (a view state), and the drawing of a whiteboard (it is not in the
// page's nodes but in the document's root map).

/** A portable (ProseMirror JSON) node, as `PageContentGet` and `PageHistoryGet` answer it. */
export interface PortableNode {
  readonly type: string;
  readonly attrs?: Readonly<Record<string, unknown>>;
  readonly content?: readonly PortableNode[];
  readonly text?: string;
  readonly marks?: readonly { readonly type: string; readonly attrs?: Readonly<Record<string, unknown>> }[];
}

/** How a part of the merged document differs from the current page. */
export type DiffStatus = "same" | "added" | "removed" | "changed";

/** The merged document: the version's blocks with the current page's own blocks and text runs inserted, every part marked. */
export interface PageDiff {
  /** A portable document whose nodes carry `attrs.historyDiff` (a `DiffStatus`) and whose text runs carry a `historyDiff` mark. */
  readonly doc: PortableNode;
  /** Blocks only in the version, only on the page now, and in both but different. */
  readonly summary: { readonly added: number; readonly removed: number; readonly changed: number };
}

/** The attribute of a block of the merged document that holds its `DiffStatus`. */
export const HISTORY_DIFF_ATTRIBUTE = "historyDiff";

/** The mark of an inline run of the merged document (`attrs.status`: `added` or `removed`). */
export const HISTORY_DIFF_MARK = "historyDiff";

/**
 * The most cells of a table a longest-common-subsequence search fills (16-bit counts: 8 MB). A search over more is not made:
 * the part that differs is shown as replaced.
 */
export const LCS_CELL_LIMIT = 4_000_000;

/** What is inline in a text block: text, a line break and the atoms that sit in text. */
const INLINE_TYPES: ReadonlySet<string> = new Set(["text", "hardBreak", "pageRef", "dateMention", "personMention", "mathInline"]);

/** The nodes that hold inline content (an empty one has no content to tell it by). */
const TEXT_BLOCK_TYPES: ReadonlySet<string> = new Set(["paragraph", "heading", "codeBlock", "mathBlock", "detailsSummary"]);

/** The text blocks in which every space counts (code and LaTeX): the others compare a run of spaces as one. */
const VERBATIM_TYPES: ReadonlySet<string> = new Set(["codeBlock", "mathBlock"]);

/** The most cells the pairing of unmatched blocks without ids may visit. */
const PAIRING_CELL_LIMIT = 250_000;

type Counts = { added: number; removed: number; changed: number };
type Pair = readonly [number, number];
type Attrs = Readonly<Record<string, unknown>>;

// ----- reading nodes -----------------------------------------------------------------------------------------------

function idOf(node: PortableNode): string | null {
  const id = node.attrs?.id;
  return typeof id === "string" && id !== "" ? id : null;
}

/** Whether the node holds inline content (text and atoms), not blocks. */
function hasInlineContent(node: PortableNode): boolean {
  if (node.type === "text" || TEXT_BLOCK_TYPES.has(node.type)) {
    return true;
  }
  const first = node.content?.[0];
  return first !== undefined && INLINE_TYPES.has(first.type);
}

/** A value as a string that is equal for equal values: keys sorted, `null` and absent alike. */
function stable(value: unknown): string {
  if (value === null || value === undefined) {
    return "";
  }
  if (Array.isArray(value)) {
    return `[${value.map(stable).join(",")}]`;
  }
  if (typeof value === "object") {
    const record = value as Record<string, unknown>;
    return `{${Object.keys(record)
      .sort()
      .filter((key) => record[key] !== null && record[key] !== undefined)
      .map((key) => `${key}:${stable(record[key])}`)
      .join(",")}}`;
  }
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  return typeof value === "number" || typeof value === "boolean" || typeof value === "bigint" ? String(value) : "";
}

/** Attributes that say nothing about the content: the id, the diff's own, whether a toggle is open. */
function ignoredAttribute(name: string): boolean {
  return name === "id" || name === HISTORY_DIFF_ATTRIBUTE || name === "open";
}

/** The attributes of a node that count, as one string. */
function attrsSignature(attrs: Attrs | undefined): string {
  if (attrs === undefined) {
    return "";
  }
  return Object.keys(attrs)
    .filter((name) => !ignoredAttribute(name) && attrs[name] !== null && attrs[name] !== undefined)
    .sort()
    .map((name) => `${name}=${stable(attrs[name])}`)
    .join(";");
}

function marksSignature(marks: PortableNode["marks"]): string {
  return (marks ?? []).map((mark) => `${mark.type}(${attrsSignature(mark.attrs)})`).join("+");
}

/** A node and everything under it as one string, ids left out: equal strings are the same content. */
function signature(node: PortableNode): string {
  if (node.type === "text") {
    return `T${marksSignature(node.marks)}:${JSON.stringify(node.text ?? "")}`;
  }
  const inner = (node.content ?? []).map(signature).join("");
  return `<${node.type}${marksSignature(node.marks)} ${attrsSignature(node.attrs)}>${inner}</>`;
}

function signatureOf(nodes: readonly PortableNode[] | undefined): string {
  return (nodes ?? []).map(signature).join("");
}

/** Whether two nodes differ in their own type or attributes (not in what they hold). */
function ownDiffers(a: PortableNode, b: PortableNode): boolean {
  return a.type !== b.type || attrsSignature(a.attrs) !== attrsSignature(b.attrs);
}

function withStatus(attrs: Attrs | undefined, status: DiffStatus): Attrs {
  return { ...attrs, [HISTORY_DIFF_ATTRIBUTE]: status };
}

/** The node and every block under it marked with `status` (its text is left as it is). */
function stamp(node: PortableNode, status: DiffStatus): PortableNode {
  const inner = node.content;
  const content = inner === undefined || hasInlineContent(node) ? inner : inner.map((child) => stamp(child, status));
  return { ...node, attrs: withStatus(node.attrs, status), ...(content === undefined ? {} : { content }) };
}

// ----- sequences ---------------------------------------------------------------------------------------------------

/**
 * The pairs `[i, j]` of a longest common subsequence of `a` and `b` (equal strings), in order. The common start and end are
 * paired without a table; when the table for what lies between them would pass `limit` cells, that middle is not searched
 * (no pair in it), so the answer is a common subsequence that is not the longest.
 */
function commonSubsequence(a: readonly string[], b: readonly string[], limit = LCS_CELL_LIMIT): Pair[] {
  const shortest = Math.min(a.length, b.length);
  let start = 0;
  while (start < shortest && a[start] === b[start]) {
    start += 1;
  }
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const rows = endA - start;
  const columns = endB - start;
  const middle: Pair[] = [];
  const width = columns + 1;
  if (rows > 0 && columns > 0 && (rows + 1) * width <= limit) {
    // `table[i * width + j]` is the length of a longest common subsequence of a[start + i ..] and b[start + j ..]; it is at
    // most the shorter side, which the limit keeps far below 65536.
    const table = new Uint16Array((rows + 1) * width);
    const cell = (i: number, j: number): number => table[i * width + j] ?? 0;
    for (let i = rows - 1; i >= 0; i -= 1) {
      for (let j = columns - 1; j >= 0; j -= 1) {
        table[i * width + j] =
          a[start + i] === b[start + j] ? cell(i + 1, j + 1) + 1 : Math.max(cell(i + 1, j), cell(i, j + 1));
      }
    }
    let i = 0;
    let j = 0;
    while (i < rows && j < columns) {
      if (a[start + i] === b[start + j]) {
        middle.push([start + i, start + j]);
        i += 1;
        j += 1;
      } else if (cell(i + 1, j) >= cell(i, j + 1)) {
        i += 1;
      } else {
        j += 1;
      }
    }
  }
  const pairs: Pair[] = [];
  for (let i = 0; i < start; i += 1) {
    pairs.push([i, i]);
  }
  pairs.push(...middle);
  for (let k = 0; k < a.length - endA; k += 1) {
    pairs.push([endA + k, endB + k]);
  }
  return pairs;
}

/** The indices of a longest strictly increasing subsequence of `values` (patience sorting, `O(n log n)`). */
function increasingSubsequence(values: readonly number[]): number[] {
  const tails: number[] = []; // tails[k]: the index of the smallest last value of an increasing run of length k + 1
  const previous: number[] = [];
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index] ?? 0;
    let low = 0;
    let high = tails.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if ((values[tails[middle] ?? 0] ?? 0) < value) {
        low = middle + 1;
      } else {
        high = middle;
      }
    }
    previous[index] = low > 0 ? (tails[low - 1] ?? -1) : -1;
    tails[low] = index;
  }
  const result: number[] = [];
  let at = tails.length > 0 ? (tails[tails.length - 1] ?? -1) : -1;
  while (at >= 0) {
    result.push(at);
    at = previous[at] ?? -1;
  }
  return result.reverse();
}

// ----- blocks ------------------------------------------------------------------------------------------------------

type Op =
  | { readonly kind: "pair"; readonly a: PortableNode; readonly b: PortableNode }
  | { readonly kind: "added"; readonly a: PortableNode }
  | { readonly kind: "removed"; readonly b: PortableNode };

/** The id of every node of a sequence, a repeated one numbered so that it stays unique; `null` for a node without one. */
function keysOf(nodes: readonly PortableNode[]): (string | null)[] {
  const seen = new Map<string, number>();
  return nodes.map((node) => {
    const id = idOf(node);
    if (id === null) {
      return null;
    }
    const count = seen.get(id) ?? 0;
    seen.set(id, count + 1);
    return count === 0 ? id : `${id}#${String(count)}`;
  });
}

/** Whether two nodes that lack an id on one side or both may pair (the same type, and no two different ids). */
function mayPair(a: PortableNode, b: PortableNode): boolean {
  return a.type === b.type && (idOf(a) === null || idOf(b) === null);
}

/** What lies between two aligned blocks, the page's first and then the version's: removed, then added. */
function leave(a: readonly PortableNode[], b: readonly PortableNode[], ops: Op[]): void {
  for (const node of b) {
    ops.push({ kind: "removed", b: node });
  }
  for (const node of a) {
    ops.push({ kind: "added", a: node });
  }
}

/** The pairs of unmatched blocks of the same type in order (`a` against `b`), with a bounded look ahead. */
function pairByPosition(a: readonly PortableNode[], b: readonly PortableNode[], ops: Op[]): void {
  if (a.length * b.length > PAIRING_CELL_LIMIT) {
    leave(a, b, ops);
    return;
  }
  let nextB = 0;
  let waitingA: PortableNode[] = [];
  let waitingB: PortableNode[] = [];
  for (const node of a) {
    let found = -1;
    for (let j = nextB; j < b.length; j += 1) {
      const candidate = b[j];
      if (candidate !== undefined && mayPair(node, candidate)) {
        found = j;
        break;
      }
    }
    if (found < 0) {
      waitingA.push(node);
      continue;
    }
    waitingB = waitingB.concat(b.slice(nextB, found));
    leave(waitingA, waitingB, ops);
    waitingA = [];
    waitingB = [];
    const partner = b[found];
    if (partner !== undefined) {
      ops.push({ kind: "pair", a: node, b: partner });
    }
    nextB = found + 1;
  }
  leave(waitingA, waitingB.concat(b.slice(nextB)), ops);
}

/** A second try at what the ids left unmatched between two aligned blocks: blocks without an id pair by content, then by type. */
function resolveGap(a: readonly PortableNode[], b: readonly PortableNode[], ops: Op[]): void {
  const withoutId = (nodes: readonly PortableNode[]): boolean => nodes.some((node) => idOf(node) === null);
  if (a.length === 0 || b.length === 0 || (!withoutId(a) && !withoutId(b))) {
    leave(a, b, ops);
    return;
  }
  const exact = commonSubsequence(a.map(signature), b.map(signature), PAIRING_CELL_LIMIT).filter(([i, j]) => {
    const left = a[i];
    const right = b[j];
    return left !== undefined && right !== undefined && mayPair(left, right);
  });
  let atA = 0;
  let atB = 0;
  for (const [i, j] of exact) {
    pairByPosition(a.slice(atA, i), b.slice(atB, j), ops);
    const left = a[i];
    const right = b[j];
    if (left !== undefined && right !== undefined) {
      ops.push({ kind: "pair", a: left, b: right });
    }
    atA = i + 1;
    atB = j + 1;
  }
  pairByPosition(a.slice(atA), b.slice(atB), ops);
}

/** The blocks of the version and of the page, aligned: the merged sequence of pairs, added and removed blocks. */
function align(a: readonly PortableNode[], b: readonly PortableNode[]): Op[] {
  const aKeys = keysOf(a);
  const bPosition = new Map<string, number>();
  keysOf(b).forEach((key, index) => {
    if (key !== null) {
      bPosition.set(key, index);
    }
  });
  const common: { a: number; b: number }[] = [];
  aKeys.forEach((key, index) => {
    const position = key === null ? undefined : bPosition.get(key);
    if (position !== undefined) {
      common.push({ a: index, b: position });
    }
  });
  const anchors = increasingSubsequence(common.map((entry) => entry.b)).map((index) => common[index]);
  const ops: Op[] = [];
  let atA = 0;
  let atB = 0;
  for (const anchor of anchors) {
    if (anchor === undefined) {
      continue;
    }
    resolveGap(a.slice(atA, anchor.a), b.slice(atB, anchor.b), ops);
    const left = a[anchor.a];
    const right = b[anchor.b];
    if (left !== undefined && right !== undefined) {
      ops.push({ kind: "pair", a: left, b: right });
    }
    atA = anchor.a + 1;
    atB = anchor.b + 1;
  }
  resolveGap(a.slice(atA), b.slice(atB), ops);
  return ops;
}

/** The children of two aligned blocks (or the page), merged. */
function diffChildren(a: readonly PortableNode[], b: readonly PortableNode[], counts: Counts): PortableNode[] {
  const merged: PortableNode[] = [];
  for (const op of align(a, b)) {
    switch (op.kind) {
      case "added":
        counts.added += 1;
        merged.push(stamp(op.a, "added"));
        break;
      case "removed":
        counts.removed += 1;
        merged.push(stamp(op.b, "removed"));
        break;
      case "pair":
        merged.push(...diffPair(op.a, op.b, counts));
        break;
    }
  }
  return merged;
}

/** Two aligned blocks, merged: one block, or (when they cannot be merged) the page's removed and the version's added. */
function diffPair(a: PortableNode, b: PortableNode, counts: Counts): PortableNode[] {
  const aInline = hasInlineContent(a);
  const bInline = hasInlineContent(b);
  const sameType = a.type === b.type;
  const bothHoldBlocks = !aInline && !bInline && (a.content?.length ?? 0) > 0 && (b.content?.length ?? 0) > 0;
  if (aInline !== bInline || (!sameType && !aInline && !bothHoldBlocks)) {
    counts.removed += 1;
    counts.added += 1;
    return [stamp(b, "removed"), stamp(a, "added")];
  }
  let content: readonly PortableNode[] | undefined;
  let changed = ownDiffers(a, b);
  if (aInline) {
    const text = diffInline(a.content, b.content, VERBATIM_TYPES.has(a.type));
    content = text.content;
    changed = changed || text.changed;
  } else {
    const children = diffChildren(a.content ?? [], b.content ?? [], counts);
    content = children.length > 0 ? children : undefined;
  }
  if (changed) {
    counts.changed += 1;
  }
  return [{ type: a.type, attrs: withStatus(a.attrs, changed ? "changed" : "same"), ...(content === undefined ? {} : { content }) }];
}

// ----- words -------------------------------------------------------------------------------------------------------

/**
 * The pieces of a text: spaces, a letter-and-digit word (an apostrophe inside it stays), one character of a script written
 * without spaces, or one other character.
 */
const WORDS =
  /\s+|[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Thai}]|[\p{L}\p{M}\p{N}_]+(?:['’][\p{L}\p{M}\p{N}_]+)*|[^\s\p{L}\p{M}\p{N}_]/gu;

/** One word of a text block: what is compared (`key`) and what is shown (`node`, a slice of a text node or an atom). */
interface Token {
  readonly key: string;
  readonly node: PortableNode;
  readonly space: boolean;
}

function tokensOf(content: readonly PortableNode[] | undefined, verbatim: boolean): Token[] {
  const tokens: Token[] = [];
  for (const node of content ?? []) {
    if (node.type !== "text") {
      tokens.push({ key: `\u0000${node.type}:${attrsSignature(node.attrs)}`, node, space: false });
      continue;
    }
    const marks = node.marks === undefined || node.marks.length === 0 ? {} : { marks: node.marks };
    for (const match of (node.text ?? "").matchAll(WORDS)) {
      const text = match[0];
      const space = /^\s/u.test(text);
      tokens.push({ key: space && !verbatim ? " " : text, node: { type: "text", text, ...marks }, space });
    }
  }
  return tokens;
}

type Piece =
  | { readonly equal: true; readonly a: Token; readonly b: Token }
  | { readonly equal: false; readonly removed: readonly Token[]; readonly added: readonly Token[] };

/** A run of the merged text: the token's node with a `historyDiff` mark of `status` after its own marks. */
function run(token: Token, status: "added" | "removed"): PortableNode {
  const marks = [...(token.node.marks ?? []), { type: HISTORY_DIFF_MARK, attrs: { status } }];
  return { ...token.node, marks };
}

/** The words of two text blocks pieced together: equal words and the changes between them. */
function piecesOf(a: readonly Token[], b: readonly Token[]): Piece[] {
  const pairs = commonSubsequence(
    a.map((token) => token.key),
    b.map((token) => token.key),
  );
  const pieces: Piece[] = [];
  let atA = 0;
  let atB = 0;
  for (const [i, j] of pairs) {
    if (i > atA || j > atB) {
      pieces.push({ equal: false, removed: b.slice(atB, j), added: a.slice(atA, i) });
    }
    const left = a[i];
    const right = b[j];
    if (left !== undefined && right !== undefined) {
      pieces.push({ equal: true, a: left, b: right });
    }
    atA = i + 1;
    atB = j + 1;
  }
  if (atA < a.length || atB < b.length) {
    pieces.push({ equal: false, removed: b.slice(atB), added: a.slice(atA) });
  }
  return pieces;
}

/** A space that two changes surround is part of the change: "a b c" against "x y z" is one replacement, not three. */
function absorbSpaces(pieces: readonly Piece[]): Piece[] {
  const result: Piece[] = [];
  pieces.forEach((piece, index) => {
    const before = pieces[index - 1];
    const after = pieces[index + 1];
    const surrounded = before !== undefined && after !== undefined && !before.equal && !after.equal;
    const merged: Piece = piece.equal && piece.a.space && surrounded ? { equal: false, removed: [piece.b], added: [piece.a] } : piece;
    const last = result[result.length - 1];
    if (last !== undefined && !last.equal && !merged.equal) {
      result[result.length - 1] = { equal: false, removed: [...last.removed, ...merged.removed], added: [...last.added, ...merged.added] };
    } else {
      result.push(merged);
    }
  });
  return result;
}

/** Text nodes with the same marks, next to each other, become one. */
function coalesce(nodes: readonly PortableNode[]): PortableNode[] {
  const result: PortableNode[] = [];
  for (const node of nodes) {
    const last = result[result.length - 1];
    if (last !== undefined && last.type === "text" && node.type === "text" && marksSignature(last.marks) === marksSignature(node.marks)) {
      result[result.length - 1] = { ...last, text: `${last.text ?? ""}${node.text ?? ""}` };
    } else {
      result.push(node);
    }
  }
  return result;
}

/**
 * The inline content of two text blocks merged: the version's content where nothing differs; else the version's words with
 * the page's own removed runs and the version's added runs marked. `changed` is also true when only the marks differ.
 */
function diffInline(
  a: readonly PortableNode[] | undefined,
  b: readonly PortableNode[] | undefined,
  verbatim: boolean,
): { content: readonly PortableNode[] | undefined; changed: boolean } {
  if (signatureOf(a) === signatureOf(b)) {
    return { content: a, changed: false };
  }
  const found = piecesOf(tokensOf(a, verbatim), tokensOf(b, verbatim));
  const pieces = verbatim ? found : absorbSpaces(found);
  if (pieces.every((piece) => piece.equal)) {
    // The words are the same: only a difference of marks counts (a different run of spaces is no change).
    const marked = pieces.some((piece) => piece.equal && marksSignature(piece.a.node.marks) !== marksSignature(piece.b.node.marks));
    return { content: a, changed: marked };
  }
  const nodes: PortableNode[] = [];
  for (const piece of pieces) {
    if (piece.equal) {
      nodes.push(piece.a.node);
    } else {
      nodes.push(...piece.removed.map((token) => run(token, "removed")), ...piece.added.map((token) => run(token, "added")));
    }
  }
  return { content: coalesce(nodes), changed: true };
}

// ----- the page ----------------------------------------------------------------------------------------------------

/** The differences between `version` and `current` (both `{ type: "doc", content }`). */
export function diffPages(version: PortableNode, current: PortableNode): PageDiff {
  const counts: Counts = { added: 0, removed: 0, changed: 0 };
  const content = diffChildren(version.content ?? [], current.content ?? [], counts);
  const doc: PortableNode = {
    type: version.type,
    ...(version.attrs === undefined ? {} : { attrs: version.attrs }),
    ...(content.length === 0 ? {} : { content }),
  };
  return { doc, summary: counts };
}
