//! The reference remote of protocol v2 (M13, ADR-0045, `docs/development/milestone-13.md` section 1.3):
//! an in-memory server with an access list that the fleet simulator of `kb-core`
//! (`tests_sharing.rs`) and the conformance suite run against. It implements [`RemoteStore`] and,
//! like v1's, a synchronous twin `<method>_sync` of every trait method (same arguments, same
//! answer) for the simulator, which cannot await.
//!
//! It keeps: feeds and their kinds; the folded membership log of each shared feed ([`FeedRules`]:
//! roles, joined and removed cursors); compare-and-swap on the membership head and atomic batches;
//! invites (open, use, revoke, expire on its own clock); share signups and their limits;
//! `pending_rekey` and `stale_epoch`; the pruning rule of ADR-0045 (acknowledgements, idle devices,
//! disputes); quota per billing account (passed to the earliest Owner when the creator leaves);
//! chunk copy; per-feed heads and chains; closed feeds. The knobs behind the `testing` feature make
//! it misbehave on demand for the adversarial suite ([`Adversary`]).
//!
//! # Connections
//!
//! A [`MemoryRemote`] value is one client connection to a shared server. Cloning it opens a **new
//! connection** to the same server: the clone shares every account, feed and session table but
//! starts unauthenticated, exactly as a second `HttpRemote` would. Give each simulated device its
//! own clone and authenticate it once; one handle shared by several devices works too if each
//! authenticates before its calls (the last `authenticate` wins). The per-client limits (30
//! `invite_open` calls per hour) count per connection.
//!
//! # Time
//!
//! The server clock (`now_ms`, 0 until a test sets it with [`MemoryRemote::set_now_ms`]) decides
//! invite expiry and the invite horizon, snapshot arrival times for pruning, mailbox expiry, device
//! idleness and the Rekey rate limit. `pair_messages` never waits: it answers at once whatever
//! `wait_seconds` says.
//!
//! # Realtime
//!
//! [`MemoryRemote::ws_open`] opens a [`MemorySocket`]: the in-memory twin of `GET /v2/ws` with the same
//! rules as `kb-server` (see `memory/socket.rs` and [`super::ws`]). An append, a membership
//! append and nothing else pushes frames; they are queued on the sockets at the moment the call
//! returns, so a test reads them with `recv` straight after. The knobs `ws_drop_next` and
//! `ws_swap_next_two` lose and reorder frames, `ws_open_count` counts the open sockets.
//!
//! # Feed semantics
//!
//! Cursors come from one counter shared by every feed (so a feed's cursors increase and may leave
//! gaps, like the real server's autoincrement). The chain hash of a feed starts at `chain_genesis` of
//! the server epoch and advances over every appended item's bytes, membership entries included.
//! Pruned entries keep their place in the chain but are no longer returned.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use kb_crypto::{Signature, VerifyingKey};
use kb_keys::membership::{EntryBody, EntryKind, LogSource, MembershipEntry};
use kb_keys::{DeviceLog, LogEntry, LogEntryBody, LogError, LogState};
use kb_types::devices::{
    MAILBOX_ID_LEN, PAIRING_MAX_MESSAGE_BYTES, PAIRING_MAX_MESSAGES, PAIRING_TTL_SECONDS,
    RECOVERY_BLOB_MAX_BYTES,
};
use kb_types::sharing::{
    INVITE_OPEN_PER_IP_PER_HOUR, LINK_GETS_PER_MINUTE_PER_IP, LINK_MAX_CHUNKS,
    MAX_FEEDS_PER_ACCOUNT, SHARE_SIGNUPS_DEFAULT, SHARE_SIGNUPS_PER_ACCOUNT_DEFAULT,
};
use kb_types::sync::{CHANGES_PAGE_LIMIT, FeedKind, KEYRING_DOC_ID, MAX_WIRE_BYTES};
use kb_types::{AccountId, ChunkId, DeviceId, DocId, FeedId, InviteId, LinkId, MemberRole};

use super::guard::{Access, BatchContext, FeedRules};
use super::{
    FeedCreated, FeedListing, InviteOpened, ItemKind, LinkFetch, LinkPut, LogChunk, LogRequest,
    MarksPage, MembershipHead, MembershipItem, MembershipPage, PAIR_ROLE_OPENER, PAIR_ROLE_TYPIST,
    RemoteStore, ServerInfo, SnapshotMark, chain_genesis, chain_next, peek_feed_item, peek_head,
    peek_publication, peek_snapshot, verify_head, verify_signature,
};
use crate::{
    ChangesPage, PairingMessage, PairingPage, SnapshotsPage, SyncError, auth_message,
    check_item_size, verify_auth,
};

mod socket;
#[cfg(test)]
mod tests;

pub use socket::MemorySocket;

/// Invite token a new remote accepts until [`MemoryRemote::set_invite_tokens`] changes the set.
pub const DEFAULT_INVITE_TOKEN: &str = "dev";

/// Milliseconds per day.
const DAY_MS: i64 = 86_400_000;
/// Milliseconds per hour.
const HOUR_MS: i64 = 3_600_000;
/// Milliseconds per minute.
const MINUTE_MS: i64 = 60_000;

/// How the remote misbehaves on demand (set with `MemoryRemote::set_adversary`, behind the `testing`
/// feature): the adversarial suite of `milestone-13.md`, 1.8. One mode at a time; `None` restores
/// honest behaviour. Every mode except [`Adversary::AcceptAll`] and
/// [`Adversary::AddMemberWithoutEntry`] changes only what reads answer; the stored feed stays
/// intact, and a page's chain hash is recomputed over exactly the items the adversary returns (a
/// lying server keeps its pages self-consistent, so detection has to come from signatures, the
/// fold, sequences and heads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adversary {
    /// Serves membership entries without the ones from `from_seq` on (a dropped tail or a hidden
    /// `Remove`).
    DropMembership {
        /// The first `seq` withheld.
        from_seq: u64,
    },
    /// Serves two adjacent membership entries in the wrong order: the bytes of the last two swap
    /// places, the cursors stay.
    ReorderMembership,
    /// Adds `account` to the access list without any log entry (a server that adds a member): it
    /// reads and appends as an Editor.
    AddMemberWithoutEntry {
        /// The account the server pretends is a member.
        account: AccountId,
    },
    /// Serves two different membership logs to two readers (a fork): every reader except the
    /// feed's billing account (its creator, until it leaves) misses the newest membership entry.
    ForkMembership,
    /// Serves a device log of `account` without its entries from `keep` on (a withheld revocation).
    WithholdLogTail {
        /// The account.
        account: AccountId,
        /// How many entries are served.
        keep: u64,
    },
    /// Serves, for a document that has been snapshotted twice, the older snapshot instead of the
    /// newest (the newest "omits" what only it covers).
    TruncateSnapshot,
    /// Serves an invite that expired as if it were open: the server clock is not consulted for
    /// invites (`invite_open`, the signup, the `Join`).
    ServeExpiredInvite,
    /// Accepts anything: appends, snapshots, heads and chunks skip the access list, the signature
    /// checks, `pending_rekey`, `stale_epoch` and `closed`; membership entries are stored without
    /// the fold (and change no state). Clients must reject what they receive.
    AcceptAll,
    /// Forks the feed: a reader sees the items appended after the mode was set only when its own
    /// device wrote them (every device sees its own tail).
    ForkFeed,
    /// Hides every `Remove` and `Leave` entry from every reader but its signer.
    HideMembership,
    /// Repeats the oldest document entry of every feed once more: unchanged (a valid signature, a
    /// repeated `device_seq`), at a cursor of its own after the entries that existed when the mode
    /// was set.
    Replay,
    /// Adds after the entries that existed when the mode was set a copy of each feed's newest
    /// document entry whose `device_seq` is raised by one, so its signature no longer verifies. It
    /// takes a cursor of its own (real entries never get it).
    Forge,
    /// Answers every feed as if its last `n` items did not exist.
    Rollback(u64),
    /// Leaves the items of this document out of every page of every feed.
    Withhold {
        /// The withheld document.
        doc: DocId,
    },
}

/// The chunk calls that reached the store, per kind (see [`MemoryRemote::chunk_calls`]).
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChunkCalls {
    /// `put_chunk` calls.
    pub puts: u64,
    /// `get_chunk` calls.
    pub gets: u64,
    /// `delete_chunk` calls.
    pub deletes: u64,
    /// `copy_chunk` calls.
    pub copies: u64,
}

/// The state of the chunk knobs, shared by every connection.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
struct ChunkKnobs {
    /// Chunk calls still to fail with `Unavailable("injected")`.
    failures: u32,
    calls: ChunkCalls,
}

/// The kind of a chunk call.
#[derive(Clone, Copy)]
enum ChunkOp {
    Put,
    Get,
    Delete,
    Copy,
}

/// What the knobs of the `testing` feature hold; without the feature nothing sets them.
#[derive(Default)]
#[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
struct Knobs {
    adversary: Option<Adversary>,
    /// Membership compare-and-swaps still to lose, by feed.
    lost_cas: HashMap<FeedId, u32>,
    /// `(account, device)` pairs that count as idle for pruning whatever the clock says.
    idle: HashSet<(AccountId, DeviceId)>,
    /// The newest cursor of a feed when the adversary was set (for [`Adversary::ForkFeed`]).
    fork_from: HashMap<FeedId, u64>,
    /// Realtime frames still to lose (`MemoryRemote::ws_drop_next`).
    ws_drop: usize,
    /// The reordering of the next two frames a socket gets (`MemoryRemote::ws_swap_next_two`).
    ws_swap: socket::WsSwap,
    /// The item an adversary adds to a feed ([`Adversary::Replay`], [`Adversary::Forge`]) and the
    /// cursor it took.
    injected: HashMap<FeedId, (u64, Vec<u8>)>,
    #[cfg(any(test, feature = "testing"))]
    chunks: ChunkKnobs,
}

/// A pairing mailbox.
struct Mailbox {
    expires_at: i64,
    closed: bool,
    /// `(role, bytes)` in sequence order; message 0 is the offer.
    messages: Vec<(u8, Vec<u8>)>,
}

/// A stored recovery blob and the account whose session stored it.
struct RecoveryBlob {
    owner: AccountId,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Session {
    account: AccountId,
    device: DeviceId,
}

struct Account {
    log: DeviceLog,
    state: LogState,
    /// When each device last opened a session (server clock).
    last_seen: HashMap<DeviceId, i64>,
    /// The invite whose share signup created this account.
    created_via: Option<InviteId>,
}

/// One stored feed item.
#[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
struct Item {
    cursor: u64,
    kind: ItemKind,
    doc_id: DocId,
    account: AccountId,
    device: DeviceId,
    /// `device_seq` of an entry, the `seq` of a membership entry.
    seq: u64,
    bytes: Vec<u8>,
    /// Running chain hash after this item.
    chain: [u8; 32],
    pruned: bool,
    /// Server clock at arrival (kept for inspection with a debugger).
    #[allow(dead_code)]
    at_ms: i64,
}

/// The latest snapshot of a document.
#[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
struct SnapRow {
    bytes: Vec<u8>,
    covers: u64,
    /// The cover as pruning trusts it: never beyond the feed's newest cursor when it arrived.
    prune_covers: u64,
    /// Who published it (kept for inspection with a debugger).
    #[allow(dead_code)]
    device: DeviceId,
    #[allow(dead_code)]
    account: AccountId,
    arrived_at: i64,
    /// The snapshot it replaced (served by [`Adversary::TruncateSnapshot`]).
    previous: Option<Vec<u8>>,
}

struct Chunk {
    bytes: Vec<u8>,
}

/// A view link (M13 slice 4): the sealed publication as it was uploaded, the chunk hashes it
/// declared and what the first upload fixed.
struct LinkRow {
    /// The feed the link's page lives in.
    feed: FeedId,
    /// The account of the session that made the first upload.
    created_by: AccountId,
    publication: Vec<u8>,
    /// 1 at the first upload, +1 per upload.
    pub_seq: u64,
    /// When the link stops working (the server clock, Unix ms); fixed by the first upload.
    expires_at: Option<i64>,
    /// The chunks the link may serve (while the feed holds them).
    chunks: BTreeSet<ChunkId>,
}

struct Feed {
    kind: FeedKind,
    /// The account whose quota the feed's bytes count against.
    billing: AccountId,
    items: Vec<Item>,
    /// `(kind, doc, device, seq)` -> index in `items`.
    seen: HashMap<(ItemKind, DocId, DeviceId, u64), usize>,
    /// Chain hash after the newest item.
    chain: [u8; 32],
    /// The membership view of a shared feed; `None` for a personal feed.
    rules: Option<FeedRules>,
    closed_at: Option<i64>,
    snapshots: BTreeMap<DocId, SnapRow>,
    marks: Vec<SnapshotMark>,
    next_mark: u64,
    /// `(document, covers_cursor)` of snapshots that are disputed.
    disputes: BTreeSet<(DocId, u64)>,
    chunks: BTreeMap<ChunkId, Chunk>,
    /// Latest signed head per device with the cursor it states.
    heads: BTreeMap<DeviceId, (Vec<u8>, u64)>,
    bytes_used: u64,
}

impl Feed {
    fn new(kind: FeedKind, billing: AccountId, epoch: &[u8; 16]) -> Self {
        Self {
            kind,
            billing,
            items: Vec::new(),
            seen: HashMap::new(),
            chain: chain_genesis(epoch),
            rules: None,
            closed_at: None,
            snapshots: BTreeMap::new(),
            marks: Vec::new(),
            next_mark: 0,
            disputes: BTreeSet::new(),
            chunks: BTreeMap::new(),
            heads: BTreeMap::new(),
            bytes_used: 0,
        }
    }

