//! The feeds of protocol v2 (M13, ADR-0043 to ADR-0046, `docs/security/sharing.md` section 6) in the
//! server's SQLite file.
//!
//! Tables (schema version 3; the names of `sharing.md` section 6, with a `feed_` prefix): `devices` (a device id belongs to one
//! account), `feeds` (kind, billing account, epoch, `pending_rekey`, `closed_at`), `feed_members`
//! (the materialised view of the verified log: role, joined cursor, removed cursor), `feed_invites`,
//! `feed_entries` (the feed: the global autoincrement is the cursor, filtered by feed; unique on
//! `(feed, kind, doc, device, seq)`, membership entries included), `feed_snapshots`,
//! `feed_snapshot_marks`, `feed_snapshot_disputes`, `feed_chunks`, `feed_heads`, and the view links of slice 4
//! (`links`, `link_chunks`; `store/v2/links.rs`). (`accounts` has
//! `created_via` and `share_invite_id` in `store.rs`.)
//!
//! The rules are those of `kb_sync::v2::guard` (the fold and the checks only a server makes), the
//! same code `MemoryRemote` runs: a membership batch is applied to a copy of the feed's
//! [`FeedRules`], then stored in one transaction. The rules of a feed are kept in memory and rebuilt
//! from the stored membership entries (and the device logs) on first use after a start.
//!
//! Every call that names a feed takes the session's account, resolves its standing
//! ([`Access`]) and answers `404` for a stranger, `403` for a role that is too low, as the
//! `RemoteStore` documentation says. Ids never reach an error message or a log line.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kb_crypto::{EnvelopeKind, Signature, VerifyingKey};
use kb_keys::membership::{
    EntryBody, InviteRecord, InviteRecordStatus, LogSource, MembershipEntry,
};
use kb_keys::{DeviceLog, LogEntry, LogEntryBody, LogState};
use kb_sync::v2::guard::{Access, BatchContext, BatchErrorKind, FeedRules};
use kb_sync::v2::{
    FeedCreated, FeedItemInfo, FeedListing, InviteOpened, ItemKind, MarksPage, MembershipHead,
    MembershipItem, MembershipPage, SnapshotMark,
};
use kb_types::sharing::{CLOSE_READ_DAYS, MAX_FEEDS_PER_ACCOUNT};
use kb_types::sync::{CHANGES_PAGE_LIMIT, FeedKind};
use kb_types::{AccountId, DeviceId, DocId, FeedId, InviteId, MemberRole};
use rusqlite::{OptionalExtension, Transaction, params};

use super::{Store, array, len_i64, storage, to_i64, to_u64};
use crate::error::HttpError;

mod links;

pub(crate) use links::LinkRead;
pub(super) use links::purge_expired_links;

const DAY_MS: i64 = 86_400_000;

/// The `kind` column of `feed_entries`: the envelope kind byte.
const KIND_ENTRY: i64 = EnvelopeKind::Entry as i64;
const KIND_KEYRING: i64 = EnvelopeKind::Keyring as i64;
const KIND_MEMBERSHIP: i64 = EnvelopeKind::Membership as i64;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS devices (
  device_id BLOB PRIMARY KEY,
  account_id BLOB NOT NULL REFERENCES accounts(account_id)
);
CREATE TABLE IF NOT EXISTS feeds (
  feed_id BLOB PRIMARY KEY,
  kind INTEGER NOT NULL,
  billing_account BLOB NOT NULL REFERENCES accounts(account_id),
  created_at INTEGER NOT NULL,
  bytes_used INTEGER NOT NULL DEFAULT 0,
  member_seq INTEGER,
  member_head BLOB,
  epoch INTEGER NOT NULL DEFAULT 0,
  pending_rekey INTEGER NOT NULL DEFAULT 0,
  closed_at INTEGER
);
CREATE INDEX IF NOT EXISTS feeds_billing ON feeds (billing_account);
CREATE TABLE IF NOT EXISTS feed_members (
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  account BLOB NOT NULL,
  role INTEGER,
  joined_cursor INTEGER NOT NULL,
  removed_cursor INTEGER,
  PRIMARY KEY (feed, account)
);
CREATE INDEX IF NOT EXISTS feed_members_account ON feed_members (account);
CREATE TABLE IF NOT EXISTS feed_invites (
  invite_id BLOB PRIMARY KEY,
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  invite_pub BLOB NOT NULL,
  role INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  bound_account BLOB,
  created_by BLOB NOT NULL,
  used_by BLOB,
  revoked INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS feed_invites_feed ON feed_invites (feed);
CREATE INDEX IF NOT EXISTS feed_invites_creator ON feed_invites (created_by);
CREATE TABLE IF NOT EXISTS feed_entries (
  cursor INTEGER PRIMARY KEY AUTOINCREMENT,
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  kind INTEGER NOT NULL,
  doc_id BLOB NOT NULL,
  account_id BLOB NOT NULL,
  device_id BLOB NOT NULL,
  device_seq INTEGER NOT NULL,
  bytes BLOB NOT NULL,
  chain_hash BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  pruned INTEGER NOT NULL DEFAULT 0,
  UNIQUE (feed, kind, doc_id, device_id, device_seq)
);
CREATE INDEX IF NOT EXISTS feed_entries_feed ON feed_entries (feed, cursor);
CREATE INDEX IF NOT EXISTS feed_entries_doc ON feed_entries (feed, doc_id, cursor);
CREATE TABLE IF NOT EXISTS feed_snapshots (
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  doc_id BLOB NOT NULL,
  device_id BLOB NOT NULL,
  account_id BLOB NOT NULL,
  covers_cursor INTEGER NOT NULL,
  prune_covers INTEGER NOT NULL,
  bytes BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (feed, doc_id)
);
CREATE TABLE IF NOT EXISTS feed_snapshot_marks (
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  mark INTEGER NOT NULL,
  doc_id BLOB NOT NULL,
  device_id BLOB NOT NULL,
  account_id BLOB NOT NULL,
  covers_cursor INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (feed, mark)
);
CREATE TABLE IF NOT EXISTS feed_snapshot_disputes (
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  doc_id BLOB NOT NULL,
  covers_cursor INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (feed, doc_id, covers_cursor)
);
CREATE TABLE IF NOT EXISTS feed_chunks (
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  hash BLOB NOT NULL,
  uploader BLOB NOT NULL,
  bytes BLOB NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (feed, hash)
);
CREATE TABLE IF NOT EXISTS feed_heads (
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  device_id BLOB NOT NULL,
  account_id BLOB NOT NULL,
  bytes BLOB NOT NULL,
  cursor INTEGER NOT NULL,
  seen_at INTEGER NOT NULL,
  PRIMARY KEY (feed, device_id)
);
CREATE TABLE IF NOT EXISTS links (
  link_id BLOB PRIMARY KEY,
  feed BLOB NOT NULL REFERENCES feeds(feed_id) ON DELETE CASCADE,
  created_by BLOB NOT NULL,
  publication BLOB NOT NULL,
  pub_seq INTEGER NOT NULL,
  expires_at INTEGER
);
CREATE INDEX IF NOT EXISTS links_feed ON links (feed);
CREATE INDEX IF NOT EXISTS links_expiry ON links (expires_at) WHERE expires_at IS NOT NULL;
CREATE TABLE IF NOT EXISTS link_chunks (
  link_id BLOB NOT NULL REFERENCES links(link_id) ON DELETE CASCADE,
  hash BLOB NOT NULL,
  PRIMARY KEY (link_id, hash)
);
";

/// Creates the tables of the feeds.
pub(super) fn migrate(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(SCHEMA)
}

/// Records the devices of a verified log as the account's; a device id that another account
/// holds is `Conflict` (device ids are unique across the server's accounts).
pub(super) fn register_devices(
    tx: &Transaction<'_>,
    account: AccountId,
    state: &LogState,
) -> Result<(), HttpError> {
    for record in &state.devices {
        let owner: Option<Vec<u8>> = tx
            .query_row(
                "SELECT account_id FROM devices WHERE device_id = ?1",
                [record.device_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        match owner {
            Some(owner) if owner.as_slice() != account.as_bytes().as_slice() => {
                return Err(HttpError::conflict(
                    "a device id of the log is used by another account",
                ));
            }
            Some(_) => {}
            None => {
                tx.execute(
                    "INSERT INTO devices (device_id, account_id) VALUES (?1, ?2)",
                    params![
                        record.device_id.as_bytes().as_slice(),
                        account.as_bytes().as_slice()
                    ],
                )
                .map_err(storage)?;
            }
        }
    }
    Ok(())
}

/// The device logs of some accounts, as the fold reads them.
struct LogMap(HashMap<AccountId, Arc<DeviceLog>>);

impl LogSource for LogMap {
    fn device_log(&self, account: AccountId) -> Option<&DeviceLog> {
        self.0.get(&account).map(|log| &**log)
    }
}

/// The session's standing in a feed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Standing {
    /// What the account may do.
    pub(crate) access: Access,
    /// The feed is the account's own personal feed.
    pub(crate) personal: bool,
    /// The account whose quota the feed's bytes count against.
    pub(crate) billing: AccountId,
    /// An Owner closed the feed.
    pub(crate) closed: bool,
}

impl Standing {
    /// The role if it is at least `min` (`404` for a stranger or a removed account, `403` for a
    /// role that is too low).
    pub(crate) fn require(&self, min: MemberRole) -> Result<MemberRole, HttpError> {
        self.access.require(min).map_err(HttpError::from)
    }
}

/// Where an appended item landed.
#[derive(Debug, Clone)]
pub(crate) struct Appended {
    pub(crate) cursor: u64,
    pub(crate) chain_hash: [u8; 32],
    /// `false` when the item was already stored (deduplicated).
    pub(crate) new: bool,
    /// The accounts whose devices should be told that the feed grew: the members of a shared feed,
    /// the account itself for its personal feed.
    pub(crate) notify: Vec<AccountId>,
    /// The cursor of the item before this one in the feed (0 when it is the first); the position
    /// the inline frame of a WebSocket carries. 0 for a duplicate.
    pub(crate) prev_cursor: u64,
}

/// Where one membership entry of a batch landed (for the WebSocket frames).
#[derive(Debug, Clone, Copy)]
pub(crate) struct StoredAt {
    /// The cursor of the item before it in the feed.
    pub(crate) prev_cursor: u64,
    /// Its own cursor.
    pub(crate) cursor: u64,
    /// The chain hash after it.
    pub(crate) chain_hash: [u8; 32],
}

/// One page of a feed.
#[derive(Debug, Clone)]
pub(crate) struct FeedPage {
    pub(crate) changes: Vec<(u64, Vec<u8>)>,
    pub(crate) chain_hash_after: [u8; 32],
    pub(crate) more: bool,
}

/// The answer of a membership append.
#[derive(Debug, Clone)]
pub(crate) struct MembershipAppended {
    pub(crate) head: MembershipHead,
    pub(crate) entries: usize,
    /// The members after the batch.
    pub(crate) notify: Vec<AccountId>,
    /// The members the batch ended (a `Remove` or a `Leave`), with the cursor of the entry that did:
    /// they are told of it and of nothing after it.
    pub(crate) ended: Vec<(AccountId, u64)>,
    /// Where each entry of the batch landed, in batch order.
    pub(crate) stored_at: Vec<StoredAt>,
}

/// What one pruning pass of the feeds did.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FeedPrune {
    pub(crate) entries: u64,
    pub(crate) bytes: u64,
    pub(crate) feeds: u64,
}

/// The share signup limits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SignupLimits {
    pub(crate) total: u32,
    pub(crate) per_account: u32,
}

