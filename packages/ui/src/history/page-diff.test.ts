// The differences between a version of a page and the page now (`page-diff.ts`, ADR-0057): the alignment of blocks by id at every
// level, the word diff of text blocks, blocks that are not text, pages without ids, and pages of thousands of blocks.

import { describe, expect, it } from "vitest";

import { diffPages } from "./page-diff";
import type { DiffStatus, PortableNode } from "./page-diff";

// ----- documents ---------------------------------------------------------------------------------------------------------

/** A block id: `id(3)` is `00000000-0000-4000-8000-000000000003`. */
function id(index: number): string {
  return `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
}

type Mark = NonNullable<PortableNode["marks"]>[number];

function text(value: string, ...marks: string[]): PortableNode {
  return { type: "text", text: value, ...(marks.length === 0 ? {} : { marks: marks.map((type): Mark => ({ type })) }) };
}

function para(n: number, ...content: (string | PortableNode)[]): PortableNode {
  return {
    type: "paragraph",
    attrs: { id: id(n) },
    ...(content.length === 0 ? {} : { content: content.map((part) => (typeof part === "string" ? text(part) : part)) }),
  };
}

function block(type: string, n: number, content: PortableNode[], attrs: Record<string, unknown> = {}): PortableNode {
  return { type, attrs: { id: id(n), ...attrs }, content };
}

function leaf(type: string, n: number, attrs: Record<string, unknown> = {}): PortableNode {
  return { type, attrs: { id: id(n), ...attrs } };
}

function doc(...content: PortableNode[]): PortableNode {
  return { type: "doc", content };
}

// ----- reading the answer ------------------------------------------------------------------------------------------------

function statusOf(node: PortableNode): DiffStatus | undefined {
  const status = node.attrs?.historyDiff;
  return status === "same" || status === "added" || status === "removed" || status === "changed" ? status : undefined;
}

/** The blocks of the merged document, depth first, as `type:status` (a leading `id` shows it: `paragraph#3:added`). */
function outline(node: PortableNode, depth = 0): string[] {
  const lines: string[] = [];
  for (const child of node.content ?? []) {
    const status = statusOf(child);
    if (status === undefined) {
      continue; // text and inline atoms
    }
    const label = typeof child.attrs?.id === "string" ? `#${String(Number.parseInt(child.attrs.id.slice(-12), 10))}` : "";
    lines.push(`${"  ".repeat(depth)}${child.type}${label}:${status}`);
    lines.push(...outline(child, depth + 1));
  }
  return lines;
}

/** The top-level blocks as `n:status`, in merged order. */
function top(result: PortableNode): string[] {
  return (result.content ?? []).map((child) => {
    const n = typeof child.attrs?.id === "string" ? Number.parseInt(child.attrs.id.slice(-12), 10) : "?";
    return `${String(n)}:${statusOf(child) ?? "-"}`;
  });
}

/** The runs of a text block as `text` (same) or `[+text]` (in the version only) and `[-text]` (on the page now only). */
function runs(node: PortableNode): string[] {
  return (node.content ?? []).map((child) => {
    const mark = child.marks?.find((candidate) => candidate.type === "historyDiff");
    const body = child.type === "text" ? (child.text ?? "") : `<${child.type}>`;
    if (mark === undefined) {
      return body;
    }
    return mark.attrs?.status === "added" ? `[+${body}]` : `[-${body}]`;
  });
}

/** The marks of a text run other than the diff's, by type. */
function markTypes(node: PortableNode): string[] {
  return (node.marks ?? []).filter((mark) => mark.type !== "historyDiff").map((mark) => mark.type);
}

function plain(node: PortableNode): string {
  return (node.content ?? []).map((child) => (child.type === "text" ? (child.text ?? "") : "")).join("");
}

function first(result: PortableNode): PortableNode {
  const block = result.content?.[0];
  if (block === undefined) {
    throw new Error("no block");
  }
  return block;
}

// ----- the same page -----------------------------------------------------------------------------------------------------

describe("diffPages: nothing differs", () => {
  it("marks every block of two equal pages as the same and counts nothing", () => {
    const page = doc(
      para(1, "Hello ", text("world", "bold")),
      block("bulletList", 2, [block("listItem", 3, [para(4, "one")]), block("listItem", 5, [para(6, "two")])]),
      leaf("image", 7, { attachmentId: "a", alt: "cat" }),
    );
    const result = diffPages(page, page);
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 0 });
    expect(outline(result.doc)).toEqual([
      "paragraph#1:same",
      "bulletList#2:same",
      "  listItem#3:same",
      "    paragraph#4:same",
      "  listItem#5:same",
      "    paragraph#6:same",
      "image#7:same",
    ]);
    // The text keeps its runs and its marks, and no run is marked.
    expect(runs(first(result.doc))).toEqual(["Hello ", "world"]);
    expect(first(result.doc).content?.[1]?.marks).toEqual([{ type: "bold" }]);
  });

  it("answers a document of the version's type, and nothing for two empty pages", () => {
    expect(diffPages({ type: "doc" }, { type: "doc" })).toEqual({
      doc: { type: "doc" },
      summary: { added: 0, removed: 0, changed: 0 },
    });
    expect(diffPages(doc(), doc()).doc.content).toBeUndefined();
  });

  it("does not take two blocks with different ids for one block, whatever their text", () => {
    const result = diffPages(doc(para(1, "same")), doc(para(2, "same")));
    expect(top(result.doc)).toEqual(["2:removed", "1:added"]);
    expect(result.summary).toEqual({ added: 1, removed: 1, changed: 0 });
  });
});

