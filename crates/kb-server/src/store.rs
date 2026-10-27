//! The server's storage: one plain SQLite file (`server.db`, WAL, foreign keys). Every
//! payload is ciphertext or public-key material; the server never holds a key.
//!
//! This file holds the tables every protocol version of the server shares (`meta`, the random
//! `server_epoch`; `accounts`, with `bytes_used` for the quota of the billing account;
//! `device_log`; `challenges`; `sessions`, BLAKE3 of the token, never the token; the pairing
//! mailboxes; the recovery blobs; `devices_seen`); the feeds of protocol v2 (`feeds`,
//! `feed_members`, `feed_entries`, ...) are in `store/v2.rs`. Protocol v1 is retired (ADR-0045:
//! v2 replaces it without a server migration), so a database that holds v1 data is refused at
//! open: the operator starts a fresh data directory.
//!
//! Pruning keeps a small tombstone row per pruned entry (payload emptied, `pruned = 1`):
//! deduplication on `(feed, kind, doc_id, device_id, device_seq)` and the chain hash at any
//! cursor survive the payload.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use kb_keys::{DeviceLog, LogEntry, LogError, LogState};
use kb_types::{AccountId, DeviceId};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::ServerError;
use crate::error::HttpError;

mod v2;

pub(crate) use v2::{LinkRead, SignupLimits};

/// Schema version stored in `PRAGMA user_version`. Version 3 (Milestone 13) is the schema of
/// protocol v2 and starts fresh: the earlier versions held protocol v1's tables (`entries`,
/// `snapshots`, `snapshot_marks`, `chunks`, `heads`), which nothing reads any more, and are refused
/// by [`Store::open`]. Every statement of [`SCHEMA`] is `IF NOT EXISTS`.
const SCHEMA_VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  server_epoch BLOB NOT NULL CHECK (length(server_epoch) = 16),
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS accounts (
  account_id BLOB PRIMARY KEY,
  created_at INTEGER NOT NULL,
  bytes_used INTEGER NOT NULL DEFAULT 0,
  created_via TEXT,
  share_invite_id BLOB
);
CREATE TABLE IF NOT EXISTS device_log (
  account_id BLOB NOT NULL REFERENCES accounts(account_id),
  seq INTEGER NOT NULL,
  entry BLOB NOT NULL,
  hash BLOB NOT NULL,
  PRIMARY KEY (account_id, seq)
);
CREATE TABLE IF NOT EXISTS challenges (
  nonce BLOB PRIMARY KEY,
  account_id BLOB NOT NULL REFERENCES accounts(account_id),
  device_id BLOB NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash BLOB PRIMARY KEY,
  account_id BLOB NOT NULL REFERENCES accounts(account_id),
  device_id BLOB NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mailboxes (
  id BLOB PRIMARY KEY,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  closed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS mailboxes_expiry ON mailboxes (expires_at);
CREATE TABLE IF NOT EXISTS mailbox_messages (
  mailbox_id BLOB NOT NULL REFERENCES mailboxes(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  role INTEGER NOT NULL,
  bytes BLOB NOT NULL,
  PRIMARY KEY (mailbox_id, seq)
);
CREATE TABLE IF NOT EXISTS recovery_blobs (
  id BLOB PRIMARY KEY,
  account_id BLOB NOT NULL REFERENCES accounts(account_id),
  bytes BLOB NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS devices_seen (
  account_id BLOB NOT NULL REFERENCES accounts(account_id),
  device_id BLOB NOT NULL,
  last_seen INTEGER NOT NULL,
  PRIMARY KEY (account_id, device_id)
);
";

/// Who posted a pairing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Joiner,
    Sponsor,
}

impl Role {
    fn to_sql(self) -> i64 {
        match self {
            Self::Joiner => 0,
            Self::Sponsor => 1,
        }
    }

    fn from_sql(value: i64) -> Result<Self, HttpError> {
        match value {
            0 => Ok(Self::Joiner),
            1 => Ok(Self::Sponsor),
            _ => Err(HttpError::internal("a stored pairing role is unknown")),
        }
    }

    /// The JSON name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Joiner => "joiner",
            Self::Sponsor => "sponsor",
        }
    }
}

/// A pairing mailbox id.
pub(crate) type MailboxId = [u8; kb_types::devices::MAILBOX_ID_LEN];

/// One stored pairing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailboxMessage {
    pub(crate) seq: u32,
    pub(crate) role: Role,
    pub(crate) bytes: Vec<u8>,
}

/// A mailbox as a reader sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MailboxView {
    pub(crate) expires_at: i64,
    pub(crate) closed: bool,
    /// The messages from the reader's `after` on, in sequence order.
    pub(crate) messages: Vec<MailboxMessage>,
}

/// A live session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionRow {
    pub(crate) account_id: AccountId,
    pub(crate) device_id: DeviceId,
    pub(crate) expires_at: i64,
}

/// A pending challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChallengeRow {
    pub(crate) account_id: AccountId,
    pub(crate) device_id: DeviceId,
    pub(crate) expires_at: i64,
}

