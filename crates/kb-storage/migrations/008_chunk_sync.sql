-- Schema v8 (Milestone 8): the per-chunk transfer state of attachment sync. Forward-only;
-- never edit a released migration, add a new numbered file instead. See
-- docs/data-model/sqlite-schema.md, docs/development/milestone-08.md ("Storage") and
-- ADR-0023. The table is derived bookkeeping like `attachments_index`: no foreign keys, not
-- backed up, rebuilt by the core from the manifests and the blob store when it is missing.
-- Ids are BLOBs (the chunk id is the 32-byte BLAKE3 hash of the ciphertext, the attachment
-- id 16 bytes); times are Unix milliseconds.

-- One row per chunk of every manifest this device knows. `state` is defined by the core:
-- 1 upload (local, the server lacks it), 2 download (wanted here, the server should have
-- it), 3 synced (both have it), 4 delete (a purge here asks the server to drop it).
-- `attempts` counts the tries that did not finish and `next_at` is the earliest time of
-- the next one (NULL = now).
CREATE TABLE chunk_sync (
  hash BLOB PRIMARY KEY,            -- chunk id (BLAKE3 of the ciphertext)
  attachment_id BLOB NOT NULL,      -- the manifest it belongs to (one per chunk by construction)
  state INTEGER NOT NULL,           -- 1 upload, 2 download, 3 synced, 4 delete; the core defines the meaning
  attempts INTEGER NOT NULL DEFAULT 0,
  next_at INTEGER                   -- not before this time (Unix ms); NULL = now
);
CREATE INDEX chunk_sync_work ON chunk_sync (state, next_at);
CREATE INDEX chunk_sync_attachment ON chunk_sync (attachment_id);