    fn last_cursor(&self) -> u64 {
        self.items.last().map_or(0, |item| item.cursor)
    }

    /// Stores an item at `cursor`, advancing the chain.
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        cursor: u64,
        kind: ItemKind,
        doc_id: DocId,
        account: AccountId,
        device: DeviceId,
        seq: u64,
        bytes: &[u8],
        at_ms: i64,
    ) -> [u8; 32] {
        let chain = chain_next(&self.chain, bytes);
        self.chain = chain;
        self.seen
            .insert((kind, doc_id, device, seq), self.items.len());
        self.items.push(Item {
            cursor,
            kind,
            doc_id,
            account,
            device,
            seq,
            bytes: bytes.to_vec(),
            chain,
            pruned: false,
            at_ms,
        });
        self.bytes_used = self.bytes_used.saturating_add(len_u64(bytes.len()));
        chain
    }
}

/// The server state every connection shares.
struct Inner {
    /// Random at creation; `reset_data` draws a new one.
    server_epoch: [u8; 16],
    /// Seed of the internal generator (BLAKE3 in counter mode) for epochs, nonces and tokens.
    seed: [u8; 32],
    counter: u64,
    invite_tokens: Vec<String>,
    account_quota: Option<u64>,
    share_signups: u32,
    share_signups_per_account: u32,
    idle_device_days: Option<u32>,
    accounts: BTreeMap<AccountId, Account>,
    /// Device ids across accounts (they are unique on a server).
    devices: HashMap<DeviceId, AccountId>,
    sessions: HashMap<[u8; 32], Session>,
    /// The server clock, Unix ms.
    now_ms: i64,
    offline: bool,
    /// The last feed cursor handed out (one counter for every feed).
    last_cursor: u64,
    /// Connections opened so far (the per-client limits count per connection).
    connections: u64,
    feeds: BTreeMap<FeedId, Feed>,
    /// Which feed holds each invite.
    invites: HashMap<InviteId, FeedId>,
    /// Times of the `invite_open` calls of each connection.
    open_calls: HashMap<u64, VecDeque<i64>>,
    /// The view links by id (M13 slice 4).
    links: BTreeMap<LinkId, LinkRow>,
    /// Times of the link reads (publications and chunks together) of each connection.
    link_reads: HashMap<u64, VecDeque<i64>>,
    mailboxes: HashMap<[u8; MAILBOX_ID_LEN], Mailbox>,
    recovery: HashMap<[u8; 32], RecoveryBlob>,
    knobs: Knobs,
    /// The open realtime sockets (`MemoryRemote::ws_open`), by id.
    sockets: BTreeMap<u64, socket::SocketSlot>,
    /// The id of the next socket.
    next_socket: u64,
}

fn len_u64(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// The device logs of the accounts, as the fold reads them.
struct Logs<'a>(&'a BTreeMap<AccountId, Account>);

impl LogSource for Logs<'_> {
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog> {
        self.0.get(&account).map(|stored| &stored.log)
    }
}

/// One item as a reader sees it.
struct ViewItem<'a> {
    cursor: u64,
    kind: ItemKind,
    seq: u64,
    bytes: &'a [u8],
    pruned: bool,
    chain: [u8; 32],
}

/// Who is reading, for the adversary: the session of the call, `None` for a call without one.
type Reader<'a> = Option<&'a Session>;

impl Inner {
    fn random<const N: usize>(&mut self) -> [u8; N] {
        let mut input = [0u8; 40];
        input[..32].copy_from_slice(&self.seed);
        input[32..].copy_from_slice(&self.counter.to_le_bytes());
        self.counter = self.counter.wrapping_add(1);
        let digest = kb_crypto::hash(&input);
        let mut out = [0u8; N];
        for (byte, value) in out.iter_mut().zip(digest.iter().cycle()) {
            *byte = *value;
        }
        out
    }

    fn check_online(&self) -> Result<(), SyncError> {
        if self.offline {
            Err(SyncError::Unavailable("the server is offline".to_owned()))
        } else {
            Ok(())
        }
    }

    fn adversary(&self) -> Option<Adversary> {
        self.knobs.adversary
    }

    fn accept_all(&self) -> bool {
        self.knobs.adversary == Some(Adversary::AcceptAll)
    }

    fn serve_expired(&self) -> bool {
        self.knobs.adversary == Some(Adversary::ServeExpiredInvite)
    }

    fn next_cursor(&mut self) -> u64 {
        self.last_cursor = self.last_cursor.saturating_add(1);
        self.last_cursor
    }

    /// The start of every chunk call: in testing builds a failure injected with `fail_chunk_ops`
    /// answers here, before anything else.
    fn chunk_call_starts(&mut self) -> Result<(), SyncError> {
        #[cfg(any(test, feature = "testing"))]
        {
            if self.knobs.chunks.failures > 0 {
                self.knobs.chunks.failures -= 1;
                return Err(SyncError::Unavailable("injected".to_owned()));
            }
        }
        Ok(())
    }

    /// A chunk call got past the offline and session checks: in testing builds it is counted for
    /// `chunk_calls`.
    fn chunk_call_reached(&mut self, op: ChunkOp) {
        #[cfg(any(test, feature = "testing"))]
        {
            let calls = &mut self.knobs.chunks.calls;
            let counter = match op {
                ChunkOp::Put => &mut calls.puts,
                ChunkOp::Get => &mut calls.gets,
                ChunkOp::Delete => &mut calls.deletes,
                ChunkOp::Copy => &mut calls.copies,
            };
            *counter = counter.saturating_add(1);
        }
        #[cfg(not(any(test, feature = "testing")))]
        let _ = op;
    }

    fn account(&self, id: AccountId) -> Result<&Account, SyncError> {
        self.accounts.get(&id).ok_or(SyncError::NotFound)
    }

    /// The signing key of a device that is active and, when idle expiry is on, not idle.
    fn admissible_key(
        &self,
        account: AccountId,
        device: DeviceId,
    ) -> Result<VerifyingKey, SyncError> {
        let stored = self.account(account)?;
        let key = stored
            .state
            .active(device)
            .map(|record| record.keys.signing)
            .ok_or(SyncError::Unauthorized)?;
        if let Some(days) = self.idle_device_days {
            let last_seen = stored
                .last_seen
                .get(&device)
                .copied()
                .unwrap_or(self.now_ms);
            let limit = i64::from(days).saturating_mul(DAY_MS);
            if self.now_ms.saturating_sub(last_seen) > limit {
                return Err(SyncError::Idle);
            }
        }
        Ok(key)
    }

    /// The live mailbox `id`; expired ones are `NotFound` (and forgotten).
    fn mailbox(&mut self, id: [u8; MAILBOX_ID_LEN]) -> Result<&mut Mailbox, SyncError> {
        let now = self.now_ms;
        if self.mailboxes.get(&id).is_some_and(|m| now >= m.expires_at) {
            self.mailboxes.remove(&id);
        }
        self.mailboxes.get_mut(&id).ok_or(SyncError::NotFound)
    }

    /// The standing of the session's account in a feed (a personal feed belongs to its account, an
    /// unknown feed is a stranger's).
    fn access(&self, session: &Session, feed: FeedId) -> Access {
        if self.accept_all() {
            return Access::Member(MemberRole::Owner);
        }
        let Some(stored) = self.feeds.get(&feed) else {
            return Access::Stranger;
        };
        match &stored.rules {
            None => {
                if feed == FeedId::from_account(session.account) {
                    Access::Member(MemberRole::Owner)
                } else {
                    Access::Stranger
                }
            }
            Some(rules) => {
                let access = rules.access(session.account);
                if access == Access::Stranger
                    && self.knobs.adversary
                        == Some(Adversary::AddMemberWithoutEntry {
                            account: session.account,
                        })
                {
                    return Access::Member(MemberRole::Editor);
                }
                access
            }
        }
    }

    /// The feed the session may use at `min`, with its access: `NotFound` for a stranger or a removed
    /// account, `Forbidden` for a role that is too low.
    fn feed_for(
        &self,
        session: &Session,
        feed: FeedId,
        min: MemberRole,
    ) -> Result<Access, SyncError> {
        let access = self.access(session, feed);
        access.require(min)?;
        if !self.feeds.contains_key(&feed) {
            return Err(SyncError::NotFound);
        }
        Ok(access)
    }

    /// Bytes counted against an account: its personal feed and the feeds it is billed for.
    fn billed(&self, account: AccountId) -> u64 {
        self.feeds
            .values()
            .filter(|feed| feed.billing == account)
            .map(|feed| feed.bytes_used)
            .fold(0, u64::saturating_add)
    }

    /// Refuses `extra` more bytes (after `freed` are given back) beyond the quota of `billing`.
    fn check_quota(&self, billing: AccountId, extra: u64, freed: u64) -> Result<(), SyncError> {
        let after = self
            .billed(billing)
            .saturating_sub(freed)
            .saturating_add(extra);
        if self.account_quota.is_some_and(|quota| after > quota) {
            Err(SyncError::Quota)
        } else {
            Ok(())
        }
    }

    /// Whether a shared feed waits for a `Rekey`.
    fn pending_rekey(&self, feed: FeedId) -> bool {
        self.feeds
            .get(&feed)
            .and_then(|stored| stored.rules.as_ref())
            .is_some_and(|rules| rules.pending_rekey(&Logs(&self.accounts)))
    }

    /// The invite if it can still be redeemed: open in its feed's log, the feed is not closed and the
    /// server clock has not passed its expiry.
    fn live_invite(
        &self,
        invite_id: InviteId,
    ) -> Option<(FeedId, VerifyingKey, &kb_keys::membership::InviteRecord)> {
        let feed_id = *self.invites.get(&invite_id)?;
        let feed = self.feeds.get(&feed_id)?;
        let rules = feed.rules.as_ref()?;
        let record = rules.state().invite(invite_id)?;
        if record.status != kb_keys::membership::InviteRecordStatus::Open
            || rules.closed()
            || (!self.serve_expired() && record.is_expired(self.now_ms))
        {
            return None;
        }
        Some((feed_id, record.invite_pub, record))
    }

    /// The view link `id` if it exists and the server clock has not reached its expiry (an expired
    /// link is kept until revoked, but nothing public sees it).
    fn live_link(&self, id: LinkId) -> Option<&LinkRow> {
        self.links
            .get(&id)
            .filter(|link| link.expires_at.is_none_or(|at| self.now_ms < at))
    }

    /// Counts one public link read (a publication or a chunk) of connection `conn`: at most
    /// `LINK_GETS_PER_MINUTE_PER_IP` in the last minute, then `Unavailable` (429). Every read counts,
    /// whether the link exists or not.
    fn count_link_read(&mut self, conn: u64) -> Result<(), SyncError> {
        let now = self.now_ms;
        let reads = self.link_reads.entry(conn).or_default();
        while reads
            .front()
            .is_some_and(|at| *at <= now.saturating_sub(MINUTE_MS))
        {
            reads.pop_front();
        }
        if reads.len() >= usize::try_from(LINK_GETS_PER_MINUTE_PER_IP).unwrap_or(120) {
            return Err(SyncError::Unavailable(
                "too many link requests; try again later".to_owned(),
            ));
        }
        reads.push_back(now);
        Ok(())
    }

    /// Registers an account (its log verified) and its personal feed. `Conflict` when the account,
    /// a device id of its log, or a shared feed of that id exists already.
    fn register_account(
        &mut self,
        log: DeviceLog,
        state: LogState,
        created_via: Option<InviteId>,
    ) -> Result<AccountId, SyncError> {
        let account = state.account_id;
        if self.accounts.contains_key(&account)
            || self.feeds.contains_key(&FeedId::from_account(account))
        {
            return Err(SyncError::Conflict("the account already exists".to_owned()));
        }
        for record in &state.devices {
            if self.devices.contains_key(&record.device_id) {
                return Err(SyncError::Conflict(
                    "a device id of the log is used by another account".to_owned(),
                ));
            }
        }
        for record in &state.devices {
            self.devices.insert(record.device_id, account);
        }
        let epoch = self.server_epoch;
        self.feeds.insert(
            FeedId::from_account(account),
            Feed::new(FeedKind::Personal, account, &epoch),
        );
        self.accounts.insert(
            account,
            Account {
                log,
                state,
                last_seen: HashMap::new(),
                created_via,
            },
        );
        Ok(account)
    }

