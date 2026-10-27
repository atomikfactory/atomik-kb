//! The seam between the core and the sync driver, protocol v2 (M13, ADR-0045 section 9,
//! `docs/development/milestone-13.md` sections 1.5 and 1.6): the plain data `kb_core::Core`'s per-feed
//! sync methods answer and `kb-net`'s `SyncHost` (v2) hands to the driver. Both crates depend on
//! `kb-sync`, so the types live here once (v1 defines them twice, in `kb-core` and `kb-net`, and the
//! shell converts). Pure data.

use kb_types::sync::FeedKind;
use kb_types::{AttachmentId, ChunkId, DocId, FeedId, LinkId, SpaceId};

use super::LinkPut;

/// A feed the driver works this cycle, as the core's `sync_feeds()` lists it (the personal feed
/// first, then the shared feeds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedWork {
    /// The feed.
    pub feed: FeedId,
    /// Personal or shared.
    pub kind: FeedKind,
    /// The space of a shared feed (the server never learns it).
    pub space_id: Option<SpaceId>,
    /// The last feed cursor this device applied (the `after` of the next `changes`).
    pub cursor: u64,
    /// An Owner closed the feed: the driver only reads it.
    pub closed: bool,
    /// The server's history of this feed does not match this device's: the driver does not upload.
    pub forked: bool,
}

/// One sealed entry ready to upload to a feed (document or keyring entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundEntry {
    /// The document; `KEYRING_DOC_ID` for a keyring entry.
    pub doc_id: DocId,
    /// The writer device's sequence for the document.
    pub device_seq: u64,
    /// The sealed entry.
    pub bytes: Vec<u8>,
    /// The last local update sequence this entry carries (`sync_pushed` marks it).
    pub up_to_seq: i64,
}

/// What one applied page of changes did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncApplied {
    /// Items applied.
    pub applied: u32,
    /// Items this device wrote itself.
    pub skipped_own: u32,
    /// Items parked until a key, a document or history arrives.
    pub pending: u32,
    /// Items refused: bad, or quarantined for a role or a membership violation (the chain still
    /// advanced past them).
    pub rejected: u32,
    /// The feed cursor after the page.
    pub new_cursor: u64,
    /// Cursor gaps seen (the server pruned entries): the driver fetches the snapshots again.
    pub gaps: u32,
}

/// What one applied page of snapshots did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapshotsApplied {
    /// Snapshots applied.
    pub applied: u32,
    /// Snapshots that failed the dominance check (they omit entries they cover), as `(document,
    /// covers_cursor)`: never applied; the driver posts `snapshot_dispute` for each and the core
    /// republishes a full snapshot.
    pub disputed: Vec<(DocId, u64)>,
}

/// What the driver does with one chunk of a feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedChunkKind {
    /// The chunk is here and the feed's namespace lacks it: `put_chunk`.
    Upload,
    /// The chunk is wanted here: `get_chunk`.
    Download,
    /// A purge by an Owner here: `delete_chunk`.
    Delete,
    /// The chunk sits in this account's personal namespace and the shared feed lacks it (the share
    /// step, a move into a shared space): `copy_chunk`.
    Copy,
}

/// One item of chunk work of a feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedChunkWork {
    /// The feed whose namespace the chunk is in.
    pub feed: FeedId,
    /// The chunk.
    pub hash: ChunkId,
    /// The attachment it belongs to.
    pub attachment_id: AttachmentId,
    /// What to do.
    pub kind: FeedChunkKind,
}

/// What an invitee's device needs to create its account through a share invite
/// (`RemoteStore::account_create_share`): its own device log and the signature of
/// `kb_keys::invite::sign_signup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteSignup {
    /// The invite.
    pub invite_id: kb_types::InviteId,
    /// The device log, encoded entries, genesis first.
    pub log: Vec<Vec<u8>>,
    /// The signup claim.
    pub signature: kb_crypto::Signature,
}