fn envelope_kind(kind: ItemKind) -> i64 {
    match kind {
        ItemKind::Entry => KIND_ENTRY,
        ItemKind::Keyring => KIND_KEYRING,
        ItemKind::Membership => KIND_MEMBERSHIP,
    }
}

fn hash16(blob: &[u8]) -> Result<[u8; 16], HttpError> {
    array::<16>(blob)
}

/// The accounts a batch of membership entries names: signers, joiners, members of a Rekey and
/// whoever an entry is about.
fn named_accounts(entries: &[MembershipEntry]) -> Vec<AccountId> {
    let mut out = Vec::new();
    for entry in entries {
        out.push(entry.header.account);
        match &entry.header.body {
            EntryBody::Create { member_heads, .. } => {
                out.extend(member_heads.iter().map(|m| m.account));
            }
            EntryBody::Join { account, .. } => out.push(*account),
            EntryBody::RoleSet { target, .. } => out.push(*target),
            EntryBody::Remove { target, rekey } => {
                out.push(*target);
                out.extend(rekey.member_heads.iter().map(|m| m.account));
            }
            EntryBody::Rekey(rekey) => out.extend(rekey.member_heads.iter().map(|m| m.account)),
            EntryBody::Grant { member, .. } => out.push(*member),
            _ => {}
        }
    }
    out
}

impl Store {
    // ----- logs and rules ------------------------------------------------------------------

    /// The account's verified device log (cached; dropped when the log grows).
    fn device_log(&mut self, account: AccountId) -> Result<Option<Arc<DeviceLog>>, HttpError> {
        if let Some(log) = self.device_logs.get(&account) {
            return Ok(Some(Arc::clone(log)));
        }
        let entries = self.load_log_entries(account)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let log = DeviceLog::from_entries(entries)
            .map_err(|_| HttpError::internal("the stored device log does not verify"))?;
        let log = Arc::new(log);
        self.device_logs.insert(account, Arc::clone(&log));
        Ok(Some(log))
    }

    /// The device logs of the given accounts that the server knows.
    fn logs_for(
        &mut self,
        accounts: impl IntoIterator<Item = AccountId>,
    ) -> Result<LogMap, HttpError> {
        let mut map = HashMap::new();
        for account in accounts {
            if map.contains_key(&account) {
                continue;
            }
            if let Some(log) = self.device_log(account)? {
                map.insert(account, log);
            }
        }
        Ok(LogMap(map))
    }

