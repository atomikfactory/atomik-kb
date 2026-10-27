//! The core API contract (ADR-0015): one request enum in, one response enum out,
//! events drained after each call, one error type.
//!
//! JSON shape (also what the generated TypeScript sees):
//! enums are adjacently tagged, `{ "type": "WorkspaceCreate", "data": { "name": "x" } }`;
//! struct fields are camelCase.
//!
//! Changing or removing a variant is a breaking change and bumps
//! [`API_VERSION`](crate::API_VERSION). Adding variants is not.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::Json;
use crate::comments::{CommentAnchor, CommentBodyItem, NotificationInfo, ThreadInfo};
use crate::ids::{
    AccountId, AttachmentId, BlockId, CommentId, DeviceId, InviteId, LinkId, OptionId, PageId,
    PropertyId, SpaceId, ThreadId, WorkspaceId,
};
use crate::interop::{
    CsvColumn, ExportBatch, ExportFormat, ExportPlan, ImportPlanInfo, ImportProgress,
};
use crate::properties::{
    OptionColor, PageProperties, PropertyConfig, PropertyDef, PropertyFilter, PropertyKind,
    PropertyOption, PropertySchema, PropertyValue, StatusGroup,
};
use crate::sharing::{
    ArchiveReason, FeedSyncInfo, InviteLinkInfo, InviteSessionInfo, MemberRole, MembersInfo,
    PersonInfo, PresenceInfo, ProfileInfo, SpaceSharePlan, SpaceShareState, ViewLinkInfo,
};

/// Everything a shell can ask the core to do. Milestones 1 to 5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all_fields = "camelCase")]
pub enum Request {
    /// Health and configuration summary; always available, even before onboarding.
    AppStatus,
    /// Creates the (single) workspace. Fails with `AlreadyExists` if one exists.
    WorkspaceCreate {
        /// Display name, 1 to 200 characters after trimming.
        name: String,
        /// When set, a first space with this name is created along with the workspace.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        first_space_name: Option<String>,
    },
    /// The workspace, or `null` before onboarding.
    WorkspaceGet,
    /// Reads a setting. Keys are `namespace.name`, namespaces `ui` and `app`.
    SettingsGet {
        /// Setting key.
        key: String,
    },
    /// Writes a setting (any JSON value, at most 64 KiB serialised).
    SettingsSet {
        /// Setting key.
        key: String,
        /// New value.
        value: Json,
    },
    /// This device's identity and name.
    DeviceInfo,
    /// Renames this device (1 to 100 characters after trimming).
    DeviceRename {
        /// New display name.
        name: String,
    },

