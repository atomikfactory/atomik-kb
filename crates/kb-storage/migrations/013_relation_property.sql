-- Schema v13 (the `relation` kind of page properties, ADR-0027): `page_properties` takes the kind
-- `relation` and a column `related_page_id`. Forward-only; never edit a released migration, add a
-- new numbered file instead. See docs/data-model/sqlite-schema.md.
--
-- SQLite cannot change a CHECK constraint, so the table is rebuilt exactly as v12 left it (same
-- columns, indexes and row order, `rowid`) plus the new column. A `relation` value has one row per
-- related page that exists (`related_page_id`, in the order of the value), so "which pages list this
-- page" is an index lookup. The rows are what a page *stores*; the other half of a two-way relation
-- is the same table read through the paired property. The `rollup` kind and the computed kinds have
-- no rows.

CREATE TABLE page_properties_v13 (
  space_id        BLOB NOT NULL,
  page_id         BLOB NOT NULL,
  property_id     BLOB NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN
                    ('text', 'number', 'select', 'multi_select', 'date', 'checkbox', 'url',
                     'status', 'email', 'phone', 'files', 'checklist', 'place', 'relation')),
  value_text      TEXT,
  value_number    REAL,
  value_bool      INTEGER CHECK (value_bool IS NULL OR value_bool IN (0, 1)),
  value_date      TEXT,
  value_time      TEXT,
  option_id       BLOB,
  attachment_id   BLOB,
  value_total     INTEGER,
  related_page_id BLOB
);

INSERT INTO page_properties_v13
    (space_id, page_id, property_id, kind, value_text, value_number, value_bool, value_date,
     value_time, option_id, attachment_id, value_total)
  SELECT space_id, page_id, property_id, kind, value_text, value_number, value_bool, value_date,
         value_time, option_id, attachment_id, value_total
    FROM page_properties
   ORDER BY rowid;

DROP TABLE page_properties;
ALTER TABLE page_properties_v13 RENAME TO page_properties;

CREATE UNIQUE INDEX page_properties_scalar
  ON page_properties(page_id, property_id)
  WHERE option_id IS NULL AND attachment_id IS NULL AND related_page_id IS NULL;
CREATE UNIQUE INDEX page_properties_option_row
  ON page_properties(page_id, property_id, option_id) WHERE option_id IS NOT NULL;
CREATE UNIQUE INDEX page_properties_attachment_row
  ON page_properties(page_id, property_id, attachment_id) WHERE attachment_id IS NOT NULL;
CREATE UNIQUE INDEX page_properties_related_row
  ON page_properties(page_id, property_id, related_page_id) WHERE related_page_id IS NOT NULL;
CREATE INDEX page_properties_page ON page_properties(page_id);
CREATE INDEX page_properties_by_option ON page_properties(space_id, property_id, option_id);
CREATE INDEX page_properties_by_text ON page_properties(space_id, property_id, value_text);
CREATE INDEX page_properties_by_attachment
  ON page_properties(attachment_id) WHERE attachment_id IS NOT NULL;
CREATE INDEX page_properties_by_related
  ON page_properties(related_page_id) WHERE related_page_id IS NOT NULL;
