//! MCP clients and the audit log (schema v15, ADR-0037, `docs/architecture/ai.md` section 7): the
//! tables `mcp_clients` and `mcp_audit`. Same conventions as [`crate::icon_refs`]: every method
//! exists on [`Db`] and [`Tx`] through [`McpQueries`], both running the same private functions over
//! a connection, and the multi-statement methods of a [`Db`] run as one atomic unit. Rows are read
//! as raw column values first and validated afterwards, so a damaged row is a typed `InvalidData`
//! error and never a panic. Errors never hold a name, a token hash or a summary.
//!
//! # The tables
//!
//! `mcp_clients(id, name, token_hash, grants, created_at, last_used_at)`: `id` is 32 lower-case hex
//! characters, `token_hash` the 32 bytes of the SHA-256 of the bearer token (unique: the lookup of
//! a call), `grants` a JSON array of [`McpGrant`]. This crate stores the grants it is given; which
//! spaces exist is the core's business. Deleting a client keeps its audit rows (they carry the
//! client's name at the time).
//!
//! `mcp_audit(id, at, client_id, client_name, tool, space_id, page_id, summary, outcome)`: one row
//! per tool call, `id` increasing, `outcome` one of `ok`, `denied`, `invalid`, `error`. An insert
//! prunes the table to the newest [`MCP_AUDIT_KEEP`] rows in the same unit.

use kb_types::ai::{McpGrant, McpOutcome};
use kb_types::{PageId, SpaceId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// How many audit rows are kept: an insert deletes the older ones.
pub const MCP_AUDIT_KEEP: u32 = 10_000;

/// A row of `mcp_clients`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientRow {
    /// 32 lower-case hex characters.
    pub id: String,
    /// The name the person gave it.
    pub name: String,
    /// SHA-256 of the bearer token.
    pub token_hash: [u8; 32],
    /// What the client may do, per space.
    pub grants: Vec<McpGrant>,
    /// Creation time, Unix ms.
    pub created_at: i64,
    /// Last authenticated call, Unix ms.
    pub last_used_at: Option<i64>,
}

/// An audit row to insert (the id is the database's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMcpAudit {
    /// Time, Unix ms.
    pub at: i64,
    /// Client id.
    pub client_id: String,
    /// The client's name now.
    pub client_name: String,
    /// Tool name.
    pub tool: String,
    /// Space concerned.
    pub space_id: Option<SpaceId>,
    /// Page concerned.
    pub page_id: Option<PageId>,
    /// The query or the page title (never page text); stored as given.
    pub summary: String,
    /// Outcome.
    pub outcome: McpOutcome,
}

/// A row of `mcp_audit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuditRow {
    /// Row id (increasing).
    pub id: i64,
    /// Time, Unix ms.
    pub at: i64,
    /// Client id.
    pub client_id: String,
    /// The client's name at the time.
    pub client_name: String,
    /// Tool name.
    pub tool: String,
    /// Space concerned.
    pub space_id: Option<SpaceId>,
    /// Page concerned.
    pub page_id: Option<PageId>,
    /// The query or the page title.
    pub summary: String,
    /// Outcome.
    pub outcome: McpOutcome,
}

