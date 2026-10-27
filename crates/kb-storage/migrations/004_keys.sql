-- Schema v4 (Milestone 4): the device log and the keyring. Forward-only; never edit a
-- released migration, add a new numbered file instead. See docs/data-model/sqlite-schema.md
-- and docs/development/milestone-04.md ("Storage"). Blobs are opaque here: kb-keys
-- encodes and verifies them. Ids are 16-byte BLOBs.

-- The account's device log: hash-chained, signed entries in sequence order.
CREATE TABLE device_log (
  seq   INTEGER PRIMARY KEY,
  entry BLOB NOT NULL,
  hash  BLOB NOT NULL
);

-- Keyring items: one row per (space, subject), a record or a tombstone.
CREATE TABLE keyring (
  space_id  BLOB NOT NULL,
  subject   BLOB NOT NULL,
  record    BLOB NOT NULL,
  tombstone INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (space_id, subject)
);
