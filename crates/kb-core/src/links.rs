//! View-only links in the core (M13 slice 4, ADR-0051, `docs/security/sharing.md` section 10,
//! `docs/development/milestone-13.md` sections 4.1 and 4.2 and their contract notes): creating a link for
//! a page, keeping its publication fresh, revoking it and the host side the sync driver calls.
//!
//! The core stays sans-IO: it builds the sealed publications and hands them to the driver as
//! [`LinkWork`] (`kb-net`'s `links.rs` uploads them), and it never contacts the server. A view link has
//! four parts, all written here and nowhere else:
//!
//! 1. the **link key** `K_link` (`LinkKey::generate`), whose `Link` record (`kb_keys::keyring::wrap_link_key`,
//!    under the space key of the current epoch) goes into the keyring of the space's feed (the personal
//!    keyring for a personal space), so every member's device can refresh the link;
//! 2. the **map entry** `links/<link id>` of the structure document (`StructureDoc::link_put`:
//!    page, creator, creation time, expiry), which lists the links to every member and to the Owners'
//!    orphan sweep;
//! 3. the **server row**, created and refreshed by the driver's `PUT` of the publication;
//! 4. the **device-local state** (the first upload pending, the time of the last upload, the due time
//!    of the next refresh, the deletions to retry), kept in `sync_config` under keys `links.*` (no
//!    migration) and in memory.
//!
//! # `ViewLinkCreate`
//!
//! Needs, in this order: the role (`require_role`, so an archive is `Forbidden` "archived"): an Owner, or
//! an Editor when the space's `editorsMayLink` setting is on, anyone in a personal space; an Editor with
//! the setting off is `ApiError::forbidden_role(Owner)`, a Commenter or Viewer `forbidden_role(Editor)`;
//! `expiresInDays` absent or 1 to `LINK_EXPIRY_MAX_DAYS` (`InvalidInput` for `expiresInDays`); a **live**
//! page of the space with at most `LINK_MAX_CHUNKS` attachment chunks in the attachments it references
//! (`InvalidInput` for `pageId`, never leaking which of the reasons); sync configured and the account
//! created (`InvalidInput` for `serverUrl`, as for sharing). It then makes `K_link`, writes the
//! record and the map entry in one commit, notes the first upload as pending (the link is `Publishing`
//! on this device until the driver reports it), emits `ViewLinksChanged` and `SyncRequested`, and answers
//! the link with its URL, `<server url without a trailing slash>/view#<LinkKey::to_fragment>`. **That answer
//! is the only place the URL is shown**: it is never stored, never logged and never listed again.
//!
//! # `ViewLinkList` and the status
//!
//! The entries of the structure's `links` map of the space, newest `created_at` first, for any member
//! and for an archive. `createdByName` is the member's name in the space (`FormerMemberInfo` when they
//! left or were removed: the link survives its creator until an Owner revokes it). **`url` is always the
//! empty string** (the URL is shown once, in the `ViewLinkCreate` answer, even though the `Link` record
//! would let a member rebuild it); a map entry whose record is missing or tombstoned is not listed (it is
//! an Owner's to clean up). `status`: `Expired` when `expires_at <= now`; `Publishing` on the device that
//! made the link until its first upload succeeded; else `Live`. Devices that did not make the link say
//! `Live` (they cannot know), and `lastPublishedAt` is this device's own latest upload (absent when it
//! never uploaded).
//!
//! # `ViewLinkRevoke`
//!
//! The link's creator or an Owner (`forbidden_role(Owner)` for another member; an archive is
//! `Forbidden` "archived"; `InvalidInput` for `linkId` when the space has no such entry). It removes the
//! map entry, **tombstones the `Link` record** when this device may (an Owner's device, or any device
//! in a personal space: tombstones in a shared feed are Owner only), queues a server `Delete` that
//! survives restarts until `sync_link_done` says `Deleted` or `Refused`, drops the local state of
//! the link, emits `ViewLinksChanged` and answers the links that remain. A non-Owner creator leaves
//! the tombstone to an Owner's device (see "Orphans").
//!
//! # Refreshing
//!
//! [`Core::view_links_page_changed`] is the hook: it is called for a page whose document changed on
//! this device, locally (`pages.rs`, `persist_page`) or by an entry applied from the feed (`sync.rs`,
//! next to `PageContentChanged { source: Remote }`). It does nothing unless the page has map entries in
//! its space and this device's role is Editor or Owner (a Viewer's or Commenter's device never
//! publishes; a comment is another document and never calls the hook). For each such link it notes the
//! page as dirty, **due 60 seconds (`LINK_REFRESH_DEBOUNCE_MS`) after the first change not yet
//! published**: later changes ride along, so a person who types continuously publishes once a minute,
//! never more often, and a device that applies a burst of remote entries publishes once. Every Editor
//! device of the space does this, so a refresh follows an edit within about a minute wherever it was
//! made.
//!
//! # The host side (`sync_link_work` and `sync_link_done`)
//!
//! `sync_link_work` returns the work due now: the first upload of a link made on this device
//! (no debounce), the refreshes whose time has come, and the queued deletions. Each `Put` carries a
//! publication made now: the page document's shallow snapshot (`Doc::export_shallow_snapshot`, no
//! history), the title, icon, cover and `cover_y` from the structure, the attachments the page's blocks,
//! icon and cover reference (the manifests from the structure, the keys from the keyring; an
//! attachment this device cannot read is left out), the names of the people the page mentions (members'
//! names in the space, or their last known name; never a name nobody mentioned) and no comments, sealed with
//! `kb_sync::v2::seal_publication`, plus the declared chunks (`Publication::chunk_ids`) and the
//! map entry's expiry. No work when the device's role is below Editor, the space is an archive, the
//! feed is closed or sync is not configured. `sync_link_done` records the outcome: `Published` sets
//! `last_published_at`, clears the dirty note, and emits `ViewLinksChanged` once per link when the status
//! changed from `Publishing`; `Deleted` and `Refused` end a deletion; `Refused` for a `Put` stops the
//! offers of that link until a refresh is noted again (a role change) and is shown as
//! `Publishing` to its creator.
//!
//! # Orphans and expiry
//!
//! [`Core::view_links_maintain`] runs on every sync cycle (from `sync_feeds`) on devices whose role is
//! Owner. It **tombstones a `Link` record** (a signed tombstone through the keyring, which travels in the
//! feed) when its signing device's account is not an Owner and the structure's `links` map has no
//! entry for its link id (a non-Owner creator revoked the link and could not tombstone), and the
//! record has been an orphan on this device for at least `PULL_INTERVAL_MS` (the entry may still be in
//! flight: the keyring and the structure are two documents). Expiry is the server's; the core only
//! reports `Expired` and never deletes an expired link's entry by itself (revoking cleans it).
//!
//! # Implementation notes (beyond the contract)
//!
//! - **A key this device cannot open.** A `Link` record wrapped under a space key epoch this device never
//!   held (a member who joined after a Rekey) is listed like any other (the list shows no URL), so an Owner
//!   can still revoke it; the link cannot be refreshed from this device. An entry with no record, or a
//!   tombstoned one, is not listed.
//! - **A closed space.** The server deletes the links of a closed feed, so the links of an archive that an
//!   Owner closed are listed `Revoked`; those of an archive a member was removed from stay `Live` (nobody
//!   here can refresh them any more).
//! - **Structure edits.** A title, icon or cover edit changes the publication too: `mutate_structure` and
//!   the remote apply of the structure call the refresh hook for the linked pages whose look changed.
//! - **Purging a page.** An Owner's (or a personal) device that purges a page revokes its links in the same
//!   step (the publication would otherwise outlive the page it shows).
//! - **Links changed elsewhere.** A structure update or a `Link` keyring item that changes the space's
//!   links emits `ViewLinksChanged`, and so does a link that crossed its expiry (seen on the next
//!   cycle, `view_links_maintain`).
//! - **No work is built for** a link whose page is trashed or purged, an expired link, a page whose
//!   document is not on this device, or a key this device cannot open; the dirty note stays and the work
//!   comes when it can be built. A publication over `LINK_MAX_CHUNKS` chunks (the page grew after the link
//!   was made) leaves out the attachments past the limit, in attachment id order.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use kb_doc::DocKind;
use kb_keys::keyring as kr;
use kb_keys::link::LinkKey;
use kb_keys::{KeySubject, KeyringItem};
use kb_model::{PageDoc, StructureDoc, ViewLinkRecord};
use kb_storage::{DocQueries, FeedQueries, SyncQueries, UpdateOrigin};
use kb_sync::v2::publication::{
    PUBLICATION_MAX_MENTIONS, PUBLICATION_SHORT_MAX_BYTES, PUBLICATION_TITLE_MAX_BYTES,
};
use kb_sync::v2::{
    LinkOutcome, LinkPut, LinkWork, Publication, PublishedAttachment, PublishedPerson,
    seal_publication,
};
use kb_types::sharing::{LINK_EXPIRY_MAX_DAYS, LINK_MAX_CHUNKS, LINK_REFRESH_DEBOUNCE_MS};
use kb_types::sync::PULL_INTERVAL_MS;
use kb_types::{
    AccountId, ApiError, ArchiveReason, AttachmentId, ChangeSource, DeviceId, DocId, Event, FeedId,
    LinkId, MemberRole, PageId, Request, Response, SpaceId, ViewLinkInfo, ViewLinkStatus,
};

