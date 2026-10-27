// The in-memory twin of the core's Milestone 2 documents (account order of spaces and one
// structure document per space), used by `MemoryTransport`. It follows `kb-core`'s
// handlers (`documents.rs`) and `kb-model`'s `StructureDoc`: validation, error shapes,
// event order and the trash, restore, purge and undo semantics of
// `docs/development/milestone-02.md`. Nothing here knows about requests or responses;
// `MemoryTransport` maps one onto the other. Page content (Milestone 3) lives in
// `./content`, which reads titles, icons and trash state through the lookups below.

import { CoreError, internalError } from "./errors";
import type { AttachmentId } from "./generated/AttachmentId";
import type { Event } from "./generated/Event";
import type { PageFont } from "./generated/PageFont";
import type { PageId } from "./generated/PageId";
import type { PageInfo } from "./generated/PageInfo";
import type { PageNode } from "./generated/PageNode";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyValue } from "./generated/PropertyValue";
import type { SpaceId } from "./generated/SpaceId";
import type { SpaceInfo } from "./generated/SpaceInfo";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { TrashedPage } from "./generated/TrashedPage";
import type { TreeView } from "./generated/TreeView";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { type LocalMoment, resolveTitleTokens } from "./page-copy";
import type { SeedOption, SeedProperty, SeedValue } from "./properties";
import {
  type PageAppearanceChange,
  COVER_Y_DEFAULT,
  COVER_Y_MAX,
  coverAttachment,
  cover as validCover,
  coverY as validCoverY,
  displayName,
  icon as validIcon,
  iconAttachment,
  invalid,
  appearance as validAppearance,
  pageTitle,
  position,
  spaceIcon as validSpaceIcon,
} from "./validate";

/** Depth of the per-space undo and redo stacks. */
const UNDO_DEPTH = 100;
const MAX_SPACE_NAME_CHARS = 100;

/** The fonts a page stores (ADR-0040): the two that are not the default, which is no value. */
export type StoredPageFont = Exclude<PageFont, "default">;

/** A page to create when seeding a `MemoryTransport` (see `MemoryTransportOptions.spaces`). */
export interface SeedPage {
  title: string;
  icon?: string;
  /**
   * The page's cover (ADR-0040): `attachment:<id>` (checked like any request, so seed attachments with
   * an id of their own are not usable here), `color:<palette name>` or `gradient:<name>`.
   */
  cover?: string;
  /** The cover's vertical position in percent (0 to 100, 50 by default). */
  coverY?: number;
  /** The page's font (ADR-0040): `serif` or `mono`; none is the app's default. */
  font?: StoredPageFont;
  /** Smaller text on the page (ADR-0040). */
  smallText?: boolean;
  /** The page opens read-only in the UI (ADR-0040). */
  locked?: boolean;
  /** The page is a template (ADR-0056), like `PageSetTemplate` makes it: no event, no undo history. */
  template?: boolean;
  /**
   * The page's template placeholders (ADR-0056) by the property's NAME (`today` and `now` on a `date` property, `me` on a
   * `person` property): set once the space's properties exist, so a name no property has, or a kind that does not take the
   * placeholder, is an `Internal` error at seed time. They only show while the page is in a template (it or an ancestor
   * has `template`).
   */
  templatePlaceholders?: Readonly<Record<string, TemplatePlaceholder>>;
  /**
   * The page's content as portable ProseMirror JSON (`{ "type": "doc", "content": [...] }`,
   * `docs/development/milestone-03.md`, "Portable JSON"), built into the page's document
   * at seed time. Without it the document is created on first open (one empty paragraph).
   * As in the core, only blocks whose `id` is a valid UUID are searchable and own
   * references, and a `pageRef` or `pageLink` needs a valid `pageId` (seeded pages get their ids from
   * `MemoryTransportOptions.newId`, in creation order, which lets seeds reference each other).
   */
  content?: JsonValue;
  /**
   * Values of the space's properties (ADR-0026) by the property's NAME (`SeedSpace.properties`,
   * or `"Tags"` as long as Tags is not renamed), by the kind of the property: a string for text,
   * URL, email and phone; a number; a boolean; `{ date, time? }`; the option NAME for select and
   * status; the option names for multi-select (see `SeedValue`). A name no property has is an
   * `Internal` error at seed time, so is an option name the property does not list (only Tags
   * gets missing options created, see `tags`). Values are checked like `PagePropertySet` checks
   * them, and an empty one is no value. Seeding leaves the page's `updatedAt` alone.
   */
  properties?: Readonly<Record<string, SeedValue>>;
  /**
   * Shorthand for the Tags value: the names of its options. A name Tags does not have yet
   * becomes a new option (colour gray), created when this page is reached (pages in the order
   * they are created). A `properties` entry for Tags applies after `tags` and replaces it.
   */
  tags?: readonly string[];
  /**
   * Marks the page as a favourite of its space (ADR-0029), like `PageFavoriteSet` does: pages are
   * added in the order they are created (depth first), last each time. No event, no undo entry.
   */
  favorite?: boolean;
  /** Child pages, in order. */
  children?: readonly SeedPage[];
}

/** A space to create when seeding a `MemoryTransport`. */
export interface SeedSpace {
  name: string;
  icon?: string;
  /** Root pages, in order. */
  pages?: readonly SeedPage[];
  /**
   * Options of the built-in Tags property (ADR-0026), in order; Tags is written (it stops being
   * virtual) when there is one.
   */
  tagOptions?: readonly SeedOption[];
  /**
   * The space's property schema, in order, after Tags (ADR-0026): created without events or
   * undo history, at most 100 properties with Tags, names checked like `PropertyCreate`.
   *
   * Property and option ids come from `MemoryTransportOptions.newId` unless given, AFTER every
   * id the seeds of spaces, pages, security and attachments draw (so adding properties to a
   * seed never shifts the ids of its spaces and pages): space by space in order, first the
   * ids of `tagOptions` that have none, then each property (its id, then the ids of its
   * options, or of the three default options of a status property), then, walking the space's
   * pages in creation order, one id for every Tags option that a page's `tags` or
   * `properties` create and one for every checklist item of a page's `properties` that has no
   * `id` (in the order the page's values are seeded).
   */
  properties?: readonly SeedProperty[];
}

/** A page as a backup of the mirror carries it (`MemoryTransport.backupExport`): ids and times kept. */
export interface PageState {
  id: PageId;
  title: string;
  icon?: string;
  /** The cover (ADR-0040): `attachment:<id>`, `color:<name>` or `gradient:<name>`. */
  cover?: string;
  /** The cover's vertical position in percent; absent for the default 50. */
  coverY?: number;
  /** The font (ADR-0040); absent for the default. */
  font?: StoredPageFont;
  /** Smaller text (ADR-0040); absent when off. */
  smallText?: true;
  /** Locked in the UI (ADR-0040); absent when off. */
  locked?: true;
  /** The page is a template (ADR-0056, the node meta `template`); absent when it is not. */
  template?: true;
  /**
   * The page's placeholders (ADR-0056, the node meta `template_values`, `"templateValues"` in the portable JSON of the
   * core): property id to `today`, `now` or `me`; absent when there are none.
   */
  templateValues?: Record<PropertyId, TemplatePlaceholder>;
  createdAt: number;
  updatedAt: number;
  /** The device that created the page (ADR-0026, amendment of 2026-10-08); absent when unknown. */
  createdBy?: string;
  /** The device that last edited the page, written with `updatedAt`; absent when unknown. */
  editedBy?: string;
  /** Structural parent; `null` at the space root and for the top page of a trashed subtree. */
  parentId: PageId | null;
  /** Children in order. */
  children: PageId[];
  /** Set on every page of a trashed subtree. */
  trashedAt?: number;
  /** Set on the top page of a trashed subtree: its trash order and where it came from. */
  trash?: { seq: number; from: PageId | null };
  /**
   * The values the page holds, as stored (ADR-0026): property id to value, including the ones the
   * schema no longer reads. Left out when there are none, and in backups written before properties.
   */
  props?: [PropertyId, PropertyValue][];
}

/** A space as a backup of the mirror carries it: every page, live and trashed; no undo history. */
export interface SpaceState {
  id: SpaceId;
  name: string;
  icon?: string;
  createdAt: number;
  /** Live root pages in order. */
  roots: PageId[];
  pages: PageState[];
  /**
   * The written property schema (ADR-0026) in order: Tags first when it has been written, then
   * the other properties. Left out when there is none, and in backups written before properties.
   */
  properties?: PropertyDef[];
  /**
   * The favourite pages of the space in order (ADR-0029), as stored: hidden ones (in the trash)
   * and ids of pages that are gone included. Left out when there are none, and in backups written
   * before favourites.
   */
  favorites?: PageId[];
}

/** What the content mirror needs to know about a live or trashed page. */
export interface PageMeta {
  id: PageId;
  spaceId: SpaceId;
  title: string;
  icon: string | undefined;
  /** The cover, when the page has one (ADR-0040). */
  cover: string | undefined;
  /** In the trash (itself or below a trashed page). */
  trashed: boolean;
  /**
   * The page is a template or inside one (ADR-0056): it or an ancestor has the flag. The core's `pages_index.in_template`:
   * search, backlinks, reminders and the AI index leave such pages out.
   */
  inTemplate: boolean;
  /** Creation time, Unix ms. */
  createdAt: number;
  /** Last edit, Unix ms (`updatedAt`). */
  updatedAt: number;
  /** The device that created the page, when known. */
  createdBy: string | undefined;
  /** The device that last edited the page, when known. */
  editedBy: string | undefined;
}