    /// Makes sure the rules of a shared feed are in memory (rebuilt from its stored membership
    /// log). The feed must exist and be shared.
    fn ensure_rules(&mut self, feed: FeedId) -> Result<(), HttpError> {
        if self.rules.contains_key(&feed) {
            return Ok(());
        }
        let rows: Vec<(i64, i64, Vec<u8>)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT cursor, created_at, bytes FROM feed_entries
                     WHERE feed = ?1 AND kind = ?2 ORDER BY cursor",
                )
                .map_err(storage)?;
            let rows = stmt
                .query_map(
                    params![feed.as_bytes().as_slice(), KIND_MEMBERSHIP],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(storage)?;
            rows.collect::<Result<_, _>>().map_err(storage)?
        };
        let decoded: Vec<MembershipEntry> = rows
            .iter()
            .map(|(_, _, bytes)| {
                MembershipEntry::decode(bytes)
                    .map_err(|_| HttpError::internal("a stored membership entry is malformed"))
            })
            .collect::<Result<_, _>>()?;
        let logs = self.logs_for(named_accounts(&decoded))?;
        let rules = FeedRules::rebuild(
            feed,
            rows.iter()
                .map(|(cursor, at, bytes)| (to_u64(*cursor), *at, bytes.as_slice())),
            &logs,
        )
        .map_err(|_| HttpError::internal("a stored membership log does not fold"))?;
        self.rules.insert(feed, rules);
        Ok(())
    }

    /// The row of a feed: `(kind, billing account, closed_at)`.
    fn feed_row(
        &self,
        feed: FeedId,
    ) -> Result<Option<(FeedKind, AccountId, Option<i64>)>, HttpError> {
        let row: Option<(i64, Vec<u8>, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT kind, billing_account, closed_at FROM feeds WHERE feed_id = ?1",
                [feed.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        row.map(|(kind, billing, closed_at)| {
            let kind = u8::try_from(kind)
                .ok()
                .and_then(FeedKind::from_byte)
                .ok_or_else(|| HttpError::internal("a stored feed kind is unknown"))?;
            Ok((kind, AccountId::from_bytes(hash16(&billing)?), closed_at))
        })
        .transpose()
    }

    /// The session account's standing in `feed`: its own personal feed (created on first use), a
    /// shared feed by its folded membership log, or a stranger's.
    pub(crate) fn standing(
        &mut self,
        account: AccountId,
        feed: FeedId,
        now_ms: i64,
    ) -> Result<Standing, HttpError> {
        if feed == FeedId::from_account(account) {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO feeds (feed_id, kind, billing_account, created_at)
                     VALUES (?1, 1, ?2, ?3)",
                    params![
                        feed.as_bytes().as_slice(),
                        account.as_bytes().as_slice(),
                        now_ms
                    ],
                )
                .map_err(storage)?;
            return Ok(Standing {
                access: Access::Member(MemberRole::Owner),
                personal: true,
                billing: account,
                closed: false,
            });
        }
        let stranger = Standing {
            access: Access::Stranger,
            personal: false,
            billing: account,
            closed: false,
        };
        let Some((kind, billing, closed_at)) = self.feed_row(feed)? else {
            return Ok(stranger);
        };
        if kind != FeedKind::Shared {
            return Ok(stranger);
        }
        self.ensure_rules(feed)?;
        let access = self
            .rules
            .get(&feed)
            .map_or(Access::Stranger, |rules| rules.access(account));
        Ok(Standing {
            access,
            personal: false,
            billing,
            closed: closed_at.is_some(),
        })
    }

    /// Whether a shared feed waits for a `Rekey` (a `Leave` or an uncovered device revocation).
    fn pending_rekey(&mut self, feed: FeedId) -> Result<bool, HttpError> {
        self.ensure_rules(feed)?;
        let members: Vec<AccountId> = self
            .rules
            .get(&feed)
            .map(|rules| rules.members().iter().map(|m| m.account).collect())
            .unwrap_or_default();
        let logs = self.logs_for(members)?;
        Ok(self
            .rules
            .get(&feed)
            .is_some_and(|rules| rules.pending_rekey(&logs)))
    }

    /// The accounts that are members of a shared feed (the ones to tell when it grows).
    fn members_of(&self, feed: FeedId) -> Vec<AccountId> {
        self.rules
            .get(&feed)
            .map(|rules| rules.members().iter().map(|m| m.account).collect())
            .unwrap_or_default()
    }

    // ----- helpers over the tables ---------------------------------------------------------

    /// The next feed cursor (the global autoincrement; cursors only ever grow).
    fn next_cursor(tx: &Transaction<'_>) -> Result<i64, HttpError> {
        let last: Option<i64> = tx
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'feed_entries'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let top: Option<i64> = tx
            .query_row("SELECT MAX(cursor) FROM feed_entries", [], |row| row.get(0))
            .map_err(storage)?;
        Ok(last.unwrap_or(0).max(top.unwrap_or(0)).saturating_add(1))
    }

    /// The chain hash after the feed's newest item (`chain_genesis` when it is empty).
    fn feed_chain(
        tx: &Transaction<'_>,
        epoch: &[u8; 16],
        feed: FeedId,
    ) -> Result<[u8; 32], HttpError> {
        let hash: Option<Vec<u8>> = tx
            .query_row(
                "SELECT chain_hash FROM feed_entries WHERE feed = ?1 ORDER BY cursor DESC LIMIT 1",
                [feed.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        match hash {
            Some(blob) => array(&blob),
            None => Ok(kb_sync::v2::chain_genesis(epoch)),
        }
    }

    /// The feed's newest cursor (0 when it is empty).
    fn feed_last_cursor(conn: &rusqlite::Connection, feed: FeedId) -> Result<u64, HttpError> {
        let cursor: Option<i64> = conn
            .query_row(
                "SELECT MAX(cursor) FROM feed_entries WHERE feed = ?1",
                [feed.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        Ok(cursor.map_or(0, to_u64))
    }

    /// Counts `delta` bytes against the feed and its billing account.
    fn add_feed_bytes(
        tx: &Transaction<'_>,
        feed: FeedId,
        billing: AccountId,
        delta: i64,
    ) -> Result<(), HttpError> {
        tx.execute(
            "UPDATE feeds SET bytes_used = MAX(0, bytes_used + ?2) WHERE feed_id = ?1",
            params![feed.as_bytes().as_slice(), delta],
        )
        .map_err(storage)?;
        Self::add_bytes(tx, billing, delta)
    }

    /// Refuses `delta` more bytes beyond the quota of the billing account.
    fn check_billing(
        tx: &Transaction<'_>,
        billing: AccountId,
        delta: i64,
        quota: u64,
    ) -> Result<(), HttpError> {
        Self::check_quota(Self::bytes_used(tx, billing)?, delta, quota)
    }

    /// Stores a feed item at `cursor` after the feed's current chain; answers the chain hash.
    #[allow(clippy::too_many_arguments)]
    fn insert_item(
        tx: &Transaction<'_>,
        epoch: &[u8; 16],
        feed: FeedId,
        kind: i64,
        info: &FeedItemInfo,
        cursor: i64,
        bytes: &[u8],
        now_ms: i64,
    ) -> Result<[u8; 32], HttpError> {
        let prev = Self::feed_chain(tx, epoch, feed)?;
        let chain = kb_sync::v2::chain_next(&prev, bytes);
        tx.execute(
            "INSERT INTO feed_entries (cursor, feed, kind, doc_id, account_id, device_id,
                                       device_seq, bytes, chain_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                cursor,
                feed.as_bytes().as_slice(),
                kind,
                info.doc_id.as_bytes().as_slice(),
                info.account_id.as_bytes().as_slice(),
                info.device_id.as_bytes().as_slice(),
                to_i64(info.device_seq, "device_seq is out of range")?,
                bytes,
                chain.as_slice(),
                now_ms
            ],
        )
        .map_err(storage)?;
        Ok(chain)
    }

    /// Rewrites the materialised views of a feed (`feed_members`, `feed_invites`, the `feeds`
    /// columns) from its rules.
    fn store_rules(
        tx: &Transaction<'_>,
        feed: FeedId,
        rules: &FeedRules,
        closed_now: Option<i64>,
    ) -> Result<(), HttpError> {
        let feed_bytes = feed.as_bytes().as_slice();
        tx.execute("DELETE FROM feed_members WHERE feed = ?1", [feed_bytes])
            .map_err(storage)?;
        for member in rules.members() {
            tx.execute(
                "INSERT INTO feed_members (feed, account, role, joined_cursor) VALUES (?1, ?2, ?3, ?4)",
                params![
                    feed_bytes,
                    member.account.as_bytes().as_slice(),
                    i64::from(member.role.to_byte()),
                    to_i64(member.joined_cursor, "a cursor is out of range")?
                ],
            )
            .map_err(storage)?;
        }
        for (account, cursor) in rules.departed() {
            tx.execute(
                "INSERT INTO feed_members (feed, account, role, joined_cursor, removed_cursor)
                 VALUES (?1, ?2, NULL, 0, ?3)",
                params![
                    feed_bytes,
                    account.as_bytes().as_slice(),
                    to_i64(*cursor, "a cursor is out of range")?
                ],
            )
            .map_err(storage)?;
        }
        tx.execute("DELETE FROM feed_invites WHERE feed = ?1", [feed_bytes])
            .map_err(storage)?;
        for invite in &rules.state().invites {
            Self::store_invite(tx, feed, invite)?;
        }
        let head = rules.head();
        tx.execute(
            "UPDATE feeds SET member_seq = ?2, member_head = ?3, epoch = ?4, pending_rekey = ?5,
                              closed_at = COALESCE(closed_at, ?6)
             WHERE feed_id = ?1",
            params![
                feed_bytes,
                head.map(|h| to_i64(h.seq, "a seq is out of range"))
                    .transpose()?,
                head.map(|h| h.hash.to_vec()),
                i64::from(rules.epoch()),
                i64::from(rules.state().pending_rekey),
                closed_now
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    fn store_invite(
        tx: &Transaction<'_>,
        feed: FeedId,
        invite: &InviteRecord,
    ) -> Result<(), HttpError> {
        let (used_by, revoked) = match invite.status {
            InviteRecordStatus::Open => (None, false),
            InviteRecordStatus::Used { by } => (Some(by.as_bytes().to_vec()), false),
            InviteRecordStatus::Revoked => (None, true),
        };
        tx.execute(
            "INSERT INTO feed_invites (invite_id, feed, invite_pub, role, expires_at,
                                       bound_account, created_by, used_by, revoked)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                invite.invite_id.as_bytes().as_slice(),
                feed.as_bytes().as_slice(),
                invite.invite_pub.to_bytes().as_slice(),
                i64::from(invite.role.to_byte()),
                invite.expires_at,
                invite.bound_account.map(|a| a.as_bytes().to_vec()),
                invite.created_by.as_bytes().as_slice(),
                used_by,
                i64::from(revoked)
            ],
        )
        .map_err(storage)?;
        Ok(())
    }

    /// Moves the feed's billing to `successor` when its creator is not a member any more.
    fn rebill(
        tx: &Transaction<'_>,
        feed: FeedId,
        billing: AccountId,
        rules: &FeedRules,
    ) -> Result<AccountId, HttpError> {
        if matches!(rules.access(billing), Access::Member(_)) {
            return Ok(billing);
        }
        let Some(successor) = rules.billing_successor() else {
            return Ok(billing);
        };
        let size: i64 = tx
            .query_row(
                "SELECT bytes_used FROM feeds WHERE feed_id = ?1",
                [feed.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        tx.execute(
            "UPDATE feeds SET billing_account = ?2 WHERE feed_id = ?1",
            params![feed.as_bytes().as_slice(), successor.as_bytes().as_slice()],
        )
        .map_err(storage)?;
        Self::add_bytes(tx, billing, size.saturating_neg())?;
        Self::add_bytes(tx, successor, size)?;
        Ok(successor)
    }

    // ----- feeds and membership ------------------------------------------------------------

    /// Creates a shared feed from its `Create` (the entry's signer is the creator and the billing
    /// account; any session may post a verified `Create`, so a member can re-post the log after a
    /// server reset).
    pub(crate) fn feeds_create(
        &mut self,
        bytes: &[u8],
        quota: u64,
        now_ms: i64,
    ) -> Result<FeedCreated, HttpError> {
        let entry = MembershipEntry::decode(bytes)
            .map_err(|error| HttpError::bad_request("encoding", &error.to_string()))?;
        let feed = entry.header.feed;
        let signer = entry.header.account;
        let taken: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM feeds WHERE feed_id = ?1
                 UNION SELECT 1 FROM accounts WHERE account_id = ?1",
                [feed.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if taken.is_some() {
            return Err(HttpError::conflict("the feed id is taken"));
        }
        if entry.kind() != kb_keys::membership::EntryKind::Create {
            return Err(HttpError::bad_request(
                "bad_request",
                "a feed starts with a Create entry",
            ));
        }
        let owned: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM feeds WHERE kind = 2 AND billing_account = ?1",
                [signer.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if usize::try_from(owned).unwrap_or(usize::MAX) >= MAX_FEEDS_PER_ACCOUNT {
            return Err(HttpError::role_forbidden());
        }
        let size = len_i64(bytes);
        let logs = self.logs_for(named_accounts(std::slice::from_ref(&entry)))?;
        let epoch = self.server_epoch;
        let tx = self.conn.transaction().map_err(storage)?;
        let account_known: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM accounts WHERE account_id = ?1",
                [signer.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if account_known.is_none() {
            return Err(HttpError::from(kb_sync::SyncError::Conflict(
                "a device log the entry names is not on the server yet".to_owned(),
            )));
        }
        Self::check_billing(&tx, signer, size, quota)?;
        let cursor = Self::next_cursor(&tx)?;
        let ctx = BatchContext {
            now_ms,
            logs: &logs,
            serve_expired: false,
        };
        let rules = FeedRules::new(feed)
            .apply_batch(&[(to_u64(cursor), entry.clone())], &ctx)
            .map_err(|error| match error.kind {
                BatchErrorKind::Conflict => {
                    HttpError::bad_request("bad_request", "a feed starts with the Create at seq 0")
                }
                _ => HttpError::from(error.sync_error(true)),
            })?;
        tx.execute(
            "INSERT INTO feeds (feed_id, kind, billing_account, created_at) VALUES (?1, 2, ?2, ?3)",
            params![
                feed.as_bytes().as_slice(),
                signer.as_bytes().as_slice(),
                now_ms
            ],
        )
        .map_err(storage)?;
        let info = FeedItemInfo {
            kind: ItemKind::Membership,
            feed,
            account_id: signer,
            doc_id: kb_types::sync::KEYRING_DOC_ID,
            device_id: entry.header.device,
            device_seq: entry.header.seq,
        };
        Self::insert_item(
            &tx,
            &epoch,
            feed,
            KIND_MEMBERSHIP,
            &info,
            cursor,
            bytes,
            now_ms,
        )?;
        Self::add_feed_bytes(&tx, feed, signer, size)?;
        Self::store_rules(&tx, feed, &rules, None)?;
        tx.commit().map_err(storage)?;
        self.rules.insert(feed, rules);
        Ok(FeedCreated {
            feed,
            head: entry.head(),
            cursor: to_u64(cursor),
        })
    }

    /// The feeds the account belongs to or belonged to, the personal feed first.
    pub(crate) fn feeds_list(&mut self, account: AccountId) -> Result<Vec<FeedListing>, HttpError> {
        let mut out = vec![FeedListing {
            feed: FeedId::from_account(account),
            kind: FeedKind::Personal,
            role: None,
            removed: false,
            closed: false,
        }];
        let mut stmt = self
            .conn
            .prepare(
                "SELECT m.feed, m.role, m.removed_cursor, f.closed_at
                 FROM feed_members m JOIN feeds f ON f.feed_id = m.feed
                 WHERE m.account = ?1 ORDER BY f.created_at, m.feed",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map([account.as_bytes().as_slice()], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(storage)?;
        for row in rows {
            let (feed, role, removed, closed) = row.map_err(storage)?;
            let role = role
                .and_then(|byte| u8::try_from(byte).ok())
                .and_then(MemberRole::from_byte);
            out.push(FeedListing {
                feed: FeedId::from_bytes(hash16(&feed)?),
                kind: FeedKind::Shared,
                role,
                removed: removed.is_some(),
                closed: closed.is_some(),
            });
        }
        Ok(out)
    }

    /// Appends membership entries as a compare-and-swap (see `RemoteStore::membership_append`).
    pub(crate) fn membership_append(
        &mut self,
        session: AccountId,
        feed: FeedId,
        entries: &[Vec<u8>],
        quota: u64,
        now_ms: i64,
    ) -> Result<MembershipAppended, HttpError> {
        let decoded: Vec<MembershipEntry> = entries
            .iter()
            .map(|bytes| {
                MembershipEntry::decode(bytes)
                    .map_err(|error| HttpError::bad_request("encoding", &error.to_string()))
            })
            .collect::<Result<_, _>>()?;
        let Some((kind, billing, _)) = self.feed_row(feed)? else {
            return Err(HttpError::not_found("not found"));
        };
        if kind != FeedKind::Shared {
            return Err(HttpError::not_found("not found"));
        }
        self.ensure_rules(feed)?;
        let member = self
            .rules
            .get(&feed)
            .is_some_and(|rules| matches!(rules.access(session), Access::Member(_)));
        if !member
            && !decoded.iter().all(|entry| {
                matches!(entry.header.body, EntryBody::Join { .. })
                    && entry.header.account == session
            })
        {
            // A stranger may only post the `Join`s that its invite proves.
            return Err(HttpError::not_found("not found"));
        }
        let mut accounts = named_accounts(&decoded);
        accounts.extend(self.members_of(feed));
        let logs = self.logs_for(accounts)?;
        let total: i64 = entries.iter().map(|bytes| len_i64(bytes)).sum();
        let epoch = self.server_epoch;
        let tx = self.conn.transaction().map_err(storage)?;
        Self::check_billing(&tx, billing, total, quota)?;
        let first = Self::next_cursor(&tx)?;
        let batch: Vec<(u64, MembershipEntry)> = (to_u64(first)..).zip(decoded).collect();
        let ctx = BatchContext {
            now_ms,
            logs: &logs,
            serve_expired: false,
        };
        let current = self
            .rules
            .get(&feed)
            .ok_or_else(|| HttpError::internal("the rules of the feed are missing"))?;
        let rules = current
            .apply_batch(&batch, &ctx)
            .map_err(|error| HttpError::from(error.sync_error(member)))?;
        let members_before: Vec<AccountId> = current.members().iter().map(|m| m.account).collect();
        // Committed from here on.
        let mut last_head = None;
        let mut previous = Self::feed_last_cursor(&tx, feed)?;
        let mut stored_at = Vec::with_capacity(batch.len());
        for (((cursor, entry), bytes), offset) in batch.iter().zip(entries).zip(0i64..) {
            let info = FeedItemInfo {
                kind: ItemKind::Membership,
                feed,
                account_id: entry.header.account,
                doc_id: kb_types::sync::KEYRING_DOC_ID,
                device_id: entry.header.device,
                device_seq: entry.header.seq,
            };
            let at = i64::try_from(*cursor).unwrap_or(first.saturating_add(offset));
            let chain_hash =
                Self::insert_item(&tx, &epoch, feed, KIND_MEMBERSHIP, &info, at, bytes, now_ms)?;
            stored_at.push(StoredAt {
                prev_cursor: previous,
                cursor: to_u64(at),
                chain_hash,
            });
            previous = to_u64(at);
            last_head = Some(entry.head());
        }
        Self::add_feed_bytes(&tx, feed, billing, total)?;
        let closed_now = (rules.closed() && current_open(&tx, feed)?).then_some(now_ms);
        if closed_now.is_some() {
            // Closing a feed deletes its view links (ADR-0051).
            links::drop_feed_links(&tx, feed)?;
        }
        Self::store_rules(&tx, feed, &rules, closed_now)?;
        Self::rebill(&tx, feed, billing, &rules)?;
        tx.commit().map_err(storage)?;
        let notify = rules.members().iter().map(|m| m.account).collect();
        // Whoever the batch took out of the members, with the cursor it ended at.
        let ended = members_before
            .into_iter()
            .filter_map(|account| match rules.access(account) {
                Access::Removed { cursor } => Some((account, cursor)),
                _ => None,
            })
            .collect();
        self.rules.insert(feed, rules);
        Ok(MembershipAppended {
            head: last_head.ok_or_else(|| HttpError::bad_request("bad_request", "no entries"))?,
            entries: entries.len(),
            notify,
            ended,
            stored_at,
        })
    }

    /// The membership log after `after_seq`; a removed account sees it up to its own `Remove`.
    pub(crate) fn membership_get(
        &mut self,
        account: AccountId,
        feed: FeedId,
        after_seq: Option<u64>,
        now_ms: i64,
    ) -> Result<MembershipPage, HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        if standing.personal {
            return Err(HttpError::not_found("not found"));
        }
        let max_cursor = match standing.access {
            Access::Member(_) => i64::MAX,
            Access::Removed { cursor } => to_i64(cursor, "a cursor is out of range")?,
            Access::Stranger => return Err(HttpError::not_found("not found")),
        };
        let limit = i64::from(CHANGES_PAGE_LIMIT);
        let after = after_seq.map_or(-1, |seq| i64::try_from(seq).unwrap_or(i64::MAX));
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cursor, device_seq, bytes FROM feed_entries
                 WHERE feed = ?1 AND kind = ?2 AND device_seq > ?3 AND cursor <= ?4
                 ORDER BY cursor LIMIT ?5",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(
                params![
                    feed.as_bytes().as_slice(),
                    KIND_MEMBERSHIP,
                    after,
                    max_cursor,
                    limit + 1
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .map_err(storage)?;
        let mut entries = Vec::new();
        let mut more = false;
        for row in rows {
            let (cursor, _, bytes) = row.map_err(storage)?;
            if entries.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
                more = true;
                break;
            }
            entries.push(MembershipItem {
                cursor: to_u64(cursor),
                bytes,
            });
        }
        let head: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT device_seq, bytes FROM feed_entries
                 WHERE feed = ?1 AND kind = ?2 AND cursor <= ?3 ORDER BY cursor DESC LIMIT 1",
                params![feed.as_bytes().as_slice(), KIND_MEMBERSHIP, max_cursor],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        let head = head.map_or(
            MembershipHead {
                seq: 0,
                hash: [0u8; 32],
            },
            |(seq, bytes)| MembershipHead {
                seq: to_u64(seq),
                hash: kb_crypto::hash(&bytes),
            },
        );
        Ok(MembershipPage {
            entries,
            head,
            more,
        })
    }

    // ----- entries -------------------------------------------------------------------------

    /// Appends a sealed entry (document or keyring) to a feed (the header checks and the signature
    /// are the caller's).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_item(
        &mut self,
        account: AccountId,
        feed: FeedId,
        info: &FeedItemInfo,
        key_epoch: u32,
        bytes: &[u8],
        quota: u64,
        now_ms: i64,
    ) -> Result<Appended, HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        standing.require(MemberRole::Commenter)?;
        let kind = envelope_kind(info.kind);
        let existing: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT cursor, chain_hash FROM feed_entries
                 WHERE feed = ?1 AND kind = ?2 AND doc_id = ?3 AND device_id = ?4 AND device_seq = ?5",
                params![
                    feed.as_bytes().as_slice(),
                    kind,
                    info.doc_id.as_bytes().as_slice(),
                    info.device_id.as_bytes().as_slice(),
                    to_i64(info.device_seq, "device_seq is out of range")?
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        if let Some((cursor, hash)) = existing {
            return Ok(Appended {
                cursor: to_u64(cursor),
                chain_hash: array(&hash)?,
                new: false,
                notify: Vec::new(),
                prev_cursor: 0,
            });
        }
        if !standing.personal {
            if standing.closed {
                return Err(HttpError::feed_closed());
            }
            if self.pending_rekey(feed)? {
                return Err(HttpError::rekey_needed());
            }
            let epoch = self.rules.get(&feed).map_or(0, FeedRules::epoch);
            if key_epoch < epoch {
                return Err(HttpError::stale_epoch());
            }
        }
        let size = len_i64(bytes);
        let epoch = self.server_epoch;
        let tx = self.conn.transaction().map_err(storage)?;
        Self::check_billing(&tx, standing.billing, size, quota)?;
        let cursor = Self::next_cursor(&tx)?;
        let prev_cursor = Self::feed_last_cursor(&tx, feed)?;
        let chain = Self::insert_item(&tx, &epoch, feed, kind, info, cursor, bytes, now_ms)?;
        Self::add_feed_bytes(&tx, feed, standing.billing, size)?;
        tx.commit().map_err(storage)?;
        let notify = if standing.personal {
            vec![account]
        } else {
            self.members_of(feed)
        };
        Ok(Appended {
            cursor: to_u64(cursor),
            chain_hash: chain,
            new: true,
            notify,
            prev_cursor,
        })
    }

    /// The items after `after`, at most `limit` and at most `max_bytes` of payload (always at least
    /// one when any follows); a removed account reads up to its `Remove` and is `403` at and past it.
    pub(crate) fn feed_changes(
        &mut self,
        account: AccountId,
        feed: FeedId,
        after: u64,
        limit: u32,
        max_bytes: u64,
        now_ms: i64,
    ) -> Result<FeedPage, HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        let max_cursor = match standing.access {
            Access::Member(_) => i64::MAX,
            Access::Removed { cursor } => {
                if after >= cursor {
                    return Err(HttpError::role_forbidden());
                }
                to_i64(cursor, "a cursor is out of range")?
            }
            Access::Stranger => return Err(HttpError::not_found("not found")),
        };
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cursor, length(bytes), chain_hash, bytes FROM feed_entries
                 WHERE feed = ?1 AND cursor > ?2 AND cursor <= ?3 AND pruned = 0
                 ORDER BY cursor LIMIT ?4",
            )
            .map_err(storage)?;
        let mut rows = stmt
            .query(params![
                feed.as_bytes().as_slice(),
                after,
                max_cursor,
                i64::from(limit).saturating_add(1)
            ])
            .map_err(storage)?;
        let mut changes = Vec::new();
        let mut total: u64 = 0;
        let mut last_hash = None;
        let mut more = false;
        while let Some(row) = rows.next().map_err(storage)? {
            let cursor: i64 = row.get(0).map_err(storage)?;
            let len = to_u64(row.get(1).map_err(storage)?);
            let full = changes.len() >= usize::try_from(limit).unwrap_or(usize::MAX);
            if full || (!changes.is_empty() && total.saturating_add(len) > max_bytes) {
                more = true;
                break;
            }
            let hash: Vec<u8> = row.get(2).map_err(storage)?;
            let bytes: Vec<u8> = row.get(3).map_err(storage)?;
            total = total.saturating_add(len);
            last_hash = Some(array::<32>(&hash)?);
            changes.push((to_u64(cursor), bytes));
        }
        drop(rows);
        drop(stmt);
        let chain_hash_after = match last_hash {
            Some(hash) => hash,
            None => {
                let hash: Option<Vec<u8>> = self
                    .conn
                    .query_row(
                        "SELECT chain_hash FROM feed_entries WHERE feed = ?1 AND cursor <= ?2
                         ORDER BY cursor DESC LIMIT 1",
                        params![feed.as_bytes().as_slice(), after],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;
                match hash {
                    Some(blob) => array(&blob)?,
                    None => kb_sync::v2::chain_genesis(&self.server_epoch),
                }
            }
        };
        Ok(FeedPage {
            changes,
            chain_hash_after,
            more,
        })
    }

    /// Which of `feeds` a WebSocket of `account` may subscribe to: its own personal feed and the
    /// shared feeds it is a current member of (a removed or departed member, a stranger and an unknown
    /// feed are left out, all alike). Answers `(feed, personal)`.
    pub(crate) fn subscribable(
        &mut self,
        account: AccountId,
        feeds: &[FeedId],
        now_ms: i64,
    ) -> Result<Vec<(FeedId, bool)>, HttpError> {
        let mut out = Vec::new();
        for feed in feeds {
            let Ok(standing) = self.standing(account, *feed, now_ms) else {
                continue;
            };
            if matches!(standing.access, Access::Member(_)) && !out.iter().any(|(f, _)| f == feed) {
                out.push((*feed, standing.personal));
            }
        }
        Ok(out)
    }

    /// The newest cursor of each feed (0 when it holds nothing). The WebSocket answers it, as a
    /// `changed` frame per feed, to a connection that lagged behind, so one frame covers every
    /// notice it missed.
    pub(crate) fn feed_head_cursors(
        &self,
        feeds: &[FeedId],
    ) -> Result<Vec<(FeedId, u64)>, HttpError> {
        feeds
            .iter()
            .map(|feed| Ok((*feed, Self::feed_last_cursor(&self.conn, *feed)?)))
            .collect()
    }

    // ----- snapshots -----------------------------------------------------------------------

    /// Replaces a document's snapshot in the feed (Commenter and up; the header checks and the
    /// signature are the caller's). A snapshot that covers less than the stored one is `Conflict`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_feed_snapshot(
        &mut self,
        account: AccountId,
        device: DeviceId,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
        bytes: &[u8],
        quota: u64,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        standing.require(MemberRole::Commenter)?;
        if standing.closed {
            return Err(HttpError::feed_closed());
        }
        let covers = to_i64(covers_cursor, "covers_cursor is out of range")?;
        let tx = self.conn.transaction().map_err(storage)?;
        let old: Option<(i64, i64)> = tx
            .query_row(
                "SELECT covers_cursor, length(bytes) FROM feed_snapshots WHERE feed = ?1 AND doc_id = ?2",
                params![feed.as_bytes().as_slice(), doc.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        let old_len = match old {
            Some((old_covers, _)) if covers < old_covers => {
                return Err(HttpError::conflict(
                    "the snapshot covers less of the feed than the stored one",
                ));
            }
            Some((_, len)) => len,
            None => 0,
        };
        let delta = len_i64(bytes).saturating_sub(old_len);
        Self::check_billing(&tx, standing.billing, delta, quota)?;
        let head = to_i64(
            Self::feed_last_cursor(&tx, feed)?,
            "a cursor is out of range",
        )?;
        tx.execute(
            "INSERT INTO feed_snapshots (feed, doc_id, device_id, account_id, covers_cursor,
                                         prune_covers, bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (feed, doc_id) DO UPDATE SET
               device_id = excluded.device_id, account_id = excluded.account_id,
               covers_cursor = excluded.covers_cursor, prune_covers = excluded.prune_covers,
               bytes = excluded.bytes, created_at = excluded.created_at",
            params![
                feed.as_bytes().as_slice(),
                doc.as_bytes().as_slice(),
                device.as_bytes().as_slice(),
                account.as_bytes().as_slice(),
                covers,
                covers.min(head),
                bytes,
                now_ms
            ],
        )
        .map_err(storage)?;
        // The snapshot it replaced is gone, and so is any dispute of it.
        tx.execute(
            "DELETE FROM feed_snapshot_disputes WHERE feed = ?1 AND doc_id = ?2",
            params![feed.as_bytes().as_slice(), doc.as_bytes().as_slice()],
        )
        .map_err(storage)?;
        let mark: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(mark), 0) + 1 FROM feed_snapshot_marks WHERE feed = ?1",
                [feed.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        tx.execute(
            "INSERT INTO feed_snapshot_marks (feed, mark, doc_id, device_id, account_id,
                                              covers_cursor, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                feed.as_bytes().as_slice(),
                mark,
                doc.as_bytes().as_slice(),
                device.as_bytes().as_slice(),
                account.as_bytes().as_slice(),
                covers,
                now_ms
            ],
        )
        .map_err(storage)?;
        Self::add_feed_bytes(&tx, feed, standing.billing, delta)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    /// A document's latest snapshot (any member).
    pub(crate) fn feed_snapshot(
        &mut self,
        account: AccountId,
        feed: FeedId,
        doc: DocId,
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>, HttpError> {
        self.standing(account, feed, now_ms)?
            .require(MemberRole::Viewer)?;
        self.conn
            .query_row(
                "SELECT bytes FROM feed_snapshots WHERE feed = ?1 AND doc_id = ?2",
                params![feed.as_bytes().as_slice(), doc.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    /// Every document's latest snapshot, by document id (any member).
    pub(crate) fn feed_snapshots(
        &mut self,
        account: AccountId,
        feed: FeedId,
        now_ms: i64,
    ) -> Result<Vec<Vec<u8>>, HttpError> {
        self.standing(account, feed, now_ms)?
            .require(MemberRole::Viewer)?;
        let mut stmt = self
            .conn
            .prepare("SELECT bytes FROM feed_snapshots WHERE feed = ?1 ORDER BY doc_id")
            .map_err(storage)?;
        let rows = stmt
            .query_map([feed.as_bytes().as_slice()], |row| row.get(0))
            .map_err(storage)?;
        rows.collect::<Result<Vec<Vec<u8>>, _>>().map_err(storage)
    }

    /// The snapshot marks after `after` (any member), at most `CHANGES_PAGE_LIMIT`.
    pub(crate) fn feed_snapshot_marks(
        &mut self,
        account: AccountId,
        feed: FeedId,
        after: u64,
        now_ms: i64,
    ) -> Result<MarksPage, HttpError> {
        self.standing(account, feed, now_ms)?
            .require(MemberRole::Viewer)?;
        let limit = i64::from(CHANGES_PAGE_LIMIT);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT mark, doc_id, device_id, account_id, covers_cursor FROM feed_snapshot_marks
                 WHERE feed = ?1 AND mark > ?2 ORDER BY mark LIMIT ?3",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(
                params![
                    feed.as_bytes().as_slice(),
                    i64::try_from(after).unwrap_or(i64::MAX),
                    limit + 1
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(storage)?;
        let mut marks = Vec::new();
        let mut more = false;
        for row in rows {
            let (mark, doc, device, account, covers) = row.map_err(storage)?;
            if marks.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
                more = true;
                break;
            }
            marks.push(SnapshotMark {
                mark: to_u64(mark),
                doc_id: DocId::from_bytes(hash16(&doc)?),
                device_id: DeviceId::from_bytes(hash16(&device)?),
                account_id: AccountId::from_bytes(hash16(&account)?),
                covers_cursor: to_u64(covers),
            });
        }
        Ok(MarksPage { marks, more })
    }

    /// Disputes the latest snapshot of a document (Editor and up).
    pub(crate) fn feed_snapshot_dispute(
        &mut self,
        account: AccountId,
        feed: FeedId,
        doc: DocId,
        covers_cursor: u64,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        self.standing(account, feed, now_ms)?
            .require(MemberRole::Editor)?;
        let current: Option<i64> = self
            .conn
            .query_row(
                "SELECT covers_cursor FROM feed_snapshots WHERE feed = ?1 AND doc_id = ?2",
                params![feed.as_bytes().as_slice(), doc.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(current) = current else {
            return Err(HttpError::not_found("not found"));
        };
        let covers = to_i64(covers_cursor, "covers_cursor is out of range")?;
        if current == covers {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO feed_snapshot_disputes (feed, doc_id, covers_cursor, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        feed.as_bytes().as_slice(),
                        doc.as_bytes().as_slice(),
                        covers,
                        now_ms
                    ],
                )
                .map_err(storage)?;
        }
        Ok(())
    }

    // ----- heads ---------------------------------------------------------------------------

    /// Replaces a device's head in the feed (any member; the checks of the header and the
    /// signature are the caller's). A cursor the feed has not reached is refused.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_feed_head(
        &mut self,
        account: AccountId,
        device: DeviceId,
        feed: FeedId,
        cursor: u64,
        bytes: &[u8],
        quota: u64,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        standing.require(MemberRole::Viewer)?;
        if standing.closed {
            return Err(HttpError::feed_closed());
        }
        let tx = self.conn.transaction().map_err(storage)?;
        if cursor > Self::feed_last_cursor(&tx, feed)? {
            return Err(HttpError::bad_request(
                "bad_request",
                "the head names a cursor the feed has not reached",
            ));
        }
        let old_len: i64 = tx
            .query_row(
                "SELECT length(bytes) FROM feed_heads WHERE feed = ?1 AND device_id = ?2",
                params![feed.as_bytes().as_slice(), device.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?
            .unwrap_or(0);
        let delta = len_i64(bytes).saturating_sub(old_len);
        Self::check_billing(&tx, standing.billing, delta, quota)?;
        tx.execute(
            "INSERT INTO feed_heads (feed, device_id, account_id, bytes, cursor, seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (feed, device_id) DO UPDATE SET
               account_id = excluded.account_id, bytes = excluded.bytes,
               cursor = excluded.cursor, seen_at = excluded.seen_at",
            params![
                feed.as_bytes().as_slice(),
                device.as_bytes().as_slice(),
                account.as_bytes().as_slice(),
                bytes,
                to_i64(cursor, "the head cursor is out of range")?,
                now_ms
            ],
        )
        .map_err(storage)?;
        Self::add_feed_bytes(&tx, feed, standing.billing, delta)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    /// Every device's latest head in the feed, by device id (any member).
    pub(crate) fn feed_heads(
        &mut self,
        account: AccountId,
        feed: FeedId,
        now_ms: i64,
    ) -> Result<Vec<Vec<u8>>, HttpError> {
        self.standing(account, feed, now_ms)?
            .require(MemberRole::Viewer)?;
        let mut stmt = self
            .conn
            .prepare("SELECT bytes FROM feed_heads WHERE feed = ?1 ORDER BY device_id")
            .map_err(storage)?;
        let rows = stmt
            .query_map([feed.as_bytes().as_slice()], |row| row.get(0))
            .map_err(storage)?;
        rows.collect::<Result<Vec<Vec<u8>>, _>>().map_err(storage)
    }

    // ----- chunks --------------------------------------------------------------------------

    /// Stores a chunk in the feed's namespace (Editor and up in a shared feed); storing it again
    /// changes nothing.
    pub(crate) fn put_feed_chunk(
        &mut self,
        account: AccountId,
        feed: FeedId,
        hash: &[u8; 32],
        bytes: &[u8],
        quota: u64,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        standing.require(MemberRole::Editor)?;
        if standing.closed {
            return Err(HttpError::feed_closed());
        }
        let tx = self.conn.transaction().map_err(storage)?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM feed_chunks WHERE feed = ?1 AND hash = ?2",
                params![feed.as_bytes().as_slice(), hash.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if exists.is_some() {
            return Ok(());
        }
        let size = len_i64(bytes);
        Self::check_billing(&tx, standing.billing, size, quota)?;
        tx.execute(
            "INSERT INTO feed_chunks (feed, hash, uploader, bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                feed.as_bytes().as_slice(),
                hash.as_slice(),
                account.as_bytes().as_slice(),
                bytes,
                now_ms
            ],
        )
        .map_err(storage)?;
        Self::add_feed_bytes(&tx, feed, standing.billing, size)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    /// The feed's chunk (any member).
    pub(crate) fn feed_chunk(
        &mut self,
        account: AccountId,
        feed: FeedId,
        hash: &[u8; 32],
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>, HttpError> {
        self.standing(account, feed, now_ms)?
            .require(MemberRole::Viewer)?;
        self.conn
            .query_row(
                "SELECT bytes FROM feed_chunks WHERE feed = ?1 AND hash = ?2",
                params![feed.as_bytes().as_slice(), hash.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    /// Deletes the feed's chunk (an Owner of a shared feed, the account itself for its personal
    /// feed) and gives its bytes back to the billing account; a chunk the feed does not hold is fine.
    pub(crate) fn delete_feed_chunk(
        &mut self,
        account: AccountId,
        feed: FeedId,
        hash: &[u8; 32],
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        standing.require(MemberRole::Owner)?;
        if standing.closed {
            return Err(HttpError::feed_closed());
        }
        let tx = self.conn.transaction().map_err(storage)?;
        let size: Option<i64> = tx
            .query_row(
                "SELECT length(bytes) FROM feed_chunks WHERE feed = ?1 AND hash = ?2",
                params![feed.as_bytes().as_slice(), hash.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(size) = size else {
            return Ok(());
        };
        tx.execute(
            "DELETE FROM feed_chunks WHERE feed = ?1 AND hash = ?2",
            params![feed.as_bytes().as_slice(), hash.as_slice()],
        )
        .map_err(storage)?;
        Self::add_feed_bytes(&tx, feed, standing.billing, size.saturating_neg())?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    /// Copies a chunk of the account's personal namespace into the feed's (Editor and up).
    pub(crate) fn copy_feed_chunk(
        &mut self,
        account: AccountId,
        feed: FeedId,
        hash: &[u8; 32],
        quota: u64,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let standing = self.standing(account, feed, now_ms)?;
        standing.require(MemberRole::Editor)?;
        let personal = FeedId::from_account(account);
        self.standing(account, personal, now_ms)?;
        let source: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT bytes FROM feed_chunks WHERE feed = ?1 AND hash = ?2",
                params![personal.as_bytes().as_slice(), hash.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(bytes) = source else {
            return Err(HttpError::not_found("not found"));
        };
        if standing.closed {
            return Err(HttpError::feed_closed());
        }
        let tx = self.conn.transaction().map_err(storage)?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM feed_chunks WHERE feed = ?1 AND hash = ?2",
                params![feed.as_bytes().as_slice(), hash.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if exists.is_some() {
            return Ok(());
        }
        let size = len_i64(&bytes);
        Self::check_billing(&tx, standing.billing, size, quota)?;
        tx.execute(
            "INSERT INTO feed_chunks (feed, hash, uploader, bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                feed.as_bytes().as_slice(),
                hash.as_slice(),
                account.as_bytes().as_slice(),
                bytes,
                now_ms
            ],
        )
        .map_err(storage)?;
        Self::add_feed_bytes(&tx, feed, standing.billing, size)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    // ----- invites and share signups -------------------------------------------------------

    /// The invite if it can still be redeemed: open in its feed's log, the feed is not closed and
    /// the clock has not passed its expiry. `(feed, invite public key, creator, bound account)`.
    #[allow(clippy::type_complexity)]
    fn live_invite(
        &mut self,
        invite_id: InviteId,
        now_ms: i64,
    ) -> Result<Option<(FeedId, VerifyingKey, AccountId, Option<AccountId>)>, HttpError> {
        let feed: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT feed FROM feed_invites WHERE invite_id = ?1",
                [invite_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(feed) = feed else {
            return Ok(None);
        };
        let feed = FeedId::from_bytes(hash16(&feed)?);
        self.ensure_rules(feed)?;
        let Some(rules) = self.rules.get(&feed) else {
            return Ok(None);
        };
        let Some(record) = rules.state().invite(invite_id) else {
            return Ok(None);
        };
        if record.status != InviteRecordStatus::Open || rules.closed() || record.is_expired(now_ms)
        {
            return Ok(None);
        }
        Ok(Some((
            feed,
            record.invite_pub,
            record.created_by,
            record.bound_account,
        )))
    }

    /// Opens an invite: the feed and its whole membership log. A dead invite and a signature that
    /// does not verify are the same `404`.
    pub(crate) fn invite_open(
        &mut self,
        invite_id: InviteId,
        sig: &Signature,
        now_ms: i64,
    ) -> Result<InviteOpened, HttpError> {
        let Some((feed, invite_pub, _, _)) = self.live_invite(invite_id, now_ms)? else {
            return Err(HttpError::not_found("not found"));
        };
        kb_keys::invite::verify_open(&invite_pub, invite_id, self.server_epoch, sig)
            .map_err(|_| HttpError::not_found("not found"))?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT cursor, bytes FROM feed_entries WHERE feed = ?1 AND kind = ?2 ORDER BY cursor",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(
                params![feed.as_bytes().as_slice(), KIND_MEMBERSHIP],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .map_err(storage)?;
        let mut log = Vec::new();
        for row in rows {
            let (cursor, bytes) = row.map_err(storage)?;
            log.push(MembershipItem {
                cursor: to_u64(cursor),
                bytes,
            });
        }
        Ok(InviteOpened { feed, log })
    }

    /// Creates an account through a share invite (see `RemoteStore::account_create_share`).
    pub(crate) fn account_create_share(
        &mut self,
        entries: Vec<LogEntry>,
        invite_id: InviteId,
        sig: &Signature,
        limits: SignupLimits,
        now_ms: i64,
    ) -> Result<AccountId, HttpError> {
        let Some((_, invite_pub, creator, bound)) = self.live_invite(invite_id, now_ms)? else {
            return Err(HttpError::not_found("not found"));
        };
        let log = DeviceLog::from_entries(entries.clone())
            .map_err(|error| super::log_error(&error, false))?;
        let state = log
            .verify()
            .map_err(|error| super::log_error(&error, false))?;
        let account = state.account_id;
        kb_keys::invite::verify_signup(&invite_pub, account, self.server_epoch, sig)
            .map_err(|_| HttpError::bad_signature())?;
        if bound.is_some_and(|bound| bound != account) {
            return Err(HttpError::signup_limit());
        }
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM accounts WHERE account_id = ?1",
                [account.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if exists.is_some() {
            return Err(HttpError::conflict("the account exists"));
        }
        let used: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM accounts WHERE share_invite_id = ?1",
                [invite_id.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let total: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM accounts WHERE created_via = 'share'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let by_creator: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM accounts a JOIN feed_invites i ON a.share_invite_id = i.invite_id
                 WHERE i.created_by = ?1",
                [creator.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if used.is_some()
            || to_u64(total) >= u64::from(limits.total)
            || to_u64(by_creator) >= u64::from(limits.per_account)
        {
            return Err(HttpError::signup_limit());
        }
        self.create_account_via(entries, now_ms, Some(invite_id))
    }

    // ----- idle devices and pruning --------------------------------------------------------

    /// The `(idle, head cursor)` of every active device of every current member of a shared feed.
    fn ack_devices(
        &mut self,
        feed: FeedId,
        now_ms: i64,
        idle_limit_ms: i64,
    ) -> Result<Vec<(bool, Option<u64>)>, HttpError> {
        self.ensure_rules(feed)?;
        let members = self.members_of(feed);
        let mut out = Vec::new();
        for account in members {
            let Some(state) = self.log_state(account)? else {
                continue;
            };
            for record in state.devices.iter().filter(|d| d.revoked_at.is_none()) {
                let seen = self
                    .last_seen(account, record.device_id)?
                    .unwrap_or(record.added_at);
                let idle = now_ms.saturating_sub(seen) > idle_limit_ms;
                let head: Option<i64> = self
                    .conn
                    .query_row(
                        "SELECT cursor FROM feed_heads WHERE feed = ?1 AND device_id = ?2",
                        params![
                            feed.as_bytes().as_slice(),
                            record.device_id.as_bytes().as_slice()
                        ],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(storage)?;
                out.push((idle, head.map(to_u64)));
            }
        }
        Ok(out)
    }

    /// The pruning pass of the feeds (ADR-0045): deletes the payload of the document entries a
    /// snapshot covers once the snapshot is `grace_ms` old, nobody disputes it and, in a shared feed,
    /// every active device of every member that is not idle has a head at or past the cover; deletes
    /// the feeds closed `CLOSE_READ_DAYS` ago. Membership and keyring entries are never pruned.
    pub(crate) fn prune_feeds(
        &mut self,
        now_ms: i64,
        grace_ms: i64,
        idle_limit_ms: i64,
    ) -> Result<FeedPrune, HttpError> {
        let mut report = FeedPrune::default();
        // Closed feeds past the read window.
        let window = i64::from(CLOSE_READ_DAYS).saturating_mul(DAY_MS);
        let expired: Vec<(Vec<u8>, Vec<u8>, i64)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT feed_id, billing_account, bytes_used FROM feeds
                     WHERE closed_at IS NOT NULL AND closed_at + ?1 <= ?2",
                )
                .map_err(storage)?;
            let rows = stmt
                .query_map(params![window, now_ms], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(storage)?;
            rows.collect::<Result<_, _>>().map_err(storage)?
        };
        for (feed, billing, size) in expired {
            let tx = self.conn.transaction().map_err(storage)?;
            tx.execute("DELETE FROM feeds WHERE feed_id = ?1", [feed.as_slice()])
                .map_err(storage)?;
            Self::add_bytes(
                &tx,
                AccountId::from_bytes(hash16(&billing)?),
                size.saturating_neg(),
            )?;
            tx.commit().map_err(storage)?;
            self.rules.remove(&FeedId::from_bytes(hash16(&feed)?));
            report.feeds += 1;
        }
        // Snapshots old enough and not disputed.
        let cutoff = now_ms.saturating_sub(grace_ms);
        let candidates: Vec<(Vec<u8>, Vec<u8>, i64)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT s.feed, s.doc_id, s.prune_covers FROM feed_snapshots s
                     WHERE s.created_at <= ?1 AND NOT EXISTS (
                       SELECT 1 FROM feed_snapshot_disputes d
                       WHERE d.feed = s.feed AND d.doc_id = s.doc_id AND d.covers_cursor = s.covers_cursor)
                     ORDER BY s.feed",
                )
                .map_err(storage)?;
            let rows = stmt
                .query_map([cutoff], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(storage)?;
            rows.collect::<Result<_, _>>().map_err(storage)?
        };
        let mut ack: HashMap<Vec<u8>, Vec<(bool, Option<u64>)>> = HashMap::new();
        let mut seen_docs: HashSet<(Vec<u8>, Vec<u8>)> = HashSet::new();
        for (feed_bytes, doc, covers) in candidates {
            if !seen_docs.insert((feed_bytes.clone(), doc.clone())) {
                continue;
            }
            let feed = FeedId::from_bytes(hash16(&feed_bytes)?);
            let Some((kind, billing, _)) = self.feed_row(feed)? else {
                continue;
            };
            if kind == FeedKind::Shared {
                if !ack.contains_key(&feed_bytes) {
                    let devices = self.ack_devices(feed, now_ms, idle_limit_ms)?;
                    ack.insert(feed_bytes.clone(), devices);
                }
                let covered = ack.get(&feed_bytes).is_none_or(|devices| {
                    devices.iter().all(|(idle, head)| {
                        *idle || head.is_some_and(|head| head >= to_u64(covers))
                    })
                });
                if !covered {
                    continue;
                }
            }
            let tx = self.conn.transaction().map_err(storage)?;
            let (count, bytes): (i64, i64) = tx
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(length(bytes)), 0) FROM feed_entries
                     WHERE feed = ?1 AND doc_id = ?2 AND cursor <= ?3 AND pruned = 0 AND kind = ?4",
                    params![feed_bytes.as_slice(), doc.as_slice(), covers, KIND_ENTRY],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(storage)?;
            if count == 0 {
                continue;
            }
            tx.execute(
                "UPDATE feed_entries SET bytes = x'', pruned = 1
                 WHERE feed = ?1 AND doc_id = ?2 AND cursor <= ?3 AND pruned = 0 AND kind = ?4",
                params![feed_bytes.as_slice(), doc.as_slice(), covers, KIND_ENTRY],
            )
            .map_err(storage)?;
            Self::add_feed_bytes(&tx, feed, billing, bytes.saturating_neg())?;
            tx.commit().map_err(storage)?;
            report.entries = report.entries.saturating_add(to_u64(count));
            report.bytes = report.bytes.saturating_add(to_u64(bytes));
        }
        Ok(report)
    }
}

/// Whether the feed's `closed_at` is still unset (the first `Close` stamps the server's time).
fn current_open(tx: &Transaction<'_>, feed: FeedId) -> Result<bool, HttpError> {
    let closed: Option<i64> = tx
        .query_row(
            "SELECT closed_at FROM feeds WHERE feed_id = ?1",
            [feed.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    Ok(closed.is_none())
}

/// The log entries of an account that revoke a device (what makes a `Rekey` pending): kept for the
/// tests and diagnostics.
#[allow(dead_code)]
pub(crate) fn revocations(entries: &[LogEntry]) -> usize {
    entries
        .iter()
        .filter(|entry| matches!(entry.body, LogEntryBody::DeviceRevoked { .. }))
        .count()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{TestDevice, genesis_log};
    use axum::http::StatusCode;

    const QUOTA: u64 = 10_000;

    fn store_with_account(seed: u64) -> (Store, AccountId, TestDevice) {
        let mut store = Store::open_in_memory(0).unwrap();
        let (device, log) = genesis_log(seed);
        let account = store.create_account(log.entries().to_vec(), 0).unwrap();
        (store, account, device)
    }

    fn info(account: AccountId, device: &TestDevice, doc: DocId, seq: u64) -> FeedItemInfo {
        FeedItemInfo {
            kind: ItemKind::Entry,
            feed: FeedId::from_account(account),
            account_id: account,
            doc_id: doc,
            device_id: device.id,
            device_seq: seq,
        }
    }

    fn append(
        store: &mut Store,
        account: AccountId,
        device: &TestDevice,
        seq: u64,
        bytes: &[u8],
        quota: u64,
    ) -> Result<Appended, HttpError> {
        let doc = DocId::from_bytes([7; 16]);
        store.append_item(
            account,
            FeedId::from_account(account),
            &info(account, device, doc, seq),
            0,
            bytes,
            quota,
            0,
        )
    }

    #[test]
    fn a_personal_feed_assigns_cursors_chains_and_deduplicates() {
        let (mut store, account, device) = store_with_account(1);
        let a = append(&mut store, account, &device, 1, b"one", QUOTA).unwrap();
        let b = append(&mut store, account, &device, 2, b"two", QUOTA).unwrap();
        assert!(a.new && b.new && b.cursor > a.cursor);
        let epoch = store.server_epoch();
        let first = kb_sync::v2::chain_next(&kb_sync::v2::chain_genesis(&epoch), b"one");
        assert_eq!(a.chain_hash, first);
        assert_eq!(b.chain_hash, kb_sync::v2::chain_next(&first, b"two"));
        // Pushing an entry again answers the stored cursor and chain, whatever its bytes.
        let again = append(&mut store, account, &device, 1, b"resealed", QUOTA).unwrap();
        assert_eq!(
            (again.cursor, again.chain_hash, again.new),
            (a.cursor, a.chain_hash, false)
        );
        let feed = FeedId::from_account(account);
        let page = store
            .feed_changes(account, feed, 0, 200, 1 << 20, 0)
            .unwrap();
        assert_eq!(
            page.changes,
            vec![(a.cursor, b"one".to_vec()), (b.cursor, b"two".to_vec())]
        );
        assert_eq!(page.chain_hash_after, b.chain_hash);
        assert!(!page.more);
        // After a cursor: the hash is the running one at that point.
        let tail = store
            .feed_changes(account, feed, a.cursor, 200, 1 << 20, 0)
            .unwrap();
        assert_eq!(tail.changes.len(), 1);
        let empty = store
            .feed_changes(account, feed, b.cursor, 200, 1 << 20, 0)
            .unwrap();
        assert!(empty.changes.is_empty());
        assert_eq!(empty.chain_hash_after, b.chain_hash);
    }

    #[test]
    fn pages_stop_at_the_byte_budget_but_carry_at_least_one_item() {
        let (mut store, account, device) = store_with_account(2);
        for seq in 1..=3 {
            append(&mut store, account, &device, seq, &[0u8; 10], QUOTA).unwrap();
        }
        let feed = FeedId::from_account(account);
        let page = store.feed_changes(account, feed, 0, 200, 15, 0).unwrap();
        assert_eq!(page.changes.len(), 1);
        assert!(page.more);
        let page = store.feed_changes(account, feed, 0, 200, 5, 0).unwrap();
        assert_eq!(page.changes.len(), 1);
        let page = store.feed_changes(account, feed, 0, 200, 30, 0).unwrap();
        assert_eq!(page.changes.len(), 3);
        assert!(!page.more);
        let page = store.feed_changes(account, feed, 0, 2, 1 << 20, 0).unwrap();
        assert_eq!((page.changes.len(), page.more), (2, true));
    }

    #[test]
    fn quota_counts_every_write_and_refuses_overflow() {
        let (mut store, account, device) = store_with_account(3);
        let base = store.account_bytes(account).unwrap();
        let quota = base + 100;
        let feed = FeedId::from_account(account);
        append(&mut store, account, &device, 1, &[0u8; 60], quota).unwrap();
        assert_eq!(store.account_bytes(account).unwrap(), base + 60);
        let error = append(&mut store, account, &device, 2, &[0u8; 41], quota).unwrap_err();
        assert_eq!(error.status, StatusCode::INSUFFICIENT_STORAGE);
        store
            .put_feed_chunk(account, feed, &[1; 32], &[0u8; 40], quota, 0)
            .unwrap();
        // Storing the same chunk again costs nothing.
        store
            .put_feed_chunk(account, feed, &[1; 32], &[0u8; 40], quota, 0)
            .unwrap();
        assert_eq!(store.account_bytes(account).unwrap(), base + 100);
        let error = store
            .put_feed_head(account, device.id, feed, 1, &[0u8; 1], quota, 0)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::INSUFFICIENT_STORAGE);
    }

    #[test]
    fn deleting_a_chunk_frees_its_bytes_once_and_only_in_its_feed() {
        let (mut store, account, _device) = store_with_account(4);
        let (other, _other_device) = {
            let (device, log) = genesis_log(5);
            let account = store.create_account(log.entries().to_vec(), 0).unwrap();
            (account, device)
        };
        let feed = FeedId::from_account(account);
        let other_feed = FeedId::from_account(other);
        let base = store.account_bytes(account).unwrap();
        let other_base = store.account_bytes(other).unwrap();
        store
            .put_feed_chunk(account, feed, &[1; 32], &[0u8; 50], QUOTA, 0)
            .unwrap();
        store
            .put_feed_chunk(other, other_feed, &[1; 32], &[0u8; 70], QUOTA, 0)
            .unwrap();
        assert_eq!(store.account_bytes(account).unwrap(), base + 50);
        assert_eq!(store.account_bytes(other).unwrap(), other_base + 70);
        store.delete_feed_chunk(account, feed, &[1; 32], 0).unwrap();
        assert_eq!(store.account_bytes(account).unwrap(), base);
        assert_eq!(store.account_bytes(other).unwrap(), other_base + 70);
        // Again, or a hash never stored: nothing is freed.
        store.delete_feed_chunk(account, feed, &[1; 32], 0).unwrap();
        store.delete_feed_chunk(account, feed, &[2; 32], 0).unwrap();
        assert_eq!(store.account_bytes(account).unwrap(), base);
        assert!(
            store
                .feed_chunk(other, other_feed, &[1; 32], 0)
                .unwrap()
                .is_some()
        );
        // Another account's feed is not a feed of this one.
        let error = store
            .delete_feed_chunk(account, other_feed, &[1; 32], 0)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_subscription_is_the_accounts_own_personal_feed_and_nothing_it_may_not_read() {
        let (mut store, account, device) = store_with_account(6);
        let (other, other_device) = {
            let (device, log) = genesis_log(7);
            let account = store.create_account(log.entries().to_vec(), 0).unwrap();
            (account, device)
        };
        let mine = FeedId::from_account(account);
        let theirs = FeedId::from_account(other);
        let unknown = FeedId::from_bytes([9; 16]);
        // Another account's personal feed and a feed that does not exist look alike: left out.
        assert_eq!(
            store
                .subscribable(account, &[theirs, unknown, mine, mine], 0)
                .unwrap(),
            vec![(mine, true)]
        );
        assert_eq!(
            store.subscribable(other, &[mine, theirs], 0).unwrap(),
            vec![(theirs, true)]
        );
        assert_eq!(store.subscribable(account, &[], 0).unwrap(), vec![]);
        // The head cursors are per feed.
        assert_eq!(
            store.feed_head_cursors(&[mine, theirs, unknown]).unwrap(),
            vec![(mine, 0), (theirs, 0), (unknown, 0)]
        );
        let first = append(&mut store, account, &device, 1, b"mine", QUOTA).unwrap();
        let theirs_first = append(&mut store, other, &other_device, 1, b"theirs", QUOTA).unwrap();
        let second = append(&mut store, account, &device, 2, b"mine again", QUOTA).unwrap();
        assert_eq!(
            store.feed_head_cursors(&[mine, theirs]).unwrap(),
            vec![(mine, second.cursor), (theirs, theirs_first.cursor)]
        );
        // The previous cursor is the feed's own previous item, not the global previous one.
        assert_eq!(first.prev_cursor, 0);
        assert_eq!(theirs_first.prev_cursor, 0);
        assert_eq!(second.prev_cursor, first.cursor);
        assert!(second.cursor > theirs_first.cursor);
        // A duplicate is not a new item and has no position of its own.
        let again = append(&mut store, account, &device, 2, b"mine again", QUOTA).unwrap();
        assert!(!again.new);
        assert_eq!((again.cursor, again.prev_cursor), (second.cursor, 0));
    }
}
