import { MAX_ATTACHMENT_BYTES, sizeLimitError, sliceIntoChunks, validateAttachmentName, validateMime } from "./attachments";
import type { UploadSource } from "./attachments";
import type { DictationApi } from "./dictation";
import type { DriveApi } from "./drive";
import type { BackupFolderListing } from "./generated/BackupFolderListing";
import { internalError, toCoreError } from "./errors";
import type { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { AiChatInput } from "./generated/AiChatInput";
import type { AiGenerateInput } from "./generated/AiGenerateInput";
import type { AiIndexInfo } from "./generated/AiIndexInfo";
import type { AiScope } from "./generated/AiScope";
import type { AiSource } from "./generated/AiSource";
import type { AiStatus } from "./generated/AiStatus";
import type { AiStreamEvent } from "./generated/AiStreamEvent";
import type { AiTranscribed } from "./generated/AiTranscribed";
import type { AppStatus } from "./generated/AppStatus";
import type { AttachmentId } from "./generated/AttachmentId";
import type { AttachmentInfo } from "./generated/AttachmentInfo";
import type { Backlink } from "./generated/Backlink";
import type { ByteRange } from "./generated/ByteRange";
import type { CommentAnchor } from "./generated/CommentAnchor";
import type { CommentBodyItem } from "./generated/CommentBodyItem";
import type { CommentId } from "./generated/CommentId";
import type { CsvColumn } from "./generated/CsvColumn";
import type { DeviceEntry } from "./generated/DeviceEntry";
import type { DeviceId } from "./generated/DeviceId";
import type { DeviceInfo } from "./generated/DeviceInfo";
import type { Event } from "./generated/Event";
import type { GcSummary } from "./generated/GcSummary";
import type { ImportPlanInfo } from "./generated/ImportPlanInfo";
import type { ImportProgress } from "./generated/ImportProgress";
import type { ImportReport } from "./generated/ImportReport";
import type { InviteId } from "./generated/InviteId";
import type { InviteLinkInfo } from "./generated/InviteLinkInfo";
import type { LinkId } from "./generated/LinkId";
import type { InviteSessionInfo } from "./generated/InviteSessionInfo";
import type { McpAuditEntry } from "./generated/McpAuditEntry";
import type { McpClientCreated } from "./generated/McpClientCreated";
import type { McpClientInfo } from "./generated/McpClientInfo";
import type { McpGrant } from "./generated/McpGrant";
import type { McpStatus } from "./generated/McpStatus";
import type { MemberRole } from "./generated/MemberRole";
import type { MembersInfo } from "./generated/MembersInfo";
import type { NotificationInfo } from "./generated/NotificationInfo";
import type { OptionColor } from "./generated/OptionColor";
import type { OptionId } from "./generated/OptionId";
import type { PageContent } from "./generated/PageContent";
import type { PageFirstImage } from "./generated/PageFirstImage";
import type { PageExcerpt } from "./generated/PageExcerpt";
import type { BulkAction } from "./generated/BulkAction";
import type { PageFont } from "./generated/PageFont";
import type { PageId } from "./generated/PageId";
import type { PageInfo } from "./generated/PageInfo";
import type { PageProperties } from "./generated/PageProperties";
import type { PairingInfo } from "./generated/PairingInfo";
import type { PairingRole } from "./generated/PairingRole";
import type { PersonInfo } from "./generated/PersonInfo";
import type { PresenceInfo } from "./generated/PresenceInfo";
import type { PresenceState } from "./generated/PresenceState";
import type { ProfileInfo } from "./generated/ProfileInfo";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyFilter } from "./generated/PropertyFilter";
import type { PropertyId } from "./generated/PropertyId";
import type { PropertyKind } from "./generated/PropertyKind";
import type { PropertyOption } from "./generated/PropertyOption";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RecoveryInfo } from "./generated/RecoveryInfo";
import type { RecoveryKit } from "./generated/RecoveryKit";
import type { ReminderDue } from "./generated/ReminderDue";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SearchHit } from "./generated/SearchHit";
import type { SearchScope } from "./generated/SearchScope";
import type { SecurityInfo } from "./generated/SecurityInfo";
import type { SpaceId } from "./generated/SpaceId";
import type { SpaceInfo } from "./generated/SpaceInfo";
import type { SpaceMoveSummary } from "./generated/SpaceMoveSummary";
import type { SpaceSharePlan } from "./generated/SpaceSharePlan";
import type { StatusGroup } from "./generated/StatusGroup";
import type { SyncInfo } from "./generated/SyncInfo";
import type { ThreadId } from "./generated/ThreadId";
import type { ThreadInfo } from "./generated/ThreadInfo";
import type { TreeView } from "./generated/TreeView";
import type { ViewLinkInfo } from "./generated/ViewLinkInfo";
import type { Workspace } from "./generated/Workspace";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { PageHistory } from "./generated/PageHistory";
import type { PageVersionContent } from "./generated/PageVersionContent";
import type {
  AiTransport,
  AttachmentSaveResult,
  AttachmentTransport,
  BackupResult,
  CoreStateInfo,
  DocTransport,
  ExportRunOptions,
  ExportRunResult,
  ImportRunOptions,
  McpTransport,
  PresenceUpdate,
  ShellEvent,
  ShellLabels,
  Transport,
} from "./transport";
import {
  inviteToken as validateInviteToken,
  kitText as validateKitText,
  pairingCode as validatePairingCode,
  serverUrl as validateServerUrl,
} from "./validate";

type ResponseType = Response["type"];
type ResponseOf<T extends ResponseType> = Extract<Response, { type: T }>;

/** Arguments of {@link CoreClient.pageCreate}. */
export interface PageCreateInput {
  /** Space the page is created in. */
  spaceId: SpaceId;
  /** Parent page; leave out (or `null`) for the space root. */
  parentId?: PageId | null;
  /** Position among the siblings, clamped to their count; leave out to append. */
  index?: number;
  /** Title, trimmed by the core; may be empty (the UI shows "Untitled"). */
  title: string;
}

/** Options of {@link CoreClient.search}; everything left out takes the core's default. */
export interface SearchOptions {
  /** Only pages of this space (default: every space). */
  spaceId?: SpaceId;
  /** `"Titles"` for page pickers, `"All"` (default) for titles and page bodies. */
  scope?: SearchScope;
  /** Maximum number of hits; the core clamps it to 1..=100 (default 20). */
  limit?: number;
  /** Only pages last edited at or after this time, in Unix milliseconds (default: any time). */
  editedAfter?: number;
}

/** What {@link CoreClient.propertyOptionUpdate} changes; what is left out stays as it is. */
export interface PropertyOptionPatch {
  /** New name (1 to 100 characters after trimming). */
  name?: string;
  /** New colour. */
  color?: OptionColor;
  /** New group (`status` options only). */
  group?: StatusGroup;
}

/** Arguments of {@link CoreClient.pairingStart}: the joiner gives `serverUrl`, the sponsor `code`. */
export interface PairingStartOptions {
  /** The server to pair through (joiner: it has no sync configuration yet). */
  serverUrl?: string;
  /** The code the joiner shows, as typed (sponsor; its server is the sync server). */
  code?: string;
}

/**
 * Progress of {@link CoreClient.attachmentUpload}: the bytes the core has received so far and
 * the size of the upload when it is known (a `Blob` source; `undefined` for a stream).
 */
export type UploadProgress = (sentBytes: number, totalBytes?: number) => void;

/**
 * Checks that `response` is the variant the request kind promises and narrows it.
 * A mismatch means the shell and the client disagree about the contract, so it
 * surfaces as an `Internal` error rather than as silently wrong data.
 */
function expectResponse<T extends ResponseType>(
  response: Response,
  expected: T,
  request: Request["type"],
): ResponseOf<T> {
  const received: unknown = (response as { type?: unknown } | null | undefined)?.type;
  if (received !== expected) {
    throw internalError(
      `Unexpected response to ${request}: expected ${expected}, got ${
        typeof received === "string" ? received : "a malformed response"
      }`,
    );
  }
  return response as ResponseOf<T>;
}

/** `Internal`: the transport lacks the method a client call needs. */
function unsupported(method: string): CoreError {
  return internalError(`${method} is not supported by this transport`);
}

/**
 * Typed helpers over the core API (Milestones 1 to 8). Every method sends one request
 * through the {@link Transport}, checks the response variant and returns its payload;
 * failures reject with a `CoreError` (see `isApiError`). Page documents travel as raw
 * bytes through {@link CoreClient.docs}, never inside a request; the app lock, backups and
 * the bytes of attachments are transport methods of their own (see
 * {@link CoreClient.hasAppLock}, {@link CoreClient.hasBackups} and
 * {@link CoreClient.hasAttachments}).
 */
export class CoreClient {
  readonly #transport: Transport;

  constructor(transport: Transport) {
    this.#transport = transport;
  }

  /** Sends `request` and checks that the answer is the `expected` variant. */
  async #call<T extends ResponseType>(request: Request, expected: T): Promise<ResponseOf<T>> {
    return expectResponse(await this.#transport.dispatch(request), expected, request.type);
  }

  /** Health and configuration summary; available before onboarding. */
  async appStatus(): Promise<AppStatus> {
    return (await this.#call({ type: "AppStatus" }, "AppStatus")).data;
  }

  /**
   * Creates the single workspace. Rejects with `AlreadyExists` if there is one. With
   * `firstSpaceName` the core creates a first space with the workspace.
   */
  async workspaceCreate(name: string, firstSpaceName?: string): Promise<Workspace> {
    const { data } = await this.#call(
      {
        type: "WorkspaceCreate",
        data: firstSpaceName === undefined ? { name } : { name, firstSpaceName },
      },
      "Workspace",
    );
    if (data === null) {
      throw internalError("WorkspaceCreate succeeded but returned no workspace");
    }
    return data;
  }