/// What one pruning pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PruneReport {
    /// Feed entries whose payload was deleted.
    pub entries: u64,
    /// Payload bytes released.
    pub bytes: u64,
    /// Expired challenges, sessions and view links deleted.
    pub expired: u64,
    /// Expired pairing mailboxes deleted (with their messages).
    pub mailboxes: u64,
    /// Closed shared feeds deleted after their read window (`CLOSE_READ_DAYS`).
    pub feeds: u64,
}

/// The database plus the verified device logs (cached per account; a log is verified
/// once per process and again on every append).
pub(crate) struct Store {
    conn: Connection,
    server_epoch: [u8; 16],
    logs: HashMap<AccountId, Arc<LogState>>,
    /// The verified device logs the membership fold reads (protocol v2), dropped when a log grows.
    device_logs: HashMap<AccountId, Arc<DeviceLog>>,
    /// The membership rules of each shared feed (protocol v2), rebuilt from the stored log on first
    /// use after a start.
    rules: HashMap<kb_types::FeedId, kb_sync::v2::guard::FeedRules>,
}

/// A log that does not verify: a broken chain is `Conflict` (409) when appending, a
/// bad signature is `authentication` (400), anything else `invalid_log` (400).
fn log_error(error: &LogError, appending: bool) -> HttpError {
    match error {
        LogError::Chain { .. } if appending => {
            HttpError::conflict("the entries do not continue the stored device log")
        }
        LogError::Signature { .. } => HttpError::bad_signature(),
        other => HttpError::bad_request("invalid_log", &other.to_string()),
    }
}

fn storage(error: rusqlite::Error) -> HttpError {
    HttpError::storage(&error)
}