/// Queries of the MCP tables, available on a connection or inside a transaction. On a [`Db`] every
/// method is atomic by itself.
pub trait McpQueries {
    /// Adds a client. A duplicate id or token hash is an error and changes nothing.
    fn mcp_client_insert(&self, row: &McpClientRow) -> Result<(), StorageError>;
    /// One client by id.
    fn mcp_client_get(&self, id: &str) -> Result<Option<McpClientRow>, StorageError>;
    /// The client whose token has this SHA-256 (the lookup of every call).
    fn mcp_client_by_token_hash(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<McpClientRow>, StorageError>;
    /// Every client, oldest first (creation time, then insertion order).
    fn mcp_clients_list(&self) -> Result<Vec<McpClientRow>, StorageError>;
    /// Replaces a client's grants. `false`: no such client.
    fn mcp_client_grants_set(&self, id: &str, grants: &[McpGrant]) -> Result<bool, StorageError>;
    /// Sets a client's last use. `false`: no such client.
    fn mcp_client_touch(&self, id: &str, at: i64) -> Result<bool, StorageError>;
    /// Deletes a client (its audit rows stay). `false`: no such client.
    fn mcp_client_delete(&self, id: &str) -> Result<bool, StorageError>;
    /// Appends an audit row, answers its id and deletes the rows beyond the newest
    /// [`MCP_AUDIT_KEEP`], as one atomic unit.
    fn mcp_audit_insert(&self, entry: &NewMcpAudit) -> Result<i64, StorageError>;
    /// Audit rows, newest first: only a client's when `client_id` is given, only rows with an id
    /// below `before` when it is given, at most `limit`.
    fn mcp_audit_list(
        &self,
        client_id: Option<&str>,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<McpAuditRow>, StorageError>;
    /// How many audit rows there are.
    fn mcp_audit_count(&self) -> Result<u64, StorageError>;
    /// Deletes every audit row and answers how many there were.
    fn mcp_audit_clear(&self) -> Result<u64, StorageError>;
}

impl McpQueries for Db {
    fn mcp_client_insert(&self, row: &McpClientRow) -> Result<(), StorageError> {
        mcp_client_insert_on(&self.conn, row)
    }
    fn mcp_client_get(&self, id: &str) -> Result<Option<McpClientRow>, StorageError> {
        mcp_client_get_on(&self.conn, id)
    }
    fn mcp_client_by_token_hash(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<McpClientRow>, StorageError> {
        mcp_client_by_token_hash_on(&self.conn, token_hash)
    }
    fn mcp_clients_list(&self) -> Result<Vec<McpClientRow>, StorageError> {
        mcp_clients_list_on(&self.conn)
    }
    fn mcp_client_grants_set(&self, id: &str, grants: &[McpGrant]) -> Result<bool, StorageError> {
        mcp_client_grants_set_on(&self.conn, id, grants)
    }
    fn mcp_client_touch(&self, id: &str, at: i64) -> Result<bool, StorageError> {
        mcp_client_touch_on(&self.conn, id, at)
    }
    fn mcp_client_delete(&self, id: &str) -> Result<bool, StorageError> {
        mcp_client_delete_on(&self.conn, id)
    }
    fn mcp_audit_insert(&self, entry: &NewMcpAudit) -> Result<i64, StorageError> {
        atomically(&self.conn, |conn| mcp_audit_insert_on(conn, entry))
    }
    fn mcp_audit_list(
        &self,
        client_id: Option<&str>,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<McpAuditRow>, StorageError> {
        mcp_audit_list_on(&self.conn, client_id, before, limit)
    }
    fn mcp_audit_count(&self) -> Result<u64, StorageError> {
        mcp_audit_count_on(&self.conn)
    }
    fn mcp_audit_clear(&self) -> Result<u64, StorageError> {
        mcp_audit_clear_on(&self.conn)
    }
}

impl McpQueries for Tx<'_> {
    fn mcp_client_insert(&self, row: &McpClientRow) -> Result<(), StorageError> {
        mcp_client_insert_on(&self.tx, row)
    }
    fn mcp_client_get(&self, id: &str) -> Result<Option<McpClientRow>, StorageError> {
        mcp_client_get_on(&self.tx, id)
    }
    fn mcp_client_by_token_hash(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<McpClientRow>, StorageError> {
        mcp_client_by_token_hash_on(&self.tx, token_hash)
    }
    fn mcp_clients_list(&self) -> Result<Vec<McpClientRow>, StorageError> {
        mcp_clients_list_on(&self.tx)
    }
    fn mcp_client_grants_set(&self, id: &str, grants: &[McpGrant]) -> Result<bool, StorageError> {
        mcp_client_grants_set_on(&self.tx, id, grants)
    }
    fn mcp_client_touch(&self, id: &str, at: i64) -> Result<bool, StorageError> {
        mcp_client_touch_on(&self.tx, id, at)
    }
    fn mcp_client_delete(&self, id: &str) -> Result<bool, StorageError> {
        mcp_client_delete_on(&self.tx, id)
    }
    fn mcp_audit_insert(&self, entry: &NewMcpAudit) -> Result<i64, StorageError> {
        mcp_audit_insert_on(&self.tx, entry)
    }
    fn mcp_audit_list(
        &self,
        client_id: Option<&str>,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<McpAuditRow>, StorageError> {
        mcp_audit_list_on(&self.tx, client_id, before, limit)
    }
    fn mcp_audit_count(&self) -> Result<u64, StorageError> {
        mcp_audit_count_on(&self.tx)
    }
    fn mcp_audit_clear(&self) -> Result<u64, StorageError> {
        mcp_audit_clear_on(&self.tx)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// The columns of `mcp_clients` in the order [`ClientColumns::read`] expects.
const CLIENT_COLUMNS: &str = "id, name, token_hash, grants, created_at, last_used_at";

/// Newest first, through the primary key. The columns are the order [`AuditColumns::read`] expects.
const AUDIT_LIST_SQL: &str = "SELECT id, at, client_id, client_name, tool, space_id, page_id, \
     summary, outcome FROM mcp_audit WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2";

/// Newest first, through `mcp_audit_client`.
const AUDIT_LIST_OF_CLIENT_SQL: &str = "SELECT id, at, client_id, client_name, tool, space_id, \
     page_id, summary, outcome FROM mcp_audit WHERE client_id = ?1 AND (?2 IS NULL OR id < ?2) \
     ORDER BY id DESC LIMIT ?3";

/// Deletes everything older than the newest `?1` rows: the id of the row at that offset (counted
/// from the newest) and below. No such row (fewer rows than the offset): nothing is deleted.
const AUDIT_PRUNE_SQL: &str = "DELETE FROM mcp_audit WHERE id <= \
     (SELECT id FROM mcp_audit ORDER BY id DESC LIMIT 1 OFFSET ?1)";

/// Raw column values of an `mcp_clients` row.
struct ClientColumns {
    id: String,
    name: String,
    token_hash: Vec<u8>,
    grants: String,
    created_at: i64,
    last_used_at: Option<i64>,
}

impl ClientColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            token_hash: row.get(2)?,
            grants: row.get(3)?,
            created_at: row.get(4)?,
            last_used_at: row.get(5)?,
        })
    }

    fn decode(self) -> Result<McpClientRow, StorageError> {
        let token_hash = <[u8; 32]>::try_from(self.token_hash.as_slice()).map_err(|_| {
            StorageError::InvalidData("the token hash of an MCP client is not 32 bytes long".into())
        })?;
        let grants = serde_json::from_str(&self.grants).map_err(|_| {
            StorageError::InvalidData("the grants of an MCP client are not valid".into())
        })?;
        Ok(McpClientRow {
            id: self.id,
            name: self.name,
            token_hash,
            grants,
            created_at: self.created_at,
            last_used_at: self.last_used_at,
        })
    }
}

/// Raw column values of an `mcp_audit` row.
struct AuditColumns {
    id: i64,
    at: i64,
    client_id: String,
    client_name: String,
    tool: String,
    space_id: Option<Vec<u8>>,
    page_id: Option<Vec<u8>>,
    summary: String,
    outcome: String,
}

impl AuditColumns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            at: row.get(1)?,
            client_id: row.get(2)?,
            client_name: row.get(3)?,
            tool: row.get(4)?,
            space_id: row.get(5)?,
            page_id: row.get(6)?,
            summary: row.get(7)?,
            outcome: row.get(8)?,
        })
    }

    fn decode(self) -> Result<McpAuditRow, StorageError> {
        Ok(McpAuditRow {
            id: self.id,
            at: self.at,
            client_id: self.client_id,
            client_name: self.client_name,
            tool: self.tool,
            space_id: self
                .space_id
                .map(|bytes| id_from(&bytes, "audit space id").map(SpaceId::from_bytes))
                .transpose()?,
            page_id: self
                .page_id
                .map(|bytes| id_from(&bytes, "audit page id").map(PageId::from_bytes))
                .transpose()?,
            summary: self.summary,
            outcome: outcome_from(&self.outcome)?,
        })
    }
}

