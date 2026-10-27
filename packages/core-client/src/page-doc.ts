// Page document shape v1 (ADR-0016, `docs/development/milestone-03.md`, "Page document"
// and "Portable JSON") on the TypeScript side: the portable JSON <-> Loro conversion,
// the normalisation both runtimes compare against, and the block/link extraction the
// core performs for its text index. `MemoryTransport` uses these to mirror the core; they
// are exported as helpers for tests and fixtures. The editor itself talks to Loro
// through the vendored binding (packages/ui), whose element writes this module
// reproduces exactly: element node = `LoroMap { nodeName, attributes: LoroMap,
// children: LoroList }` with regular child containers (`insertContainer`/`setContainer`),
// consecutive text nodes = one `LoroText` whose marks are Loro text marks
// (`mark name -> attrs object`), `null` attributes never stored.
//
// The root is the exception: whoever creates a page document (the core, this mirror,
// the fixture script) makes the root map's `attributes` and `children` *mergeable*
// containers (`ensureMergeableMap` / `ensureMergeableList`). Their ids derive from the
// root and the key instead of from an operation, so two devices that create the same
// page document concurrently share one children list and both first paragraphs survive
// the merge; with regular containers one creation would hide the other's list.
// Readers find containers by key, so a root with regular containers (as the binding
// would create it if it ever initialised a document) reads exactly the same.

import { LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import type { Delta, Value } from "loro-crdt";

import type { AccountId } from "./generated/AccountId";
import type { AttachmentId } from "./generated/AttachmentId";
import type { BlockId } from "./generated/BlockId";
import type { PageId } from "./generated/PageId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { invalid } from "./validate";
import { WHITEBOARDS, importWhiteboards, readWhiteboard, whiteboardTexts, whiteboardsPortable } from "./whiteboard";

/** Root map of a page document. */
const ROOT = "doc";
/** Node name key of an element node. */
const NODE_NAME = "nodeName";
/** Attributes map of an element node. */
const ATTRIBUTES = "attributes";
/** Children list of an element node. */
const CHILDREN = "children";
/** Node name of the inline page reference. */
const PAGE_REF = "pageRef";
/** Node name of the page link block (a card; the same `pageId` and `blockId` as a `pageRef`). */
const PAGE_LINK = "pageLink";
/**
 * Node name of the inline date mention (`dateMention { id, date, time? }`, ADR-0016 amendment of
 * 2026-10-07): an inline atom whose `date` is a floating local date (`YYYY-MM-DD`, no time zone)
 * and whose optional `time` is `HH:mm`. It reads as its ISO text (`dateMentionText`).
 */
const DATE_MENTION = "dateMention";
/**
 * Node name of the inline equation (`mathInline { id, latex }`, ADR-0016 amendment of 2026-10-07): an
 * inline atom whose `latex` attribute is the LaTeX source (a string, empty while it is being written).
 * It reads as that source (`mathInlineSource`).
 */
const MATH_INLINE = "mathInline";
/**
 * Node name of the inline person mention (`personMention { accountId, label }`, ADR-0049): an inline atom that refers to a
 * member by account id, with the name at insertion as a fallback label. It has no block id. It reads as its `label` inside
 * the text around it (`personMentionLabel`).
 */
const PERSON_MENTION = "personMention";
/**
 * Node name of the template button block (`templateButton { id, label, position }`, content `block+`; ADR-0056): a button
 * that holds blocks and inserts a fresh copy of them when it is clicked. Its content is template material, which the
 * readers of text, links, date mentions and person mentions skip (`extractBlocks`, `extractLinks`,
 * `extractDateMentions`, `extractPersonMentions`).
 */
const TEMPLATE_BUTTON = "templateButton";
/** Node name of a hard line break. */
const HARD_BREAK = "hardBreak";
/** Node type of text in ProseMirror JSON. */
const TEXT = "text";
/**
 * Node name of an image block (leaf; attributes `attachmentId`, `alt`, `width`, and, from the
 * amendment of 2026-10-09 (ADR-0028), `widthPercent`, `align` and `caption`).
 */
const IMAGE = "image";
/** Node name of a file block (leaf; attributes `attachmentId` and, since 2026-10-09 (ADR-0031), `view`). */
const FILE = "file";
/**
 * Node name of a photo gallery block (leaf; attributes `items`, a list of `{ attachmentId, caption? }`,
 * and `columns`; the amendment of 2026-10-09, ADR-0030).
 */
const GALLERY = "gallery";
/**
 * Node name of a whiteboard block (leaf; attributes `id` and `height`; the amendment of 2026-10-10,
 * ADR-0033). Its elements are not in the node: they live in the root map `whiteboards`
 * (`whiteboard.ts`).
 */
const WHITEBOARD = "whiteboard";

/**
 * The marks of shape v1 with their expansion, in the fixed order of the spec (and of
 * `kb_doc::PAGE_MARKS`). Marks outside the table never expand. Part of the persisted
 * shape: every replica of a page document is configured with it. `textColor` and
 * `highlight` (2026-10-07, additive) each carry one attribute, `color`, a palette name
 * (`gray`, `brown`, `orange`, `yellow`, `green`, `blue`, `purple`, `pink`, `red`; a name this
 * version does not know is kept as it is), and come after the six marks of the first table.
 */
export const PAGE_MARKS: readonly (readonly [name: string, expand: "after" | "none"])[] = [
  ["bold", "after"],
  ["italic", "after"],
  ["underline", "after"],
  ["strike", "after"],
  ["code", "none"],
  ["link", "none"],
  ["textColor", "after"],
  ["highlight", "after"],
];

const MARK_RANK = new Map(PAGE_MARKS.map(([name], rank) => [name, rank]));

/** One piece of a block's direct inline content (`kb_model::Inline`). Marks are dropped. */
export type PageInline =
  | { type: "text"; text: string }
  | { type: "pageRef"; pageId: PageId; blockId?: BlockId }
  | { type: "break" };

/** A node carrying an `id` attribute, with its direct inline content (`kb_model::BlockText`). */
export interface PageBlock {
  /** The node's `id` attribute. */
  id: BlockId;
  /** The node name (`"paragraph"`, `"heading"`, `"listItem"`, `"image"`, `"file"`, `"pageLink"`, ...). */
  kind: string;
  /** Text runs, page references and hard breaks that are direct children, in order; empty for containers. */
  inlines: PageInline[];
  /**
   * The block sits inside a `templateButton` (ADR-0056): template material, listed with no inlines (so the text index,
   * the backlinks' text and the AI index hold none of it) and not the page's first picture.
   */
  templateContent?: true;
  /** `image` and `file` blocks: the attachment shown, when the attribute holds a valid id. */
  attachmentId?: AttachmentId;
  /** `image` blocks: the alt text, when it is a string (indexed for search). */
  alt?: string;
  /** `image` blocks: the display width in pixels, when it is an integer. */
  width?: number;
  /**
   * `image` blocks: the display width as a share of the block's container, when it is an integer
   * from 10 to 100 (`50.0` is 50; anything else reads as absent). Wins over {@link width}. Layout,
   * not text.
   */
  widthPercent?: number;
  /**
   * `image` blocks: where the picture sits in its block, when it is `"left"`, `"center"` or
   * `"right"`. Layout, not text.
   */
  align?: "left" | "center" | "right";
  /**
   * `image` blocks: the caption, when it is a string. Text: the block's inlines hold it before the
   * alt text (see {@link extractBlocks}).
   */
  caption?: string;
  /**
   * `file` blocks: how the file is shown, `"preview"` (an inline viewer) when the attribute says
   * so; absent for the card and for any damaged value (ADR-0031). Layout, not text.
   */
  view?: "preview";
  /**
   * `gallery` blocks: the pictures in order, the entries that are a map with a valid
   * `attachmentId` (a damaged entry is left out); the caption is kept when it is a string.
   */
  items?: GalleryItem[];
  /** `gallery` blocks: the number of columns, when it is an integer from 2 to 4 (else absent: 3). */
  columns?: 2 | 3 | 4;
  /**
   * `whiteboard` blocks: the height of the inline view in CSS pixels, when it is an integer from 160
   * to 1200 (else absent: 360). Layout, not text.
   */
  height?: number;
}

/** One picture of a `gallery` block. */
export interface GalleryItem {
  /** The attachment shown (canonical lowercase id). */
  attachmentId: AttachmentId;
  /** The caption, when it is a string (possibly empty). */
  caption?: string;
}

/**
 * The usable entries of a gallery's `items` value: maps with a valid `attachmentId`, a caption kept
 * when it is a string. `undefined` when the value is no list. Mirrors `Element::gallery_entries`
 * (the entries with an attachment).
 */
export function galleryItemsOf(value: unknown): GalleryItem[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const items: GalleryItem[] = [];
  for (const entry of value as unknown[]) {
    if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
      continue;
    }
    const raw: unknown = (entry as Record<string, unknown>).attachmentId;
    const attachmentId = typeof raw === "string" ? canonicalId(raw) : undefined;
    if (attachmentId === undefined) {
      continue;
    }
    const caption: unknown = (entry as Record<string, unknown>).caption;
    items.push(typeof caption === "string" ? { attachmentId, caption } : { attachmentId });
  }
  return items;
}