/** What the structure needs from its owner. */
export interface StructureDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Id generator (UUID text). */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /**
   * Told about pages that an undo or redo removed from a space (an undone create, a
   * redone purge), before `StructureChanged` is emitted, as `kb-core` orders it.
   */
  pagesRemoved?: (pageIds: PageId[]) => void;
  /**
   * The values of a page's properties as `PageInfo.properties` lists them (schema order, readable
   * ones only); the structure leaves the key out when this is empty or absent.
   */
  pageProperties?: (spaceId: SpaceId, pageId: PageId) => [PropertyId, PropertyValue][];
  /**
   * The space's properties in schema order (Tags first), with their kinds: the placeholders of `PageInfo`
   * (ADR-0056) list only the properties that still exist, in this order, whose kind takes the placeholder. Without it
   * (or for a space it does not know) no placeholder is listed.
   */
  propertyKinds?: (spaceId: SpaceId) => readonly { id: PropertyId; kind: PropertyKind }[];
  /**
   * The value a page stores for a property, as stored (a copy), `undefined` when it has none: what setting a placeholder
   * removes and an undo of that puts back (ADR-0056).
   */
  storedValue?: (spaceId: SpaceId, pageId: PageId, propertyId: PropertyId) => PropertyValue | undefined;
  /**
   * Stores a value a page keeps for a property, or removes it (`undefined`), without an event and without a stamp: the
   * values side of setting a placeholder and of its undo and redo (ADR-0056). A page that is not in the space is left alone.
   */
  restoreValue?: (spaceId: SpaceId, pageId: PageId, propertyId: PropertyId, value: PropertyValue | undefined) => void;
  /**
   * The finished attachment with this id, if any (`undefined` for an unknown or purged one): the
   * space it belongs to and its MIME type. `PageSetIcon` and `PageSetCover` with `attachment:<id>` check it
   * (ADR-0029, ADR-0040); without this lookup every such icon or cover is refused as unknown.
   */
  attachmentOf?: (attachmentId: AttachmentId) => { spaceId: SpaceId; mime: string } | undefined;
  /**
   * The id of this device, recorded as `createdBy` of the pages made here and as `editedBy` of
   * the pages edited here (ADR-0026, amendment of 2026-10-08). Default: none are recorded.
   */
  deviceId?: () => string;
  /**
   * Adds what sharing says to a space as listed (`shareState`, `role`, `memberCount`, `archiveReason`,
   * `aiEnabled`, Milestone 13); a personal space comes back as it is. Default: no decoration.
   */
  decorateSpace?: (info: SpaceInfo) => SpaceInfo;
}

/** The least time between two stamps of a page's last edit by a content edit (`EDIT_STAMP_INTERVAL_MS`). */
export const EDIT_STAMP_INTERVAL_MS = 60_000;

/** Most pages a move to another space carries (`kb-core`'s `MAX_MOVED_PAGES`, ADR-0038). */
export const MAX_MOVED_PAGES = 10_000;

/** Most pages one duplication copies, the page itself included (`kb-core`'s `MAX_DUPLICATED_PAGES`, ADR-0040). */
export const MAX_DUPLICATED_PAGES = 1_000;

/** The longest page title, in characters (`validate.pageTitle`). */
const MAX_TITLE_CHARS = 500;

/** How the copies of a subtree differ from their originals: a duplicate (ADR-0040) or a page made from a template (ADR-0056). */
type CopyStyle =
  | { kind: "duplicate"; topTitle: string }
  | { kind: "template"; parentId: PageId | null; index: number | undefined; moment: LocalMoment };

/** What a duplication settled before anything is written. */
export interface DuplicateCheck {
  /** The space of the page (the copies go there). */
  spaceId: SpaceId;
  /** The page and its live subtree, parents first. */
  pageIds: PageId[];
}

/** What a move to another space settled before anything is written. */
export interface MoveCheck {
  /** The space the page is in now. */
  fromSpaceId: SpaceId;
  /** The page and its live subtree, parents first. */
  pageIds: PageId[];
}

/** What a move to another space did to the structure. */
export interface MovedPages extends MoveCheck {
  /** The moved pages that were favourites of the source, in the order they were moved. */
  favorites: PageId[];
}

interface PageRecord {
  id: PageId;
  title: string;
  icon: string | undefined;
  /** The cover (ADR-0040), as the node meta `cover` stores it. */
  cover: string | undefined;
  /** The vertical position in percent as `coverY` stores it: never 50, never without a cover. */
  coverY: number | undefined;
  /** The font (ADR-0040), as the node meta `font` stores it: `undefined` for the default. */
  font: StoredPageFont | undefined;
  /** Smaller text (ADR-0040): the node meta `smallText`. */
  smallText: boolean;
  /** Locked in the UI (ADR-0040): the node meta `locked`. */
  locked: boolean;
  /** A template (ADR-0056): the node meta `template`; absent or `false` = a page. A flag of the page itself, not of a subtree. */
  template?: boolean | undefined;
  /**
   * The placeholders of the page (ADR-0056): the node meta `template_values`, property id to `today`, `now` or `me`;
   * absent = none. Kept when the page leaves a template, ignored until it is in one again. Never changed in place (an
   * undo snapshot shares it): a change makes a new map.
   */
  templateValues?: ReadonlyMap<PropertyId, TemplatePlaceholder> | undefined;
  createdAt: number;
  updatedAt: number;
  createdBy: string | undefined;
  editedBy: string | undefined;
  /**
   * Structural parent: a page, or `null` at the space root and for the top page of a
   * trashed subtree. Pages inside a trashed subtree keep their (trashed) parent.
   */
  parentId: PageId | null;
  /** Children in order. */
  children: PageId[];
  /** Set on every page of a trashed subtree, like `pages_index.trashed_at`. */
  trashedAt: number | undefined;
  /**
   * Set on the top page of a trashed subtree only: where it came from, and a sequence
   * number that orders pages trashed within the same millisecond.
   */
  trash: { seq: number; from: PageId | null } | undefined;
}

/** A stored property value (or its absence) that a structural change wrote and an undo or redo puts back (ADR-0056). */
interface ValueEdit {
  pageId: PageId;
  propertyId: PropertyId;
  value: PropertyValue | undefined;
}

interface Snapshot {
  pages: Map<PageId, PageRecord>;
  roots: PageId[];
  /**
   * The values the change after this snapshot removed (setting a placeholder replaces the page's value of the property,
   * ADR-0056), as they were before it; going back to this snapshot stores them again. Values are otherwise not part of
   * structural undo.
   */
  values?: ValueEdit[];
}

interface SpaceRecord {
  id: SpaceId;
  name: string;
  icon: string | undefined;
  createdAt: number;
  pages: Map<PageId, PageRecord>;
  /** Live root pages in order. */
  roots: PageId[];
  /**
   * The favourite pages in order (ADR-0029), as the `favorites` map of the core's structure
   * document stores them: entries of trashed pages stay (hidden), and so may entries of pages that
   * are gone (ignored on read). Not part of the undo snapshots: undo and redo never touch them.
   */
  favorites: PageId[];
  undo: Snapshot[];
  redo: Snapshot[];
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

function copyRecord(page: PageRecord): PageRecord {
  return {
    ...page,
    children: [...page.children],
    trash: page.trash === undefined ? undefined : { ...page.trash },
  };
}

/** A page record as plain data; optional fields left out when unset. */
function pageState(page: PageRecord): PageState {
  const state: PageState = {
    id: page.id,
    title: page.title,
    createdAt: page.createdAt,
    updatedAt: page.updatedAt,
    parentId: page.parentId,
    children: [...page.children],
  };
  if (page.createdBy !== undefined) {
    state.createdBy = page.createdBy;
  }
  if (page.editedBy !== undefined) {
    state.editedBy = page.editedBy;
  }
  if (page.icon !== undefined) {
    state.icon = page.icon;
  }
  if (page.cover !== undefined) {
    state.cover = page.cover;
  }
  if (page.coverY !== undefined) {
    state.coverY = page.coverY;
  }
  if (page.font !== undefined) {
    state.font = page.font;
  }
  if (page.smallText) {
    state.smallText = true;
  }
  if (page.locked) {
    state.locked = true;
  }
  if (page.template === true) {
    state.template = true;
  }
  if (page.templateValues !== undefined && page.templateValues.size > 0) {
    state.templateValues = Object.fromEntries(page.templateValues);
  }
  if (page.trashedAt !== undefined) {
    state.trashedAt = page.trashedAt;
  }
  if (page.trash !== undefined) {
    state.trash = { ...page.trash };
  }
  return state;
}

function pageRecord(state: PageState): PageRecord {
  return {
    id: state.id,
    title: state.title,
    icon: state.icon,
    cover: state.cover,
    coverY: state.cover === undefined ? undefined : storedCoverY(state.coverY),
    font: storedFont(state.font),
    smallText: state.smallText === true,
    locked: state.locked === true,
    template: state.template === true ? true : undefined,
    templateValues: storedTemplateValues(state.templateValues),
    createdAt: state.createdAt,
    updatedAt: state.updatedAt,
    createdBy: state.createdBy,
    editedBy: state.editedBy,
    parentId: state.parentId,
    children: [...state.children],
    trashedAt: state.trashedAt,
    trash: state.trash === undefined ? undefined : { ...state.trash },
  };
}

/** The placeholders a page stores: only known ones (anything else a backup carries is dropped); `undefined` when none. */
function storedTemplateValues(
  raw: Readonly<Record<PropertyId, unknown>> | undefined,
): ReadonlyMap<PropertyId, TemplatePlaceholder> | undefined {
  const kept = new Map<PropertyId, TemplatePlaceholder>();
  for (const [propertyId, placeholder] of Object.entries(raw ?? {})) {
    if (placeholder === "today" || placeholder === "now" || placeholder === "me") {
      kept.set(propertyId, placeholder);
    }
  }
  return kept.size > 0 ? kept : undefined;
}

/**
 * Whether a property of this kind takes the placeholder (ADR-0056): `today` and `now` a `date` property, `me` a `person`
 * property.
 */
export function placeholderTakes(placeholder: TemplatePlaceholder, kind: PropertyKind): boolean {
  return placeholder === "me" ? kind === "person" : kind === "date";
}

/** The position a page stores: only a valid one other than the default (anything else reads as the default). */
function storedCoverY(raw: number | undefined): number | undefined {
  return raw !== undefined && Number.isInteger(raw) && raw >= 0 && raw <= COVER_Y_MAX && raw !== COVER_Y_DEFAULT
    ? raw
    : undefined;
}

/** The font a page stores: only one of the two (anything else a backup carries reads as the default). */
function storedFont(raw: string | undefined): StoredPageFont | undefined {
  return raw === "serif" || raw === "mono" ? raw : undefined;
}

/** The attachments a page references through its icon and its cover, each once. */
function pageImages(page: PageRecord): AttachmentId[] {
  const found = new Set<AttachmentId>();
  for (const id of [iconAttachment(page.icon), coverAttachment(page.cover)]) {
    if (id !== undefined) {
      found.add(id);
    }
  }
  return [...found];
}

function snapshotOf(space: SpaceRecord): Snapshot {
  const pages = new Map<PageId, PageRecord>();
  for (const [id, page] of space.pages) {
    pages.set(id, copyRecord(page));
  }
  return { pages, roots: [...space.roots] };
}

/** Takes the moving pages, and whatever hangs below them in this snapshot, out of a history snapshot. */
function strip(snapshot: Snapshot, moving: ReadonlySet<PageId>): void {
  const gone = new Set<PageId>();
  const visit = (id: PageId): void => {
    if (!gone.has(id)) {
      gone.add(id);
      snapshot.pages.get(id)?.children.forEach(visit);
    }
  };
  moving.forEach(visit);
  for (const id of gone) {
    snapshot.pages.delete(id);
  }
  snapshot.roots = snapshot.roots.filter((id) => !gone.has(id));
  for (const page of snapshot.pages.values()) {
    page.children = page.children.filter((id) => !gone.has(id));
  }
}

/**
 * Puts moved pages (the top one first) into a history snapshot of their new space, under the parent
 * they moved to when the snapshot has that page live, else at the end of its roots.
 */
function graft(snapshot: Snapshot, records: readonly PageRecord[], parentId: PageId | null): void {
  const top = records[0];
  if (top === undefined) {
    return;
  }
  for (const record of records) {
    snapshot.pages.set(record.id, copyRecord(record));
  }
  const parent = parentId === null ? undefined : snapshot.pages.get(parentId);
  const copy = snapshot.pages.get(top.id);
  if (parent !== undefined && parent.trashedAt === undefined) {
    parent.children.push(top.id);
  } else {
    snapshot.roots.push(top.id);
    if (copy !== undefined) {
      copy.parentId = null;
    }
  }
}

/** `index` among `length` siblings: `undefined` appends, anything past the end clamps. */
function clamp(index: number | undefined, length: number): number {
  return index === undefined ? length : Math.min(index, length);
}

/**
 * The ordered spaces of a workspace with the page tree, trash and undo history of each.
 * Methods throw `CoreError`s exactly where the core answers with an `ApiError`, and emit
 * events in the order the core emits them, after the change is applied.
 */
export class MemoryStructure {
  readonly #deps: StructureDeps;
  readonly #spaces = new Map<SpaceId, SpaceRecord>();
  /** Display order of spaces (the account document). */
  #order: SpaceId[] = [];
  #trashSeq = 0;
  #silent = false;