use crate::documents::map_model_error;
use crate::{Core, documents, map_doc_error, map_storage_error, validate};

/// One day in milliseconds.
const DAY_MS: i64 = 86_400_000;
/// The cover position a page without one shows at (`PageInfo::cover_y` absent).
const DEFAULT_COVER_Y: u8 = 50;

/// What the core remembers about links in memory (the durable parts are `sync_config` keys `links.*`).
#[derive(Debug, Default)]
pub(crate) struct LinksRuntime {
    /// The uploads handed to the driver and not reported yet, each with the time of the first change that
    /// arrived after the work was made (so `Published` keeps a note for it).
    inflight: HashMap<LinkId, Option<i64>>,
    /// When this device first saw each `Link` record without a map entry (the Owner sweep waits).
    orphans: HashMap<LinkId, i64>,
    /// The links whose expiry was announced with `ViewLinksChanged`.
    expired: HashSet<LinkId>,
}

/// The look of a page a publication shows (everything but its content).
type PageLook = (String, Option<String>, Option<String>, Option<u8>);

/// The links of a space and the look of their pages, as a structure document held them at one moment:
/// what the remote apply compares before and after.
#[derive(Debug, Default)]
pub(crate) struct LinkSnapshot {
    links: Vec<ViewLinkRecord>,
    pages: BTreeMap<PageId, PageLook>,
}