/** A page reference attributed to the nearest enclosing node with an id (`kb_model::PageLink`). */
export interface PageLink {
  /** The nearest ancestor of the `pageRef` that carries an `id`; a `pageLink` block is its own. */
  fromBlock: BlockId;
  /** The referenced page. */
  toPage: PageId;
  /** The referenced block, when the reference names one. */
  toBlock?: BlockId;
}

/**
 * A date mention attributed to the nearest enclosing node with an id: a row of the derived
 * `date_mentions` table (`kb_model::DateMention`, ADR-0024).
 */
export interface PageDateMention {
  /** The nearest ancestor of the mention that carries an `id`. */
  blockId: BlockId;
  /** The mention's own `id`. */
  nodeId: BlockId;
  /** The day, `YYYY-MM-DD` (always valid here). */
  date: string;
  /** The time of day, `HH:mm`; `null` when the mention has none or its time is malformed. */
  time: string | null;
  /** Minutes before the moment that a reminder fires; `null` for no reminder (also a damaged one). */
  reminderMinutes: number | null;
  /**
   * When the reminder fires, in seconds since 1970-01-01T00:00:00 on the local wall clock (the
   * calendar fields counted as UTC); `null` exactly when there is no reminder.
   */
  fireAtLocal: number | null;
  /**
   * The account whose devices fire the reminder (`reminderFor`, ADR-0047): a valid account id in its canonical form; absent when the
   * attribute is absent or damaged (then the Owners' devices fire in a shared space).
   */
  reminderFor?: AccountId;
}

/** Non-null attribute values by name. */
export type Attrs = { [key: string]: JsonValue };

/**
 * A node of the normalised portable tree: the internal form behind every conversion here
 * (package-internal; the public functions take and return `JsonValue`).
 */
export interface PortableNode {
  type: string;
  /** Non-null attributes with sorted keys; `undefined` when there are none. */
  attrs: Attrs | undefined;
  content: PortableNode[];
  /** Text nodes only: marks in canonical order. */
  marks: PortableMark[];
  /** Text nodes only. */
  text: string | undefined;
}

/** A mark of a text node in the normalised tree. */
export interface PortableMark {
  type: string;
  attrs: Attrs | undefined;
}

/**
 * Configures a page document's text styles: the mark table of shape v1 ({@link PAGE_MARKS})
 * and `none` for any other mark, exactly as `kb-doc` configures the core's replica.
 * Idempotent. Every function here that writes marks calls it first.
 */
export function configurePageDoc(doc: LoroDoc): void {
  const styles: { [mark: string]: { expand: "after" | "none" } } = {};
  for (const [name, expand] of PAGE_MARKS) {
    styles[name] = { expand };
  }
  doc.configTextStyle(styles);
  doc.configDefaultTextStyle({ expand: "none" });
}

/**
 * Builds the Loro shape of a portable page (`{ "type": "doc", "content": [...] }`) into
 * an empty `LoroDoc` and commits: a mergeable root (see the module comment), every
 * element node the way the editor binding writes it. Accepts
 * the normalised portable form as well as raw ProseMirror JSON (`null` attributes are
 * skipped; marks on non-text nodes are dropped like the binding drops them). Mark
 * attributes become the mark's value, marks without attributes get `{}`. The document
 * is configured with {@link configurePageDoc} first.
 *
 * Throws an `InvalidInput` `CoreError` (field `content`) when `json` is not a node tree
 * with a `doc` root, or when the document already has content.
 */
