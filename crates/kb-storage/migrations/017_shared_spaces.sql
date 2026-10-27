-- Schema v17 (Milestone 13, slice 1: shared spaces). Forward-only; never edit a released migration,
-- add a new numbered file instead. See docs/development/milestone-13.md (1.4), docs/security/sharing.md
-- (section 5) and ADR-0043 to ADR-0045. Local bookkeeping of this device's view of the feeds, like
-- the sync tables of v6: none of it is part of the account, none of it is backed up, and a device
-- that loses it re-derives it (a new server epoch makes the core reset it). Ids are BLOBs (16 bytes
-- unless noted); times are Unix milliseconds. There are no foreign keys on purpose, as in v6.

-- One row per feed this device syncs. The personal feed's id is the account id; the core inserts
-- its row once the account id exists. kind: 1 personal, 2 shared (kb_types::sync::FeedKind).
CREATE TABLE sync_feeds (
  feed BLOB PRIMARY KEY,               -- 16 bytes; the account id for the personal feed
  kind INTEGER NOT NULL,               -- 1 personal, 2 shared
  space_id BLOB,                       -- NULL for the personal feed
  cursor INTEGER NOT NULL DEFAULT 0,
  chain_hash BLOB,
  max_covers INTEGER NOT NULL DEFAULT 0,
  chain_history BLOB,                  -- the bounded (cursor, chain hash) pairs of v1
  fork INTEGER NOT NULL DEFAULT 0,
  server_epoch BLOB
);

-- The device logs of the other members' accounts, as the server served them (verified by replay
-- before use; a row is one encoded LogEntry).
CREATE TABLE foreign_device_logs (
  account_id BLOB NOT NULL, seq INTEGER NOT NULL, entry BLOB NOT NULL, fetched_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, seq)
);

-- Quarantine (ADR-0043): items a role does not allow, or that a non-member wrote. Bounded per feed
-- (kb-storage's sharing module: REJECTED_MAX_ROWS_PER_FEED rows and REJECTED_MAX_BYTES_PER_FEED
-- bytes, the oldest go first); what the bound dropped is counted in sync_rejected_dropped.
CREATE TABLE sync_rejected (
  feed BLOB NOT NULL, cursor INTEGER NOT NULL, doc_id BLOB NOT NULL, -- the all-zero id for keyring and membership items
  account_id BLOB NOT NULL, device_id BLOB NOT NULL,
  reason INTEGER NOT NULL,            -- 1 not a member at the cursor, 2 role does not allow it, 3 snapshot not dominant
  bytes BLOB NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (feed, cursor, doc_id)
);

-- Snapshot dominance (ADR-0045): the end version vector and cursor of the last entry applied per
-- (document, writer device).
CREATE TABLE sync_entry_marks (
  doc_id BLOB NOT NULL, device_id BLOB NOT NULL, end_vv BLOB NOT NULL, cursor INTEGER NOT NULL,
  PRIMARY KEY (doc_id, device_id)
);

-- The membership log of a shared feed as fetched (the encoded MembershipEntry, with the feed cursor
-- it was found at), and the derived cache of its fold (the encoded MembershipState; rebuilt from the
-- entries whenever it is missing or stale).
CREATE TABLE membership_entries (feed BLOB NOT NULL, seq INTEGER NOT NULL, cursor INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (feed, seq));
CREATE TABLE membership_state (feed BLOB PRIMARY KEY, seq INTEGER NOT NULL, head_hash BLOB NOT NULL, state BLOB NOT NULL);

-- How many quarantined items of each writer the bound of `sync_rejected` dropped, so the member
-- panel's "N changes from Bob were refused" keeps counting them.
CREATE TABLE sync_rejected_dropped (
  feed BLOB NOT NULL, account_id BLOB NOT NULL, dropped INTEGER NOT NULL,
  PRIMARY KEY (feed, account_id)
);

-- ---- the per-feed forms of the v6 bookkeeping --------------------------------------------------
-- A feed has its own cursor, chain, heads, per-document push state and continuity, so these four
-- tables gain a `feed` column, first in the key: `sync_heads` (feed, device_id), `sync_doc_state`
-- (feed, doc_id) and `sync_seen` (feed, doc_id, device_id) (the keyring pseudo-document, the all-zero
-- document id, exists once per feed), and `sync_pending` (feed, cursor, doc_id), which a feed that
-- is left must be able to forget. Every row that exists now belongs to the personal feed: its id is
-- the account id, which `meta` holds as a UUID string ('account_id'); with no (or an unreadable)
-- account id, which no synced device lacks, the rows go to the all-zero id and no feed finds them.
-- `sync_config` stays for the settings that are not per feed. All the rows are carried over.