    /// The feed as `reader` sees it (up to cursor `max_cursor` when given), with the chain hash
    /// after each item, under the configured adversary. Up to the first item the adversary hides or
    /// changes, the chain values are the stored ones; after it they are folded over the bytes the
    /// reader sees.
    fn view<'a>(
        &'a self,
        feed_id: FeedId,
        feed: &'a Feed,
        reader: Reader<'_>,
        max_cursor: Option<u64>,
    ) -> Vec<ViewItem<'a>> {
        let genesis = chain_genesis(&self.server_epoch);
        let candidates: Vec<&Item> = feed
            .items
            .iter()
            .filter(|item| max_cursor.is_none_or(|max| item.cursor <= max))
            .collect();
        // Which candidates the adversary hides, and the bytes of each shown one.
        let hidden: Vec<bool> = candidates
            .iter()
            .enumerate()
            .map(|(index, item)| self.hides(feed_id, feed, &candidates, index, item, reader))
            .collect();
        let mut bytes: Vec<&[u8]> = candidates
            .iter()
            .map(|item| item.bytes.as_slice())
            .collect();
        let mut changed = vec![false; candidates.len()];
        if self.knobs.adversary == Some(Adversary::ReorderMembership) {
            let shown: Vec<usize> = candidates
                .iter()
                .enumerate()
                .filter(|(index, item)| item.kind == ItemKind::Membership && !hidden[*index])
                .map(|(index, _)| index)
                .collect();
            if let [.., first, second] = shown.as_slice() {
                bytes.swap(*first, *second);
                changed[*first] = true;
                changed[*second] = true;
            }
        }
        let mut out = Vec::with_capacity(candidates.len() + 1);
        let mut chain = genesis;
        let mut diverged = false;
        let mut injected = self
            .knobs
            .injected
            .get(&feed_id)
            .filter(|(cursor, _)| max_cursor.is_none_or(|max| *cursor <= max));
        for (index, item) in candidates.iter().enumerate() {
            if let Some((cursor, injected_bytes)) = injected
                && *cursor < item.cursor
            {
                injected = None;
                diverged = true;
                chain = chain_next(&chain, injected_bytes);
                out.push(ViewItem {
                    cursor: *cursor,
                    kind: ItemKind::Entry,
                    seq: 0,
                    bytes: injected_bytes,
                    pruned: false,
                    chain,
                });
            }
            if hidden[index] {
                diverged = true;
                continue;
            }
            if changed[index] {
                diverged = true;
            }
            if !diverged {
                chain = item.chain;
            } else if !item.pruned {
                chain = chain_next(&chain, bytes[index]);
            }
            out.push(ViewItem {
                cursor: item.cursor,
                kind: item.kind,
                seq: item.seq,
                bytes: bytes[index],
                pruned: item.pruned,
                chain,
            });
        }
        if let Some((cursor, injected_bytes)) = injected {
            chain = chain_next(&chain, injected_bytes);
            out.push(ViewItem {
                cursor: *cursor,
                kind: ItemKind::Entry,
                seq: 0,
                bytes: injected_bytes,
                pruned: false,
                chain,
            });
        }
        out
    }

    /// Whether the configured adversary hides candidate `index` from `reader`.
    #[allow(clippy::too_many_arguments)]
    fn hides(
        &self,
        feed_id: FeedId,
        feed: &Feed,
        candidates: &[&Item],
        index: usize,
        item: &Item,
        reader: Reader<'_>,
    ) -> bool {
        let membership = item.kind == ItemKind::Membership;
        match self.knobs.adversary {
            Some(Adversary::DropMembership { from_seq }) => membership && item.seq >= from_seq,
            Some(Adversary::HideMembership) => {
                membership
                    && reader.is_none_or(|session| session.account != item.account)
                    && MembershipEntry::peek(&item.bytes).is_ok_and(|header| {
                        matches!(header.kind(), EntryKind::Remove | EntryKind::Leave)
                    })
            }
            Some(Adversary::ForkMembership) => {
                let newest = candidates
                    .iter()
                    .rposition(|other| other.kind == ItemKind::Membership);
                membership
                    && newest == Some(index)
                    && reader.is_none_or(|session| session.account != feed.billing)
            }
            Some(Adversary::ForkFeed) => {
                let from = self.knobs.fork_from.get(&feed_id).copied().unwrap_or(0);
                item.cursor > from && reader.is_none_or(|session| session.device != item.device)
            }
            Some(Adversary::Rollback(n)) => {
                index
                    >= candidates
                        .len()
                        .saturating_sub(usize::try_from(n).unwrap_or(usize::MAX))
            }
            Some(Adversary::Withhold { doc }) => !membership && item.doc_id == doc,
            _ => false,
        }
    }
}

/// A copy of a document entry whose header claims the next `device_seq`; the original signature no
/// longer matches.
#[cfg(any(test, feature = "testing"))]
fn forge(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut header = super::peek_entry(bytes).ok()?;
    header.device_seq = header.device_seq.wrapping_add(1);
    let mut envelope = kb_crypto::Envelope::decode(bytes).ok()?;
    envelope.header = super::encode_entry_header(&header);
    Some(envelope.encode())
}