// ----- blocks added, removed and moved -------------------------------------------------------------------------------------

describe("diffPages: blocks", () => {
  it("marks a block only in the version as added, in its place", () => {
    const result = diffPages(doc(para(1, "a"), para(2, "b"), para(3, "c")), doc(para(1, "a"), para(3, "c")));
    expect(top(result.doc)).toEqual(["1:same", "2:added", "3:same"]);
    expect(result.summary).toEqual({ added: 1, removed: 0, changed: 0 });
  });

  it("marks a block only on the page now as removed, inserted where it stands", () => {
    const result = diffPages(doc(para(1, "a"), para(3, "c")), doc(para(1, "a"), para(2, "b"), para(3, "c"), para(4, "d")));
    expect(top(result.doc)).toEqual(["1:same", "2:removed", "3:same", "4:removed"]);
    expect(result.summary).toEqual({ added: 0, removed: 2, changed: 0 });
    // The page's own block comes whole, with its text.
    expect(plain(result.doc.content?.[1] ?? { type: "x" })).toBe("b");
  });

  it("shows a replaced block as the page's removed one and then the version's added one", () => {
    const result = diffPages(doc(para(1, "a"), para(2, "new"), para(4, "z")), doc(para(1, "a"), para(3, "old"), para(4, "z")));
    expect(top(result.doc)).toEqual(["1:same", "3:removed", "2:added", "4:same"]);
    expect(result.summary).toEqual({ added: 1, removed: 1, changed: 0 });
  });

  it("shows a block that moved as added where the version has it and removed where the page has it now", () => {
    const result = diffPages(doc(para(3, "c"), para(1, "a"), para(2, "b")), doc(para(1, "a"), para(2, "b"), para(3, "c")));
    expect(top(result.doc)).toEqual(["3:added", "1:same", "2:same", "3:removed"]);
    expect(result.summary).toEqual({ added: 1, removed: 1, changed: 0 });
  });

  it("keeps the longest common order when blocks are swapped", () => {
    const result = diffPages(doc(para(1, "a"), para(2, "b"), para(3, "c"), para(4, "d")), doc(para(1, "a"), para(3, "c"), para(2, "b"), para(4, "d")));
    // Two of the three in the middle keep their order; exactly one block counts as moved.
    expect(result.summary.added).toBe(1);
    expect(result.summary.removed).toBe(1);
    expect(top(result.doc).filter((entry) => entry.endsWith(":same"))).toHaveLength(3);
  });

  it("marks a whole block that is added or removed, and everything inside it, once", () => {
    const list = block("bulletList", 2, [block("listItem", 3, [para(4, "x")]), block("listItem", 5, [para(6, "y")])]);
    const added = diffPages(doc(para(1, "a"), list), doc(para(1, "a")));
    expect(outline(added.doc)).toEqual([
      "paragraph#1:same",
      "bulletList#2:added",
      "  listItem#3:added",
      "    paragraph#4:added",
      "  listItem#5:added",
      "    paragraph#6:added",
    ]);
    expect(added.summary).toEqual({ added: 1, removed: 0, changed: 0 });
    // Its text carries no run marks: the block says it.
    expect(runs(added.doc.content?.[1]?.content?.[0]?.content?.[0] ?? { type: "x" })).toEqual(["x"]);

    const removed = diffPages(doc(para(1, "a")), doc(para(1, "a"), list));
    expect(outline(removed.doc).filter((line) => line.endsWith(":removed"))).toHaveLength(5);
    expect(removed.summary).toEqual({ added: 0, removed: 1, changed: 0 });
  });

  it("does not copy the version's or the page's objects: the inputs stay as they were", () => {
    const version = doc(para(1, "a"), para(2, "b"));
    const current = doc(para(1, "a b"), para(3, "c"));
    const before = JSON.stringify([version, current]);
    diffPages(version, current);
    expect(JSON.stringify([version, current])).toBe(before);
  });

  it("copes with an id that appears twice in a page", () => {
    const twice = doc(para(1, "a"), para(1, "a again"), para(2, "b"));
    const result = diffPages(twice, doc(para(1, "a"), para(2, "b")));
    expect(top(result.doc)).toEqual(["1:same", "1:added", "2:same"]);
    expect(diffPages(twice, twice).summary).toEqual({ added: 0, removed: 0, changed: 0 });
  });
});

