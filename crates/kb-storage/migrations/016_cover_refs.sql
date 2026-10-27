-- Schema v16 (page covers, ADR-0040): `icon_refs` also holds the attachment a page's cover uses.
-- Forward-only; never edit a released migration, add a new numbered file instead. See
-- docs/data-model/sqlite-schema.md.
--
-- A page whose cover is `attachment:<attachment id>` references that attachment like one whose
-- icon is an uploaded image (014): for as long as the page exists, in the trash included. A page
-- can have an image icon and an image cover at once, so the key of the table changes from the page
-- to the pair (page, attachment); the same attachment as icon and as cover is one row, one
-- reference of the page. The table stays derived data: the cover and the icon live in the page's
-- meta map of the structure document, the core rewrites the rows with the document and compares
-- them at open. Every existing row is kept (it is valid under the new key, and no cover exists
-- yet). Ids are 16-byte BLOBs.

CREATE TABLE icon_refs_v16 (
  page_id       BLOB NOT NULL,
  space_id      BLOB NOT NULL,
  attachment_id BLOB NOT NULL,
  PRIMARY KEY (page_id, attachment_id)
);

INSERT INTO icon_refs_v16 (page_id, space_id, attachment_id)
  SELECT page_id, space_id, attachment_id FROM icon_refs;

DROP TABLE icon_refs;
ALTER TABLE icon_refs_v16 RENAME TO icon_refs;

-- What the reference count and the retention read, and what a space's rows are rewritten through
-- (the pages of a space are found by the primary key's first column).
CREATE INDEX icon_refs_by_attachment ON icon_refs(attachment_id);
CREATE INDEX icon_refs_by_space ON icon_refs(space_id);