export function portableToLoro(doc: LoroDoc, json: JsonValue): void {
  const node = parseNode(json, "doc");
  if (node.type !== ROOT) {
    throw invalid("content", "the root node must be a doc");
  }
  const root = doc.getMap(ROOT);
  if (root.size > 0) {
    throw invalid("content", "the document is not empty");
  }
  const whiteboards = isObject(json) ? json[WHITEBOARDS] : undefined;
  configurePageDoc(doc);
  if (whiteboards !== undefined && whiteboards !== null) {
    // Validated before anything is written (ADR-0033).
    importWhiteboards(doc, whiteboards);
  }
  writeNode(root, node, true);
  doc.commit();
}

/**
 * Reads a page document into portable JSON (format 1): ProseMirror JSON whose `attrs`
 * hold non-null values only (keys sorted, omitted when empty), `content` omitted when
 * empty, marks `{ type, attrs? }` in the order of {@link PAGE_MARKS} (other marks after
 * them by name), keys in the order `type, attrs, content, marks, text`. Text is split
 * per distinct mark set like ProseMirror does: adjacent runs (also across adjacent
 * `LoroText`s, which concurrent edits can leave behind) with the same marks are one text
 * node. Element maps without a string `nodeName` are skipped. An empty document reads
 * as `{ "type": "doc" }`.
 */
export function loroToPortable(doc: LoroDoc): JsonValue {
  const node = readNode(doc.getMap(ROOT), ROOT) ?? emptyNode(ROOT);
  const json = toJson(node);
  // The elements of the whiteboards that have a node (ADR-0033); orphans are not exported.
  const live = new Set<string>();
  const collect = (n: PortableNode): void => {
    const id = n.type === WHITEBOARD ? n.attrs?.id : undefined;
    const canonical = typeof id === "string" ? canonicalId(id) : undefined;
    if (canonical !== undefined) {
      live.add(canonical);
    }
    n.content.forEach(collect);
  };
  collect(node);
  const whiteboards = live.size === 0 ? undefined : whiteboardsPortable(doc, live);
  if (whiteboards !== undefined && isObject(json)) {
    json[WHITEBOARDS] = whiteboards;
  }
  return json;
}

/**
 * Rewrites the attributes of every `pageList` node of a document (the saved views of ADR-0016's
 * amendments; a move to another space remaps the schema ids they hold, ADR-0038). `edit` gets a
 * node's attributes as plain JSON (non-null values, keys sorted) and answers the attributes the node
 * should have: a key it leaves out is deleted, an entry it changes is set, a node whose attributes
 * come out equal is not written. Answers whether anything was written; the caller commits.
 */
export function editPageListAttrs(doc: LoroDoc, edit: (attrs: Attrs) => Attrs): boolean {
  let changed = false;
  const visit = (map: LoroMap): void => {
    const attributes = nodeNameOf(map) === "pageList" ? attributesOf(map) : undefined;
    if (attributes !== undefined) {
      const raw: unknown = attributes.toJSON();
      const before = isObject(raw) ? (cleanAttrs(raw) ?? {}) : {};
      const after = edit(before);
      if (canonicalText(before) !== canonicalText(after)) {
        for (const key of Object.keys(isObject(raw) ? raw : {})) {
          if (!(key in after)) {
            attributes.delete(key);
          }
        }
        for (const [key, value] of Object.entries(after)) {
          if (canonicalText(before[key] ?? null) !== canonicalText(value)) {
            attributes.set(key, value);
          }
        }
        changed = true;
      }
    }
    for (const child of childrenOf(map)) {
      if (isLoroMap(child)) {
        visit(child);
      }
    }
  };
  visit(doc.getMap(ROOT));
  return changed;
}

/**
 * Applies the portable normalisation of {@link loroToPortable} to ProseMirror JSON (for
 * example Tiptap's `editor.getJSON()`), so that what the editor holds and what the Loro
 * document holds compare with `toEqual`: `null` attributes and empty `attrs`/`content`/
 * `marks` removed, adjacent text nodes with the same marks merged, empty text nodes
 * dropped, marks on non-text nodes dropped, mark order canonical, keys ordered.
 *
 * Throws an `InvalidInput` `CoreError` (field `content`) when `json` is not a node tree.
 */
export function normalizePortable(json: JsonValue): JsonValue {
  return toJson(parsePortable(json));
}

/** The validated, normalised tree of ProseMirror JSON (package-internal; see {@link normalizePortable}). */
export function parsePortable(json: JsonValue): PortableNode {
  return parseNode(json, "doc");
}

/** A stored number that is an integer (a bigint counts), else `undefined`. */
function integerOf(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isInteger(value)) {
    return value;
  }
  return typeof value === "bigint" ? Number(value) : undefined;
}

/**
 * Every node whose `id` attribute is a valid id (32 hex digits; given back in canonical
 * lowercase form), in document order (pre-order), with its direct inline content: text
 * (consecutive runs joined, marks dropped), `pageRef` references with a valid `pageId`
 * and `hardBreak`s; any other child node ends a text run. Container blocks (lists, list
 * items, quotes) have no inlines of their own; inline nodes that carry an id (`pageRef`)
 * are listed too, with none (`hardBreak` has no id in schema v1, so it is only ever an
 * inline break). `image` and `file` blocks (Milestone 5) are leaves: an image's inlines are
 * its caption then its alt text (2026-10-09, ADR-0028: one text inline, the two joined by one
 * space when both are non-empty, either alone when the other is empty or not a string, none when
 * both are), a file's are empty; both also carry their `attachmentId` (when it is a valid id),
 * images their `alt` and `caption` (strings), `width` (an integer), `widthPercent` (an integer
 * from 10 to 100) and `align` (`"left"`, `"center"` or `"right"`); a damaged value is left out.
 * A `pageLink` block (the card, 2026-10-06) reads as the one `pageRef`
 * inline it stands for (none when its `pageId` is not valid), so the text index treats a card
 * like a chip: the referenced page's title is its text. A `dateMention` (2026-10-07) reads as
 * its ISO text (`2026-10-07`, `2026-10-07 14:30`) inside the text around it (it does not split
 * the run; `dateMentionText`), so the text index and the backlinks' text hold the date as typed.
 * A `mathInline` (2026-10-07) reads as its LaTeX source inside the text around it in the same way
 * (`mathInlineSource`), so a search for `E=mc^2` finds the block.
 * The blocks inside a `templateButton` (ADR-0056) are listed with no inlines at all and marked `templateContent`.
 * Mirrors `PageDoc::blocks`.
 */
