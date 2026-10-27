// The attachment blocks of the page schema (ADR-0020, milestone-05.md "Model"): `image`
// `{ id, attachmentId, alt, width, widthPercent, align, caption }` and `file`
// `{ id, attachmentId, view }`, leaf blocks allowed wherever blocks are. They hold a reference
// only: the name, type and size come from the attachment's manifest when the block is shown.
// Attributes are plain values in the page document, like a page reference's, and `null` ones
// (a width never set) are not stored. The layout attributes of an image (ADR-0028,
// ADR-0016 amendment of 2026-10-09) are optional and read leniently: a damaged value is absent.

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";

export const IMAGE_NODE = "image";
export const FILE_NODE = "file";

/** The node names that reference an attachment. */
export const ATTACHMENT_NODES: readonly string[] = [IMAGE_NODE, FILE_NODE];

export interface AttachmentNodeOptions {
  /** How the editor shows the block; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
  /** The attachment's file name when known, for text and Markdown copies of a file block. */
  name: (attachmentId: string) => string | null;
}

/** True for `image` and `file` nodes. */
export function isAttachmentNode(node: PmNode): boolean {
  return ATTACHMENT_NODES.includes(node.type.name);
}

/** The `view` of a file block that shows it as an inline viewer (ADR-0031); absent = the card. */
export type FileView = "preview";

/** `"preview"`, or `null` (absent, or a value this version does not know: the card). */
export function parseFileView(value: unknown): FileView | null {
  return value === "preview" ? "preview" : null;
}

/** True for the MIME type of a PDF (what the preview can show). */
export function isPdfMime(mime: string): boolean {
  return mime.trim().toLowerCase() === "application/pdf";
}

/**
 * Whether an attachment is a PDF the preview can show: its type says so, or it has no specific
 * type (`application/octet-stream`, what a file of unknown type is stored as) and a name that ends
 * in `.pdf`.
 */
export function isPdfFile(name: string, mime: string): boolean {
  const type = mime.trim().toLowerCase();
  return isPdfMime(type) || ((type === "" || type === "application/octet-stream") && /\.pdf$/iu.test(name.trim()));
}

/** What a media player can show. */
export type MediaKind = "video" | "audio";

/**
 * `"video"` or `"audio"` for an attachment whose MIME type says so (`video/*`, `audio/*`; the shell's
 * media protocol serves nothing else, ADR-0032), else `null`. The type decides, not the name: a
 * file of unknown type stays a card.
 */
export function mediaKindOf(mime: string): MediaKind | null {
  const type = mime.trim().toLowerCase();
  if (type.startsWith("video/") && type.length > "video/".length) {
    return "video";
  }
  if (type.startsWith("audio/") && type.length > "audio/".length) {
    return "audio";
  }
  return null;
}

/** The attachment a block references, `null` when it has none (a damaged block). */
export function attachmentIdOf(node: PmNode): string | null {
  const id: unknown = node.attrs.attachmentId;
  return typeof id === "string" && id !== "" ? id : null;
}

/** The URL Markdown export gives an attachment. */
export function attachmentUrl(attachmentId: string): string {
  return `attachment://${attachmentId}`;
}

/** The body of a double-quoted Markdown link title: `\` and `"` escaped, a line break one space. */
export function markdownTitle(text: string): string {
  return text.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\r\n|\r|\n/g, " ");
}

/**
 * The Markdown of an attachment block, as the core's export writes it:
 * `![alt](attachment://<id>)` for an image (`![alt](attachment://<id> "caption")` with a
 * caption), `[<name or id>](attachment://<id>)` for a file.
 */
export function attachmentMarkdown(node: PmNode, name: string | null): string {
  const id = attachmentIdOf(node) ?? "";
  if (node.type.name === IMAGE_NODE) {
    const alt = typeof node.attrs.alt === "string" ? node.attrs.alt : "";
    const caption = parseCaption(node.attrs.caption);
    const title = caption === null ? "" : ` "${markdownTitle(caption)}"`;
    return `![${alt}](${attachmentUrl(id)}${title})`;
  }
  return `[${name ?? id}](${attachmentUrl(id)})`;
}

/** A whole number of pixels, or `null` (the image's own width). */
export function parseWidth(value: unknown): number | null {
  const number = typeof value === "string" ? Number.parseInt(value, 10) : value;
  return typeof number === "number" && Number.isInteger(number) && number > 0 ? number : null;
}

/** The narrowest an image can be made, in percent of its column. */
export const MIN_WIDTH_PERCENT = 10;
/** The widest: the whole column. */
export const MAX_WIDTH_PERCENT = 100;

/** A whole number from 10 to 100 (the image's width in percent of its column), or `null`. */
export function parseWidthPercent(value: unknown): number | null {
  const number = typeof value === "string" ? Number.parseInt(value, 10) : value;
  return typeof number === "number" &&
    Number.isInteger(number) &&
    number >= MIN_WIDTH_PERCENT &&
    number <= MAX_WIDTH_PERCENT
    ? number
    : null;
}

