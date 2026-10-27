-- Schema v2 (Milestone 2): documents and the derived page index. Forward-only; never edit
-- a released migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md.
-- Conventions: ids are 16-byte BLOBs, times are Unix milliseconds. The blob columns hold
-- opaque bytes (the kb-doc envelope lives inside them); this crate never interprets them.

CREATE TABLE docs (
  doc_id             BLOB PRIMARY KEY,
  kind               INTEGER NOT NULL,   -- 0 structure, 1 page, 2 account
  space_id           BLOB,               -- owning space; NULL for the account document
  created_at         INTEGER NOT NULL,
  format_version     INTEGER NOT NULL,
  min_reader_version INTEGER NOT NULL
);

-- At most one snapshot per document; the updates that follow it live in doc_updates.
CREATE TABLE doc_snapshots (
  doc_id     BLOB PRIMARY KEY REFERENCES docs(doc_id) ON DELETE CASCADE,
  version    BLOB NOT NULL,              -- encoded version the snapshot was taken at
  blob       BLOB NOT NULL,
  created_at INTEGER NOT NULL
);

-- The update log since the snapshot, in local_seq order (1, 2, 3, ... per document).
-- Compaction deletes the rows a new snapshot covers.
CREATE TABLE doc_updates (
  doc_id     BLOB NOT NULL REFERENCES docs(doc_id) ON DELETE CASCADE,
  local_seq  INTEGER NOT NULL,
  blob       BLOB NOT NULL,
  origin     INTEGER NOT NULL,           -- 0 local, 1 remote, 2 core
  created_at INTEGER NOT NULL,
  PRIMARY KEY (doc_id, local_seq)
);

-- Derived read model of the structure documents; rebuilt whenever it disagrees with them.
CREATE TABLE pages_index (
  page_id    BLOB PRIMARY KEY,
  space_id   BLOB NOT NULL,
  parent_id  BLOB,
  title      TEXT NOT NULL,
  trashed_at INTEGER,
  updated_at INTEGER NOT NULL
);

CREATE INDEX docs_kind                 ON docs(kind);
CREATE INDEX docs_space                ON docs(space_id);
CREATE INDEX pages_index_space         ON pages_index(space_id);
CREATE INDEX pages_index_space_trashed ON pages_index(space_id, trashed_at);