export function extractBlocks(doc: LoroDoc): PageBlock[] {
  const blocks: PageBlock[] = [];
  const visit = (map: LoroMap, inButton: boolean): void => {
    const children = childrenOf(map);
    const id = idAttribute(map, "id");
    const kind = nodeNameOf(map);
    if (id !== undefined && kind !== undefined) {
      const block: PageBlock = { id, kind, inlines: directInlines(children) };
      if (kind === PAGE_LINK) {
        // A leaf: it reads as the reference it stands for, whatever children it has.
        const ref = pageRefOf(map);
        block.inlines = ref === undefined ? [] : [pageRefInline(ref)];
      }
      if (kind === IMAGE || kind === FILE) {
        // Leaves: whatever children they have are not text of theirs (a file reads as nothing).
        block.inlines = [];
        const attachmentId = idAttribute(map, "attachmentId");
        if (attachmentId !== undefined) {
          block.attachmentId = attachmentId;
        }
      }
      if (kind === FILE && attributesOf(map)?.get("view") === "preview") {
        block.view = "preview";
      }
      if (kind === IMAGE) {
        const alt = attributesOf(map)?.get("alt");
        if (typeof alt === "string") {
          block.alt = alt;
        }
        const caption: unknown = attributesOf(map)?.get("caption");
        if (typeof caption === "string") {
          block.caption = caption;
        }
        // A leaf: it reads as its caption then its alt text (what the index holds), one space
        // apart when both are there, nothing when both are empty.
        const text = [typeof caption === "string" ? caption : "", typeof alt === "string" ? alt : ""]
          .filter((part) => part !== "")
          .join(" ");
        block.inlines = text === "" ? [] : [{ type: "text", text }];
        const width = integerOf(attributesOf(map)?.get("width"));
        if (width !== undefined) {
          block.width = width;
        }
        const widthPercent = integerOf(attributesOf(map)?.get("widthPercent"));
        if (widthPercent !== undefined && widthPercent >= 10 && widthPercent <= 100) {
          block.widthPercent = widthPercent;
        }
        const align: unknown = attributesOf(map)?.get("align");
        if (align === "left" || align === "center" || align === "right") {
          block.align = align;
        }
      }
      if (kind === GALLERY) {
        // A leaf: it reads as the captions of its pictures, one space apart (ADR-0030).
        const items = galleryItemsOf(attributesOf(map)?.get("items")) ?? [];
        block.items = items;
        const text = items
          .map((item) => item.caption ?? "")
          .filter((caption) => caption !== "")
          .join(" ");
        block.inlines = text === "" ? [] : [{ type: "text", text }];
        const columns = integerOf(attributesOf(map)?.get("columns"));
        if (columns === 2 || columns === 3 || columns === 4) {
          block.columns = columns;
        }
      }
      if (kind === WHITEBOARD) {
        // A leaf: it reads as the live text elements of its drawing, one space apart (ADR-0033).
        const text = whiteboardTexts(readWhiteboard(doc, id)).join(" ");
        block.inlines = text === "" ? [] : [{ type: "text", text }];
        const height = integerOf(attributesOf(map)?.get("height"));
        if (height !== undefined && height >= 160 && height <= 1200) {
          block.height = height;
        }
      }
      if (inButton) {
        // Template material: the block is listed, its text is not.
        block.inlines = [];
        block.templateContent = true;
      }
      blocks.push(block);
    }
    const insideButton = inButton || kind === TEMPLATE_BUTTON;
    for (const child of children) {
      if (isLoroMap(child) && nodeNameOf(child) !== undefined) {
        visit(child, insideButton);
      }
    }
  };
  visit(doc.getMap(ROOT), false);
  return blocks;
}

/** A block that shows an attachment (`kb_model::PageDoc::attachment_refs`). */
export interface PageAttachmentRef {
  /** The `image`, `file` or `gallery` block. */
  blockId: BlockId;
  /** The attachment it shows. */
  attachmentId: AttachmentId;
}

/**
 * The attachments the `image`, `file` and `gallery` blocks of `blocks` (as {@link extractBlocks} lists
 * them) show, in document order, duplicates kept: what the core's reference index holds for
 * the page. Blocks without a valid id or `attachmentId` reference nothing.
 */
export function attachmentRefsOf(blocks: readonly PageBlock[]): PageAttachmentRef[] {
  const refs: PageAttachmentRef[] = [];
  for (const block of blocks) {
    if (block.attachmentId !== undefined) {
      refs.push({ blockId: block.id, attachmentId: block.attachmentId });
    }
    // A gallery references each picture it lists (ADR-0030); a picture twice is listed twice.
    for (const item of block.items ?? []) {
      refs.push({ blockId: block.id, attachmentId: item.attachmentId });
    }
  }
  return refs;
}

/** {@link attachmentRefsOf} the blocks of a page document; mirrors `PageDoc::attachment_refs`. */
export function extractAttachmentRefs(doc: LoroDoc): PageAttachmentRef[] {
  return attachmentRefsOf(extractBlocks(doc));
}

/**
 * Every `pageRef` with a valid `pageId`, in document order, attributed to the nearest
 * enclosing node (strict ancestor) with a valid `id`; references without such an
 * ancestor are skipped, a malformed `blockId` reads as none, and the same target can
 * appear several times for one block. A `pageLink` block (the card, 2026-10-06) counts
 * the same way except that it is the block: it belongs to itself (its own `id`, or the
 * enclosing node's when it has none). The content of a `templateButton` is template material (ADR-0056): none of
 * its references is listed. Mirrors `PageDoc::links`.
 */
export function extractLinks(doc: LoroDoc): PageLink[] {
  const links: PageLink[] = [];
  const visit = (map: LoroMap, enclosing: BlockId | undefined): void => {
    const name = nodeNameOf(map);
    if (name === TEMPLATE_BUTTON) {
      return;
    }
    const from =
      name === PAGE_REF ? enclosing : name === PAGE_LINK ? (idAttribute(map, "id") ?? enclosing) : undefined;
    if (from !== undefined) {
      const ref = pageRefOf(map);
      if (ref !== undefined) {
        const link: PageLink = { fromBlock: from, toPage: ref.pageId };
        if (ref.blockId !== undefined) {
          link.toBlock = ref.blockId;
        }
        links.push(link);
      }
    }
    const inner = idAttribute(map, "id") ?? enclosing;
    for (const child of childrenOf(map)) {
      if (isLoroMap(child) && nodeNameOf(child) !== undefined) {
        visit(child, inner);
      }
    }
  };
  visit(doc.getMap(ROOT), undefined);
  return links;
}

