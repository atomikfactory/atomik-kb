// The content of a page copy (ADR-0040, "Duplicate a page with its sub-pages"): the twin of
// `kb-model`'s `page/remap.rs`. A copy of a page is built from the portable JSON of the original after
// its ids were replaced here:
//
// - every node with a valid `attrs.id` gets the new id `CopyIds.blocks` holds for it, so the copy shares
//   no block id with the original;
// - a `pageRef` or `pageLink` that points to a page of the copied subtree points to the page's copy, and
//   its `blockId` (the block it names) to the copy of that block when the block was copied; a link to any
//   other page is left exactly as it is;
// - the `whiteboards` root map follows its blocks: the elements of a block are stored under the block's
//   new id (the elements themselves, and their own ids, are unchanged);
// - a `pageList` whose view names a `defaultTemplate` inside the copied subtree (ADR-0056) names the copy of that page;
// - a `pageList` whose `source` is `{ "kind": "page", "pageId" }` (ADR-0058) with a `pageId` inside the copied subtree
//   names the copy of that page; any other source (children, tag, space, a page outside the subtree) is kept.
//
// A page made from a template (ADR-0056, `docs/architecture/templates.md` section 5) goes through a second pass,
// `fillTemplateContent`: a `dateMention` marked `relative` becomes the day (and for `now` the time) of the creation, a
// `personMention` marked `relative: "me"` the account that creates the page; a `templateButton`'s content is left exactly
// as it is (its relative mentions are resolved when the button is clicked).
//
// The functions are pure: they read and write JSON, never a document, and never look at text, marks or
// attributes other than the ones above. Ids are compared in their canonical form (lowercase, hyphenated).

import { civilFromDays } from "./formula/dates";
import type { AccountId } from "./generated/AccountId";
import type { BlockId } from "./generated/BlockId";
import type { PageId } from "./generated/PageId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { canonicalId } from "./page-doc";
import { WHITEBOARDS } from "./whiteboard";

/** Deepest nesting followed (the readers of the document stop there too). */
const MAX_DEPTH = 128;
/** The longest `personMention` label, in characters. */
const MAX_LABEL_CHARS = 100;

/** The ids a copy of a subtree gets: the page copies and the block copies, by original id (canonical). */
export interface CopyIds {
  /** The copy of each page of the copied subtree. */
  readonly pages: ReadonlyMap<PageId, PageId>;
  /** The copy of each block of the copied pages (every node with an id). */
  readonly blocks: ReadonlyMap<BlockId, BlockId>;
}

type JsonObject = { [key: string]: JsonValue | undefined };

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The canonical id an attribute holds, when it holds a valid one. */
function idAttr(attrs: JsonObject, key: string): string | undefined {
  const value = attrs[key];
  return typeof value === "string" ? canonicalId(value) : undefined;
}

/**
 * The ids of the nodes of portable page JSON (`attrs.id` that is a valid id), in document order, each once
 * (a duplicated id, which concurrent edits can leave, is listed once).
 */