// ----- nested blocks -----------------------------------------------------------------------------------------------------

describe("diffPages: blocks inside blocks", () => {
  const nested = (...items: PortableNode[]) => doc(block("bulletList", 1, items));

  it("aligns the items of a list and of a list inside an item", () => {
    const version = nested(
      block("listItem", 2, [para(3, "first"), block("bulletList", 4, [block("listItem", 5, [para(6, "nested")])])]),
      block("listItem", 7, [para(8, "second")]),
    );
    const current = nested(
      block("listItem", 2, [para(3, "first"), block("bulletList", 4, [block("listItem", 5, [para(6, "nested")]), block("listItem", 9, [para(10, "gone")])])]),
      block("listItem", 7, [para(8, "second")]),
      block("listItem", 11, [para(12, "last")]),
    );
    const result = diffPages(version, current);
    expect(outline(result.doc)).toEqual([
      "bulletList#1:same",
      "  listItem#2:same",
      "    paragraph#3:same",
      "    bulletList#4:same",
      "      listItem#5:same",
      "        paragraph#6:same",
      "      listItem#9:removed",
      "        paragraph#10:removed",
      "  listItem#7:same",
      "    paragraph#8:same",
      "  listItem#11:removed",
      "    paragraph#12:removed",
    ]);
    expect(result.summary).toEqual({ added: 0, removed: 2, changed: 0 });
  });

  it("finds a changed paragraph deep in a nested list and leaves the list around it as the same", () => {
    const item = (text: string) => nested(block("listItem", 2, [block("bulletList", 3, [block("listItem", 4, [para(5, text)])])]));
    const result = diffPages(item("deep new"), item("deep old"));
    expect(outline(result.doc).filter((line) => !line.endsWith(":same"))).toEqual(["        paragraph#5:changed"]);
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 1 });
  });

  it("aligns the blocks of a toggle, its summary and its content, and does not count whether it is open", () => {
    const toggle = (open: boolean, summary: string, ...body: PortableNode[]) =>
      doc({
        type: "details",
        attrs: { id: id(1), open },
        content: [
          { type: "detailsSummary", attrs: { id: id(2) }, content: [text(summary)] },
          { type: "detailsContent", attrs: { id: id(3) }, content: body },
        ],
      });
    const closed = diffPages(toggle(false, "Title", para(4, "one")), toggle(true, "Title", para(4, "one")));
    expect(closed.summary).toEqual({ added: 0, removed: 0, changed: 0 });
    expect(closed.doc.content?.[0]?.attrs?.open).toBe(false); // the version's own state

    const result = diffPages(toggle(false, "New title", para(4, "one"), para(5, "two")), toggle(false, "Old title", para(4, "one")));
    expect(outline(result.doc)).toEqual([
      "details#1:same",
      "  detailsSummary#2:changed",
      "  detailsContent#3:same",
      "    paragraph#4:same",
      "    paragraph#5:added",
    ]);
    expect(result.summary).toEqual({ added: 1, removed: 0, changed: 1 });
  });

  it("aligns the blocks of callouts, columns, template buttons and tables", () => {
    const rich = (...extra: PortableNode[]) =>
      doc(
        block("callout", 1, [para(2, "note")], { icon: "💡" }),
        block("columnList", 3, [block("column", 4, [para(5, "left")]), block("column", 6, [para(7, "right")])]),
        block("templateButton", 8, [para(9, "body")], { label: "Add", position: "below" }),
        block("table", 10, [block("tableRow", 11, [block("tableHeader", 12, [para(13, "H")]), block("tableCell", 14, [para(15, "C")])]), ...extra]),
      );
    const row = block("tableRow", 16, [block("tableHeader", 17, [para(18, "x")]), block("tableCell", 19, [para(20, "y")])]);
    const result = diffPages(rich(), rich(row));
    expect(result.summary).toEqual({ added: 0, removed: 1, changed: 0 });
    expect(outline(result.doc).filter((line) => line.includes("removed")).map((line) => line.trim())).toEqual([
      "tableRow#16:removed",
      "tableHeader#17:removed",
      "paragraph#18:removed",
      "tableCell#19:removed",
      "paragraph#20:removed",
    ]);
    const back = diffPages(rich(row), rich());
    expect(back.summary).toEqual({ added: 1, removed: 0, changed: 0 });
  });

  it("finds a text change inside a template button's content and a callout's", () => {
    const page = (inner: string, label: string) =>
      doc(block("callout", 1, [para(2, inner)], { icon: "💡" }), block("templateButton", 3, [para(4, inner)], { label, position: "below" }));
    const result = diffPages(page("new", "Add"), page("old", "Add"));
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 2 });
    expect(outline(result.doc)).toEqual([
      "callout#1:same",
      "  paragraph#2:changed",
      "templateButton#3:same",
      "  paragraph#4:changed",
    ]);
  });

  it("merges a list that turned into another kind of list as the version's, changed, with its items compared", () => {
    const bullet = doc(block("bulletList", 1, [block("listItem", 2, [para(3, "a")])]));
    const ordered = doc(block("orderedList", 1, [block("listItem", 2, [para(3, "a")])]));
    const result = diffPages(bullet, ordered);
    expect(outline(result.doc)).toEqual(["bulletList#1:changed", "  listItem#2:same", "    paragraph#3:same"]);
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 1 });
  });

  it("shows blocks of kinds that cannot be merged as the page's removed and the version's added", () => {
    const result = diffPages(doc(leaf("image", 1, { attachmentId: "a" })), doc(para(1, "was text")));
    expect(top(result.doc)).toEqual(["1:removed", "1:added"]);
    expect(result.summary).toEqual({ added: 1, removed: 1, changed: 0 });
  });
});

