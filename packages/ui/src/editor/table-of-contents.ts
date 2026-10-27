// The table of contents block: block schema v1's `tableOfContents` (a leaf atom, group
// `block`, attribute `id` only; ADR-0016, amendment of 2026-10-06). It stores nothing of the
// list: its view (`TableOfContentsView.tsx`) reads the headings of the editor's document and
// draws them, and clicking one moves the caret there. `collectHeadings` and `findHeading`
// are the pure part of that, over a ProseMirror document.

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";

import { insertLeafBlock, TABLE_OF_CONTENTS_NODE } from "./derived-blocks";
import { toggleHeadingLevel } from "./toggle-heading";

export { TABLE_OF_CONTENTS_NODE } from "./derived-blocks";

/** The heading levels the list shows (the schema allows these three). */
export const TOC_LEVELS: readonly number[] = [1, 2, 3];

export interface TableOfContentsOptions {
  /** How the editor shows the block; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    tableOfContents: {
      /** Inserts a table of contents where the selection is, the caret in the block after it. */
      insertTableOfContents: () => ReturnType;
    };
  }
}

/**
 * One line of the list: a heading of the page, as the document holds it now. A toggle heading
 * (ADR-0040) is one too, at its level: its entry is its summary, which stays on the page when the
 * toggle is closed.
 */
export interface TocEntry {
  /** The heading's block id, a toggle heading's summary's (a heading without one is found by its place in the list). */
  readonly id: string | null;
  readonly level: number;
  /** The heading's text without marks, a line break read as a space; empty for an empty heading. */
  readonly text: string;
}

/** The text of a heading as one line: marks dropped, a line break a space, a page reference nothing. */
function headingText(heading: PmNode): string {
  return heading.textBetween(0, heading.content.size, "", (leaf) => (leaf.type.name === "hardBreak" ? " " : ""));
}

/**
 * The level the list shows `node` at: a heading's (1 to 3) or a toggle heading's (the level of its
 * summary), `null` for any other node, a heading of another level and a plain toggle's summary.
 */
function levelOf(node: PmNode): number | null {
  if (node.type.name === "heading") {
    const level = Number(node.attrs.level);
    return TOC_LEVELS.includes(level) ? level : null;
  }
  return node.type.name === "detailsSummary" ? toggleHeadingLevel(node.attrs.level) : null;
}

/**
 * The headings of levels 1 to 3 and the toggle headings anywhere in `doc` (inside columns, callouts,
 * toggles and table cells too), in reading order. Walks the blocks only: it does not descend into text.
 */
export function collectHeadings(doc: PmNode): TocEntry[] {
  const entries: TocEntry[] = [];
  doc.descendants((node) => {
    const level = levelOf(node);
    if (level !== null) {
      const id: unknown = node.attrs.id;
      entries.push({ id: typeof id === "string" && id !== "" ? id : null, level, text: headingText(node) });
    }
    return !node.isTextblock;
  });
  return entries;
}

/** True when both lists show the same lines (so a view can keep what it has). */
export function sameEntries(a: readonly TocEntry[], b: readonly TocEntry[]): boolean {
  return a.length === b.length && a.every((entry, index) => {
    const other = b[index];
    return other !== undefined && entry.id === other.id && entry.level === other.level && entry.text === other.text;
  });
}

/**
 * Where the heading of `entry` starts in `doc` (just before it), or `null` when it is gone.
 * The block id finds it; a heading without one is the `ordinal`-th line of the list.
 */
export function findHeading(doc: PmNode, entry: TocEntry, ordinal: number): number | null {
  let found: number | null = null;
  let seen = 0;
  doc.descendants((node, pos) => {
    if (found !== null) {
      return false;
    }
    if (levelOf(node) !== null) {
      if (entry.id === null ? seen === ordinal : node.attrs.id === entry.id) {
        found = pos;
      }
      seen += 1;
    }
    return !node.isTextblock;
  });
  return found;
}

/**
 * The table of contents (ADR-0016, amendment of 2026-10-06): a leaf block with the attribute
 * `id` only. In HTML it is an empty `<div data-type="tableOfContents">`: what it lists is
 * derived from the page, so nothing of it is copied.
 */
export const TableOfContents = Node.create<TableOfContentsOptions>({
  name: TABLE_OF_CONTENTS_NODE,
  group: "block",
  atom: true,
  selectable: true,

  addOptions() {
    return { nodeView: null };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="tableOfContents"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": TABLE_OF_CONTENTS_NODE, class: "toc-block" }, HTMLAttributes)];
  },

  addCommands() {
    return {
      insertTableOfContents:
        () =>
        (props) =>
          insertLeafBlock(this.name, props),
    };
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
