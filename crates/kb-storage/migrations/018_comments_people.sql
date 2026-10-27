-- Schema v18 (Milestone 13, slice 2: comments, person mentions and the Person property). Forward-only;
-- never edit a released migration, add a new numbered file instead (this one is unreleased until
-- slice 2 merges, so its owner may still refine it). See docs/development/milestone-13.md (2.2),
-- docs/security/sharing.md (section 8), ADR-0048 and ADR-0049. Ids are BLOBs (16 bytes); times are
-- Unix milliseconds. There are no foreign keys on purpose, as in v6 and v17.

-- ---- comments (ADR-0048) -----------------------------------------------------------------------
-- The records of a page's comment stream that this device holds: every record the core verified
-- (signature, author account, page, the author's role at the carrying item's cursor), whether the
-- fold counts it or not (an Edit that arrived before its Post counts once the Post is here). One row
-- per record; the fold is computed from the rows, so a snapshot of the stream lists the rows the fold
-- keeps. `record` is the whole encoded, signed record (kb_model::CommentRecord::encode); the other
-- columns are its fields (and what the core resolved for it), kept to read a page's threads without
-- decoding every record. The stream's entries travel as documents (a `docs` row of kind 3, whose
-- updates are the records' blobs): this table is the device's index of them. Local, not synced.
CREATE TABLE comments (
  record_id  BLOB PRIMARY KEY,
  space_id   BLOB NOT NULL,
  page_id    BLOB NOT NULL,
  thread_id  BLOB NOT NULL,
  comment_id BLOB NOT NULL,
  op         INTEGER NOT NULL CHECK (op IN (1, 2, 3, 4)),  -- 1 post, 2 edit, 3 delete, 4 resolve
  author     BLOB NOT NULL,                               -- the author's account id
  device     BLOB NOT NULL,
  rev        INTEGER NOT NULL,
  at         INTEGER NOT NULL,
  cursor     INTEGER,                                     -- the feed cursor of the carrying item; NULL for a local record not pushed yet
  author_role INTEGER NOT NULL,                           -- kb_types::MemberRole::to_byte at that cursor
  record     BLOB NOT NULL
);
CREATE INDEX comments_page ON comments(page_id, thread_id);
CREATE INDEX comments_comment ON comments(comment_id);
CREATE INDEX comments_space ON comments(space_id);

-- The words of comments for the text index, labelled "in comment" in the results: one row per comment
-- whose Post counts and that is not deleted (its latest valid body, the text items of the body joined
-- with one space; mentions and page references add no words). The core rewrites a comment's row
-- whenever the fold changes it. A search joins `page_id` to `pages_index` for the title, the space and
-- the trash. Diacritics are folded like `fts`. `page_id`, `comment_id` and `space_id` are stored but
-- not indexed (they are 16-byte blobs, not words).
CREATE VIRTUAL TABLE comment_fts USING fts5(
  body,
  page_id UNINDEXED,
  comment_id UNINDEXED,
  space_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);

-- ---- notifications (ADR-0048, ADR-0049) --------------------------------------------------------
-- This device's inbox: rows raised from remote changes only, never from this device's own writes. The
-- actor's name and the page's title are read when a row is listed, never stored. `kind` is
-- kb_types::NotificationKind: 1 ThreadReply, 2 CommentMention, 3 PageMention, 4 PersonAssigned.
-- `id` only ever grows while the newest row exists, so `NotificationList` pages by it. The table is
-- kept to kb_types::comments::NOTIFICATIONS_MAX_ROWS rows. Local to the device: not synced, not backed up.
CREATE TABLE notifications (
  id         INTEGER PRIMARY KEY,
  kind       INTEGER NOT NULL CHECK (kind IN (1, 2, 3, 4)),
  space_id   BLOB NOT NULL,
  page_id    BLOB,
  block_id   BLOB,
  comment_id BLOB,
  actor      BLOB NOT NULL,                               -- the actor's account id
  at         INTEGER NOT NULL,
  read       INTEGER NOT NULL DEFAULT 0 CHECK (read IN (0, 1))
);
CREATE INDEX notifications_unread ON notifications(read, id);