// ----- the words of a text block -------------------------------------------------------------------------------------------

describe("diffPages: text", () => {
  it("marks the words only in the version added and the words only on the page removed, the removed first", () => {
    const result = diffPages(doc(para(1, "The quick brown fox")), doc(para(1, "The slow brown fox jumps")));
    const block = first(result.doc);
    expect(runs(block)).toEqual(["The ", "[-slow]", "[+quick]", " brown fox", "[- jumps]"]);
    expect(statusOf(block)).toBe("changed");
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 1 });
  });

  it("marks a word inserted in the middle and one deleted from it", () => {
    expect(runs(first(diffPages(doc(para(1, "one two three")), doc(para(1, "one three"))).doc))).toEqual(["one ", "[+two ]", "three"]);
    expect(runs(first(diffPages(doc(para(1, "one three")), doc(para(1, "one two three"))).doc))).toEqual(["one ", "[-two ]", "three"]);
  });

  it("gives the whole text back when the version's text is read without the runs of the page", () => {
    const result = diffPages(doc(para(1, "alpha beta gamma delta")), doc(para(1, "alpha BETA gamma epsilon zeta")));
    const merged = first(result.doc);
    const versionText = (merged.content ?? [])
      .filter((child) => !child.marks?.some((mark) => mark.type === "historyDiff" && mark.attrs?.status === "removed"))
      .map((child) => child.text ?? "")
      .join("");
    expect(versionText).toBe("alpha beta gamma delta");
    const pageText = (merged.content ?? [])
      .filter((child) => !child.marks?.some((mark) => mark.type === "historyDiff" && mark.attrs?.status === "added"))
      .map((child) => child.text ?? "")
      .join("");
    expect(pageText).toBe("alpha BETA gamma epsilon zeta");
  });

  it("reads a run of words that differ as one replacement, not as words joined by the spaces they share", () => {
    const result = diffPages(doc(para(1, "a b c")), doc(para(1, "x y z")));
    expect(runs(first(result.doc))).toEqual(["[-x y z]", "[+a b c]"]);
  });

  it("keeps the version's marks on its words and the page's marks on the words it removed", () => {
    const version = para(1, "keep ", text("version", "bold"), " tail");
    const current = para(1, "keep ", text("page", "italic"), " tail");
    const merged = first(diffPages(doc(version), doc(current)).doc);
    expect(runs(merged)).toEqual(["keep ", "[-page]", "[+version]", " tail"]);
    expect(markTypes(merged.content?.[1] ?? { type: "x" })).toEqual(["italic"]);
    expect(markTypes(merged.content?.[2] ?? { type: "x" })).toEqual(["bold"]);
    // The words that stay are the version's, marks and all.
    expect(markTypes(merged.content?.[0] ?? { type: "x" })).toEqual([]);
  });

  it("ignores marks in the diff: a word that is bold in one and not in the other is not a change of words", () => {
    const result = diffPages(doc(para(1, "a ", text("b", "bold"), " c")), doc(para(1, "a b c")));
    const merged = first(result.doc);
    expect(merged.content?.some((child) => child.marks?.some((mark) => mark.type === "historyDiff"))).toBe(false);
    // Still a change of the block, so that it is outlined.
    expect(statusOf(merged)).toBe("changed");
    expect(result.summary.changed).toBe(1);
    expect(plain(merged)).toBe("a b c");
  });

  it("treats a line break and an inline atom as words of their own", () => {
    const ref = (n: number, pageId: string): PortableNode => ({ type: "pageRef", attrs: { id: id(n), pageId } });
    const version = para(1, "see ", ref(2, "p-1"), " and more", { type: "hardBreak" });
    const current = para(1, "see ", ref(3, "p-2"), " and more");
    const merged = first(diffPages(doc(version), doc(current)).doc);
    expect(runs(merged)).toEqual(["see ", "[-<pageRef>]", "[+<pageRef>]", " and more", "[+<hardBreak>]"]);
    // The same reference under another atom id is the same word.
    const same = diffPages(doc(para(1, "see ", ref(2, "p-1"))), doc(para(1, "see ", ref(9, "p-1"))));
    expect(same.summary.changed).toBe(0);
  });

  it("compares the spaces of code as they are, and the spaces of prose as one", () => {
    const code = (value: string): PortableNode => ({ type: "codeBlock", attrs: { id: id(1), language: "rust" }, content: [text(value)] });
    expect(diffPages(doc(code("a  b")), doc(code("a b"))).summary.changed).toBe(1);
    expect(diffPages(doc(para(1, "a  b")), doc(para(1, "a b"))).summary.changed).toBe(0);
  });

  it("splits scripts that have no spaces into characters", () => {
    const merged = first(diffPages(doc(para(1, "今日は晴れです")), doc(para(1, "今日は雨です"))).doc);
    expect(runs(merged)).toEqual(["今日は", "[-雨]", "[+晴れ]", "です"]);
  });

  it("makes a block that turned into another kind of text block the version's kind, changed, with its words compared", () => {
    const version: PortableNode = { type: "heading", attrs: { id: id(1), level: 2 }, content: [text("Title")] };
    const result = diffPages(doc(version), doc(para(1, "Title")));
    expect(first(result.doc).type).toBe("heading");
    expect(statusOf(first(result.doc))).toBe("changed");
    expect(runs(first(result.doc))).toEqual(["Title"]);
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 1 });
  });

  it("reads an empty block against one with text", () => {
    const result = diffPages(doc(para(1)), doc(para(1, "was here")));
    expect(runs(first(result.doc))).toEqual(["[-was here]"]);
    const back = diffPages(doc(para(1, "back")), doc(para(1)));
    expect(runs(first(back.doc))).toEqual(["[+back]"]);
  });

  it("falls back to replacing the part that differs when the words are too many to compare", () => {
    const words = (prefix: string, count: number) => Array.from({ length: count }, (_, index) => `${prefix}${String(index)}`).join(" ");
    // 3,000 against 3,000 words with nothing in common: the table would have 9 million cells.
    const result = diffPages(doc(para(1, "same start ", words("a", 3000), " same end")), doc(para(1, "same start ", words("b", 3000), " same end")));
    const merged = first(result.doc);
    const all = runs(merged);
    expect(all[0]).toBe("same start ");
    expect(all[all.length - 1]).toBe(" same end");
    expect(all).toHaveLength(4); // start, removed run, added run, end
    expect(all[1]?.startsWith("[-b0 b1")).toBe(true);
    expect(all[2]?.startsWith("[+a0 a1")).toBe(true);
  });
});