export function portableBlockIds(json: JsonValue): BlockId[] {
  const seen = new Set<BlockId>();
  const out: BlockId[] = [];
  const visit = (node: JsonValue | undefined, depth: number): void => {
    if (!isObject(node) || depth > MAX_DEPTH) {
      return;
    }
    const id = isObject(node.attrs) ? idAttr(node.attrs, "id") : undefined;
    if (id !== undefined && !seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
    if (Array.isArray(node.content)) {
      for (const child of node.content) {
        visit(child, depth + 1);
      }
    }
  };
  visit(json, 0);
  return out;
}

function remapNode(node: JsonValue, ids: CopyIds, depth: number): JsonValue {
  if (!isObject(node) || depth > MAX_DEPTH) {
    return node;
  }
  const out: JsonObject = { ...node };
  if (isObject(node.attrs)) {
    const attrs: JsonObject = { ...node.attrs };
    const id = idAttr(attrs, "id");
    const copy = id === undefined ? undefined : ids.blocks.get(id);
    if (copy !== undefined) {
      attrs.id = copy;
    }
    if (node.type === "pageRef" || node.type === "pageLink") {
      const page = idAttr(attrs, "pageId");
      const pageCopy = page === undefined ? undefined : ids.pages.get(page);
      if (pageCopy !== undefined) {
        attrs.pageId = pageCopy;
        const block = idAttr(attrs, "blockId");
        const blockCopy = block === undefined ? undefined : ids.blocks.get(block);
        if (blockCopy !== undefined) {
          attrs.blockId = blockCopy;
        }
      }
    }
    if (node.type === "pageList" && Array.isArray(attrs.views)) {
      attrs.views = attrs.views.map((view): JsonValue => {
        if (!isObject(view)) {
          return view;
        }
        const template = idAttr(view, "defaultTemplate");
        const templateCopy = template === undefined ? undefined : ids.pages.get(template);
        return templateCopy === undefined ? view : { ...view, defaultTemplate: templateCopy };
      });
    }
    if (node.type === "pageList" && isObject(attrs.source) && attrs.source.kind === "page") {
      const source = idAttr(attrs.source, "pageId");
      const sourceCopy = source === undefined ? undefined : ids.pages.get(source);
      if (sourceCopy !== undefined) {
        attrs.source = { ...attrs.source, pageId: sourceCopy };
      }
    }
    out.attrs = attrs as JsonValue;
  }
  if (Array.isArray(node.content)) {
    out.content = node.content.map((child) => remapNode(child, ids, depth + 1));
  }
  return out as JsonValue;
}

/**
 * The portable JSON of the copy of a page: `json` with the ids replaced as the module comment says. Ids the
 * maps do not know stay as they are. The input is not modified.
 */
export function remapPortable(json: JsonValue, ids: CopyIds): JsonValue {
  const out = remapNode(json, ids, 0);
  if (isObject(out) && isObject(out[WHITEBOARDS])) {
    const boards: JsonObject = {};
    for (const [key, elements] of Object.entries(out[WHITEBOARDS])) {
      const block = canonicalId(key);
      boards[(block === undefined ? undefined : ids.blocks.get(block)) ?? key] = elements;
    }
    out[WHITEBOARDS] = boards as JsonValue;
  }
  return out;
}

// ----- a page made from a template (ADR-0056) -----------------------------------------------------------------------

/** The local day and time of the moment a page is made from a template. */
export interface LocalMoment {
  /** The local day, `YYYY-MM-DD`. */
  readonly date: string;
  /** The local time of day, `HH:mm`. */
  readonly time: string;
}

/** The local day and time of `nowMs` on a device whose offset from UTC is `offsetSeconds` (east positive): `LocalMoment::new`. */
export function localMoment(nowMs: number, offsetSeconds: number): LocalMoment {
  const local = Math.floor(nowMs / 1000) + offsetSeconds;
  const days = Math.floor(local / 86_400);
  const ofDay = local - days * 86_400;
  const { y, m, d } = civilFromDays(days);
  const two = (n: number): string => n.toString().padStart(2, "0");
  return {
    date: `${y.toString().padStart(4, "0")}-${two(m)}-${two(d)}`,
    time: `${two(Math.floor(ofDay / 3_600))}:${two(Math.floor((ofDay % 3_600) / 60))}`,
  };
}

/**
 * `title` with every `{{date}}` replaced by the day and every `{{time}}` by the time (case sensitive, no other token, no
 * escaping), cut to `maxChars` characters when the tokens made it longer: `LocalMoment::resolve_title`.
 */
export function resolveTitleTokens(title: string, moment: LocalMoment, maxChars: number): string {
  if (!title.includes("{{")) {
    return title;
  }
  const resolved = title.replaceAll("{{date}}", moment.date).replaceAll("{{time}}", moment.time);
  const letters = Array.from(resolved);
  return letters.length <= maxChars ? resolved : letters.slice(0, maxChars).join("").trimEnd();
}

/** The person `me` stands for in a page made from a template. */
export interface TemplateMe {
  /** The account that makes the page. */
  readonly account: AccountId;
  /** Its display name, when known. */
  readonly label: string | undefined;
}

/** What `fillTemplateContent` resolves relative mentions to. */
export interface TemplateFill {
  /** The local day and time of the creation. */
  readonly moment: LocalMoment;
  /** The account that makes the page; `undefined` leaves a `me` mention with the account and label it was written with. */
  readonly me: TemplateMe | undefined;
}

function fillDateMention(attrs: JsonObject, moment: LocalMoment): void {
  const relative = attrs.relative;
  if (relative !== "today" && relative !== "now") {
    return;
  }
  delete attrs.relative;
  attrs.date = moment.date;
  if (relative === "now") {
    attrs.time = moment.time;
  }
}

function fillPersonMention(attrs: JsonObject, me: TemplateMe | undefined): void {
  if (attrs.relative !== "me") {
    return;
  }
  delete attrs.relative;
  if (me === undefined) {
    return;
  }
  attrs.accountId = me.account;
  const label = me.label?.trim() ?? "";
  if (label !== "") {
    attrs.label = Array.from(label).slice(0, MAX_LABEL_CHARS).join("");
  }
}

function fillNode(node: JsonValue, fill: TemplateFill, depth: number): JsonValue {
  if (!isObject(node) || depth > MAX_DEPTH || node.type === "templateButton") {
    return node;
  }
  const out: JsonObject = { ...node };
  if ((node.type === "dateMention" || node.type === "personMention") && isObject(node.attrs)) {
    const attrs: JsonObject = { ...node.attrs };
    if (node.type === "dateMention") {
      fillDateMention(attrs, fill.moment);
    } else {
      fillPersonMention(attrs, fill.me);
    }
    out.attrs = attrs as JsonValue;
  }
  if (Array.isArray(node.content)) {
    out.content = node.content.map((child) => fillNode(child, fill, depth + 1));
  }
  return out as JsonValue;
}

/**
 * The portable JSON of a page made from a template: `json` with its relative mentions resolved (see the module comment). A
 * `dateMention` whose `relative` is `today` gets `date` = the local day, one whose `relative` is `now` also `time` = the local
 * time, and both lose `relative`; a `personMention` whose `relative` is `me` gets `accountId` = me and `label` = my display
 * name when known, and loses `relative`. Any other value of `relative` (a newer build's) is left as it is, and so is everything
 * inside a `templateButton`. The input is not modified.
 */
export function fillTemplateContent(json: JsonValue, fill: TemplateFill): JsonValue {
  return fillNode(json, fill, 0);
}
