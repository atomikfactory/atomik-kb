// The in-memory twin of the core's Milestone 3 content side (`kb-core`'s `pages.rs`,
// `docs/development/milestone-03.md`, "Core"): one Loro page document per page, the binary
// document sessions (`page_doc_open/import/export/close`), and what the core derives from
// the documents for its `fts` and `links` tables, behind `Search`, `BacklinksGet`,
// `PageContentGet`, `PageExportMarkdown` and `IndexRebuild`. `MemoryTransport` maps
// requests and `Transport.docs` onto it; titles, icons, spaces and trash state come from
// the structure mirror.

import { VersionVector } from "loro-crdt";
import type { LoroDoc, OpId } from "loro-crdt";

import { toBase64 } from "./backup";
import { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { AttachmentId } from "./generated/AttachmentId";
import type { Backlink } from "./generated/Backlink";
import type { ChangeSource } from "./generated/ChangeSource";
import type { CommentId } from "./generated/CommentId";
import type { Event } from "./generated/Event";
import type { PageContent } from "./generated/PageContent";
import type { PageExcerpt } from "./generated/PageExcerpt";
import type { PageFirstImage } from "./generated/PageFirstImage";
import type { PageId } from "./generated/PageId";
import type { ReminderDue } from "./generated/ReminderDue";
import type { SearchHit } from "./generated/SearchHit";
import type { SearchScope } from "./generated/SearchScope";
import type { SpaceId } from "./generated/SpaceId";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { renderMarkdown } from "./markdown";
import {
  attachmentRefsOf,
  configurePageDoc,
  editPageListAttrs,
  extractBlocks,
  extractDateMentions,
  extractLinks,
  extractPersonMentions,
  inlineText,
  loroToPortable,
  newPageDoc,
  portableToLoro,
  textAnchorOf,
} from "./page-doc";
import type { Attrs, PageBlock, PageDateMention, PageLink, PagePersonMention } from "./page-doc";
import { matchText, parseDistinctQuery, parseQuery } from "./search";
import type { TextMatch } from "./search";
import { rankFuzzyTitles } from "./search-fuzzy";
import type { MemoryStructure, PageMeta } from "./structure";
import { displayName, invalid } from "./validate";

/** Largest update `import` accepts (`kb_core::MAX_DOC_UPDATE_BYTES`, 16 MiB). */
const MAX_UPDATE_BYTES = 16 * 1024 * 1024;
const MAX_QUERY_CHARS = 200;
const DEFAULT_SEARCH_LIMIT = 20;
const MAX_SEARCH_LIMIT = 100;
/** Longest `Backlink.text`, in characters. */
const MAX_BACKLINK_CHARS = 200;
/** Longest `ReminderDue.text`, in characters. */
const MAX_REMINDER_TEXT_CHARS = 200;
/** Most pages one `PageFirstImages` request asks about (ADR-0054). */
const MAX_FIRST_IMAGE_PAGES = 500;
/** Most pages one `PageExcerpts` request asks about (ADR-0058). */
const MAX_EXCERPT_PAGES = 500;
/** Most characters (Unicode scalar values) of one excerpt, before the `…` of a cut (ADR-0058). */
export const EXCERPT_CHARS = 280;

/** What Rust's `char::is_whitespace` counts as whitespace (not the BOM, which `\s` of JavaScript adds). */
const WHITESPACE = "\\t-\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const WHITESPACE_RUN = new RegExp(`[${WHITESPACE}]+`, "gu");
const WHITESPACE_CHAR = new RegExp(`[${WHITESPACE}]`, "u");
/**
 * What the core's text index stores in place of its two highlight markers (`U+E000`, `U+E001`) and what it puts for inline atoms
 * (`U+FFFD`): none of them is text of an excerpt. The twin keeps the markers as they are, so it drops them too.
 */
const INDEX_MARKS = /[�]/gu;

/**
 * One indexed block as a line of an excerpt (ADR-0058, section 6.2): the index marks removed, every run of whitespace one
 * space, trimmed; `undefined` when nothing is left. The core's `excerpt_row`.
 */
export function excerptRow(row: string): string | undefined {
  const line = row.replace(INDEX_MARKS, "").replace(WHITESPACE_RUN, " ").replace(/^ /u, "").replace(/ $/u, "");
  return line === "" ? undefined : line;
}

/**
 * The excerpt of the lines of a page joined with a line break: the text itself when it has at most {@link EXCERPT_CHARS}
 * Unicode scalar values, else cut at the last whitespace among the first {@link EXCERPT_CHARS} (at the limit when there is none)
 * with `…` appended. The core's `cut_excerpt`.
 */
export function cutExcerpt(text: string): string {
  const chars = Array.from(text);
  if (chars.length <= EXCERPT_CHARS) {
    return text;
  }
  const head = chars.slice(0, EXCERPT_CHARS);
  let end = head.length;
  for (let at = head.length - 1; at >= 0; at -= 1) {
    if (WHITESPACE_CHAR.test(head[at] ?? "")) {
      end = at;
      break;
    }
  }
  // A cut at the very start would leave only the mark: keep the limit then.
  return `${head.slice(0, end === 0 ? head.length : end).join("")}…`;
}

/**
 * The attachment of the first picture in document order: the first `image` block, or the first item of the first
 * `gallery` block, whichever comes first, at any depth. Blocks without a valid id or attachment id are not in
 * `blocks`; `file` blocks and whiteboards are no picture, nor is anything inside a template button (ADR-0056). Mirrors
 * `PageDoc::first_image`.
 */
function firstImageOf(blocks: readonly PageBlock[]): AttachmentId | undefined {
  for (const block of blocks) {
    if (block.templateContent === true) {
      continue; // template material (ADR-0056)
    }
    if (block.kind === "image" && block.attachmentId !== undefined) {
      return block.attachmentId;
    }
    const first = block.kind === "gallery" ? block.items?.[0] : undefined;
    if (first !== undefined) {
      return first.attachmentId;
    }
  }
  return undefined;
}

/** What the content mirror needs from its owner. */
export interface ContentDeps {
  /** Id generator (UUID text), for the first paragraph of a new page document. */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Titles, icons, spaces and trash state of pages. */
  structure: MemoryStructure;
  /**
   * The name of a finished attachment (`undefined` when unknown or purged): the label of a
   * `file` block in Markdown. Without it files show their attachment id.
   */
  attachmentName?: (attachmentId: AttachmentId) => string | undefined;
  /**
   * Told after a document changed and was indexed, before its events, with the attachments
   * the page referenced before or references now (the core's reference bookkeeping).
   */
  documentChanged?: (touched: AttachmentId[]) => void;
  /**
   * Told after a page's document was written (the editor's update, a core write, a change that arrived from elsewhere) and
   * indexed, before the events (ADR-0057, the journal of `./memory-history`): `before` is the document's version vector
   * (peer to end counter) from before the write. Not told for a restore, which the history records itself.
   */
  documentWritten?: (pageId: PageId, source: ChangeSource, before: ReadonlyMap<string, number>) => void;
  /**
   * The texts the text index holds for a page's properties (ADR-0026): one per property that has
   * some, in schema order. Without it no property is searched.
   */
  propertyTexts?: (spaceId: SpaceId, pageId: PageId) => readonly string[];
  /**
   * How many `files` property values list each attachment (ADR-0026, amendment of 2026-10-07):
   * references of the attachment like the blocks of the documents, added to their counts. Without
   * it only blocks reference attachments.
   */
  propertyReferences?: () => ReadonlyMap<AttachmentId, number>;
  /**
   * How many pages have each attachment as their icon (ADR-0029, `attachment:<id>`): one reference
   * per page, the trashed pages included, like the `icon_refs` rows of the core. Added to the counts
   * like `propertyReferences`. Without it only blocks reference attachments.
   */
  iconReferences?: () => ReadonlyMap<AttachmentId, number>;
  /**
   * A page's content was edited by the local editor (`source` `"Editor"`): the owner stamps the
   * page's last edit (ADR-0026, amendment of 2026-10-08). Called after the content events.
   */
  documentEdited?: (pageId: PageId) => void;
  /**
   * The texts of a page's comments, for the text index (M13 slice 2, ADR-0048): one per comment that was not deleted. A hit
   * on one is a page-level hit that carries the comment's id (the UI labels it "in comment"). Without it no comment is searched.
   */
  commentTexts?: (spaceId: SpaceId, pageId: PageId) => readonly { commentId: CommentId; text: string }[];
  /**
   * Whether this device fires a reminder of a date mention in a space (ADR-0047, `Core::fires_reminder`): a personal space's always;
   * a shared space's on the devices of `reminderFor` when the mention names an account (every role), and on an Owner's device when
   * it names nobody. Without it every reminder fires.
   */
  firesReminder?: (spaceId: SpaceId, reminderFor: AccountId | undefined) => boolean;
}

/** The `data` of a `Search` request. */
export interface SearchInput {
  query: string;
  spaceId?: SpaceId;
  scope?: SearchScope;
  limit?: number;
  /** Only pages last edited at or after this time (Unix ms). */
  editedAfter?: number;
}

/** A page document as a backup of the mirror carries it: portable JSON. */
export interface DocumentState {
  pageId: PageId;
  content: JsonValue;
}

/** A page document with what the core derives from it: its block rows, link rows and date mentions. */
interface PageRecord {
  doc: LoroDoc;
  blocks: PageBlock[];
  links: PageLink[];
  /** The `date_mentions` rows of the page (ADR-0024). */
  dateMentions: PageDateMention[];
  /** The `person_mentions` rows of the page (ADR-0049). */
  personMentions: PagePersonMention[];
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** Byte order of 16-byte ids equals the order of their lowercase hex text. */
function compareStrings(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** `limit` is a `u32` in the contract; the core clamps it to 1..=100. */
function searchLimit(raw: number | undefined): number {
  if (raw === undefined) {
    return DEFAULT_SEARCH_LIMIT;
  }
  if (!Number.isInteger(raw) || raw < 0) {
    throw invalid("limit", "must be a non-negative integer");
  }
  return Math.min(MAX_SEARCH_LIMIT, Math.max(1, raw));
}

/** `scope` is an enum in the contract; only untyped callers can send anything else. */
function searchScope(raw: SearchScope | undefined): SearchScope {
  const scope: unknown = raw ?? "All";
  if (scope !== "All" && scope !== "Titles") {
    throw invalid("scope", "must be All or Titles");
  }
  return scope;
}

/** `editedAfter` is an `i64` of Unix milliseconds in the contract; only untyped callers can send anything else. */
function searchEditedAfter(raw: number | undefined): number | undefined {
  if (raw !== undefined && !Number.isFinite(raw)) {
    throw invalid("editedAfter", "must be a number of milliseconds");
  }
  return raw;
}

/** Keeps the first `max` characters (code points, not UTF-16 units). */
function truncateChars(text: string, max: number): string {
  const chars = Array.from(text);
  return chars.length <= max ? text : chars.slice(0, max).join("");
}

function targetsOf(links: readonly PageLink[]): Set<PageId> {
  return new Set(links.map((link) => link.toPage));
}

/**
 * Page documents and their derived index. Methods throw `CoreError`s where the core
 * answers with an `ApiError` and emit events in the core's order, after the change.
 */
export class MemoryContent {
  readonly #deps: ContentDeps;
  /** Documents by page id. Purged pages keep theirs (see {@link MemoryContent.pagesRemoved}). */
  readonly #pages = new Map<PageId, PageRecord>();

  constructor(deps: ContentDeps) {
    this.#deps = deps;
  }

  // ----- document sessions (Transport.docs) -----------------------------------------

  /** The raw Loro snapshot; creates the document (one empty paragraph) on first use. */
  open(pageId: PageId): Uint8Array {
    this.#requirePage(pageId);
    return this.#record(pageId).doc.export({ mode: "snapshot" });
  }

  /**
   * Imports raw Loro bytes from the editor, derives blocks and links again, then emits
   * `PageContentChanged { source: "Editor" }` and `BacklinksChanged` for every page that
   * this page started or stopped referencing.
   */
  import(pageId: PageId, update: Uint8Array): void {
    if (update.length === 0) {
      throw invalid("update", "must not be empty");
    }
    if (update.length > MAX_UPDATE_BYTES) {
      throw invalid("update", `must be at most ${String(MAX_UPDATE_BYTES)} bytes`);
    }
    this.#requirePage(pageId);
    const record = this.#record(pageId);
    const before = record.doc.oplogVersion().toJSON();
    try {
      record.doc.import(update);
    } catch (error) {
      throw invalid("update", `rejected: ${error instanceof Error ? error.message : String(error)}`);
    }
    this.#reindexAndNotify(pageId, record, "Editor", before);
  }

  /**
   * What the document has beyond the encoded version vector `since` (everything when it
   * is absent or empty), as a raw Loro update; an empty array when `since` already
   * includes everything.
   */
  export(pageId: PageId, since: Uint8Array | undefined): Uint8Array {
    this.#requirePage(pageId);
    const { doc } = this.#record(pageId);
    if (since === undefined || since.length === 0) {
      return doc.export({ mode: "update" });
    }
    let from: VersionVector;
    try {
      from = VersionVector.decode(since);
    } catch {
      throw invalid("since", "not a valid version vector");
    }
    const order = doc.oplogVersion().compare(from);
    if (order === 0 || order === -1) {
      return new Uint8Array();
    }
    return doc.export({ mode: "update", from });
  }

  // ----- seeding, test hooks, structure duties --------------------------------------

  /** Builds a page's document from portable JSON before the app "starts" (no events). */
  seed(pageId: PageId, content: JsonValue): void {
    this.#pages.set(pageId, this.#build(content));
  }

  // ----- keyring counts and backups ---------------------------------------------------

  /** Pages that have a document (created on first use or seeded); purged pages keep theirs. */
  documentPageIds(): PageId[] {
    return [...this.#pages.keys()];
  }

  /** The documents of those of `pageIds` that have one, in that order, as portable JSON. */
  exportDocuments(pageIds: Iterable<PageId>): DocumentState[] {
    const found: DocumentState[] = [];
    for (const pageId of pageIds) {
      const record = this.#pages.get(pageId);
      if (record !== undefined) {
        found.push({ pageId, content: loroToPortable(record.doc) });
      }
    }
    return found;
  }

  /**
   * Builds documents from portable JSON (throwing like `portableToLoro` on malformed content)
   * and returns the function that installs them in place of every document, without events.
   * Nothing changes until it is called.
   */
  prepareRestore(documents: readonly DocumentState[]): () => void {
    const built = new Map<PageId, PageRecord>();
    for (const { pageId, content } of documents) {
      built.set(pageId, this.#build(content));
    }
    return () => {
      this.#pages.clear();
      for (const [pageId, record] of built) {
        this.#pages.set(pageId, record);
      }
    };
  }

  /** Changes a page's document as another writer would, then indexes and notifies with `source`. */
  change(pageId: PageId, edit: (doc: LoroDoc) => void, source: ChangeSource): void {
    this.#requirePage(pageId);
    const record = this.#record(pageId);
    const before = record.doc.oplogVersion().toJSON();
    edit(record.doc);
    record.doc.commit();
    this.#reindexAndNotify(pageId, record, source, before);
  }

  /**
   * The page's document read at `frontiers` (a version of its history, ADR-0057): the document forked there, as portable
   * JSON. `InvalidInput` (`version`) while the page has no document; the caller has checked that the changes exist.
   */
  contentAt(pageId: PageId, frontiers: readonly OpId[]): JsonValue {
    this.#requirePage(pageId);
    const doc = this.#pages.get(pageId)?.doc;
    if (doc === undefined) {
      throw invalid("version", "the page has no such version");
    }
    const fork = doc.forkAt([...frontiers]);
    configurePageDoc(fork);
    return loroToPortable(fork);
  }

  /**
   * Makes the page's document what it was at `frontiers` (ADR-0057, Loro's `revertTo`) as one new change on its own replica,
   * indexed and announced like a core write (`PageContentChanged { source: "Core" }`, `BacklinksChanged`, the attachment
   * bookkeeping). The journal is not told: the history records the restore itself. Answers whether anything was written (a
   * version whose state is the current one writes nothing and announces nothing) and the version vector from before.
   * `InvalidInput` (`version`) while the page has no document.
   */
  revertTo(pageId: PageId, frontiers: readonly OpId[]): { changed: boolean; before: ReadonlyMap<string, number> } {
    this.#requirePage(pageId);
    const record = this.#pages.get(pageId);
    if (record === undefined) {
      throw invalid("version", "the page has no such version");
    }
    const before = record.doc.oplogVersion().toJSON();
    const opsBefore = record.doc.opCount();
    record.doc.revertTo([...frontiers]);
    record.doc.commit();
    const changed = record.doc.opCount() !== opsBefore;
    if (changed) {
      this.#reindexAndNotify(pageId, record, "Core", before, false);
    }
    return { changed, before };
  }

  /**
   * The mirror's document of a page, `undefined` when it has none yet (none is made). The simulated people of the presence
   * mirror take their cursors from it (Milestone 13, slice 3); nothing writes through it.
   */
  docOf(pageId: PageId): LoroDoc | undefined {
    return this.#pages.get(pageId)?.doc;
  }

  /** The person mentions of a page's document (the `person_mentions` rows, ADR-0049); none for a page that has no document yet. */
  personMentions(pageId: PageId): readonly PagePersonMention[] {
    return this.#pages.get(pageId)?.personMentions ?? [];
  }

  /**
   * Where `quote` sits in the page's document, as a comment's anchor holds it (the block id and the base64 of the Loro cursors
   * of both ends); `undefined` when no run of text holds it, or the page has no document yet (none is made: seeds draw no id).
   * For the mirror's seeds.
   */
  textAnchor(pageId: PageId, quote: string): { blockId: string; start: string; end: string } | undefined {
    this.#requirePage(pageId);
    const doc = this.#pages.get(pageId)?.doc;
    const found = doc === undefined ? undefined : textAnchorOf(doc, quote);
    return found === undefined ? undefined : { blockId: found.blockId, start: toBase64(found.start), end: toBase64(found.end) };
  }

  /**
   * After `PageTrash` or `PageRestore` (`pageIds`: the page and its subtree): backlinks
   * list live sources only, so every page these pages reference gets `BacklinksChanged`.
   */
  sourcesMoved(pageIds: readonly PageId[]): void {
    this.#notifyTargets(this.#targetsOf(pageIds));
  }

  /**
   * After pages left the structure (`PagePurge`, or an undo/redo that removes them): the
   * pages they referenced, apart from removed ones, get `BacklinksChanged`. The documents
   * themselves stay, as in the core (which deletes orphaned documents at its next start),
   * so an undo that brings a page back brings its content back too.
   */
  pagesRemoved(pageIds: readonly PageId[]): void {
    const removed = new Set(pageIds);
    this.#notifyTargets([...this.#targetsOf(pageIds)].filter((target) => !removed.has(target)));
  }

  // ----- requests ---------------------------------------------------------------------

  /**
   * The query is validated like a display name (trimmed, 1 to 200 characters, no control
   * characters); an unknown `spaceId` is `NotFound`. A page matches when every word of the query
   * matches somewhere in it (ADR-0040): the title, a block or a property. The hits are, in this order:
   *
   * 1. the rows that hold every word: title hits, then block hits, then property hits, each group in
   *    display order (spaces in order, the live tree depth-first, blocks in document order, a page's
   *    properties in schema order): the core ranks with bm25, which this mirror does not reproduce. A
   *    property hit (ADR-0026, the text of a page's option names or text, URL, email and phone values)
   *    is matched like a block (every word inside that one text) but is a page-level hit: no `blockId`,
   *    the property's text as the snippet; a titles-only search never sees it. A comment of the page (M13 slice 2, ADR-0048) is
   *    matched the same way, after the property hits: a page-level hit with the comment's id (`SearchHit.commentId`, "in
   *    comment") and the comment's text as the snippet;
   * 2. for each other page whose words are spread over its rows, one hit: the row that holds the most
   *    words (the title first, then the blocks, then the properties on a tie), highlighted for the words
   *    that row holds, the pages with more words in their row first;
   * 3. when that is fewer hits than `limit`, the fuzzy title hits of {@link rankFuzzyTitles}: pages not
   *    hit yet whose title has the query's letters in order or a word with one typo, best first, with no
   *    `blockId` and the title as the snippet.
   *
   * `editedAfter` keeps the pages whose `updatedAt` is that or later. Trashed pages never match, nor do templates and the
   * pages inside them (ADR-0056); blocks without visible text are not indexed.
   */
  search(input: SearchInput): SearchHit[] {
    const query = displayName(input.query, MAX_QUERY_CHARS, "query");
    if (input.spaceId !== undefined && this.#deps.structure.spacePageIds(input.spaceId) === undefined) {
      throw notFound("space not found");
    }
    const scope = searchScope(input.scope);
    const limit = searchLimit(input.limit);
    const editedAfter = searchEditedAfter(input.editedAfter);
    const phrases = parseQuery(query);
    if (phrases.length === 0) {
      return [];
    }
    const spreadPhrases = parseDistinctQuery(query);
    // A template and the pages inside it are not found (ADR-0056: title, body, property, comment, spread and fuzzy hits alike).
    const pages = this.#deps.structure
      .livePages(input.spaceId)
      .filter((page) => !page.inTemplate && (editedAfter === undefined || page.updatedAt >= editedAfter));
    const titleHits: SearchHit[] = [];
    const blockHits: SearchHit[] = [];
    const propertyHits: SearchHit[] = [];
    const commentHits: SearchHit[] = [];
    const spreadHits: { hit: SearchHit; words: number }[] = [];
    for (const page of pages) {
      const title = matchText(page.title, phrases);
      if (title !== undefined) {
        titleHits.push(this.#hit(page, undefined, title));
      }
      if (scope === "Titles") {
        continue;
      }
      const blocks = (this.#pages.get(page.id)?.blocks ?? []).map((block) => ({
        id: block.id,
        text: this.#indexedText(block),
      }));
      const properties = this.#deps.propertyTexts?.(page.spaceId, page.id) ?? [];
      let whole = title !== undefined;
      for (const block of blocks) {
        const match = matchText(block.text, phrases);
        if (match !== undefined) {
          blockHits.push(this.#hit(page, block.id, match));
          whole = true;
        }
      }
      for (const text of properties) {
        const match = matchText(text, phrases);
        if (match !== undefined) {
          propertyHits.push(this.#hit(page, undefined, match));
          whole = true;
        }
      }
      // The words of a comment (ADR-0048): a page-level hit that names the comment ("in comment").
      for (const { commentId, text } of this.#deps.commentTexts?.(page.spaceId, page.id) ?? []) {
        const match = matchText(text, phrases);
        if (match !== undefined) {
          commentHits.push({ ...this.#hit(page, undefined, match), commentId });
          whole = true;
        }
      }
      if (!whole && spreadPhrases.length >= 2) {
        const rows = [
          { id: undefined, text: page.title },
          ...blocks,
          ...properties.map((text) => ({ id: undefined, text })),
        ];
        const spread = this.#spreadHit(page, rows, spreadPhrases);
        if (spread !== undefined) {
          spreadHits.push(spread);
        }
      }
    }
    // Pages with more words in their row first; the sort is stable, so display order breaks ties.
    spreadHits.sort((a, b) => b.words - a.words);
    const hits = [
      ...titleHits,
      ...blockHits,
      ...propertyHits,
      ...commentHits,
      ...spreadHits.map((spread) => spread.hit),
    ].slice(0, limit);
    if (hits.length < limit) {
      const hit = new Set(hits.map((found) => found.pageId));
      const candidates = pages.filter((page) => !hit.has(page.id)).map((page) => [page, page.title] as const);
      for (const { key: page, match } of rankFuzzyTitles(query, candidates, limit - hits.length)) {
        hits.push(
          this.#hit(page, undefined, {
            snippet: page.title,
            highlights: match.highlights.map(([start, end]) => ({ start, end })),
          }),
        );
      }
    }
    return hits;
  }

  /**
   * The hit of a page whose words are spread over its rows (`rows`: the title, the blocks, the
   * properties), or `undefined` when some word is in none of them: the row that holds the most words
   * (the first on a tie), shown for those words.
   */
  #spreadHit(
    page: PageMeta,
    rows: readonly { id: string | undefined; text: string }[],
    phrases: readonly (readonly string[])[],
  ): { hit: SearchHit; words: number } | undefined {
    const found = new Set<number>();
    let best: { id: string | undefined; text: string; held: (readonly string[])[] } | undefined;
    for (const row of rows) {
      const held = phrases.filter((phrase, index) => {
        const matches = matchText(row.text, [phrase]) !== undefined;
        if (matches) {
          found.add(index);
        }
        return matches;
      });
      if (held.length > 0 && (best === undefined || held.length > best.held.length)) {
        best = { id: row.id, text: row.text, held };
      }
    }
    if (best === undefined || found.size < phrases.length) {
      return undefined;
    }
    const match = matchText(best.text, best.held);
    return match === undefined ? undefined : { hit: this.#hit(page, best.id, match), words: best.held.length };
  }

  /**
   * Live pages whose blocks reference `pageId` (`NotFound` for unknown pages; a template and the pages inside it are
   * no source, ADR-0056): one entry
   * per referencing block, with the block's indexed text cut at 200 characters, ordered by
   * source page id, then block id.
   */
  backlinks(pageId: PageId): Backlink[] {
    this.#requirePage(pageId);
    const found: Backlink[] = [];
    for (const [sourceId, record] of this.#pages) {
      const source = this.#deps.structure.pageMeta(sourceId);
      // A link from a template (or from a page inside one) is no backlink (ADR-0056).
      if (source === undefined || source.trashed || source.inTemplate) {
        continue;
      }
      const seen = new Set<string>();
      for (const link of record.links) {
        if (link.toPage !== pageId || seen.has(link.fromBlock)) {
          continue;
        }
        seen.add(link.fromBlock);
        const block = record.blocks.find((candidate) => candidate.id === link.fromBlock);
        const backlink: Backlink = {
          pageId: sourceId,
          spaceId: source.spaceId,
          title: source.title,
          blockId: link.fromBlock,
          text: truncateChars(block === undefined ? "" : this.#indexedText(block), MAX_BACKLINK_CHARS),
        };
        if (source.icon !== undefined) {
          backlink.icon = source.icon;
        }
        found.push(backlink);
      }
    }
    return found.sort((a, b) => compareStrings(a.pageId, b.pageId) || compareStrings(a.blockId, b.blockId));
  }

  /**
   * The reminders of date mentions of live pages (not trashed, not templates or inside one, ADR-0056) whose fire time, on the local wall
   * clock, is after `after` and not after `until`: oldest first, then by page id and mention id,
   * at most `limit`. The page's title and the block's indexed text (cut at 200 characters) are
   * the current ones. The core's `date_mentions_due` joined with the indexes.
   */
  dueReminders(after: number, until: number, limit: number): ReminderDue[] {
    const due: ReminderDue[] = [];
    for (const [pageId, record] of this.#pages) {
      const page = this.#deps.structure.pageMeta(pageId);
      // A date mention inside a template (or a page inside one) never reminds (ADR-0056).
      if (page === undefined || page.trashed || page.inTemplate) {
        continue;
      }
      const seen = new Set<string>();
      for (const mention of record.dateMentions) {
        const { fireAtLocal, reminderMinutes } = mention;
        // The first row of a node id wins, as in the table's primary key.
        if (seen.has(mention.nodeId)) {
          continue;
        }
        seen.add(mention.nodeId);
        if (fireAtLocal === null || reminderMinutes === null || fireAtLocal <= after || fireAtLocal > until) {
          continue;
        }
        // A shared space's reminder is somebody's: it fires on the devices of the account it names, else on an Owner's.
        if (this.#deps.firesReminder?.(page.spaceId, mention.reminderFor) === false) {
          continue;
        }
        const block = record.blocks.find((candidate) => candidate.id === mention.blockId);
        const reminder: ReminderDue = {
          pageId,
          spaceId: page.spaceId,
          blockId: mention.blockId,
          nodeId: mention.nodeId,
          pageTitle: page.title,
          text: truncateChars(block === undefined ? "" : this.#indexedText(block), MAX_REMINDER_TEXT_CHARS),
          date: mention.date,
          reminderMinutes,
          fireAt: fireAtLocal,
        };
        if (mention.time !== null) {
          reminder.time = mention.time;
        }
        due.push(reminder);
      }
    }
    due.sort(
      (a, b) => a.fireAt - b.fireAt || compareStrings(a.pageId, b.pageId) || compareStrings(a.nodeId, b.nodeId),
    );
    return due.slice(0, limit);
  }

  /** The page's content as portable JSON (a new page reads as one empty paragraph). */
  content(pageId: PageId): PageContent {
    this.#requirePage(pageId);
    return { pageId, content: loroToPortable(this.#record(pageId).doc) };
  }

  /**
   * The page's stored document as portable JSON, `undefined` while it has none (a page nobody
   * opened): unlike {@link MemoryContent.content} this never creates the document, which is what
   * an export needs (ADR-0036: an export creates nothing, such a page exports as an empty one).
   */
  storedContent(pageId: PageId): JsonValue | undefined {
    this.#requirePage(pageId);
    const record = this.#pages.get(pageId);
    return record === undefined ? undefined : loroToPortable(record.doc);
  }

  /**
   * The first document of a page, written by the core itself (an import, ADR-0036) from portable
   * JSON whose blocks already carry their ids: built, indexed and announced like a content change
   * with source `"Core"`, with the attachment bookkeeping before the event and `BacklinksChanged`
   * for the pages it references. `InvalidInput` (`content`) for malformed content or a page that
   * already has a document.
   */
  createDocument(pageId: PageId, content: JsonValue): void {
    this.#requirePage(pageId);
    if (this.#pages.has(pageId)) {
      throw invalid("pageId", "the page already has a document");
    }
    const record = this.#build(content);
    this.#pages.set(pageId, record);
    this.#deps.documentWritten?.(pageId, "Core", new Map());
    this.#deps.documentChanged?.(attachmentRefsOf(record.blocks).map((ref) => ref.attachmentId));
    this.#deps.emit({ type: "PageContentChanged", data: { pageId, source: "Core" } });
    this.#notifyTargets(targetsOf(record.links));
  }

  /**
   * The first image of each of `pageIds` that has one, as `Request::PageFirstImages` answers (ADR-0054): the
   * attachment of the first `image` block or of the first item of the first `gallery` block, in document order,
   * at any depth; the order asked; pages without an image, of another space or unknown left out; `NotFound` for
   * an unknown space and `InvalidInput` (`pageIds`) for more than 500 ids. Live and trashed pages are both
   * answered. The core keeps a device-local index for this; the twin reads the documents it holds (a page
   * that was never opened has none, hence no image).
   */
  firstImages(spaceId: SpaceId, pageIds: readonly PageId[]): PageFirstImage[] {
    if (pageIds.length > MAX_FIRST_IMAGE_PAGES) {
      throw invalid("pageIds", `at most ${MAX_FIRST_IMAGE_PAGES} pages at a time`);
    }
    if (this.#deps.structure.spacePageIds(spaceId) === undefined) {
      throw notFound("space not found");
    }
    const found: PageFirstImage[] = [];
    const asked = new Set<PageId>();
    for (const pageId of pageIds) {
      if (asked.has(pageId)) {
        continue;
      }
      asked.add(pageId);
      if (this.#deps.structure.pageMeta(pageId)?.spaceId !== spaceId) {
        continue;
      }
      const record = this.#pages.get(pageId);
      const attachmentId = record === undefined ? undefined : firstImageOf(record.blocks);
      if (attachmentId !== undefined) {
        found.push({ pageId, attachmentId });
      }
    }
    return found;
  }

  /**
   * The first lines of the text of each of `pageIds`, as `Request::PageExcerpts` answers (ADR-0058): the indexed text of the
   * blocks in document order, each line cleaned by {@link excerptRow}, joined with a line break and cut by {@link cutExcerpt};
   * the order asked and each page once; a page that is not a live page of the space, a template or a page inside one, and a page
   * without text are left out; `NotFound` for an unknown space and `InvalidInput` (`pageIds`) for more than 500 ids. The core
   * reads its text index; the twin reads the documents it holds (a page that was never opened has none, hence no excerpt).
   */
  excerpts(spaceId: SpaceId, pageIds: readonly PageId[]): PageExcerpt[] {
    if (pageIds.length > MAX_EXCERPT_PAGES) {
      throw invalid("pageIds", `at most ${MAX_EXCERPT_PAGES} pages at a time`);
    }
    if (this.#deps.structure.spacePageIds(spaceId) === undefined) {
      throw notFound("space not found");
    }
    const found: PageExcerpt[] = [];
    const asked = new Set<PageId>();
    for (const pageId of pageIds) {
      if (asked.has(pageId)) {
        continue;
      }
      asked.add(pageId);
      const page = this.#deps.structure.pageMeta(pageId);
      if (page?.spaceId !== spaceId || page.trashed || page.inTemplate) {
        continue;
      }
      let text = "";
      let chars = 0;
      for (const block of this.#pages.get(pageId)?.blocks ?? []) {
        const line = excerptRow(this.#indexedText(block));
        if (line === undefined) {
          continue;
        }
        if (chars > 0) {
          text += "\n";
          chars += 1;
        }
        text += line;
        chars += Array.from(line).length;
        // Past the limit the rest of the page cannot change the excerpt.
        if (chars > EXCERPT_CHARS) {
          break;
        }
      }
      if (chars > 0) {
        found.push({ pageId, text: cutExcerpt(text) });
      }
    }
    return found;
  }

  /** The page's content as Markdown, page references resolved to the current titles and files to their names. */
  markdown(pageId: PageId): string {
    this.#requirePage(pageId);
    return renderMarkdown(loroToPortable(this.#record(pageId).doc), (id) => this.#titleOf(id), {
      resolveAttachmentName: (id) => this.#deps.attachmentName?.(id),
    });
  }

  /**
   * How many `image`, `file` and `gallery` blocks show each attachment, over the documents of live and
   * trashed pages (purged pages reference nothing): the core's `attachment_refs` index; plus the
   * `files` values of properties that list it (`page_properties.attachment_id`) and the pages
   * that have it as their icon (`icon_refs`).
   */
  attachmentReferenceCounts(): Map<AttachmentId, number> {
    const counts = new Map<AttachmentId, number>();
    for (const [pageId, record] of this.#pages) {
      if (this.#deps.structure.pageMeta(pageId) === undefined) {
        continue;
      }
      // One reference per (block, attachment): a gallery that lists a picture twice shows it once.
      const seen = new Set<string>();
      for (const { blockId, attachmentId } of attachmentRefsOf(record.blocks)) {
        const key = `${blockId}|${attachmentId}`;
        if (!seen.has(key)) {
          seen.add(key);
          counts.set(attachmentId, (counts.get(attachmentId) ?? 0) + 1);
        }
      }
    }
    for (const [attachmentId, count] of this.#deps.propertyReferences?.() ?? []) {
      counts.set(attachmentId, (counts.get(attachmentId) ?? 0) + count);
    }
    for (const [attachmentId, count] of this.#deps.iconReferences?.() ?? []) {
      counts.set(attachmentId, (counts.get(attachmentId) ?? 0) + count);
    }
    return counts;
  }

  /**
   * The pages whose documents show each attachment (`image`, `file` and `gallery` blocks of live and
   * trashed pages): `attachmentReferenceCounts` by page, for the move to another space (ADR-0038).
   */
  attachmentReferrers(): Map<AttachmentId, Set<PageId>> {
    const found = new Map<AttachmentId, Set<PageId>>();
    for (const [pageId, record] of this.#pages) {
      if (this.#deps.structure.pageMeta(pageId) === undefined) {
        continue;
      }
      for (const { attachmentId } of attachmentRefsOf(record.blocks)) {
        const pages = found.get(attachmentId) ?? new Set<PageId>();
        pages.add(pageId);
        found.set(attachmentId, pages);
      }
    }
    return found;
  }

  /**
   * After a move to another space (ADR-0038): rewrites the attributes of the saved views (`pageList`
   * blocks) of those of `pageIds` that have a document, through `remap` (see `editPageListAttrs`). A
   * document that changed is committed, indexed and announced as a change by the core
   * (`PageContentChanged { source: "Core" }`); pages without a document have no views and get none.
   * Answers the pages that changed.
   */
  remapViews(pageIds: readonly PageId[], remap: (attrs: Attrs) => Attrs): PageId[] {
    const changed: PageId[] = [];
    for (const pageId of pageIds) {
      const record = this.#pages.get(pageId);
      const before = record?.doc.oplogVersion().toJSON();
      if (record !== undefined && before !== undefined && editPageListAttrs(record.doc, remap)) {
        record.doc.commit();
        this.#reindexAndNotify(pageId, record, "Core", before);
        changed.push(pageId);
      }
    }
    return changed;
  }

  /** Derives blocks and links again from the documents of one space (`NotFound` when unknown) or of all. */
  rebuild(spaceId: SpaceId | undefined): void {
    const pageIds =
      spaceId === undefined ? [...this.#pages.keys()] : this.#deps.structure.spacePageIds(spaceId);
    if (pageIds === undefined) {
      throw notFound("space not found");
    }
    for (const pageId of pageIds) {
      const record = this.#pages.get(pageId);
      if (record !== undefined) {
        this.#reindex(record);
      }
    }
  }

  // ----- plumbing ---------------------------------------------------------------------

  /** Live and trashed pages have content; unknown and purged pages are `NotFound`. */
  #requirePage(pageId: PageId): void {
    if (this.#deps.structure.pageMeta(pageId) === undefined) {
      throw notFound("page not found");
    }
  }

  /** The page's document, created like the core does on first use: root plus one empty paragraph. */
  #record(pageId: PageId): PageRecord {
    let record = this.#pages.get(pageId);
    if (record === undefined) {
      const doc = newPageDoc();
      portableToLoro(doc, { type: "doc", content: [{ type: "paragraph", attrs: { id: this.#deps.newId() } }] });
      record = { doc, blocks: [], links: [], dateMentions: [], personMentions: [] };
      this.#reindex(record);
      this.#pages.set(pageId, record);
    }
    return record;
  }

  /** A page document built from portable JSON, indexed. */
  #build(content: JsonValue): PageRecord {
    const doc = newPageDoc();
    portableToLoro(doc, content);
    const record: PageRecord = { doc, blocks: [], links: [], dateMentions: [], personMentions: [] };
    this.#reindex(record);
    return record;
  }

  #reindex(record: PageRecord): void {
    record.blocks = extractBlocks(record.doc);
    record.links = extractLinks(record.doc);
    record.dateMentions = extractDateMentions(record.doc);
    record.personMentions = extractPersonMentions(record.doc);
  }

  /**
   * As `persist_page`: the journal (ADR-0057, unless `journal` is false: a restore records itself), then the content event,
   * then the symmetric difference of referenced pages. `version` is the document's version vector from before the write.
   */
  #reindexAndNotify(
    pageId: PageId,
    record: PageRecord,
    source: ChangeSource,
    version: ReadonlyMap<string, number>,
    journal = true,
  ): void {
    const before = targetsOf(record.links);
    const refsBefore = attachmentRefsOf(record.blocks);
    this.#reindex(record);
    const after = targetsOf(record.links);
    if (journal) {
      this.#deps.documentWritten?.(pageId, source, version);
    }
    // The core's bookkeeping runs (and may announce `AttachmentsChanged`) before the content event.
    this.#deps.documentChanged?.(
      [...refsBefore, ...attachmentRefsOf(record.blocks)].map((ref) => ref.attachmentId),
    );
    this.#deps.emit({ type: "PageContentChanged", data: { pageId, source } });
    this.#notifyTargets([
      ...[...before].filter((target) => !after.has(target)),
      ...[...after].filter((target) => !before.has(target)),
    ]);
    if (source === "Editor") {
      this.#deps.documentEdited?.(pageId);
    }
  }

  /** Pages referenced from any of `pageIds` (by their current documents). */
  #targetsOf(pageIds: readonly PageId[]): Set<PageId> {
    const targets = new Set<PageId>();
    for (const pageId of pageIds) {
      for (const link of this.#pages.get(pageId)?.links ?? []) {
        targets.add(link.toPage);
      }
    }
    return targets;
  }

  /** `BacklinksChanged` once per page, in page id order (the core walks a `BTreeSet`). */
  #notifyTargets(targets: Iterable<PageId>): void {
    for (const pageId of [...new Set(targets)].sort(compareStrings)) {
      this.#deps.emit({ type: "BacklinksChanged", data: { pageId } });
    }
  }

  /** Titles of live and trashed pages; purged and unknown pages have none. */
  #titleOf(pageId: PageId): string | undefined {
    return this.#deps.structure.pageMeta(pageId)?.title;
  }

  /**
   * A block's text as the core indexes it (references resolved to the current titles,
   * breaks as spaces; an `image` reads as its alt text, a `file` as nothing); empty when it
   * is blank, since blank blocks get no index row.
   */
  #indexedText(block: PageBlock): string {
    const text = inlineText(block.inlines, (id) => this.#titleOf(id));
    return text.trim() === "" ? "" : text;
  }

  #hit(page: PageMeta, blockId: string | undefined, match: TextMatch): SearchHit {
    const hit: SearchHit = {
      pageId: page.id,
      spaceId: page.spaceId,
      title: page.title,
      snippet: match.snippet,
      highlights: match.highlights,
    };
    if (page.icon !== undefined) {
      hit.icon = page.icon;
    }
    if (blockId !== undefined) {
      hit.blockId = blockId;
    }
    return hit;
  }
}