/// The snapshot of a structure document (empty, and cheap, when the space has no link).
pub(crate) fn link_snapshot(model: &StructureDoc) -> LinkSnapshot {
    let links = model.links();
    let mut pages = BTreeMap::new();
    for record in &links {
        if let Some(info) = model.page(record.page_id) {
            pages.insert(
                record.page_id,
                (info.title, info.icon, info.cover, info.cover_y),
            );
        }
    }
    LinkSnapshot { links, pages }
}

/// A page's document as a publication needs it.
struct PageContent {
    /// `Doc::export_shallow_snapshot`.
    snapshot: Vec<u8>,
    /// The attachments the blocks reference, each once.
    refs: BTreeSet<AttachmentId>,
    /// The accounts the page mentions with the label stored at insertion.
    mentions: BTreeMap<AccountId, String>,
}

/// `text` cut to at most `max` bytes at a character boundary.
fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The `sync_config` key of one piece of device-local link state.
fn cfg_key(kind: &str, link_id: LinkId) -> String {
    format!("links.{kind}.{link_id}")
}

/// The first upload of a link made on this device is still to do.
const PENDING: &str = "p";
/// When this device's latest upload succeeded (Unix ms).
const PUBLISHED_AT: &str = "a";
/// When the next refresh is due (Unix ms).
const DUE: &str = "d";
/// The server refused the last upload: no offers until a change is noted again.
const REFUSED: &str = "r";
/// A deletion to retry.
const DELETE: &str = "x";

impl Core {
    /// Routes `ViewLinkCreate`, `ViewLinkList` and `ViewLinkRevoke`.
    pub(crate) fn dispatch_links(&mut self, request: Request) -> Result<Response, ApiError> {
        match request {
            Request::ViewLinkCreate {
                space_id,
                page_id,
                expires_in_days,
            } => Ok(Response::ViewLink(self.view_link_create(
                space_id,
                page_id,
                expires_in_days,
            )?)),
            Request::ViewLinkList { space_id } => {
                Ok(Response::ViewLinks(self.view_link_list(space_id)?))
            }
            Request::ViewLinkRevoke { space_id, link_id } => Ok(Response::ViewLinks(
                self.view_link_revoke(space_id, link_id)?,
            )),
            other => Err(ApiError::internal(format!(
                "{} is not a view link request",
                other.kind()
            ))),
        }
    }

    // ----- device-local state ---------------------------------------------------------------

    fn link_cfg(&self, kind: &str, link_id: LinkId) -> Result<Option<String>, ApiError> {
        self.db
            .sync_config_get(&cfg_key(kind, link_id))
            .map_err(map_storage_error)
    }

    fn link_cfg_set(&self, kind: &str, link_id: LinkId, value: &str) -> Result<(), ApiError> {
        self.db
            .sync_config_set(&cfg_key(kind, link_id), value)
            .map_err(map_storage_error)
    }

    fn link_cfg_del(&self, kind: &str, link_id: LinkId) -> Result<(), ApiError> {
        self.db
            .sync_config_delete(&cfg_key(kind, link_id))
            .map_err(map_storage_error)
    }

    fn link_cfg_i64(&self, kind: &str, link_id: LinkId) -> Result<Option<i64>, ApiError> {
        Ok(self
            .link_cfg(kind, link_id)?
            .and_then(|value| value.parse().ok()))
    }

    /// Drops the device-local state of an upload (not the queued deletion).
    fn link_state_clear(&mut self, link_id: LinkId) -> Result<(), ApiError> {
        for kind in [PENDING, PUBLISHED_AT, DUE, REFUSED] {
            self.link_cfg_del(kind, link_id)?;
        }
        self.shared.links.inflight.remove(&link_id);
        self.shared.links.orphans.remove(&link_id);
        self.shared.links.expired.remove(&link_id);
        Ok(())
    }

    // ----- lookups --------------------------------------------------------------------------

    /// The feed a space's links belong to and are published through: the shared feed of a live shared
    /// space, the personal feed for a personal space; `None` for an archive.
    fn link_feed(&self, space_id: SpaceId) -> Result<Option<FeedId>, ApiError> {
        match self.shared_feed_of(space_id)? {
            Some(row) => Ok(self.feed_is_live(row.feed)?.then_some(row.feed)),
            None => self.personal_feed(),
        }
    }

    /// The link key of a link of `space_id`: `None` when the record is missing, tombstoned, wrapped under a
    /// key this device does not hold or damaged.
    fn link_key_of(
        &mut self,
        space_id: SpaceId,
        link_id: LinkId,
    ) -> Result<Option<LinkKey>, ApiError> {
        let Some(record) = self
            .security
            .keyring
            .record(space_id, &KeySubject::Link { link_id })
            .cloned()
        else {
            return Ok(None);
        };
        let Some(space_key) = self.space_key_at(space_id, record.wrap_epoch)? else {
            return Ok(None);
        };
        Ok(kr::unwrap_link_key(&space_key, &record).ok())
    }

    /// The account a device belongs to, from the device logs this device holds.
    fn link_device_account(&self, device: DeviceId) -> Option<AccountId> {
        if let Some(log) = self.security.log.as_ref()
            && log.device_keys(device).is_some()
        {
            return self.sync_identity().ok().flatten().map(|(me, _)| me);
        }
        self.shared
            .logs
            .iter()
            .find(|(_, log)| log.device_keys(device).is_some())
            .map(|(account, _)| *account)
    }

    /// Whether this device may publish links of the space: an Editor or an Owner of a live space.
    fn link_may_publish(&mut self, space_id: SpaceId) -> bool {
        self.require_role(space_id, MemberRole::Editor).is_ok()
    }