/**
 * Every date mention with a valid `date` and an `id` of its own, in document order, attributed
 * to the nearest enclosing node (strict ancestor) with a valid `id`; a mention without one is
 * skipped. Mirrors `PageDoc::date_mentions`: `time` is the mention's time when valid,
 * `reminderMinutes` its `reminder` when a non-negative integer (see `reminderMinutesOf`), and
 * `fireAtLocal` the fire time (see `reminderFireAtLocal`). The content of a `templateButton` is template material
 * (ADR-0056): none of its mentions is listed, so none ever reminds.
 */
export function extractDateMentions(doc: LoroDoc): PageDateMention[] {
  const mentions: PageDateMention[] = [];
  const visit = (map: LoroMap, enclosing: BlockId | undefined): void => {
    if (nodeNameOf(map) === TEMPLATE_BUTTON) {
      return;
    }
    if (nodeNameOf(map) === DATE_MENTION) {
      const attributes = attributesOf(map);
      const nodeId = idAttribute(map, "id");
      const date = attributes?.get("date");
      if (enclosing !== undefined && nodeId !== undefined && typeof date === "string" && isIsoDate(date)) {
        const rawTime = attributes?.get("time");
        const time = typeof rawTime === "string" && isIsoTime(rawTime) ? rawTime : null;
        const reminderMinutes = reminderMinutesOf(attributes?.get("reminder"));
        const fireAtLocal =
          reminderMinutes === null ? null : reminderFireAtLocal(date, time, reminderMinutes);
        const rawFor = attributes?.get("reminderFor");
        const reminderFor = typeof rawFor === "string" ? canonicalId(rawFor) : undefined;
        mentions.push({
          blockId: enclosing,
          nodeId,
          date,
          time,
          // A reminder whose fire time overflows is no reminder, as in the core.
          reminderMinutes: fireAtLocal === null ? null : reminderMinutes,
          fireAtLocal,
          ...(reminderFor === undefined ? {} : { reminderFor }),
        });
      }
    }
    const inner = idAttribute(map, "id") ?? enclosing;
    for (const child of childrenOf(map)) {
      if (isLoroMap(child) && nodeNameOf(child) !== undefined) {
        visit(child, inner);
      }
    }
  };
  visit(doc.getMap(ROOT), undefined);
  return mentions;
}

/**
 * A person mention attributed to the nearest enclosing node with an id: a row of the derived `person_mentions` table
 * (`kb_model::PersonMention`, ADR-0049).
 */
export interface PagePersonMention {
  /** The nearest ancestor of the mention that carries an `id`. */
  blockId: BlockId;
  /** The mentioned account (canonical form). */
  accountId: string;
  /** The name at insertion. */
  label: string;
}

/**
 * Every person mention with a valid `accountId`, in document order, attributed to the nearest enclosing node (strict
 * ancestor) with a valid `id`; one without such an ancestor is skipped. The content of a `templateButton` is template
 * material (ADR-0056): none of its mentions is listed. Mirrors `PageDoc::person_mentions`.
 */
export function extractPersonMentions(doc: LoroDoc): PagePersonMention[] {
  const mentions: PagePersonMention[] = [];
  const visit = (map: LoroMap, enclosing: BlockId | undefined): void => {
    if (nodeNameOf(map) === TEMPLATE_BUTTON) {
      return;
    }
    if (nodeNameOf(map) === PERSON_MENTION && enclosing !== undefined) {
      const accountId = idAttribute(map, "accountId");
      if (accountId !== undefined) {
        mentions.push({ blockId: enclosing, accountId, label: personMentionLabel(attributesOf(map)?.get("label")) });
      }
    }
    const inner = idAttribute(map, "id") ?? enclosing;
    for (const child of childrenOf(map)) {
      if (isLoroMap(child) && nodeNameOf(child) !== undefined) {
        visit(child, inner);
      }
    }
  };
  visit(doc.getMap(ROOT), undefined);
  return mentions;
}

/** Where `quote` sits in a page document: a block id and the Loro cursors of the start and the end of the words. */
export interface PageTextAnchor {
  blockId: BlockId;
  /** The Loro cursor bytes of the start (`Cursor.encode()`). */
  start: Uint8Array;
  /** The Loro cursor bytes of the end. */
  end: Uint8Array;
}

/**
 * The first place in the document whose text holds `quote` (inside one text run of a node with an `id`): the block and
 * the cursors of the quote's two ends, in the form a comment's anchor stores them. `undefined` when no run holds it. For
 * seeding anchors of the mirror; the editor makes its own from the selection.
 */
export function textAnchorOf(doc: LoroDoc, quote: string): PageTextAnchor | undefined {
  if (quote === "") {
    return undefined;
  }
  let found: PageTextAnchor | undefined;
  const visit = (map: LoroMap, enclosing: BlockId | undefined): void => {
    const inner = idAttribute(map, "id") ?? enclosing;
    for (const child of childrenOf(map)) {
      if (found !== undefined) {
        return;
      }
      if (child instanceof LoroText && inner !== undefined) {
        const text = child.toString();
        const at = text.indexOf(quote);
        if (at >= 0) {
          // The cursors count the same units `indexOf` does.
          const start = child.getCursor(at);
          const end = child.getCursor(at + quote.length);
          if (start !== undefined && end !== undefined) {
            found = { blockId: inner, start: start.encode(), end: end.encode() };
          }
        }
      } else if (isLoroMap(child) && nodeNameOf(child) !== undefined) {
        visit(child, inner);
      }
    }
  };
  visit(doc.getMap(ROOT), undefined);
  return found;
}

/** One word of a page document's text: its text run and where it starts and ends in it (the units `indexOf` counts). */
export interface PageTextWord {
  text: LoroText;
  start: number;
  end: number;
}

/**
 * The words of a page document in document order: the maximal runs of characters that are not white space, in every
 * text run of the tree. The in-memory presence (`./memory-presence`) walks its simulated people's cursors through them,
 * so a cursor is a real Loro cursor of the page's text, as the editor makes.
 */
