-- Schema v1 (Milestone 1): the baseline. Forward-only; never edit a released migration,
-- add a new numbered file instead. See docs/data-model/sqlite-schema.md.
-- Conventions: ids are 16-byte BLOBs, times are Unix milliseconds, JSON is TEXT.
CREATE TABLE meta      (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE workspace (id BLOB PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE settings  (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);