-- ---- person mentions (ADR-0049) ----------------------------------------------------------------
-- Derived data like `links` and `date_mentions`: the mentions live in the page documents, the core
-- rewrites the rows of a page in the same transaction as its update row on every save, `IndexRebuild`
-- recreates them from scratch, and the table is not synced and not part of a backup. One row per
-- (page document, enclosing block, mentioned account); the diff of a remote change against it decides
-- whether a `PageMention` notification is raised. A page document's id is the page id (ADR-0016).
CREATE TABLE person_mentions (
  doc_id     BLOB NOT NULL,
  block_id   BLOB NOT NULL,
  account_id BLOB NOT NULL,
  PRIMARY KEY (doc_id, block_id, account_id)
);
CREATE INDEX person_mentions_account ON person_mentions(account_id);

-- ---- reminders per account (ADR-0047) ----------------------------------------------------------
-- `reminderFor` of a date mention: the account whose devices fire the reminder in a shared space.
-- NULL: the attribute is absent (the Owners' devices fire it in a shared space, this device in a
-- personal one). Derived like the rest of the table.
ALTER TABLE date_mentions ADD COLUMN reminder_for BLOB;

-- ---- the Person property (ADR-0049) ------------------------------------------------------------
-- `page_properties` takes the kind `person` and a column `account_id`. SQLite cannot change a CHECK
-- constraint, so the table is rebuilt exactly as v13 left it (same columns, indexes and row order,
-- `rowid`) plus the new column. A `person` value has one row per listed account (`account_id`, in the
-- order of the value), so "which pages list this account" is an index lookup. As for every kind the
-- rows are what a page stores, derived from the structure document.

CREATE TABLE page_properties_v18 (
  space_id        BLOB NOT NULL,
  page_id         BLOB NOT NULL,
  property_id     BLOB NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN
                    ('text', 'number', 'select', 'multi_select', 'date', 'checkbox', 'url',
                     'status', 'email', 'phone', 'files', 'checklist', 'place', 'relation',
                     'person')),
  value_text      TEXT,
  value_number    REAL,
  value_bool      INTEGER CHECK (value_bool IS NULL OR value_bool IN (0, 1)),
  value_date      TEXT,
  value_time      TEXT,
  option_id       BLOB,
  attachment_id   BLOB,
  value_total     INTEGER,
  related_page_id BLOB,
  account_id      BLOB
);

INSERT INTO page_properties_v18
    (space_id, page_id, property_id, kind, value_text, value_number, value_bool, value_date,
     value_time, option_id, attachment_id, value_total, related_page_id)
  SELECT space_id, page_id, property_id, kind, value_text, value_number, value_bool, value_date,
         value_time, option_id, attachment_id, value_total, related_page_id
    FROM page_properties
   ORDER BY rowid;

DROP TABLE page_properties;
ALTER TABLE page_properties_v18 RENAME TO page_properties;

CREATE UNIQUE INDEX page_properties_scalar
  ON page_properties(page_id, property_id)
  WHERE option_id IS NULL AND attachment_id IS NULL AND related_page_id IS NULL
    AND account_id IS NULL;
CREATE UNIQUE INDEX page_properties_option_row
  ON page_properties(page_id, property_id, option_id) WHERE option_id IS NOT NULL;
CREATE UNIQUE INDEX page_properties_attachment_row
  ON page_properties(page_id, property_id, attachment_id) WHERE attachment_id IS NOT NULL;
CREATE UNIQUE INDEX page_properties_related_row
  ON page_properties(page_id, property_id, related_page_id) WHERE related_page_id IS NOT NULL;
CREATE UNIQUE INDEX page_properties_person_row
  ON page_properties(page_id, property_id, account_id) WHERE account_id IS NOT NULL;
CREATE INDEX page_properties_page ON page_properties(page_id);
CREATE INDEX page_properties_by_option ON page_properties(space_id, property_id, option_id);
CREATE INDEX page_properties_by_text ON page_properties(space_id, property_id, value_text);
CREATE INDEX page_properties_by_attachment
  ON page_properties(attachment_id) WHERE attachment_id IS NOT NULL;
CREATE INDEX page_properties_by_related
  ON page_properties(related_page_id) WHERE related_page_id IS NOT NULL;
-- What `PagesByProperty` with `ContainsPerson` and `ContainsMe` reads.
CREATE INDEX page_properties_by_account
  ON page_properties(space_id, property_id, account_id) WHERE account_id IS NOT NULL;