// ----- blocks that are not text --------------------------------------------------------------------------------------------

describe("diffPages: attributes of blocks that are not text", () => {
  it("marks a block whose attributes differ as changed, and one whose attributes are the same as the same", () => {
    const image = (alt: string, extra: Record<string, unknown> = {}) => leaf("image", 1, { attachmentId: "att-1", alt, ...extra });
    expect(statusOf(first(diffPages(doc(image("new")), doc(image("old"))).doc))).toBe("changed");
    expect(diffPages(doc(image("new")), doc(image("old"))).summary).toEqual({ added: 0, removed: 0, changed: 1 });
    expect(statusOf(first(diffPages(doc(image("same")), doc(image("same"))).doc))).toBe("same");
    // Absent and null are the same; the version's own attributes are what the merged block has.
    const merged = first(diffPages(doc(image("a", { width: 300 })), doc(image("a", { width: 300, caption: null }))).doc);
    expect(statusOf(merged)).toBe("same");
    expect(merged.attrs).toMatchObject({ id: id(1), attachmentId: "att-1", alt: "a", width: 300 });
  });

  it("compares attributes that are lists and objects by value", () => {
    const gallery = (...items: { attachmentId: string; caption?: string }[]) => doc(leaf("gallery", 1, { items, columns: 3 }));
    expect(diffPages(gallery({ attachmentId: "a" }, { attachmentId: "b", caption: "x" }), gallery({ attachmentId: "a" }, { attachmentId: "b", caption: "x" })).summary.changed).toBe(0);
    expect(diffPages(gallery({ attachmentId: "a" }), gallery({ attachmentId: "a" }, { attachmentId: "b" })).summary.changed).toBe(1);
  });

  it("marks a container whose own attributes differ as changed and still compares what is inside", () => {
    const callout = (color: string, inner: string) => doc(block("callout", 1, [para(2, inner)], { icon: "💡", color }));
    const result = diffPages(callout("blue", "text"), callout("red", "text"));
    expect(outline(result.doc)).toEqual(["callout#1:changed", "  paragraph#2:same"]);
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 1 });
    const both = diffPages(callout("blue", "new"), callout("red", "old"));
    expect(outline(both.doc)).toEqual(["callout#1:changed", "  paragraph#2:changed"]);
    expect(both.summary.changed).toBe(2);
  });

  it("marks the attributes of a text block that differ (a heading level, a code block's language)", () => {
    const heading = (level: number): PortableNode => ({ type: "heading", attrs: { id: id(1), level }, content: [text("Same")] });
    const result = diffPages(doc(heading(2)), doc(heading(1)));
    expect(statusOf(first(result.doc))).toBe("changed");
    expect(runs(first(result.doc))).toEqual(["Same"]);
  });

  it("does not compare a task item's checkbox as text: it is an attribute", () => {
    const task = (checked: boolean) => doc(block("taskList", 1, [block("taskItem", 2, [para(3, "do it")], { checked })]));
    const result = diffPages(task(true), task(false));
    expect(outline(result.doc)).toEqual(["taskList#1:same", "  taskItem#2:changed", "    paragraph#3:same"]);
  });
});

