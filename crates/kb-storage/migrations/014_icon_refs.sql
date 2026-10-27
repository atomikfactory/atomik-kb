-- Schema v14 (page icons that are images, ADR-0029): the `icon_refs` table. Forward-only; never edit
-- a released migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md.
--
-- A page whose icon is `attachment:<attachment id>` references that attachment like a block does,
-- for as long as the page exists (in the trash included). The icon itself lives in the page's meta
-- map of the structure document; this table is the derived copy the retention of attachments reads:
-- one row per page that has an image icon (`page_id` is the primary key, a page has one icon), and
-- the reference count of an attachment adds the rows of this table to the blocks and the `files`
-- values (`kb_storage::AttachmentQueries`). Nothing is copied by the migration: a database from
-- before v14 has no image icon, and the core compares the table with the structure documents at open
-- (`verify_attachment_index`), rewriting what differs. Ids are 16-byte BLOBs.

CREATE TABLE icon_refs (
  page_id       BLOB PRIMARY KEY,
  space_id      BLOB NOT NULL,
  attachment_id BLOB NOT NULL
);

-- What the reference count and the retention read, and what a space's rows are rewritten through.
CREATE INDEX icon_refs_by_attachment ON icon_refs(attachment_id);
CREATE INDEX icon_refs_by_space ON icon_refs(space_id);
