-- Schema v3 (Milestone 3): link index and full-text search. Forward-only; never edit a
-- released migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md
-- and docs/development/milestone-03.md ("Storage"). Both indexes are derived from the
-- documents (rebuildable, never synced). Ids are 16-byte BLOBs.

-- One row per (referencing block, referenced page): the inline `pageRef` nodes of a page.
CREATE TABLE links (
  from_page  BLOB NOT NULL,
  from_block BLOB NOT NULL,
  to_page    BLOB NOT NULL,
  to_block   BLOB,
  PRIMARY KEY (from_page, from_block, to_page)
);
CREATE INDEX links_to ON links(to_page);

-- The text of the index: one title row per page (block_id NULL, title set, body empty) and
-- one body row per text-bearing block (title empty). A regular table, so that the rows of a
-- page or a block are found through an index; `fts` below only indexes the words.
CREATE TABLE fts_blocks (
  id       INTEGER PRIMARY KEY,
  page_id  BLOB NOT NULL,
  block_id BLOB,                         -- NULL for the title row
  title    TEXT NOT NULL DEFAULT '',
  body     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX fts_blocks_page ON fts_blocks(page_id, block_id);

-- The full-text index over `fts_blocks` (external content: it stores no text of its own).
-- Diacritics are folded so "cafe" finds "café". Never write to `fts` directly: the triggers
-- below keep it in step with `fts_blocks`. The columns count from 0 in snippet() and bm25():
-- title 0, body 1.
CREATE VIRTUAL TABLE fts USING fts5(
  title,
  body,
  content = 'fts_blocks',
  content_rowid = 'id',
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER fts_blocks_ai AFTER INSERT ON fts_blocks BEGIN
  INSERT INTO fts (rowid, title, body) VALUES (new.id, new.title, new.body);
END;

CREATE TRIGGER fts_blocks_ad AFTER DELETE ON fts_blocks BEGIN
  INSERT INTO fts (fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
END;

CREATE TRIGGER fts_blocks_au AFTER UPDATE ON fts_blocks BEGIN
  INSERT INTO fts (fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
  INSERT INTO fts (rowid, title, body) VALUES (new.id, new.title, new.body);
END;

-- Milestone 2 follow-ups: the single-column index is covered by the composite one, and
-- `doc_for_space` looks up (space_id, kind).
DROP INDEX pages_index_space;
CREATE INDEX docs_space_kind ON docs(space_id, kind);