// ----- pages without ids ---------------------------------------------------------------------------------------------------

describe("diffPages: blocks without ids", () => {
  const bare = (value: string, type = "paragraph"): PortableNode => ({ type, content: [text(value)] });

  it("aligns blocks without ids by content, then by position and type", () => {
    const same = diffPages(doc(bare("a"), bare("b")), doc(bare("a"), bare("b")));
    expect(same.summary).toEqual({ added: 0, removed: 0, changed: 0 });
    expect(same.doc.content?.every((child) => statusOf(child) === "same")).toBe(true);

    // A paragraph inserted on the page: one removed block, the others matched (not shifted into changes).
    const inserted = diffPages(doc(bare("a"), bare("b"), bare("c")), doc(bare("a"), bare("new"), bare("b"), bare("c")));
    expect(inserted.summary).toEqual({ added: 0, removed: 1, changed: 0 });
    expect((inserted.doc.content ?? []).map((child) => `${plain(child)}:${statusOf(child) ?? "-"}`)).toEqual(["a:same", "new:removed", "b:same", "c:same"]);

    // A paragraph edited in place: the same position and type, so one changed block with its words compared.
    const edited = diffPages(doc(bare("one fish two fish")), doc(bare("one fish red fish")));
    expect(edited.summary).toEqual({ added: 0, removed: 0, changed: 1 });
    expect(runs(first(edited.doc))).toEqual(["one fish ", "[-red]", "[+two]", " fish"]);
  });

  it("does not pair a block without an id with one of another type", () => {
    const result = diffPages(doc(bare("a", "heading")), doc(bare("a")));
    expect(result.summary).toEqual({ added: 1, removed: 1, changed: 0 });
  });

  it("matches a version written without ids to a page that has them, by content", () => {
    const version = doc(bare("a"), bare("b"), bare("c"));
    const current = doc(para(1, "a"), para(2, "b"), para(3, "c"));
    const result = diffPages(version, current);
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 0 });
    expect(result.doc.content?.every((child) => statusOf(child) === "same")).toBe(true);
    // And the other way round.
    expect(diffPages(current, version).summary).toEqual({ added: 0, removed: 0, changed: 0 });
  });

  it("never pairs two blocks that both have ids and different ones, whatever their content", () => {
    const result = diffPages(doc(para(1, "same text"), bare("x")), doc(para(2, "same text"), bare("x")));
    expect(result.summary.added).toBe(1);
    expect(result.summary.removed).toBe(1);
  });

  it("leaves a lot of unmatched blocks without ids as added and removed instead of searching them all", () => {
    const many = (prefix: string) => doc(...Array.from({ length: 1200 }, (_, index) => bare(`${prefix}${String(index)}`)));
    const result = diffPages(many("a"), many("b"));
    expect(result.summary.added + result.summary.removed).toBe(2400);
    expect(result.summary.changed).toBe(0);
  });
});