const fn outcome_str(outcome: McpOutcome) -> &'static str {
    match outcome {
        McpOutcome::Ok => "ok",
        McpOutcome::Denied => "denied",
        McpOutcome::Invalid => "invalid",
        McpOutcome::Error => "error",
    }
}

fn outcome_from(text: &str) -> Result<McpOutcome, StorageError> {
    match text {
        "ok" => Ok(McpOutcome::Ok),
        "denied" => Ok(McpOutcome::Denied),
        "invalid" => Ok(McpOutcome::Invalid),
        "error" => Ok(McpOutcome::Error),
        _ => Err(StorageError::InvalidData(
            "the outcome of an MCP audit row is unknown".into(),
        )),
    }
}

fn grants_json(grants: &[McpGrant]) -> Result<String, StorageError> {
    serde_json::to_string(grants)
        .map_err(|_| StorageError::InvalidData("the grants cannot be written".into()))
}

fn mcp_client_insert_on(conn: &Connection, row: &McpClientRow) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO mcp_clients (id, name, token_hash, grants, created_at, last_used_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.id,
            row.name,
            row.token_hash.as_slice(),
            grants_json(&row.grants)?,
            row.created_at,
            row.last_used_at,
        ],
    )?;
    Ok(())
}

fn mcp_client_get_on(conn: &Connection, id: &str) -> Result<Option<McpClientRow>, StorageError> {
    conn.query_row(
        &format!("SELECT {CLIENT_COLUMNS} FROM mcp_clients WHERE id = ?1"),
        [id],
        ClientColumns::read,
    )
    .optional()?
    .map(ClientColumns::decode)
    .transpose()
}

