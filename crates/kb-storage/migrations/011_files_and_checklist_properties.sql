-- Schema v11 (the `files` and `checklist` kinds of page properties, ADR-0026 amendment of
-- 2026-10-07): `page_properties` takes the two kinds, a column for the attachment of a `files` row
-- and one for the number of items of a `checklist` row. Forward-only; never edit a released
-- migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md.
--
-- SQLite cannot change a CHECK constraint, so the table is rebuilt: the rows are copied in the
-- order they were written (`rowid`, which a page's rows are read back by), then the indexes are
-- created again, with the two new ones. Derived data like the rest of the table: nothing is lost
-- that `IndexRebuild` could not recreate from the structure documents.
--
-- A `files` property has one row per attached file (`attachment_id`, a 16-byte BLOB; the other
-- value columns are NULL). A `checklist` has one row: `value_number` is the number of items done,
-- `value_total` the number of items (at least 1; an empty list is no value and has no row).
-- Attachments are referenced from here as well as from the blocks of page documents
-- (`attachment_refs`): the reference count of an attachment, and the list of the attachments a
-- page references, add the rows of this table (see `kb_storage::AttachmentQueries`).

CREATE TABLE page_properties_v11 (
  space_id      BLOB NOT NULL,
  page_id       BLOB NOT NULL,
  property_id   BLOB NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN
                  ('text', 'number', 'select', 'multi_select', 'date', 'checkbox', 'url',
                   'status', 'email', 'phone', 'files', 'checklist')),
  value_text    TEXT,
  value_number  REAL,
  value_bool    INTEGER CHECK (value_bool IS NULL OR value_bool IN (0, 1)),
  value_date    TEXT,
  value_time    TEXT,
  option_id     BLOB,
  attachment_id BLOB,
  value_total   INTEGER
);

INSERT INTO page_properties_v11
    (space_id, page_id, property_id, kind, value_text, value_number, value_bool, value_date,
     value_time, option_id)
  SELECT space_id, page_id, property_id, kind, value_text, value_number, value_bool, value_date,
         value_time, option_id
    FROM page_properties
   ORDER BY rowid;

DROP TABLE page_properties;
ALTER TABLE page_properties_v11 RENAME TO page_properties;

-- A page has at most one row per scalar property, one per option and one per attached file (the
-- three partial unique indexes), and its rows are found (and replaced) through
-- `page_properties_page`: a partial index serves only a query that implies its condition.
CREATE UNIQUE INDEX page_properties_scalar
  ON page_properties(page_id, property_id) WHERE option_id IS NULL AND attachment_id IS NULL;
CREATE UNIQUE INDEX page_properties_option_row
  ON page_properties(page_id, property_id, option_id) WHERE option_id IS NOT NULL;
CREATE UNIQUE INDEX page_properties_attachment_row
  ON page_properties(page_id, property_id, attachment_id) WHERE attachment_id IS NOT NULL;
CREATE INDEX page_properties_page ON page_properties(page_id);

-- What `PagesByProperty` reads: the pages with an option, and the pages with a text value.
CREATE INDEX page_properties_by_option ON page_properties(space_id, property_id, option_id);
CREATE INDEX page_properties_by_text ON page_properties(space_id, property_id, value_text);

-- What the retention of attachments reads: the rows that attach a file.
CREATE INDEX page_properties_by_attachment
  ON page_properties(attachment_id) WHERE attachment_id IS NOT NULL;