// ----- big pages ---------------------------------------------------------------------------------------------------------

describe("diffPages: big pages", () => {
  function bigPage(count: number, edit: (index: number) => string | null): PortableNode {
    const blocks: PortableNode[] = [];
    for (let index = 0; index < count; index += 1) {
      const value = edit(index);
      if (value !== null) {
        blocks.push(para(index, value));
      }
    }
    return doc(...blocks);
  }

  it("diffs thousands of blocks with a few edits in well under a second, and finds exactly them", () => {
    const count = 6000;
    const line = (index: number) => `Block ${String(index)} says something about the number ${String(index * 7)} in a sentence.`;
    const version = bigPage(count, (index) => {
      if (index === 1000) {
        return null; // only on the page now
      }
      return index === 3000 ? `${line(index)} Edited in the version.` : line(index);
    });
    const current = bigPage(count, (index) => (index === 5000 ? null : line(index)));
    const started = performance.now();
    const result = diffPages(version, current);
    const took = performance.now() - started;
    expect(result.summary).toEqual({ added: 1, removed: 1, changed: 1 });
    expect(result.doc.content).toHaveLength(count);
    expect(took).toBeLessThan(1500);
  });

  it("diffs two pages that share no id (nothing in common) without a quadratic table", () => {
    const count = 5000;
    const a = doc(...Array.from({ length: count }, (_, index) => para(index, `a${String(index)}`)));
    const b = doc(...Array.from({ length: count }, (_, index) => para(count + index, `b${String(index)}`)));
    const started = performance.now();
    const result = diffPages(a, b);
    expect(performance.now() - started).toBeLessThan(1500);
    expect(result.summary).toEqual({ added: count, removed: count, changed: 0 });
  });

  it("diffs a page whose blocks were all moved into reverse order", () => {
    const count = 4000;
    const forward = doc(...Array.from({ length: count }, (_, index) => para(index, `n${String(index)}`)));
    const backward = doc(...Array.from({ length: count }, (_, index) => para(count - 1 - index, `n${String(count - 1 - index)}`)));
    const started = performance.now();
    const result = diffPages(forward, backward);
    expect(performance.now() - started).toBeLessThan(1500);
    // One block stays; the rest are moved: each is added where the version has it and removed where the page does.
    expect(result.summary).toEqual({ added: count - 1, removed: count - 1, changed: 0 });
  });

  it("diffs a deeply nested page", () => {
    const nest = (depth: number, value: string): PortableNode => {
      let inner: PortableNode = para(1000, value);
      for (let level = depth; level > 0; level -= 1) {
        inner = block("blockquote", level, [inner]);
      }
      return inner;
    };
    const result = diffPages(doc(nest(60, "new")), doc(nest(60, "old")));
    expect(result.summary).toEqual({ added: 0, removed: 0, changed: 1 });
  });
});