/// What an invitee's device sends to open an invite (`RemoteStore::invite_open`): the signature of
/// `kb_keys::invite::sign_open`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteOpenRequest {
    /// The invite.
    pub invite_id: kb_types::InviteId,
    /// The open request's signature.
    pub signature: kb_crypto::Signature,
}

/// Membership entries minted for a feed that the server has not taken yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMembership {
    /// The feed.
    pub feed: FeedId,
    /// The encoded entries (`MembershipEntry::encode`), in log order.
    pub entries: Vec<Vec<u8>>,
}

// ----- slice 3: realtime and presence (ADR-0050) ----------------------------------------------

/// What offering one inline entry (`kb_sync::v2::ws::InlineEntry`) to the core did
/// (`kb_core::Core::sync_feed_apply_inline`). An error answer is for a failure of the core itself
/// (storage, a lock), never for a bad entry: a bad entry is quarantined like a pulled one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineOutcome {
    /// The entry was the next item of the feed (`prev_cursor` was this device's cursor, and the chain
    /// hash after it matched): it went through the apply path of a pulled item (chain, signature,
    /// membership at the cursor, role gating) and was stored and applied in the same transaction.
    /// The counts say what it did (a verification failure shows as `rejected`, an entry of this
    /// device's own as `skipped_own`; `new_cursor` is the feed's cursor now).
    Applied(SyncApplied),
    /// This device is at or past the entry's cursor already (a duplicate or a late frame): nothing
    /// was done.
    Known,
    /// The entry cannot be applied from here: this device is not at `prev_cursor` (a missed or
    /// reordered frame), the chain hash after it does not match, the feed is not live or has not
    /// finished its first read, or the item needs something the pull has first (a device log, a
    /// membership entry). Nothing was stored; the driver schedules an ordinary pull of the feed, and
    /// the entry comes with it.
    Pull,
}

/// A presence frame sealed by the core (`kb_core::Core::presence_seal`), ready for the socket:
/// `kb-net` frames it as `2 | feed | envelope` (`kb_sync::v2::ws::PresenceFrame`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPresence {
    /// The shared feed of the space.
    pub feed: FeedId,
    /// `kb_sync::v2::seal_presence` output (an envelope of kind `Presence`).
    pub envelope: Vec<u8>,
}

// ----- slice 4: view links (ADR-0051) ---------------------------------------------------------

/// One piece of view-link work the core hands the driver (`kb_core::Core::sync_link_work`, run after
/// the chunks of the feed so that the attachments a publication declares are there): upload a
/// publication, or delete a link that was revoked here. The driver answers each with
/// [`LinkOutcome`] through `sync_link_done`, except for a transient failure (the network, a 5xx, a
/// 429): then it says nothing and the same work is offered again in the next cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkWork {
    /// `RemoteStore::link_put(link_id, &put)`: create or refresh the publication.
    Put {
        /// The link.
        link_id: LinkId,
        /// The upload (the feed, the expiry, the declared chunks and the sealed publication).
        put: LinkPut,
    },
    /// `RemoteStore::link_delete(link_id)`: the link was revoked on this device and the server may
    /// still hold its row.
    Delete {
        /// The link.
        link_id: LinkId,
    },
}

impl LinkWork {
    /// The link the work is about.
    #[must_use]
    pub const fn link_id(&self) -> LinkId {
        match self {
            Self::Put { link_id, .. } | Self::Delete { link_id } => *link_id,
        }
    }
}

/// How the server answered a piece of [`LinkWork`] (`kb_core::Core::sync_link_done`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOutcome {
    /// The server took the publication; its counter for the link (`ViewLinkInfo` becomes `Live`, the
    /// time of the upload is `last_published_at`).
    Published {
        /// The server's `pub_seq` for the link.
        pub_seq: u64,
    },
    /// The server no longer holds the link: it deleted the row, or answered 404 (a link that is gone
    /// is revoked all the same).
    Deleted,
    /// The server refused for good: 403 (this account may not publish in the feed any more) or 404
    /// for a `Put` (the feed is gone for this account), 409 `feed_closed` or a feed conflict. The core
    /// stops offering the upload until something changes (a role or a new refresh).
    Refused,
}