    /// The names the publication gives to the people a page mentions: the space's names (a former member
    /// keeps the last), this account's own.
    fn link_names(&self, space_id: SpaceId) -> HashMap<AccountId, String> {
        let mut names = self.space_people_names(space_id);
        if let Some(row) = self.shared_feed_of(space_id).ok().flatten()
            && let Some(meta) = self.shared.metas.get(&row.feed)
        {
            for (account, name) in &meta.former_names {
                names.entry(*account).or_insert_with(|| name.clone());
            }
        }
        names
    }

    // ----- the publication ------------------------------------------------------------------

    /// A page's document for a publication. With `create`, a page that has no document yet gets one (an
    /// empty page); without it, a page whose document is not on this device is `None`.
    fn link_page_content(
        &mut self,
        page_id: PageId,
        create: bool,
    ) -> Result<Option<PageContent>, ApiError> {
        let content_of = |model: &PageDoc| -> Result<PageContent, ApiError> {
            let snapshot = model
                .doc()
                .export_shallow_snapshot()
                .map_err(map_doc_error)?;
            let refs = model
                .attachment_refs()
                .into_iter()
                .map(|(_, attachment)| attachment)
                .collect();
            let mut mentions = BTreeMap::new();
            for mention in model.person_mentions() {
                mentions.entry(mention.account_id).or_insert(mention.label);
            }
            Ok(PageContent {
                snapshot,
                refs,
                mentions,
            })
        };
        if create {
            let open = self.open_page(page_id)?;
            return content_of(&open.model).map(Some);
        }
        if let Some(open) = self.docs.pages.get(&page_id) {
            return content_of(&open.model).map(Some);
        }
        let Some(row) = self
            .db
            .doc_get(DocId::for_page(page_id))
            .map_err(map_storage_error)?
        else {
            return Ok(None);
        };
        let doc = documents::load_doc(&self.db, &row, DocKind::Page, self.rng.as_ref())?;
        let model = PageDoc::from_doc(doc).map_err(map_model_error)?;
        content_of(&model).map(Some)
    }

    /// The manifest of an attachment, from this space's structure first, then from any other (an
    /// attachment that moved with its pages, ADR-0038).
    fn link_manifest(
        &self,
        space_id: SpaceId,
        attachment_id: AttachmentId,
    ) -> Option<kb_model::AttachmentManifest> {
        self.docs
            .structures
            .get(&space_id)
            .and_then(|open| open.model.attachment(attachment_id))
            .or_else(|| {
                self.docs
                    .structures
                    .values()
                    .find_map(|open| open.model.attachment(attachment_id))
            })
    }

    /// The publication of a page as it reads now (see the module documentation): `None` when the page
    /// is not live in the space or its document is not here. The flag says the attachments past
    /// `LINK_MAX_CHUNKS` chunks were left out.
    fn link_publication(
        &mut self,
        space_id: SpaceId,
        page_id: PageId,
        create: bool,
    ) -> Result<Option<(Publication, bool)>, ApiError> {
        let Some(info) = self
            .structure(space_id)?
            .model
            .page(page_id)
            .filter(|info| info.trashed_at.is_none())
        else {
            return Ok(None);
        };
        let Some(content) = self.link_page_content(page_id, create)? else {
            return Ok(None);
        };
        // The attachments the blocks, the icon and the cover reference.
        let mut ids = content.refs;
        ids.extend(info.icon.as_deref().and_then(validate::icon_attachment));
        ids.extend(info.cover.as_deref().and_then(validate::cover_attachment));
        let mut attachments = Vec::new();
        let mut chunks = 0usize;
        let mut over = false;
        for attachment_id in ids {
            let Some(manifest) = self.link_manifest(space_id, attachment_id) else {
                continue;
            };
            let Ok(key) = self.attachment_key(space_id, attachment_id) else {
                continue;
            };
            if chunks.saturating_add(manifest.chunks.len()) > LINK_MAX_CHUNKS {
                over = true;
                continue;
            }
            chunks += manifest.chunks.len();
            attachments.push(PublishedAttachment {
                attachment_id,
                key,
                name: clip(&manifest.name, PUBLICATION_SHORT_MAX_BYTES),
                mime: clip(&manifest.mime, PUBLICATION_SHORT_MAX_BYTES),
                size: manifest.size,
                hash: manifest.hash,
                chunk_size: manifest.chunk_size,
                chunks: manifest.chunks,
            });
        }
        // The people the page mentions, by the names the space knows them by: never a name nobody
        // mentioned.
        let names = self.link_names(space_id);
        let mentions = content
            .mentions
            .into_iter()
            .take(PUBLICATION_MAX_MENTIONS)
            .map(|(account_id, label)| {
                let name = names
                    .get(&account_id)
                    .cloned()
                    .or_else(|| (!label.trim().is_empty()).then_some(label))
                    .unwrap_or_else(|| crate::membership::fallback_name(account_id));
                PublishedPerson {
                    account_id,
                    name: clip(&name, PUBLICATION_SHORT_MAX_BYTES),
                }
            })
            .collect();
        let short = |text: Option<String>| {
            text.filter(|text| !text.is_empty() && text.len() <= PUBLICATION_SHORT_MAX_BYTES)
        };
        Ok(Some((
            Publication {
                page_id,
                title: clip(&info.title, PUBLICATION_TITLE_MAX_BYTES),
                icon: short(info.icon),
                cover: short(info.cover),
                cover_y: info.cover_y.unwrap_or(DEFAULT_COVER_Y),
                snapshot: content.snapshot,
                attachments,
                mentions,
            },
            over,
        )))
    }