    // ----- Milestone 2: spaces and page tree -----
    /// All spaces in display order.
    SpaceList,
    /// Creates a space (name 1 to 100 characters after trimming) appended last.
    SpaceCreate {
        /// Display name.
        name: String,
        /// Optional icon (ADR-0029): an emoji or short text of 1 to 32 bytes after trimming, or
        /// `icon:<name>[:<color>]` from the bundled set (never an image for a space).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        icon: Option<String>,
    },
    /// Renames a space.
    SpaceRename {
        /// Space.
        space_id: SpaceId,
        /// New name.
        name: String,
    },
    /// Sets or clears a space icon.
    SpaceSetIcon {
        /// Space.
        space_id: SpaceId,
        /// New icon, or `None` to clear: an emoji or short text, or `icon:<name>[:<color>]`
        /// (ADR-0029; an `attachment:` image is refused for a space).
        icon: Option<String>,
    },
    /// Moves a space to `index` in the display order (clamped).
    SpaceMove {
        /// Space.
        space_id: SpaceId,
        /// Target position, 0-based.
        index: u32,
    },
    /// Deletes a personal space with all its live and trashed pages, for good (a purge of each). A shared
    /// space is left or closed instead.
    SpaceDelete {
        /// Space.
        space_id: SpaceId,
    },
    /// The live page tree, trash and undo state of a space.
    TreeGet {
        /// Space.
        space_id: SpaceId,
    },
    /// One page's metadata.
    PageGet {
        /// Page.
        page_id: PageId,
    },
    /// Creates a page (title 0 to 500 characters after trimming) under `parent_id`
    /// (or at the space root) at `index` among its siblings (default: last).
    PageCreate {
        /// Space.
        space_id: SpaceId,
        /// Parent page, or `None` for the space root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
        /// Position among siblings; `None` appends.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        index: Option<u32>,
        /// Title; may be empty.
        title: String,
    },
    /// Renames a page.
    PageRename {
        /// Page.
        page_id: PageId,
        /// New title.
        title: String,
    },
    /// Sets or clears a page icon.
    PageSetIcon {
        /// Page.
        page_id: PageId,
        /// New icon, or `None` to clear (ADR-0029): an emoji or short text, `icon:<name>[:<color>]`
        /// from the bundled set, or `attachment:<id>`, an image attachment of the page's space
        /// (kept while the page references it).
        icon: Option<String>,
    },
    /// Sets, repositions or clears a page cover (ADR-0040). `cover` set: the page gets that cover and
    /// `cover_y` (50 when absent). `cover` absent with `cover_y` set: only the position changes
    /// (`InvalidInput` when the page has no cover). Both absent: the cover and its position are
    /// cleared.
    PageSetCover {
        /// Page.
        page_id: PageId,
        /// New cover: `attachment:<id>` (an image attachment of the page's space, kept while the
        /// page references it), `color:<palette name>` or `gradient:<name>` (a bundled gradient the
        /// UI defines; the core checks the shape `[a-z0-9-]{1,32}` only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        cover: Option<String>,
        /// Vertical position of the image in the banner, 0 (top) to 100 (bottom) percent; 50, the
        /// default, is stored as no value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        cover_y: Option<u8>,
    },
    /// Changes how a page looks and whether the UI lets it be edited (ADR-0040): its font, small text and
    /// lock. Each field is optional and an absent one is left as it is. A field is cleared by its
    /// neutral value: `font` `default`, `small_text` and `locked` `false`. Setting what the page already
    /// has changes nothing. The reply is the page. One undo step, like the icon.
    ///
    /// The lock is a guard against accidental edits in the UI, not a permission: the core does not
    /// refuse a write to a locked page (moves, trash, import and MCP writes all go through).
    PageSetAppearance {
        /// Page.
        page_id: PageId,
        /// The page's font: `serif`, `mono`, or `default` to clear the choice.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        font: Option<PageFont>,
        /// Smaller text on the page; `false` clears it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        small_text: Option<bool>,
        /// The page opens read-only in the UI; `false` unlocks it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        locked: Option<bool>,
    },
    /// Moves a page (with its subtree) under `parent_id` (or the space root) to
    /// `index` among its new siblings. Moving under its own descendant is
    /// `InvalidInput`.
    PageMove {
        /// Page.
        page_id: PageId,
        /// New parent, or `None` for the space root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
        /// Position among the new siblings, clamped.
        index: u32,
    },
    /// Puts the live children of one parent into the given order, in one step (ADR-0052). It is a
    /// general reorder: the caller computes the order (the UI owns the collation, "sort subpages" is
    /// one use) and the core applies it with the fewest moves, in one commit, one persisted update,
    /// one `StructureChanged` and **one structural undo step**. Nothing about the order is stored
    /// (no "keep sorted" flag); manual moves stay free afterwards. The pages are not edited: no
    /// `updated_at` or `edited_by` is stamped. Needs the Editor role in a shared space, like
    /// `PageMove`. Answers `Ok`.
    ///
    /// `order` must be exactly the live children of `parent_id` (or the live top-level pages), each
    /// once. `InvalidInput`: any other set (`order`; a page was added, moved away or trashed since the
    /// caller read the tree: it reads again and retries), a parent that is not a live page of the
    /// space (`parentId`). Writes nothing, and emits nothing, when the children already stand in
    /// that order.
    PageReorderChildren {
        /// The space.
        space_id: SpaceId,
        /// The parent page, or `None` for the top-level pages of the space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
        /// The live children, in the wanted order.
        order: Vec<PageId>,
    },
    /// Applies one action to a set of pages of one space, all or nothing (ADR-0058,
    /// `docs/architecture/database-power-ups.md` section 5.4): set or clear a property value,
    /// move under a parent, duplicate, or move to the trash. Structural actions (move, duplicate,
    /// trash) first drop the pages whose ancestor is in the list, and are **one structural undo
    /// step**; value changes are one properties change, not in structural undo (as
    /// `PagePropertySet`). Needs the role of the action's single-page request. Answers
    /// [`Response::PageBulk`]: the top copies for a duplicate, else the pages acted on.
    ///
    /// `InvalidInput`: no page or more than 1,000, a repeat, a page that is not a live page of the
    /// space (`pageIds`); more than 1,000 pages to copy (`pageIds`); `values` of another length
    /// than `pageIds` (`values`); a parent that is not a live page of the space or is one of the
    /// pages or inside one (`parentId`); a value the property refuses (as `PagePropertySet`).
    PageBulk {
        /// The space.
        space_id: SpaceId,
        /// The pages, in the order the action uses (the view's display order).
        page_ids: Vec<PageId>,
        /// What to do with them.
        action: BulkAction,
    },
    /// Moves a page and its subtree to the trash.
    PageTrash {
        /// Page.
        page_id: PageId,
    },
    /// Restores a trashed page to its previous parent (or the root if that parent
    /// is gone).
    PageRestore {
        /// Page.
        page_id: PageId,
    },
    /// Permanently deletes a page and its subtree. Irreversible.
    PagePurge {
        /// Page.
        page_id: PageId,
    },
    /// What moving a live page and its live subtree to another space would do (ADR-0038):
    /// the same checks as [`Request::PageMoveToSpace`], nothing written. Answers
    /// `SpaceMovePlanned`.
    PageMoveToSpacePlan {
        /// Page.
        page_id: PageId,
        /// Target space (not the page's own).
        space_id: SpaceId,
        /// New parent: a live page of the target space, or `None` for its root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
    },
    /// Moves a live page and its live subtree to another space, keeping their ids (ADR-0038).
    /// Values follow by property name (missing properties created in the target), relation
    /// values keep only pages that move along, views are remapped, favourites follow,
    /// attachments used only by the moved pages move with them. Not an undo step.
    /// `InvalidInput`: a trashed page (`pageId`), the page's own space (`spaceId`: use
    /// `PageMove`), a parent that is not a live page of the target (`parentId`), more than
    /// 10,000 pages (`pageId`). Answers `PagesMovedToSpace`; emits `PagesMovedSpace`,
    /// `StructureChanged` for both spaces and the usual schema, attachment and content events.
    PageMoveToSpace {
        /// Page.
        page_id: PageId,
        /// Target space.
        space_id: SpaceId,
        /// New parent in the target, or `None` for its root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
        /// Position among the new parent's live children, clamped; `None` = last.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        index: Option<u32>,
    },
    /// Duplicates a live page with its live sub-pages (ADR-0040). Every copy has a new page id, its
    /// blocks have new block ids, and the copy of the page goes right after it among its siblings.
    /// Titles are kept, except the top copy's: the literal " (copy)" is appended (English only, the
    /// core has no translations; the title is cut to fit the 500 characters). Icons and property
    /// values are copied; a relation value that lists a page of the copied subtree lists that page's
    /// copy, any other related page is kept, and a two-way relation keeps both sides paired. In the
    /// content, a page reference or card that points to a page of the subtree points to its copy
    /// (and to the copy of the block it names), any other link is kept, and a whiteboard's drawing
    /// goes with its block, and a view's `defaultTemplate` that points into the subtree follows the
    /// copy. The copy of a template is a template with the same placeholders (ADR-0056). Favourites
    /// are not copied, trashed sub-pages are skipped, attachments stay shared by id (the copies'
    /// blocks, values and icons keep them referenced). The copy is
    /// one structural undo step. `InvalidInput`: a page in the trash, or a subtree of more than
    /// 1,000 pages (both `pageId`); `NotFound` for an unknown page. Answers `Page` with the copy of
    /// the page; emits `StructureChanged`, `SpacesChanged`, `PageContentChanged` (source `Core`) for
    /// every new page and `BacklinksChanged` for the pages the copies link to.
    PageDuplicate {
        /// The page to duplicate.
        page_id: PageId,
    },
    /// Makes a page a template or a page again (ADR-0056, `docs/architecture/templates.md`). A
    /// template is a page like any other (edited, synced, shared, moved, exported), hidden from the
    /// tree's lists, views, search, backlinks, reminders and the AI index together with its
    /// sub-pages; `PageCreateFromTemplate` makes pages from it. Setting what the page already is
    /// changes nothing. `InvalidInput` for a page in the trash; Editor role. Answers `Page`; emits
    /// `StructureChanged`. One undo step, like the appearance.
    PageSetTemplate {
        /// Page.
        page_id: PageId,
        /// Whether the page is a template.
        template: bool,
    },
    /// Sets or clears the placeholder of one property on a template page (ADR-0056): `today` and
    /// `now` for a `date` property, `me` for a `person` property, resolved when a page is made from
    /// the template. Setting a placeholder removes the page's stored value of that property, and
    /// `PagePropertySet` on it removes the placeholder. `InvalidInput`: the page is not a template
    /// and not inside one, the property's kind does not take that placeholder, or the page is in
    /// the trash; `NotFound` for an unknown page or property. Editor role. Answers `Page`; emits
    /// `StructureChanged`.
    PageSetTemplatePlaceholder {
        /// The template page (or a page inside one).
        page_id: PageId,
        /// The property.
        property_id: PropertyId,
        /// The placeholder; `None` clears it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        placeholder: Option<TemplatePlaceholder>,
    },
    /// Makes a page from a template (ADR-0056): a copy of the template's live subtree (at most
    /// 1,000 pages) placed under `parent_id` (the space's top level when absent) at `index` (last
    /// when absent), in the template's space. The copies are pages (not templates); placeholders
    /// are resolved at the time of the request with the core's clock and local offset and the
    /// requesting account: property placeholders, date mentions and person mentions marked
    /// `relative`, and `{{date}}` and `{{time}}` in the titles; `values` are then written on the
    /// top copy (a database's tag, a board column's value, a calendar day). Relations, page links,
    /// mentions and the `defaultTemplate` of views that point inside the subtree follow the copies,
    /// as `PageDuplicate` does. One structural undo step. `InvalidInput`: the page is not a template,
    /// the parent is in another space, in the trash or inside the template being copied, a value is
    /// refused, or the subtree is too large; `NotFound` for an unknown page. Editor role. Answers
    /// `Page` with the new top page; emits the events of `PageDuplicate`.
    PageCreateFromTemplate {
        /// The template page.
        template_id: PageId,
        /// Where the new page goes; the space's top level when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
        /// The position among the parent's live children; last when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        index: Option<u32>,
        /// Values written on the new top page after the placeholders.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[cfg_attr(
            feature = "ts",
            ts(optional, as = "Option<Vec<(PropertyId, PropertyValue)>>")
        )]
        values: Vec<(PropertyId, PropertyValue)>,
    },
    /// Adds a page to its space's favourites (last) or removes it (ADR-0029). Favourites are
    /// synced per space and shown at the top of the sidebar, in order (`TreeView::favorites`).
    /// Idempotent: adding a favourite that exists keeps its place, removing one that does not
    /// exist is fine. Adding a page that is in the trash is `InvalidInput` (field `pageId`);
    /// removing works for any page. A favourite whose page is trashed is kept and hidden, and
    /// comes back with a restore. Not an undo step. Emits `StructureChanged`.
    PageFavoriteSet {
        /// Page.
        page_id: PageId,
        /// `true` to add the page to the favourites, `false` to remove it.
        favorite: bool,
    },
    /// Moves a favourite in front of another favourite of the same space (`before`), or last
    /// when `before` is `None`. `InvalidInput` when `page_id` is not a favourite (field
    /// `pageId`) or `before` is not one (field `before`). Not an undo step. Emits
    /// `StructureChanged`.
    PageFavoriteMove {
        /// The favourite to move.
        page_id: PageId,
        /// The favourite it goes in front of, or `None` for the end.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        before: Option<PageId>,
    },
    /// Undoes this device's last structural change in a space.
    StructureUndo {
        /// Space.
        space_id: SpaceId,
    },
    /// Redoes the last undone structural change in a space.
    StructureRedo {
        /// Space.
        space_id: SpaceId,
    },

    // ----- Milestone 3: page content, links, search -----
    /// Full-text search over page titles and block text; trashed pages are excluded.
    /// Tokens match by prefix (`not` finds "notes"); the query is 1 to 200 characters
    /// after trimming.
    ///
    /// A page matches when **every** word matches somewhere in it: its title, a block or a
    /// property (ADR-0040). The hits are, first, the rows that hold every word (a page can have
    /// several), then one hit for each other page whose words are spread over several rows, for the
    /// row that holds the most of them, highlighted for the words that row holds. When that is
    /// fewer hits than `limit`, a fuzzy pass over the titles of the live pages adds the pages whose
    /// title has the query's letters in order, or (for a word of 4 or more characters) a word
    /// with one typo, after every other hit and without repeating a page; a fuzzy hit has no
    /// `block_id` and its snippet is the title. The `Titles` scope searches titles only, with the
    /// same fuzzy pass.
    Search {
        /// What to look for.
        query: String,
        /// Restrict the search to one space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        space_id: Option<SpaceId>,
        /// Titles only (page pickers) or titles and bodies (default).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        scope: Option<SearchScope>,
        /// Maximum number of hits, clamped to 1..=100; default 20.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        limit: Option<u32>,
        /// Only pages last edited at or after this time, in Unix milliseconds (the page's
        /// `updated_at`); absent for any time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        edited_after: Option<i64>,
    },
    /// Pages (and blocks) that reference a page, excluding trashed sources.
    BacklinksGet {
        /// The referenced page.
        page_id: PageId,
    },
    /// A page's content as portable (ProseMirror) JSON. Works for trashed pages too.
    PageContentGet {
        /// Page.
        page_id: PageId,
    },
    /// The versions of a page, newest first (ADR-0057, `docs/architecture/page-history.md`): one per
    /// editing session this device recorded in its local journal (dated, with an author), then the
    /// older history of the document as undated versions. Any member of the space may ask; works
    /// for trashed pages. `before` pages through the list (a version id of an earlier answer);
    /// `limit` defaults to 100, at most 500. `NotFound` for an unknown page; `InvalidInput` for a
    /// `before` the page does not have. Answers `PageHistory`.
    PageHistoryList {
        /// Page.
        page_id: PageId,
        /// Only versions older than this one (a version id from an earlier answer).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        before: Option<String>,
        /// Most versions in the answer (default 100, at most 500).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        limit: Option<u32>,
    },
    /// The content of a page at a version, as portable JSON, and its title then when known
    /// (ADR-0057). Any member; `InvalidInput` for a version the page does not have. Answers
    /// `PageVersionContent`.
    PageHistoryGet {
        /// Page.
        page_id: PageId,
        /// A version id from `PageHistoryList`.
        version: String,
    },
    /// Makes a page's content (and its title, when the version knows it) what it was at a version
    /// (ADR-0057): a new change on top of the current one, synced and recorded in the journal as a
    /// restore, so the version before it stays in the history and can be restored in turn. Editor
    /// role; not for a page in the trash (`InvalidInput`); `InvalidInput` for a version the page does
    /// not have. Answers `Page`; emits `PageContentChanged` (source `Core`), `BacklinksChanged`, and
    /// `StructureChanged` when the title changes.
    PageHistoryRestore {
        /// Page.
        page_id: PageId,
        /// A version id from `PageHistoryList`.
        version: String,
    },
    /// The first image of each of the given pages, in document order (ADR-0054): the attachment of
    /// the first `image` block, or the first item of the first `gallery` block, whichever comes
    /// first, at any depth (columns, toggles, callouts). Answered from a device-local index, so it
    /// is cheap enough for a gallery of a whole database. A page without an image, a page that is
    /// not in `space_id` and an unknown page are left out of the answer; live and trashed pages are
    /// both answered. At most 500 ids (`InvalidInput` for more), the answer is in the order of the
    /// ids asked. `NotFound` for an unknown space. Read only: no role is needed beyond membership.
    PageFirstImages {
        /// Space of the pages.
        space_id: SpaceId,
        /// The pages, at most 500.
        page_ids: Vec<PageId>,
    },
    /// The first lines of text of pages, for a feed card (ADR-0058,
    /// `docs/architecture/database-power-ups.md` section 6.2), read from the text index without
    /// opening a document: at most 280 characters, blocks joined with a line break, cut at a word
    /// with `…`. Pages that are not live pages of the space, templates and pages without body text
    /// are left out of the answer. `InvalidInput` (`pageIds`) for more than 500 pages. A read: any
    /// member may ask.
    PageExcerpts {
        /// Space of the pages.
        space_id: SpaceId,
        /// The pages, at most 500.
        page_ids: Vec<PageId>,
    },
    /// A page's content rendered as Markdown (page references become `[[Title]]`).
    PageExportMarkdown {
        /// Page.
        page_id: PageId,
    },
    /// Drops and rebuilds the search, link and date-mention indexes from the documents, for
    /// one space or for the whole workspace.
    IndexRebuild {
        /// Space, or `None` for every space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        space_id: Option<SpaceId>,
    },

    // ----- Reminders on date mentions (ADR-0024) -----
    /// The reminders of date mentions that fell due since this device last looked: those
    /// whose fire time is after the `app.reminders.last_checked` mark (at most seven days back:
    /// the window starts seven days before `until` when the mark is unset or older) and not
    /// after `until`, oldest first, at most 100, trashed pages left out. A pure query: it
    /// neither fires anything nor moves the mark (the scheduler tick does, see
    /// `Core::reminders_tick`).
    RemindersDue {
        /// The device's wall-clock time now, in seconds since 1970-01-01T00:00:00 as the
        /// local clock reads it (UTC plus the device's offset). Date mentions are floating
        /// local dates (ADR-0016), so a reminder is due when the local wall clock reaches
        /// its moment, and every time of this API is on that clock.
        until: i64,
    },

    // ----- Typed page properties (ADR-0026). The schema is per space and lives in its
    // structure document, so it syncs; values live per page. -----
    /// The property schema of a space: Tags first (even before anything wrote it), then the
    /// other properties in the order of their positions.
    PropertySchemaGet {
        /// Space.
        space_id: SpaceId,
    },
    /// Creates a property (name 1 to 100 characters after trimming) of a kind, appended last.
    /// A `status` property starts with the options "Not started", "In progress" and "Done".
    /// At most 100 properties per space (Tags included). A `relation` takes a
    /// [`PropertyConfig::Relation`] (a two-way one creates the paired property too, in the same
    /// commit; the reply is the forward property) and a `rollup` requires a
    /// [`PropertyConfig::Rollup`] (ADR-0027); the other kinds take none.
    PropertyCreate {
        /// Space.
        space_id: SpaceId,
        /// Display name.
        name: String,
        /// Kind, fixed from now on.
        kind: PropertyKind,
        /// The configuration of a `relation` or `rollup` (absent: a one-way relation).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        config: Option<PropertyConfig>,
    },
    /// Changes the configuration of a `rollup` property (relation, target and function). A
    /// relation's configuration is fixed at creation (`InvalidInput`). Replies with the property.
    PropertyConfigSet {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
        /// The new configuration (a rollup's).
        config: PropertyConfig,
    },
    /// Renames a property.
    PropertyRename {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
        /// New name.
        name: String,
    },
    /// Removes a property from the schema (Tags cannot be deleted: `InvalidInput`). Its values
    /// leave the index at once and stay in the page documents until the next write of that
    /// page's properties removes them.
    PropertyDelete {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
    },
    /// Moves a property before another one (`None` = last). Tags always stays first.
    PropertyMove {
        /// Space.
        space_id: SpaceId,
        /// Property to move.
        property_id: PropertyId,
        /// The property it goes in front of, or `None` for the end.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        before: Option<PropertyId>,
    },
    /// Adds an option (name 1 to 100 characters after trimming) to a `select`, `multi_select`
    /// or `status` property, appended last. A `status` option has a group (default `todo`);
    /// the other kinds take none.
    PropertyOptionAdd {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
        /// Display name.
        name: String,
        /// Colour from the palette.
        color: OptionColor,
        /// Group of a `status` option.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        group: Option<StatusGroup>,
    },
    /// Changes an option's name, colour or group (what is left out stays as it is).
    PropertyOptionUpdate {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
        /// Option.
        option_id: OptionId,
        /// New name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        name: Option<String>,
        /// New colour.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        color: Option<OptionColor>,
        /// New group (`status` options only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        group: Option<StatusGroup>,
    },
    /// Removes an option and takes it out of every page's value of that property in the space,
    /// in one change.
    PropertyOptionDelete {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
        /// Option.
        option_id: OptionId,
    },
    /// Sets (or, with `None`, clears) one property value of a page, live or trashed. The value's
    /// kind must be the property's, option ids must belong to it, and an empty text, URL or
    /// multi-select list clears the value.
    PagePropertySet {
        /// Page.
        page_id: PageId,
        /// Property.
        property_id: PropertyId,
        /// New value, or `None` to clear.
        value: Option<PropertyValue>,
    },
    /// The property values of a page, in schema order.
    PagePropertiesGet {
        /// Page.
        page_id: PageId,
    },
    /// The live pages of a space whose value of a property matches a filter, ordered by title
    /// (ASCII case-insensitive, then exact, then page id). Trashed pages are left out.
    PagesByProperty {
        /// Space.
        space_id: SpaceId,
        /// Property.
        property_id: PropertyId,
        /// What to look for.
        filter: PropertyFilter,
    },

    // ----- Import and export (ADR-0036) -----
    /// Plans an export of one page (`include_children: false`), a page and its descendants, or
    /// a whole space (`page_id: None`) and opens an export session. Live pages only, in tree
    /// order. Replaces any export session still open. Nothing is read from the page documents
    /// yet: they are rendered by `ExportNext`.
    ExportBegin {
        /// Space.
        space_id: SpaceId,
        /// Root page, or `None` for every live page of the space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        page_id: Option<PageId>,
        /// Include the root page's descendants (ignored without `page_id`).
        include_children: bool,
        /// File format.
        format: ExportFormat,
    },
    /// The next entries of an export, at most `max_entries` (1 to 500), each page rendered
    /// as it is reached. Emits `ExportProgress`. `NotFound` for an unknown session.
    ExportNext {
        /// Session.
        export_id: u32,
        /// Most entries to return.
        max_entries: u32,
    },
    /// Closes an export session (finished or cancelled). Closing an unknown one is fine.
    ExportEnd {
        /// Session.
        export_id: u32,
    },
    /// The rows of a view as CSV (RFC 4180, CRLF line ends, a UTF-8 byte order mark): a header
    /// row, then one row per page in the order given. Pages that are unknown, trashed or of
    /// another space are skipped; property columns the schema lacks are `InvalidInput`.
    ExportCsv {
        /// Space.
        space_id: SpaceId,
        /// The pages, in row order (at most 50,000).
        page_ids: Vec<PageId>,
        /// The columns, in order (at most 101).
        columns: Vec<CsvColumn>,
    },
    /// Opens an import session that will create pages under `parent_id` (or at the space
    /// root). Replaces any import session still open (its pages were not created). The session
    /// lives in memory only and is dropped when the app locks.
    ImportBegin {
        /// Space.
        space_id: SpaceId,
        /// Destination page, or `None` for the space root.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
        /// Name of the source (the zip's or folder's name without extension): when the source
        /// holds several top-level pages they go into one container page of this title. `None`
        /// (files picked one by one): every top-level page goes directly under the destination.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        source_name: Option<String>,
    },
    /// Opens an import session whose destination is new spaces (ADR-0039): `ImportPlan` creates
    /// one space per space of the source (an AppFlowy backup's spaces; one space named
    /// `source_name`, or "Imported", for a source without spaces) and lists them in
    /// `ImportPlanInfo::spaces`. Otherwise like [`Request::ImportBegin`]; an aborted session
    /// deletes the spaces it created that are still empty. Answers `ImportStarted`.
    ImportBeginSpaces {
        /// Name of the source (the zip's name without extension).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        source_name: Option<String>,
    },
    /// Adds one text file of the source: Markdown (`.md`, `.markdown`), plain text (`.txt`),
    /// HTML (`.html`, `.htm`), CSV (`.csv`), a whiteboard (`.excalidraw`), the sidecar
    /// `atomik-export.json`, or a `.json` file of an AppFlowy backup (ADR-0039). `path` is relative to the source root, `/`-separated. At most
    /// 16 MiB per file and 10,000 files per session (`InvalidInput`).
    ImportAddFile {
        /// Session.
        import_id: u32,
        /// Path in the source.
        path: String,
        /// The file's text (decoded by the shell; a byte order mark removed).
        text: String,
    },
    /// Plans the import from the text files added and the paths of the source's other files,
    /// and answers which of those the pages need as attachments.
    ImportPlan {
        /// Session.
        import_id: u32,
        /// Every other file of the source, relative and `/`-separated (at most 100,000).
        binary_paths: Vec<String>,
    },
    /// Reports that a needed file (or a remote file, by its key) was uploaded as an attachment of
    /// the space the plan gave it.
    ImportAddAttachment {
        /// Session.
        import_id: u32,
        /// Path, as listed in `ImportPlanInfo::needed_files`, or a key of
        /// `ImportPlanInfo::remote_files`.
        path: String,
        /// The attachment holding its bytes.
        attachment_id: AttachmentId,
    },
    /// Writes the next pages of a planned import, at most `max_pages` (1 to 500). The first
    /// call also creates the properties the pages need. Emits `ImportProgress`, then the usual
    /// structure, schema, content and attachment events. The last call answers `done` with the
    /// report and closes the session.
    ImportCommit {
        /// Session.
        import_id: u32,
        /// Most pages to write in this call.
        max_pages: u32,
    },
    /// Drops an import session. Pages already written stay (the person can trash the
    /// container); attachments uploaded for it and never used are collected by the retention
    /// of attachments. Aborting an unknown session is fine.
    ImportAbort {
        /// Session.
        import_id: u32,
    },

    // ----- Local AI and MCP (ADR-0037) -----
    /// The semantic index: pages indexed, waiting, chunks, model.
    AiIndexStatus,
    /// The next pages whose vectors are stale, chunked, with the chunks that need an embedding
    /// (at most `max_chunks`, 1 to 64, of those; at least one page when any is pending). Empty
    /// with `model: None` while AI or the index is off or no embedding model is set. A model
    /// change clears the index first. Shell only (the webview's is refused).
    AiIndexNext {
        /// Most chunks needing a vector in the answer.
        max_chunks: u32,
    },
    /// Stores the vectors of a page computed from `AiIndexNext`'s work. Ignored (answers `Ok`,
    /// the page stays dirty) when the page or the model changed since. Emits `AiIndexChanged`.
    /// Shell only.
    AiIndexStore {
        /// Page.
        page_id: PageId,
        /// `AiIndexPage::source_hash` of the work.
        source_hash: String,
        /// `AiIndexWork::model` of the work.
        model: String,
        /// The vectors of the chunks that needed one.
        vectors: Vec<crate::ai::AiChunkVector>,
    },
    /// Deletes every vector and marks every page for indexing. Emits `AiIndexChanged`.
    AiIndexClear,
    /// Passages for a question: semantic (when `vector` is given and the index has chunks) and
    /// any-word full text, fused; trashed pages excluded; at most 3 per page.
    AiRetrieve {
        /// The question (1 to 2,000 characters).
        query: String,
        /// The question's embedding (with the query prefix of the index's model).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        vector: Option<Vec<f32>>,
        /// Where to look.
        scope: crate::ai::AiScope,
        /// Most passages, 1 to 20 (default 8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        limit: Option<u32>,
    },
    /// MCP clients, oldest first.
    McpClientList,
    /// Creates an MCP client without grants and returns its token, once. Emits `McpChanged`.
    McpClientCreate {
        /// Name (1 to 60 characters after trimming).
        name: String,
    },
    /// Replaces a client's grants (write implies read; spaces that do not exist are dropped).
    /// Emits `McpChanged`. Shell only: the webview calls `mcp_client_grants_set`, which asks for
    /// confirmation in a native dialog when access is added.
    McpClientGrantsSet {
        /// Client.
        client_id: String,
        /// Every grant of the client.
        grants: Vec<crate::ai::McpGrant>,
    },
    /// Deletes a client; its token stops working. Emits `McpChanged`.
    McpClientDelete {
        /// Client.
        client_id: String,
    },
    /// Audit rows, newest first.
    McpAuditList {
        /// Only this client's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        client_id: Option<String>,
        /// Only rows with an id below this one (paging).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        before: Option<i64>,
        /// Most rows, 1 to 500 (default 100).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        limit: Option<u32>,
    },
    /// Deletes every audit row. Emits `McpChanged`.
    McpAuditClear,
    /// One MCP message from the loopback server: the token is checked (`InvalidInput` for
    /// `token` when unknown), the message handled by `kb-mcp`, tool calls checked against the
    /// grants and audited. Shell only.
    McpCall {
        /// Bearer token.
        token: String,
        /// JSON-RPC message text.
        message: String,
    },

    // ----- Milestone 4: keys and identity -----
    /// Recovery kit, app lock, device and keyring status.
    SecurityStatus,
    /// Creates the recovery kit (and the account's device log). `AlreadyExists` when
    /// one exists. The kit is returned once and never stored.
    RecoveryKitCreate,
    /// Checks a typed kit (24 words or the code form) against the account.
    RecoveryKitVerify {
        /// The kit as typed.
        kit: String,
    },
    /// Issues a new kit: new recovery key, new root key (everything re-wrapped, the
    /// database rekeyed). The previous kit stops working. While the app lock is on,
    /// the passphrase is required to re-wrap the root key.
    RecoveryKitReissue {
        /// App-lock passphrase, required while the lock is enabled.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        passphrase: Option<String>,
    },
    /// Enables the app lock with a passphrase (8 to 256 bytes).
    AppLockEnable {
        /// Passphrase.
        passphrase: String,
    },
    /// Disables the app lock; the current passphrase is required.
    AppLockDisable {
        /// Current passphrase.
        passphrase: String,
    },
    /// Changes the app-lock passphrase.
    AppLockChange {
        /// Current passphrase.
        current: String,
        /// New passphrase.
        new: String,
    },
    /// The devices of the account, from the device log.
    DeviceList,

    // ----- Attachments (Milestone 5, ADR-0020). The bytes travel through the binary
    // API (`attachment_write` / `attachment_read`), never inside a request. -----
    /// Starts an upload: allocates the attachment id and its key; bytes follow through
    /// the binary API, then [`Request::AttachmentFinish`].
    AttachmentBegin {
        /// Owning space.
        space_id: SpaceId,
        /// File name shown to the user (1..=255 bytes, no path separators).
        name: String,
        /// MIME type (printable ASCII with one `/`, at most 127 bytes).
        mime: String,
    },
    /// Seals the last chunk and records the attachment (manifest, key, index).
    AttachmentFinish {
        /// The upload.
        attachment_id: AttachmentId,
    },
    /// Drops an upload in progress and the chunks it wrote.
    AttachmentAbort {
        /// The upload.
        attachment_id: AttachmentId,
    },
    /// The attachments of a space, newest first.
    AttachmentList {
        /// Space.
        space_id: SpaceId,
    },
    /// One attachment.
    AttachmentGet {
        /// Attachment.
        attachment_id: AttachmentId,
    },
    /// Purges an attachment now, whatever references it (its blocks then show
    /// "deleted"): key tombstone, manifest removed, chunks deleted. Irreversible.
    AttachmentDelete {
        /// Attachment.
        attachment_id: AttachmentId,
    },
    /// Collects attachments unreferenced for longer than the retention setting and
    /// sweeps chunks no manifest claims.
    AttachmentGc,
    /// Every chunk of the attachment this device lacks is wanted now, whatever the
    /// download policy (ADR-0023); the driver is poked. A complete attachment answers
    /// its info unchanged.
    AttachmentDownload {
        /// Attachment.
        attachment_id: AttachmentId,
    },

    // ----- Sync (Milestone 6, ADR-0021). The network lives in the shell's driver; these
    // configure it and read its status. -----
    /// Points this device at a server (`None` disables sync and forgets the session).
    /// The invite token is needed only when the account does not exist on that server
    /// yet; it is never stored.
    SyncConfigure {
        /// `http(s)://host[:port]`; `None` disables.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        server_url: Option<String>,
        /// Operator invite token for creating the account on the server.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        invite_token: Option<String>,
    },
    /// The sync status.
    SyncStatus,
    /// Asks the driver for a cycle now.
    SyncNow,
    /// Starts pairing (ADR-0022). The joiner (a fresh installation) gives the server
    /// address and no code; the sponsor gives the code typed from the joiner's screen
    /// (its server is the sync configuration).
    PairingStart {
        /// This side's role.
        role: PairingRole,
        /// The server to pair through (joiner).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        server_url: Option<String>,
        /// The pairing code (sponsor).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        code: Option<String>,
    },
    /// The user compared the six digits on both devices and they match.
    PairingConfirm,
    /// Ends the pairing session.
    PairingCancel,
    /// The pairing session's state.
    PairingStatus,
    /// Revokes another device: `DeviceRevoked`, then the root key and the space keys
    /// rotate and a new recovery kit is issued (the old one is void). The passphrase
    /// is required when the app lock is on.
    DeviceRevoke {
        /// The device to remove.
        device_id: DeviceId,
        /// App-lock passphrase.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        passphrase: Option<String>,
    },
    /// Finishes a root key rotation another device made, when the app lock here
    /// needs the passphrase to store the new key.
    RotationApply {
        /// App-lock passphrase.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        passphrase: Option<String>,
    },
    /// Recovers the account from a server with the recovery kit (fresh installation).
    RecoveryStart {
        /// The server holding the recovery blob.
        server_url: String,
        /// The kit (words or code).
        kit: String,
    },
    /// The recovery's state.
    RecoveryStatus,
    /// Ends the recovery.
    RecoveryCancel,

    // ----- Milestone 13: shared spaces (ADR-0041 to ADR-0051, `docs/security/sharing.md`,
    // `docs/development/milestone-13.md`, slice 1). A request that writes in a space is gated by
    // the account's role there: `Forbidden` with `details` `{ "reason": "role", "required": ... }`,
    // or `{ "reason": "archived" }` for any write to an archive. -----
    /// What sharing a space would send, counted; writes nothing. Any space of this workspace.
    SpaceSharePlan {
        /// Space.
        space_id: SpaceId,
    },
    /// Shares a personal space with its whole history (ADR-0045, section 10): mints the first
    /// shared epoch, posts the `Create` that makes the feed, then publishes every document. Answers
    /// `shareState: Pending` at once; `Shared` follows when the server accepts the `Create`.
    /// Needs sync, the account (the recovery kit) and a profile name. `InvalidInput` for `spaceId`
    /// (already shared), `serverUrl` (no sync), `account` (no kit yet) or `displayName` (no
    /// profile name). Emits `SpacesChanged`, `StructureChanged`, `SyncRequested`.
    SpaceShare {
        /// Space.
        space_id: SpaceId,
    },
    /// The members, invites, settings and health of a shared space (any member). `InvalidInput`
    /// for `spaceId` on a personal space.
    SpaceMembers {
        /// Space.
        space_id: SpaceId,
    },
    /// Changes a member's role (Owner): a `RoleSet` entry. `InvalidInput` for `accountId` (not a
    /// member) or `role` (it would leave no Owner). Emits `SpaceMembersChanged`.
    MemberRoleSet {
        /// Space.
        space_id: SpaceId,
        /// The member.
        account_id: AccountId,
        /// The new role.
        role: MemberRole,
    },
    /// Removes a member (Owner, not this account): a `Remove` entry with its Rekey, so the member
    /// cannot read anything written afterwards. `InvalidInput` for `accountId`. Emits
    /// `SpaceMembersChanged`.
    MemberRemove {
        /// Space.
        space_id: SpaceId,
        /// The member to remove.
        account_id: AccountId,
    },
    /// Leaves a shared space (a member, not the last Owner): a `Leave` entry, then the local data
    /// goes. On an archive it only deletes the local copy. Emits `SpacesChanged`.
    SpaceLeave {
        /// Space.
        space_id: SpaceId,
    },
    /// Closes a shared space for everyone (Owner): a `Close` entry; the space becomes an archive
    /// (`archiveReason: Closed`). Emits `SpacesChanged`, `SpaceRemoved`.
    SpaceClose {
        /// Space.
        space_id: SpaceId,
    },
    /// Changes the space's settings (Owner): a `Settings` entry. A field left out stays. Emits
    /// `SpaceMembersChanged`.
    SpaceSettingsSet {
        /// Space.
        space_id: SpaceId,
        /// Days a trashed page stays before the Owners' devices purge it; 0 keeps for ever.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        trash_retention_days: Option<u32>,
        /// Editors may create view links (slice 4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        editors_may_link: Option<bool>,
    },
    /// Turns local AI and MCP on or off for a shared space on this account's devices (any member;
    /// `spaceSettings` of the account document, ADR-0047). Off by default for a shared space.
    /// Emits `SpacesChanged`.
    SpaceAiSet {
        /// Space.
        space_id: SpaceId,
        /// Allowed.
        enabled: bool,
    },
    /// This account's profile.
    ProfileGet,
    /// Sets the display name (1 to 100 characters after trimming) and appends a `Profile` entry
    /// to every shared space. `InvalidInput` for `displayName`. Emits `PeopleChanged`.
    ProfileSet {
        /// The new name.
        display_name: String,
    },
    /// A person known from a shared space or the verified list. `NotFound` for an unknown account.
    PersonGet {
        /// The account.
        account_id: AccountId,
    },
    /// Marks a person verified (their safety number was compared) or not: `verifiedPeople` of the
    /// account document. Emits `PeopleChanged`, `SpaceMembersChanged`.
    PersonVerifiedSet {
        /// The account.
        account_id: AccountId,
        /// Verified.
        verified: bool,
    },
    /// Creates a link invite (Owner; the role is not Owner): an `Invite` entry whose secret goes
    /// into the returned link, once. Expires after `expiresInDays` (default 7, at most 30).
    /// `InvalidInput` for `role` or `expiresInDays`. Emits `SpaceMembersChanged`.
    InviteLinkCreate {
        /// Space.
        space_id: SpaceId,
        /// The role the invite grants.
        role: MemberRole,
        /// Lifetime in days.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        expires_in_days: Option<u32>,
    },
    /// Revokes an open invite (Owner): an `InviteRevoke` entry. `InvalidInput` for `inviteId`.
    /// Emits `SpaceMembersChanged`.
    InviteRevoke {
        /// Space.
        space_id: SpaceId,
        /// The invite.
        invite_id: InviteId,
    },
    /// Starts the inviter side of the code flow (Owner): opens a mailbox and shows the code.
    /// Emits `InviteChanged`, `InviteRequested`.
    InviteCodeStart {
        /// Space.
        space_id: SpaceId,
        /// The role the invite grants (not Owner).
        role: MemberRole,
    },
    /// Starts the invitee side of the code flow: needs a workspace and a recovery kit.
    /// `InvalidInput` for `serverUrl`, `code` or `workspace`. Emits `InviteChanged`,
    /// `InviteRequested`.
    InviteCodeJoin {
        /// The inviter's server, `http(s)://host[:port]`.
        server_url: String,
        /// The code the inviter shows.
        code: String,
    },
    /// Starts the invitee side of the link flow; accepts `atomik-kb://invite?...` and the
    /// `https://<server>/join#...` form. `InvalidInput` for `link`. Emits `InviteChanged`,
    /// `InviteRequested`.
    InviteLinkOpen {
        /// The link.
        link: String,
    },
    /// The digits match (code flow) or the preview is accepted (link flow).
    InviteConfirm,
    /// Ends the invite session.
    InviteCancel,
    /// The invite session's state.
    InviteStatus,

    // ----- Milestone 13, slice 2: comments, person mentions and notifications (ADR-0048,
    // ADR-0049, `docs/development/milestone-13.md` section 2.1). Comments are signed records of a
    // stream of their own, never part of the page document: a Commenter writes them without being
    // able to edit the page. A write is gated by the account's role (`Forbidden`, as in slice 1). -----
    /// The threads of a page, oldest root first (any member; a personal space answers an empty
    /// list). A thread whose comments are all deleted is left out. `NotFound` for the page.
    CommentList {
        /// Page.
        page_id: PageId,
    },
    /// Posts a comment (Commenter and up): a new thread when `threadId` is absent (only then an
    /// `anchor`, and the new comment's id is the thread's id), a reply when it names a thread of the
    /// page. Answers the thread. `InvalidInput` for `body` (empty, over 10 KB serialised, a page or
    /// account id that does not parse), `anchor` (a quote over 200 characters, a malformed cursor,
    /// given with a `threadId`) or `threadId` (not a thread of this page); `NotFound` for the page;
    /// `Forbidden` for a Viewer or an archive. Emits `CommentsChanged`, `SyncRequested`.
    CommentPost {
        /// Page.
        page_id: PageId,
        /// The thread to reply in; absent starts a new thread.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        thread_id: Option<ThreadId>,
        /// Where a new thread points; refused for a reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        anchor: Option<CommentAnchor>,
        /// The body.
        body: Vec<CommentBodyItem>,
    },
    /// Replaces the body of a comment (the author only; a deleted comment cannot be edited).
    /// Answers the thread. `InvalidInput` for `body`; `NotFound` for the page or the comment;
    /// `Forbidden` for anyone else. Emits `CommentsChanged`, `SyncRequested`.
    CommentEdit {
        /// Page.
        page_id: PageId,
        /// The comment.
        comment_id: CommentId,
        /// The new body.
        body: Vec<CommentBodyItem>,
    },
    /// Deletes a comment (the author, or an Owner): final. A comment that has replies stays as a
    /// placeholder (`deleted: true`, empty body). Answers the thread (with no comments when this was
    /// the last one). `NotFound` for the page or the comment; `Forbidden` for anyone else. Emits
    /// `CommentsChanged`, `SyncRequested`.
    CommentDelete {
        /// Page.
        page_id: PageId,
        /// The comment.
        comment_id: CommentId,
    },
    /// Resolves or reopens a thread (the thread's author, or Editor and up). Answers the thread.
    /// `NotFound` for the page or the thread; `Forbidden` for anyone else. Emits `CommentsChanged`,
    /// `SyncRequested`.
    CommentResolve {
        /// Page.
        page_id: PageId,
        /// The thread.
        thread_id: ThreadId,
        /// Resolved (`true`) or open again.
        resolved: bool,
    },
    /// The device's notifications, newest first (this account; a local table, ADR-0048).
    NotificationList {
        /// How many rows at most (default 50, at most 200).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        limit: Option<u32>,
        /// Only rows with a smaller id than this (paging).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        before_id: Option<i64>,
        /// Only rows not yet read.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        unread_only: Option<bool>,
    },
    /// Marks notifications read: the listed ones, or every one when `ids` is absent. Emits
    /// `NotificationsChanged`.
    NotificationsMarkRead {
        /// The rows; absent marks all.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        ids: Option<Vec<i64>>,
    },
    /// The people the `@` menu and a Person property offer in a space: its members, this account
    /// included (in a personal space, this account alone). `NotFound` for the space.
    PeopleMentionable {
        /// Space.
        space_id: SpaceId,
    },

    // ----- Milestone 13, slice 3: realtime and presence (ADR-0050,
    // `docs/development/milestone-13.md` section 3.1). Presence frames are not requests: the shell's
    // commands `presence_send` and `presence_events` carry them (the sealed bytes never reach the
    // webview). -----
    /// Who else is in the space now, one entry per device: other members' devices (this account's
    /// other devices too, never this device) with a presence frame accepted in the last 30 seconds,
    /// in no particular order. Empty for a personal space and for an archive. `NotFound` for the
    /// space.
    PresenceList {
        /// Space.
        space_id: SpaceId,
    },

    // ----- Milestone 13, slice 4: view-only links (ADR-0051,
    // `docs/development/milestone-13.md` section 4.1). The server and the viewer are not part of this API:
    // `kb-net` uploads publications and `apps/viewer` reads them. -----
    /// Creates a view link for a page (Owner; Editor and up when the space's `editorsMayLink` is set; any
    /// account in a personal space): a fresh link key `K_link`, its `Link` record in the keyring, the map
    /// entry in the structure, and the first publication, uploaded by the next sync cycle. The link
    /// starts `Publishing`. `expiresInDays` is 1 to `LINK_EXPIRY_MAX_DAYS`, absent for no expiry.
    /// `Forbidden` for the role or the setting; `InvalidInput` for `pageId` (not a page of the space,
    /// trashed, purged, or carrying more attachment chunks than `LINK_MAX_CHUNKS`), `expiresInDays`
    /// or `serverUrl` (sync is not configured: there is no server to publish to). Emits
    /// `ViewLinksChanged`.
    ViewLinkCreate {
        /// Space.
        space_id: SpaceId,
        /// The page to publish.
        page_id: PageId,
        /// Lifetime in days; absent: the link never expires.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        expires_in_days: Option<u32>,
    },
    /// The space's view links, newest first (any member; an archive too): every entry of the
    /// structure's `links` map. Empty for a space with none. `NotFound` for the space.
    ViewLinkList {
        /// Space.
        space_id: SpaceId,
    },
    /// Revokes a view link (its creator, or an Owner): the server row and publication are deleted
    /// (retried until the server answers), the `Link` record is tombstoned (an Owner's device; the
    /// creator's, when they are not an Owner, leaves it to an Owner's device) and the map entry goes.
    /// Answers the links that remain. `Forbidden` for another member; `InvalidInput` for `linkId` (not a
    /// link of the space). Emits `ViewLinksChanged`.
    ViewLinkRevoke {
        /// Space.
        space_id: SpaceId,
        /// The link.
        link_id: LinkId,
    },
}