fn mcp_client_by_token_hash_on(
    conn: &Connection,
    token_hash: &[u8; 32],
) -> Result<Option<McpClientRow>, StorageError> {
    conn.query_row(
        &format!("SELECT {CLIENT_COLUMNS} FROM mcp_clients WHERE token_hash = ?1"),
        [token_hash.as_slice()],
        ClientColumns::read,
    )
    .optional()?
    .map(ClientColumns::decode)
    .transpose()
}

fn mcp_clients_list_on(conn: &Connection) -> Result<Vec<McpClientRow>, StorageError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CLIENT_COLUMNS} FROM mcp_clients ORDER BY created_at, rowid"
    ))?;
    let rows = stmt
        .query_map([], ClientColumns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(ClientColumns::decode).collect()
}

fn mcp_client_grants_set_on(
    conn: &Connection,
    id: &str,
    grants: &[McpGrant],
) -> Result<bool, StorageError> {
    let changed = conn.execute(
        "UPDATE mcp_clients SET grants = ?2 WHERE id = ?1",
        params![id, grants_json(grants)?],
    )?;
    Ok(changed > 0)
}

fn mcp_client_touch_on(conn: &Connection, id: &str, at: i64) -> Result<bool, StorageError> {
    let changed = conn.execute(
        "UPDATE mcp_clients SET last_used_at = ?2 WHERE id = ?1",
        params![id, at],
    )?;
    Ok(changed > 0)
}

fn mcp_client_delete_on(conn: &Connection, id: &str) -> Result<bool, StorageError> {
    let changed = conn.execute("DELETE FROM mcp_clients WHERE id = ?1", [id])?;
    Ok(changed > 0)
}

