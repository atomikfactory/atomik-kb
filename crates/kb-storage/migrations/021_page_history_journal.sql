-- Schema v21 (page history, ADR-0057): the `page_history_journal` table. Forward-only; never edit a released
-- migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md.
--
-- A device keeps one row per editing session of a page document it witnessed (its own edits and the sync entries it
-- applied): when the session began and ended on this device, the document version at its end, who made it (this device,
-- or the account and device of the sync entry), and the page's title then. Dates and authors exist nowhere else (the
-- documents record neither, ADR-0011), so the table is NOT derived: `IndexRebuild` leaves it alone. It is device-local:
-- never synced and not part of a backup (a backup holds documents, settings and keys, not tables). It lives and dies with
-- the page's document row (the cascade) and is also deleted by name when a page is purged.
--
-- `version` is the id of the document version at the end of the session: the document's frontiers written
-- "<peer>@<counter>" (peers in numeric order, joined with ","). `base_version` is the document's version before the
-- session's first write (the empty text for a document that began with the session): the operations of `version` that are
-- not in `base_version` are what the session wrote, which is how the history older than the journal is told apart.
-- Ids are 16-byte BLOBs, times are Unix milliseconds.

CREATE TABLE page_history_journal (
  doc_id       BLOB    NOT NULL REFERENCES docs(doc_id) ON DELETE CASCADE,  -- the page's document (= the page id)
  seq          INTEGER NOT NULL,                                            -- 1, 2, 3, ... per document
  kind         TEXT    NOT NULL CHECK (kind IN ('edit', 'restore')),
  started_at   INTEGER NOT NULL,
  ended_at     INTEGER NOT NULL,
  base_version TEXT    NOT NULL,
  version      TEXT    NOT NULL,
  device_id    BLOB,                                                        -- NULL: unknown
  account_id   BLOB,                                                        -- NULL: unknown
  this_device  INTEGER NOT NULL CHECK (this_device IN (0, 1)),
  title        TEXT,                                                        -- NULL: not recorded
  PRIMARY KEY (doc_id, seq)
) WITHOUT ROWID;