  constructor(deps: StructureDeps) {
    this.#deps = deps;
  }

  /**
   * Creates spaces and pages without events and without undo history, as if they had been
   * there before the app started. Names, titles and icons are validated like any request.
   * `onPage` is told about every page created, with the seed it came from; `onSpace` about every
   * space, right after it is created (before its pages).
   */
  seed(
    spaces: readonly SeedSpace[],
    onPage?: (page: PageInfo, seed: SeedPage) => void,
    onSpace?: (space: SpaceInfo, seed: SeedSpace) => void,
  ): void {
    this.#silent = true;
    try {
      for (const seedSpace of spaces) {
        const info = this.spaceCreate(seedSpace.name, seedSpace.icon);
        onSpace?.(info, seedSpace);
        const createPages = (pages: readonly SeedPage[], parentId: PageId | undefined): void => {
          for (const seedPage of pages) {
            const page = this.pageCreate(info.id, parentId, undefined, seedPage.title);
            if (seedPage.icon !== undefined) {
              this.pageSetIcon(page.id, seedPage.icon);
            }
            if (seedPage.cover !== undefined) {
              this.pageSetCover(page.id, seedPage.cover, seedPage.coverY);
            }
            if (seedPage.font !== undefined || seedPage.smallText !== undefined || seedPage.locked !== undefined) {
              this.pageSetAppearance(page.id, {
                font: seedPage.font,
                smallText: seedPage.smallText,
                locked: seedPage.locked,
              });
            }
            if (seedPage.template === true) {
              this.pageSetTemplate(page.id, true);
            }
            if (seedPage.favorite === true) {
              this.pageFavoriteSet(page.id, true);
            }
            onPage?.(page, seedPage);
            createPages(seedPage.children ?? [], page.id);
          }
        };
        createPages(seedSpace.pages ?? [], undefined);
      }
    } finally {
      this.#silent = false;
    }
    for (const space of this.#spaces.values()) {
      space.undo.length = 0;
      space.redo.length = 0;
    }
  }

  // ----- spaces ---------------------------------------------------------------------

  spaceList(): SpaceInfo[] {
    return this.#order.map((id) => this.#spaceInfo(this.#space(id)));
  }

  spaceCreate(rawName: string, rawIcon: string | null | undefined): SpaceInfo {
    const name = displayName(rawName, MAX_SPACE_NAME_CHARS, "name");
    const icon = validSpaceIcon(rawIcon);
    const space: SpaceRecord = {
      id: this.#deps.newId(),
      name,
      icon,
      createdAt: this.#deps.now(),
      pages: new Map(),
      roots: [],
      favorites: [],
      undo: [],
      redo: [],
    };
    this.#spaces.set(space.id, space);
    this.#order.push(space.id);
    this.#emit({ type: "SpacesChanged" });
    return this.#spaceInfo(space);
  }

  spaceRename(spaceId: SpaceId, rawName: string): SpaceInfo {
    const name = displayName(rawName, MAX_SPACE_NAME_CHARS, "name");
    const space = this.#space(spaceId);
    space.name = name;
    this.#emit({ type: "SpacesChanged" });
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    return this.#spaceInfo(space);
  }

  spaceSetIcon(spaceId: SpaceId, rawIcon: string | null): SpaceInfo {
    const icon = validSpaceIcon(rawIcon);
    const space = this.#space(spaceId);
    space.icon = icon;
    this.#emit({ type: "SpacesChanged" });
    return this.#spaceInfo(space);
  }

  spaceMove(spaceId: SpaceId, rawIndex: number): SpaceInfo[] {
    const index = position(rawIndex);
    this.#space(spaceId);
    const others = this.#order.filter((id) => id !== spaceId);
    others.splice(clamp(index, others.length), 0, spaceId);
    this.#order = others;
    this.#emit({ type: "SpacesChanged" });
    return this.spaceList();
  }

  spaceDelete(spaceId: SpaceId): void {
    const space = this.#space(spaceId);
    if (space.pages.size > 0) {
      throw invalid("spaceId", "the space still contains pages (live or in the trash)");
    }
    this.#spaces.delete(spaceId);
    this.#order = this.#order.filter((id) => id !== spaceId);
    this.#emit({ type: "SpacesChanged" });
  }

  /**
   * The local copy of a space goes with all its pages, live and trashed (leaving a shared space, removing an
   * archive from this device; Milestone 13). Answers the ids of the pages that went. Emits `SpacesChanged`.
   */
  spaceDrop(spaceId: SpaceId): PageId[] {
    const space = this.#space(spaceId);
    const pageIds = [...space.pages.keys()];
    this.#spaces.delete(spaceId);
    this.#order = this.#order.filter((id) => id !== spaceId);
    this.#emit({ type: "SpacesChanged" });
    return pageIds;
  }

  /** One space as listed; `NotFound` for an unknown one. */
  spaceInfo(spaceId: SpaceId): SpaceInfo {
    return this.#spaceInfo(this.#space(spaceId));
  }

  // ----- pages ----------------------------------------------------------------------