fn mcp_audit_insert_on(conn: &Connection, entry: &NewMcpAudit) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO mcp_audit (at, client_id, client_name, tool, space_id, page_id, summary, \
         outcome) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            entry.at,
            entry.client_id,
            entry.client_name,
            entry.tool,
            entry.space_id.map(SpaceId::to_bytes),
            entry.page_id.map(PageId::to_bytes),
            entry.summary,
            outcome_str(entry.outcome),
        ],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(AUDIT_PRUNE_SQL, [i64::from(MCP_AUDIT_KEEP)])?;
    Ok(id)
}

fn mcp_audit_list_on(
    conn: &Connection,
    client_id: Option<&str>,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<McpAuditRow>, StorageError> {
    let limit = i64::from(limit);
    let rows = match client_id {
        Some(client_id) => conn
            .prepare(AUDIT_LIST_OF_CLIENT_SQL)?
            .query_map(params![client_id, before, limit], AuditColumns::read)?
            .collect::<Result<Vec<_>, _>>()?,
        None => conn
            .prepare(AUDIT_LIST_SQL)?
            .query_map(params![before, limit], AuditColumns::read)?
            .collect::<Result<Vec<_>, _>>()?,
    };
    rows.into_iter().map(AuditColumns::decode).collect()
}

fn mcp_audit_count_on(conn: &Connection) -> Result<u64, StorageError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM mcp_audit", [], |row| row.get(0))?;
    u64::try_from(count)
        .map_err(|_| StorageError::InvalidData("the audit row count is out of range".into()))
}