export function textWordsOf(doc: LoroDoc): PageTextWord[] {
  const words: PageTextWord[] = [];
  const visit = (map: LoroMap): void => {
    for (const child of childrenOf(map)) {
      if (child instanceof LoroText) {
        for (const match of child.toString().matchAll(/\S+/gu)) {
          words.push({ text: child, start: match.index, end: match.index + match[0].length });
        }
      } else if (isLoroMap(child) && nodeNameOf(child) !== undefined) {
        visit(child);
      }
    }
  };
  visit(doc.getMap(ROOT));
  return words;
}

/**
 * Plain text of a block's inlines as the core indexes it (`kb-core`'s FTS body, also the
 * text of a backlink): text runs as they are, page references as the referenced page's
 * title (nothing when it cannot be resolved), hard breaks as a space.
 */
export function inlineText(inlines: readonly PageInline[], titleOf: (pageId: PageId) => string | undefined): string {
  let text = "";
  for (const inline of inlines) {
    if (inline.type === "text") {
      text += inline.text;
    } else if (inline.type === "pageRef") {
      text += titleOf(inline.pageId) ?? "";
    } else {
      text += " ";
    }
  }
  return text;
}

/**
 * Whether `text` is exactly `YYYY-MM-DD` and a real day (the date of a `dateMention`): a year
 * from `0001` to `9999`, a month from `01` to `12`, a day that exists in that month (Gregorian
 * leap years: divisible by 4, except centuries not divisible by 400). Mirrors the Rust check.
 */
export function isIsoDate(text: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/u.exec(text);
  if (match === null) {
    return false;
  }
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (year < 1 || month < 1 || month > 12) {
    return false;
  }
  const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
  let days = 31;
  if (month === 2) {
    days = leap ? 29 : 28;
  } else if ([4, 6, 9, 11].includes(month)) {
    days = 30;
  }
  return day >= 1 && day <= days;
}

/** Whether `text` is exactly `HH:mm` (24 hours: `00:00` to `23:59`), the time of a `dateMention`. */
export function isIsoTime(text: string): boolean {
  return /^([01]\d|2[0-3]):[0-5]\d$/u.test(text);
}

/**
 * What a `dateMention` reads as: its `date`, then a space and its `time` when it has one
 * (`2026-10-07`, `2026-10-07 14:30`); `undefined` unless `date` is a valid date, a malformed
 * `time` is left out. The text index, the backlinks' text and the Markdown export all use it.
 * Mirrors `kb-model`'s reading.
 */
export function dateMentionText(date: unknown, time: unknown): string | undefined {
  if (typeof date !== "string" || !isIsoDate(date)) {
    return undefined;
  }
  return typeof time === "string" && isIsoTime(time) ? `${date} ${time}` : date;
}

/** The largest reminder the derived table keeps (`u32`, in minutes); a bigger one is no reminder. */
export const MAX_REMINDER_MINUTES = 0xffff_ffff;

/**
 * The `reminder` of a `dateMention`: minutes before its moment, a non-negative integer (up to
 * `MAX_REMINDER_MINUTES`). `null` when it is absent, and also when it is not one (negative,
 * fractional, not a number), which readers treat as no reminder. Mirrors `kb-model`.
 */
export function reminderMinutesOf(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_REMINDER_MINUTES
    ? value
    : null;
}

/** The time of day of a mention of a whole day, in minutes since midnight: 09:00. */
export const WHOLE_DAY_MOMENT_MINUTES = 9 * 60;

/**
 * When a reminder fires on the device's wall clock: the mention's moment (`date` at `time`, or at
 * 09:00 when there is no valid `time`) minus `reminderMinutes`, in seconds since
 * 1970-01-01T00:00:00 as that clock reads it, the calendar fields counted as if they were UTC
 * (a mention is a floating local date, ADR-0016, so no time zone enters: a reminder is due when
 * the local clock reaches its moment, ADR-0024). `null` unless `date` is a valid date. Mirrors
 * `kb-model`'s `fire_at_local`; the local time of "now" for a comparison is
 * `Math.floor(Date.now() / 1000) - new Date().getTimezoneOffset() * 60`.
 */
export function reminderFireAtLocal(date: string, time: string | null, reminderMinutes: number): number | null {
  if (!isIsoDate(date)) {
    return null;
  }
  const moment = new Date(0);
  moment.setUTCFullYear(Number(date.slice(0, 4)), Number(date.slice(5, 7)) - 1, Number(date.slice(8, 10)));
  const minutes =
    time !== null && isIsoTime(time)
      ? Number(time.slice(0, 2)) * 60 + Number(time.slice(3, 5))
      : WHOLE_DAY_MOMENT_MINUTES;
  moment.setUTCHours(0, minutes, 0, 0);
  const seconds = moment.getTime() / 1000 - reminderMinutes * 60;
  return Number.isSafeInteger(seconds) ? seconds : null;
}

/**
 * What a `mathInline` reads as: its `latex` attribute when that is a string (empty while the
 * equation is being written); `undefined` otherwise (the node then reads as nothing). The text
 * index, the backlinks' text and the Markdown export all use it. Mirrors `kb-model`'s reading.
 */
export function mathInlineSource(latex: unknown): string | undefined {
  return typeof latex === "string" ? latex : undefined;
}

/**
 * What a `personMention` reads as: its `label` attribute when that is a string (the member's name when the mention was
 * inserted, 1 to 100 characters in a valid document), nothing otherwise. The text index and the text folds use it;
 * Markdown writes it as `@label`.
 */
export function personMentionLabel(label: unknown): string {
  return typeof label === "string" ? label : "";
}

/** A new page document configured for shape v1 (random peer id, nothing in it yet). */
export function newPageDoc(): LoroDoc {
  const doc = new LoroDoc();
  configurePageDoc(doc);
  return doc;
}

// ----- portable JSON -> normalised tree ----------------------------------------------

function isObject(value: unknown): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Uint8Array);
}

function emptyNode(type: string): PortableNode {
  return { type, attrs: undefined, content: [], marks: [], text: undefined };
}