fn to_i64(value: u64, what: &'static str) -> Result<i64, HttpError> {
    i64::try_from(value).map_err(|_| HttpError::bad_request("encoding", what))
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn len_i64(bytes: &[u8]) -> i64 {
    i64::try_from(bytes.len()).unwrap_or(i64::MAX)
}

fn array<const N: usize>(blob: &[u8]) -> Result<[u8; N], HttpError> {
    blob.try_into()
        .map_err(|_| HttpError::internal("a stored value has the wrong length"))
}

impl Store {
    /// Opens (creating when needed) `server.db` in `data_dir`, applies the schema and
    /// creates the server epoch on first start.
    pub(crate) fn open(data_dir: &Path, now_ms: i64) -> Result<Self, ServerError> {
        std::fs::create_dir_all(data_dir).map_err(|e| ServerError::Io(e.to_string()))?;
        let conn = Connection::open(data_dir.join("server.db"))
            .map_err(|e| ServerError::Storage(e.to_string()))?;
        Self::init(conn, now_ms)
    }

    /// An in-memory store (unit tests).
    #[cfg(test)]
    pub(crate) fn open_in_memory(now_ms: i64) -> Result<Self, ServerError> {
        let conn = Connection::open_in_memory().map_err(|e| ServerError::Storage(e.to_string()))?;
        Self::init(conn, now_ms)
    }

    fn init(mut conn: Connection, now_ms: i64) -> Result<Self, ServerError> {
        let fail = |e: rusqlite::Error| ServerError::Storage(e.to_string());
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(fail)?;
        // `journal_mode` answers the resulting mode; an in-memory database stays
        // `memory`, which is fine for tests.
        let _mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(fail)?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")
            .map_err(fail)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(fail)?;
        if version > SCHEMA_VERSION {
            return Err(ServerError::Storage(format!(
                "server.db has schema version {version}, newer than this server ({SCHEMA_VERSION})"
            )));
        }
        // Protocol v1's tables only exist in a file of protocol v1's days: nothing of it is
        // readable, and clients would find a server epoch that does not match what they hold.
        let v1_data: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'entries')",
                [],
                |row| row.get(0),
            )
            .map_err(fail)?;
        if v1_data || (1..SCHEMA_VERSION).contains(&version) {
            return Err(ServerError::Storage(format!(
                "server.db holds protocol v1 data (schema version {version}); protocol v2 starts \
                 a fresh server (ADR-0045): move the data directory away and start again"
            )));
        }
        let tx = conn.transaction().map_err(fail)?;
        tx.execute_batch(SCHEMA).map_err(fail)?;
        v2::migrate(&tx).map_err(fail)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(fail)?;
        let existing: Option<Vec<u8>> = tx
            .query_row("SELECT server_epoch FROM meta WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(fail)?;
        let server_epoch = match existing {
            Some(blob) => <[u8; 16]>::try_from(blob.as_slice())
                .map_err(|_| ServerError::Storage("the stored server epoch is malformed".into()))?,
            None => {
                let mut epoch = [0u8; 16];
                getrandom::fill(&mut epoch).map_err(|_| ServerError::Random)?;
                tx.execute(
                    "INSERT INTO meta (id, server_epoch, created_at) VALUES (1, ?1, ?2)",
                    params![epoch.as_slice(), now_ms],
                )
                .map_err(fail)?;
                epoch
            }
        };
        tx.commit().map_err(fail)?;
        Ok(Self {
            conn,
            server_epoch,
            logs: HashMap::new(),
            device_logs: HashMap::new(),
            rules: HashMap::new(),
        })
    }

    /// The random id of this storage (ADR-0021 §3).
    pub(crate) fn server_epoch(&self) -> [u8; 16] {
        self.server_epoch
    }

    // ----- accounts and the device log ----------------------------------------------------

    /// Creates an account from its verified log; `Conflict` when it exists.
    pub(crate) fn create_account(
        &mut self,
        entries: Vec<LogEntry>,
        now_ms: i64,
    ) -> Result<AccountId, HttpError> {
        self.create_account_via(entries, now_ms, None)
    }

    /// [`Self::create_account`] for an account a share invite created (`created_via`,
    /// `share_invite_id`); a device id of the log that another account uses is `Conflict`.
    pub(crate) fn create_account_via(
        &mut self,
        entries: Vec<LogEntry>,
        now_ms: i64,
        via: Option<kb_types::InviteId>,
    ) -> Result<AccountId, HttpError> {
        let encoded: Vec<Vec<u8>> = entries.iter().map(LogEntry::encode).collect();
        let log = DeviceLog::from_entries(entries).map_err(|e| log_error(&e, false))?;
        let state = log.verify().map_err(|e| log_error(&e, false))?;
        let account = state.account_id;
        let tx = self.conn.transaction().map_err(storage)?;
        let exists: Option<i64> = tx
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
        // A shared feed or a personal feed under this id would shadow the account's own.
        let taken: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM feeds WHERE feed_id = ?1 AND kind = 2",
                [account.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if taken.is_some() {
            return Err(HttpError::conflict("the account id is taken by a feed"));
        }
        let bytes: i64 = encoded.iter().map(|e| len_i64(e)).sum();
        tx.execute(
            "INSERT INTO accounts (account_id, created_at, bytes_used, created_via, share_invite_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account.as_bytes().as_slice(),
                now_ms,
                bytes,
                via.map(|_| "share"),
                via.map(|id| id.as_bytes().to_vec())
            ],
        )
        .map_err(storage)?;
        for (entry, bytes) in log.entries().iter().zip(&encoded) {
            insert_log_entry(&tx, account, entry, bytes)?;
        }
        v2::register_devices(&tx, account, &state)?;
        tx.commit().map_err(storage)?;
        self.logs.insert(account, Arc::new(state));
        self.device_logs.remove(&account);
        Ok(account)
    }

    /// Appends entries that continue the stored log. Entries already stored (same
    /// sequence, same bytes) are accepted again and skipped; anything that does not
    /// continue the chain is `Conflict`. Answers the devices the entries made inactive (a revocation):
    /// their sessions and open challenges are deleted in the same transaction, so they cannot come
    /// back, and the caller closes their sockets.
    pub(crate) fn append_log(
        &mut self,
        account: AccountId,
        entries: Vec<LogEntry>,
    ) -> Result<Vec<DeviceId>, HttpError> {
        let stored = self.load_log_entries(account)?;
        if stored.is_empty() {
            return Err(HttpError::not_found("unknown account"));
        }
        let before = self.log_state(account)?;
        let stored_len = u64::try_from(stored.len()).unwrap_or(u64::MAX);
        let mut log = DeviceLog::from_entries(stored)
            .map_err(|_| HttpError::internal("the stored device log does not verify"))?;
        let mut fresh = Vec::new();
        for entry in entries {
            if entry.seq < stored_len {
                let same = usize::try_from(entry.seq)
                    .ok()
                    .and_then(|i| log.entries().get(i))
                    .is_some_and(|existing| existing == &entry);
                if same {
                    continue;
                }
                return Err(HttpError::conflict(
                    "the entries do not continue the stored device log",
                ));
            }
            log.append(entry.clone()).map_err(|e| log_error(&e, true))?;
            fresh.push(entry);
        }
        if fresh.is_empty() {
            return Ok(Vec::new());
        }
        let state = log.verify().map_err(|e| log_error(&e, true))?;
        let tx = self.conn.transaction().map_err(storage)?;
        let mut added = 0i64;
        for entry in &fresh {
            let bytes = entry.encode();
            added = added.saturating_add(len_i64(&bytes));
            insert_log_entry(&tx, account, entry, &bytes)?;
        }
        tx.execute(
            "UPDATE accounts SET bytes_used = bytes_used + ?2 WHERE account_id = ?1",
            params![account.as_bytes().as_slice(), added],
        )
        .map_err(storage)?;
        v2::register_devices(&tx, account, &state)?;
        // Active before, not any more: what the new entries revoked.
        let revoked: Vec<DeviceId> = before
            .iter()
            .flat_map(|before| before.devices.iter())
            .map(|record| record.device_id)
            .filter(|device| {
                before.as_ref().is_some_and(|b| b.active(*device).is_some())
                    && state.active(*device).is_none()
            })
            .collect();
        for device in &revoked {
            for table in ["sessions", "challenges"] {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE account_id = ?1 AND device_id = ?2"),
                    params![account.as_bytes().as_slice(), device.as_bytes().as_slice()],
                )
                .map_err(storage)?;
            }
        }
        tx.commit().map_err(storage)?;
        self.logs.insert(account, Arc::new(state));
        self.device_logs.remove(&account);
        Ok(revoked)
    }

    /// The encoded log entries in order (empty for an unknown account).
    pub(crate) fn fetch_log(&self, account: AccountId) -> Result<Vec<Vec<u8>>, HttpError> {
        let mut stmt = self
            .conn
            .prepare("SELECT entry FROM device_log WHERE account_id = ?1 ORDER BY seq")
            .map_err(storage)?;
        let rows = stmt
            .query_map([account.as_bytes().as_slice()], |row| row.get(0))
            .map_err(storage)?;
        rows.collect::<Result<Vec<Vec<u8>>, _>>().map_err(storage)
    }

    fn load_log_entries(&self, account: AccountId) -> Result<Vec<LogEntry>, HttpError> {
        self.fetch_log(account)?
            .iter()
            .map(|bytes| {
                LogEntry::decode(bytes)
                    .map_err(|_| HttpError::internal("a stored device log entry is malformed"))
            })
            .collect()
    }

    /// The verified state of an account's log; `None` for an unknown account.
    pub(crate) fn log_state(
        &mut self,
        account: AccountId,
    ) -> Result<Option<Arc<LogState>>, HttpError> {
        if let Some(state) = self.logs.get(&account) {
            return Ok(Some(Arc::clone(state)));
        }
        let entries = self.load_log_entries(account)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let state = DeviceLog::from_entries(entries)
            .and_then(|log| log.verify())
            .map_err(|_| HttpError::internal("the stored device log does not verify"))?;
        let state = Arc::new(state);
        self.logs.insert(account, Arc::clone(&state));
        Ok(Some(state))
    }

    // ----- challenges and sessions --------------------------------------------------------

    pub(crate) fn put_challenge(
        &mut self,
        nonce: &[u8; 32],
        row: ChallengeRow,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        self.conn
            .execute("DELETE FROM challenges WHERE expires_at <= ?1", [now_ms])
            .map_err(storage)?;
        self.conn
            .execute(
                "INSERT INTO challenges (nonce, account_id, device_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    nonce.as_slice(),
                    row.account_id.as_bytes().as_slice(),
                    row.device_id.as_bytes().as_slice(),
                    row.expires_at
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// Removes and returns a challenge (single use, expired or not).
    pub(crate) fn take_challenge(
        &mut self,
        nonce: &[u8; 32],
    ) -> Result<Option<ChallengeRow>, HttpError> {
        let row: Option<(Vec<u8>, Vec<u8>, i64)> = self
            .conn
            .query_row(
                "DELETE FROM challenges WHERE nonce = ?1
                 RETURNING account_id, device_id, expires_at",
                [nonce.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        row.map(|(account, device, expires_at)| {
            Ok(ChallengeRow {
                account_id: AccountId::from_bytes(array(&account)?),
                device_id: DeviceId::from_bytes(array(&device)?),
                expires_at,
            })
        })
        .transpose()
    }

    pub(crate) fn put_session(
        &mut self,
        token_hash: &[u8; 32],
        row: SessionRow,
    ) -> Result<(), HttpError> {
        self.conn
            .execute(
                "INSERT INTO sessions (token_hash, account_id, device_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    token_hash.as_slice(),
                    row.account_id.as_bytes().as_slice(),
                    row.device_id.as_bytes().as_slice(),
                    row.expires_at
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// A session that has not expired.
    pub(crate) fn session(
        &self,
        token_hash: &[u8; 32],
        now_ms: i64,
    ) -> Result<Option<SessionRow>, HttpError> {
        let row: Option<(Vec<u8>, Vec<u8>, i64)> = self
            .conn
            .query_row(
                "SELECT account_id, device_id, expires_at FROM sessions
                 WHERE token_hash = ?1 AND expires_at > ?2",
                params![token_hash.as_slice(), now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        row.map(|(account, device, expires_at)| {
            Ok(SessionRow {
                account_id: AccountId::from_bytes(array(&account)?),
                device_id: DeviceId::from_bytes(array(&device)?),
                expires_at,
            })
        })
        .transpose()
    }

    // ----- the quota ----------------------------------------------------------------------

    fn bytes_used(tx: &Transaction<'_>, account: AccountId) -> Result<i64, HttpError> {
        tx.query_row(
            "SELECT bytes_used FROM accounts WHERE account_id = ?1",
            [account.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| HttpError::not_found("unknown account"))
    }

    fn check_quota(used: i64, delta: i64, quota: u64) -> Result<(), HttpError> {
        let after = used.saturating_add(delta);
        if delta > 0 && to_u64(after) > quota {
            return Err(HttpError::quota());
        }
        Ok(())
    }

    fn add_bytes(tx: &Transaction<'_>, account: AccountId, delta: i64) -> Result<(), HttpError> {
        tx.execute(
            "UPDATE accounts SET bytes_used = MAX(0, bytes_used + ?2) WHERE account_id = ?1",
            params![account.as_bytes().as_slice(), delta],
        )
        .map_err(storage)?;
        Ok(())
    }

    /// The account's stored bytes (tests and diagnostics).
    pub(crate) fn account_bytes(&self, account: AccountId) -> Result<u64, HttpError> {
        let used: Option<i64> = self
            .conn
            .query_row(
                "SELECT bytes_used FROM accounts WHERE account_id = ?1",
                [account.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        Ok(used.map_or(0, to_u64))
    }

    // ----- expiry -------------------------------------------------------------------------

    /// Drops expired challenges, sessions, view links and pairing mailboxes. (The entries of the feeds are
    /// pruned by `prune_feeds`, which waits for the acknowledgement of every member.)
    pub(crate) fn prune(&mut self, now_ms: i64) -> Result<PruneReport, HttpError> {
        let tx = self.conn.transaction().map_err(storage)?;
        let challenges = tx
            .execute("DELETE FROM challenges WHERE expires_at <= ?1", [now_ms])
            .map_err(storage)?;
        let sessions = tx
            .execute("DELETE FROM sessions WHERE expires_at <= ?1", [now_ms])
            .map_err(storage)?;
        let mailboxes = purge_mailboxes(&tx, now_ms)?;
        // View links past their expiry (ADR-0051); the server also refuses to serve them from the moment.
        let links = v2::purge_expired_links(&tx, now_ms)?;
        tx.commit().map_err(storage)?;
        Ok(PruneReport {
            expired: u64::try_from(challenges.saturating_add(sessions).saturating_add(links))
                .unwrap_or(0),
            mailboxes: u64::try_from(mailboxes).unwrap_or(0),
            ..PruneReport::default()
        })
    }
}

/// Deletes expired mailboxes; their messages go with them (`ON DELETE CASCADE`).
fn purge_mailboxes(tx: &Transaction<'_>, now_ms: i64) -> Result<usize, HttpError> {
    tx.execute("DELETE FROM mailboxes WHERE expires_at <= ?1", [now_ms])
        .map_err(storage)
}

impl Store {
    // ----- pairing mailboxes --------------------------------------------------------------

    /// Creates a mailbox holding the joiner's offer as message 0; answers its random id
    /// and expiry. Expired mailboxes are purged on the way.
    pub(crate) fn create_mailbox(
        &mut self,
        offer: &[u8],
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<(MailboxId, i64), HttpError> {
        let expires_at = now_ms.saturating_add(ttl_ms);
        let tx = self.conn.transaction().map_err(storage)?;
        purge_mailboxes(&tx, now_ms)?;
        let mut id: MailboxId = [0; kb_types::devices::MAILBOX_ID_LEN];
        let mut created = false;
        for _ in 0..8 {
            getrandom::fill(&mut id).map_err(|_| HttpError::internal("no randomness available"))?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO mailboxes (id, created_at, expires_at)
                     VALUES (?1, ?2, ?3)",
                    params![id.as_slice(), now_ms, expires_at],
                )
                .map_err(storage)?;
            if inserted == 1 {
                created = true;
                break;
            }
        }
        if !created {
            return Err(HttpError::internal("no free mailbox id"));
        }
        tx.execute(
            "INSERT INTO mailbox_messages (mailbox_id, seq, role, bytes) VALUES (?1, 0, ?2, ?3)",
            params![id.as_slice(), Role::Joiner.to_sql(), offer],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok((id, expires_at))
    }

    /// `(expires_at, closed, message count)` of a live mailbox; `None` when unknown or
    /// expired.
    fn mailbox_state(
        conn: &Connection,
        id: &MailboxId,
        now_ms: i64,
    ) -> Result<Option<(i64, bool, u32)>, HttpError> {
        let state: Option<(i64, i64, i64)> = conn
            .query_row(
                "SELECT expires_at, closed,
                        (SELECT COUNT(*) FROM mailbox_messages WHERE mailbox_id = mailboxes.id)
                 FROM mailboxes WHERE id = ?1 AND expires_at > ?2",
                params![id.as_slice(), now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        Ok(state.map(|(expires_at, closed, count)| {
            (
                expires_at,
                closed != 0,
                u32::try_from(count).unwrap_or(u32::MAX),
            )
        }))
    }

    /// Whether a mailbox exists and has not expired (closed or not).
    pub(crate) fn mailbox_live(&self, id: &MailboxId, now_ms: i64) -> Result<bool, HttpError> {
        Ok(Self::mailbox_state(&self.conn, id, now_ms)?.is_some())
    }

    /// The messages with `seq >= after` (`after` counts the messages the reader already
    /// has); `None` for an unknown or expired mailbox.
    pub(crate) fn read_mailbox(
        &self,
        id: &MailboxId,
        after: u32,
        now_ms: i64,
    ) -> Result<Option<MailboxView>, HttpError> {
        let Some((expires_at, closed, _)) = Self::mailbox_state(&self.conn, id, now_ms)? else {
            return Ok(None);
        };
        let from = i64::from(after);
        let mut stmt = self
            .conn
            .prepare(
                "SELECT seq, role, bytes FROM mailbox_messages
                 WHERE mailbox_id = ?1 AND seq >= ?2 ORDER BY seq",
            )
            .map_err(storage)?;
        let rows = stmt
            .query_map(params![id.as_slice(), from], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(storage)?;
        let mut messages = Vec::new();
        for row in rows {
            let (seq, role, bytes) = row.map_err(storage)?;
            messages.push(MailboxMessage {
                seq: u32::try_from(seq)
                    .map_err(|_| HttpError::internal("a stored sequence is out of range"))?,
                role: Role::from_sql(role)?,
                bytes,
            });
        }
        Ok(Some(MailboxView {
            expires_at,
            closed,
            messages,
        }))
    }

    /// Appends a message; answers its sequence. Unknown or expired: 404; closed or full
    /// (`PAIRING_MAX_MESSAGES`): 409.
    pub(crate) fn post_mailbox(
        &mut self,
        id: &MailboxId,
        role: Role,
        bytes: &[u8],
        now_ms: i64,
    ) -> Result<u32, HttpError> {
        let tx = self.conn.transaction().map_err(storage)?;
        let (_, closed, count) = Self::mailbox_state(&tx, id, now_ms)?
            .ok_or_else(|| HttpError::not_found("unknown or expired mailbox"))?;
        if closed {
            return Err(HttpError::conflict("the mailbox is closed"));
        }
        if count >= kb_types::devices::PAIRING_MAX_MESSAGES {
            return Err(HttpError::conflict("the mailbox is full"));
        }
        tx.execute(
            "INSERT INTO mailbox_messages (mailbox_id, seq, role, bytes) VALUES (?1, ?2, ?3, ?4)",
            params![id.as_slice(), i64::from(count), role.to_sql(), bytes],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(count)
    }

    /// Closes a mailbox (it stays readable, as closed, until it expires). Closing it
    /// again is fine; unknown or expired: 404.
    pub(crate) fn close_mailbox(&mut self, id: &MailboxId, now_ms: i64) -> Result<(), HttpError> {
        let changed = self
            .conn
            .execute(
                "UPDATE mailboxes SET closed = 1 WHERE id = ?1 AND expires_at > ?2",
                params![id.as_slice(), now_ms],
            )
            .map_err(storage)?;
        if changed == 0 {
            return Err(HttpError::not_found("unknown or expired mailbox"));
        }
        Ok(())
    }

    // ----- recovery blobs -----------------------------------------------------------------

    /// The owner (and size) of a stored recovery blob.
    fn recovery_owner(
        conn: &Connection,
        id: &[u8; 32],
    ) -> Result<Option<(Vec<u8>, i64)>, HttpError> {
        conn.query_row(
            "SELECT account_id, length(bytes) FROM recovery_blobs WHERE id = ?1",
            [id.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)
    }

    /// Stores (or replaces) the account's recovery blob under `id`. An id held by another
    /// account: 403. Recovery blobs (at most 4 KiB each) are outside the account quota.
    pub(crate) fn put_recovery(
        &mut self,
        id: &[u8; 32],
        account: AccountId,
        bytes: &[u8],
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let tx = self.conn.transaction().map_err(storage)?;
        if let Some((owner, _)) = Self::recovery_owner(&tx, id)?
            && owner.as_slice() != account.as_bytes().as_slice()
        {
            return Err(HttpError::forbidden(
                "the recovery id belongs to another account",
            ));
        }
        tx.execute(
            "INSERT INTO recovery_blobs (id, account_id, bytes, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET bytes = excluded.bytes,
                                            updated_at = excluded.updated_at",
            params![id.as_slice(), account.as_bytes().as_slice(), bytes, now_ms],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    /// The blob stored under `id`, whoever owns it.
    pub(crate) fn recovery(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>, HttpError> {
        self.conn
            .query_row(
                "SELECT bytes FROM recovery_blobs WHERE id = ?1",
                [id.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    /// Deletes the account's blob; an id that holds nothing is fine (deleted already),
    /// another account's blob is 403.
    pub(crate) fn delete_recovery(
        &mut self,
        id: &[u8; 32],
        account: AccountId,
    ) -> Result<(), HttpError> {
        let tx = self.conn.transaction().map_err(storage)?;
        match Self::recovery_owner(&tx, id)? {
            None => return Ok(()),
            Some((owner, _)) if owner.as_slice() != account.as_bytes().as_slice() => {
                return Err(HttpError::forbidden(
                    "the recovery id belongs to another account",
                ));
            }
            Some(_) => {}
        }
        tx.execute("DELETE FROM recovery_blobs WHERE id = ?1", [id.as_slice()])
            .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(())
    }

    // ----- idle devices -------------------------------------------------------------------

    /// Records that the device opened a session now.
    pub(crate) fn device_seen(
        &mut self,
        account: AccountId,
        device: DeviceId,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        self.conn
            .execute(
                "INSERT INTO devices_seen (account_id, device_id, last_seen) VALUES (?1, ?2, ?3)
                 ON CONFLICT (account_id, device_id) DO UPDATE SET last_seen = excluded.last_seen",
                params![
                    account.as_bytes().as_slice(),
                    device.as_bytes().as_slice(),
                    now_ms
                ],
            )
            .map_err(storage)?;
        Ok(())
    }

    /// When the device last opened a session (`None`: never).
    pub(crate) fn last_seen(
        &self,
        account: AccountId,
        device: DeviceId,
    ) -> Result<Option<i64>, HttpError> {
        self.conn
            .query_row(
                "SELECT last_seen FROM devices_seen WHERE account_id = ?1 AND device_id = ?2",
                params![account.as_bytes().as_slice(), device.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }
}

fn insert_log_entry(
    tx: &Transaction<'_>,
    account: AccountId,
    entry: &LogEntry,
    bytes: &[u8],
) -> Result<(), HttpError> {
    let seq = to_i64(entry.seq, "the log sequence is out of range")?;
    tx.execute(
        "INSERT INTO device_log (account_id, seq, entry, hash) VALUES (?1, ?2, ?3, ?4)",
        params![
            account.as_bytes().as_slice(),
            seq,
            bytes,
            kb_crypto::hash(bytes).as_slice()
        ],
    )
    .map_err(storage)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{TestDevice, genesis_log};
    use axum::http::StatusCode;

    fn store_with_account() -> (Store, AccountId, TestDevice) {
        let mut store = Store::open_in_memory(0).unwrap();
        let (device, log) = genesis_log(1);
        let account = store.create_account(log.entries().to_vec(), 0).unwrap();
        (store, account, device)
    }

    #[test]
    fn epoch_is_created_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = Store::open(dir.path(), 1).unwrap().server_epoch();
        let second = Store::open(dir.path(), 2).unwrap().server_epoch();
        assert_eq!(first, second);
        let other = tempfile::tempdir().unwrap();
        assert_ne!(Store::open(other.path(), 1).unwrap().server_epoch(), first);
    }

    #[test]
    fn creating_an_account_twice_conflicts() {
        let (mut store, account, device) = store_with_account();
        assert!(store.log_state(account).unwrap().is_some());
        assert!(store.account_bytes(account).unwrap() > 0);
        let (_, log) = genesis_log(1);
        let error = store.create_account(log.entries().to_vec(), 0).unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        let state = store.log_state(account).unwrap().unwrap();
        assert!(state.active(device.id).is_some());
    }

    #[test]
    fn mailboxes_keep_order_close_and_expire() {
        let mut store = Store::open_in_memory(0).unwrap();
        let (id, expires_at) = store.create_mailbox(b"offer", 1_000, 600_000).unwrap();
        assert_eq!(expires_at, 601_000);
        assert_eq!(
            store
                .post_mailbox(&id, Role::Sponsor, b"answer", 2_000)
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .post_mailbox(&id, Role::Joiner, b"more", 2_000)
                .unwrap(),
            2
        );
        let view = store.read_mailbox(&id, 1, 2_000).unwrap().unwrap();
        assert_eq!(
            view.messages,
            vec![
                MailboxMessage {
                    seq: 1,
                    role: Role::Sponsor,
                    bytes: b"answer".to_vec()
                },
                MailboxMessage {
                    seq: 2,
                    role: Role::Joiner,
                    bytes: b"more".to_vec()
                },
            ]
        );
        let all = store.read_mailbox(&id, 0, 2_000).unwrap().unwrap();
        assert_eq!(all.messages[0].role, Role::Joiner);
        assert_eq!(
            (all.messages.len(), all.closed, all.expires_at),
            (3, false, 601_000)
        );

        store.close_mailbox(&id, 3_000).unwrap();
        store.close_mailbox(&id, 3_000).unwrap();
        assert!(store.read_mailbox(&id, 0, 3_000).unwrap().unwrap().closed);
        let error = store
            .post_mailbox(&id, Role::Sponsor, b"late", 3_000)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);

        assert!(store.read_mailbox(&id, 0, 601_000).unwrap().is_none());
        let error = store.close_mailbox(&id, 601_000).unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(store.prune(601_000).unwrap().mailboxes, 1);
    }

    #[test]
    fn a_mailbox_holds_at_most_its_message_limit() {
        let mut store = Store::open_in_memory(0).unwrap();
        let (id, _) = store.create_mailbox(b"offer", 0, 600_000).unwrap();
        for seq in 1..kb_types::devices::PAIRING_MAX_MESSAGES {
            assert_eq!(
                store.post_mailbox(&id, Role::Sponsor, b"m", 1).unwrap(),
                seq
            );
        }
        let error = store.post_mailbox(&id, Role::Sponsor, b"m", 1).unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        let error = store
            .post_mailbox(&[9; 6], Role::Sponsor, b"m", 1)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn recovery_blobs_are_owned_replaced_and_outside_the_quota() {
        let (mut store, account, _) = store_with_account();
        let mut other_store_account = || {
            let (_, log) = genesis_log(2);
            store.create_account(log.entries().to_vec(), 0).unwrap()
        };
        let other = other_store_account();
        let base = store.account_bytes(account).unwrap();
        store
            .put_recovery(&[1; 32], account, &[0u8; 100], 0)
            .unwrap();
        store
            .put_recovery(&[1; 32], account, &[1u8; 300], 0)
            .unwrap();
        assert_eq!(store.account_bytes(account).unwrap(), base);
        assert_eq!(store.recovery(&[1; 32]).unwrap(), Some(vec![1u8; 300]));
        let error = store.put_recovery(&[1; 32], other, b"x", 0).unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        let error = store.delete_recovery(&[1; 32], other).unwrap_err();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        store.delete_recovery(&[1; 32], account).unwrap();
        store.delete_recovery(&[1; 32], account).unwrap();
        assert_eq!(store.recovery(&[1; 32]).unwrap(), None);
        assert_eq!(store.account_bytes(account).unwrap(), base);
    }

    #[test]
    fn devices_seen_records_the_last_session() {
        let (mut store, account, device) = store_with_account();
        assert_eq!(store.last_seen(account, device.id).unwrap(), None);
        store.device_seen(account, device.id, 5).unwrap();
        store.device_seen(account, device.id, 9).unwrap();
        assert_eq!(store.last_seen(account, device.id).unwrap(), Some(9));
    }

    #[test]
    fn a_database_of_protocol_v1_days_is_refused_and_a_fresh_one_is_fine() {
        // A file of version 2 (v1's tables, mailboxes and recovery blobs).
        let dir = tempfile::tempdir().unwrap();
        Store::open(dir.path(), 1).unwrap();
        {
            let conn = Connection::open(dir.path().join("server.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE entries (cursor INTEGER PRIMARY KEY); PRAGMA user_version = 2;",
            )
            .unwrap();
        }
        let Err(error) = Store::open(dir.path(), 2) else {
            panic!("a protocol v1 database must be refused");
        };
        let text = error.to_string();
        assert!(text.contains("protocol v1"), "{text}");
        assert!(text.contains("fresh"), "{text}");

        // A version 3 file that still has the v1 tables (the beside-v1 builds made those) too.
        let dir = tempfile::tempdir().unwrap();
        Store::open(dir.path(), 1).unwrap();
        {
            let conn = Connection::open(dir.path().join("server.db")).unwrap();
            conn.execute_batch("CREATE TABLE entries (cursor INTEGER PRIMARY KEY);")
                .unwrap();
        }
        assert!(Store::open(dir.path(), 2).is_err());

        // A fresh store creates the schema once and opens again.
        let dir = tempfile::tempdir().unwrap();
        let epoch = Store::open(dir.path(), 1).unwrap().server_epoch();
        let again = Store::open(dir.path(), 2).unwrap();
        assert_eq!(again.server_epoch(), epoch);
        let version: i64 = again
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let tables: i64 = again
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
                 ('entries', 'snapshots', 'snapshot_marks', 'chunks', 'heads')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "no table of protocol v1");
    }

    #[test]
    fn challenges_are_single_use_and_sessions_expire() {
        let (mut store, account, device) = store_with_account();
        let nonce = [9u8; 32];
        let row = ChallengeRow {
            account_id: account,
            device_id: device.id,
            expires_at: 100,
        };
        store.put_challenge(&nonce, row, 0).unwrap();
        assert_eq!(store.take_challenge(&nonce).unwrap(), Some(row));
        assert_eq!(store.take_challenge(&nonce).unwrap(), None);

        let session = SessionRow {
            account_id: account,
            device_id: device.id,
            expires_at: 1_000,
        };
        store.put_session(&[3; 32], session).unwrap();
        assert_eq!(store.session(&[3; 32], 999).unwrap(), Some(session));
        assert_eq!(store.session(&[3; 32], 1_000).unwrap(), None);
        assert_eq!(store.session(&[4; 32], 0).unwrap(), None);
        assert_eq!(store.prune(2_000).unwrap().expired, 1);
    }

    #[test]
    fn the_log_appends_continuations_and_refuses_forks() {
        let (mut store, account, device) = store_with_account();
        let mut log = DeviceLog::from_entries(
            store
                .fetch_log(account)
                .unwrap()
                .iter()
                .map(|b| LogEntry::decode(b).unwrap())
                .collect(),
        )
        .unwrap();
        let second = TestDevice::new(2);
        log.add_device(
            second.id,
            second.keys.public(),
            vec![1, 2, 3],
            kb_keys::LogSigner::Device(&device.keys, device.id),
            5,
        )
        .unwrap();
        let entries = log.entries().to_vec();
        // Re-sending what is stored plus the continuation works, twice.
        store.append_log(account, entries.clone()).unwrap();
        store.append_log(account, entries.clone()).unwrap();
        assert_eq!(store.fetch_log(account).unwrap().len(), 2);
        assert!(
            store
                .log_state(account)
                .unwrap()
                .unwrap()
                .active(second.id)
                .is_some()
        );
        // A different entry at a stored position is a fork.
        let mut fork = DeviceLog::from_entries(vec![entries[0].clone()]).unwrap();
        let third = TestDevice::new(3);
        fork.add_device(
            third.id,
            third.keys.public(),
            vec![],
            kb_keys::LogSigner::Device(&device.keys, device.id),
            6,
        )
        .unwrap();
        let error = store
            .append_log(account, vec![fork.entries()[1].clone()])
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        // An unknown account.
        let error = store
            .append_log(AccountId::from_bytes([0; 16]), entries)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }
}