    /// The upload of one link: its publication sealed under its key. `None` when it cannot be made now
    /// (see the module documentation, "No work is built for").
    fn link_put_work(
        &mut self,
        space_id: SpaceId,
        feed: FeedId,
        record: &ViewLinkRecord,
    ) -> Result<Option<LinkWork>, ApiError> {
        let Some(key) = self.link_key_of(space_id, record.link_id)? else {
            return Ok(None);
        };
        let Some((publication, _)) = self.link_publication(space_id, record.page_id, false)? else {
            return Ok(None);
        };
        let chunks = publication.chunk_ids();
        let sealed = seal_publication(&key, &publication, self.rng.as_ref())
            .map_err(|_| ApiError::internal("a publication could not be sealed"))?;
        Ok(Some(LinkWork::Put {
            link_id: record.link_id,
            put: LinkPut {
                feed,
                expires_at: record.expires_at,
                chunks,
                publication: sealed,
            },
        }))
    }

    // ----- ViewLinkCreate -------------------------------------------------------------------

    /// Makes a view link for a page (see the module documentation, "`ViewLinkCreate`").
    pub(crate) fn view_link_create(
        &mut self,
        space_id: SpaceId,
        page_id: PageId,
        expires_in_days: Option<u32>,
    ) -> Result<ViewLinkInfo, ApiError> {
        self.structure(space_id)?;
        // The role: an Owner, or an Editor when the space allows it (a personal space: its owner).
        self.require_role(space_id, MemberRole::Editor)?;
        if let Some(row) = self.shared_feed_of(space_id)? {
            let owner = self
                .role_in_space(space_id)?
                .is_some_and(MemberRole::is_owner);
            let allowed = self
                .shared
                .metas
                .get(&row.feed)
                .is_some_and(|meta| meta.settings.editors_may_link);
            if !owner && !allowed {
                return Err(ApiError::forbidden_role(MemberRole::Owner));
            }
        }
        let now = self.clock.now_ms();
        let expires_at = match expires_in_days {
            None => None,
            Some(days) if (1..=LINK_EXPIRY_MAX_DAYS).contains(&days) => {
                Some(now.saturating_add(i64::from(days) * DAY_MS))
            }
            Some(_) => {
                return Err(ApiError::invalid(
                    "expiresInDays",
                    format!("a link lasts 1 to {LINK_EXPIRY_MAX_DAYS} days, or never"),
                ));
            }
        };
        // One answer for every reason: a page that is not here, in the trash, or too heavy.
        let unlinkable = || ApiError::invalid("pageId", "this page cannot be shared by a link");
        let (_, over) = self
            .link_publication(space_id, page_id, true)?
            .ok_or_else(unlinkable)?;
        if over {
            return Err(unlinkable());
        }
        let Some(server) = self.sync_server_url()? else {
            return Err(ApiError::invalid(
                "serverUrl",
                "turn sync on before making a view link",
            ));
        };
        let Some((me, _)) = self.sync_identity()? else {
            return Err(ApiError::invalid(
                "serverUrl",
                "create your recovery kit before making a view link",
            ));
        };

        // The key, its record in the keyring, the map entry.
        let link_key = LinkKey::generate(self.rng.as_ref());
        let link_id = link_key.link_id();
        let epoch = self.space_epoch(space_id);
        let space_key = self.ensure_space_key(space_id)?;
        let record = kr::wrap_link_key(
            &link_key,
            &space_key,
            space_id,
            epoch,
            &self.security.device_keys,
            self.device.device_id,
            now,
            self.rng.as_ref(),
        );
        self.put_keyring_item(KeyringItem::Record(record))?;
        let entry = ViewLinkRecord {
            link_id,
            page_id,
            created_by: me,
            created_at: now,
            expires_at,
        };
        self.structure_mut(space_id)?
            .model
            .link_put(&entry)
            .map_err(map_model_error)?;
        self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        // The first upload is pending until the driver reports it.
        self.link_cfg_set(PENDING, link_id, "1")?;
        self.push_event_once(Event::ViewLinksChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        tracing::info!(link = %link_id.short_hex(), "a view link was made");
        // The one time the URL is shown: built here, handed to the caller, kept nowhere.
        let url = format!(
            "{}/view#{}",
            server.trim_end_matches('/'),
            link_key.to_fragment()
        );
        Ok(self.link_info(space_id, &entry, url, now))
    }

    // ----- ViewLinkList ---------------------------------------------------------------------

    /// The info of one link; `url` is the empty string everywhere but in the answer of `ViewLinkCreate`.
    fn link_info(
        &self,
        space_id: SpaceId,
        record: &ViewLinkRecord,
        url: String,
        now: i64,
    ) -> ViewLinkInfo {
        let shared = self.shared_feed_of(space_id).ok().flatten();
        let created_by_name = match &shared {
            Some(row) => self.member_name(row.feed, record.created_by),
            None => self
                .profile_name_of_account()
                .unwrap_or_else(|| crate::membership::fallback_name(record.created_by)),
        };
        let closed = shared.as_ref().is_some_and(|row| {
            matches!(
                self.feed_archived(row.feed),
                Ok(Some(ArchiveReason::Closed))
            )
        });
        let pending = self
            .link_cfg(PENDING, record.link_id)
            .ok()
            .flatten()
            .is_some();
        let status = if closed {
            ViewLinkStatus::Revoked
        } else if record.expires_at.is_some_and(|at| at <= now) {
            ViewLinkStatus::Expired
        } else if pending {
            ViewLinkStatus::Publishing
        } else {
            ViewLinkStatus::Live
        };
        ViewLinkInfo {
            link_id: record.link_id,
            page_id: record.page_id,
            url,
            created_by: record.created_by,
            created_by_name,
            created_at: record.created_at,
            expires_at: record.expires_at,
            last_published_at: self
                .link_cfg_i64(PUBLISHED_AT, record.link_id)
                .ok()
                .flatten(),
            status,
        }
    }

    /// The space's view links, newest first (see the module documentation, "`ViewLinkList`").
    pub(crate) fn view_link_list(
        &mut self,
        space_id: SpaceId,
    ) -> Result<Vec<ViewLinkInfo>, ApiError> {
        let records = self.structure(space_id)?.model.links();
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.clock.now_ms();
        let mut out = Vec::with_capacity(records.len());
        for record in records {
            let subject = KeySubject::Link {
                link_id: record.link_id,
            };
            if self.security.keyring.is_tombstoned(space_id, &subject)
                || self.security.keyring.record(space_id, &subject).is_none()
            {
                continue;
            }
            out.push(self.link_info(space_id, &record, String::new(), now));
        }
        Ok(out)
    }

    // ----- ViewLinkRevoke -------------------------------------------------------------------

    /// Revokes a link and answers the links that remain (see the module documentation,
    /// "`ViewLinkRevoke`").
    pub(crate) fn view_link_revoke(
        &mut self,
        space_id: SpaceId,
        link_id: LinkId,
    ) -> Result<Vec<ViewLinkInfo>, ApiError> {
        self.structure(space_id)?;
        // An archive is read-only.
        self.require_role(space_id, MemberRole::Viewer)?;
        let record = self
            .structure(space_id)?
            .model
            .link(link_id)
            .ok_or_else(|| ApiError::invalid("linkId", "this space has no such view link"))?;
        let me = self.me()?;
        if record.created_by != me && !self.may_purge_in(space_id) {
            return Err(ApiError::forbidden_role(MemberRole::Owner));
        }
        self.link_revoke_record(space_id, &record)?;
        self.view_link_list(space_id)
    }

    /// Removes a link: its map entry, its `Link` record (tombstoned where this device may), its local
    /// state, and queues the deletion of the server's row.
    fn link_revoke_record(
        &mut self,
        space_id: SpaceId,
        record: &ViewLinkRecord,
    ) -> Result<(), ApiError> {
        let link_id = record.link_id;
        self.structure_mut(space_id)?
            .model
            .link_remove(link_id)
            .map_err(map_model_error)?;
        self.persist_structure(space_id, Some(&[]), UpdateOrigin::Local)?;
        // Tombstones in a shared feed are an Owner's; a creator who is not one leaves it to an Owner's
        // device (`view_links_maintain`).
        let subject = KeySubject::Link { link_id };
        if self.may_purge_in(space_id) && !self.security.keyring.is_tombstoned(space_id, &subject) {
            let now = self.clock.now_ms();
            let tombstone = kr::tombstone(
                space_id,
                subject,
                &self.security.device_keys,
                self.device.device_id,
                now,
            );
            self.put_keyring_item(KeyringItem::Tombstone(tombstone))?;
        }
        self.link_state_clear(link_id)?;
        self.link_cfg_set(DELETE, link_id, "1")?;
        self.push_event_once(Event::ViewLinksChanged { space_id });
        self.push_event_once(Event::SyncRequested);
        tracing::info!(link = %link_id.short_hex(), "a view link was revoked");
        Ok(())
    }

    /// After a purge: the links of the purged pages go too (an Owner's or a personal device purges).
    /// Best effort: a failure is logged by its code and never undoes the purge.
    pub(crate) fn view_links_after_purge(&mut self, space_id: SpaceId, purged: &[PageId]) {
        let Some(open) = self.docs.structures.get(&space_id) else {
            return;
        };
        let doomed: Vec<ViewLinkRecord> = open
            .model
            .links()
            .into_iter()
            .filter(|record| purged.contains(&record.page_id))
            .collect();
        for record in doomed {
            if let Err(error) = self.link_revoke_record(space_id, &record) {
                tracing::warn!(code = ?error.code, "a view link of a purged page was not revoked");
            }
        }
    }

    /// Forgets the device-local state of a space's links (the local copy of the space is deleted): no
    /// server call, no tombstone.
    pub(crate) fn view_links_forget_space(&mut self, space_id: SpaceId) {
        let Some(open) = self.docs.structures.get(&space_id) else {
            return;
        };
        let ids: Vec<LinkId> = open.model.links().into_iter().map(|r| r.link_id).collect();
        for link_id in ids {
            if let Err(error) = self.link_state_clear(link_id) {
                tracing::warn!(code = ?error.code, "the state of a view link was not dropped");
            }
        }
    }

    // ----- refreshing -----------------------------------------------------------------------

    /// The refresh hook (see the module documentation, "Refreshing"): the page's document changed on
    /// this device, locally or through an applied entry. Cheap when the page has no link; never fails
    /// an edit, so callers ignore an `Err` (they log nothing but the code).
    pub(crate) fn view_links_page_changed(
        &mut self,
        page_id: PageId,
        source: ChangeSource,
    ) -> Result<(), ApiError> {
        // A comment is another document and never calls the hook; the source does not matter here.
        let _ = source;
        let Ok(space_id) = self.space_of_page(page_id) else {
            return Ok(());
        };
        self.view_links_pages_changed(space_id, &[page_id])
    }

    /// [`Self::view_links_page_changed`] for several pages of one space.
    pub(crate) fn view_links_pages_changed(
        &mut self,
        space_id: SpaceId,
        pages: &[PageId],
    ) -> Result<(), ApiError> {
        let Some(open) = self.docs.structures.get(&space_id) else {
            return Ok(());
        };
        let links: Vec<LinkId> = open
            .model
            .links()
            .into_iter()
            .filter(|record| pages.contains(&record.page_id))
            .map(|record| record.link_id)
            .collect();
        if links.is_empty() || !self.link_may_publish(space_id) {
            return Ok(());
        }
        let now = self.clock.now_ms();
        for link_id in links {
            self.link_note_change(link_id, now)?;
        }
        Ok(())
    }

    /// Notes a change of a link's page: due a minute after the first change not yet published.
    fn link_note_change(&mut self, link_id: LinkId, now: i64) -> Result<(), ApiError> {
        // A change after a refusal is a reason to try again.
        self.link_cfg_del(REFUSED, link_id)?;
        if let Some(redirty) = self.shared.links.inflight.get_mut(&link_id) {
            // The upload under way does not carry this change: it keeps a note when it is reported.
            redirty.get_or_insert(now);
            return Ok(());
        }
        if self.link_cfg(DUE, link_id)?.is_none() {
            let due = now.saturating_add(i64::try_from(LINK_REFRESH_DEBOUNCE_MS).unwrap_or(0));
            self.link_cfg_set(DUE, link_id, &due.to_string())?;
        }
        Ok(())
    }

    /// After a remote update of a structure document: `ViewLinksChanged` when its links changed, and a
    /// refresh noted for the linked pages whose title, icon or cover changed.
    pub(crate) fn view_links_structure_applied(
        &mut self,
        space_id: SpaceId,
        before: &LinkSnapshot,
    ) {
        let Some(open) = self.docs.structures.get(&space_id) else {
            return;
        };
        let after = link_snapshot(&open.model);
        if after.links != before.links {
            self.push_event_once(Event::ViewLinksChanged { space_id });
        }
        // A link revoked elsewhere: this device may have published it a moment ago (a refresh and the
        // revocation crossed), so the row is deleted again, and the state of the link goes.
        for gone in before
            .links
            .iter()
            .filter(|old| !after.links.iter().any(|now| now.link_id == old.link_id))
        {
            let published = self
                .link_cfg(PUBLISHED_AT, gone.link_id)
                .ok()
                .flatten()
                .is_some();
            let _ = self.link_state_clear(gone.link_id);
            if published {
                let _ = self.link_cfg_set(DELETE, gone.link_id, "1");
            }
        }
        let changed: Vec<PageId> = after
            .pages
            .iter()
            .filter(|(page, look)| before.pages.get(page).is_some_and(|old| old != *look))
            .map(|(page, _)| *page)
            .collect();
        if !changed.is_empty() {
            let _ = self.view_links_pages_changed(space_id, &changed);
        }
    }

    // ----- the Owner sweep and expiry -------------------------------------------------------

    /// The Owner-device sweep of orphaned `Link` records (see the module documentation, "Orphans and
    /// expiry"), called from `sync_feeds` on every cycle; it also announces the links that expired.
    pub(crate) fn view_links_maintain(&mut self) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        self.view_links_note_expiry(now);
        let rows: Vec<(SpaceId, FeedId)> = self
            .db
            .sync_feed_list()
            .map_err(map_storage_error)?
            .into_iter()
            .filter(|row| row.kind == kb_types::sync::FeedKind::Shared)
            .filter_map(|row| Some((row.space_id?, row.feed)))
            .collect();
        for (space_id, feed) in rows {
            if !self.feed_is_live(feed)? || !self.may_purge_in(space_id) {
                continue;
            }
            self.view_links_sweep(space_id, feed, now)?;
        }
        Ok(())
    }