impl Request {
    /// The variant name, for logs and metrics. Never includes payload data.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Request::AppStatus => "AppStatus",
            Request::WorkspaceCreate { .. } => "WorkspaceCreate",
            Request::WorkspaceGet => "WorkspaceGet",
            Request::SettingsGet { .. } => "SettingsGet",
            Request::SettingsSet { .. } => "SettingsSet",
            Request::DeviceInfo => "DeviceInfo",
            Request::DeviceRename { .. } => "DeviceRename",
            Request::SpaceList => "SpaceList",
            Request::SpaceCreate { .. } => "SpaceCreate",
            Request::SpaceRename { .. } => "SpaceRename",
            Request::SpaceSetIcon { .. } => "SpaceSetIcon",
            Request::SpaceMove { .. } => "SpaceMove",
            Request::SpaceDelete { .. } => "SpaceDelete",
            Request::TreeGet { .. } => "TreeGet",
            Request::PageGet { .. } => "PageGet",
            Request::PageCreate { .. } => "PageCreate",
            Request::PageRename { .. } => "PageRename",
            Request::PageSetIcon { .. } => "PageSetIcon",
            Request::PageSetCover { .. } => "PageSetCover",
            Request::PageSetAppearance { .. } => "PageSetAppearance",
            Request::PageMove { .. } => "PageMove",
            Request::PageReorderChildren { .. } => "PageReorderChildren",
            Request::PageBulk { .. } => "PageBulk",
            Request::PageMoveToSpacePlan { .. } => "PageMoveToSpacePlan",
            Request::PageMoveToSpace { .. } => "PageMoveToSpace",
            Request::PageDuplicate { .. } => "PageDuplicate",
            Request::PageSetTemplate { .. } => "PageSetTemplate",
            Request::PageSetTemplatePlaceholder { .. } => "PageSetTemplatePlaceholder",
            Request::PageCreateFromTemplate { .. } => "PageCreateFromTemplate",
            Request::PageTrash { .. } => "PageTrash",
            Request::PageRestore { .. } => "PageRestore",
            Request::PagePurge { .. } => "PagePurge",
            Request::PageFavoriteSet { .. } => "PageFavoriteSet",
            Request::PageFavoriteMove { .. } => "PageFavoriteMove",
            Request::StructureUndo { .. } => "StructureUndo",
            Request::StructureRedo { .. } => "StructureRedo",
            Request::Search { .. } => "Search",
            Request::BacklinksGet { .. } => "BacklinksGet",
            Request::PageHistoryList { .. } => "PageHistoryList",
            Request::PageHistoryGet { .. } => "PageHistoryGet",
            Request::PageHistoryRestore { .. } => "PageHistoryRestore",
            Request::PageContentGet { .. } => "PageContentGet",
            Request::PageFirstImages { .. } => "PageFirstImages",
            Request::PageExcerpts { .. } => "PageExcerpts",
            Request::PageExportMarkdown { .. } => "PageExportMarkdown",
            Request::IndexRebuild { .. } => "IndexRebuild",
            Request::RemindersDue { .. } => "RemindersDue",
            Request::PropertySchemaGet { .. } => "PropertySchemaGet",
            Request::PropertyCreate { .. } => "PropertyCreate",
            Request::PropertyConfigSet { .. } => "PropertyConfigSet",
            Request::PropertyRename { .. } => "PropertyRename",
            Request::PropertyDelete { .. } => "PropertyDelete",
            Request::PropertyMove { .. } => "PropertyMove",
            Request::PropertyOptionAdd { .. } => "PropertyOptionAdd",
            Request::PropertyOptionUpdate { .. } => "PropertyOptionUpdate",
            Request::PropertyOptionDelete { .. } => "PropertyOptionDelete",
            Request::PagePropertySet { .. } => "PagePropertySet",
            Request::PagePropertiesGet { .. } => "PagePropertiesGet",
            Request::PagesByProperty { .. } => "PagesByProperty",
            Request::SecurityStatus => "SecurityStatus",
            Request::RecoveryKitCreate => "RecoveryKitCreate",
            Request::RecoveryKitVerify { .. } => "RecoveryKitVerify",
            Request::RecoveryKitReissue { .. } => "RecoveryKitReissue",
            Request::AppLockEnable { .. } => "AppLockEnable",
            Request::AppLockDisable { .. } => "AppLockDisable",
            Request::AppLockChange { .. } => "AppLockChange",
            Request::DeviceList => "DeviceList",
            Request::AttachmentBegin { .. } => "AttachmentBegin",
            Request::AttachmentFinish { .. } => "AttachmentFinish",
            Request::AttachmentAbort { .. } => "AttachmentAbort",
            Request::AttachmentList { .. } => "AttachmentList",
            Request::AttachmentGet { .. } => "AttachmentGet",
            Request::AttachmentDelete { .. } => "AttachmentDelete",
            Request::AttachmentGc => "AttachmentGc",
            Request::AttachmentDownload { .. } => "AttachmentDownload",
            Request::SyncConfigure { .. } => "SyncConfigure",
            Request::SyncStatus => "SyncStatus",
            Request::SyncNow => "SyncNow",
            Request::PairingStart { .. } => "PairingStart",
            Request::PairingConfirm => "PairingConfirm",
            Request::PairingCancel => "PairingCancel",
            Request::PairingStatus => "PairingStatus",
            Request::DeviceRevoke { .. } => "DeviceRevoke",
            Request::RotationApply { .. } => "RotationApply",
            Request::RecoveryStart { .. } => "RecoveryStart",
            Request::RecoveryStatus => "RecoveryStatus",
            Request::RecoveryCancel => "RecoveryCancel",
            Request::ExportBegin { .. } => "ExportBegin",
            Request::ExportNext { .. } => "ExportNext",
            Request::ExportEnd { .. } => "ExportEnd",
            Request::ExportCsv { .. } => "ExportCsv",
            Request::ImportBegin { .. } => "ImportBegin",
            Request::ImportBeginSpaces { .. } => "ImportBeginSpaces",
            Request::ImportAddFile { .. } => "ImportAddFile",
            Request::ImportPlan { .. } => "ImportPlan",
            Request::ImportAddAttachment { .. } => "ImportAddAttachment",
            Request::ImportCommit { .. } => "ImportCommit",
            Request::ImportAbort { .. } => "ImportAbort",
            Request::AiIndexStatus => "AiIndexStatus",
            Request::AiIndexNext { .. } => "AiIndexNext",
            Request::AiIndexStore { .. } => "AiIndexStore",
            Request::AiIndexClear => "AiIndexClear",
            Request::AiRetrieve { .. } => "AiRetrieve",
            Request::McpClientList => "McpClientList",
            Request::McpClientCreate { .. } => "McpClientCreate",
            Request::McpClientGrantsSet { .. } => "McpClientGrantsSet",
            Request::McpClientDelete { .. } => "McpClientDelete",
            Request::McpAuditList { .. } => "McpAuditList",
            Request::McpAuditClear => "McpAuditClear",
            Request::McpCall { .. } => "McpCall",
            Request::SpaceSharePlan { .. } => "SpaceSharePlan",
            Request::SpaceShare { .. } => "SpaceShare",
            Request::SpaceMembers { .. } => "SpaceMembers",
            Request::MemberRoleSet { .. } => "MemberRoleSet",
            Request::MemberRemove { .. } => "MemberRemove",
            Request::SpaceLeave { .. } => "SpaceLeave",
            Request::SpaceClose { .. } => "SpaceClose",
            Request::SpaceSettingsSet { .. } => "SpaceSettingsSet",
            Request::SpaceAiSet { .. } => "SpaceAiSet",
            Request::ProfileGet => "ProfileGet",
            Request::ProfileSet { .. } => "ProfileSet",
            Request::PersonGet { .. } => "PersonGet",
            Request::PersonVerifiedSet { .. } => "PersonVerifiedSet",
            Request::InviteLinkCreate { .. } => "InviteLinkCreate",
            Request::InviteRevoke { .. } => "InviteRevoke",
            Request::InviteCodeStart { .. } => "InviteCodeStart",
            Request::InviteCodeJoin { .. } => "InviteCodeJoin",
            Request::InviteLinkOpen { .. } => "InviteLinkOpen",
            Request::InviteConfirm => "InviteConfirm",
            Request::InviteCancel => "InviteCancel",
            Request::InviteStatus => "InviteStatus",
            Request::CommentList { .. } => "CommentList",
            Request::CommentPost { .. } => "CommentPost",
            Request::CommentEdit { .. } => "CommentEdit",
            Request::CommentDelete { .. } => "CommentDelete",
            Request::CommentResolve { .. } => "CommentResolve",
            Request::NotificationList { .. } => "NotificationList",
            Request::NotificationsMarkRead { .. } => "NotificationsMarkRead",
            Request::PeopleMentionable { .. } => "PeopleMentionable",
            Request::PresenceList { .. } => "PresenceList",
            Request::ViewLinkCreate { .. } => "ViewLinkCreate",
            Request::ViewLinkList { .. } => "ViewLinkList",
            Request::ViewLinkRevoke { .. } => "ViewLinkRevoke",
        }
    }
}

