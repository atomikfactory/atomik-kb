// The photo gallery block of the page schema (ADR-0030, ADR-0016 amendment of 2026-10-09):
// `gallery { id, items, columns }`, a leaf block (group `block`, no content) that shows several
// pictures in a grid.
//
// - `items`: the pictures in order, a list of plain maps `{ attachmentId, caption? }` set whole
//   (like `pageList`'s `properties` and `sort`): `[]` by default. The attachment is the picture,
//   the caption a line of plain text (absent = none).
// - `columns`: 2, 3 or 4 columns (default 3, stored).
//
// Every attachment an item lists is a reference of the block (the core's derived reference
// index, so retention never collects it) and the captions are the block's text. In HTML it is
// `<div data-type="gallery" data-columns data-items>` with `data-items` as JSON text and the
// captions as its text. Markdown writes one image line per item (`![](attachment://id "caption")`)
// and nothing of the grid.
//
// Reading is lenient and never rewrites: an entry that is not a map or has no valid
// `attachmentId` is not shown, a caption that is not a string reads as none, `columns` outside
// 2..4 reads as 3. A control the person uses writes the list it shows (a damaged entry is not in it).

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";

import { attachmentUrl, markdownTitle, normalizeCaption } from "./attachment-nodes";
import { insertLeafBlock } from "./derived-blocks";

export const GALLERY_NODE = "gallery";

/** The most pictures one gallery holds (the editor refuses more; readers show every one). */
export const MAX_GALLERY_ITEMS = 50;

/** The column counts of the grid. */
export const GALLERY_COLUMNS = [2, 3, 4] as const;
export type GalleryColumns = (typeof GALLERY_COLUMNS)[number];
export const DEFAULT_GALLERY_COLUMNS: GalleryColumns = 3;

/** One picture of a gallery. */
export interface GalleryItem {
  /** The attachment shown, a UUID in canonical lowercase form. */
  attachmentId: string;
  /** Plain text under the picture; absent = none (an empty one is never stored). */
  caption?: string;
}

/** The id as the core parses it (hyphens ignored, 32 hex digits), in canonical form, or `null`. */
export function canonicalAttachmentId(value: unknown): string | null {
  if (typeof value !== "string") {
    return null;
  }
  const hex = value.replaceAll("-", "").toLowerCase();
  if (!/^[0-9a-f]{32}$/u.test(hex)) {
    return null;
  }
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** The usable items of an `items` value: maps with a valid `attachmentId`, a caption only if it is non-empty text. */
export function readGalleryItems(value: unknown): GalleryItem[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const items: GalleryItem[] = [];
  for (const entry of value as unknown[]) {
    if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
      continue;
    }
    const { attachmentId, caption } = entry as Record<string, unknown>;
    const id = canonicalAttachmentId(attachmentId);
    if (id === null) {
      continue;
    }
    items.push(typeof caption === "string" && caption !== "" ? { attachmentId: id, caption } : { attachmentId: id });
  }
  return items;
}

/** The number of columns: 2, 3 or 4 (a numeric string counts), else 3. */
export function readGalleryColumns(value: unknown): GalleryColumns {
  const number = typeof value === "string" ? Number.parseInt(value, 10) : value;
  return number === 2 || number === 3 || number === 4 ? number : DEFAULT_GALLERY_COLUMNS;
}

/** The items of a gallery node. */
export function galleryItemsOf(node: PmNode): GalleryItem[] {
  return readGalleryItems(node.attrs.items);
}

/** `item` with its caption set to the edited `text` (one line, trimmed, at most 1000 characters; none when empty). */
export function withCaption(item: GalleryItem, text: string): GalleryItem {
  const caption = normalizeCaption(text);
  return caption === null ? { attachmentId: item.attachmentId } : { attachmentId: item.attachmentId, caption };
}

/** `items` with the one at `from` moved to index `to` (both in range, else unchanged). */
export function moveItem(items: readonly GalleryItem[], from: number, to: number): GalleryItem[] {
  const next = [...items];
  const moved = next[from];
  if (moved === undefined || to < 0 || to >= next.length || from === to) {
    return next;
  }
  next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

/** The Markdown of a gallery, as the core's export writes it: one image line per item, blank lines between. */
export function galleryMarkdown(node: PmNode): string {
  return galleryItemsOf(node)
    .map((item) => {
      const title = item.caption === undefined ? "" : ` "${markdownTitle(item.caption)}"`;
      return `![](${attachmentUrl(item.attachmentId)}${title})`;
    })
    .join("\n\n");
}

export interface GalleryOptions {
  /** How the editor shows the block; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    gallery: {
      /** Inserts an empty gallery where the selection is, the caret in the block after it. */
      insertGallery: () => ReturnType;
    };
  }
}

/** A JSON attribute of the HTML form, or `undefined` when the text is missing or is not JSON. */
function parseJson(text: string | null): unknown {
  if (text === null) {
    return undefined;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

/**
 * The photo gallery (ADR-0030): a leaf block with the attributes `items` and `columns`. The
 * pictures are attachments, shown from their bytes by the view (`GalleryView.tsx`).
 */
export const Gallery = Node.create<GalleryOptions>({
  name: GALLERY_NODE,
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  // No drop cursor over a gallery: a picture dragged inside it, or files dropped on it, go into the
  // grid, not between the page's blocks (ProseMirror's dropcursor reads this from the node's spec).
  extendNodeSchema() {
    return { disableDropCursor: true };
  },

  addOptions() {
    return { nodeView: null };
  },

  addAttributes() {
    return {
      items: {
        default: [],
        parseHTML: (element) => readGalleryItems(parseJson(element.getAttribute("data-items"))),
        renderHTML: (attributes) => ({ "data-items": JSON.stringify(readGalleryItems(attributes.items)) }),
      },
      columns: {
        default: DEFAULT_GALLERY_COLUMNS,
        parseHTML: (element) => readGalleryColumns(element.getAttribute("data-columns")),
        renderHTML: (attributes) => ({ "data-columns": String(readGalleryColumns(attributes.columns)) }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="gallery"]' }];
  },

  renderHTML({ node, HTMLAttributes }) {
    const captions = galleryItemsOf(node)
      .map((item) => item.caption ?? "")
      .filter((caption) => caption !== "")
      .join(" ");
    return ["div", mergeAttributes({ "data-type": GALLERY_NODE, class: "gallery-block" }, HTMLAttributes), captions];
  },

  renderText({ node }) {
    return galleryMarkdown(node);
  },

  addCommands() {
    return {
      insertGallery:
        () =>
        (props) =>
          insertLeafBlock(this.name, props),
    };
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