fn mcp_audit_clear_on(conn: &Connection) -> Result<u64, StorageError> {
    let deleted = conn.execute("DELETE FROM mcp_audit", [])?;
    Ok(deleted as u64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    const fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    const fn page(n: u8) -> PageId {
        PageId::from_bytes([n; 16])
    }

    fn client(n: u8, created_at: i64) -> McpClientRow {
        McpClientRow {
            id: format!("{n:02x}").repeat(16),
            name: format!("client {n}"),
            token_hash: [n; 32],
            grants: Vec::new(),
            created_at,
            last_used_at: None,
        }
    }

    fn grant(n: u8, read: bool, write: bool) -> McpGrant {
        McpGrant {
            space_id: space(n),
            read,
            write,
        }
    }

    fn audit(client: &McpClientRow, at: i64, tool: &str, outcome: McpOutcome) -> NewMcpAudit {
        NewMcpAudit {
            at,
            client_id: client.id.clone(),
            client_name: client.name.clone(),
            tool: tool.to_owned(),
            space_id: None,
            page_id: None,
            summary: String::new(),
            outcome,
        }
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that both
    /// `McpQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn McpQueries, &Connection)) {
        let db = new_db();
        scenario(&db, db.connection());
        let mut db = new_db();
        db.transaction(|tx| {
            scenario(tx, &tx.tx);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_client_roundtrips_by_id_and_by_token_hash() {
        on_db_and_tx(|q, _| {
            let mut row = client(1, 1_000);
            row.grants = vec![grant(7, true, false), grant(8, true, true)];
            row.last_used_at = Some(2_000);
            q.mcp_client_insert(&row).unwrap();
            assert_eq!(q.mcp_client_get(&row.id).unwrap(), Some(row.clone()));
            assert_eq!(
                q.mcp_client_by_token_hash(&row.token_hash).unwrap(),
                Some(row)
            );
            assert_eq!(q.mcp_client_get(&"ff".repeat(16)).unwrap(), None);
            assert_eq!(q.mcp_client_by_token_hash(&[9; 32]).unwrap(), None);
        });
    }

    #[test]
    fn clients_list_oldest_first_and_ties_keep_the_insertion_order() {
        on_db_and_tx(|q, _| {
            for (n, created_at) in [(3, 300), (1, 100), (2, 100), (4, 50)] {
                q.mcp_client_insert(&client(n, created_at)).unwrap();
            }
            let order: Vec<String> = q
                .mcp_clients_list()
                .unwrap()
                .into_iter()
                .map(|c| c.name)
                .collect();
            assert_eq!(order, ["client 4", "client 1", "client 2", "client 3"]);
        });
    }

    #[test]
    fn a_duplicate_id_or_token_hash_is_refused_and_changes_nothing() {
        on_db_and_tx(|q, _| {
            let first = client(1, 1);
            q.mcp_client_insert(&first).unwrap();
            let mut same_id = client(2, 2);
            same_id.id.clone_from(&first.id);
            assert!(q.mcp_client_insert(&same_id).is_err());
            let mut same_hash = client(3, 3);
            same_hash.token_hash = first.token_hash;
            assert!(q.mcp_client_insert(&same_hash).is_err());
            assert_eq!(q.mcp_clients_list().unwrap(), [first]);
        });
    }

    #[test]
    fn grants_are_replaced_as_json_and_an_unknown_client_is_reported() {
        on_db_and_tx(|q, conn| {
            let row = client(1, 1);
            q.mcp_client_insert(&row).unwrap();
            let grants = [grant(5, true, false), grant(6, true, true)];
            assert!(q.mcp_client_grants_set(&row.id, &grants).unwrap());
            assert_eq!(q.mcp_client_get(&row.id).unwrap().unwrap().grants, grants);
            // The column holds a JSON array of `{spaceId, read, write}`.
            let raw: String = conn
                .query_row("SELECT grants FROM mcp_clients", [], |r| r.get(0))
                .unwrap();
            let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(json[0]["read"], true);
            assert_eq!(json[1]["write"], true);
            assert_eq!(json[0]["spaceId"], space(5).to_string());
            assert!(q.mcp_client_grants_set(&row.id, &[]).unwrap());
            assert!(
                q.mcp_client_get(&row.id)
                    .unwrap()
                    .unwrap()
                    .grants
                    .is_empty()
            );
            assert!(!q.mcp_client_grants_set(&"ee".repeat(16), &grants).unwrap());
        });
    }

    #[test]
    fn touch_sets_the_last_use_and_delete_removes_the_client_but_not_its_audit_rows() {
        on_db_and_tx(|q, _| {
            let row = client(1, 1);
            q.mcp_client_insert(&row).unwrap();
            assert!(q.mcp_client_touch(&row.id, 5_000).unwrap());
            assert_eq!(
                q.mcp_client_get(&row.id).unwrap().unwrap().last_used_at,
                Some(5_000)
            );
            assert!(!q.mcp_client_touch(&"ee".repeat(16), 1).unwrap());
            q.mcp_audit_insert(&audit(&row, 10, "search", McpOutcome::Ok))
                .unwrap();
            assert!(q.mcp_client_delete(&row.id).unwrap());
            assert!(!q.mcp_client_delete(&row.id).unwrap());
            assert_eq!(q.mcp_client_get(&row.id).unwrap(), None);
            assert_eq!(q.mcp_client_by_token_hash(&row.token_hash).unwrap(), None);
            let rows = q.mcp_audit_list(None, None, 10).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].client_name, "client 1");
        });
    }

    #[test]
    fn audit_rows_roundtrip_every_field_and_outcome() {
        on_db_and_tx(|q, _| {
            let c = client(1, 1);
            let outcomes = [
                McpOutcome::Ok,
                McpOutcome::Denied,
                McpOutcome::Invalid,
                McpOutcome::Error,
            ];
            let mut ids = Vec::new();
            for (n, outcome) in outcomes.into_iter().enumerate() {
                let mut entry = audit(&c, 100 + n as i64, "read_page", outcome);
                entry.space_id = Some(space(n as u8 + 1));
                entry.page_id = Some(page(n as u8 + 10));
                entry.summary = format!("summary {n} é");
                ids.push(q.mcp_audit_insert(&entry).unwrap());
            }
            assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids increase: {ids:?}");
            let rows = q.mcp_audit_list(None, None, 10).unwrap();
            assert_eq!(rows.len(), 4);
            // Newest first.
            assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), {
                let mut newest_first = ids.clone();
                newest_first.reverse();
                newest_first
            });
            let last = &rows[0];
            assert_eq!(last.outcome, McpOutcome::Error);
            assert_eq!(last.at, 103);
            assert_eq!(last.client_id, c.id);
            assert_eq!(last.client_name, "client 1");
            assert_eq!(last.tool, "read_page");
            assert_eq!(last.space_id, Some(space(4)));
            assert_eq!(last.page_id, Some(page(13)));
            assert_eq!(last.summary, "summary 3 é");
            assert_eq!(rows[3].outcome, McpOutcome::Ok);
            // No space, no page.
            let id = q
                .mcp_audit_insert(&audit(&c, 200, "list_spaces", McpOutcome::Ok))
                .unwrap();
            let newest = &q.mcp_audit_list(None, None, 1).unwrap()[0];
            assert_eq!(
                (newest.id, newest.space_id, newest.page_id),
                (id, None, None)
            );
        });
    }

    #[test]
    fn audit_lists_filter_by_client_page_by_id_and_limit() {
        on_db_and_tx(|q, _| {
            let (a, b) = (client(1, 1), client(2, 2));
            let mut ids = Vec::new();
            for n in 0..10 {
                let who = if n % 3 == 0 { &b } else { &a };
                ids.push(
                    q.mcp_audit_insert(&audit(who, n, "search", McpOutcome::Ok))
                        .unwrap(),
                );
            }
            let of_b = q.mcp_audit_list(Some(&b.id), None, 100).unwrap();
            assert_eq!(
                of_b.iter().map(|r| r.at).collect::<Vec<_>>(),
                [9, 6, 3, 0],
                "b made calls 0, 3, 6 and 9"
            );
            assert!(of_b.iter().all(|r| r.client_id == b.id));
            assert!(
                q.mcp_audit_list(Some(&"ee".repeat(16)), None, 100)
                    .unwrap()
                    .is_empty()
            );
            // Paging: `before` is exclusive, and the limit applies after the filters.
            let page_one = q.mcp_audit_list(None, None, 4).unwrap();
            assert_eq!(
                page_one.iter().map(|r| r.at).collect::<Vec<_>>(),
                [9, 8, 7, 6]
            );
            let page_two = q.mcp_audit_list(None, Some(page_one[3].id), 4).unwrap();
            assert_eq!(
                page_two.iter().map(|r| r.at).collect::<Vec<_>>(),
                [5, 4, 3, 2]
            );
            let b_page = q.mcp_audit_list(Some(&b.id), Some(ids[6]), 1).unwrap();
            assert_eq!(b_page.iter().map(|r| r.at).collect::<Vec<_>>(), [3]);
            assert!(q.mcp_audit_list(None, Some(ids[0]), 10).unwrap().is_empty());
            assert!(q.mcp_audit_list(None, None, 0).unwrap().is_empty());
        });
    }

    #[test]
    fn the_audit_log_keeps_the_newest_ten_thousand_rows() {
        on_db_and_tx(|q, _| {
            let c = client(1, 1);
            let total = i64::from(MCP_AUDIT_KEEP) + 7;
            let mut last = 0;
            for n in 0..total {
                last = q
                    .mcp_audit_insert(&audit(&c, n, "search", McpOutcome::Ok))
                    .unwrap();
            }
            assert_eq!(q.mcp_audit_count().unwrap(), u64::from(MCP_AUDIT_KEEP));
            let newest = q.mcp_audit_list(None, None, 1).unwrap();
            assert_eq!((newest[0].id, newest[0].at), (last, total - 1));
            let oldest = q.mcp_audit_list(None, Some(last - 9_999), 5).unwrap();
            assert!(
                oldest.is_empty(),
                "nothing is older than the 10,000th newest"
            );
            let all = q.mcp_audit_list(None, None, 20_000).unwrap();
            assert_eq!(all.len(), MCP_AUDIT_KEEP as usize);
            assert_eq!(all.last().unwrap().at, 7, "the 7 oldest rows are gone");
        });
    }

    #[test]
    fn clear_deletes_the_audit_rows_only() {
        on_db_and_tx(|q, _| {
            let c = client(1, 1);
            q.mcp_client_insert(&c).unwrap();
            for n in 0..3 {
                q.mcp_audit_insert(&audit(&c, n, "search", McpOutcome::Ok))
                    .unwrap();
            }
            assert_eq!(q.mcp_audit_clear().unwrap(), 3);
            assert_eq!(q.mcp_audit_clear().unwrap(), 0);
            assert_eq!(q.mcp_audit_count().unwrap(), 0);
            assert_eq!(q.mcp_clients_list().unwrap().len(), 1);
            // The ids keep working afterwards.
            assert!(
                q.mcp_audit_insert(&audit(&c, 9, "search", McpOutcome::Ok))
                    .is_ok()
            );
        });
    }

    #[test]
    fn ids_and_hashes_are_stored_as_blobs_of_the_right_width() {
        on_db_and_tx(|q, conn| {
            let c = client(1, 1);
            q.mcp_client_insert(&c).unwrap();
            let mut entry = audit(&c, 1, "read_page", McpOutcome::Ok);
            entry.space_id = Some(space(2));
            entry.page_id = Some(page(3));
            q.mcp_audit_insert(&entry).unwrap();
            let hash: String = conn
                .query_row(
                    "SELECT typeof(token_hash) || length(token_hash) FROM mcp_clients",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hash, "blob32");
            let widths: (String, String) = conn
                .query_row(
                    "SELECT typeof(space_id) || length(space_id), \
                            typeof(page_id) || length(page_id) FROM mcp_audit",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(widths, ("blob16".to_owned(), "blob16".to_owned()));
        });
    }

    #[test]
    fn damaged_rows_are_typed_errors_and_never_a_panic() {
        let db = new_db();
        let conn = db.connection();
        let invalid = |result: Result<(), StorageError>| {
            assert!(
                matches!(result, Err(StorageError::InvalidData(_))),
                "{result:?}"
            );
        };
        // A token hash of the wrong width.
        conn.execute(
            "INSERT INTO mcp_clients (id, name, token_hash, grants, created_at) \
             VALUES ('a', 'x', x'0102', '[]', 1)",
            [],
        )
        .unwrap();
        invalid(db.mcp_client_get("a").map(|_| ()));
        invalid(db.mcp_clients_list().map(|_| ()));
        conn.execute("DELETE FROM mcp_clients", []).unwrap();
        // Grants that are not JSON, or not grants.
        for bad in ["not json", "{}", "[{\"spaceId\":\"nope\"}]"] {
            conn.execute(
                "INSERT INTO mcp_clients (id, name, token_hash, grants, created_at) \
                 VALUES ('b', 'x', zeroblob(32), ?1, 1)",
                [bad],
            )
            .unwrap();
            invalid(db.mcp_client_get("b").map(|_| ()));
            conn.execute("DELETE FROM mcp_clients", []).unwrap();
        }
        // An unknown outcome, an id of the wrong width.
        conn.execute(
            "INSERT INTO mcp_audit (at, client_id, client_name, tool, summary, outcome) \
             VALUES (1, 'a', 'x', 't', '', 'strange')",
            [],
        )
        .unwrap();
        invalid(db.mcp_audit_list(None, None, 10).map(|_| ()));
        conn.execute("DELETE FROM mcp_audit", []).unwrap();
        conn.execute(
            "INSERT INTO mcp_audit (at, client_id, client_name, tool, space_id, summary, outcome) \
             VALUES (1, 'a', 'x', 't', x'01', '', 'ok')",
            [],
        )
        .unwrap();
        invalid(db.mcp_audit_list(None, None, 10).map(|_| ()));
    }
}