/// Successful results, one variant per request kind that returns data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data")]
pub enum Response {
    /// Reply to [`Request::AppStatus`].
    AppStatus(AppStatus),
    /// Reply to [`Request::WorkspaceCreate`] and [`Request::WorkspaceGet`].
    Workspace(Option<Workspace>),
    /// Reply to [`Request::SettingsGet`]; `None` when unset.
    Setting(Option<Json>),
    /// Reply to [`Request::DeviceInfo`] and [`Request::DeviceRename`].
    DeviceInfo(DeviceInfo),
    /// Reply to [`Request::SpaceList`] and [`Request::SpaceMove`].
    Spaces(Vec<SpaceInfo>),
    /// Reply to [`Request::SpaceCreate`], [`Request::SpaceRename`], [`Request::SpaceSetIcon`],
    /// [`Request::SpaceShare`], [`Request::SpaceClose`] and [`Request::SpaceAiSet`].
    Space(SpaceInfo),
    /// Reply to [`Request::TreeGet`], [`Request::StructureUndo`], [`Request::StructureRedo`].
    Tree(TreeView),
    /// Reply to [`Request::PageGet`], [`Request::PageCreate`], [`Request::PageRename`],
    /// [`Request::PageSetIcon`], [`Request::PageSetCover`], [`Request::PageSetAppearance`],
    /// [`Request::PageRestore`], [`Request::PageDuplicate`], [`Request::PageSetTemplate`],
    /// [`Request::PageSetTemplatePlaceholder`], [`Request::PageCreateFromTemplate`].
    Page(PageInfo),
    /// Reply to [`Request::Search`].
    Search(Vec<SearchHit>),
    /// Reply to [`Request::BacklinksGet`].
    Backlinks(Vec<Backlink>),
    /// Reply to [`Request::PageContentGet`].
    PageContent(PageContent),
    /// Reply to [`Request::PageHistoryList`].
    PageHistory(PageHistory),
    /// Reply to [`Request::PageHistoryGet`].
    PageVersionContent(PageVersionContent),
    /// Reply to [`Request::PageFirstImages`].
    PageFirstImages(Vec<PageFirstImage>),
    /// Reply to [`Request::PageExcerpts`].
    PageExcerpts(Vec<PageExcerpt>),
    /// Reply to [`Request::PageBulk`]: the top copies of a duplicate, else the pages acted on.
    PageBulk(Vec<PageId>),
    /// Reply to [`Request::PageExportMarkdown`].
    Markdown(String),
    /// Reply to [`Request::RemindersDue`].
    Reminders(Vec<ReminderDue>),
    /// Reply to [`Request::PropertySchemaGet`].
    PropertySchema(PropertySchema),
    /// Reply to [`Request::PropertyCreate`] and [`Request::PropertyRename`].
    Property(PropertyDef),
    /// Reply to [`Request::PropertyOptionAdd`] and [`Request::PropertyOptionUpdate`].
    PropertyOption(PropertyOption),
    /// Reply to [`Request::PagePropertySet`] (the page's values after the change) and
    /// [`Request::PagePropertiesGet`].
    PageProperties(PageProperties),
    /// Reply to [`Request::PagesByProperty`].
    PageIds(Vec<PageId>),
    /// Reply to [`Request::SecurityStatus`].
    Security(SecurityInfo),
    /// Reply to [`Request::RecoveryKitCreate`] and [`Request::RecoveryKitReissue`].
    RecoveryKit(RecoveryKit),
    /// Reply to [`Request::DeviceList`].
    Devices(Vec<DeviceEntry>),
    /// Reply to [`Request::AttachmentBegin`]: the id to send bytes to.
    AttachmentStarted(AttachmentId),
    /// Reply to [`Request::AttachmentFinish`], [`Request::AttachmentGet`] and
    /// [`Request::AttachmentDownload`].
    Attachment(AttachmentInfo),
    /// Reply to [`Request::AttachmentList`].
    Attachments(Vec<AttachmentInfo>),
    /// Reply to [`Request::AttachmentGc`].
    AttachmentGc(GcSummary),
    /// Reply to [`Request::SyncConfigure`] and [`Request::SyncStatus`].
    Sync(SyncInfo),
    /// Reply to the pairing requests.
    Pairing(PairingInfo),
    /// Reply to the recovery requests.
    Recovery(RecoveryInfo),
    /// Reply to [`Request::ExportBegin`].
    ExportPlanned(ExportPlan),
    /// Reply to [`Request::ExportNext`].
    ExportBatch(ExportBatch),
    /// Reply to [`Request::ExportCsv`]: the CSV text.
    Csv(String),
    /// Reply to [`Request::ImportBegin`] and [`Request::ImportBeginSpaces`]: the session id.
    ImportStarted(u32),
    /// Reply to [`Request::ImportPlan`].
    ImportPlanned(ImportPlanInfo),
    /// Reply to [`Request::ImportCommit`].
    ImportProgress(ImportProgress),
    /// Reply to [`Request::PageMoveToSpacePlan`].
    SpaceMovePlanned(SpaceMoveSummary),
    /// Reply to [`Request::PageMoveToSpace`].
    PagesMovedToSpace(SpaceMoveSummary),
    /// Reply to [`Request::AiIndexStatus`].
    AiIndex(crate::ai::AiIndexInfo),
    /// Reply to [`Request::AiIndexNext`].
    AiIndexWork(crate::ai::AiIndexWork),
    /// Reply to [`Request::AiRetrieve`].
    AiPassages(Vec<crate::ai::AiSource>),
    /// Reply to [`Request::McpClientList`].
    McpClients(Vec<crate::ai::McpClientInfo>),
    /// Reply to [`Request::McpClientCreate`].
    McpClientCreated(crate::ai::McpClientCreated),
    /// Reply to [`Request::McpClientGrantsSet`].
    McpClient(crate::ai::McpClientInfo),
    /// Reply to [`Request::McpAuditList`].
    McpAudit(Vec<crate::ai::McpAuditEntry>),
    /// Reply to [`Request::McpCall`].
    McpReply(crate::ai::McpReply),
    /// Reply to [`Request::SpaceSharePlan`].
    SpaceSharePlan(SpaceSharePlan),
    /// Reply to [`Request::SpaceMembers`], [`Request::MemberRoleSet`], [`Request::MemberRemove`],
    /// [`Request::SpaceSettingsSet`] and [`Request::InviteRevoke`].
    Members(MembersInfo),
    /// Reply to [`Request::InviteLinkCreate`].
    InviteLink(InviteLinkInfo),
    /// Reply to the invite session requests ([`Request::InviteCodeStart`],
    /// [`Request::InviteCodeJoin`], [`Request::InviteLinkOpen`], [`Request::InviteConfirm`],
    /// [`Request::InviteCancel`], [`Request::InviteStatus`]).
    Invite(InviteSessionInfo),
    /// Reply to [`Request::ProfileGet`] and [`Request::ProfileSet`].
    Profile(ProfileInfo),
    /// Reply to [`Request::PersonGet`] and [`Request::PersonVerifiedSet`].
    Person(PersonInfo),
    /// Reply to [`Request::CommentList`].
    Comments(Vec<ThreadInfo>),
    /// Reply to [`Request::CommentPost`], [`Request::CommentEdit`], [`Request::CommentDelete`] and
    /// [`Request::CommentResolve`]: the thread after the change.
    Thread(ThreadInfo),
    /// Reply to [`Request::NotificationList`].
    Notifications(Vec<NotificationInfo>),
    /// Reply to [`Request::PeopleMentionable`].
    People(Vec<PersonInfo>),
    /// Reply to [`Request::PresenceList`].
    Presence(Vec<PresenceInfo>),
    /// Reply to [`Request::ViewLinkCreate`].
    ViewLink(ViewLinkInfo),
    /// Reply to [`Request::ViewLinkList`] and [`Request::ViewLinkRevoke`].
    ViewLinks(Vec<ViewLinkInfo>),
    /// Reply to requests that return nothing.
    Ok,
}

/// Which side of a pairing this device is (ADR-0022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum PairingRole {
    /// The fresh installation that joins the account.
    Joiner,
    /// The existing device that admits it.
    Sponsor,
}

/// Where a pairing session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum PairingState {
    /// No session.
    Idle,
    /// The mailbox is being opened (joiner) or the offer fetched (sponsor).
    Starting,
    /// The code is shown; the sponsor has not answered yet.
    Waiting,
    /// The six digits are shown; this side has not confirmed yet.
    Confirming,
    /// This side confirmed; the other side's turn.
    PeerConfirming,
    /// The bundle is on its way.
    Transferring,
    /// Finished; the joiner has its workspace.
    Done,
    /// Failed (see `error`).
    Failed,
    /// Cancelled here or by the other side.
    Cancelled,
}

/// The pairing session as the UI shows it (ADR-0022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PairingInfo {
    /// This side.
    pub role: PairingRole,
    /// Where it stands.
    pub state: PairingState,
    /// The code to type on the sponsor (joiner, once the mailbox is open), three
    /// groups of four separated by spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub code: Option<String>,
    /// The six digits both sides compare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub sas: Option<String>,
    /// The other device's name once it identified itself (sponsor: shown with the
    /// digits; joiner: the sponsor's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub peer_name: Option<String>,
    /// The server the session goes through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub server_url: Option<String>,
    /// Why it failed, safe to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
    /// When the mailbox expires, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub expires_at: Option<i64>,
}

/// Where a recovery through the kit stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum RecoveryState {
    /// No recovery in progress.
    Idle,
    /// Fetching the recovery blob and the device log from the server.
    Fetching,
    /// Publishing this device's log entry and installing the account.
    Joining,
    /// Finished; the workspace exists and sync starts.
    Done,
    /// Failed (see `error`).
    Failed,
}

/// The recovery as the UI shows it (ADR-0022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RecoveryInfo {
    /// Where it stands.
    pub state: RecoveryState,
    /// The server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub server_url: Option<String>,
    /// Why it failed, safe to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
}

/// Where sync stands on this device (ADR-0021).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SyncInfo {
    /// A server is configured.
    pub configured: bool,
    /// The server, when configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub server_url: Option<String>,
    /// What the driver is doing.
    pub state: SyncState,
    /// Local updates not yet uploaded (documents with pending changes plus keyring
    /// items).
    pub pending_entries: u32,
    /// Document items received that wait for something this device lacks (a key, a
    /// structure document, or the history a snapshot brings); the driver fetches the
    /// snapshots again while there are any. Keyring entries that wait for an earlier
    /// root key are not counted: no snapshot brings that.
    pub parked_items: u32,
    /// Position in the account's feed this device has applied.
    pub remote_cursor: u64,
    /// The server's storage epoch, hex, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub server_epoch: Option<String>,
    /// Last successful push, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_push_at: Option<i64>,
    /// Last successful pull, Unix ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_pull_at: Option<i64>,
    /// The last failure, safe to show; never contains secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_error: Option<String>,
    /// The server's history does not match this device's: uploads are paused.
    pub fork_detected: bool,
    /// Other devices whose heads this device has seen.
    pub devices_seen: u32,
    /// The server does not know the account and no invite token was given: the
    /// next `SyncConfigure` should carry one.
    #[serde(default)]
    pub needs_invite_token: bool,
    /// Chunk transfers waiting (ADR-0023).
    #[serde(default)]
    pub transfers: TransferInfo,
    /// Where each feed stands (M13): the personal feed first, then one per shared space; absent
    /// when sync is not configured. The fields above stay the account's personal feed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<FeedSyncInfo>>"))]
    pub feeds: Vec<FeedSyncInfo>,
}

/// What the sync driver is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SyncState {
    /// No server configured.
    Disabled,
    /// Nothing to do.
    Idle,
    /// Uploading entries or snapshots.
    Pushing,
    /// Downloading and applying entries.
    Pulling,
    /// The server cannot be reached; retrying with backoff.
    Offline,
    /// The last cycle failed for another reason (see `last_error`).
    Error,
    /// A fork or rollback was detected; uploads are paused.
    Forked,
}

/// An attachment as the UI sees it (ADR-0020). Names and MIME types come from the
/// encrypted manifest; the reference count and the unreferenced timestamp from the
/// derived index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    /// Attachment.
    pub attachment_id: AttachmentId,
    /// Owning space.
    pub space_id: SpaceId,
    /// File name.
    pub name: String,
    /// MIME type.
    pub mime: String,
    /// Plaintext size in bytes.
    pub size: u64,
    /// Number of encrypted chunks.
    pub chunk_count: u32,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Blocks referencing it (trashed pages included, purged pages excluded).
    pub references: u32,
    /// Since when nothing references it, Unix ms; absent while referenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub unreferenced_since: Option<i64>,
    /// Chunks present in this device's blob store, `0..=chunk_count` (ADR-0023).
    #[serde(default)]
    pub local_chunks: u32,
    /// Whether the content is here, on its way, or not asked for.
    #[serde(default)]
    pub availability: AttachmentAvailability,
}

/// Where an attachment's content stands on this device (ADR-0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum AttachmentAvailability {
    /// Every chunk is here.
    #[default]
    Complete,
    /// Chunks are wanted and the server serves them (or none was asked for yet).
    Downloading,
    /// Chunks are wanted but the server did not hold one at the last attempt: the
    /// device that added the attachment has not finished uploading.
    Waiting,
    /// The download policy is on demand and nothing asked for the content.
    NotDownloaded,
}

/// Chunk transfers waiting on this device (ADR-0023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TransferInfo {
    /// Chunks to upload.
    pub uploads: u32,
    /// Chunks wanted that the server should serve.
    pub downloads: u32,
    /// Chunks wanted that the server did not hold at the last attempt.
    pub waiting: u32,
}

/// What a garbage collection did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct GcSummary {
    /// Attachments purged (past retention).
    pub purged: u32,
    /// Chunks deleted from the blob store (purged attachments plus orphans).
    pub chunks_deleted: u32,
    /// Ciphertext bytes freed.
    pub bytes_freed: u64,
}

/// A half-open byte range `[start, end)` of an attachment's plaintext, for the binary
/// read. `end` is at most the attachment's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ByteRange {
    /// First byte.
    pub start: u64,
    /// One past the last byte.
    pub end: u64,
}

/// Recovery kit, app lock, device and keyring status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SecurityInfo {
    /// Account id (BLAKE3 of the device log's Genesis entry); `None` before the kit
    /// exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub account_id: Option<AccountId>,
    /// Recovery kit state; `None` when no kit was created yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub recovery_kit: Option<RecoveryKitInfo>,
    /// App lock state.
    pub app_lock: AppLockInfo,
    /// This device.
    pub this_device: DeviceId,
    /// Devices in the log (including revoked ones); 0 before the kit exists.
    pub devices: u32,
    /// Keyring counts.
    pub keyring: KeyringInfo,
    /// Root key generation (0 at the first kit; +1 per rotation).
    #[serde(default)]
    pub root_generation: u32,
    /// Another device rotated the root key and the app lock here needs the passphrase
    /// to store the new one (`RotationApply`).
    #[serde(default)]
    pub rotation_pending: bool,
    /// This device was revoked from the account: sync is off, the data stays.
    #[serde(default)]
    pub this_device_revoked: bool,
    /// The recovery blob is on the configured server (kit recovery works from it).
    #[serde(default)]
    pub recovery_published: bool,
}

/// When the kit was created, verified and last re-issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RecoveryKitInfo {
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms of the last successful `RecoveryKitVerify`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub verified_at: Option<i64>,
    /// Unix ms of the last re-issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub reissued_at: Option<i64>,
    /// The kit was typed on this device (recovery or restore) or an interrupted rotation
    /// left it behind: re-issuing it is advised.
    #[serde(default)]
    pub stale: bool,
}

/// App lock state and the Argon2id parameters in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AppLockInfo {
    /// Whether the root key is wrapped under a passphrase.
    pub enabled: bool,
    /// Argon2id memory, KiB.
    pub memory_kib: u32,
    /// Argon2id iterations.
    pub iterations: u32,
    /// Argon2id parallelism.
    pub parallelism: u32,
}

/// Keyring counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct KeyringInfo {
    /// Space key records.
    pub spaces: u32,
    /// Document key records.
    pub documents: u32,
    /// Attachment key records.
    pub attachments: u32,
    /// Tombstones.
    pub tombstones: u32,
}

/// A freshly issued recovery kit. Shown once; the core keeps no copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RecoveryKit {
    /// The 24 words, in order.
    pub words: Vec<String>,
    /// The same key as `xxxx-xxxx-…-xxxx-cc`.
    pub code: String,
    /// Account id.
    pub account_id: AccountId,
    /// Unix ms.
    pub created_at: i64,
}

/// A device as the device log describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    /// Device.
    pub device_id: DeviceId,
    /// Name (this device's current name; the log's name for others).
    pub name: String,
    /// When it joined, Unix ms.
    pub added_at: i64,
    /// Joined through the recovery kit.
    pub added_via_kit: bool,
    /// When it was revoked, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub revoked_at: Option<i64>,
    /// The device this core runs on.
    pub is_this_device: bool,
    /// Ed25519 public key, lowercase hex.
    pub signing_key: String,
    /// The device that added it (`None` for the first device or a kit join).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub added_by: Option<DeviceId>,
    /// The device that revoked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub revoked_by: Option<DeviceId>,
}

/// What a backup export or import did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    /// File size.
    pub bytes: u64,
    /// Documents included.
    pub documents: u32,
    /// Settings included.
    pub settings: u32,
    /// Backup creation time, Unix ms.
    pub created_at: i64,
    /// Account the backup belongs to.
    pub account_id: AccountId,
    /// Attachment chunks included (backup format 2; 0 for older files).
    #[serde(default)]
    pub chunks: u32,
    /// Ciphertext bytes of those chunks.
    #[serde(default)]
    pub chunk_bytes: u64,
}

/// What [`Request::Search`] looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SearchScope {
    /// Page titles and block text.
    #[default]
    All,
    /// Page titles only.
    Titles,
}

/// A range of UTF-16 code units inside a string (what JavaScript indexes by).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextRange {
    /// First code unit.
    pub start: u32,
    /// One past the last code unit.
    pub end: u32,
}

/// One search hit: a page title (`block_id` absent), one block of a page, or a property of the page
/// (`block_id` absent, the property's text as the snippet). A page whose words are spread over several
/// rows has one hit, on the row that holds the most of them, and a fuzzy title hit (ADR-0040) is a title
/// hit whose highlights mark the letters or the word start that matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Page.
    pub page_id: PageId,
    /// The page's space.
    pub space_id: SpaceId,
    /// Page title (may be empty).
    pub title: String,
    /// Page icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub icon: Option<String>,
    /// The matching block; absent when the title matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub block_id: Option<BlockId>,
    /// The matching comment (M13 slice 2, ADR-0048): set when the words are in a comment of the page,
    /// which the UI labels "in comment". `block_id` is then absent and `snippet` is the comment's
    /// text; the hit is on the page, whose title and icon it carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub comment_id: Option<CommentId>,
    /// Text around the match (plain text, may be truncated with an ellipsis).
    pub snippet: String,
    /// Where the matches are inside `snippet`.
    pub highlights: Vec<TextRange>,
}

/// A page that references another page from one of its blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct Backlink {
    /// The referencing page.
    pub page_id: PageId,
    /// Its space.
    pub space_id: SpaceId,
    /// Its title.
    pub title: String,
    /// Its icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub icon: Option<String>,
    /// The block that holds the reference.
    pub block_id: BlockId,
    /// Plain text of that block, at most 200 characters.
    pub text: String,
}

/// A reminder of a date mention that is due (ADR-0024): the answer of
/// [`Request::RemindersDue`] and the payload of [`Event::ReminderDue`].
///
/// `fire_at` is on the device's wall clock, in seconds since 1970-01-01T00:00:00 as the
/// local clock reads it (see [`Request::RemindersDue`]); `date` and `time` are the
/// mention's own floating local date and time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ReminderDue {
    /// The page that holds the mention.
    pub page_id: PageId,
    /// That page's space (to open the page from anywhere).
    pub space_id: SpaceId,
    /// The block that holds the mention: where the page scrolls to.
    pub block_id: BlockId,
    /// The mention itself (the `id` of the `dateMention` node).
    pub node_id: BlockId,
    /// The page's title (may be empty).
    pub page_title: String,
    /// Plain text of the block, at most 200 characters (the date reads as its ISO text in
    /// it, as in search and backlinks).
    pub text: String,
    /// The mention's day, `YYYY-MM-DD`.
    pub date: String,
    /// The mention's time of day, `HH:mm`; absent for a mention of a whole day (its moment
    /// is then 09:00).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub time: Option<String>,
    /// Minutes before the moment that the reminder fires (0 = at the time).
    pub reminder_minutes: u32,
    /// When it fires: the moment minus `reminder_minutes`, on the device's wall clock.
    pub fire_at: i64,
}

/// A page's content in the portable format (`docs/development/milestone-03.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageContent {
    /// Page.
    pub page_id: PageId,
    /// ProseMirror JSON (`{ "type": "doc", "content": [...] }`).
    pub content: Json,
}

/// The versions of a page (ADR-0057), newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageHistory {
    /// Page.
    pub page_id: PageId,
    /// The versions, newest first.
    pub versions: Vec<PageVersion>,
    /// Older versions exist beyond this answer (ask again with `before` = the last id).
    pub more: bool,
}

/// One version of a page: the state of its content at the end of an editing session (ADR-0057).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageVersion {
    /// Opaque id of the document version (stable across devices).
    pub id: String,
    /// What made it.
    pub kind: PageVersionKind,
    /// When this device saw the session begin, Unix ms; absent for an undated version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub started_at: Option<i64>,
    /// When this device saw the session's last change, Unix ms; absent for an undated version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ended_at: Option<i64>,
    /// Who made it, as far as this device knows; absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub author: Option<VersionAuthor>,
    /// The page's title at the end of the session, when this device recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub title: Option<String>,
    /// This version is the page's content as it is now.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
    pub current: bool,
}

/// What made a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PageVersionKind {
    /// Editing (in the editor, or by the core: an import, a template, MCP).
    Edit,
    /// A `PageHistoryRestore`.
    Restore,
}

/// Who made a version, as far as the device knows: its own device (and account), or the member
/// whose sync entry brought the changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct VersionAuthor {
    /// The device, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub device_id: Option<DeviceId>,
    /// The account, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub account_id: Option<AccountId>,
    /// This device made it.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
    pub this_device: bool,
}

/// A page's content at a version (ADR-0057).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageVersionContent {
    /// Page.
    pub page_id: PageId,
    /// The version.
    pub version: String,
    /// ProseMirror JSON (`{ "type": "doc", "content": [...] }`) at that version.
    pub content: Json,
    /// The title at that version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub title: Option<String>,
}

