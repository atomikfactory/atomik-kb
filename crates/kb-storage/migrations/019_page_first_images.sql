-- Schema v19 (database views, ADR-0054): the `page_first_images` table. Forward-only; never edit a released
-- migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md.
--
-- A gallery card previews the page's cover, else the first picture in the page, and a gallery of a hundred pages
-- must not open a hundred documents: this table is the derived answer. One row per page that has been scanned
-- (live or trashed): `attachment_id` is the attachment of the first `image` block, or of the first usable item of
-- the first `gallery` block, in document order; NULL means "scanned, the page has no picture". A page with no row
-- has not been scanned: the request that needs it scans it (nothing is copied here and there is no backfill at
-- open). Device-local like `icon_refs`: never synced, not part of a backup, rebuildable from the documents (the
-- text-index pass of the core rewrites a page's row with the rest of its derived rows, `IndexRebuild` refills it).
-- The attachment is already referenced by its block, so there is no foreign key and no index by attachment: this
-- table adds no retention. Ids are 16-byte BLOBs.

CREATE TABLE page_first_images (
  page_id       BLOB PRIMARY KEY,
  space_id      BLOB NOT NULL,
  attachment_id BLOB
);
