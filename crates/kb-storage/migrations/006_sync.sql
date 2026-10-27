-- Schema v6 (Milestone 6): the sync state. Forward-only; never edit a released migration,
-- add a new numbered file instead. See docs/data-model/sqlite-schema.md,
-- docs/development/milestone-06.md ("Storage") and ADR-0021. Everything here is local
-- bookkeeping of one device's view of the server: none of it is part of the account, none
-- of it is backed up, and a device that loses it re-derives it (a new server epoch makes
-- the core reset it). Ids are 16-byte BLOBs; times are Unix milliseconds. There are no
-- foreign keys on purpose: the keyring's pseudo-document (the all-zero id) is not a row of
-- `docs`, and a pending entry names a document this device does not have yet.

-- Settings of the sync client as text pairs: server_url, enabled, server_epoch (hex),
-- remote_cursor, chain_hash (hex), last_push_at, last_pull_at, fork_detected.
CREATE TABLE sync_config (key TEXT PRIMARY KEY, value TEXT NOT NULL);

-- Per document: how far its local update log is pushed, the next sequence number of this
-- device for it, and the snapshot bookkeeping (what the last snapshot covers, how much
-- followed it).
CREATE TABLE sync_doc_state (doc_id BLOB PRIMARY KEY, pushed_up_to_seq INTEGER NOT NULL DEFAULT 0,
  next_device_seq INTEGER NOT NULL DEFAULT 1, snapshot_covers INTEGER, entries_since_snapshot INTEGER NOT NULL DEFAULT 0,
  bytes_since_snapshot INTEGER NOT NULL DEFAULT 0, remote_snapshot_covers INTEGER);

-- Continuity: the last device_seq applied per (document, device).
CREATE TABLE sync_seen (doc_id BLOB NOT NULL, device_id BLOB NOT NULL, last_seq INTEGER NOT NULL, PRIMARY KEY (doc_id, device_id));

-- The latest signed head of each device (fork and rollback detection).
CREATE TABLE sync_heads (device_id BLOB PRIMARY KEY, cursor INTEGER NOT NULL, chain_hash BLOB NOT NULL, at_ms INTEGER NOT NULL, signed BLOB NOT NULL, seen_at INTEGER NOT NULL);

-- Entries received and verified but not applicable yet, by feed cursor.
-- reason: 1 = key missing, 2 = structure missing.
CREATE TABLE sync_pending (cursor INTEGER PRIMARY KEY, doc_id BLOB NOT NULL, bytes BLOB NOT NULL, reason INTEGER NOT NULL, received_at INTEGER NOT NULL);

-- Keyring items and device log entries the server has been sent (1) or not (0). Existing
-- rows start at 0: the first sync publishes everything.
ALTER TABLE keyring ADD COLUMN published INTEGER NOT NULL DEFAULT 0;
ALTER TABLE device_log ADD COLUMN published INTEGER NOT NULL DEFAULT 0;