  /** The workspace, or `null` before onboarding. */
  async workspaceGet(): Promise<Workspace | null> {
    return (await this.#call({ type: "WorkspaceGet" }, "Workspace")).data;
  }

  /** Reads a setting; `null` when it is not set. */
  async settingsGet(key: string): Promise<JsonValue | null> {
    return (await this.#call({ type: "SettingsGet", data: { key } }, "Setting")).data;
  }

  /** Writes a setting (any JSON value, at most 64 KiB serialised). */
  async settingsSet(key: string, value: JsonValue): Promise<void> {
    await this.#call({ type: "SettingsSet", data: { key, value } }, "Ok");
  }

  /** This device's identity and name. */
  async deviceInfo(): Promise<DeviceInfo> {
    return (await this.#call({ type: "DeviceInfo" }, "DeviceInfo")).data;
  }

  /** Renames this device and returns the updated record. */
  async deviceRename(name: string): Promise<DeviceInfo> {
    return (await this.#call({ type: "DeviceRename", data: { name } }, "DeviceInfo")).data;
  }

  // ----- spaces ---------------------------------------------------------------------

  /** All spaces in display order. */
  async spaceList(): Promise<SpaceInfo[]> {
    return (await this.#call({ type: "SpaceList" }, "Spaces")).data;
  }

  /** Creates a space, appended last; the name is 1 to 100 characters, the icon 1 to 32 bytes. */
  async spaceCreate(name: string, icon?: string): Promise<SpaceInfo> {
    const data = icon === undefined ? { name } : { name, icon };
    return (await this.#call({ type: "SpaceCreate", data }, "Space")).data;
  }

  /** Renames a space and returns it. */
  async spaceRename(spaceId: SpaceId, name: string): Promise<SpaceInfo> {
    return (await this.#call({ type: "SpaceRename", data: { spaceId, name } }, "Space")).data;
  }

  /** Sets a space icon, or clears it with `null`, and returns the space. */
  async spaceSetIcon(spaceId: SpaceId, icon: string | null): Promise<SpaceInfo> {
    return (await this.#call({ type: "SpaceSetIcon", data: { spaceId, icon } }, "Space")).data;
  }

  /** Moves a space to a 0-based position in the display order (clamped) and returns the new order. */
  async spaceMove(spaceId: SpaceId, index: number): Promise<SpaceInfo[]> {
    return (await this.#call({ type: "SpaceMove", data: { spaceId, index } }, "Spaces")).data;
  }

  /** Deletes a space; refused with `InvalidInput` while it has live or trashed pages. */
  async spaceDelete(spaceId: SpaceId): Promise<void> {
    await this.#call({ type: "SpaceDelete", data: { spaceId } }, "Ok");
  }

  // ----- page tree ------------------------------------------------------------------

  /** The live page tree, the trash and the undo state of a space. */
  async treeGet(spaceId: SpaceId): Promise<TreeView> {
    return (await this.#call({ type: "TreeGet", data: { spaceId } }, "Tree")).data;
  }

  /** One page's metadata, live or trashed; `NotFound` once it is purged. */
  async pageGet(pageId: PageId): Promise<PageInfo> {
    return (await this.#call({ type: "PageGet", data: { pageId } }, "Page")).data;
  }

  /** Creates a page (title 0 to 500 characters) and returns it. */
  async pageCreate({ spaceId, parentId, index, title }: PageCreateInput): Promise<PageInfo> {
    const data: Extract<Request, { type: "PageCreate" }>["data"] = { spaceId, title };
    if (parentId !== undefined && parentId !== null) {
      data.parentId = parentId;
    }
    if (index !== undefined) {
      data.index = index;
    }
    return (await this.#call({ type: "PageCreate", data }, "Page")).data;
  }

  /** Renames a page and returns it. */
  async pageRename(pageId: PageId, title: string): Promise<PageInfo> {
    return (await this.#call({ type: "PageRename", data: { pageId, title } }, "Page")).data;
  }

  /** Sets a page icon, or clears it with `null`, and returns the page. */
  async pageSetIcon(pageId: PageId, icon: string | null): Promise<PageInfo> {
    return (await this.#call({ type: "PageSetIcon", data: { pageId, icon } }, "Page")).data;
  }

  /**
   * Sets, repositions or clears a page cover (ADR-0040) and returns the page. `cover` is
   * `attachment:<id>` (an image of the page's space), `color:<palette name>` or `gradient:<name>`;
   * `coverY` is the vertical position of the image in percent (0 to 100, 50 by default). `cover`
   * `null` with a `coverY` moves the image of the cover the page has (`InvalidInput` without one);
   * both `null` clear the cover.
   */
  async pageSetCover(pageId: PageId, cover: string | null, coverY?: number | null): Promise<PageInfo> {
    const data: Extract<Request, { type: "PageSetCover" }>["data"] = { pageId };
    if (cover !== null) {
      data.cover = cover;
    }
    if (coverY !== undefined && coverY !== null) {
      data.coverY = coverY;
    }
    return (await this.#call({ type: "PageSetCover", data }, "Page")).data;
  }

  /**
   * Sets or clears the font, the small text and the lock of a page (ADR-0040) and returns the page. A field
   * left out stays as it is; `font` `"default"`, `smallText` `false` and `locked` `false` clear. The lock is a
   * guard against accidental edits in the UI, not a permission: the core refuses no write to a locked page.
   */
  async pageSetAppearance(
    pageId: PageId,
    appearance: { font?: PageFont; smallText?: boolean; locked?: boolean },
  ): Promise<PageInfo> {
    const data: Extract<Request, { type: "PageSetAppearance" }>["data"] = { pageId };
    if (appearance.font !== undefined) {
      data.font = appearance.font;
    }
    if (appearance.smallText !== undefined) {
      data.smallText = appearance.smallText;
    }
    if (appearance.locked !== undefined) {
      data.locked = appearance.locked;
    }
    return (await this.#call({ type: "PageSetAppearance", data }, "Page")).data;
  }

  /**
   * Moves a page with its subtree under `parentId` (`null` for the space root) at `index`
   * among the new siblings, counted without the page itself and clamped. Moving a page
   * under its own descendant is refused with `InvalidInput`.
   */
  async pageMove(pageId: PageId, parentId: PageId | null, index: number): Promise<void> {
    const data: Extract<Request, { type: "PageMove" }>["data"] = { pageId, index };
    if (parentId !== null) {
      data.parentId = parentId;
    }
    await this.#call({ type: "PageMove", data }, "Ok");
  }

  /**
   * Puts the live children of `parentId` (`null` for the top-level pages of the space) into `order`, in one
   * undo step (ADR-0052). `order` must be exactly the live children, each once: any other set is refused with
   * `InvalidInput` on `order` (a page was added, moved or trashed since the tree was read; read again and retry).
   * Writes nothing when the children already stand in that order. No page is stamped as edited.
   */
  async pageReorderChildren(spaceId: SpaceId, parentId: PageId | null, order: readonly PageId[]): Promise<void> {
    const data: Extract<Request, { type: "PageReorderChildren" }>["data"] = { spaceId, order: [...order] };
    if (parentId !== null) {
      data.parentId = parentId;
    }
    await this.#call({ type: "PageReorderChildren", data }, "Ok");
  }

  /**
   * What moving a live page and its live subtree to another space would do (ADR-0038): the same
   * checks as `pageMoveToSpace`, nothing written. `parentId` `null` = the target's root.
   */
  async pageMoveToSpacePlan(
    pageId: PageId,
    spaceId: SpaceId,
    parentId: PageId | null,
  ): Promise<SpaceMoveSummary> {
    const data: Extract<Request, { type: "PageMoveToSpacePlan" }>["data"] = { pageId, spaceId };
    if (parentId !== null) {
      data.parentId = parentId;
    }
    return (await this.#call({ type: "PageMoveToSpacePlan", data }, "SpaceMovePlanned")).data;
  }

  /**
   * Moves a live page and its live subtree to another space, keeping their ids (ADR-0038):
   * under `parentId` (`null` = the target's root) at `index` (`null` = last).
   */
  async pageMoveToSpace(
    pageId: PageId,
    spaceId: SpaceId,
    parentId: PageId | null,
    index: number | null,
  ): Promise<SpaceMoveSummary> {
    const data: Extract<Request, { type: "PageMoveToSpace" }>["data"] = { pageId, spaceId };
    if (parentId !== null) {
      data.parentId = parentId;
    }
    if (index !== null) {
      data.index = index;
    }
    return (await this.#call({ type: "PageMoveToSpace", data }, "PagesMovedToSpace")).data;
  }

  /**
   * Duplicates a live page with its live sub-pages (ADR-0040) and returns the copy of the page: it goes right
   * after the page among its siblings and is titled with " (copy)" appended; the sub-pages keep their titles.
   * New page and block ids, the same icons, values and content, links into the copied pages pointing at the
   * copies, attachments shared by id. One structural undo step.
   */
  async pageDuplicate(pageId: PageId): Promise<PageInfo> {
    return (await this.#call({ type: "PageDuplicate", data: { pageId } }, "Page")).data;
  }

  /**
   * Makes a page a template or a page again (ADR-0056). A template is hidden from the tree's lists, views, search,
   * backlinks, reminders and the AI index with its sub-pages; pages are made from it with `pageCreateFromTemplate`.
   */
  async pageSetTemplate(pageId: PageId, template: boolean): Promise<PageInfo> {
    return (await this.#call({ type: "PageSetTemplate", data: { pageId, template } }, "Page")).data;
  }

  /**
   * Sets (or clears, with `undefined`) the placeholder of a property on a template page (ADR-0056): `today` or `now` for a
   * date property, `me` for a person property. It replaces the page's stored value of that property.
   */
  async pageSetTemplatePlaceholder(
    pageId: PageId,
    propertyId: PropertyId,
    placeholder: TemplatePlaceholder | undefined,
  ): Promise<PageInfo> {
    const data = placeholder === undefined ? { pageId, propertyId } : { pageId, propertyId, placeholder };
    return (await this.#call({ type: "PageSetTemplatePlaceholder", data }, "Page")).data;
  }

  /**
   * Makes a page (with its sub-pages) from a template (ADR-0056) under `parentId` (the space's top level when absent) at
   * `index` (last when absent); placeholders are resolved now, then `values` are written on the new page.
   */
  async pageCreateFromTemplate(
    templateId: PageId,
    options: {
      parentId?: PageId;
      index?: number;
      values?: readonly (readonly [PropertyId, PropertyValue])[];
    } = {},
  ): Promise<PageInfo> {
    const data: {
      templateId: PageId;
      parentId?: PageId;
      index?: number;
      values?: [PropertyId, PropertyValue][];
    } = { templateId };
    if (options.parentId !== undefined) data.parentId = options.parentId;
    if (options.index !== undefined) data.index = options.index;
    if (options.values !== undefined && options.values.length > 0) {
      data.values = options.values.map(([id, value]) => [id, value]);
    }
    return (await this.#call({ type: "PageCreateFromTemplate", data }, "Page")).data;
  }

  /** Moves a page and its subtree to the trash. */
  async pageTrash(pageId: PageId): Promise<void> {
    await this.#call({ type: "PageTrash", data: { pageId } }, "Ok");
  }

  /** Restores a trashed page to its previous parent (the root if that page is gone) and returns it. */
  async pageRestore(pageId: PageId): Promise<PageInfo> {
    return (await this.#call({ type: "PageRestore", data: { pageId } }, "Page")).data;
  }

  /** Permanently deletes a page and its subtree. Irreversible. */
  async pagePurge(pageId: PageId): Promise<void> {
    await this.#call({ type: "PagePurge", data: { pageId } }, "Ok");
  }

  /**
   * Adds a page to its space's favourites (last) or removes it (ADR-0029). Favourites sync with
   * the space and are listed, in order, in `TreeView.favorites`. Idempotent: adding a favourite
   * that exists keeps its place. Adding a page in the trash is refused with `InvalidInput`
   * (`details.field` is `pageId`); removing works for any page. Not an undo step.
   */
  async pageFavoriteSet(pageId: PageId, favorite: boolean): Promise<void> {
    await this.#call({ type: "PageFavoriteSet", data: { pageId, favorite } }, "Ok");
  }

  /**
   * Moves a favourite in front of another favourite of the same space (`before`), or last when
   * `before` is left out. `InvalidInput` when `pageId` is not a favourite (`details.field` is
   * `pageId`) or `before` is not one (`before`). Not an undo step.
   */
  async pageFavoriteMove(pageId: PageId, before?: PageId | null): Promise<void> {
    const data: Extract<Request, { type: "PageFavoriteMove" }>["data"] = { pageId };
    if (before !== undefined && before !== null) {
      data.before = before;
    }
    await this.#call({ type: "PageFavoriteMove", data }, "Ok");
  }

  /** Undoes the last structural change in a space and returns the tree as it is afterwards. */
  async structureUndo(spaceId: SpaceId): Promise<TreeView> {
    return (await this.#call({ type: "StructureUndo", data: { spaceId } }, "Tree")).data;
  }

  /** Redoes the last undone structural change in a space and returns the tree as it is afterwards. */
  async structureRedo(spaceId: SpaceId): Promise<TreeView> {
    return (await this.#call({ type: "StructureRedo", data: { spaceId } }, "Tree")).data;
  }

  // ----- page content, links and search ---------------------------------------------

  /**
   * Searches page titles and (unless `scope` is `"Titles"`) page bodies; trashed pages
   * never match. The query is trimmed and must be 1 to 200 characters (`InvalidInput`
   * otherwise). Title hits have no `blockId`; `highlights` are UTF-16 ranges in `snippet`.
   *
   * A page matches when every word is somewhere in it, not necessarily in one block (ADR-0040): a
   * page whose words are spread over several rows is one hit, on the row that holds the most of
   * them, after the rows that hold them all. When that leaves room under `limit`, pages whose title
   * has the query's letters in order, or a word with one typo, follow as title hits (no `blockId`),
   * in both scopes. `editedAfter` keeps the pages edited at or after that time.
   */
  async search(query: string, options: SearchOptions = {}): Promise<SearchHit[]> {
    const data: Extract<Request, { type: "Search" }>["data"] = { query };
    if (options.spaceId !== undefined) {
      data.spaceId = options.spaceId;
    }
    if (options.scope !== undefined) {
      data.scope = options.scope;
    }
    if (options.limit !== undefined) {
      data.limit = options.limit;
    }
    if (options.editedAfter !== undefined) {
      data.editedAfter = options.editedAfter;
    }
    return (await this.#call({ type: "Search", data }, "Search")).data;
  }

  /** Live pages that reference `pageId`, one entry per referencing block. */
  async backlinksGet(pageId: PageId): Promise<Backlink[]> {
    return (await this.#call({ type: "BacklinksGet", data: { pageId } }, "Backlinks")).data;
  }

  /** A page's content as portable ProseMirror JSON (`docs/development/milestone-03.md`). */
  /**
   * The versions of a page, newest first (ADR-0057): dated versions from this device's journal, then the older history as
   * undated versions. `before` resumes after a version id of an earlier answer; `limit` defaults to 100 (at most 500).
   */
  async pageHistoryList(pageId: PageId, options: { before?: string; limit?: number } = {}): Promise<PageHistory> {
    const data: { pageId: PageId; before?: string; limit?: number } = { pageId };
    if (options.before !== undefined) data.before = options.before;
    if (options.limit !== undefined) data.limit = options.limit;
    return (await this.#call({ type: "PageHistoryList", data }, "PageHistory")).data;
  }

  /** A page's content (portable JSON) and title at a version (ADR-0057). */
  async pageHistoryGet(pageId: PageId, version: string): Promise<PageVersionContent> {
    return (await this.#call({ type: "PageHistoryGet", data: { pageId, version } }, "PageVersionContent")).data;
  }

  /** Makes a page's content (and title, when known) what it was at a version, as a new change (ADR-0057). */
  async pageHistoryRestore(pageId: PageId, version: string): Promise<PageInfo> {
    return (await this.#call({ type: "PageHistoryRestore", data: { pageId, version } }, "Page")).data;
  }

  async pageContentGet(pageId: PageId): Promise<PageContent> {
    return (await this.#call({ type: "PageContentGet", data: { pageId } }, "PageContent")).data;
  }

  /**
   * The first image of each of `pageIds` that has one (ADR-0054): the attachment of the first `image` block, or the
   * first item of the first `gallery` block, in document order. Answered from a device-local index, in the order
   * asked; a page without an image, a page of another space and an unknown page are left out. At most 500 ids
   * (`InvalidInput` for `pageIds` beyond that), `NotFound` for an unknown space.
   */
  async pageFirstImages(spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageFirstImage[]> {
    return (await this.#call({ type: "PageFirstImages", data: { spaceId, pageIds: [...pageIds] } }, "PageFirstImages")).data;
  }

  /**
   * The first lines of text of each of `pageIds` (ADR-0058), for a feed card: at most 280 characters, blocks joined
   * with a line break, ending with `…` when cut. Read from the text index; pages of another space, templates and pages
   * without body text are left out. At most 500 ids (`InvalidInput` for `pageIds`).
   */
  async pageExcerpts(spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageExcerpt[]> {
    return (await this.#call({ type: "PageExcerpts", data: { spaceId, pageIds: [...pageIds] } }, "PageExcerpts")).data;
  }

  /**
   * Applies one action to a set of pages of one space, all or nothing (ADR-0058, `docs/architecture/database-power-ups.md`
   * section 5.4): set or clear a value (`SetValue`, `SetValues`), move, duplicate or trash. Structural actions drop the pages
   * whose ancestor is in the list and are one structural undo step. Answers the top copies of a duplicate, else the pages
   * acted on. At most 1,000 pages.
   */
  async pageBulk(spaceId: SpaceId, pageIds: readonly PageId[], action: BulkAction): Promise<PageId[]> {
    return (await this.#call({ type: "PageBulk", data: { spaceId, pageIds: [...pageIds], action } }, "PageBulk")).data;
  }

  /** A page's content as Markdown, page references rendered as `[[Title]]`. */
  async pageExportMarkdown(pageId: PageId): Promise<string> {
    return (await this.#call({ type: "PageExportMarkdown", data: { pageId } }, "Markdown")).data;
  }

  /** Rebuilds the derived text and link index of one space, or of every space when left out. */
  async indexRebuild(spaceId?: SpaceId): Promise<void> {
    await this.#call({ type: "IndexRebuild", data: spaceId === undefined ? {} : { spaceId } }, "Ok");
  }

  // ----- reminders on date mentions (ADR-0024) ---------------------------------------

  /**
   * The reminders of date mentions that fell due since this device last looked, before `until`
   * (the device's wall clock, in seconds: `Math.floor(Date.now() / 1000) - new Date().getTimezoneOffset()
   * * 60`, see {@link localNowSeconds}); oldest first, at most 100, at most seven days back. A
   * pure query: it fires nothing and moves nothing.
   */
  async remindersDue(until: number): Promise<ReminderDue[]> {
    return (await this.#call({ type: "RemindersDue", data: { until } }, "Reminders")).data;
  }

  /** Whether the transport wants the UI to drive the reminder scheduler (see {@link CoreClient.remindersTick}). */
  hasRemindersTick(): boolean {
    return this.#transport.remindersTick !== undefined;
  }

  /**
   * Runs one tick of the reminder scheduler: the core emits an `Event.ReminderDue` for every
   * reminder that fell due since the last tick and moves its mark; resolves with how many fired.
   * Only a transport whose core has no shell timer around it offers this (the in-memory one the
   * browser page uses): the desktop shell ticks its core itself, every 30 seconds, and shows the
   * OS notification. `Internal` "remindersTick is not supported by this transport" otherwise.
   */
  async remindersTick(): Promise<number> {
    if (this.#transport.remindersTick === undefined) {
      throw unsupported("remindersTick");
    }
    return this.#transport.remindersTick();
  }

  // ----- typed page properties (ADR-0026) ---------------------------------------------

  /**
   * The property schema of a space: Tags first (a built-in multi-select with the id
   * `TAGS_PROPERTY_ID`, listed even before anything wrote it), then the other properties in order.
   * `NotFound` for an unknown space.
   */
  async propertySchemaGet(spaceId: SpaceId): Promise<PropertyDef[]> {
    return (await this.#call({ type: "PropertySchemaGet", data: { spaceId } }, "PropertySchema")).data.properties;
  }

  /**
   * Creates a property (name 1 to 100 characters after trimming), appended last; its kind is fixed.
   * A `status` property starts with "Not started", "In progress" and "Done". A space holds at most
   * 100 properties, Tags included (`InvalidInput` for `spaceId`). A `relation` takes a `config` (a
   * two-way one makes the paired property too, in the same change, and the answer is the forward
   * property), a `rollup` needs one; any other kind takes none (`InvalidInput` for `config`).
   */
  async propertyCreate(
    spaceId: SpaceId,
    name: string,
    kind: PropertyKind,
    config?: PropertyConfig,
  ): Promise<PropertyDef> {
    const data: Extract<Request, { type: "PropertyCreate" }>["data"] = { spaceId, name, kind };
    if (config !== undefined) {
      data.config = config;
    }
    return (await this.#call({ type: "PropertyCreate", data }, "Property")).data;
  }

  /** Changes the configuration of a rollup (relation, target, function); a relation's is fixed at creation. */
  async propertyConfigSet(spaceId: SpaceId, propertyId: PropertyId, config: PropertyConfig): Promise<PropertyDef> {
    return (await this.#call({ type: "PropertyConfigSet", data: { spaceId, propertyId, config } }, "Property")).data;
  }

  /** Renames a property (Tags too) and returns it. */
  async propertyRename(spaceId: SpaceId, propertyId: PropertyId, name: string): Promise<PropertyDef> {
    return (await this.#call({ type: "PropertyRename", data: { spaceId, propertyId, name } }, "Property")).data;
  }

  /**
   * Removes a property from the schema (`InvalidInput` for Tags). Its values are hidden at once and
   * dropped by the next write of each page's values.
   */
  async propertyDelete(spaceId: SpaceId, propertyId: PropertyId): Promise<void> {
    await this.#call({ type: "PropertyDelete", data: { spaceId, propertyId } }, "Ok");
  }

  /**
   * Moves a property in front of `before` (the end when left out; `before` may be Tags, which
   * makes it the first of the others). Tags itself cannot be moved: it always comes first.
   */
  async propertyMove(spaceId: SpaceId, propertyId: PropertyId, before?: PropertyId): Promise<void> {
    const data: Extract<Request, { type: "PropertyMove" }>["data"] = { spaceId, propertyId };
    if (before !== undefined) {
      data.before = before;
    }
    await this.#call({ type: "PropertyMove", data }, "Ok");
  }

  /**
   * Adds an option to a select, multi-select or status property, appended last (Tags is written
   * on the first). A `status` option has a group (default `"todo"`), the other kinds take none.
   */
  async propertyOptionAdd(
    spaceId: SpaceId,
    propertyId: PropertyId,
    name: string,
    color: OptionColor,
    group?: StatusGroup,
  ): Promise<PropertyOption> {
    const data: Extract<Request, { type: "PropertyOptionAdd" }>["data"] = { spaceId, propertyId, name, color };
    if (group !== undefined) {
      data.group = group;
    }
    return (await this.#call({ type: "PropertyOptionAdd", data }, "PropertyOption")).data;
  }

  /** Changes an option's name, colour or group (what `patch` leaves out stays) and returns it. */
  async propertyOptionUpdate(
    spaceId: SpaceId,
    propertyId: PropertyId,
    optionId: OptionId,
    patch: PropertyOptionPatch,
  ): Promise<PropertyOption> {
    const data: Extract<Request, { type: "PropertyOptionUpdate" }>["data"] = { spaceId, propertyId, optionId };
    if (patch.name !== undefined) {
      data.name = patch.name;
    }
    if (patch.color !== undefined) {
      data.color = patch.color;
    }
    if (patch.group !== undefined) {
      data.group = patch.group;
    }
    return (await this.#call({ type: "PropertyOptionUpdate", data }, "PropertyOption")).data;
  }

  /** Removes an option and takes it out of every page's value of that property in the space. */
  async propertyOptionDelete(spaceId: SpaceId, propertyId: PropertyId, optionId: OptionId): Promise<void> {
    await this.#call({ type: "PropertyOptionDelete", data: { spaceId, propertyId, optionId } }, "Ok");
  }

  /**
   * Sets one property value of a page, live or trashed, or clears it with `null` (an empty text,
   * URL or multi-select list clears too). Resolves with the page's values after the change, in
   * schema order. The value's kind must be the property's and its options must belong to it
   * (`InvalidInput` for `value` otherwise).
   */
  async pagePropertySet(
    pageId: PageId,
    propertyId: PropertyId,
    value: PropertyValue | null,
  ): Promise<PageProperties["values"]> {
    return (await this.#call({ type: "PagePropertySet", data: { pageId, propertyId, value } }, "PageProperties"))
      .data.values;
  }

  /** The values of a page in schema order (Tags first): only the ones that are set and readable. */
  async pagePropertiesGet(pageId: PageId): Promise<PageProperties["values"]> {
    return (await this.#call({ type: "PagePropertiesGet", data: { pageId } }, "PageProperties")).data.values;
  }

  /**
   * The live pages of a space whose value of a property matches `filter`, ordered by title (ASCII
   * case-insensitive, then exact, then page id). Trashed pages are left out. A filter that does not
   * fit the property's kind is `InvalidInput` for `filter`.
   */
  async pagesByProperty(spaceId: SpaceId, propertyId: PropertyId, filter: PropertyFilter): Promise<PageId[]> {
    return (await this.#call({ type: "PagesByProperty", data: { spaceId, propertyId, filter } }, "PageIds")).data;
  }

  // ----- keys and identity (Milestone 4) ---------------------------------------------

  /** Recovery kit, app lock, devices and keyring counts. */
  async securityStatus(): Promise<SecurityInfo> {
    return (await this.#call({ type: "SecurityStatus" }, "Security")).data;
  }

  /**
   * Issues the recovery kit (24 words and the code form of the same key), to be shown once:
   * the core keeps no copy. Needs the workspace; `AlreadyExists` when a kit exists.
   */
  async recoveryKitCreate(): Promise<RecoveryKit> {
    return (await this.#call({ type: "RecoveryKitCreate" }, "RecoveryKit")).data;
  }

  /**
   * Checks a typed kit (the words or the code; any case; 4-letter word prefixes) against the
   * current one and records the verification; `InvalidInput` for the field `kit` otherwise.
   */
  async recoveryKitVerify(kit: string): Promise<void> {
    await this.#call({ type: "RecoveryKitVerify", data: { kit } }, "Ok");
  }

  /**
   * Replaces the recovery kit with a new one (rotating the root key); the old kit stops
   * working and only opens older backups. While the app lock is on, its passphrase is
   * required (`InvalidInput` for the field `passphrase` when missing or wrong).
   */
  async recoveryKitReissue(passphrase?: string): Promise<RecoveryKit> {
    const data = passphrase === undefined ? {} : { passphrase };
    return (await this.#call({ type: "RecoveryKitReissue", data }, "RecoveryKit")).data;
  }

  /** Enables the app lock (8 to 256 UTF-8 bytes); `AlreadyExists` when it is on. */
  async appLockEnable(passphrase: string): Promise<void> {
    await this.#call({ type: "AppLockEnable", data: { passphrase } }, "Ok");
  }

  /** Disables the app lock; a wrong passphrase is `InvalidInput` for the field `passphrase`. */
  async appLockDisable(passphrase: string): Promise<void> {
    await this.#call({ type: "AppLockDisable", data: { passphrase } }, "Ok");
  }

  /** Changes the app-lock passphrase from `current` to `next` (sent as `new`). */
  async appLockChange(current: string, next: string): Promise<void> {
    await this.#call({ type: "AppLockChange", data: { current, new: next } }, "Ok");
  }

  /** The devices of the account as the device log describes them, this device marked. */
  async deviceList(): Promise<DeviceEntry[]> {
    return (await this.#call({ type: "DeviceList" }, "Devices")).data;
  }

  // ----- attachments (Milestone 5) ----------------------------------------------------

  /**
   * Starts an upload into a space and returns the new attachment's id; then send the bytes
   * with {@link CoreClient.attachmentWrite} and end with {@link CoreClient.attachmentFinish}
   * or {@link CoreClient.attachmentAbort} ({@link CoreClient.attachmentUpload} does all of
   * it). `InvalidInput` for the fields `name` and `mime` (see `validateAttachmentName`,
   * `validateMime`) and for `workspace` before onboarding; `NotFound` for an unknown space.
   */
  async attachmentBegin(spaceId: SpaceId, name: string, mime: string): Promise<AttachmentId> {
    return (await this.#call({ type: "AttachmentBegin", data: { spaceId, name, mime } }, "AttachmentStarted")).data;
  }

  /** Seals the last chunk and records the attachment; emits `AttachmentsChanged`. `NotFound` for an unknown upload. */
  async attachmentFinish(attachmentId: AttachmentId): Promise<AttachmentInfo> {
    return (await this.#call({ type: "AttachmentFinish", data: { attachmentId } }, "Attachment")).data;
  }

  /** Drops an upload in progress and what it wrote; emits `AttachmentsChanged`. `NotFound` for an unknown upload. */
  async attachmentAbort(attachmentId: AttachmentId): Promise<void> {
    await this.#call({ type: "AttachmentAbort", data: { attachmentId } }, "Ok");
  }

  /** The attachments of a space, newest first; `NotFound` for an unknown space. */
  async attachmentList(spaceId: SpaceId): Promise<AttachmentInfo[]> {
    return (await this.#call({ type: "AttachmentList", data: { spaceId } }, "Attachments")).data;
  }

  /**
   * One attachment; `NotFound` when unknown or purged (blocks then show a "deleted" card). Its
   * `availability` (Milestone 8) says whether the content is here (`Complete`, `localChunks ===
   * chunkCount`), on its way (`Downloading`), waiting for the device that made it to finish
   * uploading (`Waiting`), or not asked for under the on-demand policy (`NotDownloaded`).
   */
  async attachmentGet(attachmentId: AttachmentId): Promise<AttachmentInfo> {
    return (await this.#call({ type: "AttachmentGet", data: { attachmentId } }, "Attachment")).data;
  }

  /**
   * Asks for the chunks this device lacks of an attachment, now, whatever the download policy
   * (`DOWNLOAD_POLICY_SETTING`), and pokes the sync driver (`SyncRequested`); resolves with the
   * attachment (an incomplete one is then `Downloading`, or still `Waiting`). The chunks arrive
   * with the sync cycles, each emitting `AttachmentsChanged`. A complete attachment resolves
   * unchanged. `NotFound` when unknown or purged.
   */
  async attachmentDownload(attachmentId: AttachmentId): Promise<AttachmentInfo> {
    return (await this.#call({ type: "AttachmentDownload", data: { attachmentId } }, "Attachment")).data;
  }

  /** Purges an attachment now, whatever references it (the blocks stay and render "deleted"). */
  async attachmentDelete(attachmentId: AttachmentId): Promise<void> {
    await this.#call({ type: "AttachmentDelete", data: { attachmentId } }, "Ok");
  }

  /**
   * Purges the attachments unreferenced for longer than the retention (`RETENTION_SETTING`,
   * default `DEFAULT_RETENTION_DAYS`, `0` = never), then deletes chunks nothing claims.
   */
  async attachmentGc(): Promise<GcSummary> {
    return (await this.#call({ type: "AttachmentGc" }, "AttachmentGc")).data;
  }

  // ----- sync (Milestone 6) -----------------------------------------------------------

  /**
   * Points this device at a sync server, or disables sync with `null` (the core then forgets
   * the session and the cursor state, never the documents), and returns the status. Checks
   * before anything is sent: the address (`http://` or `https://`, a host, an optional port and
   * path, no `@`, `?`, `#` or spaces, at most 2048 characters: `InvalidInput` for `serverUrl`,
   * see `validateServerUrl`) and, when given, the invite token (1 to 128 printable ASCII
   * characters: `InvalidInput` for `inviteToken`, see `validateInviteToken`); both are sent
   * trimmed. Connecting needs the recovery kit (the core answers `InvalidInput` for `serverUrl`
   * without one). The token is needed only when the account does not exist on that server yet,
   * which the driver finds out during its cycle: the status then shows `state: "Error"` and
   * `needsInviteToken: true` until a `syncConfigure` with a token. The token is never stored.
   * The core emits `SyncChanged`, and `SyncRequested` when connecting.
   */
  async syncConfigure(serverUrl: string | null, inviteToken?: string): Promise<SyncInfo> {
    const data: Extract<Request, { type: "SyncConfigure" }>["data"] = {};
    if (serverUrl !== null) {
      data.serverUrl = validateServerUrl(serverUrl);
    }
    if (inviteToken !== undefined) {
      data.inviteToken = validateInviteToken(inviteToken);
    }
    return (await this.#call({ type: "SyncConfigure", data }, "Sync")).data;
  }

  /** Where sync stands: configured server, driver state, pending entries, cursor, last push and pull, fork. */
  async syncStatus(): Promise<SyncInfo> {
    return (await this.#call({ type: "SyncStatus" }, "Sync")).data;
  }

  /** Asks the shell's sync driver for a cycle now (the core emits `SyncRequested`); follow it through `SyncChanged`. */
  async syncNow(): Promise<void> {
    await this.#call({ type: "SyncNow" }, "Ok");
  }

  // ----- devices: pairing, revocation, recovery (Milestone 7) ------------------------------

  /**
   * Starts pairing this installation with another device through the server and returns the
   * session; the shell's pairing task then moves it on (follow it through `PairingChanged` and
   * {@link CoreClient.pairingStatus}). The joiner (a fresh installation; `AlreadyExists`
   * otherwise) gives the server address and shows the code; the sponsor (an existing device,
   * with sync on) gives the code the joiner shows. Checks before anything is sent: the address
   * as for sync (`InvalidInput` for `serverUrl`, see `validateServerUrl`) and the code (12
   * Crockford base32 characters once spaces and dashes are removed, any case: `InvalidInput`
   * for `code`, "the code is 12 letters and digits", see `validatePairingCode`); the address is
   * sent trimmed, the code as its 12 canonical characters.
   */
  async pairingStart(role: PairingRole, options: PairingStartOptions = {}): Promise<PairingInfo> {
    const data: Extract<Request, { type: "PairingStart" }>["data"] = { role };
    if (options.serverUrl !== undefined) {
      data.serverUrl = validateServerUrl(options.serverUrl);
    }
    if (options.code !== undefined) {
      data.code = validatePairingCode(options.code);
    }
    return (await this.#call({ type: "PairingStart", data }, "Pairing")).data;
  }

  /** The user compared the six digits and they match: confirms this side of the pairing. */
  async pairingConfirm(): Promise<PairingInfo> {
    return (await this.#call({ type: "PairingConfirm" }, "Pairing")).data;
  }

  /** Cancels the pairing session in progress and returns it. */
  async pairingCancel(): Promise<PairingInfo> {
    return (await this.#call({ type: "PairingCancel" }, "Pairing")).data;
  }

  /** The pairing session: role, state, code, six digits, the other device's name, error, expiry. */
  async pairingStatus(): Promise<PairingInfo> {
    return (await this.#call({ type: "PairingStatus" }, "Pairing")).data;
  }

  /**
   * Removes another device from the account: the server refuses it from then on, the root key
   * and the space keys rotate, and a new recovery kit replaces the old one (returned, to be
   * shown once). While the app lock is on its passphrase is required (`InvalidInput` for
   * `passphrase`); this device or a device that is not active is `InvalidInput` for `deviceId`.
   * The core emits `SecurityChanged` and `DevicesChanged`.
   */
  async deviceRevoke(deviceId: DeviceId, passphrase?: string): Promise<RecoveryKit> {
    const data = passphrase === undefined ? { deviceId } : { deviceId, passphrase };
    return (await this.#call({ type: "DeviceRevoke", data }, "RecoveryKit")).data;
  }

  /**
   * Finishes a root key rotation another device made while the app lock kept this one from
   * storing the new key (`SecurityInfo.rotationPending`); the passphrase is required while the
   * lock is on. Returns the security status.
   */
  async rotationApply(passphrase?: string): Promise<SecurityInfo> {
    const data = passphrase === undefined ? {} : { passphrase };
    return (await this.#call({ type: "RotationApply", data }, "Security")).data;
  }

  /**
   * Recovers the account on this fresh installation (`AlreadyExists` otherwise) from the server
   * with the recovery kit (the words or the code) and returns the session; the shell's recovery
   * task moves it on (follow it through `RecoveryChanged`). Checks before anything is sent: the
   * address as for sync (`InvalidInput` for `serverUrl`) and the kit text (not empty, at most
   * 1024 bytes: `InvalidInput` for `kit`, see `validateKitText`); both are sent trimmed.
   */
  async recoveryStart(serverUrl: string, kit: string): Promise<RecoveryInfo> {
    const data = { serverUrl: validateServerUrl(serverUrl), kit: validateKitText(kit) };
    return (await this.#call({ type: "RecoveryStart", data }, "Recovery")).data;
  }

  /** The recovery session: state, server, error. */
  async recoveryStatus(): Promise<RecoveryInfo> {
    return (await this.#call({ type: "RecoveryStatus" }, "Recovery")).data;
  }

  /** Cancels the recovery and returns the (now idle) session. */
  async recoveryCancel(): Promise<RecoveryInfo> {
    return (await this.#call({ type: "RecoveryCancel" }, "Recovery")).data;
  }

  /** Whether the transport can move attachment bytes and files (`write`, `read`, `importPath`, `saveAs`, `open`). */
  hasAttachments(): boolean {
    const attachments: Partial<AttachmentTransport> | undefined = this.#transport.attachments;
    return (
      attachments !== undefined &&
      typeof attachments.write === "function" &&
      typeof attachments.read === "function" &&
      typeof attachments.importPath === "function" &&
      typeof attachments.saveAs === "function" &&
      typeof attachments.open === "function"
    );
  }

  /**
   * Appends plaintext (at most `CHUNK_SIZE` bytes) to an upload started with
   * {@link CoreClient.attachmentBegin}. Prefer {@link CoreClient.attachmentUpload}.
   */
  async attachmentWrite(attachmentId: AttachmentId, bytes: Uint8Array): Promise<void> {
    await this.#attachments("attachmentWrite").write(attachmentId, bytes);
  }

  /**
   * Uploads `source` (a `Blob` such as a pasted or dropped `File`, or a stream of byte pieces)
   * as a new attachment of the space and returns it. First checks the name and MIME type like
   * the core (`InvalidInput` for `name` or `mime`) and a `Blob`'s size (`InvalidInput` for
   * `size`, with `details.limit`), before anything is sent; then begins, writes the bytes in
   * order in pieces of `CHUNK_SIZE` (whatever sizes the source yields) and finishes.
   * `onProgress` is called once the upload has begun and after every piece. Any failure after
   * the upload began (of the transport, of the source or of `onProgress`) aborts it, then
   * rejects with that failure as a `CoreError`.
   */
  async attachmentUpload(
    spaceId: SpaceId,
    name: string,
    mime: string,
    source: UploadSource,
    onProgress?: UploadProgress,
  ): Promise<AttachmentInfo> {
    const attachments = this.#attachments("attachmentUpload");
    validateAttachmentName(name);
    validateMime(mime);
    const total = typeof Blob !== "undefined" && source instanceof Blob ? source.size : undefined;
    if (total !== undefined && total > MAX_ATTACHMENT_BYTES) {
      throw sizeLimitError();
    }
    const attachmentId = await this.attachmentBegin(spaceId, name, mime);
    try {
      let sent = 0;
      onProgress?.(sent, total);
      for await (const piece of sliceIntoChunks(source)) {
        await attachments.write(attachmentId, piece);
        sent += piece.length;
        onProgress?.(sent, total);
      }
      return await this.attachmentFinish(attachmentId);
    } catch (error) {
      try {
        await this.attachmentAbort(attachmentId);
      } catch {
        // The upload may be gone already (or the core locked meanwhile): the original failure
        // is what the caller needs, and the core's sweep deletes any chunk left behind.
      }
      throw toCoreError(error);
    }
  }

  /**
   * The plaintext of an attachment, whole (at most `MAX_PREVIEW_BYTES` without a range) or the
   * half-open byte `range`; see `AttachmentTransport.read` for the errors. An attachment that is
   * not complete on this device (`availability` other than `Complete`, Milestone 8) rejects with
   * `Storage`, "the attachment is not complete on this device (n of m chunks)", whatever the
   * range; under the on-demand policy that read also asks for its chunks, like
   * {@link CoreClient.attachmentDownload}. The UI avoids the read by looking at `availability`
   * first and reads again once `AttachmentsChanged` shows it `Complete`. The same holds for
   * {@link CoreClient.attachmentSaveAs} and {@link CoreClient.attachmentOpen}, which read it.
   */
  async attachmentRead(attachmentId: AttachmentId, range?: ByteRange): Promise<Uint8Array> {
    return this.#attachments("attachmentRead").read(attachmentId, range);
  }

  /**
   * The URL a `<video>` or `<audio>` element streams the attachment from (the desktop shell's
   * `kbmedia` protocol, ADR-0032), or `null` where there is none (the in-memory transport, the
   * web build): then read the bytes with {@link CoreClient.attachmentRead} and make a blob.
   */
  attachmentMediaUrl(attachmentId: AttachmentId): string | null {
    const attachments: Partial<AttachmentTransport> | undefined = this.#transport.attachments;
    return typeof attachments?.mediaUrl === "function" ? attachments.mediaUrl(attachmentId) : null;
  }

  /**
   * Uploads a file from disk through the shell: `path`, or one the user picks in the open
   * dialog (cancelling is `InvalidInput` for the field `path`).
   */
  async attachmentImportPath(spaceId: SpaceId, path?: string): Promise<AttachmentInfo> {
    return this.#attachments("attachmentImportPath").importPath(spaceId, path);
  }

  /**
   * Whether the transport can download a picture from a link (`attachments.importUrl`, ADR-0028):
   * the desktop shell can, the web build and a transport without a shell cannot. Apart from
   * {@link CoreClient.hasAttachments}, which says nothing about it.
   */
  canImportUrl(): boolean {
    return this.hasAttachments() && typeof this.#transport.attachments?.importUrl === "function";
  }

  /**
   * Has the transport download the picture the `https` link `url` points to, once, and store it
   * as an attachment of the space (ADR-0028); see `AttachmentTransport.importUrl` for what it
   * checks and how it fails (`InvalidInput` for the field `url`, `details.reason`, or `size`).
   * `Internal` "attachmentImportUrl is not supported by this transport" when
   * {@link CoreClient.canImportUrl} is false.
   */
  async attachmentImportUrl(spaceId: SpaceId, url: string): Promise<AttachmentInfo> {
    const attachments = this.#attachments("attachmentImportUrl");
    if (attachments.importUrl === undefined) {
      throw unsupported("attachmentImportUrl");
    }
    return attachments.importUrl(spaceId, url);
  }

  /**
   * Saves an attachment to `path`, or to a file the user picks in the save dialog (cancelling
   * is `InvalidInput` for the field `path`); resolves with the attachment and the file written.
   */
  async attachmentSaveAs(attachmentId: AttachmentId, path?: string): Promise<AttachmentSaveResult> {
    return this.#attachments("attachmentSaveAs").saveAs(attachmentId, path);
  }

  /** Opens an attachment with the system's default application (through a private temporary copy); resolves with the copy's path. */
  async attachmentOpen(attachmentId: AttachmentId): Promise<string> {
    return this.#attachments("attachmentOpen").open(attachmentId);
  }

  /** The transport's attachment methods; `Internal` "<method> is not supported by this transport" without them. */
  #attachments(method: string): AttachmentTransport {
    const attachments = this.#transport.attachments;
    if (attachments === undefined || !this.hasAttachments()) {
      throw unsupported(method);
    }
    return attachments;
  }

  /** Whether the transport can lock and unlock the workspace (`lock`, `unlock`). */
  hasAppLock(): boolean {
    return this.#transport.lock !== undefined && this.#transport.unlock !== undefined;
  }

  /** Whether the transport can write export files (`exportRun`, ADR-0036). */
  hasExport(): boolean {
    return this.#transport.exportRun !== undefined;
  }

  /** Whether the transport can read import sources (`importRun`, ADR-0036). */
  hasImport(): boolean {
    return this.#transport.importRun !== undefined;
  }

  /** Whether the transport can save a text file such as a CSV (`saveTextFile`, ADR-0036). */
  hasSaveTextFile(): boolean {
    return this.#transport.saveTextFile !== undefined;
  }

  /** Whether the transport runs scheduled backups to a folder (ADR-0036). */
  hasScheduledBackups(): boolean {
    return (
      this.#transport.backupFolderChoose !== undefined && this.#transport.backupRunNow !== undefined
    );
  }

  /** Whether the transport has Google Drive as a backup target and restore source (ADR-0059). */
  hasDrive(): boolean {
    return this.#transport.drive !== undefined;
  }

  /** Whether the transport lists the backups of a folder for the restore wizard (ADR-0059). */
  hasBackupFolderList(): boolean {
    return this.#transport.backupFolderList !== undefined;
  }

  /** Whether the transport can write and read backup files (`backupExport`, `backupImport`). */
  hasBackups(): boolean {
    return this.#transport.backupExport !== undefined && this.#transport.backupImport !== undefined;
  }

  /**
   * Locks the workspace: key material is dropped, {@link CoreClient.coreState} becomes
   * `"locked"` and every request rejects with `Locked` until {@link CoreClient.unlock}.
   * `InvalidInput` when the app lock is not enabled; `Internal` when the transport cannot lock.
   */
  async lock(): Promise<void> {
    const transport = this.#transport;
    if (transport.lock === undefined) {
      throw unsupported("lock");
    }
    await transport.lock();
  }

  /** Unlocks the workspace; a wrong passphrase is `InvalidInput` for the field `passphrase`. */
  async unlock(passphrase: string): Promise<void> {
    const transport = this.#transport;
    if (transport.unlock === undefined) {
      throw unsupported("unlock");
    }
    await transport.unlock(passphrase);
  }

  /**
   * Exports a page, a page with its descendants, or a whole space as Markdown or HTML (ADR-0036):
   * the shell asks where to save and writes a `.md`/`.html` file or a `.zip`. Cancelling is
   * `InvalidInput` for the field `path`.
   */
  async exportRun(options: ExportRunOptions): Promise<ExportRunResult> {
    const transport = this.#transport;
    if (transport.exportRun === undefined) {
      throw unsupported("exportRun");
    }
    return transport.exportRun(options);
  }

  /**
   * Imports files, a folder or a `.zip` into a space (ADR-0036), or into new spaces with
   * `destination: "newSpaces"` (an AppFlowy backup, ADR-0039), and resolves with the report.
   * Cancelling is `InvalidInput` for the field `path`.
   */
  async importRun(options: ImportRunOptions): Promise<ImportReport> {
    const transport = this.#transport;
    if (transport.importRun === undefined) {
      throw unsupported("importRun");
    }
    return transport.importRun(options);
  }

  /**
   * Opens an import session (ADR-0036) that will create pages under `parentId` (the space's top level when absent) and
   * resolves with its id. Without `sourceName` every top-level page of the source goes directly under the destination;
   * with it, several top-level pages go into one container page of that title. Replaces any session still open. The
   * session requests (`importAddFile`, `importPlan`, `importCommit`, `importAbort`) are what the shell's `importRun`
   * sends; the client exposes them for code that holds the text itself (the built-in starter templates).
   */
  async importBegin(spaceId: SpaceId, options: { parentId?: PageId; sourceName?: string } = {}): Promise<number> {
    const data: { spaceId: SpaceId; parentId?: PageId; sourceName?: string } = { spaceId };
    if (options.parentId !== undefined) data.parentId = options.parentId;
    if (options.sourceName !== undefined) data.sourceName = options.sourceName;
    return (await this.#call({ type: "ImportBegin", data }, "ImportStarted")).data;
  }

  /** Adds one text file (`.md`, `.txt`, `.html`, `.csv`, ...) to an import session; `path` is relative to the source. */
  async importAddFile(importId: number, path: string, text: string): Promise<void> {
    await this.#call({ type: "ImportAddFile", data: { importId, path, text } }, "Ok");
  }

  /** Plans an import session from the files added and the paths of the source's other files. */
  async importPlan(importId: number, binaryPaths: readonly string[] = []): Promise<ImportPlanInfo> {
    const data = { importId, binaryPaths: [...binaryPaths] };
    return (await this.#call({ type: "ImportPlan", data }, "ImportPlanned")).data;
  }

  /** Writes the next `maxPages` (1 to 500) pages of a planned import; the last call answers `done` with the report. */
  async importCommit(importId: number, maxPages: number): Promise<ImportProgress> {
    return (await this.#call({ type: "ImportCommit", data: { importId, maxPages } }, "ImportProgress")).data;
  }

  /** Drops an import session; the pages already written stay. Dropping an unknown session is fine. */
  async importAbort(importId: number): Promise<void> {
    await this.#call({ type: "ImportAbort", data: { importId } }, "Ok");
  }

  // ----- Local AI and MCP (ADR-0037, `docs/architecture/ai.md`) -----

  /** Whether the transport reaches Ollama through the shell (`ai`). */
  hasAi(): boolean {
    return this.#transport.ai !== undefined;
  }

  /** Endpoint, reachability, models, index and indexer state. */
  async aiStatus(): Promise<AiStatus> {
    return this.#ai("aiStatus").status();
  }

  /** Stores the Ollama endpoint (a remote host asks for consent in a native dialog). */
  async aiEndpointSet(url: string): Promise<AiStatus> {
    return this.#ai("aiEndpointSet").endpointSet(url);
  }

  /** Streams a writing action; resolves when the stream ended. */
  async aiGenerate(
    requestId: string,
    input: AiGenerateInput,
    onEvent: (event: AiStreamEvent) => void,
  ): Promise<void> {
    return this.#ai("aiGenerate").generate(requestId, input, onEvent);
  }

  /** Streams a chat answer over retrieved sources; resolves when the stream ended. */
  async aiChat(
    requestId: string,
    input: AiChatInput,
    onEvent: (event: AiStreamEvent) => void,
  ): Promise<void> {
    return this.#ai("aiChat").chat(requestId, input, onEvent);
  }

  /** Cancels a stream started with `aiGenerate` or `aiChat`. */
  async aiCancel(requestId: string): Promise<void> {
    return this.#ai("aiCancel").cancel(requestId);
  }

  /** Related passages for a query (Ctrl+K "Related"). */
  async aiSearch(query: string, scope: AiScope, limit?: number): Promise<AiSource[]> {
    return this.#ai("aiSearch").search(query, scope, limit);
  }

  /** Transcribes one 16 kHz mono PCM WAV segment. */
  async aiTranscribe(wav: Uint8Array, model?: string): Promise<AiTranscribed> {
    return this.#ai("aiTranscribe").transcribe(wav, model);
  }

  /** The semantic index: pages indexed and waiting, chunks, model. */
  async aiIndexStatus(): Promise<AiIndexInfo> {
    return (await this.#call({ type: "AiIndexStatus" }, "AiIndex")).data;
  }

  /** Deletes every vector; the indexer starts over. */
  async aiIndexClear(): Promise<void> {
    await this.#call({ type: "AiIndexClear" }, "Ok");
  }

  /** Passages for a question (keyword only without `vector`). */
  async aiRetrieve(
    query: string,
    scope: AiScope,
    options: { vector?: number[]; limit?: number } = {},
  ): Promise<AiSource[]> {
    return (
      await this.#call(
        {
          type: "AiRetrieve",
          data: {
            query,
            scope,
            ...(options.vector === undefined ? {} : { vector: options.vector }),
            ...(options.limit === undefined ? {} : { limit: options.limit }),
          },
        },
        "AiPassages",
      )
    ).data;
  }

  /** Whether the transport runs the local MCP server (`mcp`). */
  hasMcp(): boolean {
    return this.#transport.mcp !== undefined;
  }

  /** Whether the MCP server listens, its URL and the bridge's path. */
  async mcpStatus(): Promise<McpStatus> {
    return this.#mcp("mcpStatus").status();
  }

  /** Replaces a client's grants (adding access asks in a native dialog). */
  async mcpGrantsSet(clientId: string, grants: McpGrant[]): Promise<McpClientInfo> {
    return this.#mcp("mcpGrantsSet").grantsSet(clientId, grants);
  }

  /** MCP clients, oldest first. */
  async mcpClientList(): Promise<McpClientInfo[]> {
    return (await this.#call({ type: "McpClientList" }, "McpClients")).data;
  }

  /** Creates a client without grants; the token is in the answer, once. */
  async mcpClientCreate(name: string): Promise<McpClientCreated> {
    return (await this.#call({ type: "McpClientCreate", data: { name } }, "McpClientCreated")).data;
  }

  /** Deletes a client; its token stops working. */
  async mcpClientDelete(clientId: string): Promise<void> {
    await this.#call({ type: "McpClientDelete", data: { clientId } }, "Ok");
  }

  /** Audit rows, newest first. */
  async mcpAuditList(
    options: { clientId?: string; before?: number; limit?: number } = {},
  ): Promise<McpAuditEntry[]> {
    const data: { clientId?: string; before?: number; limit?: number } = {};
    if (options.clientId !== undefined) data.clientId = options.clientId;
    if (options.before !== undefined) data.before = options.before;
    if (options.limit !== undefined) data.limit = options.limit;
    return (await this.#call({ type: "McpAuditList", data }, "McpAudit")).data;
  }

  /** Deletes every audit row. */
  async mcpAuditClear(): Promise<void> {
    await this.#call({ type: "McpAuditClear" }, "Ok");
  }

  // ----- Local dictation (ADR-0053, `docs/architecture/dictation.md` section 5) -----

  /** Whether the transport can dictate: the desktop shell (`dictation`) or the in-memory fake (`?dictation=fake`). */
  hasDictation(): boolean {
    return this.#transport.dictation !== undefined;
  }

  /**
   * The transport's dictation (status, choice of engine, model installation, sessions). Throws an `Internal`
   * error when there is none: check {@link CoreClient.hasDictation} first.
   */
  dictation(): DictationApi {
    const dictation = this.#transport.dictation;
    if (dictation === undefined) {
      throw unsupported("dictation");
    }
    return dictation;
  }

  /**
   * The transport's Google Drive target (ADR-0059). Throws an `Internal` error when there is none: check
   * {@link CoreClient.hasDrive} first.
   */
  drive(): DriveApi {
    const drive = this.#transport.drive;
    if (drive === undefined) {
      throw unsupported("drive");
    }
    return drive;
  }

  #ai(method: string): AiTransport {
    const ai = this.#transport.ai;
    if (ai === undefined) {
      throw unsupported(method);
    }
    return ai;
  }

  #mcp(method: string): McpTransport {
    const mcp = this.#transport.mcp;
    if (mcp === undefined) {
      throw unsupported(method);
    }
    return mcp;
  }

  /** The rows of a view as CSV text (`ExportCsv`, ADR-0036), in the order of `pageIds`. */
  async exportCsv(spaceId: SpaceId, pageIds: PageId[], columns: CsvColumn[]): Promise<string> {
    const response = await this.#call({ type: "ExportCsv", data: { spaceId, pageIds, columns } }, "Csv");
    return response.data;
  }

  /** Saves a text file the person places (`Tasks.csv`); resolves with the path written. */
  async saveTextFile(suggestedName: string, text: string): Promise<string> {
    const transport = this.#transport;
    if (transport.saveTextFile === undefined) {
      throw unsupported("saveTextFile");
    }
    return transport.saveTextFile(suggestedName, text);
  }

  /** Picks the folder of scheduled backups (stored by the shell); resolves with it. */
  async backupFolderChoose(): Promise<string> {
    const transport = this.#transport;
    if (transport.backupFolderChoose === undefined) {
      throw unsupported("backupFolderChoose");
    }
    return transport.backupFolderChoose();
  }

  /** Writes a scheduled backup into the chosen folder now and prunes old ones. */
  async backupRunNow(): Promise<BackupResult> {
    const transport = this.#transport;
    if (transport.backupRunNow === undefined) {
      throw unsupported("backupRunNow");
    }
    return transport.backupRunNow();
  }

  /** The backups of a folder (the folder dialog first without `path`), newest first (ADR-0059). */
  async backupFolderList(path?: string): Promise<BackupFolderListing> {
    const transport = this.#transport;
    if (transport.backupFolderList === undefined) {
      throw unsupported("backupFolderList");
    }
    return transport.backupFolderList(path);
  }

  /**
   * Writes an encrypted backup of the whole workspace to `path`, or to a file the user picks
   * when it is left out (cancelling is `InvalidInput` for the field `path`). Needs a recovery
   * kit: `InvalidInput` for the field `kit` without one.
   */
  async backupExport(path?: string): Promise<BackupResult> {
    const transport = this.#transport;
    if (transport.backupExport === undefined) {
      throw unsupported("backupExport");
    }
    return transport.backupExport(path);
  }

  /**
   * Restores a backup from `path` (or a file the user picks) into an installation without a
   * workspace (`AlreadyExists` otherwise), with the recovery kit text unless the backup
   * belongs to this installation's account: a missing or wrong kit is `InvalidInput` for the
   * field `kit`, a damaged file `Crypto`.
   */
  async backupImport(path?: string, kit?: string): Promise<BackupResult> {
    const transport = this.#transport;
    if (transport.backupImport === undefined) {
      throw unsupported("backupImport");
    }
    return transport.backupImport(path, kit);
  }

  /**
   * The transport's raw-bytes page document sessions (`open`, `import`, `export`,
   * `close`); each method rejects with a `CoreError`.
   */
  get docs(): DocTransport {
    return this.#transport.docs;
  }

  /**
   * Opens an external link through the transport (the desktop shell hands http, https and
   * mailto URLs to the system). Resolves `false` when the transport cannot, in which case
   * the caller falls back to `window.open` with `noopener`.
   */
  async openExternal(url: string): Promise<boolean> {
    if (this.#transport.openExternal === undefined) {
      return false;
    }
    await this.#transport.openExternal(url);
    return true;
  }

  /**
   * Registers a listener for what the desktop shell tells the page on its own account (ADR-0040): the links to a
   * page that the operating system opened the app with, the quick capture, the tray's lock and the state of the
   * capture shortcut (see {@link ShellEvent}); returns the function that removes it. A transport with no shell
   * has none of these: the listener is never called and removing it does nothing.
   */
  onShellEvent(listener: (event: ShellEvent) => void): () => void {
    return this.#transport.onShellEvent?.(listener) ?? (() => undefined);
  }

  /**
   * Whether a desktop shell is around the core (ADR-0040): the transport can set the words of the shell's own
   * surfaces, so there is a tray, a quick-capture shortcut and a window that can hide to the tray. The in-memory
   * transport and the web build have none.
   */
  hasDesktopShell(): boolean {
    return this.#transport.shellLabelsSet !== undefined && this.#transport.onShellEvent !== undefined;
  }

  /**
   * Sends the words of the shell's own surfaces, the tray menu today (ADR-0040); only the labels named change.
   * Rejects with `InvalidInput` for a label that is not valid (1 to 64 characters, no control character) and
   * changes nothing then; `Internal` when the transport has no shell.
   */
  async shellLabelsSet(labels: ShellLabels): Promise<void> {
    const transport = this.#transport;
    if (transport.shellLabelsSet === undefined) {
      throw unsupported("shellLabelsSet");
    }
    await transport.shellLabelsSet(labels);
  }

  /**
   * Whether the desktop shell around the core has a native menu bar (ADR-0040) that a lone Alt press shows: the
   * transport can show and hide it. The in-memory transport, the web build and a shell without one have none.
   */
  hasMenuBar(): boolean {
    return this.#transport.menuBarShow !== undefined && this.#transport.menuBarHide !== undefined;
  }

  /** Shows the native menu bar (it may already be shown); `Internal` when the transport has none. */
  async menuBarShow(): Promise<void> {
    const transport = this.#transport;
    if (transport.menuBarShow === undefined) {
      throw unsupported("menuBarShow");
    }
    await transport.menuBarShow();
  }

  /** Hides the native menu bar (it may already be hidden); `Internal` when the transport has none. */
  async menuBarHide(): Promise<void> {
    const transport = this.#transport;
    if (transport.menuBarHide === undefined) {
      throw unsupported("menuBarHide");
    }
    await transport.menuBarHide();
  }

  // ----- shared spaces (Milestone 13, ADR-0041 to ADR-0051, `docs/development/milestone-13.md`) -----
  //
  // A request that writes in a space is gated by the account's role there: it rejects with `Forbidden`
  // (`details`: `{ reason: "role", required }` or `{ reason: "archived" }`), see `CoreError.details`.

  /** What sharing a space would send, counted; writes nothing (the share dialog's counts). */
  async spaceSharePlan(spaceId: SpaceId): Promise<SpaceSharePlan> {
    return (await this.#call({ type: "SpaceSharePlan", data: { spaceId } }, "SpaceSharePlan")).data;
  }

  /**
   * Shares a personal space with its whole history (ADR-0045): answers at once with `shareState: "Pending"`;
   * `"Shared"` follows (`SpacesChanged`) when the server accepts the feed. Needs sync, the recovery kit and a
   * profile name (`InvalidInput` for `serverUrl`, `account` or `displayName`).
   */
  async spaceShare(spaceId: SpaceId): Promise<SpaceInfo> {
    return (await this.#call({ type: "SpaceShare", data: { spaceId } }, "Space")).data;
  }

  /** The members, invites, settings and health of a shared space (any member; `InvalidInput` for a personal space). */
  async spaceMembers(spaceId: SpaceId): Promise<MembersInfo> {
    return (await this.#call({ type: "SpaceMembers", data: { spaceId } }, "Members")).data;
  }

  /** Changes a member's role (Owner); refused when it would leave no Owner. */
  async memberRoleSet(spaceId: SpaceId, accountId: AccountId, role: MemberRole): Promise<MembersInfo> {
    return (await this.#call({ type: "MemberRoleSet", data: { spaceId, accountId, role } }, "Members")).data;
  }

  /** Removes a member (Owner, not this account); the member can read nothing written afterwards. */
  async memberRemove(spaceId: SpaceId, accountId: AccountId): Promise<MembersInfo> {
    return (await this.#call({ type: "MemberRemove", data: { spaceId, accountId } }, "Members")).data;
  }

  /**
   * Leaves a shared space (a member, not the last Owner) and deletes the local copy; on an archive (a removed
   * member's or a closed space's read-only copy) it only deletes the local copy.
   */
  async spaceLeave(spaceId: SpaceId): Promise<void> {
    await this.#call({ type: "SpaceLeave", data: { spaceId } }, "Ok");
  }

  /** Closes a shared space for everyone (Owner): it becomes a read-only archive (`archiveReason: "Closed"`). */
  async spaceClose(spaceId: SpaceId): Promise<SpaceInfo> {
    return (await this.#call({ type: "SpaceClose", data: { spaceId } }, "Space")).data;
  }

  /** Changes the space's settings (Owner); what is left out stays. */
  async spaceSettingsSet(
    spaceId: SpaceId,
    settings: { trashRetentionDays?: number; editorsMayLink?: boolean },
  ): Promise<MembersInfo> {
    const data: Extract<Request, { type: "SpaceSettingsSet" }>["data"] = { spaceId };
    if (settings.trashRetentionDays !== undefined) {
      data.trashRetentionDays = settings.trashRetentionDays;
    }
    if (settings.editorsMayLink !== undefined) {
      data.editorsMayLink = settings.editorsMayLink;
    }
    return (await this.#call({ type: "SpaceSettingsSet", data }, "Members")).data;
  }

  /** Allows or forbids local AI and MCP in a space on this account's devices (any member; off by default when shared). */
  async spaceAiSet(spaceId: SpaceId, enabled: boolean): Promise<SpaceInfo> {
    return (await this.#call({ type: "SpaceAiSet", data: { spaceId, enabled } }, "Space")).data;
  }

  /** This account's profile (the display name co-members see). */
  async profileGet(): Promise<ProfileInfo> {
    return (await this.#call({ type: "ProfileGet" }, "Profile")).data;
  }

  /** Sets the display name (1 to 100 characters after trimming; `InvalidInput` for `displayName`). */
  async profileSet(displayName: string): Promise<ProfileInfo> {
    return (await this.#call({ type: "ProfileSet", data: { displayName } }, "Profile")).data;
  }

  /** A person known from a shared space or the verified list (`NotFound` for an unknown account). */
  async personGet(accountId: AccountId): Promise<PersonInfo> {
    return (await this.#call({ type: "PersonGet", data: { accountId } }, "Person")).data;
  }

  /** Marks a person verified (their safety number was compared) or not; it applies to every space. */
  async personVerifiedSet(accountId: AccountId, verified: boolean): Promise<PersonInfo> {
    return (await this.#call({ type: "PersonVerifiedSet", data: { accountId, verified } }, "Person")).data;
  }

  /**
   * Creates a link invite (Owner; the role is not Owner) and returns the link, **once**: the core keeps no copy of
   * the secret. Expires after `expiresInDays` (default 7, at most 30).
   */
  async inviteLinkCreate(spaceId: SpaceId, role: MemberRole, expiresInDays?: number): Promise<InviteLinkInfo> {
    const data: Extract<Request, { type: "InviteLinkCreate" }>["data"] = { spaceId, role };
    if (expiresInDays !== undefined) {
      data.expiresInDays = expiresInDays;
    }
    return (await this.#call({ type: "InviteLinkCreate", data }, "InviteLink")).data;
  }

  /** Revokes an open invite (Owner). */
  async inviteRevoke(spaceId: SpaceId, inviteId: InviteId): Promise<MembersInfo> {
    return (await this.#call({ type: "InviteRevoke", data: { spaceId, inviteId } }, "Members")).data;
  }

  /** Starts the inviter side of the code flow (Owner): the session shows the code, then the six digits. */
  async inviteCodeStart(spaceId: SpaceId, role: MemberRole): Promise<InviteSessionInfo> {
    return (await this.#call({ type: "InviteCodeStart", data: { spaceId, role } }, "Invite")).data;
  }

  /** Starts the invitee side of the code flow with the server and the code the inviter shows. */
  async inviteCodeJoin(serverUrl: string, code: string): Promise<InviteSessionInfo> {
    return (await this.#call({ type: "InviteCodeJoin", data: { serverUrl, code } }, "Invite")).data;
  }

  /** Starts the invitee side of the link flow from `atomik-kb://invite?...` or the `/join` form of the link. */
  async inviteLinkOpen(link: string): Promise<InviteSessionInfo> {
    return (await this.#call({ type: "InviteLinkOpen", data: { link } }, "Invite")).data;
  }

  /** The digits match (code flow) or the preview is accepted (link flow). */
  async inviteConfirm(): Promise<InviteSessionInfo> {
    return (await this.#call({ type: "InviteConfirm" }, "Invite")).data;
  }

  /** Ends the invite session. */
  async inviteCancel(): Promise<InviteSessionInfo> {
    return (await this.#call({ type: "InviteCancel" }, "Invite")).data;
  }

  /** The invite session: flow, side, state, code, digits, names, role, error, expiry. */
  async inviteStatus(): Promise<InviteSessionInfo> {
    return (await this.#call({ type: "InviteStatus" }, "Invite")).data;
  }

  // ----- comments, person mentions and notifications (Milestone 13 slice 2, ADR-0048 and ADR-0049) -----
  //
  // Comments are signed records of a stream of their own, never part of the page document: a Commenter
  // writes them without being able to edit the page. The writes are gated by the account's role and
  // reject with `Forbidden` like every write in a shared space (see `CoreError.details`).

  /** The threads of a page, oldest root first, with names and `isMine` (any member). */
  async commentList(pageId: PageId): Promise<ThreadInfo[]> {
    return (await this.#call({ type: "CommentList", data: { pageId } }, "Comments")).data;
  }

  /**
   * Posts a comment (Commenter and up): a new thread when `threadId` is absent (only then an `anchor`), a reply
   * when it names a thread of the page. Answers the thread. `InvalidInput` for `body`, `anchor` or `threadId`.
   */
  async commentPost(input: {
    pageId: PageId;
    threadId?: ThreadId;
    anchor?: CommentAnchor;
    body: CommentBodyItem[];
  }): Promise<ThreadInfo> {
    const { pageId, threadId, anchor, body } = input;
    const data = {
      pageId,
      body,
      ...(threadId === undefined ? {} : { threadId }),
      ...(anchor === undefined ? {} : { anchor }),
    };
    return (await this.#call({ type: "CommentPost", data }, "Thread")).data;
  }

  /** Replaces the body of a comment (its author only). Answers the thread. */
  async commentEdit(pageId: PageId, commentId: CommentId, body: CommentBodyItem[]): Promise<ThreadInfo> {
    return (await this.#call({ type: "CommentEdit", data: { pageId, commentId, body } }, "Thread")).data;
  }

  /**
   * Deletes a comment (its author, or an Owner): final. A comment with replies stays as a placeholder
   * (`deleted: true`). Answers the thread, with no comments when it was the last one.
   */
  async commentDelete(pageId: PageId, commentId: CommentId): Promise<ThreadInfo> {
    return (await this.#call({ type: "CommentDelete", data: { pageId, commentId } }, "Thread")).data;
  }

  /** Resolves (`true`) or reopens (`false`) a thread (its author, or Editor and up). Answers the thread. */
  async commentResolve(pageId: PageId, threadId: ThreadId, resolved: boolean): Promise<ThreadInfo> {
    return (await this.#call({ type: "CommentResolve", data: { pageId, threadId, resolved } }, "Thread")).data;
  }

  /**
   * The device's notifications, newest first (a local inbox raised from remote changes only). `limit` defaults
   * to 50 (at most 200), `beforeId` pages by row id, `unreadOnly` leaves the read rows out.
   */
  async notificationList(
    options: { limit?: number; beforeId?: number; unreadOnly?: boolean } = {},
  ): Promise<NotificationInfo[]> {
    const { limit, beforeId, unreadOnly } = options;
    const data = {
      ...(limit === undefined ? {} : { limit }),
      ...(beforeId === undefined ? {} : { beforeId }),
      ...(unreadOnly === undefined ? {} : { unreadOnly }),
    };
    return (await this.#call({ type: "NotificationList", data }, "Notifications")).data;
  }

  /** Marks notifications read: the listed row ids, or every row when `ids` is left out. */
  async notificationsMarkRead(ids?: number[]): Promise<void> {
    await this.#call({ type: "NotificationsMarkRead", data: ids === undefined ? {} : { ids } }, "Ok");
  }

  /** The people the `@` menu and a Person property offer in a space: its members, this account included. */
  async peopleMentionable(spaceId: SpaceId): Promise<PersonInfo[]> {
    return (await this.#call({ type: "PeopleMentionable", data: { spaceId } }, "People")).data;
  }

  // ----- Realtime and presence (Milestone 13, slice 3, ADR-0050) -----

  /**
   * Who else is in a shared space now, one entry per device (other members' devices, this account's other devices
   * too, never this one): the name, the colour index (the member's place in join order), the page and what the
   * device is doing. Empty for a personal space and an archive. Re-read it on `Event.PresenceChanged` (someone
   * joined, left or changed page; not on every cursor move).
   */
  async presenceList(spaceId: SpaceId): Promise<PresenceInfo[]> {
    return (await this.#call({ type: "PresenceList", data: { spaceId } }, "Presence")).data;
  }

  /** Whether the transport carries presence frames (the desktop shell, the in-memory mirror). */
  hasPresence(): boolean {
    return this.#transport.presence !== undefined;
  }

  /**
   * Sends this device's presence in a page of a shared space (the editor's ephemeral-store update, at most
   * `PRESENCE_UPDATE_MAX_BYTES`; `pageId` is `undefined` for an idle window). Best effort: a personal space, an
   * archive and an offline device send nothing and that is not an error. `Internal` "presenceSend is not
   * supported by this transport" without {@link CoreClient.hasPresence}.
   */
  async presenceSend(
    spaceId: SpaceId,
    pageId: PageId | undefined,
    state: PresenceState,
    update: Uint8Array,
  ): Promise<void> {
    const presence = this.#transport.presence;
    if (presence === undefined) {
      throw unsupported("presenceSend");
    }
    await presence.send(spaceId, pageId, state, update);
  }

  /**
   * Registers a listener for the presence frames other members' devices sent; returns the function that removes it.
   * A transport without presence never calls the listener.
   */
  onPresence(listener: (message: PresenceUpdate) => void): () => void {
    return this.#transport.presence?.subscribe(listener) ?? (() => undefined);
  }

  // ----- View-only links (Milestone 13, slice 4, ADR-0051) -----

  /**
   * Creates a view link for a page (Owner; Editor and up when the space's `editorsMayLink` is on; anyone in a
   * personal space). The link starts `Publishing`: its URL works once the next sync cycle uploaded the first
   * publication. `expiresInDays` is 1 to `LINK_EXPIRY_MAX_DAYS`; leave it out for a link that never expires.
   * `Forbidden` for the role or the setting; `InvalidInput` for `pageId`, `expiresInDays` or `serverUrl` (sync is
   * not configured). The URL holds the link's key: show it only to people who may copy it, never log it.
   */
  async viewLinkCreate(spaceId: SpaceId, pageId: PageId, expiresInDays?: number): Promise<ViewLinkInfo> {
    const data: Extract<Request, { type: "ViewLinkCreate" }>["data"] = { spaceId, pageId };
    if (expiresInDays !== undefined) {
      data.expiresInDays = expiresInDays;
    }
    return (await this.#call({ type: "ViewLinkCreate", data }, "ViewLink")).data;
  }

  /** The space's view links, newest first (any member). Re-read it on `Event.ViewLinksChanged`. */
  async viewLinkList(spaceId: SpaceId): Promise<ViewLinkInfo[]> {
    return (await this.#call({ type: "ViewLinkList", data: { spaceId } }, "ViewLinks")).data;
  }

  /** Revokes a view link (its creator, or an Owner) and returns the links that remain. */
  async viewLinkRevoke(spaceId: SpaceId, linkId: LinkId): Promise<ViewLinkInfo[]> {
    return (await this.#call({ type: "ViewLinkRevoke", data: { spaceId, linkId } }, "ViewLinks")).data;
  }

  // ----- lifecycle and events -------------------------------------------------------

  /** Lifecycle of the core; transports that cannot report it count as ready. */
  async coreState(): Promise<CoreStateInfo> {
    return (await this.#transport.coreState?.()) ?? "ready";
  }

  /** Registers an event listener; returns the function that removes it. */
  subscribe(onEvent: (event: Event) => void): () => void {
    return this.#transport.subscribe(onEvent);
  }
}
