-- Schema v10 (typed page properties): the property values of the pages, and a kind marker on the
-- rows of the text index. Forward-only; never edit a released migration, add a new numbered file
-- instead. See docs/data-model/sqlite-schema.md and ADR-0026. Derived data like `links` and
-- `date_mentions`: the schema and the values live in the structure documents, the core rewrites
-- the rows of a page in the same transaction as the update row of the document that changed,
-- `IndexRebuild` recreates them from scratch, and they are not synced and not part of a backup.
-- Ids are 16-byte BLOBs.

-- One row per scalar value (text, url, email, phone, number, checkbox, date) and one per selected
-- option (select and status: one; multi-select: one per option) of a page's property. Trashed
-- pages keep their rows (queries leave them out through `pages_index.trashed_at`). A value of a
-- property the schema does not have, or an option it does not list, has no row.
--
-- `value_text` is the string of a text-like value, `value_number` a number, `value_bool` a
-- checkbox (0 or 1), `value_date` and `value_time` a date (`YYYY-MM-DD`) and its optional time of
-- day (`HH:mm`), `option_id` an option. Exactly one of them is set on a row of each kind.
CREATE TABLE page_properties (
  space_id     BLOB NOT NULL,
  page_id      BLOB NOT NULL,
  property_id  BLOB NOT NULL,
  kind         TEXT NOT NULL CHECK (kind IN
                 ('text', 'number', 'select', 'multi_select', 'date', 'checkbox', 'url',
                  'status', 'email', 'phone')),
  value_text   TEXT,
  value_number REAL,
  value_bool   INTEGER CHECK (value_bool IS NULL OR value_bool IN (0, 1)),
  value_date   TEXT,
  value_time   TEXT,
  option_id    BLOB
);

-- A page has at most one row per scalar property and one per option (the two partial unique
-- indexes), and its rows are found (and replaced) through `page_properties_page`: a partial
-- index serves only a query that implies its condition.
CREATE UNIQUE INDEX page_properties_scalar
  ON page_properties(page_id, property_id) WHERE option_id IS NULL;
CREATE UNIQUE INDEX page_properties_option_row
  ON page_properties(page_id, property_id, option_id) WHERE option_id IS NOT NULL;
CREATE INDEX page_properties_page ON page_properties(page_id);

-- What `PagesByProperty` reads: the pages with an option, and the pages with a text value.
CREATE INDEX page_properties_by_option ON page_properties(space_id, property_id, option_id);
CREATE INDEX page_properties_by_text ON page_properties(space_id, property_id, value_text);

-- The rows of the text index: 0 for the title row and the block rows of a page (what v3 wrote), 1
-- for a property row (`block_id` is then the id of the property, `body` the option names or the
-- text value, `title` empty). A property row is replaced when the values of the page change,
-- without touching the rows of the document, and the other way round.
ALTER TABLE fts_blocks ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;
-- The property rows of a page, and of a space's pages, are found without a pass over the rows of
-- the documents.
CREATE INDEX fts_blocks_property_rows ON fts_blocks(page_id) WHERE kind = 1;