  treeGet(spaceId: SpaceId): TreeView {
    const space = this.#space(spaceId);
    const trashedTops: PageRecord[] = [];
    for (const page of space.pages.values()) {
      if (page.trash !== undefined) {
        trashedTops.push(page);
      }
    }
    // Most recently trashed first; `seq` breaks ties inside one millisecond.
    trashedTops.sort(
      (a, b) => (b.trashedAt ?? 0) - (a.trashedAt ?? 0) || (b.trash?.seq ?? 0) - (a.trash?.seq ?? 0),
    );
    const trashed = trashedTops.map((page): TrashedPage => {
      const entry: TrashedPage = {
        page: this.#pageInfo(space, page),
        descendantCount: this.#subtree(space, page.id).length - 1,
      };
      const from = page.trash?.from ?? null;
      if (from !== null && space.pages.has(from)) {
        entry.originalParentId = from;
      }
      return entry;
    });
    const view: TreeView = {
      spaceId,
      roots: space.roots.flatMap((id) => this.#node(space, id)),
      trashed,
      undo: { canUndo: space.undo.length > 0, canRedo: space.redo.length > 0 },
    };
    // Like the core's wire form, the list is left out when there is no live favourite.
    const favorites = this.#liveFavorites(space);
    if (favorites.length > 0) {
      view.favorites = favorites;
    }
    return view;
  }

  pageGet(pageId: PageId): PageInfo {
    const { space, page } = this.#locate(pageId);
    return this.#pageInfo(space, page);
  }

  pageCreate(
    spaceId: SpaceId,
    parentId: PageId | null | undefined,
    rawIndex: number | undefined,
    rawTitle: string,
  ): PageInfo {
    const index = rawIndex === undefined ? undefined : position(rawIndex);
    const title = pageTitle(rawTitle);
    const space = this.#space(spaceId);
    const parent = parentId === null || parentId === undefined ? null : this.#liveParent(space, parentId);

    this.#remember(space);
    const now = this.#deps.now();
    const page: PageRecord = {
      id: this.#deps.newId(),
      title,
      icon: undefined,
      cover: undefined,
      coverY: undefined,
      font: undefined,
      smallText: false,
      locked: false,
      createdAt: now,
      updatedAt: now,
      createdBy: this.#deps.deviceId?.(),
      editedBy: this.#deps.deviceId?.(),
      parentId: parent === null ? null : parent.id,
      children: [],
      trashedAt: undefined,
      trash: undefined,
    };
    space.pages.set(page.id, page);
    const siblings = parent === null ? space.roots : parent.children;
    siblings.splice(clamp(index, siblings.length), 0, page.id);
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    return this.#pageInfo(space, page);
  }

  /**
   * Creates a whole tree of pages in one change, as an import does (ADR-0036: "in as few commits
   * as the model allows, so every link can resolve"): one undo entry and one `StructureChanged`.
   * `specs` come in creation order; `parent` is the index of an earlier spec, or `null` for the
   * destination (the live page `parentId`, or the space root), where the page is appended after the
   * existing children. Titles, icons and covers (ADR-0040) are checked like `PageCreate`, `PageSetIcon` and `PageSetCover` check them,
   * all before anything changes. Answers the new pages in the order of `specs`.
   */
  pagesCreate(
    spaceId: SpaceId,
    parentId: PageId | null | undefined,
    specs: readonly {
      parent: number | null;
      title: string;
      icon?: string | undefined;
      cover?: string | undefined;
      coverY?: number | undefined;
    }[],
  ): PageInfo[] {
    const space = this.#space(spaceId);
    const destination = parentId === null || parentId === undefined ? null : this.#liveParent(space, parentId);
    const titles = specs.map((spec) => pageTitle(spec.title));
    const icons = specs.map((spec) => (spec.icon === undefined ? undefined : validIcon(spec.icon)));
    const covers = specs.map((spec) => (spec.cover === undefined ? undefined : validCover(spec.cover)));
    covers.forEach((cover) => {
      this.#checkPageImageAttachment(space.id, "cover", coverAttachment(cover));
    });
    const coverYs = specs.map((spec, at) =>
      covers[at] === undefined ? undefined : storedCoverY(validCoverY(spec.coverY)),
    );
    specs.forEach((spec, at) => {
      if (spec.parent !== null && !(Number.isInteger(spec.parent) && spec.parent >= 0 && spec.parent < at)) {
        throw invalid("parent", "a page's parent must be an earlier page of the import");
      }
    });
    this.#remember(space);
    const now = this.#deps.now();
    const made: PageRecord[] = [];
    specs.forEach((spec, at) => {
      const parent = spec.parent === null ? destination : (made[spec.parent] ?? null);
      const page: PageRecord = {
        id: this.#deps.newId(),
        title: titles[at] ?? "",
        icon: icons[at],
        cover: covers[at],
        coverY: coverYs[at],
        font: undefined,
        smallText: false,
        locked: false,
        createdAt: now,
        updatedAt: now,
        createdBy: this.#deps.deviceId?.(),
        editedBy: this.#deps.deviceId?.(),
        parentId: parent === null ? null : parent.id,
        children: [],
        trashedAt: undefined,
        trash: undefined,
      };
      space.pages.set(page.id, page);
      (parent === null ? space.roots : parent.children).push(page.id);
      made.push(page);
    });
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    return made.map((page) => this.#pageInfo(space, page));
  }

  pageRename(pageId: PageId, rawTitle: string): PageInfo {
    const title = pageTitle(rawTitle);
    const { space, page } = this.#locate(pageId);
    this.#remember(space);
    page.title = title;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  pageSetIcon(pageId: PageId, rawIcon: string | null): PageInfo {
    const icon = validIcon(rawIcon);
    const { space, page } = this.#locate(pageId);
    this.#checkPageIconAttachment(space.id, icon);
    this.#remember(space);
    page.icon = icon;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  /**
   * `PageSetCover` (ADR-0040): sets, repositions or clears the cover of a page (live or trashed).
   * `cover` set: that cover with `coverY` (the default when none). `cover` absent with `coverY`: only the
   * position changes (`InvalidInput` on `coverY` when the page has no cover). Both absent: both are
   * cleared. A position of 50 is stored as none. Setting what the page already has changes nothing (no
   * undo entry, no event).
   */
  pageSetCover(pageId: PageId, rawCover: string | null | undefined, rawCoverY: number | null | undefined): PageInfo {
    const cover = validCover(rawCover);
    const coverY = validCoverY(rawCoverY);
    const { space, page } = this.#locate(pageId);
    let nextCover: string | undefined;
    let nextY: number | undefined;
    if (cover === undefined && coverY === undefined) {
      nextCover = undefined;
      nextY = undefined;
    } else if (cover === undefined) {
      if (page.cover === undefined) {
        throw invalid("coverY", "the page has no cover to reposition");
      }
      nextCover = page.cover;
      nextY = coverY;
    } else {
      nextCover = cover;
      nextY = coverY;
    }
    // The cover the page already has is not checked again (its image may be gone by now).
    if (nextCover !== page.cover) {
      this.#checkPageImageAttachment(space.id, "cover", coverAttachment(nextCover));
    }
    nextY = nextCover === undefined ? undefined : storedCoverY(nextY);
    if (page.cover === nextCover && page.coverY === nextY) {
      return this.#pageInfo(space, page);
    }
    this.#remember(space);
    page.cover = nextCover;
    page.coverY = nextY;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  /**
   * `PageSetTemplate` (ADR-0056, `docs/architecture/templates.md`): makes a page a template, or a page again; the
   * sub-pages are inside the template without carrying the flag. `InvalidInput` (`pageId`) for a page in the trash.
   * Setting what the page already is changes nothing (no undo entry, no event, no stamp). Written like the appearance: one
   * undo entry, the page stamped as edited, `StructureChanged`. A page that stops being a template keeps its placeholders
   * (ignored until it is in a template again). Answers the page.
   */
  pageSetTemplate(pageId: PageId, template: boolean): PageInfo {
    const { space, page } = this.#locate(pageId);
    if (page.trashedAt !== undefined) {
      throw invalid("pageId", "the page is in the trash");
    }
    if ((page.template === true) === template) {
      return this.#pageInfo(space, page);
    }
    this.#remember(space);
    page.template = template ? true : undefined;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  /**
   * `PageSetTemplatePlaceholder` (ADR-0056): sets or clears (`undefined`) the placeholder of one property on a template
   * page or a page inside one; `kindOf` gives the property's kind (from the memory properties), `undefined` for an unknown
   * property. `NotFound` for an unknown page or property; `InvalidInput` (`pageId`) for a page in the trash or one that is not
   * in a template (it and its ancestors have no flag), and (`placeholder`) for a placeholder the property's kind does not
   * take (`today` and `now` need a `date` property, `me` a `person` property). Setting a placeholder removes the page's
   * stored value of the property in the same undo entry (an undo stores it again). Setting what the page already has
   * changes nothing. Otherwise one undo entry, the page stamped as edited, `StructureChanged`. Answers the page.
   */
  pageSetTemplatePlaceholder(
    pageId: PageId,
    propertyId: PropertyId,
    placeholder: TemplatePlaceholder | undefined,
    kindOf: (propertyId: PropertyId) => PropertyKind | undefined,
  ): PageInfo {
    const { space, page } = this.#locate(pageId);
    const kind = kindOf(propertyId);
    if (kind === undefined) {
      throw notFound("property not found");
    }
    if (page.trashedAt !== undefined) {
      throw invalid("pageId", "the page is in the trash");
    }
    if (!this.#inTemplate(space, page)) {
      throw invalid("pageId", "the page is not a template and not inside one");
    }
    if (placeholder !== undefined && !placeholderTakes(placeholder, kind)) {
      throw invalid("placeholder", "this kind of property does not take that placeholder");
    }
    if (page.templateValues?.get(propertyId) === placeholder) {
      return this.#pageInfo(space, page);
    }
    // The value the placeholder replaces goes in the same step.
    const value = placeholder === undefined ? undefined : this.#deps.storedValue?.(space.id, pageId, propertyId);
    this.#remember(space, value === undefined ? undefined : [{ pageId, propertyId, value }]);
    if (value !== undefined) {
      this.#deps.restoreValue?.(space.id, pageId, propertyId, undefined);
    }
    const next = new Map(page.templateValues);
    if (placeholder === undefined) {
      next.delete(propertyId);
    } else {
      next.set(propertyId, placeholder);
    }
    page.templateValues = next.size > 0 ? next : undefined;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  /**
   * `PagePropertySet` on a property that has a placeholder on the page (ADR-0056, `MemoryProperties.pageSet`): removes the
   * placeholder, whether the value is set or cleared. No undo entry of its own and no event (the caller's change is not an
   * undo step and emits its own), but no undo or redo brings the placeholder back next to the value that replaced it:
   * it is taken out of the history's snapshots too. Answers whether the page had one.
   */
  templatePlaceholderDrop(pageId: PageId, propertyId: PropertyId): boolean {
    const { space, page } = this.#locate(pageId);
    if (page.templateValues?.has(propertyId) !== true) {
      return false;
    }
    const without = (record: PageRecord): void => {
      const next = new Map(record.templateValues);
      next.delete(propertyId);
      record.templateValues = next.size > 0 ? next : undefined;
    };
    without(page);
    for (const snapshot of [...space.undo, ...space.redo]) {
      const record = snapshot.pages.get(pageId);
      if (record?.templateValues?.has(propertyId) === true) {
        without(record);
      }
    }
    return true;
  }

  /**
   * The flag and the placeholders of a page as stored (ADR-0056), whether or not the page is in a template now (a move
   * to another space carries them, `memory-move`); `undefined` for an unknown page.
   */
  templateOf(pageId: PageId): { template: boolean; values: ReadonlyMap<PropertyId, TemplatePlaceholder> } | undefined {
    const found = this.#findSpaceOf(pageId)?.pages.get(pageId);
    return found === undefined ? undefined : { template: found.template === true, values: found.templateValues ?? new Map() };
  }

  /**
   * Writes the flag and the placeholders of a page as an import or a move settles them (ADR-0056): no check, no undo entry
   * of its own (the caller's step covers it), no event, no stamp. `values` replaces the map. Nothing for an unknown page.
   */
  templateSet(pageId: PageId, template: boolean, values: ReadonlyMap<PropertyId, TemplatePlaceholder>): void {
    const space = this.#findSpaceOf(pageId);
    const page = space?.pages.get(pageId);
    if (space === undefined || page === undefined) {
      return;
    }
    const kept = values.size > 0 ? new Map(values) : undefined;
    page.template = template ? true : undefined;
    page.templateValues = kept;
    // The copies of the page in the history (a moved page is grafted into the target's) say the same.
    for (const snapshot of [...space.undo, ...space.redo]) {
      const record = snapshot.pages.get(pageId);
      if (record !== undefined) {
        record.template = page.template;
        record.templateValues = kept;
      }
    }
  }

  /**
   * `PageSetAppearance` (ADR-0040): sets or clears the font, the small text and the lock of a page (live or
   * trashed). A part that is `undefined` is left as it is; `"default"` and `false` clear. Setting what the page
   * already has changes nothing (no undo entry, no event, no stamp). The lock is a guard of the UI: nothing here
   * refuses a write to a locked page.
   */
  pageSetAppearance(pageId: PageId, raw: { font?: unknown; smallText?: unknown; locked?: unknown }): PageInfo {
    const wanted: PageAppearanceChange = validAppearance(raw);
    const { space, page } = this.#locate(pageId);
    const font = wanted.font === undefined ? page.font : storedFont(wanted.font);
    const smallText = wanted.smallText ?? page.smallText;
    const locked = wanted.locked ?? page.locked;
    if (page.font === font && page.smallText === smallText && page.locked === locked) {
      return this.#pageInfo(space, page);
    }
    this.#remember(space);
    page.font = font;
    page.smallText = smallText;
    page.locked = locked;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  pageMove(pageId: PageId, parentId: PageId | null | undefined, rawIndex: number): void {
    const index = position(rawIndex);
    const { space, page } = this.#locate(pageId);
    const parentKey = parentId ?? null;
    if (parentKey !== null) {
      const owner = this.#findSpaceOf(parentKey);
      if (owner === undefined) {
        throw notFound("page not found");
      }
      if (owner !== space) {
        throw invalid("parentId", "the parent is in another space: use PageMoveToSpace");
      }
    }
    if (page.trashedAt !== undefined) {
      throw invalid("pageId", "the page is in the trash");
    }
    const parent = parentKey === null ? null : this.#liveParent(space, parentKey);
    if (parentKey !== null && (parentKey === pageId || this.#isDescendant(space, parentKey, pageId))) {
      throw invalid("parentId", "a page cannot be moved under its own descendant");
    }

    this.#remember(space);
    this.#detach(space, page);
    // `index` counts the new siblings without the page itself (the position after the move).
    const siblings = parent === null ? space.roots : parent.children;
    siblings.splice(clamp(index, siblings.length), 0, page.id);
    page.parentId = parentKey;
    this.#stamp(page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
  }

  /**
   * `PageReorderChildren` (ADR-0052): puts the live children of `parentId` (`null` or `undefined`: the live
   * top-level pages) into `order`, as ONE undo entry and one `StructureChanged`. `order` must be exactly the
   * live children, each once (`InvalidInput` on `order` otherwise, after `NotFound` for the space and
   * `InvalidInput` on `parentId` for a parent that is not a live page of it). Nothing is written, no entry and
   * no event, when the children already stand in that order. No page is stamped as edited.
   */
  pageReorderChildren(spaceId: SpaceId, parentId: PageId | null | undefined, order: readonly PageId[]): void {
    const space = this.#space(spaceId);
    const parent = parentId === null || parentId === undefined ? null : this.#liveParent(space, parentId);
    const siblings = parent === null ? space.roots : parent.children;
    const children = new Set(siblings);
    if (order.length !== siblings.length || new Set(order).size !== order.length || !order.every((id) => children.has(id))) {
      throw invalid("order", "the order is not the current children of the parent");
    }
    if (order.every((id, at) => siblings[at] === id)) {
      return;
    }
    this.#remember(space);
    siblings.splice(0, siblings.length, ...order);
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
  }

  /**
   * The pages of `pageIds` that have no ancestor in `pageIds`, in the order given (`StructureDoc::without_descendants`):
   * the others go with their ancestor in a structural bulk action (ADR-0058). A page the space does not know is kept.
   */
  withoutDescendants(spaceId: SpaceId, pageIds: readonly PageId[]): PageId[] {
    const space = this.#space(spaceId);
    const listed = new Set(pageIds);
    return pageIds.filter((pageId) => {
      let current = space.pages.get(pageId)?.parentId ?? null;
      for (let hops = 0; current !== null && hops <= space.pages.size; hops += 1) {
        if (listed.has(current)) {
          return false;
        }
        current = space.pages.get(current)?.parentId ?? null;
      }
      return true;
    });
  }

  /**
   * `PageBulk` `Move` on the structure (ADR-0058): puts the live pages `tops` (none an ancestor of another), each with its
   * subtree, last under `parentId` (`null` or `undefined`: the live top level) in the order given, as ONE undo entry and one
   * `StructureChanged`; each page is stamped as `pageMove` stamps it. `InvalidInput` for `parentId` when it is not a live page of
   * the space or is one of the pages or below one; nothing is written then. Pages already last in that order write nothing (no
   * undo entry, no stamp) but the event, as `mutate_structure` of the core does.
   */
  moveMany(spaceId: SpaceId, tops: readonly PageId[], parentId: PageId | null | undefined): void {
    const space = this.#space(spaceId);
    const parentKey = parentId ?? null;
    const parent = parentKey === null ? null : this.#liveParent(space, parentKey);
    if (parentKey !== null && tops.some((top) => top === parentKey || this.#isDescendant(space, parentKey, top))) {
      throw invalid("parentId", "a page cannot be moved under its own descendant");
    }
    const pages = tops.map((pageId) => {
      const page = space.pages.get(pageId);
      if (page === undefined) {
        throw notFound("page not found");
      }
      if (page.trashedAt !== undefined) {
        throw invalid("pageId", "the page is in the trash");
      }
      return page;
    });
    const siblings = parent === null ? space.roots : parent.children;
    const first = siblings.length - tops.length;
    if (first >= 0 && tops.every((pageId, at) => siblings[first + at] === pageId)) {
      this.#emit({ type: "StructureChanged", data: { spaceId } });
      return;
    }
    this.#remember(space);
    const now = this.#deps.now();
    for (const page of pages) {
      this.#detach(space, page);
      (parent === null ? space.roots : parent.children).push(page.id);
      page.parentId = parentKey;
      this.#stamp(page, now);
    }
    this.#emit({ type: "StructureChanged", data: { spaceId } });
  }

  /**
   * What `PageMoveToSpace` and its plan check (ADR-0038), in the core's order: the page exists
   * (`NotFound`) and is live (`InvalidInput` for `pageId`), the target space exists (`NotFound`) and is
   * not the page's own (`InvalidInput` for `spaceId`: use `PageMove`), the parent is a live page of the
   * target (`InvalidInput` for `parentId`), the subtree has at most {@link MAX_MOVED_PAGES} pages
   * (`InvalidInput` for `pageId`). Writes nothing.
   */
  moveCheck(pageId: PageId, spaceId: SpaceId, parentId: PageId | null | undefined): MoveCheck {
    const { space, page } = this.#locate(pageId);
    if (page.trashedAt !== undefined) {
      throw invalid("pageId", "the page is in the trash");
    }
    const target = this.#space(spaceId);
    if (target === space) {
      throw invalid("spaceId", "the page is already in this space: use PageMove");
    }
    if (parentId !== null && parentId !== undefined) {
      this.#liveParent(target, parentId);
    }
    const pageIds = this.#subtree(space, pageId).map((member) => member.id);
    if (pageIds.length > MAX_MOVED_PAGES) {
      throw invalid("pageId", `a move carries at most ${String(MAX_MOVED_PAGES)} pages`);
    }
    return { fromSpaceId: space.id, pageIds };
  }

  /**
   * `PageMoveToSpace` on the structures (ADR-0038): the page and its live subtree leave the source
   * space and arrive in the target under `parentId` (`null` or `undefined`: its root) at `rawIndex`
   * (clamped; `undefined`: last), with the same ids, titles, icons and times. The favourites among them
   * leave the source's list and join the end of the target's, in the order of the move. Not an undo
   * step: the pages are taken out of the source's undo and redo snapshots (an undo can never bring them
   * back there) and put into the target's (an undo there never takes them away). Emits
   * `PagesMovedSpace`, `StructureChanged` for the source then the target, and `SpacesChanged` (the
   * page counts changed). Values, attachments and views are the owner's to follow.
   */
  moveToSpace(
    pageId: PageId,
    spaceId: SpaceId,
    parentId: PageId | null | undefined,
    rawIndex: number | undefined,
  ): MovedPages {
    const index = rawIndex === undefined ? undefined : position(rawIndex);
    const { fromSpaceId, pageIds } = this.moveCheck(pageId, spaceId, parentId);
    const from = this.#space(fromSpaceId);
    const to = this.#space(spaceId);
    const moving = new Set(pageIds);
    const records = pageIds.flatMap((id) => {
      const record = from.pages.get(id);
      return record === undefined ? [] : [record];
    });
    const top = from.pages.get(pageId);
    if (top === undefined) {
      throw notFound("page not found");
    }
    const parent = parentId === null || parentId === undefined ? null : this.#liveParent(to, parentId);
    this.#detach(from, top);
    for (const record of records) {
      from.pages.delete(record.id);
      to.pages.set(record.id, record);
    }
    top.parentId = parent === null ? null : parent.id;
    const siblings = parent === null ? to.roots : parent.children;
    siblings.splice(clamp(index, siblings.length), 0, pageId);
    const favorites = pageIds.filter((id) => from.favorites.includes(id));
    from.favorites = from.favorites.filter((id) => !moving.has(id));
    to.favorites.push(...favorites.filter((id) => !to.favorites.includes(id)));
    // History: the pages are not part of the source's any more, and stay part of the target's.
    for (const snapshot of [...from.undo, ...from.redo]) {
      strip(snapshot, moving);
    }
    for (const snapshot of [...to.undo, ...to.redo]) {
      graft(snapshot, records, parent === null ? null : parent.id);
    }
    this.#emit({ type: "PagesMovedSpace", data: { fromSpaceId, toSpaceId: spaceId, pageIds: [...pageIds] } });
    this.#emit({ type: "StructureChanged", data: { spaceId: fromSpaceId } });
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    this.#emit({ type: "SpacesChanged" });
    return { fromSpaceId, pageIds, favorites };
  }

  /**
   * What `PageDuplicate` checks (ADR-0040), in the core's order: the page exists (`NotFound`), is not in the trash
   * (`InvalidInput` for `pageId`), and its live subtree has at most {@link MAX_DUPLICATED_PAGES} pages (`InvalidInput`
   * for `pageId`). Writes nothing.
   */
  duplicateCheck(pageId: PageId): DuplicateCheck {
    const { space, page } = this.#locate(pageId);
    if (page.trashedAt !== undefined) {
      throw invalid("pageId", "the page is in the trash");
    }
    const pageIds = this.#subtree(space, pageId).map((member) => member.id);
    if (pageIds.length > MAX_DUPLICATED_PAGES) {
      throw invalid("pageId", `a page is duplicated with at most ${String(MAX_DUPLICATED_PAGES)} pages`);
    }
    return { spaceId: space.id, pageIds };
  }

  /**
   * What `PageCreateFromTemplate` checks about the template and the parent (ADR-0056, `docs/architecture/templates.md`
   * section 5.1), in the core's order: the template exists (`NotFound`), is not in the trash and is a template
   * (`InvalidInput` for `templateId`); the parent, when given, is a live page of the template's space (`InvalidInput` for
   * `parentId`) that is not in the template's subtree (`parentId`); the subtree has at most {@link MAX_DUPLICATED_PAGES} pages
   * (`templateId`). Writes nothing.
   */
  templateCheck(templateId: PageId, parentId: PageId | null | undefined): DuplicateCheck {
    const { space, page } = this.#locate(templateId);
    if (page.trashedAt !== undefined) {
      throw invalid("templateId", "the template is in the trash");
    }
    if (page.template !== true) {
      throw invalid("templateId", "the page is not a template");
    }
    const pageIds = this.#subtree(space, templateId).map((member) => member.id);
    if (parentId !== null && parentId !== undefined) {
      const parent = space.pages.get(parentId);
      if (parent === undefined || parent.trashedAt !== undefined) {
        throw invalid("parentId", "the parent page does not exist, is in the trash or is in another space");
      }
      if (pageIds.includes(parentId)) {
        throw invalid("parentId", "a page cannot be made inside the template it is copied from");
      }
    }
    if (pageIds.length > MAX_DUPLICATED_PAGES) {
      throw invalid("templateId", `a template is copied with at most ${String(MAX_DUPLICATED_PAGES)} pages`);
    }
    return { spaceId: space.id, pageIds };
  }

  /**
   * `PageDuplicate` on the structure (ADR-0040): creates the copy of a live page and of its live subtree, with the ids
   * `ids` holds for every page of {@link MemoryStructure.duplicateCheck}'s `pageIds`. The top copy goes right after
   * the page among its siblings and is titled `topTitle`; the others hang under the copies of their parents, with
   * their titles; icons, covers and the look (font, small text, lock) are kept, and so are the template flag and the
   * placeholders (the duplicate of a template is a template, ADR-0056); times and creator are new, favourites are not copied.
   * One undo entry. Emits `StructureChanged` then `SpacesChanged` (the page counts changed). Answers the copies, parents
   * first. Values, documents and attachment references are the owner's to follow.
   */
  duplicate(pageId: PageId, ids: ReadonlyMap<PageId, PageId>, topTitle: string): PageId[] {
    const { spaceId, pageIds } = this.duplicateCheck(pageId);
    return this.#copyPages(spaceId, pageIds, ids, { kind: "duplicate", topTitle });
  }

  /**
   * `PageCreateFromTemplate` on the structure (ADR-0056): creates the copy of a template's live subtree (see
   * {@link MemoryStructure.templateCheck}, which this runs first) under `parentId` (the space's top level when `null` or
   * absent) at `index` (last when absent, clamped), with the ids `ids` holds for every page of its `pageIds`. The copies are
   * pages (no flag, no placeholders), titled with the template's titles with the tokens `{{date}}` and `{{time}}` resolved
   * from `moment` (no " (copy)"); icons, covers and the look are kept, times and creator are new. One undo entry. Emits
   * `StructureChanged` then `SpacesChanged`. Answers the copies, parents first. Values, documents and attachment references
   * are the owner's to follow.
   */
  createFromTemplate(
    templateId: PageId,
    ids: ReadonlyMap<PageId, PageId>,
    place: { parentId: PageId | null | undefined; index: number | undefined; moment: LocalMoment },
  ): PageId[] {
    const { spaceId, pageIds } = this.templateCheck(templateId, place.parentId);
    return this.#copyPages(spaceId, pageIds, ids, {
      kind: "template",
      parentId: place.parentId ?? null,
      index: place.index,
      moment: place.moment,
    });
  }

  /** The copies of a checked subtree (see `duplicate` and `createFromTemplate`), `pageIds` parents first. */
  #copyPages(spaceId: SpaceId, pageIds: readonly PageId[], ids: ReadonlyMap<PageId, PageId>, style: CopyStyle): PageId[] {
    const space = this.#space(spaceId);
    const pairs = this.#copyPairs(space, pageIds, ids, new Set());
    this.#remember(space);
    this.#writeCopies(space, pairs, ids, style, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    this.#emit({ type: "SpacesChanged" });
    return pairs.map((pair) => pair.copyId);
  }

  /**
   * `PageBulk` `Duplicate` on the structure (ADR-0058): the copies of several live pages with their live subtrees, one
   * group (`pageIds`: the page and its subtree, parents first, and the title of its copy) for each page, none an ancestor of
   * another. Each top copy goes right after its page; all the copies are ONE undo entry. `ids` holds the copy of every page
   * of every group. Emits `StructureChanged` then `SpacesChanged` once. Answers the copies of all the groups in order,
   * parents first. Values, documents and attachment references are the owner's to follow.
   */
  duplicateMany(
    spaceId: SpaceId,
    groups: readonly { pageIds: readonly PageId[]; topTitle: string }[],
    ids: ReadonlyMap<PageId, PageId>,
  ): PageId[] {
    const space = this.#space(spaceId);
    const used = new Set<PageId>();
    const checked = groups.map((group) => ({ group, pairs: this.#copyPairs(space, group.pageIds, ids, used) }));
    this.#remember(space);
    const now = this.#deps.now();
    for (const { group, pairs } of checked) {
      this.#writeCopies(space, pairs, ids, { kind: "duplicate", topTitle: group.topTitle }, now);
    }
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    this.#emit({ type: "SpacesChanged" });
    return checked.flatMap(({ pairs }) => pairs.map((pair) => pair.copyId));
  }

  /**
   * The pages of a subtree with the copy each gets, checked before anything is written: every page has a copy id that is
   * free and used once (`used` holds the ids of the groups before; it gets this group's).
   */
  #copyPairs(
    space: SpaceRecord,
    pageIds: readonly PageId[],
    ids: ReadonlyMap<PageId, PageId>,
    used: Set<PageId>,
  ): { source: PageRecord; copyId: PageId }[] {
    const pairs: { source: PageRecord; copyId: PageId }[] = [];
    for (const id of pageIds) {
      const source = space.pages.get(id);
      if (source === undefined) {
        continue;
      }
      const copyId = ids.get(source.id);
      if (copyId === undefined || space.pages.has(copyId) || used.has(copyId)) {
        throw internalError("a page of the subtree has no usable copy id");
      }
      used.add(copyId);
      pairs.push({ source, copyId });
    }
    return pairs;
  }

  /** Creates the copies of a checked subtree (`pairs`, parents first; the first is the top), with the style's titles and place. */
  #writeCopies(
    space: SpaceRecord,
    pairs: readonly { source: PageRecord; copyId: PageId }[],
    ids: ReadonlyMap<PageId, PageId>,
    style: CopyStyle,
    now: number,
  ): void {
    const topParent = style.kind === "template" ? style.parentId : null;
    const device = this.#deps.deviceId?.();
    const made = new Map<PageId, PageRecord>();
    for (const [at, { source, copyId }] of pairs.entries()) {
      const parentCopy = at === 0 || source.parentId === null ? undefined : made.get(ids.get(source.parentId) ?? "");
      const topParentId = style.kind === "template" ? topParent : source.parentId;
      const copy: PageRecord = {
        id: copyId,
        title:
          style.kind === "template"
            ? resolveTitleTokens(source.title, style.moment, MAX_TITLE_CHARS)
            : at === 0
              ? style.topTitle
              : source.title,
        icon: source.icon,
        cover: source.cover,
        coverY: source.coverY,
        font: source.font,
        smallText: source.smallText,
        locked: source.locked,
        // The duplicate of a template is a template, with its placeholders; a page made from one is not (ADR-0056).
        template: style.kind === "duplicate" ? source.template : undefined,
        templateValues: style.kind === "duplicate" ? source.templateValues : undefined,
        createdAt: now,
        updatedAt: now,
        createdBy: device,
        editedBy: device,
        // The top copy goes under its chosen parent (a duplicate's: the page's own); the others hang under the copy of
        // their parent.
        parentId: at === 0 ? topParentId : (parentCopy?.id ?? null),
        children: [],
        trashedAt: undefined,
        trash: undefined,
      };
      space.pages.set(copy.id, copy);
      made.set(copy.id, copy);
      if (at !== 0) {
        parentCopy?.children.push(copy.id);
      } else if (style.kind === "template") {
        const siblings = topParent === null ? space.roots : (space.pages.get(topParent)?.children ?? space.roots);
        siblings.splice(clamp(style.index, siblings.length), 0, copy.id);
      } else {
        const siblings =
          source.parentId === null ? space.roots : (space.pages.get(source.parentId)?.children ?? space.roots);
        siblings.splice(siblings.indexOf(source.id) + 1, 0, copy.id);
      }
    }
  }

  pageTrash(pageId: PageId): void {
    const { space, page } = this.#locate(pageId);
    if (page.trashedAt !== undefined) {
      throw invalid("pageId", "the page is in the trash");
    }
    this.#remember(space);
    this.#trash(space, page, this.#deps.now());
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
  }

  /**
   * `PageBulk` `Trash` on the structure (ADR-0058): moves the live pages `tops` (none an ancestor of another), each with its
   * subtree, to the trash in the order given, as ONE undo entry, one `StructureChanged` and one `SpacesChanged`. `NotFound` for an unknown page,
   * `InvalidInput` for `pageId` for one already in the trash; nothing is written then.
   */
  trashMany(spaceId: SpaceId, tops: readonly PageId[]): void {
    const space = this.#space(spaceId);
    const pages = tops.map((pageId) => {
      const page = space.pages.get(pageId);
      if (page === undefined) {
        throw notFound("page not found");
      }
      if (page.trashedAt !== undefined) {
        throw invalid("pageId", "the page is in the trash");
      }
      return page;
    });
    this.#remember(space);
    const now = this.#deps.now();
    for (const page of pages) {
      this.#trash(space, page, now);
    }
    this.#emit({ type: "StructureChanged", data: { spaceId } });
    // The live page count of the space changed, which the core announces for a trash (`mutate_structure`).
    this.#emit({ type: "SpacesChanged" });
  }

  /** Moves a live page and its subtree to the trash: what `pageTrash` and `trashMany` write after their checks. */
  #trash(space: SpaceRecord, page: PageRecord, now: number): void {
    this.#detach(space, page);
    this.#trashSeq += 1;
    page.trash = { seq: this.#trashSeq, from: page.parentId };
    page.parentId = null;
    this.#stamp(page, now);
    for (const member of this.#subtree(space, page.id)) {
      member.trashedAt = now;
    }
  }

  pageRestore(pageId: PageId): PageInfo {
    const { space, page } = this.#locate(pageId);
    if (page.trashedAt === undefined) {
      throw invalid("pageId", "the page is not in the trash");
    }
    if (page.trash === undefined) {
      // Inside a trashed subtree: only the page that was trashed itself can be restored.
      throw invalid("pageId", "the page is in the trash");
    }
    this.#remember(space);
    // Back to the previous parent if that page is live, otherwise appended to the root.
    const former = page.trash.from === null ? undefined : space.pages.get(page.trash.from);
    const parent = former !== undefined && former.trashedAt === undefined ? former : null;
    const siblings = parent === null ? space.roots : parent.children;
    siblings.push(pageId);
    page.parentId = parent === null ? null : parent.id;
    page.trash = undefined;
    this.#stamp(page, this.#deps.now());
    for (const member of this.#subtree(space, pageId)) {
      member.trashedAt = undefined;
    }
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return this.#pageInfo(space, page);
  }

  /**
   * Removes the page and its subtree for good and returns their ids (parents first). Works
   * on live and on trashed pages.
   */
  pagePurge(pageId: PageId): PageId[] {
    const { space, page } = this.#locate(pageId);
    this.#remember(space);
    this.#detach(space, page);
    const purged = this.#subtree(space, pageId).map((member) => member.id);
    for (const id of purged) {
      space.pages.delete(id);
    }
    // A purged page leaves the favourites; not an undo step (the snapshots do not hold them).
    const gone = new Set(purged);
    space.favorites = space.favorites.filter((id) => !gone.has(id));
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
    return purged;
  }

  /**
   * Adds the page to its space's favourites (last) or removes it (`PageFavoriteSet`,
   * `StructureDoc::set_favorite`). Idempotent. Adding needs a page that is not in the trash
   * (`InvalidInput` on `pageId`); removing works for any page. Not an undo step. Emits
   * `StructureChanged`, also when nothing changed.
   */
  pageFavoriteSet(pageId: PageId, favorite: boolean): void {
    const { space, page } = this.#locate(pageId);
    if (favorite) {
      if (page.trashedAt !== undefined) {
        throw invalid("pageId", "the page is in the trash");
      }
      if (!space.favorites.includes(pageId)) {
        space.favorites.push(pageId);
      }
    } else {
      space.favorites = space.favorites.filter((id) => id !== pageId);
    }
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
  }

  /**
   * Moves a favourite in front of `before` (another live favourite of the same space), or last
   * for `null` or `undefined` (`PageFavoriteMove`, `StructureDoc::move_favorite`). `InvalidInput`
   * on `pageId` when the page is not a favourite, on `before` when that page is not one. Not an
   * undo step. Emits `StructureChanged`, also when nothing changed.
   */
  pageFavoriteMove(pageId: PageId, before: PageId | null | undefined): void {
    const { space } = this.#locate(pageId);
    const live = this.#liveFavorites(space);
    if (!live.includes(pageId)) {
      throw invalid("pageId", "the page is not a favourite");
    }
    const target = before ?? null;
    if (target !== pageId) {
      if (target !== null && !live.includes(target)) {
        throw invalid("before", "the page is not a favourite");
      }
      // Hidden entries keep their relative places; the live ones come out in the new order.
      const rest = space.favorites.filter((id) => id !== pageId);
      rest.splice(target === null ? rest.length : rest.indexOf(target), 0, pageId);
      space.favorites = rest;
    }
    this.#emit({ type: "StructureChanged", data: { spaceId: space.id } });
  }

  /** Undoes the last structural change in the space; with nothing to undo, nothing changes and nothing is emitted. */
  structureUndo(spaceId: SpaceId): TreeView {
    return this.#step(spaceId, "undo");
  }

  /** Redoes the last undone structural change; see `structureUndo`. */
  structureRedo(spaceId: SpaceId): TreeView {
    return this.#step(spaceId, "redo");
  }

  // ----- backups ----------------------------------------------------------------------

  /** Every space in display order with all its pages, as plain data (for a backup). */
  exportState(): SpaceState[] {
    return this.#order.map((spaceId) => {
      const space = this.#space(spaceId);
      const state: SpaceState = {
        id: space.id,
        name: space.name,
        createdAt: space.createdAt,
        roots: [...space.roots],
        pages: [...space.pages.values()].map(pageState),
      };
      if (space.icon !== undefined) {
        state.icon = space.icon;
      }
      if (space.favorites.length > 0) {
        state.favorites = [...space.favorites];
      }
      return state;
    });
  }

  /**
   * Checks that `spaces` form consistent page trees (unique ids; roots, children and parents
   * that exist in their space) and returns the function that replaces every space with them,
   * without events and without undo history. Nothing changes until it is called; an
   * inconsistent tree throws an `Internal` error first.
   */
  prepareRestore(spaces: readonly SpaceState[]): () => void {
    const restored = new Map<SpaceId, SpaceRecord>();
    const pageIds = new Set<PageId>();
    let trashSeq = this.#trashSeq;
    for (const state of spaces) {
      if (restored.has(state.id)) {
        throw internalError("a space appears twice");
      }
      const pages = new Map<PageId, PageRecord>();
      for (const page of state.pages) {
        if (pageIds.has(page.id)) {
          throw internalError("a page appears twice");
        }
        pageIds.add(page.id);
        pages.set(page.id, pageRecord(page));
        trashSeq = Math.max(trashSeq, page.trash?.seq ?? 0);
      }
      const known = (id: PageId | null): boolean => id === null || pages.has(id);
      for (const page of pages.values()) {
        if (!known(page.parentId) || !page.children.every(known)) {
          throw internalError("a page refers to a page that is not in its space");
        }
      }
      if (!state.roots.every(known)) {
        throw internalError("a space root refers to a page that is not in the space");
      }
      restored.set(state.id, {
        id: state.id,
        name: state.name,
        icon: state.icon,
        createdAt: state.createdAt,
        pages,
        roots: [...state.roots],
        favorites: [...(state.favorites ?? [])],
        undo: [],
        redo: [],
      });
    }
    return () => {
      this.#spaces.clear();
      for (const [id, space] of restored) {
        this.#spaces.set(id, space);
      }
      this.#order = [...restored.keys()];
      this.#trashSeq = trashSeq;
    };
  }

  // ----- lookups for the content mirror ---------------------------------------------

  /** Ids of every space, in display order. */
  spaceIds(): SpaceId[] {
    return [...this.#order];
  }

  /** Whether the space exists. */
  hasSpace(spaceId: SpaceId): boolean {
    return this.#spaces.has(spaceId);
  }

  /**
   * Sets a page's `updatedAt` as a property write does (ADR-0026): no undo entry (property
   * writes are not part of structural undo) and no event (the caller emits its own).
   */
  touchPage(pageId: PageId, at: number): void {
    const { page } = this.#locate(pageId);
    this.#stamp(page, at);
  }

  /**
   * Records that the page's content was edited here (`StructureDoc::touch_edited`): sets
   * `updatedAt` and `editedBy`, at most once per {@link EDIT_STAMP_INTERVAL_MS} per device (an
   * edit by another device, or the first by this one, is stamped at once) and never backwards.
   * Answers whether anything was written; no undo entry, no event (the caller emits its own).
   */
  touchEdited(pageId: PageId, at: number): boolean {
    const { page } = this.#locate(pageId);
    if (at <= page.updatedAt) {
      return false;
    }
    const device = this.#deps.deviceId?.();
    const sameAuthor = device === undefined || page.editedBy === device;
    if (sameAuthor && at - page.updatedAt < EDIT_STAMP_INTERVAL_MS) {
      return false;
    }
    this.#stamp(page, at);
    return true;
  }

  /** A live or trashed page; `undefined` once purged (or never created). */
  pageMeta(pageId: PageId): PageMeta | undefined {
    const space = this.#findSpaceOf(pageId);
    const page = space?.pages.get(pageId);
    return space === undefined || page === undefined ? undefined : this.#meta(space, page);
  }

  /**
   * Live pages in display order: spaces in their order (only `spaceId` when given; none
   * for an unknown space), each tree depth-first with children in order.
   */
  livePages(spaceId?: SpaceId): PageMeta[] {
    const found: PageMeta[] = [];
    for (const id of this.#order) {
      const space = this.#spaces.get(id);
      if (space === undefined || (spaceId !== undefined && id !== spaceId)) {
        continue;
      }
      const visit = (pageId: PageId): void => {
        const page = space.pages.get(pageId);
        if (page !== undefined && page.trashedAt === undefined) {
          found.push(this.#meta(space, page));
          page.children.forEach(visit);
        }
      };
      space.roots.forEach(visit);
    }
    return found;
  }

  /** Every page of a space, live and trashed; `undefined` for an unknown space. */
  spacePageIds(spaceId: SpaceId): PageId[] | undefined {
    const space = this.#spaces.get(spaceId);
    return space === undefined ? undefined : [...space.pages.keys()];
  }

  /** The page and its subtree, parents first (what `kb-core` reports as affected by a trash or restore). */
  subtreeIds(pageId: PageId): PageId[] {
    const { space } = this.#locate(pageId);
    return this.#subtree(space, pageId).map((page) => page.id);
  }

  /**
   * How many pages have each attachment as their icon or their cover (`attachment:<id>`, ADR-0029,
   * ADR-0040), over the live and the trashed pages (a purged page is gone): the core's `icon_refs` rows,
   * which count as references next to the blocks and the `files` values. At most one per page and
   * attachment (the same image as icon and cover is one).
   */
  iconReferenceCounts(): Map<AttachmentId, number> {
    const counts = new Map<AttachmentId, number>();
    for (const space of this.#spaces.values()) {
      for (const page of space.pages.values()) {
        for (const attachmentId of pageImages(page)) {
          counts.set(attachmentId, (counts.get(attachmentId) ?? 0) + 1);
        }
      }
    }
    return counts;
  }

  /** The pages that have each attachment as their icon or cover (live and trashed), as `iconReferenceCounts` counts them. */
  iconReferrers(): Map<AttachmentId, Set<PageId>> {
    const found = new Map<AttachmentId, Set<PageId>>();
    for (const space of this.#spaces.values()) {
      for (const page of space.pages.values()) {
        for (const attachmentId of pageImages(page)) {
          const pages = found.get(attachmentId) ?? new Set<PageId>();
          pages.add(page.id);
          found.set(attachmentId, pages);
        }
      }
    }
    return found;
  }

  // ----- plumbing -------------------------------------------------------------------

  /**
   * `kb-core`'s `check_page_icon_attachment`: an `attachment:<id>` icon must name an image
   * attachment of the page's space (so not a purged one, not another space's). Any other icon
   * passes. Seeding is not checked (the attachments are seeded after the pages).
   */
  #checkPageIconAttachment(spaceId: SpaceId, icon: string | undefined): void {
    this.#checkPageImageAttachment(spaceId, "icon", iconAttachment(icon));
  }

  /** The same rule for a cover (ADR-0040): the refusal is on the field `field`. */
  #checkPageImageAttachment(spaceId: SpaceId, field: string, attachmentId: AttachmentId | undefined): void {
    if (attachmentId === undefined || this.#silent) {
      return;
    }
    const found = this.#deps.attachmentOf?.(attachmentId);
    if (found === undefined) {
      throw invalid(field, "the image is not an attachment of this space (unknown or deleted)");
    }
    if (found.spaceId !== spaceId) {
      throw invalid(field, "the image belongs to another space");
    }
    if (!found.mime.toLowerCase().startsWith("image/")) {
      throw invalid(field, "the attachment is not an image (its type must start with image/)");
    }
  }

  #step(spaceId: SpaceId, direction: "undo" | "redo"): TreeView {
    const space = this.#space(spaceId);
    const from = direction === "undo" ? space.undo : space.redo;
    const to = direction === "undo" ? space.redo : space.undo;
    const target = from.pop();
    if (target !== undefined) {
      const mirror = snapshotOf(space);
      // The values the step removed come back (or go again) with it: the other direction holds what they are now.
      if (target.values !== undefined) {
        mirror.values = target.values.map(({ pageId, propertyId }) => ({
          pageId,
          propertyId,
          value: this.#deps.storedValue?.(spaceId, pageId, propertyId),
        }));
      }
      to.push(mirror);
      const before = [...space.pages.keys()];
      space.pages = target.pages;
      space.roots = target.roots;
      for (const edit of target.values ?? []) {
        this.#deps.restoreValue?.(spaceId, edit.pageId, edit.propertyId, edit.value);
      }
      const gone = before.filter((pageId) => !space.pages.has(pageId));
      if (gone.length > 0 && !this.#silent) {
        this.#deps.pagesRemoved?.(gone);
      }
      this.#emit({ type: "StructureChanged", data: { spaceId } });
    }
    return this.treeGet(spaceId);
  }

  /**
   * Records the current structure for undo; called once a change validated, before it is applied. `values`: stored
   * property values the change is about to remove, which an undo of it stores again.
   */
  #remember(space: SpaceRecord, values?: ValueEdit[]): void {
    const snapshot = snapshotOf(space);
    if (values !== undefined) {
      snapshot.values = values;
    }
    space.undo.push(snapshot);
    if (space.undo.length > UNDO_DEPTH) {
      space.undo.shift();
    }
    space.redo.length = 0;
  }

  #emit(event: Event): void {
    if (!this.#silent) {
      this.#deps.emit(event);
    }
  }

  #space(spaceId: SpaceId): SpaceRecord {
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      throw notFound("space not found");
    }
    return space;
  }

  #findSpaceOf(pageId: PageId): SpaceRecord | undefined {
    for (const space of this.#spaces.values()) {
      if (space.pages.has(pageId)) {
        return space;
      }
    }
    return undefined;
  }

