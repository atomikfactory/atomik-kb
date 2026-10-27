-- Schema v7 (Milestone 7): `sync_pending` keyed by cursor and document. Forward-only;
-- never edit a released migration, add a new numbered file instead. See
-- docs/data-model/sqlite-schema.md and docs/development/milestone-07.md.
--
-- A snapshot that waits for its key is parked under the cursor it covers, and several
-- snapshots cover the same cursor (a device publishes them in one cycle), so a key on the
-- cursor alone kept the last of them only. Feed entries keep their unique cursor. The rows
-- are carried over.
CREATE TABLE sync_pending_v7 (cursor INTEGER NOT NULL, doc_id BLOB NOT NULL, bytes BLOB NOT NULL, reason INTEGER NOT NULL, received_at INTEGER NOT NULL, PRIMARY KEY (cursor, doc_id));
INSERT INTO sync_pending_v7 (cursor, doc_id, bytes, reason, received_at) SELECT cursor, doc_id, bytes, reason, received_at FROM sync_pending;
DROP TABLE sync_pending;
ALTER TABLE sync_pending_v7 RENAME TO sync_pending;