/// The first lines of a page's text (ADR-0058): what a feed card shows under the title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageExcerpt {
    /// The page.
    pub page_id: PageId,
    /// At most 280 characters, blocks joined with a line break, ending with `…` when cut.
    pub text: String,
}

/// What [`Request::PageBulk`] does to its pages (ADR-0058).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all_fields = "camelCase")]
pub enum BulkAction {
    /// Sets (or, with `None`, clears) one property value on every page, as `PagePropertySet`.
    SetValue {
        /// Property.
        property_id: PropertyId,
        /// New value, or `None` to clear.
        value: Option<PropertyValue>,
    },
    /// Sets one value per page (the same order as `page_ids`): the undo of a `SetValue`.
    SetValues {
        /// Property.
        property_id: PropertyId,
        /// One value or `None` per page.
        values: Vec<Option<PropertyValue>>,
    },
    /// Moves the pages to the end of a parent's children, in the given order.
    Move {
        /// The new parent, or `None` for the top level of the space.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        parent_id: Option<PageId>,
    },
    /// Duplicates each page with its sub-pages (`PageDuplicate`), each copy right after its page.
    Duplicate,
    /// Moves each page and its subtree to the trash.
    Trash,
}

/// The first image of a page (ADR-0054): what a gallery card previews when the page has no cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageFirstImage {
    /// The page.
    pub page_id: PageId,
    /// The attachment of the first image in the page's content, in document order.
    pub attachment_id: AttachmentId,
}

/// Who produced a change to a page's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ChangeSource {
    /// The editor replica of this device (the UI that sent it already has it).
    Editor,
    /// The core itself (semantic operations, repairs).
    Core,
    /// Another device, through sync.
    Remote,
}

/// A space as listed in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SpaceInfo {
    /// Space id.
    pub id: SpaceId,
    /// Display name.
    pub name: String,
    /// Optional emoji/icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub icon: Option<String>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Number of live (not trashed) pages.
    pub page_count: u32,
    /// Personal, shared, or an archive (M13); absent for a personal space.
    #[serde(default, skip_serializing_if = "SpaceShareState::is_personal")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<SpaceShareState>"))]
    pub share_state: SpaceShareState,
    /// This account's role in a shared space or an archive; absent for a personal space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub role: Option<MemberRole>,
    /// Why the space is an archive; set when `shareState` is `Archived`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub archive_reason: Option<ArchiveReason>,
    /// Current members of a shared space; absent (0) for a personal space.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<u32>"))]
    pub member_count: u32,
    /// Local AI and MCP may use the space on this account's devices (ADR-0047). Absent means
    /// `true`; a shared space reports `false` until this account opts in.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
    pub ai_enabled: bool,
}

/// `serde(default = ...)` of the flags that are absent when true.
fn default_true() -> bool {
    true
}

/// `skip_serializing_if` of the flags that are absent when true.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

/// `skip_serializing_if` of the counts that are absent when 0.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

/// The typeface of a page's title, properties and text (ADR-0040), as `PageSetAppearance` names it. The
/// UI draws `serif` and `mono` from the system's fonts (a serif stack and the monospace stack of the
/// app); nothing is downloaded. `default` is the app's sans-serif and is no stored value: a page
/// never reports it (`PageInfo::font` is absent), it only clears the choice in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum PageFont {
    /// The default font: clears the choice.
    Default,
    /// A serif face.
    Serif,
    /// The monospace face.
    Mono,
}

impl PageFont {
    /// The name the wire carries (`default`, `serif`, `mono`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PageFont::Default => "default",
            PageFont::Serif => "serif",
            PageFont::Mono => "mono",
        }
    }

    /// What the document stores for this choice: the name, or `None` for the default (no value).
    #[must_use]
    pub fn stored(self) -> Option<&'static str> {
        match self {
            PageFont::Default => None,
            other => Some(other.as_str()),
        }
    }

    /// The font a stored name stands for. Only `serif` and `mono` are stored; any other text (a font
    /// a newer app wrote, or `default`) reads as no choice.
    #[must_use]
    pub fn from_stored(name: &str) -> Option<PageFont> {
        match name {
            "serif" => Some(PageFont::Serif),
            "mono" => Some(PageFont::Mono),
            _ => None,
        }
    }
}

/// `skip_serializing_if` of the flags that are absent when false.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Page metadata (no content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    /// Page id.
    pub id: PageId,
    /// Owning space.
    pub space_id: SpaceId,
    /// Parent page; `None` at the space root and for a top-level trashed page.
    /// Descendants of a trashed page keep their parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub parent_id: Option<PageId>,
    /// Title; may be empty.
    pub title: String,
    /// Optional emoji/icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub icon: Option<String>,
    /// The page's cover banner (ADR-0040): `attachment:<id>`, `color:<palette name>` or
    /// `gradient:<name>`; absent when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cover: Option<String>,
    /// Where the cover image sits in the banner, 0 (top) to 100 (bottom) percent; absent means 50
    /// (and is always absent without a cover).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cover_y: Option<u8>,
    /// The page's font (ADR-0040): `serif` or `mono`; absent for the default font (never `default`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub font: Option<PageFont>,
    /// Smaller text on the page (ADR-0040); absent when the text has its normal size.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
    pub small_text: bool,
    /// The page opens read-only in the UI (ADR-0040); absent when it is not locked. A guard
    /// against accidental edits, not a permission.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
    pub locked: bool,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Last metadata change, Unix ms.
    pub updated_at: i64,
    /// Set while the page (or an ancestor) is in the trash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub trashed_at: Option<i64>,
    /// The device that created the page, from the node meta (ADR-0026, amendment of 2026-10-08;
    /// ADR-0040); absent for pages made before that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub created_by: Option<DeviceId>,
    /// The device that last edited the page, written with `updated_at`; absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub edited_by: Option<DeviceId>,
    /// The page's property values in schema order (Tags first); absent when there are none
    /// (ADR-0026). Unset properties, values of deleted properties and options that are no longer
    /// listed are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(
        feature = "ts",
        ts(optional, as = "Option<Vec<(PropertyId, PropertyValue)>>")
    )]
    pub properties: Vec<(PropertyId, PropertyValue)>,
    /// The page is a template (ADR-0056); absent when it is not. Its sub-pages are inside the
    /// template without carrying the flag themselves.
    #[serde(default, skip_serializing_if = "is_false")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
    pub template: bool,
    /// The placeholders of a template page or a page inside one (ADR-0056), in schema order;
    /// absent when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(
        feature = "ts",
        ts(optional, as = "Option<Vec<(PropertyId, TemplatePlaceholder)>>")
    )]
    pub template_placeholders: Vec<(PropertyId, TemplatePlaceholder)>,
}

/// A value a template fills in when a page is made from it (ADR-0056).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum TemplatePlaceholder {
    /// The local day of the creation (a `date` property, a date mention).
    Today,
    /// The local day and time of the creation (a `date` property, a date mention).
    Now,
    /// The account that makes the page (a `person` property, a person mention).
    Me,
}

impl TemplatePlaceholder {
    /// The name stored in documents (`today`, `now`, `me`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            TemplatePlaceholder::Today => "today",
            TemplatePlaceholder::Now => "now",
            TemplatePlaceholder::Me => "me",
        }
    }

    /// The placeholder with this name; `None` for a name this version does not know.
    #[must_use]
    pub fn parse(name: &str) -> Option<TemplatePlaceholder> {
        [
            TemplatePlaceholder::Today,
            TemplatePlaceholder::Now,
            TemplatePlaceholder::Me,
        ]
        .into_iter()
        .find(|placeholder| placeholder.as_str() == name)
    }
}

/// A page with its live children, in sibling order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PageNode {
    /// The page.
    pub page: PageInfo,
    /// Children in order.
    pub children: Vec<PageNode>,
}

/// A top-level trashed page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TrashedPage {
    /// The page (its `trashed_at` is set).
    pub page: PageInfo,
    /// Where it was before trashing, if that page still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub original_parent_id: Option<PageId>,
    /// Number of pages trashed along with it (its subtree).
    pub descendant_count: u32,
}

/// What a move to another space does or did (ADR-0038): the answer of
/// [`Request::PageMoveToSpacePlan`] (nothing written yet) and of [`Request::PageMoveToSpace`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SpaceMoveSummary {
    /// The pages that move, in pre-order (the moved page first).
    pub page_ids: Vec<PageId>,
    /// Attachments that move with them (used by no page that stays).
    pub attachments_moved: u32,
    /// Attachments the moved pages use that stay in the source space because pages there use
    /// them too (still readable from the moved pages).
    pub attachments_left: u32,
    /// Properties created in the target space's schema.
    pub properties_created: u32,
    /// Property values that cannot follow (the target's property limit, a kind that cannot take
    /// them).
    pub values_dropped: u32,
    /// Relation entries removed: values of moved pages pointing at pages that stay, and values
    /// of pages that stay pointing at moved pages.
    pub relation_links_removed: u32,
}

/// Whether structural undo/redo is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct UndoState {
    /// `StructureUndo` would do something.
    pub can_undo: bool,
    /// `StructureRedo` would do something.
    pub can_redo: bool,
}

/// Everything the sidebar needs for one space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TreeView {
    /// Space.
    pub space_id: SpaceId,
    /// Live root pages with their subtrees, in order.
    pub roots: Vec<PageNode>,
    /// Top-level trashed pages, most recently trashed first.
    pub trashed: Vec<TrashedPage>,
    /// Undo/redo availability for this space.
    pub undo: UndoState,
    /// The space's live favourite pages, in order (ADR-0029); absent when there are none. A
    /// favourite whose page is in the trash is hidden here (it returns with a restore).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional, as = "Option<Vec<PageId>>"))]
    pub favorites: Vec<PageId>,
}

/// Health and configuration summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// Whether onboarding has created the workspace.
    pub workspace_initialized: bool,
    /// Current SQLite schema version (`PRAGMA user_version`).
    pub schema_version: u32,
    /// Whether the database reports an active cipher (SQLCipher or sqlite3mc).
    pub db_encrypted: bool,
    /// Human-readable cipher description, e.g. `"SQLCipher 4.14.0"`.
    pub db_cipher: String,
    /// This device's id.
    pub device_id: DeviceId,
    /// Where the data lives, for display.
    pub data_dir: String,
    /// Core crate version.
    pub core_version: String,
    /// API contract version; shells compare it to the one they were built for.
    pub api_version: u32,
    /// Platform name reported by the shell, e.g. `"windows"`.
    pub platform: String,
}

/// The workspace record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Workspace id.
    pub id: WorkspaceId,
    /// Display name.
    pub name: String,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
}

/// This device as the core sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Device id (random, created on first launch).
    pub device_id: DeviceId,
    /// User-visible name.
    pub name: String,
    /// Platform name reported by the shell.
    pub platform: String,
    /// First-launch time, Unix milliseconds.
    pub created_at: i64,
}

/// Notifications emitted by the core; shells drain them after each dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "data", rename_all_fields = "camelCase")]
pub enum Event {
    /// Onboarding completed.
    WorkspaceCreated {
        /// New workspace id.
        id: WorkspaceId,
    },
    /// A setting changed (value not included; re-read if needed).
    SettingChanged {
        /// Setting key.
        key: String,
    },
    /// This device was renamed.
    DeviceRenamed {
        /// New name.
        name: String,
    },
    /// The list or order of spaces changed (re-fetch `SpaceList`).
    SpacesChanged,
    /// A space's tree, trash or metadata changed (re-fetch `TreeGet`).
    StructureChanged {
        /// Space.
        space_id: SpaceId,
    },
    /// Pages moved from one space to another, by this device or another one (ADR-0038): tabs,
    /// recent pages and the selection that name them follow to `to_space_id`.
    PagesMovedSpace {
        /// The space they left.
        from_space_id: SpaceId,
        /// The space they are in now.
        to_space_id: SpaceId,
        /// The pages, the moved page first.
        page_ids: Vec<PageId>,
    },
    /// A page's content changed. An open editor pulls the delta with the binary
    /// `page_doc_export` unless it produced the change itself (`source: Editor`).
    PageContentChanged {
        /// Page.
        page_id: PageId,
        /// Who changed it.
        source: ChangeSource,
    },
    /// The property schema of a space changed (a property or an option was created, renamed,
    /// recoloured, moved or deleted, here or through sync): re-fetch `PropertySchemaGet`.
    /// `StructureChanged` follows for the same space, because page values may have changed
    /// with it (`PageInfo::properties`); a plain value change emits `StructureChanged` only.
    PropertySchemaChanged {
        /// Space.
        space_id: SpaceId,
    },
    /// The set of pages referencing this page changed (re-fetch `BacklinksGet`).
    BacklinksChanged {
        /// The referenced page.
        page_id: PageId,
    },
    /// A reminder of a date mention is due (ADR-0024): emitted once per reminder by the
    /// scheduler tick (`Core::reminders_tick`) and never while the app is locked. The
    /// desktop shell shows an OS notification, the UI an in-app toast.
    ReminderDue(ReminderDue),
    /// Recovery kit, app lock, devices or keyring changed (re-fetch `SecurityStatus`).
    SecurityChanged,
    /// A space's attachments changed (upload finished or aborted, delete, garbage
    /// collection, a page re-index changed references, or a chunk arrived or left
    /// through sync): re-fetch `AttachmentList` or the attachment's `AttachmentGet`.
    AttachmentsChanged {
        /// Space.
        space_id: SpaceId,
    },
    /// The sync status changed (re-fetch `SyncStatus`).
    SyncChanged,
    /// Local changes are waiting; the driver should run a cycle (debounced).
    SyncRequested,
    /// The pairing session changed (re-fetch `PairingStatus`).
    PairingChanged,
    /// A pairing session started: the shell runs its network task until it ends.
    PairingRequested,
    /// The recovery changed (re-fetch `RecoveryStatus`).
    RecoveryChanged,
    /// A recovery started: the shell runs its network task until it ends.
    RecoveryRequested,
    /// An export advanced (`ExportNext`): `done` of `total` pages rendered.
    ExportProgress {
        /// Session.
        export_id: u32,
        /// Pages rendered so far.
        done: u32,
        /// Pages in the export.
        total: u32,
    },
    /// An import advanced (`ImportCommit`): `done` of `total` pages written.
    ImportProgress {
        /// Session.
        import_id: u32,
        /// Pages written so far.
        done: u32,
        /// Pages to write.
        total: u32,
    },
    /// The device log changed through sync: devices joined, were revoked, or the root
    /// key rotated. Shown once; the device list is re-fetched.
    DevicesChanged {
        /// Devices that joined.
        added: Vec<DeviceEntry>,
        /// Devices that were revoked.
        revoked: Vec<DeviceId>,
        /// This device was revoked.
        this_device_revoked: bool,
        /// The root key rotated.
        rotated: bool,
    },
    /// The semantic index changed (vectors stored or cleared): re-read `AiIndexStatus`
    /// (throttled; this fires once per indexed page).
    AiIndexChanged,
    /// MCP clients, grants or the audit log changed: re-read what is shown.
    McpChanged,
    /// The membership state of a shared space changed (members, roles, invites, settings, epoch,
    /// disputes, refused counts), here or through sync: re-fetch `SpaceMembers`.
    SpaceMembersChanged {
        /// Space.
        space_id: SpaceId,
    },
    /// This account's role in a shared space changed (a Viewer's editor turns read-only, a banner
    /// shows).
    SpaceRoleChanged {
        /// Space.
        space_id: SpaceId,
        /// The new role.
        role: MemberRole,
        /// The role before.
        previous: MemberRole,
    },
    /// This account was removed from a shared space or an Owner closed it: the space is now an
    /// archive (`SpaceInfo::share_state`). `SpacesChanged` follows.
    SpaceRemoved {
        /// Space.
        space_id: SpaceId,
        /// Removed or closed.
        reason: ArchiveReason,
    },
    /// A co-member's account added a device with its recovery kit (`DeviceAdded` with `via_kit`):
    /// shown once, the safety number does not change.
    MemberDeviceViaKit {
        /// Space.
        space_id: SpaceId,
        /// The member.
        account_id: AccountId,
        /// The member's name.
        name: String,
    },
    /// Profiles or verified people changed: re-read names.
    PeopleChanged,
    /// The invite session changed (re-fetch `InviteStatus`).
    InviteChanged,
    /// An invite session started: the shell runs its network task until it ends.
    InviteRequested,
    /// Fork-out copied the changes this device could not send into pages "Unsent changes - <title>"
    /// of a personal space (this account was demoted or removed while it held unsent changes).
    UnsentChangesCopied {
        /// The shared space the changes were made in.
        space_id: SpaceId,
        /// The personal space that now holds the copies.
        target_space_id: SpaceId,
        /// The new pages.
        page_ids: Vec<PageId>,
    },
    /// A member's device published a snapshot that omits changes it should contain (Owners; the
    /// snapshot was never applied and a full one is republished).
    SnapshotDisputed {
        /// Space.
        space_id: SpaceId,
        /// The page, when the document is a page.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        page_id: Option<PageId>,
        /// The member whose device published it.
        account_id: AccountId,
    },
    /// A page's comments changed (a comment was posted, edited, deleted or resolved here, or arrived
    /// through sync): re-fetch `CommentList` (M13 slice 2).
    CommentsChanged {
        /// Page.
        page_id: PageId,
    },
    /// A remote change concerns this account (a reply, a mention of a comment or of a page, an
    /// assignment): the row just added to the inbox, for an in-app toast and the shell's OS
    /// notification (title the page's title, body a phrase with the actor's name and never the
    /// comment's text). Never raised for this device's own writes (ADR-0048). `NotificationsChanged`
    /// follows.
    NotificationRaised(NotificationInfo),
    /// The inbox changed (a row was added, or rows were marked read): re-read the unread count.
    NotificationsChanged,
    /// Someone joined, left (a timeout, a removal) or changed page in a shared space: re-fetch
    /// `PresenceList` (avatars). Not raised for a cursor move or a new frame from a device already
    /// in the same page and state (M13 slice 3).
    PresenceChanged {
        /// Space.
        space_id: SpaceId,
    },
    /// A space's view links changed (one was created or revoked here or on another device, an upload
    /// moved its status from `Publishing` to `Live`, or one expired): re-fetch `ViewLinkList` (M13
    /// slice 4).
    ViewLinksChanged {
        /// Space.
        space_id: SpaceId,
    },
}

/// Closed set of error categories. Shells branch on `code`, show `message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ErrorCode {
    /// The referenced thing does not exist.
    NotFound,
    /// The request was malformed or violated a rule; `details` may say which field.
    InvalidInput,
    /// Creating something that already exists.
    AlreadyExists,
    /// The database failed (I/O, corruption, constraint).
    Storage,
    /// A cryptographic operation failed (wrong key, tampered data).
    Crypto,
    /// The OS keystore cannot be used on this machine.
    KeystoreUnavailable,
    /// The workspace is locked (app lock); unlock first.
    Locked,
    /// A bug or an unexpected condition.
    Internal,
    /// The account's role in a shared space does not allow the request, or the space is an
    /// archive. `details` is `{ "reason": "role", "required": "owner" | "editor" | "commenter" }`
    /// or `{ "reason": "archived" }` (M13, ADR-0043).
    Forbidden,
}

/// The single error type that crosses the core boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    /// Category.
    pub code: ErrorCode,
    /// Human-readable, safe to show; never contains secrets.
    pub message: String,
    /// Optional structured context (field names, limits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub details: Option<Json>,
}

impl ApiError {
    /// Builds an error.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    /// Attaches structured details.
    #[must_use]
    pub fn with_details(mut self, details: Json) -> Self {
        self.details = Some(details);
        self
    }

