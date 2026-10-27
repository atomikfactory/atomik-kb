-- Schema v15 (local AI and the MCP server, ADR-0037, docs/architecture/ai.md sections 4 and 7).
-- Forward-only; never edit a released migration, add a new numbered file instead.
--
-- The semantic index is derived data: the vectors of chunks of the pages' indexed text
-- (`fts_blocks`), computed by an embedding model through the shell. It is device-local, never
-- synced and never in a backup; a restore or a new device rebuilds it. Ids are 16-byte BLOBs,
-- hashes 32-byte BLOBs (blake3).

-- One row per page whose chunks are stored: the hash of its chunk hashes (equal = nothing to
-- re-embed), the embedding model, how many chunks.
CREATE TABLE ai_pages (
  page_id     BLOB PRIMARY KEY,
  source_hash BLOB NOT NULL,
  model       TEXT NOT NULL,
  chunks      INTEGER NOT NULL,
  indexed_at  INTEGER NOT NULL
) WITHOUT ROWID;

-- The chunks of a page: the blocks they cover (16 bytes each, concatenated), the hash of the
-- embedded text (vectors are reused by hash), the text, the unit-length vector (f32 little-endian).
CREATE TABLE ai_chunks (
  page_id   BLOB NOT NULL,
  seq       INTEGER NOT NULL,
  block_ids BLOB NOT NULL,
  hash      BLOB NOT NULL,
  text      TEXT NOT NULL,
  vector    BLOB NOT NULL,
  PRIMARY KEY (page_id, seq)
) WITHOUT ROWID;
CREATE INDEX ai_chunks_hash ON ai_chunks(hash);

-- Pages whose indexed text changed since their chunks were stored (or that have none yet). The
-- triggers below keep it in step with `fts_blocks`; the core removes a page once it is indexed,
-- unchanged, trashed or gone.
CREATE TABLE ai_dirty (
  page_id BLOB PRIMARY KEY
) WITHOUT ROWID;

CREATE TRIGGER ai_dirty_on_fts_insert AFTER INSERT ON fts_blocks BEGIN
  INSERT OR IGNORE INTO ai_dirty(page_id) VALUES (new.page_id);
END;
CREATE TRIGGER ai_dirty_on_fts_update AFTER UPDATE ON fts_blocks BEGIN
  INSERT OR IGNORE INTO ai_dirty(page_id) VALUES (new.page_id);
END;
CREATE TRIGGER ai_dirty_on_fts_delete AFTER DELETE ON fts_blocks BEGIN
  INSERT OR IGNORE INTO ai_dirty(page_id) VALUES (old.page_id);
END;

-- Every page indexed so far starts dirty (nothing is embedded until AI is turned on).
INSERT OR IGNORE INTO ai_dirty(page_id) SELECT DISTINCT page_id FROM fts_blocks;

-- MCP clients: an outside AI tool with its own token (only its SHA-256 is stored) and its grants
-- per space (JSON array of `{spaceId, read, write}`).
CREATE TABLE mcp_clients (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,
  token_hash   BLOB NOT NULL UNIQUE,
  grants       TEXT NOT NULL DEFAULT '[]',
  created_at   INTEGER NOT NULL,
  last_used_at INTEGER
);

-- The audit log of MCP tool calls (the newest 10,000 rows are kept). `summary` is the query or
-- the page title, never page text. `outcome`: ok, denied, invalid, error.
CREATE TABLE mcp_audit (
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  client_id   TEXT NOT NULL,
  client_name TEXT NOT NULL,
  tool        TEXT NOT NULL,
  space_id    BLOB,
  page_id     BLOB,
  summary     TEXT NOT NULL,
  outcome     TEXT NOT NULL
);
CREATE INDEX mcp_audit_at ON mcp_audit(at);
CREATE INDEX mcp_audit_client ON mcp_audit(client_id, id);
