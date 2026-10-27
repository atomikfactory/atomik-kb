-- Schema v20 (templates, ADR-0056): `pages_index.in_template`. Forward-only; never edit a released migration, add a
-- new numbered file instead. See docs/data-model/sqlite-schema.md.
--
-- A template is a page whose metadata has the `template` flag, and its sub-pages are part of it (they do not carry the
-- flag). Templates stay in the tree the core returns, but every list that is not the tree leaves them out (search, the
-- fuzzy title pass, comment search, backlinks, reminders, the semantic index, MCP's lists), and the page index is where
-- the core says so: `in_template` is 1 for a page that is a template or inside one, 0 for any other page. The column is
-- derived data like the rest of the table: the core computes it from the structure document (the page or an ancestor
-- flagged), rewrites it for a whole subtree when a flag changes or a page moves into or out of a template, and compares
-- it with the document at open. Every existing row is 0 (no template can exist before this version wrote the flag; a
-- space that holds some, written by a newer device, is rebuilt at open because the count of its template pages differs).
-- Counts do not change: a template is a live page.

ALTER TABLE pages_index ADD COLUMN in_template INTEGER NOT NULL DEFAULT 0;
