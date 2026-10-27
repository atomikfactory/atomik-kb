-- Schema v9 (reminders on date mentions): the date mentions of the pages. Forward-only; never
-- edit a released migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md
-- and ADR-0024. Derived data like `links` and `attachment_refs`: the mentions live in the page
-- documents, the core rewrites the rows of a page in the same transaction as its update row on
-- every save, `IndexRebuild` recreates them from scratch, and the table is not synced and not
-- part of a backup. Ids are 16-byte BLOBs.

-- One row per `dateMention` node with a valid date. `block_id` is the nearest enclosing block
-- with an id (what a reminder opens and what its text comes from), `node_id` the mention's own
-- id. `date` is `YYYY-MM-DD` and `time` `HH:mm` (a floating local date and time, ADR-0016).
-- A reminder is `reminder_minutes` before the moment (the date at the time, or at 09:00), and
-- `fire_at_local` is that fire time in seconds since 1970-01-01T00:00:00 as the device's local
-- wall clock reads it (the calendar fields counted as UTC: no time zone data is stored or
-- needed). Both are NULL for a mention without a reminder, and neither is NULL otherwise.
CREATE TABLE date_mentions (
  doc_id           BLOB NOT NULL,   -- the page's document (its id is the page id, ADR-0016)
  page_id          BLOB NOT NULL,
  block_id         BLOB NOT NULL,
  node_id          BLOB NOT NULL,
  date             TEXT NOT NULL,
  time             TEXT,
  reminder_minutes INTEGER,
  fire_at_local    INTEGER,
  PRIMARY KEY (page_id, node_id),
  CHECK ((reminder_minutes IS NULL) = (fire_at_local IS NULL))
);

-- What `RemindersDue` and the scheduler read: the reminders whose fire time falls in a window.
CREATE INDEX date_mentions_fire ON date_mentions(fire_at_local) WHERE fire_at_local IS NOT NULL;