/// The page of `view` after `after`.
fn page_from_view(
    view: &[ViewItem<'_>],
    after: u64,
    limit: u32,
    server_epoch: [u8; 16],
) -> ChangesPage {
    let genesis = chain_genesis(&server_epoch);
    let limit = usize::try_from(limit.clamp(1, CHANGES_PAGE_LIMIT)).unwrap_or(1);
    let start = view.partition_point(|item| item.cursor <= after);
    let mut chain_hash_after = view
        .get(..start)
        .and_then(<[ViewItem<'_>]>::last)
        .map_or(genesis, |item| item.chain);
    let mut rest = view.iter().skip(start).filter(|item| !item.pruned);
    let mut changes = Vec::new();
    for item in rest.by_ref().take(limit) {
        changes.push((item.cursor, item.bytes.to_vec()));
        chain_hash_after = item.chain;
    }
    let more = rest.next().is_some();
    ChangesPage {
        server_epoch,
        changes,
        chain_hash_after,
        more,
    }
}

/// Decodes encoded device log entries.
fn decode_log(entries: &[Vec<u8>]) -> Result<Vec<LogEntry>, SyncError> {
    entries
        .iter()
        .map(|bytes| LogEntry::decode(bytes).map_err(log_error))
        .collect()
}

/// A log that does not verify: a bad signature is `Authentication`, malformed bytes `Encoding`, a
/// broken chain `Conflict` (it does not continue the stored log), any other rule `Protocol`.
fn log_error(error: LogError) -> SyncError {
    match error {
        LogError::Encoding(message) => SyncError::Encoding(message),
        LogError::Signature { .. } => SyncError::Authentication,
        LogError::Chain { .. } => SyncError::Conflict(error.to_string()),
        other => SyncError::Protocol(other.to_string()),
    }
}

/// Refuses a pairing message beyond `PAIRING_MAX_MESSAGE_BYTES`.
fn check_pairing_size(len: usize) -> Result<(), SyncError> {
    if len > PAIRING_MAX_MESSAGE_BYTES {
        Err(SyncError::Protocol(format!(
            "a pairing message of {len} bytes exceeds {PAIRING_MAX_MESSAGE_BYTES}"
        )))
    } else {
        Ok(())
    }
}

/// A feed item header that must name `feed`, the session's account and device.
fn check_owner(
    session: &Session,
    feed: FeedId,
    named_feed: FeedId,
    account: AccountId,
    device: DeviceId,
) -> Result<(), SyncError> {
    if named_feed != feed {
        return Err(SyncError::Protocol(
            "the object names another feed than the request".to_owned(),
        ));
    }
    if account != session.account || device != session.device {
        return Err(SyncError::Forbidden);
    }
    Ok(())
}

/// The reference remote. A value is one client connection; [`Clone`] opens another connection to the
/// same server (see the [module documentation](self)).
pub struct MemoryRemote {
    inner: Arc<Mutex<Inner>>,
    /// This connection's session token, once authenticated.
    session: Mutex<Option<[u8; 32]>>,
    /// This connection's number (the per-client limits count per connection).
    conn: u64,
}

impl Clone for MemoryRemote {
    /// A new, unauthenticated connection to the same server.
    fn clone(&self) -> Self {
        let conn = {
            let mut inner = self.lock();
            let conn = inner.connections;
            inner.connections = inner.connections.saturating_add(1);
            conn
        };
        Self {
            inner: Arc::clone(&self.inner),
            session: Mutex::new(None),
            conn,
        }
    }
}

impl Default for MemoryRemote {
    /// A server whose randomness (epoch, nonces, tokens) derives from an all-zero seed.
    fn default() -> Self {
        Self::from_seed([0u8; 32])
    }
}

impl core::fmt::Debug for MemoryRemote {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let inner = self.lock();
        f.debug_struct("MemoryRemote")
            .field("accounts", &inner.accounts.len())
            .field("feeds", &inner.feeds.len())
            .field("authenticated", &self.session_token().is_some())
            .finish_non_exhaustive()
    }
}

impl MemoryRemote {
    /// A fresh server with a random epoch; its later randomness (nonces, tokens, new epochs) derives
    /// from 32 bytes drawn from `rng` now, so a seeded `rng` makes the whole remote deterministic.
    /// Accepts the invite token [`DEFAULT_INVITE_TOKEN`], allows `SHARE_SIGNUPS_DEFAULT` share
    /// signups (`SHARE_SIGNUPS_PER_ACCOUNT_DEFAULT` per inviter) and has no quota until configured.
    #[must_use]
    pub fn new(rng: &dyn kb_types::Rng) -> Self {
        let mut seed = [0u8; 32];
        rng.fill_bytes(&mut seed);
        Self::from_seed(seed)
    }

    fn from_seed(seed: [u8; 32]) -> Self {
        let mut inner = Inner {
            server_epoch: [0u8; 16],
            seed,
            counter: 0,
            invite_tokens: vec![DEFAULT_INVITE_TOKEN.to_owned()],
            account_quota: None,
            share_signups: SHARE_SIGNUPS_DEFAULT,
            share_signups_per_account: SHARE_SIGNUPS_PER_ACCOUNT_DEFAULT,
            idle_device_days: None,
            accounts: BTreeMap::new(),
            devices: HashMap::new(),
            sessions: HashMap::new(),
            now_ms: 0,
            offline: false,
            last_cursor: 0,
            connections: 1,
            feeds: BTreeMap::new(),
            invites: HashMap::new(),
            open_calls: HashMap::new(),
            links: BTreeMap::new(),
            link_reads: HashMap::new(),
            mailboxes: HashMap::new(),
            recovery: HashMap::new(),
            knobs: Knobs::default(),
            sockets: BTreeMap::new(),
            next_socket: 0,
        };
        inner.server_epoch = inner.random();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            session: Mutex::new(None),
            conn: 0,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn session_token(&self) -> Option<[u8; 32]> {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_session(&self, token: Option<[u8; 32]>) {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }

    /// The account and device of this connection's session; the device must still be active in the
    /// account's log. Also refuses every call while offline.
    fn authorized(&self, inner: &Inner) -> Result<Session, SyncError> {
        inner.check_online()?;
        let token = self.session_token().ok_or(SyncError::Unauthorized)?;
        let session = *inner.sessions.get(&token).ok_or(SyncError::Unauthorized)?;
        let account = inner
            .accounts
            .get(&session.account)
            .ok_or(SyncError::Unauthorized)?;
        account
            .state
            .active(session.device)
            .ok_or(SyncError::Unauthorized)?;
        Ok(session)
    }

    /// The current storage epoch.
    #[must_use]
    pub fn server_epoch(&self) -> [u8; 16] {
        self.lock().server_epoch
    }

    /// Replaces the set of accepted operator invite tokens (default: [`DEFAULT_INVITE_TOKEN`]).
    pub fn set_invite_tokens(&self, tokens: &[&str]) {
        self.lock().invite_tokens = tokens.iter().map(|t| (*t).to_owned()).collect();
    }

    /// Limits the bytes each billing account may store (entries, snapshots, heads and chunks of its
    /// personal feed and of the shared feeds it is billed for); `None` removes the limit. Reported by
    /// `server_info`.
    pub fn set_account_quota(&self, quota: Option<u64>) {
        self.lock().account_quota = quota;
    }

    /// The share signup limits (`KB_SERVER_SHARE_SIGNUPS`, `KB_SERVER_SHARE_SIGNUPS_PER_ACCOUNT`).
    pub fn set_share_signups(&self, total: u32, per_account: u32) {
        let mut inner = self.lock();
        inner.share_signups = total;
        inner.share_signups_per_account = per_account;
    }

    // ----- server, accounts, logs, authentication -----------------------------------------------

    /// Synchronous `server_info`.
    pub fn server_info_sync(&self) -> Result<ServerInfo, SyncError> {
        let inner = self.lock();
        inner.check_online()?;
        Ok(ServerInfo {
            protocol_version: kb_types::sync::PROTOCOL_VERSION,
            server_epoch: inner.server_epoch,
            max_wire_bytes: MAX_WIRE_BYTES,
            account_quota_bytes: inner.account_quota,
            idle_device_days: inner.idle_device_days,
            share_signups: inner.share_signups,
            share_signups_per_account: inner.share_signups_per_account,
        })
    }

    /// Synchronous `create_account`: the invite token must be configured (`Unauthorized` otherwise),
    /// the log must verify with `kb-keys`, and the account id is derived from its genesis; an
    /// existing account or a device id another account uses is `Conflict`.
    pub fn create_account_sync(
        &self,
        invite_token: &str,
        log: &[Vec<u8>],
    ) -> Result<(), SyncError> {
        let mut inner = self.lock();
        inner.check_online()?;
        if !inner.invite_tokens.iter().any(|t| t == invite_token) {
            return Err(SyncError::Unauthorized);
        }
        let (log, state) = verified_log(log)?;
        inner.register_account(log, state, None).map(drop)
    }

    /// Synchronous `account_create_share`: a dead invite (unknown, used, revoked, expired on this
    /// clock, or its feed closed) is `NotFound`, a signature that does not verify `Authentication`;
    /// then `Forbidden` for a bound invite and another account, a second signup with the same
    /// invite, and the two share limits; `Conflict` for an account that exists.
    pub fn account_create_share_sync(
        &self,
        log: &[Vec<u8>],
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<(), SyncError> {
        let mut inner = self.lock();
        inner.check_online()?;
        let Some((_, invite_pub, record)) = inner.live_invite(invite_id) else {
            return Err(SyncError::NotFound);
        };
        let (bound, creator) = (record.bound_account, record.created_by);
        let (log, state) = verified_log(log)?;
        let account = state.account_id;
        kb_keys::invite::verify_signup(&invite_pub, account, inner.server_epoch, sig)
            .map_err(|_| SyncError::Authentication)?;
        if bound.is_some_and(|bound| bound != account) {
            return Err(SyncError::Forbidden);
        }
        if inner.accounts.contains_key(&account) {
            return Err(SyncError::Conflict("the account already exists".to_owned()));
        }
        let created: Vec<InviteId> = inner
            .accounts
            .values()
            .filter_map(|stored| stored.created_via)
            .collect();
        let by_creator = created
            .iter()
            .filter(|id| {
                inner
                    .invites
                    .get(id)
                    .and_then(|feed| inner.feeds.get(feed))
                    .and_then(|feed| feed.rules.as_ref())
                    .and_then(|rules| rules.state().invite(**id))
                    .is_some_and(|record| record.created_by == creator)
            })
            .count();
        if created.contains(&invite_id)
            || created.len() >= usize::try_from(inner.share_signups).unwrap_or(usize::MAX)
            || by_creator >= usize::try_from(inner.share_signups_per_account).unwrap_or(usize::MAX)
        {
            return Err(SyncError::Forbidden);
        }
        inner
            .register_account(log, state, Some(invite_id))
            .map(drop)
    }

    /// Synchronous `append_log`: every entry must continue the stored chain and verify (all or
    /// nothing). Entries the server already holds are accepted again when they are byte-identical (a
    /// re-publication), refused as `Conflict` otherwise; a device id another account uses is
    /// `Conflict`.
    pub fn append_log_sync(
        &self,
        account: AccountId,
        entries: &[Vec<u8>],
    ) -> Result<(), SyncError> {
        let mut guard = self.lock();
        guard.check_online()?;
        let decoded = decode_log(entries)?;
        let inner = &mut *guard;
        let stored = inner
            .accounts
            .get_mut(&account)
            .ok_or(SyncError::NotFound)?;
        let mut log = stored.log.clone();
        let mut fresh = Vec::new();
        for entry in decoded {
            let existing = usize::try_from(entry.seq)
                .ok()
                .and_then(|index| log.entries().get(index));
            match existing {
                Some(existing) if *existing == entry => {}
                Some(_) => {
                    return Err(SyncError::Conflict(format!(
                        "log entry {} differs from the stored one",
                        entry.seq
                    )));
                }
                None => {
                    log.append(entry.clone()).map_err(log_error)?;
                    fresh.push(entry);
                }
            }
        }
        let state = log.verify().map_err(log_error)?;
        for entry in &fresh {
            if let LogEntryBody::Genesis { device_id, .. }
            | LogEntryBody::DeviceAdded { device_id, .. } = &entry.body
                && inner
                    .devices
                    .get(device_id)
                    .is_some_and(|owner| *owner != account)
            {
                return Err(SyncError::Conflict(
                    "a device id of the log is used by another account".to_owned(),
                ));
            }
        }
        for record in &state.devices {
            inner.devices.insert(record.device_id, account);
        }
        stored.state = state;
        stored.log = log;
        // A device the log no longer lists as active loses its sessions and its sockets at once (and
        // so cannot sign in again: `authenticate` refuses it).
        let accounts = &inner.accounts;
        inner.sessions.retain(|_, session| {
            session.account != account
                || accounts
                    .get(&account)
                    .is_some_and(|stored| stored.state.active(session.device).is_some())
        });
        inner.reap_sockets();
        Ok(())
    }

    /// Synchronous `fetch_log`: the encoded entries with `seq > after`; an unknown account is
    /// `NotFound`.
    pub fn fetch_log_sync(
        &self,
        account: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<Vec<u8>>, SyncError> {
        let inner = self.lock();
        inner.check_online()?;
        let stored = inner.account(account)?;
        Ok(served_log(&inner, account, stored, after))
    }

    /// Synchronous `logs_batch`: unknown accounts are left out.
    pub fn logs_batch_sync(&self, requests: &[LogRequest]) -> Result<Vec<LogChunk>, SyncError> {
        let inner = self.lock();
        inner.check_online()?;
        Ok(requests
            .iter()
            .filter_map(|request| {
                let stored = inner.accounts.get(&request.account)?;
                Some(LogChunk {
                    account: request.account,
                    entries: served_log(&inner, request.account, stored, request.after),
                })
            })
            .collect())
    }

    /// Synchronous `authenticate`: issues a nonce, has `sign` sign [`auth_message`]`(nonce, epoch)`
    /// (outside the lock, so `sign` may call back), verifies it with the device's key from the log
    /// and opens this connection's session, recording the device as seen now (server clock). Unknown
    /// account: `NotFound`; device not active: `Unauthorized`; device idle beyond the configured
    /// days: `Idle`; bad signature: `Authentication`. Any previous session of this connection ends
    /// first.
    pub fn authenticate_sync(
        &self,
        account: AccountId,
        device: DeviceId,
        sign: &dyn Fn(&[u8]) -> Signature,
    ) -> Result<(), SyncError> {
        let (nonce, epoch) = {
            let mut inner = self.lock();
            if let Some(old) = self.session_token() {
                inner.sessions.remove(&old);
            }
            self.set_session(None);
            inner.check_online()?;
            inner.admissible_key(account, device)?;
            (inner.random::<32>(), inner.server_epoch)
        };
        let message = auth_message(&nonce, &epoch);
        let signature = sign(&message);
        let mut inner = self.lock();
        inner.check_online()?;
        if inner.server_epoch != epoch {
            // The challenge died with the data it was issued against.
            return Err(SyncError::Unauthorized);
        }
        let verifier = inner.admissible_key(account, device)?;
        verify_auth(&message, &signature, &verifier)?;
        let token = inner.random::<32>();
        inner.sessions.insert(token, Session { account, device });
        let now = inner.now_ms;
        if let Some(stored) = inner.accounts.get_mut(&account) {
            stored.last_seen.insert(device, now);
        }
        self.set_session(Some(token));
        Ok(())
    }

    // ----- feeds and membership ----------------------------------------------------------------

    /// Synchronous `feeds_create`.
    pub fn feeds_create_sync(&self, create: &[u8]) -> Result<FeedCreated, SyncError> {
        let mut guard = self.lock();
        self.authorized(&guard)?;
        check_item_size(create.len(), "a membership entry")?;
        let entry =
            MembershipEntry::decode(create).map_err(|e| SyncError::Encoding(e.to_string()))?;
        let feed = entry.header.feed;
        let signer = entry.header.account;
        let inner = &mut *guard;
        if inner.feeds.contains_key(&feed) || inner.accounts.contains_key(&feed.as_account()) {
            return Err(SyncError::Conflict("the feed id is taken".to_owned()));
        }
        if entry.kind() != EntryKind::Create {
            return Err(SyncError::Protocol(
                "a feed starts with a Create entry".to_owned(),
            ));
        }
        let owned = inner
            .feeds
            .values()
            .filter(|stored| stored.kind == FeedKind::Shared && stored.billing == signer)
            .count();
        if owned >= MAX_FEEDS_PER_ACCOUNT {
            return Err(SyncError::Forbidden);
        }
        inner.check_quota(signer, len_u64(create.len()), 0)?;
        let cursor = inner.last_cursor.saturating_add(1);
        let ctx = BatchContext {
            now_ms: inner.now_ms,
            logs: &Logs(&inner.accounts),
            serve_expired: inner.serve_expired(),
        };
        let rules = FeedRules::new(feed)
            .apply_batch(&[(cursor, entry.clone())], &ctx)
            .map_err(|error| match error.kind {
                super::guard::BatchErrorKind::Conflict => {
                    SyncError::Protocol("a feed starts with the Create at seq 0".to_owned())
                }
                _ => error.sync_error(true),
            })?;
        inner.last_cursor = cursor;
        let (epoch, now) = (inner.server_epoch, inner.now_ms);
        let mut stored = Feed::new(FeedKind::Shared, signer, &epoch);
        stored.push(
            cursor,
            ItemKind::Membership,
            KEYRING_DOC_ID,
            signer,
            entry.header.device,
            entry.header.seq,
            create,
            now,
        );
        let head = entry.head();
        stored.rules = Some(rules);
        inner.feeds.insert(feed, stored);
        Ok(FeedCreated { feed, head, cursor })
    }

    /// Synchronous `feeds_list`.
    pub fn feeds_list_sync(&self) -> Result<Vec<FeedListing>, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        let mut out = vec![FeedListing {
            feed: FeedId::from_account(session.account),
            kind: FeedKind::Personal,
            role: None,
            removed: false,
            closed: false,
        }];
        for (id, stored) in &inner.feeds {
            if stored.kind != FeedKind::Shared {
                continue;
            }
            let access = inner.access(&session, *id);
            let listing = |role, removed| FeedListing {
                feed: *id,
                kind: FeedKind::Shared,
                role,
                removed,
                closed: stored.closed_at.is_some(),
            };
            match access {
                Access::Member(role) => out.push(listing(Some(role), false)),
                Access::Removed { .. } => out.push(listing(None, true)),
                Access::Stranger => {}
            }
        }
        Ok(out)
    }

    /// Synchronous `membership_append`.
    pub fn membership_append_sync(
        &self,
        feed: FeedId,
        entries: &[Vec<u8>],
    ) -> Result<MembershipHead, SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        if entries.is_empty() {
            return Err(SyncError::Protocol("a batch holds no entries".to_owned()));
        }
        let mut decoded = Vec::with_capacity(entries.len());
        for bytes in entries {
            check_item_size(bytes.len(), "a membership entry")?;
            decoded.push(
                MembershipEntry::decode(bytes).map_err(|e| SyncError::Encoding(e.to_string()))?,
            );
        }
        let inner = &mut *guard;
        if inner
            .feeds
            .get(&feed)
            .is_none_or(|stored| stored.rules.is_none())
        {
            return Err(SyncError::NotFound);
        }
        let accept_all = inner.accept_all();
        let access = inner.access(&session, feed);
        let member = access.role().is_some();
        if !member
            && !accept_all
            && !decoded.iter().all(|entry| {
                matches!(entry.header.body, EntryBody::Join { .. })
                    && entry.header.account == session.account
            })
        {
            // A stranger may only post the `Join`s that its invite proves.
            return Err(SyncError::NotFound);
        }
        if let Some(count) = inner.knobs.lost_cas.get_mut(&feed)
            && *count > 0
        {
            *count -= 1;
            return Err(SyncError::Conflict(
                "another membership entry was appended first; read the log and retry".to_owned(),
            ));
        }
        let first_cursor = inner.last_cursor.saturating_add(1);
        let batch: Vec<(u64, MembershipEntry)> = (first_cursor..).zip(decoded).collect();
        let total: u64 = entries.iter().map(|bytes| len_u64(bytes.len())).sum();
        let now = inner.now_ms;
        let billing = inner
            .feeds
            .get(&feed)
            .map_or(session.account, |f| f.billing);
        inner.check_quota(billing, total, 0)?;
        let new_rules = if accept_all {
            None
        } else {
            let ctx = BatchContext {
                now_ms: now,
                logs: &Logs(&inner.accounts),
                serve_expired: inner.serve_expired(),
            };
            let current = inner
                .feeds
                .get(&feed)
                .and_then(|stored| stored.rules.as_ref())
                .ok_or(SyncError::NotFound)?;
            Some(
                current
                    .apply_batch(&batch, &ctx)
                    .map_err(|error| error.sync_error(member))?,
            )
        };
        // Committed from here on.
        let mut last = None;
        // Where each entry landed, for the sockets: (previous cursor, cursor, chain after, bytes).
        let mut stored_at: Vec<(u64, u64, [u8; 32], &[u8])> = Vec::with_capacity(batch.len());
        let stored = inner.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        for ((cursor, entry), bytes) in batch.iter().zip(entries) {
            let prev_cursor = stored.last_cursor();
            let chain = stored.push(
                *cursor,
                ItemKind::Membership,
                KEYRING_DOC_ID,
                entry.header.account,
                entry.header.device,
                entry.header.seq,
                bytes,
                now,
            );
            stored_at.push((prev_cursor, *cursor, chain, bytes.as_slice()));
            last = Some(entry.head());
        }
        let newest = batch.last().map_or(first_cursor, |(cursor, _)| *cursor);
        let mut new_invites = Vec::new();
        let mut closed_now = false;
        if let Some(rules) = new_rules {
            if rules.closed() && stored.closed_at.is_none() {
                stored.closed_at = Some(now);
                closed_now = true;
            }
            if !matches!(rules.access(stored.billing), Access::Member(_))
                && let Some(successor) = rules.billing_successor()
            {
                stored.billing = successor;
            }
            for (_, entry) in &batch {
                if let EntryBody::Invite { invite_id, .. } = &entry.header.body {
                    new_invites.push(*invite_id);
                }
            }
            stored.rules = Some(rules);
        }
        inner.last_cursor = inner.last_cursor.max(newest);
        for invite in new_invites {
            inner.invites.insert(invite, feed);
        }
        if closed_now {
            // Closing a feed deletes its view links (ADR-0051).
            inner.links.retain(|_, link| link.feed != feed);
        }
        // The rules are the new ones now: a member the batch removed gets its `Remove` and nothing after.
        for (prev_cursor, cursor, chain, bytes) in stored_at {
            inner.push_item(feed, prev_cursor, cursor, chain, bytes);
        }
        last.ok_or_else(|| SyncError::Protocol("a batch holds no entries".to_owned()))
    }

    /// Synchronous `membership_get`: the entries with `seq > after_seq` the session may see (a
    /// removed account sees up to its own `Remove`), at most `CHANGES_PAGE_LIMIT`.
    pub fn membership_get_sync(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> Result<MembershipPage, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        if stored.rules.is_none() {
            return Err(SyncError::NotFound);
        }
        let max_cursor = match inner.access(&session, feed) {
            Access::Member(_) => None,
            Access::Removed { cursor } => Some(cursor),
            Access::Stranger => return Err(SyncError::NotFound),
        };
        let view = inner.view(feed, stored, Some(&session), max_cursor);
        Ok(membership_page(&view, after_seq))
    }

    // ----- entries, snapshots, heads -------------------------------------------------------------

    /// Synchronous `append`: checks the size (`Protocol` beyond [`MAX_WIRE_BYTES`]), the header
    /// (`Encoding`; `Protocol` for another feed, for a protocol v1 object or for a membership entry),
    /// the access (`NotFound`, `Forbidden`), the account and device against the session
    /// (`Forbidden`) and the signature (`Authentication`); answers the existing cursor for a
    /// duplicate; then `Closed`, `RekeyNeeded`, `StaleEpoch` and the quota (`Quota`); assigns the next
    /// cursor and advances the chain hash.
    pub fn append_sync(&self, feed: FeedId, entry: &[u8]) -> Result<(u64, [u8; 32]), SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        check_item_size(entry.len(), "an entry")?;
        let info = peek_feed_item(entry)?;
        let accept_all = guard.accept_all();
        if info.kind == ItemKind::Membership && !accept_all {
            return Err(SyncError::Protocol(
                "membership entries go through membership_append".to_owned(),
            ));
        }
        if !accept_all {
            check_owner(&session, feed, info.feed, info.account_id, info.device_id)?;
        }
        guard.feed_for(&session, feed, MemberRole::Commenter)?;
        let inner = &mut *guard;
        if !accept_all {
            let key = inner.admissible_key(session.account, session.device)?;
            verify_signature(entry, &key)?;
        }
        let key = (info.kind, info.doc_id, info.device_id, info.device_seq);
        if let Some(existing) = inner
            .feeds
            .get(&feed)
            .and_then(|stored| stored.seen.get(&key).and_then(|i| stored.items.get(*i)))
        {
            return Ok((existing.cursor, existing.chain));
        }
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        if !accept_all && stored.rules.is_some() {
            if stored.closed_at.is_some() {
                return Err(SyncError::Closed);
            }
            if inner.pending_rekey(feed) {
                return Err(SyncError::RekeyNeeded);
            }
            let key_epoch = match info.kind {
                ItemKind::Entry => super::peek_entry(entry)?.key_epoch,
                ItemKind::Keyring => super::peek_keyring(entry)?.key_epoch,
                ItemKind::Membership => 0,
            };
            let epoch = stored.rules.as_ref().map_or(0, FeedRules::epoch);
            if key_epoch < epoch {
                return Err(SyncError::StaleEpoch);
            }
        }
        inner.check_quota(stored.billing, len_u64(entry.len()), 0)?;
        let cursor = inner.next_cursor();
        let now = inner.now_ms;
        let stored = inner.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        let prev_cursor = stored.last_cursor();
        let chain = stored.push(
            cursor,
            info.kind,
            info.doc_id,
            info.account_id,
            info.device_id,
            info.device_seq,
            entry,
            now,
        );
        inner.push_item(feed, prev_cursor, cursor, chain, entry);
        Ok((cursor, chain))
    }

    /// Synchronous `changes`: at most `limit` items after `after` (`limit` is clamped to
    /// `1..=CHANGES_PAGE_LIMIT`), as the configured adversary, if any, presents them. A removed
    /// account reads up to its own `Remove` and gets `Forbidden` at and past it.
    pub fn changes_sync(
        &self,
        feed: FeedId,
        after: u64,
        limit: u32,
    ) -> Result<ChangesPage, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        let max_cursor = match inner.access(&session, feed) {
            Access::Member(_) => None,
            Access::Removed { cursor } => {
                if after >= cursor {
                    return Err(SyncError::Forbidden);
                }
                Some(cursor)
            }
            Access::Stranger => return Err(SyncError::NotFound),
        };
        let view = inner.view(feed, stored, Some(&session), max_cursor);
        Ok(page_from_view(&view, after, limit, inner.server_epoch))
    }

    /// Synchronous `put_snapshot`: size, header, access (Commenter and up), account, device and
    /// signature checked like `append`; `Closed` for a closed feed; a snapshot that covers less than
    /// the stored one is `Conflict`; the quota; replaces the document's latest snapshot, records its
    /// arrival (the server clock) for pruning and a snapshot mark.
    pub fn put_snapshot_sync(&self, feed: FeedId, snapshot: &[u8]) -> Result<(), SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        check_item_size(snapshot.len(), "a snapshot")?;
        let header = peek_snapshot(snapshot)?;
        let accept_all = guard.accept_all();
        if !accept_all {
            check_owner(
                &session,
                feed,
                header.feed,
                header.account_id,
                header.device_id,
            )?;
        }
        guard.feed_for(&session, feed, MemberRole::Commenter)?;
        let inner = &mut *guard;
        if !accept_all {
            let key = inner.admissible_key(session.account, session.device)?;
            verify_signature(snapshot, &key)?;
        }
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        if !accept_all && stored.closed_at.is_some() {
            return Err(SyncError::Closed);
        }
        let old = stored.snapshots.get(&header.doc_id);
        if old.is_some_and(|old| header.covers_cursor < old.covers) {
            return Err(SyncError::Conflict(
                "the snapshot covers less of the feed than the stored one".to_owned(),
            ));
        }
        let freed = old.map_or(0, |old| len_u64(old.bytes.len()));
        inner.check_quota(stored.billing, len_u64(snapshot.len()), freed)?;
        let now = inner.now_ms;
        let stored = inner.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        let prune_covers = header.covers_cursor.min(stored.last_cursor());
        let previous = stored.snapshots.remove(&header.doc_id);
        if let Some(previous) = &previous {
            stored.disputes.remove(&(header.doc_id, previous.covers));
        }
        stored.bytes_used = stored.bytes_used.saturating_sub(freed);
        stored.bytes_used = stored.bytes_used.saturating_add(len_u64(snapshot.len()));
        stored.snapshots.insert(
            header.doc_id,
            SnapRow {
                bytes: snapshot.to_vec(),
                covers: header.covers_cursor,
                prune_covers,
                device: header.device_id,
                account: header.account_id,
                arrived_at: now,
                previous: previous.map(|row| row.bytes),
            },
        );
        stored.next_mark = stored.next_mark.saturating_add(1);
        let mark = stored.next_mark;
        stored.marks.push(SnapshotMark {
            mark,
            doc_id: header.doc_id,
            device_id: header.device_id,
            account_id: header.account_id,
            covers_cursor: header.covers_cursor,
        });
        Ok(())
    }

    /// Synchronous `get_snapshot`.
    pub fn get_snapshot_sync(
        &self,
        feed: FeedId,
        doc: DocId,
    ) -> Result<Option<Vec<u8>>, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        inner.feed_for(&session, feed, MemberRole::Viewer)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        Ok(stored
            .snapshots
            .get(&doc)
            .map(|row| served_snapshot(&inner, row)))
    }

    /// Synchronous `snapshots` (in document id order).
    pub fn snapshots_sync(&self, feed: FeedId) -> Result<SnapshotsPage, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        inner.feed_for(&session, feed, MemberRole::Viewer)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        Ok(SnapshotsPage {
            server_epoch: inner.server_epoch,
            snapshots: stored
                .snapshots
                .values()
                .map(|row| served_snapshot(&inner, row))
                .collect(),
        })
    }

    /// Synchronous `snapshot_marks`: the marks after `after`, at most `CHANGES_PAGE_LIMIT`.
    pub fn snapshot_marks_sync(&self, feed: FeedId, after: u64) -> Result<MarksPage, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        inner.feed_for(&session, feed, MemberRole::Viewer)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        let limit = usize::try_from(CHANGES_PAGE_LIMIT).unwrap_or(usize::MAX);
        let mut rest = stored.marks.iter().filter(|mark| mark.mark > after);
        let marks: Vec<SnapshotMark> = rest.by_ref().take(limit).cloned().collect();
        Ok(MarksPage {
            marks,
            more: rest.next().is_some(),
        })
    }

    /// Synchronous `snapshot_dispute`: Editor and up; no snapshot for the document is `NotFound`;
    /// a snapshot other than the one covering `covers_cursor` (replaced already) changes nothing.
    pub fn snapshot_dispute_sync(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> Result<(), SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        guard.feed_for(&session, feed, MemberRole::Editor)?;
        let stored = guard.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        let row = stored.snapshots.get(&doc).ok_or(SyncError::NotFound)?;
        if row.covers == covers_cursor {
            stored.disputes.insert((doc, covers_cursor));
        }
        Ok(())
    }

    /// Synchronous `put_head`: any member; the head must name this feed, the session's account and
    /// device and a cursor the feed has reached, and verify under the device's key; it replaces the
    /// device's previous head.
    pub fn put_head_sync(&self, feed: FeedId, head: &[u8]) -> Result<(), SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        let record = peek_head(head)?;
        let accept_all = guard.accept_all();
        if !accept_all {
            check_owner(
                &session,
                feed,
                record.feed,
                record.account_id,
                record.device_id,
            )?;
        }
        guard.feed_for(&session, feed, MemberRole::Viewer)?;
        let inner = &mut *guard;
        if !accept_all {
            let key = inner.admissible_key(session.account, session.device)?;
            verify_head(head, &key)?;
        }
        let inner_injected = inner.knobs.injected.get(&feed).cloned();
        let stored = inner.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        if !accept_all {
            if stored.closed_at.is_some() {
                return Err(SyncError::Closed);
            }
            let injected = inner_injected.as_ref().map_or(0, |(cursor, _)| *cursor);
            if record.cursor > stored.last_cursor().max(injected) {
                return Err(SyncError::Protocol(
                    "the head names a cursor the feed has not reached".to_owned(),
                ));
            }
        }
        let old = stored
            .heads
            .insert(record.device_id, (head.to_vec(), record.cursor));
        stored.bytes_used = stored
            .bytes_used
            .saturating_sub(old.map_or(0, |(bytes, _)| len_u64(bytes.len())))
            .saturating_add(len_u64(head.len()));
        Ok(())
    }

    /// Synchronous `heads` (in device id order).
    pub fn heads_sync(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, SyncError> {
        let inner = self.lock();
        let session = self.authorized(&inner)?;
        inner.feed_for(&session, feed, MemberRole::Viewer)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        Ok(stored
            .heads
            .values()
            .map(|(bytes, _)| bytes.clone())
            .collect())
    }

    // ----- chunks ---------------------------------------------------------------------------------

    /// Synchronous `put_chunk`: content-addressed (`Protocol` when BLAKE3 of `bytes` is not `hash`,
    /// or beyond [`MAX_WIRE_BYTES`]); Editor and up in a shared feed; `Closed`; counted against the
    /// feed's billing account (`Quota` beyond the quota); storing a chunk the feed holds again
    /// succeeds without counting twice, even on a full account.
    pub fn put_chunk_sync(
        &self,
        feed: FeedId,
        hash: ChunkId,
        bytes: &[u8],
    ) -> Result<(), SyncError> {
        let mut guard = self.lock();
        guard.chunk_call_starts()?;
        let session = self.authorized(&guard)?;
        guard.chunk_call_reached(ChunkOp::Put);
        check_item_size(bytes.len(), "a chunk")?;
        if kb_crypto::hash(bytes) != *hash.as_bytes() {
            return Err(SyncError::Protocol(
                "the chunk does not match its hash".to_owned(),
            ));
        }
        guard.feed_for(&session, feed, MemberRole::Editor)?;
        let inner = &mut *guard;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        if !inner.accept_all() && stored.closed_at.is_some() {
            return Err(SyncError::Closed);
        }
        if stored.chunks.contains_key(&hash) {
            return Ok(());
        }
        let len = len_u64(bytes.len());
        inner.check_quota(stored.billing, len, 0)?;
        let stored = inner.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        stored.chunks.insert(
            hash,
            Chunk {
                bytes: bytes.to_vec(),
            },
        );
        stored.bytes_used = stored.bytes_used.saturating_add(len);
        Ok(())
    }

    /// Synchronous `get_chunk`: the feed's chunk (any member); `None` when the feed holds none under
    /// `hash` (whatever other feeds hold).
    pub fn get_chunk_sync(
        &self,
        feed: FeedId,
        hash: ChunkId,
    ) -> Result<Option<Vec<u8>>, SyncError> {
        let mut inner = self.lock();
        inner.chunk_call_starts()?;
        let session = self.authorized(&inner)?;
        inner.chunk_call_reached(ChunkOp::Get);
        inner.feed_for(&session, feed, MemberRole::Viewer)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        Ok(stored.chunks.get(&hash).map(|chunk| chunk.bytes.clone()))
    }

    /// Synchronous `delete_chunk`: an Owner of a shared feed, any active device of the account for
    /// the personal feed; the chunk's size goes back to the billing account. A chunk the feed does not
    /// hold answers `Ok` and changes nothing.
    pub fn delete_chunk_sync(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError> {
        let mut guard = self.lock();
        guard.chunk_call_starts()?;
        let session = self.authorized(&guard)?;
        guard.chunk_call_reached(ChunkOp::Delete);
        guard.feed_for(&session, feed, MemberRole::Owner)?;
        let accept_all = guard.accept_all();
        let stored = guard.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        if !accept_all && stored.closed_at.is_some() {
            return Err(SyncError::Closed);
        }
        if let Some(chunk) = stored.chunks.remove(&hash) {
            stored.bytes_used = stored.bytes_used.saturating_sub(len_u64(chunk.bytes.len()));
        }
        Ok(())
    }

    /// Synchronous `copy_chunk`: Editor and up in the target feed; the source is the session
    /// account's personal namespace (`NotFound` when it holds no such chunk).
    pub fn copy_chunk_sync(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError> {
        let mut guard = self.lock();
        guard.chunk_call_starts()?;
        let session = self.authorized(&guard)?;
        guard.chunk_call_reached(ChunkOp::Copy);
        guard.feed_for(&session, feed, MemberRole::Editor)?;
        let inner = &mut *guard;
        let personal = FeedId::from_account(session.account);
        let bytes = inner
            .feeds
            .get(&personal)
            .and_then(|source| source.chunks.get(&hash))
            .map(|chunk| chunk.bytes.clone())
            .ok_or(SyncError::NotFound)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        if !inner.accept_all() && stored.closed_at.is_some() {
            return Err(SyncError::Closed);
        }
        if stored.chunks.contains_key(&hash) {
            return Ok(());
        }
        let len = len_u64(bytes.len());
        inner.check_quota(stored.billing, len, 0)?;
        let stored = inner.feeds.get_mut(&feed).ok_or(SyncError::NotFound)?;
        stored.chunks.insert(hash, Chunk { bytes });
        stored.bytes_used = stored.bytes_used.saturating_add(len);
        Ok(())
    }

    // ----- invites ---------------------------------------------------------------------------------

    /// Synchronous `invite_open`: at most `INVITE_OPEN_PER_IP_PER_HOUR` calls per connection per
    /// hour (`Unavailable` beyond; unknown invites count too); a dead invite (unknown, used, revoked,
    /// expired on this clock, its feed closed) or a signature that does not verify is `NotFound`.
    pub fn invite_open_sync(
        &self,
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<InviteOpened, SyncError> {
        let mut guard = self.lock();
        guard.check_online()?;
        let inner = &mut *guard;
        let now = inner.now_ms;
        let calls = inner.open_calls.entry(self.conn).or_default();
        while calls
            .front()
            .is_some_and(|at| *at <= now.saturating_sub(HOUR_MS))
        {
            calls.pop_front();
        }
        if calls.len() >= usize::try_from(INVITE_OPEN_PER_IP_PER_HOUR).unwrap_or(30) {
            return Err(SyncError::Unavailable(
                "too many invite requests; try again later".to_owned(),
            ));
        }
        calls.push_back(now);
        let Some((feed, invite_pub, _)) = inner.live_invite(invite_id) else {
            return Err(SyncError::NotFound);
        };
        kb_keys::invite::verify_open(&invite_pub, invite_id, inner.server_epoch, sig)
            .map_err(|_| SyncError::NotFound)?;
        let stored = inner.feeds.get(&feed).ok_or(SyncError::NotFound)?;
        let view = inner.view(feed, stored, None, None);
        let log = view
            .iter()
            .filter(|item| item.kind == ItemKind::Membership)
            .map(|item| MembershipItem {
                cursor: item.cursor,
                bytes: item.bytes.to_vec(),
            })
            .collect();
        Ok(InviteOpened { feed, log })
    }

    // ----- view links (M13 slice 4) -----------------------------------------------------------------

    /// Synchronous `link_put`; the rules are those of the `link_*` methods of
    /// [`RemoteStore`](super::RemoteStore) (module documentation, "View links"). Session needed.
    /// The checks run in this order: the session, the access to `put.feed` (`NotFound` for a
    /// stranger, `Forbidden` below Editor), `Closed`, the declaration (`Protocol` beyond
    /// `LINK_MAX_CHUNKS`), the publication (`Protocol` when it is not a publication of `link_id`,
    /// `Quota` beyond `MAX_WIRE_BYTES`) and the feed of an existing link (`Conflict`).
    pub fn link_put_sync(&self, link_id: LinkId, put: &LinkPut) -> Result<u64, SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        let inner = &mut *guard;
        inner.feed_for(&session, put.feed, MemberRole::Editor)?;
        let closed = inner
            .feeds
            .get(&put.feed)
            .is_some_and(|stored| stored.closed_at.is_some());
        if closed && !inner.accept_all() {
            return Err(SyncError::Closed);
        }
        if put.chunks.len() > LINK_MAX_CHUNKS {
            return Err(SyncError::Protocol(format!(
                "a link declares at most {LINK_MAX_CHUNKS} chunks"
            )));
        }
        let header = peek_publication(&put.publication)
            .map_err(|_| SyncError::Protocol("the upload is not a publication".to_owned()))?;
        if header.link_id != link_id {
            return Err(SyncError::Protocol(
                "the publication names another link".to_owned(),
            ));
        }
        if u64::try_from(put.publication.len()).map_or(true, |len| len > MAX_WIRE_BYTES) {
            return Err(SyncError::Quota);
        }
        let chunks: BTreeSet<ChunkId> = put.chunks.iter().copied().collect();
        if let Some(link) = inner.links.get_mut(&link_id) {
            if link.feed != put.feed {
                return Err(SyncError::Conflict(
                    "the link belongs to another feed".to_owned(),
                ));
            }
            // The creator and the expiry of the first upload stay.
            link.publication.clone_from(&put.publication);
            link.chunks = chunks;
            link.pub_seq = link.pub_seq.saturating_add(1);
            return Ok(link.pub_seq);
        }
        inner.links.insert(
            link_id,
            LinkRow {
                feed: put.feed,
                created_by: session.account,
                publication: put.publication.clone(),
                pub_seq: 1,
                expires_at: put.expires_at,
                chunks,
            },
        );
        Ok(1)
    }

    /// Synchronous `link_get`: no session needed; a link past its `expires_at` on this clock is
    /// `NotFound`, and so is one that was deleted. Every call counts against the connection's
    /// `LINK_GETS_PER_MINUTE_PER_IP` (`Unavailable` beyond); `if_none_match` equal to the stored
    /// `pub_seq` is `NotModified`.
    pub fn link_get_sync(
        &self,
        link_id: LinkId,
        if_none_match: Option<u64>,
    ) -> Result<LinkFetch, SyncError> {
        let mut guard = self.lock();
        guard.check_online()?;
        guard.count_link_read(self.conn)?;
        let link = guard.live_link(link_id).ok_or(SyncError::NotFound)?;
        if if_none_match == Some(link.pub_seq) {
            return Ok(LinkFetch::NotModified);
        }
        Ok(LinkFetch::Publication {
            pub_seq: link.pub_seq,
            bytes: link.publication.clone(),
        })
    }

    /// Synchronous `link_delete`: the creator (while still a member) or an Owner. Session needed. An
    /// expired link can be revoked too; an unknown one, and any link for a stranger or a removed
    /// account, is `NotFound`; another member is `Forbidden`.
    pub fn link_delete_sync(&self, link_id: LinkId) -> Result<(), SyncError> {
        let mut guard = self.lock();
        let session = self.authorized(&guard)?;
        let inner = &mut *guard;
        let link = inner.links.get(&link_id).ok_or(SyncError::NotFound)?;
        let (feed, creator) = (link.feed, link.created_by);
        match inner.access(&session, feed) {
            Access::Member(role) if role == MemberRole::Owner || creator == session.account => {
                inner.links.remove(&link_id);
                Ok(())
            }
            Access::Member(_) => Err(SyncError::Forbidden),
            Access::Removed { .. } | Access::Stranger => Err(SyncError::NotFound),
        }
    }

    /// Synchronous `link_chunk_get`: no session needed; only declared chunks the feed holds. Counts
    /// against the same per-connection read limit as `link_get_sync`.
    pub fn link_chunk_get_sync(
        &self,
        link_id: LinkId,
        hash: ChunkId,
    ) -> Result<Vec<u8>, SyncError> {
        let mut guard = self.lock();
        guard.check_online()?;
        guard.count_link_read(self.conn)?;
        let link = guard.live_link(link_id).ok_or(SyncError::NotFound)?;
        if !link.chunks.contains(&hash) {
            return Err(SyncError::NotFound);
        }
        guard
            .feeds
            .get(&link.feed)
            .and_then(|stored| stored.chunks.get(&hash))
            .map(|chunk| chunk.bytes.clone())
            .ok_or(SyncError::NotFound)
    }

    /// The stored publication of a link and its `pub_seq`, exactly as uploaded, whatever the clock
    /// says about its expiry: for tests that look at what the server holds. No session, no read limit.
    #[must_use]
    pub fn inspect_link(&self, link_id: LinkId) -> Option<(u64, Vec<u8>)> {
        self.lock()
            .links
            .get(&link_id)
            .map(|link| (link.pub_seq, link.publication.clone()))
    }

    /// The ids of the links the server holds, expired ones included. For tests.
    #[must_use]
    pub fn inspect_links(&self) -> Vec<LinkId> {
        self.lock().links.keys().copied().collect()
    }

    // ----- pairing mailboxes and recovery blobs ----------------------------------------------------

    /// Synchronous `pair_create`: a mailbox with a random id whose message 0 is `offer` (role
    /// opener), expiring `PAIRING_TTL_SECONDS` after the server clock's now. No session needed.
    pub fn pair_create_sync(&self, offer: &[u8]) -> Result<([u8; MAILBOX_ID_LEN], i64), SyncError> {
        let mut inner = self.lock();
        inner.check_online()?;
        check_pairing_size(offer.len())?;
        let now = inner.now_ms;
        inner
            .mailboxes
            .retain(|_, mailbox| now < mailbox.expires_at);
        let id = loop {
            let id = inner.random::<MAILBOX_ID_LEN>();
            if !inner.mailboxes.contains_key(&id) {
                break id;
            }
        };
        let ttl_ms = i64::try_from(PAIRING_TTL_SECONDS)
            .unwrap_or(i64::MAX)
            .saturating_mul(1000);
        let expires_at = now.saturating_add(ttl_ms);
        inner.mailboxes.insert(
            id,
            Mailbox {
                expires_at,
                closed: false,
                messages: vec![(PAIR_ROLE_OPENER, offer.to_vec())],
            },
        );
        Ok((id, expires_at))
    }

    /// Synchronous `pair_messages`: the messages with `seq >= after`, at once. Unknown or expired:
    /// `NotFound`.
    pub fn pair_messages_sync(
        &self,
        id: [u8; MAILBOX_ID_LEN],
        after: u32,
        wait_seconds: u32,
    ) -> Result<PairingPage, SyncError> {
        let _ = wait_seconds;
        let mut inner = self.lock();
        inner.check_online()?;
        let mailbox = inner.mailbox(id)?;
        let skip = usize::try_from(after).unwrap_or(usize::MAX);
        let messages = (0u32..)
            .zip(&mailbox.messages)
            .skip(skip)
            .map(|(seq, (role, bytes))| PairingMessage {
                seq,
                role: *role,
                bytes: bytes.clone(),
            })
            .collect();
        Ok(PairingPage {
            messages,
            expires_at: mailbox.expires_at,
            closed: mailbox.closed,
        })
    }

    /// Synchronous `pair_post`: unknown or expired `NotFound`; a role other than opener or typist, or
    /// a message beyond `PAIRING_MAX_MESSAGE_BYTES`, `Protocol`; a closed mailbox or one holding
    /// `PAIRING_MAX_MESSAGES` already, `Conflict`.
    pub fn pair_post_sync(
        &self,
        id: [u8; MAILBOX_ID_LEN],
        role: u8,
        bytes: &[u8],
    ) -> Result<(), SyncError> {
        let mut inner = self.lock();
        inner.check_online()?;
        let mailbox = inner.mailbox(id)?;
        if role != PAIR_ROLE_OPENER && role != PAIR_ROLE_TYPIST {
            return Err(SyncError::Protocol(format!("unknown pairing role {role}")));
        }
        check_pairing_size(bytes.len())?;
        if mailbox.closed {
            return Err(SyncError::Conflict("the mailbox is closed".to_owned()));
        }
        let max = usize::try_from(PAIRING_MAX_MESSAGES).unwrap_or(usize::MAX);
        if mailbox.messages.len() >= max {
            return Err(SyncError::Conflict(format!(
                "the mailbox holds {PAIRING_MAX_MESSAGES} messages already"
            )));
        }
        mailbox.messages.push((role, bytes.to_vec()));
        Ok(())
    }

    /// Synchronous `pair_close`: unknown or expired `NotFound`; closing twice is fine.
    pub fn pair_close_sync(&self, id: [u8; MAILBOX_ID_LEN]) -> Result<(), SyncError> {
        let mut inner = self.lock();
        inner.check_online()?;
        inner.mailbox(id)?.closed = true;
        Ok(())
    }

    /// Synchronous `recovery_put`: a session is needed; the blob (at most `RECOVERY_BLOB_MAX_BYTES`,
    /// `Protocol` beyond) replaces any blob of the same account under `id`; an id another account
    /// owns is `Unauthorized`.
    pub fn recovery_put_sync(&self, id: [u8; 32], bytes: &[u8]) -> Result<(), SyncError> {
        let mut inner = self.lock();
        let session = self.authorized(&inner)?;
        if bytes.len() > RECOVERY_BLOB_MAX_BYTES {
            return Err(SyncError::Protocol(format!(
                "a recovery blob of {} bytes exceeds {RECOVERY_BLOB_MAX_BYTES}",
                bytes.len()
            )));
        }
        if inner
            .recovery
            .get(&id)
            .is_some_and(|blob| blob.owner != session.account)
        {
            return Err(SyncError::Unauthorized);
        }
        inner.recovery.insert(
            id,
            RecoveryBlob {
                owner: session.account,
                bytes: bytes.to_vec(),
            },
        );
        Ok(())
    }

    /// Synchronous `recovery_get` (no session).
    pub fn recovery_get_sync(&self, id: [u8; 32]) -> Result<Option<Vec<u8>>, SyncError> {
        let inner = self.lock();
        inner.check_online()?;
        Ok(inner.recovery.get(&id).map(|blob| blob.bytes.clone()))
    }

    /// Synchronous `recovery_delete`: a session of the owning account is needed (`Unauthorized`
    /// otherwise); an id that holds nothing succeeds.
    pub fn recovery_delete_sync(&self, id: [u8; 32]) -> Result<(), SyncError> {
        let mut inner = self.lock();
        let session = self.authorized(&inner)?;
        match inner.recovery.get(&id) {
            Some(blob) if blob.owner != session.account => Err(SyncError::Unauthorized),
            _ => {
                inner.recovery.remove(&id);
                Ok(())
            }
        }
    }
}

/// Decodes and verifies a device log for an account that does not exist yet: a broken chain in a
/// fresh log is a malformed request, not a conflict.
fn verified_log(entries: &[Vec<u8>]) -> Result<(DeviceLog, LogState), SyncError> {
    let log = DeviceLog::from_entries(decode_log(entries)?).map_err(|error| match error {
        LogError::Chain { .. } => SyncError::Protocol(error.to_string()),
        other => log_error(other),
    })?;
    let state = log.verify().map_err(log_error)?;
    Ok((log, state))
}

/// The encoded entries of an account's log after `after`, as the configured adversary serves them.
fn served_log(
    inner: &Inner,
    account: AccountId,
    stored: &Account,
    after: Option<u64>,
) -> Vec<Vec<u8>> {
    let mut entries: Vec<&LogEntry> = stored
        .log
        .entries()
        .iter()
        .filter(|entry| after.is_none_or(|after| entry.seq > after))
        .collect();
    if let Some(Adversary::WithholdLogTail {
        account: target,
        keep,
    }) = inner.adversary()
        && target == account
    {
        let keep = usize::try_from(keep).unwrap_or(usize::MAX);
        entries.retain(|entry| usize::try_from(entry.seq).is_ok_and(|seq| seq < keep));
    }
    entries.into_iter().map(LogEntry::encode).collect()
}

/// The snapshot bytes as the configured adversary serves them.
fn served_snapshot(inner: &Inner, row: &SnapRow) -> Vec<u8> {
    if inner.adversary() == Some(Adversary::TruncateSnapshot)
        && let Some(previous) = &row.previous
    {
        return previous.clone();
    }
    row.bytes.clone()
}

/// The membership page of a view: the entries with `seq > after_seq`, the head of the view.
fn membership_page(view: &[ViewItem<'_>], after_seq: Option<u64>) -> MembershipPage {
    let limit = usize::try_from(CHANGES_PAGE_LIMIT).unwrap_or(usize::MAX);
    let membership: Vec<&ViewItem<'_>> = view
        .iter()
        .filter(|item| item.kind == ItemKind::Membership)
        .collect();
    let head = membership.last().map_or(
        MembershipHead {
            seq: 0,
            hash: [0u8; 32],
        },
        |item| MembershipHead {
            seq: item.seq,
            hash: kb_crypto::hash(item.bytes),
        },
    );
    let mut rest = membership
        .into_iter()
        .filter(|item| after_seq.is_none_or(|after| item.seq > after));
    let entries: Vec<MembershipItem> = rest
        .by_ref()
        .take(limit)
        .map(|item| MembershipItem {
            cursor: item.cursor,
            bytes: item.bytes.to_vec(),
        })
        .collect();
    MembershipPage {
        entries,
        head,
        more: rest.next().is_some(),
    }
}

/// Knobs and inspection for tests (the `testing` feature), the v2 counterparts of v1's.
#[cfg(any(test, feature = "testing"))]
impl MemoryRemote {
    /// Sets or clears the adversarial mode (see [`Adversary`]). For [`Adversary::ForkFeed`] the fork
    /// starts at each feed's newest cursor.
    pub fn set_adversary(&self, adversary: Option<Adversary>) {
        let mut inner = self.lock();
        let from: Vec<(FeedId, u64)> = inner
            .feeds
            .iter()
            .map(|(id, feed)| (*id, feed.last_cursor()))
            .collect();
        inner.knobs.fork_from = from.into_iter().collect();
        inner.knobs.injected.clear();
        // The item a replaying or forging server adds to each feed, with a cursor of its own.
        if matches!(adversary, Some(Adversary::Replay | Adversary::Forge)) {
            let mut items: Vec<(FeedId, Vec<u8>)> = Vec::new();
            for (id, feed) in &inner.feeds {
                let mut entries = feed
                    .items
                    .iter()
                    .filter(|item| !item.pruned && item.kind == ItemKind::Entry);
                let bytes = if adversary == Some(Adversary::Forge) {
                    entries.next_back().and_then(|item| forge(&item.bytes))
                } else {
                    entries.next().map(|item| item.bytes.clone())
                };
                if let Some(bytes) = bytes {
                    items.push((*id, bytes));
                }
            }
            for (id, bytes) in items {
                let cursor = inner.next_cursor();
                inner.knobs.injected.insert(id, (cursor, bytes));
            }
        }
        inner.knobs.adversary = adversary;
    }

    /// Makes the next `count` membership compare-and-swaps of `feed` lose the race
    /// (`SyncError::Conflict`) as if another Owner had appended first. Nothing is appended.
    pub fn inject_lost_compare_and_swap(&self, feed: FeedId, count: u32) {
        self.lock().knobs.lost_cas.insert(feed, count);
    }

    /// Sets the remote's clock, Unix ms (invite expiry, pruning, idle devices, the Rekey limit).
    pub fn set_now_ms(&self, now_ms: i64) {
        self.lock().now_ms = now_ms;
    }

    /// The remote's clock, Unix ms.
    #[must_use]
    pub fn now_ms(&self) -> i64 {
        self.lock().now_ms
    }

    /// Advances the remote's clock.
    pub fn advance_ms(&self, delta_ms: i64) {
        let mut inner = self.lock();
        inner.now_ms = inner.now_ms.saturating_add(delta_ms);
    }

    /// The next `n` chunk calls (`put_chunk`, `get_chunk`, `delete_chunk` or `copy_chunk`, through
    /// any connection to this server) answer `Unavailable("injected")` before anything else, even
    /// offline or without a session: they change nothing and are not counted by
    /// [`Self::chunk_calls`]. Replaces the count still pending (`0` clears it); other calls neither
    /// fail nor use it up, and `reset_data` keeps it.
    pub fn fail_chunk_ops(&self, n: u32) {
        self.lock().knobs.chunks.failures = n;
    }

    /// The chunk calls that reached the store since the remote was created, through every connection
    /// to this server. A call counts once it is past the injected failures, the offline check and the
    /// session check, whatever the store answers then (a duplicate put, a refusal for quota, size or
    /// hash, an absent chunk); injected failures, offline calls and calls without a valid session do
    /// not count. `reset_data` keeps the counts.
    #[must_use]
    pub fn chunk_calls(&self) -> ChunkCalls {
        self.lock().knobs.chunks.calls
    }

    /// The server loses the chunk `hash` (a disk fault, an operator's mistake): every feed holding it
    /// loses it, and the usage drops by its size. Answers whether any feed held it. Not a chunk call:
    /// neither counted nor failed by injection.
    pub fn drop_chunk(&self, hash: ChunkId) -> bool {
        let mut inner = self.lock();
        let mut dropped = false;
        for feed in inner.feeds.values_mut() {
            if let Some(chunk) = feed.chunks.remove(&hash) {
                feed.bytes_used = feed.bytes_used.saturating_sub(len_u64(chunk.bytes.len()));
                dropped = true;
            }
        }
        dropped
    }

    /// The server loses one document's snapshot in a feed (an operator's mistake, a purge): a device
    /// bootstrapping afterwards meets the document's entries without their history. Answers whether
    /// the feed had one.
    pub fn forget_snapshot(&self, feed: FeedId, doc: DocId) -> bool {
        self.lock()
            .feeds
            .get_mut(&feed)
            .is_some_and(|stored| stored.snapshots.remove(&doc).is_some())
    }

    /// Makes a device count as idle for the pruning rule (as if its last session were older than the
    /// limit).
    pub fn mark_device_idle(&self, account: AccountId, device: DeviceId) {
        self.lock().knobs.idle.insert((account, device));
    }

    /// Turns idle-device expiry on (`Some(days)`) or off: `authenticate` then refuses a device whose
    /// last session (server clock) is more than `days` days old, and the pruning pass treats it as
    /// idle (30 days when off). Reported by `server_info`.
    pub fn set_idle_device_days(&self, days: Option<u32>) {
        self.lock().idle_device_days = days;
    }

    /// While offline every call answers `Unavailable`, and the realtime sockets are closed (a
    /// connection that drops: the client opens a new one when the remote is back).
    pub fn set_offline(&self, offline: bool) {
        let mut inner = self.lock();
        inner.offline = offline;
        inner.reap_sockets();
    }

    /// Runs the pruning pass of ADR-0045 at `now_ms`: deletes the entries a snapshot covers (same
    /// document, `cursor <= covers`) once the snapshot is `PRUNE_ACK_DAYS` old, nobody disputes it
    /// and, in a shared feed, every active device of every member that is not idle has a head at or
    /// past the cover; deletes closed feeds `CLOSE_READ_DAYS` after their `Close`. Membership and
    /// keyring entries are never pruned. Answers how many entries it removed.
    pub fn prune(&self, now_ms: i64) -> u64 {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let day = DAY_MS;
        let expired: Vec<FeedId> = inner
            .feeds
            .iter()
            .filter(|(_, feed)| {
                feed.closed_at.is_some_and(|closed| {
                    now_ms.saturating_sub(closed)
                        >= i64::from(kb_types::sharing::CLOSE_READ_DAYS) * day
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            inner.feeds.remove(id);
        }
        inner.invites.retain(|_, feed| !expired.contains(feed));
        inner.links.retain(|_, link| !expired.contains(&link.feed));
        let idle_limit = i64::from(
            inner
                .idle_device_days
                .unwrap_or(kb_types::sharing::PRUNE_IDLE_DEFAULT_DAYS),
        ) * day;
        let mut removed = 0u64;
        for feed in inner.feeds.values_mut() {
            // `(idle, head cursor)` of every active device of every member.
            let devices: Option<Vec<(bool, Option<u64>)>> = feed.rules.as_ref().map(|rules| {
                let mut out = Vec::new();
                for member in rules.members() {
                    let Some(stored) = inner.accounts.get(&member.account) else {
                        continue;
                    };
                    for record in stored
                        .state
                        .devices
                        .iter()
                        .filter(|d| d.revoked_at.is_none())
                    {
                        let seen = stored
                            .last_seen
                            .get(&record.device_id)
                            .copied()
                            .unwrap_or(record.added_at);
                        let idle = inner
                            .knobs
                            .idle
                            .contains(&(member.account, record.device_id))
                            || now_ms.saturating_sub(seen) > idle_limit;
                        let head = feed.heads.get(&record.device_id).map(|(_, cursor)| *cursor);
                        out.push((idle, head));
                    }
                }
                out
            });
            let mut ready: Vec<(DocId, u64)> = Vec::new();
            for (doc, row) in &feed.snapshots {
                if now_ms.saturating_sub(row.arrived_at)
                    < i64::from(kb_types::sharing::PRUNE_ACK_DAYS) * day
                    || feed.disputes.contains(&(*doc, row.covers))
                {
                    continue;
                }
                let acknowledged = devices.as_ref().is_none_or(|devices| {
                    devices
                        .iter()
                        .all(|(idle, head)| *idle || head.is_some_and(|h| h >= row.prune_covers))
                });
                if acknowledged {
                    ready.push((*doc, row.prune_covers));
                }
            }
            let mut freed = 0u64;
            for item in &mut feed.items {
                let covered = !item.pruned
                    && item.kind == ItemKind::Entry
                    && ready
                        .iter()
                        .any(|(doc, covers)| item.doc_id == *doc && item.cursor <= *covers);
                if covered {
                    freed = freed.saturating_add(len_u64(item.bytes.len()));
                    item.bytes = Vec::new();
                    item.pruned = true;
                    removed += 1;
                }
            }
            feed.bytes_used = feed.bytes_used.saturating_sub(freed);
        }
        removed
    }

    /// Forgets all data and starts a new epoch (the server reset of ADR-0045): every account, feed,
    /// invite, session, pairing mailbox and recovery blob is gone. Configuration, the clock and the
    /// knobs stay.
    pub fn reset_data(&self) {
        let mut inner = self.lock();
        inner.server_epoch = inner.random();
        inner.accounts.clear();
        inner.devices.clear();
        inner.sessions.clear();
        inner.feeds.clear();
        inner.invites.clear();
        inner.open_calls.clear();
        inner.links.clear();
        inner.link_reads.clear();
        inner.mailboxes.clear();
        inner.recovery.clear();
        inner.sockets.clear();
        inner.last_cursor = 0;
    }

    /// The feed's items as stored: `(cursor, bytes)` in cursor order, pruned ones left out; empty for
    /// an unknown feed. No session, no adversary.
    #[must_use]
    pub fn inspect_feed(&self, feed: FeedId) -> Vec<(u64, Vec<u8>)> {
        self.lock()
            .feeds
            .get(&feed)
            .map_or_else(Vec::new, |stored| {
                stored
                    .items
                    .iter()
                    .filter(|item| !item.pruned)
                    .map(|item| (item.cursor, item.bytes.clone()))
                    .collect()
            })
    }

    /// The access list of a feed as the server derived it from the folded log: each current member
    /// with its role, in join order.
    #[must_use]
    pub fn feed_members(&self, feed: FeedId) -> Vec<(AccountId, MemberRole)> {
        self.lock()
            .feeds
            .get(&feed)
            .and_then(|stored| stored.rules.as_ref())
            .map_or_else(Vec::new, |rules| {
                rules
                    .members()
                    .iter()
                    .map(|member| (member.account, member.role))
                    .collect()
            })
    }

    /// Bytes counted against the billing account of the feed (entries, snapshots, heads and chunks).
    #[must_use]
    pub fn feed_bytes_used(&self, feed: FeedId) -> u64 {
        self.lock()
            .feeds
            .get(&feed)
            .map_or(0, |stored| stored.bytes_used)
    }

    /// The account the feed's bytes count against (its creator until it leaves, then the
    /// earliest-joined remaining Owner).
    #[must_use]
    pub fn billing_account(&self, feed: FeedId) -> Option<AccountId> {
        self.lock().feeds.get(&feed).map(|stored| stored.billing)
    }

    /// Bytes counted against an account: its personal feed plus the feeds it is billed for.
    #[must_use]
    pub fn account_bytes_used(&self, account: AccountId) -> u64 {
        self.lock().billed(account)
    }

    /// Whether the feed's `pending_rekey` flag is set (a `Leave`, or a device revocation in a
    /// member's log that no `Rekey` covers).
    #[must_use]
    pub fn pending_rekey(&self, feed: FeedId) -> bool {
        self.lock().pending_rekey(feed)
    }

    /// Whether the feed's namespace holds the chunk.
    #[must_use]
    pub fn has_chunk(&self, feed: FeedId, hash: ChunkId) -> bool {
        self.lock()
            .feeds
            .get(&feed)
            .is_some_and(|stored| stored.chunks.contains_key(&hash))
    }

    /// The newest cursor of a feed (0 when it is empty or unknown).
    #[must_use]
    pub fn last_cursor(&self, feed: FeedId) -> u64 {
        self.lock().feeds.get(&feed).map_or(0, Feed::last_cursor)
    }

    /// Whether the feed was closed and when (server clock, Unix ms).
    #[must_use]
    pub fn closed_at(&self, feed: FeedId) -> Option<i64> {
        self.lock()
            .feeds
            .get(&feed)
            .and_then(|stored| stored.closed_at)
    }
}

impl RemoteStore for MemoryRemote {
    async fn server_info(&self) -> Result<ServerInfo, SyncError> {
        self.server_info_sync()
    }
    async fn create_account(&self, invite_token: &str, log: &[Vec<u8>]) -> Result<(), SyncError> {
        self.create_account_sync(invite_token, log)
    }
    async fn account_create_share(
        &self,
        log: &[Vec<u8>],
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<(), SyncError> {
        self.account_create_share_sync(log, invite_id, sig)
    }
    async fn append_log(&self, account: AccountId, entries: &[Vec<u8>]) -> Result<(), SyncError> {
        self.append_log_sync(account, entries)
    }
    async fn fetch_log(
        &self,
        account: AccountId,
        after: Option<u64>,
    ) -> Result<Vec<Vec<u8>>, SyncError> {
        self.fetch_log_sync(account, after)
    }
    async fn logs_batch(&self, requests: &[LogRequest]) -> Result<Vec<LogChunk>, SyncError> {
        self.logs_batch_sync(requests)
    }
    async fn authenticate(
        &self,
        account: AccountId,
        device: DeviceId,
        sign: &dyn Fn(&[u8]) -> Signature,
    ) -> Result<(), SyncError> {
        self.authenticate_sync(account, device, sign)
    }
    async fn feeds_create(&self, create: &[u8]) -> Result<FeedCreated, SyncError> {
        self.feeds_create_sync(create)
    }
    async fn feeds_list(&self) -> Result<Vec<FeedListing>, SyncError> {
        self.feeds_list_sync()
    }
    async fn membership_append(
        &self,
        feed: FeedId,
        entries: &[Vec<u8>],
    ) -> Result<MembershipHead, SyncError> {
        self.membership_append_sync(feed, entries)
    }
    async fn membership_get(
        &self,
        feed: FeedId,
        after_seq: Option<u64>,
    ) -> Result<MembershipPage, SyncError> {
        self.membership_get_sync(feed, after_seq)
    }
    async fn append(&self, feed: FeedId, entry: &[u8]) -> Result<(u64, [u8; 32]), SyncError> {
        self.append_sync(feed, entry)
    }
    async fn changes(
        &self,
        feed: FeedId,
        after: u64,
        limit: u32,
    ) -> Result<ChangesPage, SyncError> {
        self.changes_sync(feed, after, limit)
    }
    async fn put_snapshot(&self, feed: FeedId, snapshot: &[u8]) -> Result<(), SyncError> {
        self.put_snapshot_sync(feed, snapshot)
    }
    async fn get_snapshot(&self, feed: FeedId, doc: DocId) -> Result<Option<Vec<u8>>, SyncError> {
        self.get_snapshot_sync(feed, doc)
    }
    async fn snapshots(&self, feed: FeedId) -> Result<SnapshotsPage, SyncError> {
        self.snapshots_sync(feed)
    }
    async fn snapshot_marks(&self, feed: FeedId, after: u64) -> Result<MarksPage, SyncError> {
        self.snapshot_marks_sync(feed, after)
    }
    async fn snapshot_dispute(
        &self,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
    ) -> Result<(), SyncError> {
        self.snapshot_dispute_sync(feed, doc, covers_cursor)
    }
    async fn put_head(&self, feed: FeedId, head: &[u8]) -> Result<(), SyncError> {
        self.put_head_sync(feed, head)
    }
    async fn heads(&self, feed: FeedId) -> Result<Vec<Vec<u8>>, SyncError> {
        self.heads_sync(feed)
    }
    async fn put_chunk(&self, feed: FeedId, hash: ChunkId, bytes: &[u8]) -> Result<(), SyncError> {
        self.put_chunk_sync(feed, hash, bytes)
    }
    async fn get_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<Option<Vec<u8>>, SyncError> {
        self.get_chunk_sync(feed, hash)
    }
    async fn delete_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError> {
        self.delete_chunk_sync(feed, hash)
    }
    async fn copy_chunk(&self, feed: FeedId, hash: ChunkId) -> Result<(), SyncError> {
        self.copy_chunk_sync(feed, hash)
    }
    async fn invite_open(
        &self,
        invite_id: InviteId,
        sig: &Signature,
    ) -> Result<InviteOpened, SyncError> {
        self.invite_open_sync(invite_id, sig)
    }
    async fn link_put(&self, link_id: LinkId, put: &LinkPut) -> Result<u64, SyncError> {
        self.link_put_sync(link_id, put)
    }
    async fn link_get(
        &self,
        link_id: LinkId,
        if_none_match: Option<u64>,
    ) -> Result<LinkFetch, SyncError> {
        self.link_get_sync(link_id, if_none_match)
    }
    async fn link_delete(&self, link_id: LinkId) -> Result<(), SyncError> {
        self.link_delete_sync(link_id)
    }
    async fn link_chunk_get(&self, link_id: LinkId, hash: ChunkId) -> Result<Vec<u8>, SyncError> {
        self.link_chunk_get_sync(link_id, hash)
    }
    async fn pair_create(&self, offer: &[u8]) -> Result<([u8; 6], i64), SyncError> {
        self.pair_create_sync(offer)
    }
    async fn pair_messages(
        &self,
        id: [u8; 6],
        after: u32,
        wait_seconds: u32,
    ) -> Result<PairingPage, SyncError> {
        self.pair_messages_sync(id, after, wait_seconds)
    }
    async fn pair_post(&self, id: [u8; 6], role: u8, bytes: &[u8]) -> Result<(), SyncError> {
        self.pair_post_sync(id, role, bytes)
    }
    async fn pair_close(&self, id: [u8; 6]) -> Result<(), SyncError> {
        self.pair_close_sync(id)
    }
    async fn recovery_put(&self, id: [u8; 32], bytes: &[u8]) -> Result<(), SyncError> {
        self.recovery_put_sync(id, bytes)
    }
    async fn recovery_get(&self, id: [u8; 32]) -> Result<Option<Vec<u8>>, SyncError> {
        self.recovery_get_sync(id)
    }
    async fn recovery_delete(&self, id: [u8; 32]) -> Result<(), SyncError> {
        self.recovery_delete_sync(id)
    }
}