    /// Announces each link that crossed its expiry since the last look, once.
    fn view_links_note_expiry(&mut self, now: i64) {
        let mut spaces = BTreeSet::new();
        for (space_id, open) in &self.docs.structures {
            for record in open.model.links() {
                if record.expires_at.is_some_and(|at| at <= now)
                    && self.shared.links.expired.insert(record.link_id)
                {
                    spaces.insert(*space_id);
                }
            }
        }
        for space_id in spaces {
            self.push_event_once(Event::ViewLinksChanged { space_id });
        }
    }

    /// Tombstones the `Link` records of one space whose creator is not an Owner and whose map entry is
    /// gone, once they have been orphans here for a pull interval.
    fn view_links_sweep(
        &mut self,
        space_id: SpaceId,
        feed: FeedId,
        now: i64,
    ) -> Result<(), ApiError> {
        let Some(open) = self.docs.structures.get(&space_id) else {
            return Ok(());
        };
        let entries: HashSet<LinkId> = open.model.links().into_iter().map(|r| r.link_id).collect();
        let Some(state) = self
            .shared
            .timelines
            .get(&feed)
            .map(|timeline| timeline.state().clone())
        else {
            return Ok(());
        };
        let candidates: Vec<(LinkId, DeviceId)> = self
            .security
            .keyring
            .space_records(space_id)
            .into_iter()
            .filter_map(|record| match record.subject {
                KeySubject::Link { link_id } => Some((link_id, record.created_by)),
                _ => None,
            })
            .collect();
        let wait = i64::try_from(PULL_INTERVAL_MS).unwrap_or(i64::MAX);
        for (link_id, signer) in candidates {
            if entries.contains(&link_id) {
                self.shared.links.orphans.remove(&link_id);
                continue;
            }
            let Some(account) = self.link_device_account(signer) else {
                continue;
            };
            if state.role_of(account).is_some_and(MemberRole::is_owner) {
                continue;
            }
            let first = *self.shared.links.orphans.entry(link_id).or_insert(now);
            if now.saturating_sub(first) < wait {
                continue;
            }
            let tombstone = kr::tombstone(
                space_id,
                KeySubject::Link { link_id },
                &self.security.device_keys,
                self.device.device_id,
                now,
            );
            self.put_keyring_item(KeyringItem::Tombstone(tombstone))?;
            self.shared.links.orphans.remove(&link_id);
            tracing::info!(link = %link_id.short_hex(), "an abandoned view link record was tombstoned");
        }
        Ok(())
    }