    /// `InvalidInput` for a specific field.
    #[must_use]
    pub fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
            .with_details(serde_json::json!({ "field": field }))
    }

    /// `NotFound`.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// `AlreadyExists`.
    #[must_use]
    pub fn already_exists(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::AlreadyExists, message)
    }

    /// `Storage`.
    #[must_use]
    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Storage, message)
    }

    /// `Crypto`.
    #[must_use]
    pub fn crypto(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Crypto, message)
    }

    /// `KeystoreUnavailable`.
    #[must_use]
    pub fn keystore_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::KeystoreUnavailable, message)
    }

    /// `Internal`.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// `Locked`.
    #[must_use]
    pub fn locked() -> Self {
        Self::new(ErrorCode::Locked, "the workspace is locked")
    }

    /// `Forbidden` because the account's role is below `required`
    /// (`details`: `{ "reason": "role", "required": "<role>" }`).
    #[must_use]
    pub fn forbidden_role(required: MemberRole) -> Self {
        Self::new(
            ErrorCode::Forbidden,
            format!("this needs the {} role in the space", required.as_str()),
        )
        .with_details(serde_json::json!({ "reason": "role", "required": required.as_str() }))
    }

    /// `Forbidden` because the space is an archive (`details`: `{ "reason": "archived" }`).
    #[must_use]
    pub fn forbidden_archived() -> Self {
        Self::new(ErrorCode::Forbidden, "this space is a read-only archive")
            .with_details(serde_json::json!({ "reason": "archived" }))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn request_json_shape_is_adjacently_tagged() {
        let req = Request::WorkspaceCreate {
            name: "Home".into(),
            first_space_name: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "WorkspaceCreate", "data": { "name": "Home" } })
        );
        let page_move: Request = serde_json::from_value(serde_json::json!({
            "type": "PageMove",
            "data": { "pageId": "12345678-9abc-4def-8123-456789abcdef", "index": 2 }
        }))
        .unwrap();
        assert!(matches!(
            page_move,
            Request::PageMove {
                parent_id: None,
                index: 2,
                ..
            }
        ));
        let unit = serde_json::to_value(Request::AppStatus).unwrap();
        assert_eq!(unit, serde_json::json!({ "type": "AppStatus" }));
        let back: Request = serde_json::from_value(json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn page_first_images_shape_names_the_space_and_the_pages() {
        let space = "12345678-9abc-4def-8123-456789abcdef";
        let page = "22345678-9abc-4def-8123-456789abcdef";
        let attachment = "32345678-9abc-4def-8123-456789abcdef";
        let request: Request = serde_json::from_value(serde_json::json!({
            "type": "PageFirstImages",
            "data": { "spaceId": space, "pageIds": [page] }
        }))
        .unwrap();
        assert_eq!(request.kind(), "PageFirstImages");
        assert!(
            matches!(&request, Request::PageFirstImages { page_ids, .. } if page_ids.len() == 1)
        );
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({ "type": "PageFirstImages", "data": { "spaceId": space, "pageIds": [page] } })
        );
        let answer: Response = serde_json::from_value(serde_json::json!({
            "type": "PageFirstImages",
            "data": [{ "pageId": page, "attachmentId": attachment }]
        }))
        .unwrap();
        assert!(matches!(&answer, Response::PageFirstImages(images) if images.len() == 1));
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            serde_json::json!({ "type": "PageFirstImages", "data": [{ "pageId": page, "attachmentId": attachment }] })
        );
    }

    #[test]
    fn page_reorder_children_shape_has_an_optional_parent() {
        let space = "12345678-9abc-4def-8123-456789abcdef";
        let a = "22345678-9abc-4def-8123-456789abcdef";
        let b = "32345678-9abc-4def-8123-456789abcdef";
        let top: Request = serde_json::from_value(serde_json::json!({
            "type": "PageReorderChildren",
            "data": { "spaceId": space, "order": [b, a] }
        }))
        .unwrap();
        assert_eq!(top.kind(), "PageReorderChildren");
        assert!(matches!(
            &top,
            Request::PageReorderChildren {
                parent_id: None,
                order,
                ..
            } if order.len() == 2
        ));
        // A top-level reorder omits the parent when it is written again.
        assert_eq!(
            serde_json::to_value(&top).unwrap(),
            serde_json::json!({
                "type": "PageReorderChildren",
                "data": { "spaceId": space, "order": [b, a] }
            })
        );
        let under: Request = serde_json::from_value(serde_json::json!({
            "type": "PageReorderChildren",
            "data": { "spaceId": space, "parentId": a, "order": [] }
        }))
        .unwrap();
        assert!(matches!(
            under,
            Request::PageReorderChildren {
                parent_id: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn search_request_defaults_and_event_source_shapes() {
        let search: Request = serde_json::from_value(serde_json::json!({
            "type": "Search",
            "data": { "query": "notes" }
        }))
        .unwrap();
        assert!(matches!(
            search,
            Request::Search {
                space_id: None,
                scope: None,
                limit: None,
                edited_after: None,
                ..
            }
        ));
        let scoped: Request = serde_json::from_value(serde_json::json!({
            "type": "Search",
            "data": { "query": "n", "scope": "Titles", "limit": 8 }
        }))
        .unwrap();
        assert!(matches!(
            scoped,
            Request::Search {
                scope: Some(SearchScope::Titles),
                limit: Some(8),
                ..
            }
        ));
        // The edited-after filter is camelCase on the wire, in Unix milliseconds, and left out when absent.
        let recent: Request = serde_json::from_value(serde_json::json!({
            "type": "Search",
            "data": { "query": "n", "editedAfter": 1_791_383_400_000_i64 }
        }))
        .unwrap();
        assert!(matches!(
            recent,
            Request::Search {
                edited_after: Some(1_791_383_400_000),
                ..
            }
        ));
        let json = serde_json::to_value(&recent).unwrap();
        assert_eq!(json["data"]["editedAfter"], 1_791_383_400_000_i64);
        let plain = serde_json::to_value(Request::Search {
            query: "n".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap();
        assert_eq!(
            plain,
            serde_json::json!({ "type": "Search", "data": { "query": "n" } })
        );
        let event = Event::PageContentChanged {
            page_id: PageId::from_bytes([7; 16]),
            source: ChangeSource::Editor,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "PageContentChanged");
        assert_eq!(json["data"]["source"], "Editor");
        assert!(json["data"]["pageId"].is_string());
        let hit = SearchHit {
            page_id: PageId::from_bytes([1; 16]),
            space_id: SpaceId::from_bytes([2; 16]),
            title: "T".into(),
            icon: None,
            block_id: None,
            comment_id: None,
            snippet: "x".into(),
            highlights: vec![TextRange { start: 0, end: 1 }],
        };
        let json = serde_json::to_value(&hit).unwrap();
        assert!(json.get("icon").is_none() && json.get("blockId").is_none());
        assert!(json.get("commentId").is_none());
        assert_eq!(json["highlights"][0]["end"], 1);
    }

    #[test]
    fn reminder_request_event_and_response_shapes() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "type": "RemindersDue",
            "data": { "until": 1_791_383_400_i64 }
        }))
        .unwrap();
        assert_eq!(
            request,
            Request::RemindersDue {
                until: 1_791_383_400
            }
        );
        assert_eq!(request.kind(), "RemindersDue");

        let reminder = ReminderDue {
            page_id: PageId::from_bytes([1; 16]),
            space_id: SpaceId::from_bytes([2; 16]),
            block_id: BlockId::from_bytes([3; 16]),
            node_id: BlockId::from_bytes([4; 16]),
            page_title: "Plan".into(),
            text: "Call the dentist".into(),
            date: "2026-10-07".into(),
            time: None,
            reminder_minutes: 15,
            fire_at: 1_791_382_500,
        };
        // The event carries the reminder as its data, in camel case; no time is no key.
        let event = serde_json::to_value(Event::ReminderDue(reminder.clone())).unwrap();
        assert_eq!(event["type"], "ReminderDue");
        assert_eq!(event["data"]["pageTitle"], "Plan");
        assert_eq!(event["data"]["reminderMinutes"], 15);
        assert_eq!(event["data"]["fireAt"], 1_791_382_500_i64);
        assert!(event["data"].get("time").is_none());
        for key in ["pageId", "spaceId", "blockId", "nodeId", "text", "date"] {
            assert!(event["data"].get(key).is_some(), "{key}");
        }
        let back: Event = serde_json::from_value(event).unwrap();
        assert_eq!(back, Event::ReminderDue(reminder.clone()));

        let with_time = ReminderDue {
            time: Some("14:30".into()),
            ..reminder
        };
        let response = Response::Reminders(vec![with_time.clone()]);
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["type"], "Reminders");
        assert_eq!(json["data"][0]["time"], "14:30");
        assert_eq!(
            serde_json::from_value::<Response>(json).unwrap(),
            Response::Reminders(vec![with_time])
        );
    }

    fn option_id(n: u8) -> OptionId {
        OptionId::from_bytes([n; 16])
    }

    #[test]
    fn property_values_are_tagged_by_kind_with_one_example_each() {
        use crate::ids::ChecklistItemId;
        use crate::ids::DeviceId;
        use crate::properties::{ChecklistItem, DateValue, PlaceValue};
        use crate::properties::{RollupFunction, RollupResult, RollupValue};
        let cases = [
            (
                PropertyValue::Relation(vec![
                    PageId::from_bytes([1; 16]),
                    PageId::from_bytes([2; 16]),
                ]),
                serde_json::json!({
                    "kind": "relation",
                    "value": [PageId::from_bytes([1; 16]).to_string(), PageId::from_bytes([2; 16]).to_string()]
                }),
            ),
            (
                PropertyValue::Rollup(RollupValue {
                    function: RollupFunction::Sum,
                    result: RollupResult::Number(12.5),
                }),
                serde_json::json!({ "kind": "rollup", "value": { "function": "sum", "result": { "type": "number", "data": 12.5 } } }),
            ),
            (
                PropertyValue::Rollup(RollupValue {
                    function: RollupFunction::Earliest,
                    result: RollupResult::Date(DateValue {
                        date: "2026-10-07".into(),
                        time: None,
                    }),
                }),
                serde_json::json!({ "kind": "rollup", "value": { "function": "earliest", "result": { "type": "date", "data": { "date": "2026-10-07" } } } }),
            ),
            (
                PropertyValue::Rollup(RollupValue {
                    function: RollupFunction::ShowOriginal,
                    result: RollupResult::Values(vec![
                        PropertyValue::Text("a".into()),
                        PropertyValue::Number(2.0),
                    ]),
                }),
                serde_json::json!({ "kind": "rollup", "value": { "function": "show_original", "result": { "type": "values", "data": [
                    { "kind": "text", "value": "a" }, { "kind": "number", "value": 2.0 } ] } } }),
            ),
            (
                PropertyValue::Rollup(RollupValue {
                    function: RollupFunction::ChecklistProgress,
                    result: RollupResult::Progress { done: 3, total: 7 },
                }),
                serde_json::json!({ "kind": "rollup", "value": { "function": "checklist_progress", "result": { "type": "progress", "data": { "done": 3, "total": 7 } } } }),
            ),
            (
                PropertyValue::Place(PlaceValue {
                    name: "Café de Flore".into(),
                    lat: Some(48.854),
                    lon: Some(2.333),
                }),
                serde_json::json!({ "kind": "place", "value": { "name": "Café de Flore", "lat": 48.854, "lon": 2.333 } }),
            ),
            (
                PropertyValue::Place(PlaceValue {
                    name: "Home".into(),
                    lat: None,
                    lon: None,
                }),
                serde_json::json!({ "kind": "place", "value": { "name": "Home" } }),
            ),
            (
                PropertyValue::CreatedTime(1_760_000_000_000),
                serde_json::json!({ "kind": "created_time", "value": 1_760_000_000_000_i64 }),
            ),
            (
                PropertyValue::LastEditedTime(1_760_000_000_001),
                serde_json::json!({ "kind": "last_edited_time", "value": 1_760_000_000_001_i64 }),
            ),
            (
                PropertyValue::CreatedBy(DeviceId::from_bytes([9; 16])),
                serde_json::json!({ "kind": "created_by", "value": DeviceId::from_bytes([9; 16]).to_string() }),
            ),
            (
                PropertyValue::LastEditedBy(DeviceId::from_bytes([10; 16])),
                serde_json::json!({ "kind": "last_edited_by", "value": DeviceId::from_bytes([10; 16]).to_string() }),
            ),
            (
                PropertyValue::Text("Quarterly plan".into()),
                serde_json::json!({ "kind": "text", "value": "Quarterly plan" }),
            ),
            (
                PropertyValue::Number(3.5),
                serde_json::json!({ "kind": "number", "value": 3.5 }),
            ),
            (
                PropertyValue::Checkbox(true),
                serde_json::json!({ "kind": "checkbox", "value": true }),
            ),
            (
                PropertyValue::Date(DateValue {
                    date: "2026-10-07".into(),
                    time: Some("14:30".into()),
                }),
                serde_json::json!({ "kind": "date", "value": { "date": "2026-10-07", "time": "14:30" } }),
            ),
            (
                PropertyValue::Date(DateValue {
                    date: "2026-10-07".into(),
                    time: None,
                }),
                serde_json::json!({ "kind": "date", "value": { "date": "2026-10-07" } }),
            ),
            (
                PropertyValue::Select(option_id(1)),
                serde_json::json!({ "kind": "select", "value": option_id(1).to_string() }),
            ),
            (
                PropertyValue::Status(option_id(2)),
                serde_json::json!({ "kind": "status", "value": option_id(2).to_string() }),
            ),
            (
                PropertyValue::MultiSelect(vec![option_id(3), option_id(4)]),
                serde_json::json!({
                    "kind": "multi_select",
                    "value": [option_id(3).to_string(), option_id(4).to_string()]
                }),
            ),
            (
                PropertyValue::Url("https://example.com".into()),
                serde_json::json!({ "kind": "url", "value": "https://example.com" }),
            ),
            (
                PropertyValue::Email("ada@example.com".into()),
                serde_json::json!({ "kind": "email", "value": "ada@example.com" }),
            ),
            (
                PropertyValue::Phone("+1 555 0100".into()),
                serde_json::json!({ "kind": "phone", "value": "+1 555 0100" }),
            ),
            (
                PropertyValue::Files(vec![
                    AttachmentId::from_bytes([5; 16]),
                    AttachmentId::from_bytes([6; 16]),
                ]),
                serde_json::json!({
                    "kind": "files",
                    "value": [
                        AttachmentId::from_bytes([5; 16]).to_string(),
                        AttachmentId::from_bytes([6; 16]).to_string()
                    ]
                }),
            ),
            (
                PropertyValue::Checklist(vec![
                    ChecklistItem {
                        id: ChecklistItemId::from_bytes([7; 16]),
                        text: "Pack the charger".into(),
                        done: true,
                    },
                    ChecklistItem {
                        id: ChecklistItemId::from_bytes([8; 16]),
                        text: String::new(),
                        done: false,
                    },
                ]),
                serde_json::json!({
                    "kind": "checklist",
                    "value": [
                        { "id": ChecklistItemId::from_bytes([7; 16]).to_string(), "text": "Pack the charger", "done": true },
                        { "id": ChecklistItemId::from_bytes([8; 16]).to_string(), "text": "", "done": false }
                    ]
                }),
            ),
        ];
        for (value, json) in cases {
            assert_eq!(serde_json::to_value(&value).unwrap(), json);
            assert_eq!(
                serde_json::from_value::<PropertyValue>(json.clone()).unwrap(),
                value
            );
            assert_eq!(
                serde_json::to_value(value.kind()).unwrap(),
                json["kind"],
                "the value's kind is spelled like the property kind"
            );
        }
        // A kind this version does not know, or a value of the wrong shape, does not parse.
        assert!(
            serde_json::from_value::<PropertyValue>(
                serde_json::json!({ "kind": "formula", "value": "x" })
            )
            .is_err()
        );
        for bad in [
            serde_json::json!({ "kind": "files", "value": ["not an id"] }),
            serde_json::json!({ "kind": "files", "value": "x" }),
            serde_json::json!({ "kind": "relation", "value": ["not an id"] }),
            serde_json::json!({ "kind": "rollup", "value": { "function": "nope", "result": { "type": "number", "data": 1 } } }),
            serde_json::json!({ "kind": "checklist", "value": [{ "id": "nope", "text": "a", "done": false }] }),
            serde_json::json!({
                "kind": "checklist",
                "value": [{ "id": ChecklistItemId::from_bytes([7; 16]).to_string(), "text": "a" }]
            }),
        ] {
            assert!(serde_json::from_value::<PropertyValue>(bad).is_err());
        }
        // The text of an id is read in any notation and written back canonical.
        let loose = serde_json::json!({
            "kind": "files",
            "value": ["05050505050505050505050505050505".to_uppercase()]
        });
        let parsed: PropertyValue = serde_json::from_value(loose).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["value"][0],
            AttachmentId::from_bytes([5; 16]).to_string()
        );
        assert!(
            serde_json::from_value::<PropertyValue>(
                serde_json::json!({ "kind": "number", "value": "3" })
            )
            .is_err()
        );
    }

    #[test]
    fn property_requests_responses_and_filters_roundtrip() {
        use crate::properties::TAGS_PROPERTY_ID;
        assert_eq!(
            TAGS_PROPERTY_ID.to_string(),
            "0000000a-0000-4000-8000-000000000001"
        );
        let create: Request = serde_json::from_value(serde_json::json!({
            "type": "PropertyCreate",
            "data": {
                "spaceId": SpaceId::from_bytes([1; 16]).to_string(),
                "name": "Due",
                "kind": "multi_select"
            }
        }))
        .unwrap();
        assert_eq!(
            create,
            Request::PropertyCreate {
                space_id: SpaceId::from_bytes([1; 16]),
                name: "Due".into(),
                kind: PropertyKind::MultiSelect,
                config: None,
            }
        );
        assert_eq!(create.kind(), "PropertyCreate");
        let relation = Request::PropertyCreate {
            space_id: SpaceId::from_bytes([1; 16]),
            name: "Related".into(),
            kind: PropertyKind::Relation,
            config: Some(crate::properties::PropertyConfig::Relation {
                two_way: true,
                paired_property_id: None,
            }),
        };
        let json = serde_json::to_value(&relation).unwrap();
        assert_eq!(
            json["data"]["config"],
            serde_json::json!({ "kind": "relation", "twoWay": true })
        );
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), relation);
        let rollup = crate::properties::PropertyConfig::Rollup {
            relation_property_id: TAGS_PROPERTY_ID,
            target: crate::properties::RollupTarget::Title,
            function: crate::properties::RollupFunction::CountAll,
        };
        let json = serde_json::to_value(&rollup).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "rollup",
                "relationPropertyId": TAGS_PROPERTY_ID.to_string(),
                "target": "title",
                "function": "count_all"
            })
        );
        assert_eq!(
            serde_json::from_value::<crate::properties::PropertyConfig>(json).unwrap(),
            rollup
        );
        let target = crate::properties::RollupTarget::Property(PropertyId::from_bytes([3; 16]));
        assert_eq!(
            serde_json::to_value(target).unwrap(),
            serde_json::json!(PropertyId::from_bytes([3; 16]).to_string())
        );
        assert!(
            serde_json::from_value::<crate::properties::RollupTarget>(serde_json::json!("nope"))
                .is_err()
        );
        let set_config = Request::PropertyConfigSet {
            space_id: SpaceId::from_bytes([1; 16]),
            property_id: TAGS_PROPERTY_ID,
            config: rollup,
        };
        assert_eq!(set_config.kind(), "PropertyConfigSet");
        let json = serde_json::to_value(&set_config).unwrap();
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), set_config);
        let add = Request::PropertyOptionAdd {
            space_id: SpaceId::from_bytes([1; 16]),
            property_id: TAGS_PROPERTY_ID,
            name: "urgent".into(),
            color: OptionColor::Red,
            group: None,
        };
        let json = serde_json::to_value(&add).unwrap();
        assert_eq!(json["data"]["color"], "red");
        assert!(json["data"].get("group").is_none());
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), add);
        assert_eq!(
            serde_json::to_value(StatusGroup::InProgress).unwrap(),
            "in_progress"
        );

        let set = Request::PagePropertySet {
            page_id: PageId::from_bytes([2; 16]),
            property_id: TAGS_PROPERTY_ID,
            value: None,
        };
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["data"]["value"], serde_json::Value::Null);
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), set);

        let filters = [
            (
                PropertyFilter::HasValue,
                serde_json::json!({ "type": "HasValue" }),
            ),
            (
                PropertyFilter::Equals(PropertyValue::Checkbox(false)),
                serde_json::json!({ "type": "Equals", "data": { "kind": "checkbox", "value": false } }),
            ),
            (
                PropertyFilter::ContainsOption(option_id(9)),
                serde_json::json!({ "type": "ContainsOption", "data": option_id(9).to_string() }),
            ),
            (
                PropertyFilter::TextContains("abc".into()),
                serde_json::json!({ "type": "TextContains", "data": "abc" }),
            ),
            (
                PropertyFilter::DateBetween {
                    from: Some("2026-10-01".into()),
                    to: None,
                },
                serde_json::json!({ "type": "DateBetween", "data": { "from": "2026-10-01" } }),
            ),
            (
                PropertyFilter::Checked(true),
                serde_json::json!({ "type": "Checked", "data": true }),
            ),
            (
                PropertyFilter::ChecklistComplete(false),
                serde_json::json!({ "type": "ChecklistComplete", "data": false }),
            ),
            (
                PropertyFilter::RelatesTo(PageId::from_bytes([4; 16])),
                serde_json::json!({ "type": "RelatesTo", "data": PageId::from_bytes([4; 16]).to_string() }),
            ),
        ];
        for (filter, json) in filters {
            assert_eq!(serde_json::to_value(&filter).unwrap(), json);
            assert_eq!(
                serde_json::from_value::<PropertyFilter>(json).unwrap(),
                filter
            );
        }

        let page = PageInfo {
            id: PageId::from_bytes([2; 16]),
            space_id: SpaceId::from_bytes([1; 16]),
            parent_id: None,
            title: "T".into(),
            icon: None,
            cover: None,
            cover_y: None,
            font: None,
            small_text: false,
            locked: false,
            created_at: 1,
            updated_at: 2,
            trashed_at: None,
            created_by: None,
            edited_by: None,
            properties: Vec::new(),
            template: false,
            template_placeholders: Vec::new(),
        };
        let json = serde_json::to_value(&page).unwrap();
        assert!(json.get("properties").is_none(), "no values, no key");
        let with = PageInfo {
            properties: vec![(
                TAGS_PROPERTY_ID,
                PropertyValue::MultiSelect(vec![option_id(1)]),
            )],
            ..page
        };
        let json = serde_json::to_value(&with).unwrap();
        assert_eq!(json["properties"][0][0], TAGS_PROPERTY_ID.to_string());
        assert_eq!(json["properties"][0][1]["kind"], "multi_select");
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), with);

        let event = serde_json::to_value(Event::PropertySchemaChanged {
            space_id: SpaceId::from_bytes([1; 16]),
        })
        .unwrap();
        assert_eq!(event["type"], "PropertySchemaChanged");
        assert!(event["data"]["spaceId"].is_string());
    }

    #[test]
    fn page_info_carries_the_devices_that_created_and_edited_the_page_when_known() {
        use crate::ids::DeviceId;
        let bare = PageInfo {
            id: PageId::from_bytes([2; 16]),
            space_id: SpaceId::from_bytes([1; 16]),
            parent_id: None,
            title: "T".into(),
            icon: None,
            cover: None,
            cover_y: None,
            font: None,
            small_text: false,
            locked: false,
            created_at: 1,
            updated_at: 2,
            trashed_at: None,
            created_by: None,
            edited_by: None,
            properties: Vec::new(),
            template: false,
            template_placeholders: Vec::new(),
        };
        // Pages made before the amendment of 2026-10-08 have no device: no key on the wire, and
        // an older payload without the keys still reads.
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("createdBy").is_none() && json.get("editedBy").is_none());
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), bare);

        let author = DeviceId::from_bytes([7; 16]);
        let editor = DeviceId::from_bytes([8; 16]);
        let known = PageInfo {
            created_by: Some(author),
            edited_by: Some(editor),
            ..bare
        };
        let json = serde_json::to_value(&known).unwrap();
        assert_eq!(json["createdBy"], author.to_string());
        assert_eq!(json["editedBy"], editor.to_string());
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), known);
    }

    #[test]
    fn page_cover_request_and_page_info_fields_have_their_shapes() {
        let page = "12345678-9abc-4def-8123-456789abcdef";
        let set = Request::PageSetCover {
            page_id: page.parse().unwrap(),
            cover: Some("gradient:sunset".into()),
            cover_y: Some(30),
        };
        assert_eq!(set.kind(), "PageSetCover");
        assert_eq!(
            serde_json::to_value(&set).unwrap(),
            serde_json::json!({
                "type": "PageSetCover",
                "data": { "pageId": page, "cover": "gradient:sunset", "coverY": 30 }
            })
        );
        // Both fields are optional on the wire: nothing at all clears the cover.
        let clear: Request = serde_json::from_value(serde_json::json!({
            "type": "PageSetCover",
            "data": { "pageId": page }
        }))
        .unwrap();
        assert!(matches!(
            clear,
            Request::PageSetCover {
                cover: None,
                cover_y: None,
                ..
            }
        ));
        let nulls: Request = serde_json::from_value(serde_json::json!({
            "type": "PageSetCover",
            "data": { "pageId": page, "cover": null, "coverY": null }
        }))
        .unwrap();
        assert_eq!(nulls, clear);

        let bare = PageInfo {
            id: PageId::from_bytes([2; 16]),
            space_id: SpaceId::from_bytes([1; 16]),
            parent_id: None,
            title: "T".into(),
            icon: None,
            cover: None,
            cover_y: None,
            font: None,
            small_text: false,
            locked: false,
            created_at: 1,
            updated_at: 2,
            trashed_at: None,
            created_by: None,
            edited_by: None,
            properties: Vec::new(),
            template: false,
            template_placeholders: Vec::new(),
        };
        // A page without a cover has no key on the wire, and an older payload without them reads.
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("cover").is_none() && json.get("coverY").is_none());
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), bare);
        let covered = PageInfo {
            cover: Some("color:blue".into()),
            cover_y: Some(80),
            ..bare
        };
        let json = serde_json::to_value(&covered).unwrap();
        assert_eq!(json["cover"], "color:blue");
        assert_eq!(json["coverY"], 80);
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), covered);
    }

    #[test]
    fn page_appearance_request_and_page_info_fields_have_their_shapes() {
        let page = "12345678-9abc-4def-8123-456789abcdef";
        let set = Request::PageSetAppearance {
            page_id: page.parse().unwrap(),
            font: Some(PageFont::Serif),
            small_text: Some(true),
            locked: Some(false),
        };
        assert_eq!(set.kind(), "PageSetAppearance");
        let json = serde_json::json!({
            "type": "PageSetAppearance",
            "data": { "pageId": page, "font": "serif", "smallText": true, "locked": false }
        });
        assert_eq!(serde_json::to_value(&set).unwrap(), json);
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), set);

        // An absent field is left as it is; a field is cleared by its neutral value.
        let nothing: Request = serde_json::from_value(serde_json::json!({
            "type": "PageSetAppearance",
            "data": { "pageId": page }
        }))
        .unwrap();
        assert!(matches!(
            nothing,
            Request::PageSetAppearance {
                font: None,
                small_text: None,
                locked: None,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(&nothing).unwrap(),
            serde_json::json!({ "type": "PageSetAppearance", "data": { "pageId": page } })
        );
        let cleared: Request = serde_json::from_value(serde_json::json!({
            "type": "PageSetAppearance",
            "data": { "pageId": page, "font": "default", "smallText": false, "locked": false }
        }))
        .unwrap();
        assert!(matches!(
            cleared,
            Request::PageSetAppearance {
                font: Some(PageFont::Default),
                small_text: Some(false),
                locked: Some(false),
                ..
            }
        ));
        // Only the three names exist on the wire (a `null` is not one).
        for bad in ["comic", "Serif", ""] {
            let refused = serde_json::from_value::<Request>(serde_json::json!({
                "type": "PageSetAppearance",
                "data": { "pageId": page, "font": bad }
            }));
            assert!(refused.is_err(), "{bad:?}");
        }
        // What the document stores: the two fonts by name, the default as no value.
        assert_eq!(PageFont::Default.stored(), None);
        assert_eq!(PageFont::Serif.stored(), Some("serif"));
        assert_eq!(PageFont::Mono.stored(), Some("mono"));
        assert_eq!(PageFont::from_stored("serif"), Some(PageFont::Serif));
        assert_eq!(PageFont::from_stored("mono"), Some(PageFont::Mono));
        for other in ["", "sans", "Serif", "serif ", "default"] {
            assert_eq!(PageFont::from_stored(other), None, "{other:?}");
        }
        assert_eq!(PageFont::Default.as_str(), "default");

        let bare = PageInfo {
            id: PageId::from_bytes([2; 16]),
            space_id: SpaceId::from_bytes([1; 16]),
            parent_id: None,
            title: "T".into(),
            icon: None,
            cover: None,
            cover_y: None,
            font: None,
            small_text: false,
            locked: false,
            created_at: 1,
            updated_at: 2,
            trashed_at: None,
            created_by: None,
            edited_by: None,
            properties: Vec::new(),
            template: false,
            template_placeholders: Vec::new(),
        };
        // A page with the defaults has no keys on the wire, and an older payload without them reads.
        let json = serde_json::to_value(&bare).unwrap();
        assert!(
            json.get("font").is_none()
                && json.get("smallText").is_none()
                && json.get("locked").is_none()
        );
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), bare);
        let styled = PageInfo {
            font: Some(PageFont::Mono),
            small_text: true,
            locked: true,
            ..bare
        };
        let json = serde_json::to_value(&styled).unwrap();
        assert_eq!(json["font"], "mono");
        assert_eq!(json["smallText"], true);
        assert_eq!(json["locked"], true);
        assert_eq!(serde_json::from_value::<PageInfo>(json).unwrap(), styled);
    }

    #[test]
    fn favourite_requests_and_the_tree_favourites_have_their_shapes() {
        let page = "12345678-9abc-4def-8123-456789abcdef";
        let other = "22345678-9abc-4def-8123-456789abcdef";
        let set = Request::PageFavoriteSet {
            page_id: page.parse().unwrap(),
            favorite: true,
        };
        assert_eq!(set.kind(), "PageFavoriteSet");
        assert_eq!(
            serde_json::to_value(&set).unwrap(),
            serde_json::json!({
                "type": "PageFavoriteSet",
                "data": { "pageId": page, "favorite": true }
            })
        );
        // `before` is optional on the wire: absent means last.
        let last: Request = serde_json::from_value(serde_json::json!({
            "type": "PageFavoriteMove",
            "data": { "pageId": page }
        }))
        .unwrap();
        assert!(matches!(
            last,
            Request::PageFavoriteMove { before: None, .. }
        ));
        assert_eq!(last.kind(), "PageFavoriteMove");
        let before = Request::PageFavoriteMove {
            page_id: page.parse().unwrap(),
            before: Some(other.parse().unwrap()),
        };
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::json!({
                "type": "PageFavoriteMove",
                "data": { "pageId": page, "before": other }
            })
        );
        let back: Request = serde_json::from_value(serde_json::to_value(&before).unwrap()).unwrap();
        assert_eq!(back, before);

        // A tree without favourites omits the field; an older payload without it reads as empty.
        let space = "32345678-9abc-4def-8123-456789abcdef";
        let mut view = TreeView {
            space_id: space.parse().unwrap(),
            roots: Vec::new(),
            trashed: Vec::new(),
            undo: UndoState {
                can_undo: false,
                can_redo: false,
            },
            favorites: Vec::new(),
        };
        let json = serde_json::to_value(&view).unwrap();
        assert!(json.get("favorites").is_none());
        let older: TreeView = serde_json::from_value(json).unwrap();
        assert!(older.favorites.is_empty());
        view.favorites = vec![page.parse().unwrap(), other.parse().unwrap()];
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["favorites"], serde_json::json!([page, other]));
        let back: TreeView = serde_json::from_value(json).unwrap();
        assert_eq!(back, view);
    }

    #[test]
    fn response_with_option_payload_roundtrips() {
        let none = Response::Workspace(None);
        let json = serde_json::to_value(&none).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "Workspace", "data": null })
        );
        let back: Response = serde_json::from_value(json).unwrap();
        assert_eq!(back, none);
    }

    #[test]
    fn error_details_are_omitted_when_absent() {
        let err = ApiError::not_found("no such setting");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "code": "NotFound", "message": "no such setting" })
        );
        let err = ApiError::invalid("name", "too long");
        assert_eq!(err.details, Some(serde_json::json!({ "field": "name" })));
        assert_eq!(err.to_string(), "InvalidInput: too long");
    }

    fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    fn account(n: u8) -> AccountId {
        AccountId::from_bytes([n; 16])
    }

    #[test]
    fn a_space_info_of_a_personal_space_is_unchanged_on_the_wire() {
        let plain = SpaceInfo {
            id: space(1),
            name: "Home".into(),
            icon: None,
            created_at: 5,
            page_count: 3,
            share_state: SpaceShareState::Personal,
            role: None,
            archive_reason: None,
            member_count: 0,
            ai_enabled: true,
        };
        // Nothing of Milestone 13 is on the wire for a personal space...
        let json = serde_json::to_value(&plain).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": space(1).to_string(), "name": "Home", "createdAt": 5, "pageCount": 3
            })
        );
        // ... and a payload written before Milestone 13 reads as a personal space with AI on.
        assert_eq!(serde_json::from_value::<SpaceInfo>(json).unwrap(), plain);

        let shared = SpaceInfo {
            share_state: SpaceShareState::Shared,
            role: Some(MemberRole::Editor),
            member_count: 4,
            ai_enabled: false,
            ..plain.clone()
        };
        let json = serde_json::to_value(&shared).unwrap();
        assert_eq!(json["shareState"], "Shared");
        assert_eq!(json["role"], "editor");
        assert_eq!(json["memberCount"], 4);
        assert_eq!(json["aiEnabled"], false);
        assert!(json.get("archiveReason").is_none());
        assert_eq!(serde_json::from_value::<SpaceInfo>(json).unwrap(), shared);

        let archive = SpaceInfo {
            share_state: SpaceShareState::Archived,
            role: Some(MemberRole::Viewer),
            archive_reason: Some(ArchiveReason::Removed),
            ..plain
        };
        let json = serde_json::to_value(&archive).unwrap();
        assert_eq!(json["shareState"], "Archived");
        assert_eq!(json["archiveReason"], "Removed");
        assert!(json.get("aiEnabled").is_none());
        assert_eq!(serde_json::from_value::<SpaceInfo>(json).unwrap(), archive);
    }

    #[test]
    fn sync_info_carries_its_feeds_only_when_there_are_some() {
        let bare = SyncInfo {
            configured: false,
            server_url: None,
            state: SyncState::Disabled,
            pending_entries: 0,
            parked_items: 0,
            remote_cursor: 0,
            server_epoch: None,
            last_push_at: None,
            last_pull_at: None,
            last_error: None,
            fork_detected: false,
            devices_seen: 0,
            needs_invite_token: false,
            transfers: TransferInfo::default(),
            feeds: Vec::new(),
        };
        let json = serde_json::to_value(&bare).unwrap();
        assert!(json.get("feeds").is_none());
        assert_eq!(serde_json::from_value::<SyncInfo>(json).unwrap(), bare);
        let with = SyncInfo {
            feeds: vec![FeedSyncInfo {
                feed_id: crate::FeedId::from_bytes([2; 16]),
                space_id: Some(space(1)),
                pending_entries: 1,
                refused: 0,
                forked: false,
                closed: false,
                server_flagged: false,
            }],
            ..bare
        };
        let json = serde_json::to_value(&with).unwrap();
        assert_eq!(json["feeds"][0]["spaceId"], space(1).to_string());
        assert_eq!(serde_json::from_value::<SyncInfo>(json).unwrap(), with);
    }

    /// Every request of Milestone 13 with its wire form: the `type`, then the `data` keys in
    /// camelCase, optional fields left out.
    #[test]
    fn the_sharing_requests_have_their_wire_shapes() {
        let s = space(1).to_string();
        let a = account(2).to_string();
        let i = InviteId::from_bytes([3; 16]).to_string();
        let cases: Vec<(Request, serde_json::Value)> = vec![
            (
                Request::SpaceSharePlan { space_id: space(1) },
                serde_json::json!({ "type": "SpaceSharePlan", "data": { "spaceId": s } }),
            ),
            (
                Request::SpaceShare { space_id: space(1) },
                serde_json::json!({ "type": "SpaceShare", "data": { "spaceId": s } }),
            ),
            (
                Request::SpaceMembers { space_id: space(1) },
                serde_json::json!({ "type": "SpaceMembers", "data": { "spaceId": s } }),
            ),
            (
                Request::MemberRoleSet {
                    space_id: space(1),
                    account_id: account(2),
                    role: MemberRole::Commenter,
                },
                serde_json::json!({ "type": "MemberRoleSet",
                    "data": { "spaceId": s, "accountId": a, "role": "commenter" } }),
            ),
            (
                Request::MemberRemove {
                    space_id: space(1),
                    account_id: account(2),
                },
                serde_json::json!({ "type": "MemberRemove",
                    "data": { "spaceId": s, "accountId": a } }),
            ),
            (
                Request::SpaceLeave { space_id: space(1) },
                serde_json::json!({ "type": "SpaceLeave", "data": { "spaceId": s } }),
            ),
            (
                Request::SpaceClose { space_id: space(1) },
                serde_json::json!({ "type": "SpaceClose", "data": { "spaceId": s } }),
            ),
            (
                Request::SpaceSettingsSet {
                    space_id: space(1),
                    trash_retention_days: Some(0),
                    editors_may_link: None,
                },
                serde_json::json!({ "type": "SpaceSettingsSet",
                    "data": { "spaceId": s, "trashRetentionDays": 0 } }),
            ),
            (
                Request::SpaceAiSet {
                    space_id: space(1),
                    enabled: true,
                },
                serde_json::json!({ "type": "SpaceAiSet",
                    "data": { "spaceId": s, "enabled": true } }),
            ),
            (
                Request::ProfileGet,
                serde_json::json!({ "type": "ProfileGet" }),
            ),
            (
                Request::ProfileSet {
                    display_name: "Alice".into(),
                },
                serde_json::json!({ "type": "ProfileSet", "data": { "displayName": "Alice" } }),
            ),
            (
                Request::PersonGet {
                    account_id: account(2),
                },
                serde_json::json!({ "type": "PersonGet", "data": { "accountId": a } }),
            ),
            (
                Request::PersonVerifiedSet {
                    account_id: account(2),
                    verified: true,
                },
                serde_json::json!({ "type": "PersonVerifiedSet",
                    "data": { "accountId": a, "verified": true } }),
            ),
            (
                Request::InviteLinkCreate {
                    space_id: space(1),
                    role: MemberRole::Viewer,
                    expires_in_days: None,
                },
                serde_json::json!({ "type": "InviteLinkCreate",
                    "data": { "spaceId": s, "role": "viewer" } }),
            ),
            (
                Request::InviteRevoke {
                    space_id: space(1),
                    invite_id: InviteId::from_bytes([3; 16]),
                },
                serde_json::json!({ "type": "InviteRevoke",
                    "data": { "spaceId": s, "inviteId": i } }),
            ),
            (
                Request::InviteCodeStart {
                    space_id: space(1),
                    role: MemberRole::Editor,
                },
                serde_json::json!({ "type": "InviteCodeStart",
                    "data": { "spaceId": s, "role": "editor" } }),
            ),
            (
                Request::InviteCodeJoin {
                    server_url: "https://kb.example".into(),
                    code: "ABCD EFGH JKMN".into(),
                },
                serde_json::json!({ "type": "InviteCodeJoin",
                    "data": { "serverUrl": "https://kb.example", "code": "ABCD EFGH JKMN" } }),
            ),
            (
                Request::InviteLinkOpen {
                    link: "atomik-kb://invite?server=x#ABCD".into(),
                },
                serde_json::json!({ "type": "InviteLinkOpen",
                    "data": { "link": "atomik-kb://invite?server=x#ABCD" } }),
            ),
            (
                Request::InviteConfirm,
                serde_json::json!({ "type": "InviteConfirm" }),
            ),
            (
                Request::InviteCancel,
                serde_json::json!({ "type": "InviteCancel" }),
            ),
            (
                Request::InviteStatus,
                serde_json::json!({ "type": "InviteStatus" }),
            ),
        ];
        let kinds = [
            "SpaceSharePlan",
            "SpaceShare",
            "SpaceMembers",
            "MemberRoleSet",
            "MemberRemove",
            "SpaceLeave",
            "SpaceClose",
            "SpaceSettingsSet",
            "SpaceAiSet",
            "ProfileGet",
            "ProfileSet",
            "PersonGet",
            "PersonVerifiedSet",
            "InviteLinkCreate",
            "InviteRevoke",
            "InviteCodeStart",
            "InviteCodeJoin",
            "InviteLinkOpen",
            "InviteConfirm",
            "InviteCancel",
            "InviteStatus",
        ];
        assert_eq!(cases.len(), kinds.len());
        for ((request, json), kind) in cases.into_iter().zip(kinds) {
            assert_eq!(serde_json::to_value(&request).unwrap(), json, "{kind}");
            assert_eq!(request.kind(), kind);
            assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
        }

        // The optional fields read from their absence.
        let none: Request = serde_json::from_value(serde_json::json!({
            "type": "SpaceSettingsSet", "data": { "spaceId": space(1).to_string() }
        }))
        .unwrap();
        assert!(matches!(
            none,
            Request::SpaceSettingsSet {
                trash_retention_days: None,
                editors_may_link: None,
                ..
            }
        ));
        let link: Request = serde_json::from_value(serde_json::json!({
            "type": "InviteLinkCreate",
            "data": { "spaceId": space(1).to_string(), "role": "editor", "expiresInDays": 30 }
        }))
        .unwrap();
        assert!(matches!(
            link,
            Request::InviteLinkCreate {
                expires_in_days: Some(30),
                ..
            }
        ));
        // A role is lowercase on the wire.
        assert!(
            serde_json::from_value::<Request>(serde_json::json!({
                "type": "InviteCodeStart",
                "data": { "spaceId": space(1).to_string(), "role": "Editor" }
            }))
            .is_err()
        );
    }

    #[test]
    fn the_sharing_responses_and_events_have_their_wire_shapes() {
        let profile = Response::Profile(ProfileInfo {
            display_name: Some("Alice".into()),
        });
        let json = serde_json::to_value(&profile).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "Profile", "data": { "displayName": "Alice" } })
        );
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), profile);

        let person = Response::Person(PersonInfo {
            account_id: account(2),
            name: "Bob".into(),
            verified: false,
            safety_number: Some("00000 00000 00000 00000 00000 00001".into()),
            is_me: false,
        });
        let json = serde_json::to_value(&person).unwrap();
        assert_eq!(json["type"], "Person");
        assert_eq!(
            json["data"]["safetyNumber"],
            "00000 00000 00000 00000 00000 00001"
        );
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), person);

        let invite = Response::Invite(InviteSessionInfo {
            flow: crate::InviteFlow::Link,
            side: crate::InviteSide::Invitee,
            state: crate::InviteSessionState::Previewing,
            code: None,
            sas: None,
            peer_name: None,
            peer_safety_number: None,
            space_name: Some("Project X".into()),
            inviter_name: Some("Alice".into()),
            role: Some(MemberRole::Editor),
            verified: false,
            server_url: None,
            error: None,
            expires_at: None,
        });
        let json = serde_json::to_value(&invite).unwrap();
        assert_eq!(json["type"], "Invite");
        assert_eq!(json["data"]["spaceName"], "Project X");
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), invite);

        let events = [
            (
                Event::SpaceMembersChanged { space_id: space(1) },
                serde_json::json!({ "type": "SpaceMembersChanged",
                    "data": { "spaceId": space(1).to_string() } }),
            ),
            (
                Event::SpaceRoleChanged {
                    space_id: space(1),
                    role: MemberRole::Viewer,
                    previous: MemberRole::Editor,
                },
                serde_json::json!({ "type": "SpaceRoleChanged",
                    "data": { "spaceId": space(1).to_string(), "role": "viewer",
                              "previous": "editor" } }),
            ),
            (
                Event::SpaceRemoved {
                    space_id: space(1),
                    reason: ArchiveReason::Closed,
                },
                serde_json::json!({ "type": "SpaceRemoved",
                    "data": { "spaceId": space(1).to_string(), "reason": "Closed" } }),
            ),
            (
                Event::MemberDeviceViaKit {
                    space_id: space(1),
                    account_id: account(2),
                    name: "Bob".into(),
                },
                serde_json::json!({ "type": "MemberDeviceViaKit",
                    "data": { "spaceId": space(1).to_string(), "accountId": account(2).to_string(),
                              "name": "Bob" } }),
            ),
            (
                Event::PeopleChanged,
                serde_json::json!({ "type": "PeopleChanged" }),
            ),
            (
                Event::InviteChanged,
                serde_json::json!({ "type": "InviteChanged" }),
            ),
            (
                Event::InviteRequested,
                serde_json::json!({ "type": "InviteRequested" }),
            ),
            (
                Event::UnsentChangesCopied {
                    space_id: space(1),
                    target_space_id: space(2),
                    page_ids: vec![PageId::from_bytes([9; 16])],
                },
                serde_json::json!({ "type": "UnsentChangesCopied",
                    "data": { "spaceId": space(1).to_string(),
                              "targetSpaceId": space(2).to_string(),
                              "pageIds": [PageId::from_bytes([9; 16]).to_string()] } }),
            ),
            (
                Event::SnapshotDisputed {
                    space_id: space(1),
                    page_id: None,
                    account_id: account(2),
                },
                serde_json::json!({ "type": "SnapshotDisputed",
                    "data": { "spaceId": space(1).to_string(),
                              "accountId": account(2).to_string() } }),
            ),
            (
                Event::SnapshotDisputed {
                    space_id: space(1),
                    page_id: Some(PageId::from_bytes([9; 16])),
                    account_id: account(2),
                },
                serde_json::json!({ "type": "SnapshotDisputed",
                    "data": { "spaceId": space(1).to_string(),
                              "pageId": PageId::from_bytes([9; 16]).to_string(),
                              "accountId": account(2).to_string() } }),
            ),
        ];
        for (event, json) in events {
            assert_eq!(serde_json::to_value(&event).unwrap(), json);
            assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
        }
    }

    #[test]
    fn forbidden_errors_say_why_and_which_role_was_needed() {
        let role = ApiError::forbidden_role(MemberRole::Editor);
        assert_eq!(role.code, ErrorCode::Forbidden);
        assert_eq!(
            role.details,
            Some(serde_json::json!({ "reason": "role", "required": "editor" }))
        );
        let archived = ApiError::forbidden_archived();
        assert_eq!(archived.code, ErrorCode::Forbidden);
        assert_eq!(
            archived.details,
            Some(serde_json::json!({ "reason": "archived" }))
        );
        let json = serde_json::to_value(&role).unwrap();
        assert_eq!(json["code"], "Forbidden");
        assert_eq!(serde_json::from_value::<ApiError>(json).unwrap(), role);
        assert_eq!(
            archived.to_string(),
            "Forbidden: this space is a read-only archive"
        );
        assert_eq!(crate::API_VERSION, 1, "the contract is additive");
    }

    #[test]
    fn the_comment_and_notification_requests_have_their_wire_shapes() {
        use crate::comments::{CommentAnchor, CommentBodyItem};
        let page = PageId::from_bytes([1; 16]).to_string();
        let comment = CommentId::from_bytes([2; 16]);
        let thread = ThreadId::of(comment);
        let body = vec![CommentBodyItem::Text { text: "Hi".into() }];
        let body_json = serde_json::json!([{ "type": "text", "text": "Hi" }]);
        let cases: Vec<(Request, serde_json::Value)> = vec![
            (
                Request::CommentList {
                    page_id: PageId::from_bytes([1; 16]),
                },
                serde_json::json!({ "type": "CommentList", "data": { "pageId": page } }),
            ),
            (
                Request::CommentPost {
                    page_id: PageId::from_bytes([1; 16]),
                    thread_id: None,
                    anchor: Some(CommentAnchor {
                        block_id: Some(BlockId::from_bytes([3; 16])),
                        start: Some("AQID".into()),
                        end: Some("BAUG".into()),
                        quote: "plan".into(),
                    }),
                    body: body.clone(),
                },
                serde_json::json!({ "type": "CommentPost", "data": {
                    "pageId": page,
                    "anchor": { "blockId": BlockId::from_bytes([3; 16]).to_string(),
                                "start": "AQID", "end": "BAUG", "quote": "plan" },
                    "body": body_json } }),
            ),
            (
                Request::CommentPost {
                    page_id: PageId::from_bytes([1; 16]),
                    thread_id: Some(thread),
                    anchor: None,
                    body: body.clone(),
                },
                serde_json::json!({ "type": "CommentPost", "data": {
                    "pageId": page, "threadId": thread.to_string(), "body": body_json } }),
            ),
            (
                Request::CommentEdit {
                    page_id: PageId::from_bytes([1; 16]),
                    comment_id: comment,
                    body: body.clone(),
                },
                serde_json::json!({ "type": "CommentEdit", "data": {
                    "pageId": page, "commentId": comment.to_string(), "body": body_json } }),
            ),
            (
                Request::CommentDelete {
                    page_id: PageId::from_bytes([1; 16]),
                    comment_id: comment,
                },
                serde_json::json!({ "type": "CommentDelete", "data": {
                    "pageId": page, "commentId": comment.to_string() } }),
            ),
            (
                Request::CommentResolve {
                    page_id: PageId::from_bytes([1; 16]),
                    thread_id: thread,
                    resolved: true,
                },
                serde_json::json!({ "type": "CommentResolve", "data": {
                    "pageId": page, "threadId": thread.to_string(), "resolved": true } }),
            ),
            (
                Request::NotificationList {
                    limit: None,
                    before_id: None,
                    unread_only: None,
                },
                serde_json::json!({ "type": "NotificationList", "data": {} }),
            ),
            (
                Request::NotificationList {
                    limit: Some(20),
                    before_id: Some(9),
                    unread_only: Some(true),
                },
                serde_json::json!({ "type": "NotificationList", "data": {
                    "limit": 20, "beforeId": 9, "unreadOnly": true } }),
            ),
            (
                Request::NotificationsMarkRead { ids: None },
                serde_json::json!({ "type": "NotificationsMarkRead", "data": {} }),
            ),
            (
                Request::NotificationsMarkRead {
                    ids: Some(vec![3, 4]),
                },
                serde_json::json!({ "type": "NotificationsMarkRead", "data": { "ids": [3, 4] } }),
            ),
            (
                Request::PeopleMentionable { space_id: space(1) },
                serde_json::json!({ "type": "PeopleMentionable",
                    "data": { "spaceId": space(1).to_string() } }),
            ),
        ];
        for (request, json) in cases {
            assert_eq!(serde_json::to_value(&request).unwrap(), json);
            assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
        }
        // Every new request has its variant name in `kind()`, which logs use instead of payloads.
        assert_eq!(
            Request::CommentPost {
                page_id: PageId::from_bytes([1; 16]),
                thread_id: None,
                anchor: None,
                body: Vec::new(),
            }
            .kind(),
            "CommentPost"
        );
        assert_eq!(
            Request::NotificationsMarkRead { ids: None }.kind(),
            "NotificationsMarkRead"
        );
        assert_eq!(
            Request::PeopleMentionable { space_id: space(1) }.kind(),
            "PeopleMentionable"
        );
    }

    #[test]
    fn the_comment_responses_and_events_have_their_wire_shapes() {
        use crate::comments::{NotificationInfo, NotificationKind, ThreadInfo};
        let thread = ThreadInfo {
            thread_id: ThreadId::from_bytes([4; 16]),
            page_id: PageId::from_bytes([1; 16]),
            anchor: None,
            resolved: true,
            resolved_by: Some(account(5)),
            comments: Vec::new(),
        };
        let responses: Vec<(Response, serde_json::Value)> = vec![
            (
                Response::Thread(thread.clone()),
                serde_json::json!({ "type": "Thread", "data": {
                    "threadId": ThreadId::from_bytes([4; 16]).to_string(),
                    "pageId": PageId::from_bytes([1; 16]).to_string(),
                    "resolved": true,
                    "resolvedBy": account(5).to_string(),
                    "comments": [] } }),
            ),
            (
                Response::Comments(vec![thread]),
                serde_json::json!({ "type": "Comments", "data": [{
                    "threadId": ThreadId::from_bytes([4; 16]).to_string(),
                    "pageId": PageId::from_bytes([1; 16]).to_string(),
                    "resolved": true,
                    "resolvedBy": account(5).to_string(),
                    "comments": [] }] }),
            ),
            (
                Response::Notifications(Vec::new()),
                serde_json::json!({ "type": "Notifications", "data": [] }),
            ),
            (
                Response::People(vec![PersonInfo {
                    account_id: account(6),
                    name: "Bob".into(),
                    verified: false,
                    safety_number: None,
                    is_me: false,
                }]),
                serde_json::json!({ "type": "People", "data": [{
                    "accountId": account(6).to_string(), "name": "Bob",
                    "verified": false, "isMe": false }] }),
            ),
        ];
        for (response, json) in responses {
            assert_eq!(serde_json::to_value(&response).unwrap(), json);
            assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
        }
        let info = NotificationInfo {
            id: 3,
            kind: NotificationKind::CommentMention,
            space_id: space(1),
            page_id: Some(PageId::from_bytes([1; 16])),
            block_id: None,
            comment_id: Some(CommentId::from_bytes([2; 16])),
            actor: account(5),
            actor_name: "Alice".into(),
            page_title: None,
            at: 1_760_000_000_000,
            read: false,
        };
        let events: Vec<(Event, serde_json::Value)> = vec![
            (
                Event::CommentsChanged {
                    page_id: PageId::from_bytes([1; 16]),
                },
                serde_json::json!({ "type": "CommentsChanged",
                    "data": { "pageId": PageId::from_bytes([1; 16]).to_string() } }),
            ),
            (
                Event::NotificationRaised(info.clone()),
                serde_json::json!({ "type": "NotificationRaised", "data": {
                    "id": 3, "kind": "CommentMention", "spaceId": space(1).to_string(),
                    "pageId": PageId::from_bytes([1; 16]).to_string(),
                    "commentId": CommentId::from_bytes([2; 16]).to_string(),
                    "actor": account(5).to_string(), "actorName": "Alice",
                    "at": 1_760_000_000_000_i64, "read": false } }),
            ),
            (
                Event::NotificationsChanged,
                serde_json::json!({ "type": "NotificationsChanged" }),
            ),
        ];
        for (event, json) in events {
            assert_eq!(serde_json::to_value(&event).unwrap(), json);
            assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
        }
    }

    #[test]
    fn the_view_link_requests_responses_and_event_have_their_wire_shapes() {
        use crate::sharing::{ViewLinkInfo, ViewLinkStatus};
        let link = LinkId::from_bytes([0xcd; 16]);
        let page = PageId::from_bytes([7; 16]);
        let cases = [
            (
                Request::ViewLinkCreate {
                    space_id: space(1),
                    page_id: page,
                    expires_in_days: Some(7),
                },
                serde_json::json!({ "type": "ViewLinkCreate", "data": {
                    "spaceId": space(1).to_string(), "pageId": page.to_string(),
                    "expiresInDays": 7 } }),
                "ViewLinkCreate",
            ),
            (
                Request::ViewLinkCreate {
                    space_id: space(1),
                    page_id: page,
                    expires_in_days: None,
                },
                serde_json::json!({ "type": "ViewLinkCreate", "data": {
                    "spaceId": space(1).to_string(), "pageId": page.to_string() } }),
                "ViewLinkCreate",
            ),
            (
                Request::ViewLinkList { space_id: space(1) },
                serde_json::json!({ "type": "ViewLinkList",
                    "data": { "spaceId": space(1).to_string() } }),
                "ViewLinkList",
            ),
            (
                Request::ViewLinkRevoke {
                    space_id: space(1),
                    link_id: link,
                },
                serde_json::json!({ "type": "ViewLinkRevoke", "data": {
                    "spaceId": space(1).to_string(), "linkId": link.to_string() } }),
                "ViewLinkRevoke",
            ),
        ];
        for (request, json, kind) in cases {
            assert_eq!(serde_json::to_value(&request).unwrap(), json, "{kind}");
            assert_eq!(request.kind(), kind);
            assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
        }

        let info = ViewLinkInfo {
            link_id: link,
            page_id: page,
            url: "http://127.0.0.1:8787/view#KEY".into(),
            created_by: account(5),
            created_by_name: "Alice".into(),
            created_at: 1_791_383_400_000,
            expires_at: None,
            last_published_at: None,
            status: ViewLinkStatus::Publishing,
        };
        let one = serde_json::json!({ "type": "ViewLink", "data": {
            "linkId": link.to_string(), "pageId": page.to_string(),
            "url": "http://127.0.0.1:8787/view#KEY",
            "createdBy": account(5).to_string(), "createdByName": "Alice",
            "createdAt": 1_791_383_400_000_i64, "status": "Publishing" } });
        let response = Response::ViewLink(info.clone());
        assert_eq!(serde_json::to_value(&response).unwrap(), one);
        assert_eq!(serde_json::from_value::<Response>(one).unwrap(), response);
        let many = Response::ViewLinks(vec![info]);
        let json = serde_json::to_value(&many).unwrap();
        assert_eq!(json["type"], "ViewLinks");
        assert_eq!(json["data"][0]["status"], "Publishing");
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), many);
        assert_eq!(
            serde_json::to_value(Response::ViewLinks(Vec::new())).unwrap(),
            serde_json::json!({ "type": "ViewLinks", "data": [] })
        );

        let event = Event::ViewLinksChanged { space_id: space(1) };
        let json = serde_json::json!({ "type": "ViewLinksChanged",
            "data": { "spaceId": space(1).to_string() } });
        assert_eq!(serde_json::to_value(&event).unwrap(), json);
        assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);

        // `expiresInDays` is an unsigned number: a negative or a text is not a request.
        for bad in [serde_json::json!(-1), serde_json::json!("7")] {
            assert!(
                serde_json::from_value::<Request>(serde_json::json!({
                    "type": "ViewLinkCreate", "data": {
                        "spaceId": space(1).to_string(), "pageId": page.to_string(),
                        "expiresInDays": bad } }))
                .is_err()
            );
        }
    }

    #[test]
    fn the_presence_request_response_and_event_have_their_wire_shapes() {
        use crate::sharing::{PresenceInfo, PresenceState};
        let request = Request::PresenceList { space_id: space(1) };
        let json = serde_json::json!({ "type": "PresenceList",
            "data": { "spaceId": space(1).to_string() } });
        assert_eq!(serde_json::to_value(&request).unwrap(), json);
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
        assert_eq!(request.kind(), "PresenceList");

        let info = PresenceInfo {
            account_id: account(5),
            device_id: DeviceId::from_bytes([6; 16]),
            name: "Bob".into(),
            color: 1,
            page_id: Some(PageId::from_bytes([7; 16])),
            state: PresenceState::Viewing,
            updated_at: 1_791_383_400_000,
        };
        let response = Response::Presence(vec![info]);
        let json = serde_json::json!({ "type": "Presence", "data": [{
            "accountId": account(5).to_string(),
            "deviceId": DeviceId::from_bytes([6; 16]).to_string(),
            "name": "Bob", "color": 1,
            "pageId": PageId::from_bytes([7; 16]).to_string(),
            "state": "Viewing", "updatedAt": 1_791_383_400_000_i64 }] });
        assert_eq!(serde_json::to_value(&response).unwrap(), json);
        assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
        assert_eq!(
            serde_json::to_value(Response::Presence(Vec::new())).unwrap(),
            serde_json::json!({ "type": "Presence", "data": [] })
        );

        let event = Event::PresenceChanged { space_id: space(1) };
        let json = serde_json::json!({ "type": "PresenceChanged",
            "data": { "spaceId": space(1).to_string() } });
        assert_eq!(serde_json::to_value(&event).unwrap(), json);
        assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
    }

    #[test]
    fn person_values_and_filters_have_their_wire_shapes() {
        use crate::properties::MAX_PEOPLE;
        let value = PropertyValue::Person(vec![account(1), account(2)]);
        let json = serde_json::json!({
            "kind": "person",
            "value": [account(1).to_string(), account(2).to_string()]
        });
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        assert_eq!(
            serde_json::from_value::<PropertyValue>(json).unwrap(),
            value
        );
        assert_eq!(value.kind(), PropertyKind::Person);
        assert_eq!(PropertyKind::Person.as_str(), "person");
        assert_eq!(PropertyKind::parse("person"), Some(PropertyKind::Person));
        assert!(PropertyKind::ALL.contains(&PropertyKind::Person));
        assert!(!PropertyKind::Person.is_computed());
        assert_eq!(MAX_PEOPLE, 20);
        assert!(
            serde_json::from_value::<PropertyValue>(
                serde_json::json!({ "kind": "person", "value": ["not an id"] })
            )
            .is_err()
        );
        let filters = [
            (
                PropertyFilter::ContainsPerson(account(3)),
                serde_json::json!({ "type": "ContainsPerson", "data": account(3).to_string() }),
            ),
            (
                PropertyFilter::ContainsMe,
                serde_json::json!({ "type": "ContainsMe" }),
            ),
        ];
        for (filter, json) in filters {
            assert_eq!(serde_json::to_value(&filter).unwrap(), json);
            assert_eq!(
                serde_json::from_value::<PropertyFilter>(json).unwrap(),
                filter
            );
        }
    }
}