/** Validates and normalises one node of ProseMirror JSON. `path` locates it in error messages (never content). */
function parseNode(value: JsonValue, path: string): PortableNode {
  if (!isObject(value)) {
    throw invalid("content", `${path} is not a node object`);
  }
  const type = value.type;
  if (typeof type !== "string" || type === "") {
    throw invalid("content", `${path}.type must be a non-empty string`);
  }
  if (type === TEXT) {
    const text = value.text;
    if (typeof text !== "string") {
      throw invalid("content", `${path}.text must be a string`);
    }
    return { type, attrs: undefined, content: [], marks: parseMarks(value.marks, `${path}.marks`), text };
  }
  return {
    type,
    attrs: parseAttrs(value.attrs, `${path}.attrs`),
    content: parseContent(value.content, `${path}.content`),
    marks: [],
    text: undefined,
  };
}

function parseAttrs(value: JsonValue | undefined, path: string): Attrs | undefined {
  if (value === undefined || value === null) {
    return undefined;
  }
  if (!isObject(value)) {
    throw invalid("content", `${path} must be an object`);
  }
  return cleanAttrs(value);
}

function parseContent(value: JsonValue | undefined, path: string): PortableNode[] {
  if (value === undefined || value === null) {
    return [];
  }
  if (!Array.isArray(value)) {
    throw invalid("content", `${path} must be an array`);
  }
  return mergeTextRuns(value.map((child, index) => parseNode(child, `${path}[${String(index)}]`)));
}

function parseMarks(value: JsonValue | undefined, path: string): PortableMark[] {
  if (value === undefined || value === null) {
    return [];
  }
  if (!Array.isArray(value)) {
    throw invalid("content", `${path} must be an array`);
  }
  const byType = new Map<string, PortableMark>();
  value.forEach((mark, index) => {
    const at = `${path}[${String(index)}]`;
    if (!isObject(mark) || typeof mark.type !== "string" || mark.type === "") {
      throw invalid("content", `${at} must be a mark object with a type`);
    }
    // One value per mark name, as in a Loro text mark: a repeated name keeps the last one.
    byType.set(mark.type, { type: mark.type, attrs: parseAttrs(mark.attrs, `${at}.attrs`) });
  });
  return sortMarks([...byType.values()]);
}

// ----- Loro -> normalised tree ------------------------------------------------------

// `instanceof` alone would narrow to `LoroMap<any>`; these keep the default `unknown` values.
function isLoroMap(value: unknown): value is LoroMap {
  return value instanceof LoroMap;
}

function isLoroList(value: unknown): value is LoroList {
  return value instanceof LoroList;
}

function nodeNameOf(map: LoroMap): string | undefined {
  const name = map.get(NODE_NAME);
  return typeof name === "string" && name !== "" ? name : undefined;
}

function attributesOf(map: LoroMap): LoroMap | undefined {
  const attributes = map.get(ATTRIBUTES);
  return isLoroMap(attributes) ? attributes : undefined;
}

function childrenOf(map: LoroMap): unknown[] {
  const children = map.get(CHILDREN);
  return isLoroList(children) ? children.toArray() : [];
}

/**
 * An id as `kb-types` parses it: exactly 32 hex digits once hyphens are ignored, given
 * back in the canonical lowercase 8-4-4-4-12 form; anything else is no id.
 */
export function canonicalId(text: string): string | undefined {
  const hex = text.replaceAll("-", "");
  if (!/^[0-9a-f]{32}$/iu.test(hex)) {
    return undefined;
  }
  const digits = hex.toLowerCase();
  return [digits.slice(0, 8), digits.slice(8, 12), digits.slice(12, 16), digits.slice(16, 20), digits.slice(20)].join(
    "-",
  );
}

/** An attribute holding an id (block id, page id); malformed ids read as none. */
function idAttribute(map: LoroMap, key: string): string | undefined {
  const value = attributesOf(map)?.get(key);
  return typeof value === "string" ? canonicalId(value) : undefined;
}

function pageRefOf(map: LoroMap): { pageId: PageId; blockId: BlockId | undefined } | undefined {
  const pageId = idAttribute(map, "pageId");
  return pageId === undefined ? undefined : { pageId, blockId: idAttribute(map, "blockId") };
}

/** The `pageRef` inline for a reference (no `blockId` key when it names none). */
function pageRefInline(ref: { pageId: PageId; blockId: BlockId | undefined }): PageInline {
  return ref.blockId === undefined
    ? { type: "pageRef", pageId: ref.pageId }
    : { type: "pageRef", pageId: ref.pageId, blockId: ref.blockId };
}

/**
 * Consecutive text (several `LoroText`s in a row included) is one text inline; any other
 * element child ends the run, whether it is a reference, a break or a nested node, except a
 * date mention, which is part of it as its ISO text, and an inline equation, as its source.
 */
function directInlines(children: readonly unknown[]): PageInline[] {
  const inlines: PageInline[] = [];
  let text = "";
  const flush = (): void => {
    if (text !== "") {
      inlines.push({ type: "text", text });
      text = "";
    }
  };
  for (const child of children) {
    if (child instanceof LoroText) {
      text += child.toString();
      continue;
    }
    const name = isLoroMap(child) ? nodeNameOf(child) : undefined;
    if (!isLoroMap(child) || name === undefined) {
      continue; // not a node: skipped without ending the text run
    }
    if (name === DATE_MENTION) {
      // Reads as text, in the run around it: `Meeting 2026-10-07 at the office`.
      const attributes = attributesOf(child);
      text += dateMentionText(attributes?.get("date"), attributes?.get("time")) ?? "";
      continue;
    }
    if (name === MATH_INLINE) {
      // Reads as its LaTeX source, in the run around it: `Energy E=mc^2 is`.
      text += mathInlineSource(attributesOf(child)?.get("latex")) ?? "";
      continue;
    }
    if (name === PERSON_MENTION) {
      // Reads as its label, in the run around it, like the core's `PageDoc::blocks`: `Ask Bob about it`.
      text += personMentionLabel(attributesOf(child)?.get("label"));
      continue;
    }
    flush();
    if (name === PAGE_REF) {
      const ref = pageRefOf(child);
      if (ref !== undefined) {
        inlines.push(pageRefInline(ref));
      }
    } else if (name === HARD_BREAK) {
      inlines.push({ type: "break" });
    }
  }
  flush();
  return inlines;
}

/** Reads an element map; `fallback` names the root when it has no `nodeName` yet. */
function readNode(map: LoroMap, fallback?: string): PortableNode | undefined {
  const type = nodeNameOf(map) ?? fallback;
  // Only a `LoroText` can be text; a map that claims to be text is malformed.
  if (type === undefined || type === TEXT) {
    return undefined;
  }
  const raw: unknown = attributesOf(map)?.toJSON();
  return {
    type,
    attrs: isObject(raw) ? cleanAttrs(raw) : undefined,
    content: readChildren(childrenOf(map)),
    marks: [],
    text: undefined,
  };
}