    // ----- the host side --------------------------------------------------------------------

    /// The link work due now for the driver (see the module documentation, "The host side"): `kb_net`'s
    /// `SyncHost::sync_link_work`.
    pub fn sync_link_work(&mut self) -> Result<Vec<LinkWork>, ApiError> {
        if self.sync_server_url()?.is_none() || self.sync_identity()?.is_none() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        // The deletions that survive restarts.
        let prefix = "links.x.";
        for (key, _) in self.db.sync_config_all().map_err(map_storage_error)? {
            if let Some(id) = key
                .strip_prefix(prefix)
                .and_then(|text| text.parse::<LinkId>().ok())
            {
                out.push(LinkWork::Delete { link_id: id });
            }
        }
        // The uploads: the first one of a link made here, and the refreshes whose time has come.
        let now = self.clock.now_ms();
        let spaces: Vec<SpaceId> = self
            .docs
            .structures
            .iter()
            .filter(|(_, open)| !open.model.links().is_empty())
            .map(|(space, _)| *space)
            .collect();
        for space_id in spaces {
            let Some(feed) = self.link_feed(space_id)? else {
                continue;
            };
            if !self.link_may_publish(space_id) {
                continue;
            }
            for record in self.structure(space_id)?.model.links() {
                if record.expires_at.is_some_and(|at| at <= now)
                    || self.link_cfg(REFUSED, record.link_id)?.is_some()
                {
                    continue;
                }
                let first = self.link_cfg(PENDING, record.link_id)?.is_some();
                let due = self
                    .link_cfg_i64(DUE, record.link_id)?
                    .is_some_and(|due| due <= now);
                if !first && !due {
                    continue;
                }
                match self.link_put_work(space_id, feed, &record) {
                    Ok(Some(work)) => {
                        self.shared.links.inflight.insert(record.link_id, None);
                        out.push(work);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(link = %record.link_id.short_hex(), code = ?error.code,
                            "a view link publication could not be made");
                    }
                }
            }
        }
        Ok(out)
    }