  #locate(pageId: PageId): { space: SpaceRecord; page: PageRecord } {
    for (const space of this.#spaces.values()) {
      const page = space.pages.get(pageId);
      if (page !== undefined) {
        return { space, page };
      }
    }
    throw notFound("page not found");
  }

  /**
   * The space's live favourites in order (`StructureDoc::favorites`): pages that exist and are
   * not in the trash. Entries of trashed pages (hidden) and of pages that are gone are left out.
   */
  #liveFavorites(space: SpaceRecord): PageId[] {
    return space.favorites.filter((id) => {
      const page = space.pages.get(id);
      return page !== undefined && page.trashedAt === undefined;
    });
  }

  /** A page that may receive children: it exists in the space and is not in the trash. */
  #liveParent(space: SpaceRecord, parentId: PageId): PageRecord {
    const parent = space.pages.get(parentId);
    if (parent === undefined || parent.trashedAt !== undefined) {
      throw invalid("parentId", "the parent page does not exist or is in the trash");
    }
    return parent;
  }

  /** Takes a page out of the sibling list it is in (nothing to do for a top-level trashed page). */
  #detach(space: SpaceRecord, page: PageRecord): void {
    const siblings = page.parentId === null ? space.roots : space.pages.get(page.parentId)?.children;
    const at = siblings?.indexOf(page.id) ?? -1;
    if (at >= 0) {
      siblings?.splice(at, 1);
    }
  }

  /** The page and everything below it, parents before children. */
  #subtree(space: SpaceRecord, pageId: PageId): PageRecord[] {
    const found: PageRecord[] = [];
    const visit = (id: PageId): void => {
      const page = space.pages.get(id);
      if (page !== undefined) {
        found.push(page);
        page.children.forEach(visit);
      }
    };
    visit(pageId);
    return found;
  }

  /** True when `candidateId` lies below `ancestorId`. */
  #isDescendant(space: SpaceRecord, candidateId: PageId, ancestorId: PageId): boolean {
    let current = space.pages.get(candidateId)?.parentId ?? null;
    while (current !== null) {
      if (current === ancestorId) {
        return true;
      }
      current = space.pages.get(current)?.parentId ?? null;
    }
    return false;
  }

  #node(space: SpaceRecord, pageId: PageId): PageNode[] {
    const page = space.pages.get(pageId);
    if (page === undefined) {
      return [];
    }
    return [
      {
        page: this.#pageInfo(space, page),
        children: page.children.flatMap((id) => this.#node(space, id)),
      },
    ];
  }

  #meta(space: SpaceRecord, page: PageRecord): PageMeta {
    return {
      id: page.id,
      spaceId: space.id,
      title: page.title,
      icon: page.icon,
      cover: page.cover,
      trashed: page.trashedAt !== undefined,
      inTemplate: this.#inTemplate(space, page),
      createdAt: page.createdAt,
      updatedAt: page.updatedAt,
      createdBy: page.createdBy,
      editedBy: page.editedBy,
    };
  }

  /** The page is a template or inside one: it or an ancestor has the flag (ADR-0056). */
  #inTemplate(space: SpaceRecord, page: PageRecord): boolean {
    let current: PageRecord | undefined = page;
    while (current !== undefined) {
      if (current.template === true) {
        return true;
      }
      current = current.parentId === null ? undefined : space.pages.get(current.parentId);
    }
    return false;
  }

  /**
   * The placeholders `PageInfo` lists (ADR-0056): only for a page in a template, only the properties that still exist
   * and whose kind takes the placeholder, in schema order.
   */
  #placeholders(space: SpaceRecord, page: PageRecord): [PropertyId, TemplatePlaceholder][] {
    const stored = page.templateValues;
    if (stored === undefined || stored.size === 0 || !this.#inTemplate(space, page)) {
      return [];
    }
    return (this.#deps.propertyKinds?.(space.id) ?? []).flatMap((def): [PropertyId, TemplatePlaceholder][] => {
      const placeholder = stored.get(def.id);
      return placeholder !== undefined && placeholderTakes(placeholder, def.kind) ? [[def.id, placeholder]] : [];
    });
  }

  /** Sets `updatedAt` and, when this device is known, `editedBy` (the core's `stamp_edit`). */
  #stamp(page: PageRecord, at: number): void {
    page.updatedAt = at;
    const device = this.#deps.deviceId?.();
    if (device !== undefined) {
      page.editedBy = device;
    }
  }

  #pageInfo(space: SpaceRecord, page: PageRecord): PageInfo {
    // Optional fields are left out when unset, as the core's serialisation does.
    const info: PageInfo = {
      id: page.id,
      spaceId: space.id,
      title: page.title,
      createdAt: page.createdAt,
      updatedAt: page.updatedAt,
    };
    if (page.parentId !== null) {
      info.parentId = page.parentId;
    }
    if (page.icon !== undefined) {
      info.icon = page.icon;
    }
    if (page.cover !== undefined) {
      info.cover = page.cover;
      if (page.coverY !== undefined) {
        info.coverY = page.coverY;
      }
    }
    if (page.font !== undefined) {
      info.font = page.font;
    }
    if (page.smallText) {
      info.smallText = true;
    }
    if (page.locked) {
      info.locked = true;
    }
    if (page.trashedAt !== undefined) {
      info.trashedAt = page.trashedAt;
    }
    if (page.createdBy !== undefined) {
      info.createdBy = page.createdBy;
    }
    if (page.editedBy !== undefined) {
      info.editedBy = page.editedBy;
    }
    const properties = this.#deps.pageProperties?.(space.id, page.id);
    if (properties !== undefined && properties.length > 0) {
      info.properties = properties;
    }
    if (page.template === true) {
      info.template = true;
    }
    const placeholders = this.#placeholders(space, page);
    if (placeholders.length > 0) {
      info.templatePlaceholders = placeholders;
    }
    return info;
  }

  #spaceInfo(space: SpaceRecord): SpaceInfo {
    let pageCount = 0;
    for (const page of space.pages.values()) {
      if (page.trashedAt === undefined) {
        pageCount += 1;
      }
    }
    const info: SpaceInfo = { id: space.id, name: space.name, createdAt: space.createdAt, pageCount };
    if (space.icon !== undefined) {
      info.icon = space.icon;
    }
    return this.#deps.decorateSpace?.(info) ?? info;
  }
}