export type ImageAlign = "left" | "center" | "right";

/** `"left"`, `"center"` or `"right"`, or `null` (absent, which shows as centered). */
export function parseAlign(value: unknown): ImageAlign | null {
  return value === "left" || value === "center" || value === "right" ? value : null;
}

/** The caption of an image: its text, or `null` when there is none (absent, empty, not a string). */
export function parseCaption(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

/** The most characters a caption can have (the field's `maxLength`). */
export const CAPTION_MAX_LENGTH = 1000;

/**
 * What an edited caption becomes in the block: line breaks turned into spaces, the ends
 * trimmed, cut to `CAPTION_MAX_LENGTH`; `null` when nothing is left (the key is then removed).
 */
export function normalizeCaption(text: string): string | null {
  const caption = text.replace(/\r\n|\r|\n/g, " ").trim().slice(0, CAPTION_MAX_LENGTH).trim();
  return caption === "" ? null : caption;
}

const attachmentIdAttribute = {
  attachmentId: {
    default: null,
    parseHTML: (element: HTMLElement) => element.getAttribute("data-attachment-id"),
    renderHTML: (attributes: Record<string, unknown>) =>
      typeof attributes.attachmentId === "string" ? { "data-attachment-id": attributes.attachmentId } : {},
  },
};

/**
 * An image: shown from the attachment's bytes, with its alt text, its width, alignment and
 * caption. In HTML it is
 * `<figure data-type="image" data-attachment-id data-alt data-width data-width-percent data-align data-caption>`
 * whose `<figcaption>` holds the caption, else the alt text (no `src`: the bytes never leave
 * the app through the clipboard).
 */
export const ImageBlock = Node.create<AttachmentNodeOptions>({
  name: IMAGE_NODE,
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addOptions() {
    return { nodeView: null, name: () => null };
  },

  addAttributes() {
    return {
      ...attachmentIdAttribute,
      alt: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-alt") ?? "",
        renderHTML: (attributes) => (typeof attributes.alt === "string" ? { "data-alt": attributes.alt } : {}),
      },
      width: {
        default: null,
        parseHTML: (element) => parseWidth(element.getAttribute("data-width")),
        renderHTML: (attributes) => {
          const width = parseWidth(attributes.width);
          return width === null ? {} : { "data-width": String(width) };
        },
      },
      widthPercent: {
        default: null,
        parseHTML: (element) => parseWidthPercent(element.getAttribute("data-width-percent")),
        renderHTML: (attributes) => {
          const percent = parseWidthPercent(attributes.widthPercent);
          return percent === null ? {} : { "data-width-percent": String(percent) };
        },
      },
      align: {
        default: null,
        parseHTML: (element) => parseAlign(element.getAttribute("data-align")),
        renderHTML: (attributes) => {
          const align = parseAlign(attributes.align);
          return align === null ? {} : { "data-align": align };
        },
      },
      caption: {
        default: null,
        parseHTML: (element) => parseCaption(element.getAttribute("data-caption")),
        renderHTML: (attributes) => {
          const caption = parseCaption(attributes.caption);
          return caption === null ? {} : { "data-caption": caption };
        },
      },
    };
  },

  parseHTML() {
    return [{ tag: 'figure[data-type="image"]' }];
  },

  renderHTML({ node, HTMLAttributes }) {
    const alt = typeof node.attrs.alt === "string" ? node.attrs.alt : "";
    return [
      "figure",
      mergeAttributes({ "data-type": "image", class: "image-block" }, HTMLAttributes),
      ["figcaption", {}, parseCaption(node.attrs.caption) ?? alt],
    ];
  },

  renderText({ node }) {
    return attachmentMarkdown(node, null);
  },

  addNodeView() {
    return this.options.nodeView;
  },
});

/**
 * A file: a card with the attachment's name and size, or, with `view: "preview"` (ADR-0031), an
 * inline viewer when the file is a PDF. `view` is stored only when set. In HTML it is
 * `<div data-type="file" data-attachment-id data-view>` with the name as text.
 */
export const FileBlock = Node.create<AttachmentNodeOptions>({
  name: FILE_NODE,
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addOptions() {
    return { nodeView: null, name: () => null };
  },

  addAttributes() {
    return {
      ...attachmentIdAttribute,
      view: {
        default: null,
        parseHTML: (element) => parseFileView(element.getAttribute("data-view")),
        renderHTML: (attributes) => {
          const view = parseFileView(attributes.view);
          return view === null ? {} : { "data-view": view };
        },
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="file"]' }];
  },

  renderHTML({ node, HTMLAttributes }) {
    const id = attachmentIdOf(node);
    const name = (id === null ? null : this.options.name(id)) ?? "";
    return ["div", mergeAttributes({ "data-type": "file", class: "file-block" }, HTMLAttributes), name];
  },

  renderText({ node }) {
    const id = attachmentIdOf(node);
    return attachmentMarkdown(node, id === null ? null : this.options.name(id));
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