    /// How the server answered a piece of link work: `kb_net`'s `SyncHost::sync_link_done`.
    pub fn sync_link_done(
        &mut self,
        link_id: LinkId,
        outcome: LinkOutcome,
    ) -> Result<(), ApiError> {
        let upload = self.shared.links.inflight.remove(&link_id);
        let revoked = self.link_cfg(DELETE, link_id)?.is_some();
        match outcome {
            LinkOutcome::Deleted => self.link_cfg_del(DELETE, link_id),
            // A `Refused` with no upload under way ends a deletion.
            LinkOutcome::Refused if upload.is_none() => self.link_cfg_del(DELETE, link_id),
            // The link was revoked while its upload was under way: the deletion follows.
            _ if revoked => Ok(()),
            LinkOutcome::Refused => {
                // Offers stop until a change is noted again; the creator keeps seeing `Publishing`.
                self.link_cfg_set(REFUSED, link_id, "1")
            }
            LinkOutcome::Published { pub_seq } => {
                let _ = pub_seq;
                // The link was revoked on another device while the upload was under way: the row the
                // upload made is deleted again.
                if self.link_space_of(link_id).is_none() {
                    self.link_state_clear(link_id)?;
                    return self.link_cfg_set(DELETE, link_id, "1");
                }
                let now = self.clock.now_ms();
                let was_pending = self.link_cfg(PENDING, link_id)?.is_some();
                self.link_cfg_set(PUBLISHED_AT, link_id, &now.to_string())?;
                self.link_cfg_del(PENDING, link_id)?;
                self.link_cfg_del(REFUSED, link_id)?;
                match upload.flatten() {
                    // A change arrived while the upload was under way: due a minute after it.
                    Some(changed_at) => {
                        let due = changed_at
                            .saturating_add(i64::try_from(LINK_REFRESH_DEBOUNCE_MS).unwrap_or(0));
                        self.link_cfg_set(DUE, link_id, &due.to_string())?;
                    }
                    None => self.link_cfg_del(DUE, link_id)?,
                }
                if was_pending && let Some(space_id) = self.link_space_of(link_id) {
                    self.push_event_once(Event::ViewLinksChanged { space_id });
                }
                Ok(())
            }
        }
    }

    /// The space whose structure lists a link.
    fn link_space_of(&self, link_id: LinkId) -> Option<SpaceId> {
        self.docs
            .structures
            .iter()
            .find(|(_, open)| open.model.link(link_id).is_some())
            .map(|(space, _)| *space)
    }
}