function readChildren(children: readonly unknown[]): PortableNode[] {
  const nodes: PortableNode[] = [];
  for (const child of children) {
    if (child instanceof LoroText) {
      for (const run of child.toDelta()) {
        if (typeof run.insert === "string") {
          nodes.push({ type: TEXT, attrs: undefined, content: [], marks: marksOf(run.attributes), text: run.insert });
        }
      }
    } else if (isLoroMap(child)) {
      const node = readNode(child);
      if (node !== undefined) {
        nodes.push(node);
      }
    }
  }
  return mergeTextRuns(nodes);
}

/** Text marks of one delta run: unmarked (`null`) styles are absent; non-object values are marks without attributes. */
function marksOf(attributes: { [key: string]: Value } | undefined): PortableMark[] {
  const marks: PortableMark[] = [];
  for (const [type, value] of Object.entries(attributes ?? {})) {
    if (value === null || value === undefined) {
      continue;
    }
    marks.push({ type, attrs: isObject(value) ? cleanAttrs(value) : undefined });
  }
  return sortMarks(marks);
}

// ----- normalised tree -> Loro / JSON ------------------------------------------------

/** Writes `node` into `map` in the binding's order: name, attributes, children. */
function writeNode(map: LoroMap, node: PortableNode, isRoot: boolean): void {
  map.set(NODE_NAME, node.type);
  // The root's containers are mergeable (deterministic ids, see the module comment);
  // element nodes get regular ones, as the binding creates them.
  const attributes = isRoot ? map.ensureMergeableMap(ATTRIBUTES) : map.setContainer(ATTRIBUTES, new LoroMap());
  for (const [key, value] of Object.entries(node.attrs ?? {})) {
    attributes.set(key, value);
  }
  const children = isRoot ? map.ensureMergeableList(CHILDREN) : map.setContainer(CHILDREN, new LoroList());
  let run: PortableNode[] = [];
  const flush = (): void => {
    if (run.length > 0) {
      const text = children.insertContainer(children.length, new LoroText());
      text.applyDelta(
        run.map((textNode): Delta<string> => ({ insert: textNode.text ?? "", attributes: markValues(textNode.marks) })),
      );
      run = [];
    }
  };
  for (const child of node.content) {
    if (child.type === TEXT) {
      run.push(child);
    } else {
      flush();
      writeNode(children.insertContainer(children.length, new LoroMap()), child, false);
    }
  }
  flush();
}

function markValues(marks: readonly PortableMark[]): { [key: string]: Value } {
  const values: { [key: string]: Value } = {};
  for (const mark of marks) {
    values[mark.type] = mark.attrs ?? {};
  }
  return values;
}

function toJson(node: PortableNode): JsonValue {
  const json: { [key: string]: JsonValue } = { type: node.type };
  if (node.attrs !== undefined) {
    json.attrs = node.attrs;
  }
  if (node.content.length > 0) {
    json.content = node.content.map(toJson);
  }
  if (node.marks.length > 0) {
    json.marks = node.marks.map((mark) => {
      const markJson: { [key: string]: JsonValue } = { type: mark.type };
      if (mark.attrs !== undefined) {
        markJson.attrs = mark.attrs;
      }
      return markJson;
    });
  }
  if (node.text !== undefined) {
    json.text = node.text;
  }
  return json;
}

// ----- shared normalisation ---------------------------------------------------------

/** Drops empty text nodes and joins adjacent text nodes that carry the same marks. */
function mergeTextRuns(nodes: readonly PortableNode[]): PortableNode[] {
  const merged: PortableNode[] = [];
  for (const node of nodes) {
    if (node.type !== TEXT) {
      merged.push(node);
      continue;
    }
    if (node.text === undefined || node.text === "") {
      continue;
    }
    const last = merged.at(-1);
    if (last?.type === TEXT && sameMarks(last.marks, node.marks)) {
      merged[merged.length - 1] = { ...last, text: `${last.text ?? ""}${node.text}` };
    } else {
      merged.push(node);
    }
  }
  return merged;
}

function sameMarks(a: readonly PortableMark[], b: readonly PortableMark[]): boolean {
  const key = (marks: readonly PortableMark[]): string =>
    canonicalText(marks.map((mark) => [mark.type, mark.attrs ?? null]));
  return key(a) === key(b);
}

/** Marks of the v1 table first, in table order, then any other mark by name. */
function sortMarks(marks: PortableMark[]): PortableMark[] {
  const rank = (mark: PortableMark): number => MARK_RANK.get(mark.type) ?? PAGE_MARKS.length;
  return marks.sort((a, b) => rank(a) - rank(b) || compareStrings(a.type, b.type));
}

/** Non-null attributes, keys sorted, values copied; `undefined` when nothing remains. */
function cleanAttrs(raw: { [key: string]: unknown }): Attrs | undefined {
  const attrs: Attrs = {};
  for (const key of Object.keys(raw).sort(compareStrings)) {
    const value = raw[key];
    if (value === null || value === undefined) {
      continue;
    }
    const copy = toJsonValue(value);
    if (copy !== undefined) {
      attrs[key] = copy;
    }
  }
  return Object.keys(attrs).length > 0 ? attrs : undefined;
}

/** A deep copy as plain JSON with object keys sorted; `undefined` for values JSON cannot hold. */
function toJsonValue(value: unknown): JsonValue | undefined {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    return Number.isFinite(value) ? value : null;
  }
  if (typeof value === "bigint") {
    return Number(value);
  }
  if (value instanceof Uint8Array) {
    return Array.from(value);
  }
  if (Array.isArray(value)) {
    return value.map((item) => toJsonValue(item) ?? null);
  }
  if (typeof value === "object") {
    const copy: { [key: string]: JsonValue } = {};
    const record = value as { [key: string]: unknown };
    for (const key of Object.keys(record).sort(compareStrings)) {
      const item = toJsonValue(record[key]);
      if (item !== undefined) {
        copy[key] = item;
      }
    }
    return copy;
  }
  return undefined;
}

/** JSON text with object keys sorted at every level, for comparing values. */
function canonicalText(value: unknown): string {
  return JSON.stringify(toJsonValue(value) ?? null);
}

function compareStrings(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