CREATE TABLE sync_doc_state_v17 (
  feed BLOB NOT NULL,
  doc_id BLOB NOT NULL,
  pushed_up_to_seq INTEGER NOT NULL DEFAULT 0,
  next_device_seq INTEGER NOT NULL DEFAULT 1,
  snapshot_covers INTEGER,
  entries_since_snapshot INTEGER NOT NULL DEFAULT 0,
  bytes_since_snapshot INTEGER NOT NULL DEFAULT 0,
  remote_snapshot_covers INTEGER,
  PRIMARY KEY (feed, doc_id)
);
WITH personal(feed) AS (
  SELECT CASE WHEN length(unhex(replace(value, '-', ''))) = 16
              THEN unhex(replace(value, '-', '')) ELSE zeroblob(16) END
  FROM (SELECT (SELECT value FROM meta WHERE key = 'account_id') AS value)
)
INSERT INTO sync_doc_state_v17 (feed, doc_id, pushed_up_to_seq, next_device_seq, snapshot_covers,
    entries_since_snapshot, bytes_since_snapshot, remote_snapshot_covers)
  SELECT personal.feed, doc_id, pushed_up_to_seq, next_device_seq, snapshot_covers,
    entries_since_snapshot, bytes_since_snapshot, remote_snapshot_covers
  FROM sync_doc_state, personal;
DROP TABLE sync_doc_state;
ALTER TABLE sync_doc_state_v17 RENAME TO sync_doc_state;

CREATE TABLE sync_seen_v17 (
  feed BLOB NOT NULL,
  doc_id BLOB NOT NULL,
  device_id BLOB NOT NULL,
  last_seq INTEGER NOT NULL,
  PRIMARY KEY (feed, doc_id, device_id)
);
WITH personal(feed) AS (
  SELECT CASE WHEN length(unhex(replace(value, '-', ''))) = 16
              THEN unhex(replace(value, '-', '')) ELSE zeroblob(16) END
  FROM (SELECT (SELECT value FROM meta WHERE key = 'account_id') AS value)
)
INSERT INTO sync_seen_v17 (feed, doc_id, device_id, last_seq)
  SELECT personal.feed, doc_id, device_id, last_seq FROM sync_seen, personal;
DROP TABLE sync_seen;
ALTER TABLE sync_seen_v17 RENAME TO sync_seen;

CREATE TABLE sync_heads_v17 (
  feed BLOB NOT NULL,
  device_id BLOB NOT NULL,
  cursor INTEGER NOT NULL,
  chain_hash BLOB NOT NULL,
  at_ms INTEGER NOT NULL,
  signed BLOB NOT NULL,
  seen_at INTEGER NOT NULL,
  PRIMARY KEY (feed, device_id)
);
WITH personal(feed) AS (
  SELECT CASE WHEN length(unhex(replace(value, '-', ''))) = 16
              THEN unhex(replace(value, '-', '')) ELSE zeroblob(16) END
  FROM (SELECT (SELECT value FROM meta WHERE key = 'account_id') AS value)
)
INSERT INTO sync_heads_v17 (feed, device_id, cursor, chain_hash, at_ms, signed, seen_at)
  SELECT personal.feed, device_id, cursor, chain_hash, at_ms, signed, seen_at
  FROM sync_heads, personal;
DROP TABLE sync_heads;
ALTER TABLE sync_heads_v17 RENAME TO sync_heads;

CREATE TABLE sync_pending_v17 (
  feed BLOB NOT NULL,
  cursor INTEGER NOT NULL,
  doc_id BLOB NOT NULL,
  bytes BLOB NOT NULL,
  reason INTEGER NOT NULL,
  received_at INTEGER NOT NULL,
  PRIMARY KEY (feed, cursor, doc_id)
);
WITH personal(feed) AS (
  SELECT CASE WHEN length(unhex(replace(value, '-', ''))) = 16
              THEN unhex(replace(value, '-', '')) ELSE zeroblob(16) END
  FROM (SELECT (SELECT value FROM meta WHERE key = 'account_id') AS value)
)
INSERT INTO sync_pending_v17 (feed, cursor, doc_id, bytes, reason, received_at)
  SELECT personal.feed, cursor, doc_id, bytes, reason, received_at
  FROM sync_pending, personal;
DROP TABLE sync_pending;
ALTER TABLE sync_pending_v17 RENAME TO sync_pending;
