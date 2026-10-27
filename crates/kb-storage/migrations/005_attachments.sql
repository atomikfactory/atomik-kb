-- Schema v5 (Milestone 5): the attachment index and the attachment references. Forward-only;
-- never edit a released migration, add a new numbered file instead. See
-- docs/data-model/sqlite-schema.md, docs/development/milestone-05.md ("Storage") and
-- ADR-0020. Both tables are derived data: the manifests live in the structure document of
-- the space and the references come from the page documents, so the core can rebuild them.
-- Ids are 16-byte BLOBs; times are Unix milliseconds.

-- One row per attachment (a mirror of its manifest, for listing and for garbage collection).
-- `unreferenced_since` is NULL while a block references the attachment, and the time it
-- last lost its final reference otherwise.
CREATE TABLE attachments_index (
  attachment_id BLOB PRIMARY KEY, space_id BLOB NOT NULL, name TEXT NOT NULL,
  mime TEXT NOT NULL, size INTEGER NOT NULL, chunk_count INTEGER NOT NULL,
  created_at INTEGER NOT NULL, unreferenced_since INTEGER);
CREATE INDEX attachments_index_space ON attachments_index(space_id, created_at DESC);

-- One row per (attachment, page, block) that shows it.
CREATE TABLE attachment_refs (
  attachment_id BLOB NOT NULL, page_id BLOB NOT NULL, block_id BLOB NOT NULL,
  PRIMARY KEY (attachment_id, page_id, block_id));
CREATE INDEX attachment_refs_page ON attachment_refs(page_id);
