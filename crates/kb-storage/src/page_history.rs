//! The page history journal (schema v21, ADR-0057, `docs/architecture/page-history.md` section 3): one row per
//! editing session of a page document that this device witnessed, with when it began and ended here, the document
//! version it ended at, who made it and the page's title then. Dates and authors exist nowhere else (the documents
//! record neither), so the table is **not derived**: `IndexRebuild` does not clear it. It is device-local (never
//! synced, not part of a backup) and follows its document: the rows go with the `docs` row (cascade) and are deleted by
//! name when a page is purged.
//!
//! Same conventions as [`crate::page_first_images`]: every method exists on [`Db`] and [`Tx`] through
//! [`PageHistoryQueries`], both running the same private functions over a connection, so a method behaves identically
//! in either place; the multi-statement methods of a [`Db`] run as one atomic unit. Ids are bound as 16-byte blobs.
//! Rows are read as raw column values first and validated afterwards, so a damaged row is a typed `InvalidData` error
//! and never a panic.
//!
//! # The table
//!
//! `page_history_journal(doc_id, seq, kind, started_at, ended_at, base_version, version, device_id, account_id,
//! this_device, title)`, primary key `(doc_id, seq)`; `seq` counts 1, 2, 3, ... per document. `version` and
//! `base_version` are version ids as `kb-doc` writes them (text this crate never interprets): the document version at
//! the end of the session and before its first write.

use kb_types::{AccountId, DeviceId, DocId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// What made a journal row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalKind {
    /// Editing (in the editor, or by the core: an import, a template, MCP) or an applied sync entry.
    Edit,
    /// A restore of an earlier version.
    Restore,
}

impl JournalKind {
    const fn text(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Restore => "restore",
        }
    }

    fn from_text(text: &str) -> Result<Self, StorageError> {
        match text {
            "edit" => Ok(Self::Edit),
            "restore" => Ok(Self::Restore),
            other => Err(StorageError::InvalidData(format!(
                "unknown journal kind {other:?}"
            ))),
        }
    }
}

/// A journal row to append (the store gives it its `seq`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJournalRow {
    /// The page's document.
    pub doc_id: DocId,
    /// What made it.
    pub kind: JournalKind,
    /// When the session began on this device, Unix ms.
    pub started_at: i64,
    /// When the session's last change was seen here, Unix ms.
    pub ended_at: i64,
    /// The document version before the session's first write (empty for a document that began with it).
    pub base_version: String,
    /// The document version at the end of the session.
    pub version: String,
    /// The device that made it, when known.
    pub device_id: Option<DeviceId>,
    /// The account that made it, when known.
    pub account_id: Option<AccountId>,
    /// This device made it.
    pub this_device: bool,
    /// The page's title at the end of the session, when recorded.
    pub title: Option<String>,
}

/// A journal row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRow {
    /// The page's document.
    pub doc_id: DocId,
    /// 1, 2, 3, ... per document.
    pub seq: i64,
    /// What made it.
    pub kind: JournalKind,
    /// When the session began on this device, Unix ms.
    pub started_at: i64,
    /// When the session's last change was seen here, Unix ms.
    pub ended_at: i64,
    /// The document version before the session's first write.
    pub base_version: String,
    /// The document version at the end of the session.
    pub version: String,
    /// The device that made it, when known.
    pub device_id: Option<DeviceId>,
    /// The account that made it, when known.
    pub account_id: Option<AccountId>,
    /// This device made it.
    pub this_device: bool,
    /// The page's title at the end of the session, when recorded.
    pub title: Option<String>,
}

/// One write of the journal, decided by the core before the storage transaction it rides in (the read of the
/// latest row and the title are not made inside the transaction): a row to append or a row to continue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalOp {
    /// A new row, the newest of its document.
    Append(NewJournalRow),
    /// The session of an existing row goes on.
    Extend {
        /// The document.
        doc_id: DocId,
        /// The row.
        seq: i64,
        /// The new end of the session, Unix ms.
        ended_at: i64,
        /// The document version at the new end.
        version: String,
        /// The page's title now.
        title: Option<String>,
    },
}

/// Queries of the `page_history_journal` table, available on a connection or inside a transaction. On a [`Db`] every
/// method is atomic by itself.
pub trait PageHistoryQueries {
    /// Appends a row as the newest of its document and answers its `seq` (one more than the document's highest, 1 for
    /// the first). The document must exist (`docs` row), else the foreign key refuses it.
    fn journal_append(&self, row: &NewJournalRow) -> Result<i64, StorageError>;
    /// Continues the row `seq` of a document (a session that goes on): sets its `ended_at`, `version` and `title`.
    /// `false` when the row does not exist. The start, the base, the author and the kind stay.
    fn journal_extend(
        &self,
        doc_id: DocId,
        seq: i64,
        ended_at: i64,
        version: &str,
        title: Option<&str>,
    ) -> Result<bool, StorageError>;
    /// The newest row of a document (the highest `seq`).
    fn journal_latest(&self, doc_id: DocId) -> Result<Option<JournalRow>, StorageError>;
    /// The rows of a document, newest first. `before_seq` keeps only the rows older than that one (paging: pass the
    /// last `seq` of the previous page), `limit` the first that many.
    fn journal_list(
        &self,
        doc_id: DocId,
        before_seq: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<JournalRow>, StorageError>;
    /// The newest row of a document whose `version` is this one.
    fn journal_find_version(
        &self,
        doc_id: DocId,
        version: &str,
    ) -> Result<Option<JournalRow>, StorageError>;
    /// Removes every row of the given documents (purge). Unknown documents and an empty list succeed. One atomic unit.
    fn journal_delete_docs(&self, docs: &[DocId]) -> Result<(), StorageError>;
    /// Runs one [`JournalOp`] inside a SAVEPOINT: when it fails (the document is gone, a damaged row), only the
    /// journal statements are rolled back and the error is answered, so a caller that rides the journal in the
    /// transaction of the write it records can log it and let that write commit. One statement, no commit of its own
    /// inside a transaction.
    fn journal_record(&self, op: &JournalOp) -> Result<(), StorageError>;
}

impl PageHistoryQueries for Db {
    fn journal_append(&self, row: &NewJournalRow) -> Result<i64, StorageError> {
        journal_append_on(&self.conn, row)
    }
    fn journal_extend(
        &self,
        doc_id: DocId,
        seq: i64,
        ended_at: i64,
        version: &str,
        title: Option<&str>,
    ) -> Result<bool, StorageError> {
        journal_extend_on(&self.conn, doc_id, seq, ended_at, version, title)
    }
    fn journal_latest(&self, doc_id: DocId) -> Result<Option<JournalRow>, StorageError> {
        journal_latest_on(&self.conn, doc_id)
    }
    fn journal_list(
        &self,
        doc_id: DocId,
        before_seq: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<JournalRow>, StorageError> {
        journal_list_on(&self.conn, doc_id, before_seq, limit)
    }
    fn journal_find_version(
        &self,
        doc_id: DocId,
        version: &str,
    ) -> Result<Option<JournalRow>, StorageError> {
        journal_find_version_on(&self.conn, doc_id, version)
    }
    fn journal_delete_docs(&self, docs: &[DocId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| journal_delete_docs_on(conn, docs))
    }
    fn journal_record(&self, op: &JournalOp) -> Result<(), StorageError> {
        journal_record_on(&self.conn, op)
    }
}

impl PageHistoryQueries for Tx<'_> {
    fn journal_append(&self, row: &NewJournalRow) -> Result<i64, StorageError> {
        journal_append_on(&self.tx, row)
    }
    fn journal_extend(
        &self,
        doc_id: DocId,
        seq: i64,
        ended_at: i64,
        version: &str,
        title: Option<&str>,
    ) -> Result<bool, StorageError> {
        journal_extend_on(&self.tx, doc_id, seq, ended_at, version, title)
    }
    fn journal_latest(&self, doc_id: DocId) -> Result<Option<JournalRow>, StorageError> {
        journal_latest_on(&self.tx, doc_id)
    }
    fn journal_list(
        &self,
        doc_id: DocId,
        before_seq: Option<i64>,
        limit: Option<u32>,
    ) -> Result<Vec<JournalRow>, StorageError> {
        journal_list_on(&self.tx, doc_id, before_seq, limit)
    }
    fn journal_find_version(
        &self,
        doc_id: DocId,
        version: &str,
    ) -> Result<Option<JournalRow>, StorageError> {
        journal_find_version_on(&self.tx, doc_id, version)
    }
    fn journal_delete_docs(&self, docs: &[DocId]) -> Result<(), StorageError> {
        journal_delete_docs_on(&self.tx, docs)
    }
    fn journal_record(&self, op: &JournalOp) -> Result<(), StorageError> {
        journal_record_on(&self.tx, op)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// One statement computes the next number and inserts, so it is atomic even outside a transaction.
const APPEND_SQL: &str = "INSERT INTO page_history_journal \
     (doc_id, seq, kind, started_at, ended_at, base_version, version, device_id, account_id, this_device, title) \
     SELECT ?1, COALESCE(MAX(seq), 0) + 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
     FROM page_history_journal WHERE doc_id = ?1 \
     RETURNING seq";

/// Through the primary key.
const EXTEND_SQL: &str = "UPDATE page_history_journal SET ended_at = ?3, version = ?4, title = ?5 \
     WHERE doc_id = ?1 AND seq = ?2";

/// Through the primary key, the newest first.
const LATEST_SQL: &str = "SELECT doc_id, seq, kind, started_at, ended_at, base_version, version, device_id, \
     account_id, this_device, title FROM page_history_journal WHERE doc_id = ?1 ORDER BY seq DESC LIMIT 1";

/// Through the primary key; `?2` is the exclusive upper bound of `seq` and `?3` the row limit (both may be open).
const LIST_SQL: &str = "SELECT doc_id, seq, kind, started_at, ended_at, base_version, version, device_id, \
     account_id, this_device, title FROM page_history_journal \
     WHERE doc_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3";

/// Through the primary key, the newest first.
const FIND_SQL: &str = "SELECT doc_id, seq, kind, started_at, ended_at, base_version, version, device_id, \
     account_id, this_device, title FROM page_history_journal \
     WHERE doc_id = ?1 AND version = ?2 ORDER BY seq DESC LIMIT 1";

/// Through the primary key (the document).
const DELETE_SQL: &str = "DELETE FROM page_history_journal WHERE doc_id = ?1";

/// Raw column values of a row.
struct Columns {
    doc_id: Vec<u8>,
    seq: i64,
    kind: String,
    started_at: i64,
    ended_at: i64,
    base_version: String,
    version: String,
    device_id: Option<Vec<u8>>,
    account_id: Option<Vec<u8>>,
    this_device: i64,
    title: Option<String>,
}

impl Columns {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            doc_id: row.get(0)?,
            seq: row.get(1)?,
            kind: row.get(2)?,
            started_at: row.get(3)?,
            ended_at: row.get(4)?,
            base_version: row.get(5)?,
            version: row.get(6)?,
            device_id: row.get(7)?,
            account_id: row.get(8)?,
            this_device: row.get(9)?,
            title: row.get(10)?,
        })
    }

    fn decode(self) -> Result<JournalRow, StorageError> {
        Ok(JournalRow {
            doc_id: DocId::from_bytes(id_from(&self.doc_id, "journal document id")?),
            seq: self.seq,
            kind: JournalKind::from_text(&self.kind)?,
            started_at: self.started_at,
            ended_at: self.ended_at,
            base_version: self.base_version,
            version: self.version,
            device_id: self
                .device_id
                .map(|bytes| id_from(&bytes, "journal device id").map(DeviceId::from_bytes))
                .transpose()?,
            account_id: self
                .account_id
                .map(|bytes| id_from(&bytes, "journal account id").map(AccountId::from_bytes))
                .transpose()?,
            this_device: self.this_device != 0,
            title: self.title,
        })
    }
}

fn journal_append_on(conn: &Connection, row: &NewJournalRow) -> Result<i64, StorageError> {
    let seq = conn.query_one(
        APPEND_SQL,
        params![
            row.doc_id.to_bytes(),
            row.kind.text(),
            row.started_at,
            row.ended_at,
            row.base_version,
            row.version,
            row.device_id.map(DeviceId::to_bytes),
            row.account_id.map(AccountId::to_bytes),
            i64::from(row.this_device),
            row.title,
        ],
        |r| r.get::<_, i64>(0),
    )?;
    Ok(seq)
}

fn journal_extend_on(
    conn: &Connection,
    doc_id: DocId,
    seq: i64,
    ended_at: i64,
    version: &str,
    title: Option<&str>,
) -> Result<bool, StorageError> {
    let changed = conn.execute(
        EXTEND_SQL,
        params![doc_id.to_bytes(), seq, ended_at, version, title],
    )?;
    Ok(changed > 0)
}

fn journal_latest_on(conn: &Connection, doc_id: DocId) -> Result<Option<JournalRow>, StorageError> {
    conn.query_row(LATEST_SQL, [doc_id.to_bytes()], Columns::read)
        .optional()?
        .map(Columns::decode)
        .transpose()
}

fn journal_list_on(
    conn: &Connection,
    doc_id: DocId,
    before_seq: Option<i64>,
    limit: Option<u32>,
) -> Result<Vec<JournalRow>, StorageError> {
    // An open bound is `i64::MAX` and an open limit is -1 (no limit in SQLite).
    let before = before_seq.unwrap_or(i64::MAX);
    let limit = limit.map_or(-1, i64::from);
    let mut stmt = conn.prepare(LIST_SQL)?;
    let raw = stmt
        .query_map(params![doc_id.to_bytes(), before, limit], Columns::read)?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter().map(Columns::decode).collect()
}

fn journal_find_version_on(
    conn: &Connection,
    doc_id: DocId,
    version: &str,
) -> Result<Option<JournalRow>, StorageError> {
    conn.query_row(FIND_SQL, params![doc_id.to_bytes(), version], Columns::read)
        .optional()?
        .map(Columns::decode)
        .transpose()
}

fn journal_delete_docs_on(conn: &Connection, docs: &[DocId]) -> Result<(), StorageError> {
    let mut delete = conn.prepare(DELETE_SQL)?;
    for doc_id in docs {
        delete.execute([doc_id.to_bytes()])?;
    }
    Ok(())
}

fn journal_record_on(conn: &Connection, op: &JournalOp) -> Result<(), StorageError> {
    conn.execute_batch("SAVEPOINT page_history")?;
    let result = match op {
        JournalOp::Append(row) => journal_append_on(conn, row).map(|_| ()),
        JournalOp::Extend {
            doc_id,
            seq,
            ended_at,
            version,
            title,
        } => {
            journal_extend_on(conn, *doc_id, *seq, *ended_at, version, title.as_deref()).map(|_| ())
        }
    };
    match result {
        Ok(()) => {
            conn.execute_batch("RELEASE page_history")?;
            Ok(())
        }
        Err(err) => {
            // Only the journal's statements are undone; what the caller wrote before stays.
            let _ = conn.execute_batch("ROLLBACK TO page_history; RELEASE page_history");
            Err(err)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    const fn doc(n: u8) -> DocId {
        DocId::from_bytes([n; 16])
    }

    /// Makes the `docs` row the journal's foreign key needs.
    fn make_doc(conn: &Connection, n: u8) {
        conn.execute(
            "INSERT INTO docs (doc_id, kind, space_id, created_at, format_version, min_reader_version) \
             VALUES (?1, 1, NULL, 0, 1, 1)",
            [doc(n).to_bytes()],
        )
        .unwrap();
    }

    fn row(n: u8, version: &str) -> NewJournalRow {
        NewJournalRow {
            doc_id: doc(n),
            kind: JournalKind::Edit,
            started_at: 100,
            ended_at: 100,
            base_version: String::new(),
            version: version.to_owned(),
            device_id: Some(DeviceId::from_bytes([0xd1; 16])),
            account_id: Some(AccountId::from_bytes([0xa1; 16])),
            this_device: true,
            title: Some("Plan".to_owned()),
        }
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that both
    /// `PageHistoryQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn PageHistoryQueries, &Connection)) {
        let db = new_db();
        scenario(&db, db.connection());
        let mut db = new_db();
        db.transaction(|tx| {
            scenario(tx, &tx.tx);
            Ok(())
        })
        .unwrap();
    }

    fn versions(rows: &[JournalRow]) -> Vec<&str> {
        rows.iter().map(|r| r.version.as_str()).collect()
    }

    #[test]
    fn append_numbers_the_rows_of_each_document_from_one() {
        on_db_and_tx(|q, conn| {
            make_doc(conn, 1);
            make_doc(conn, 2);
            assert_eq!(q.journal_append(&row(1, "1@0")).unwrap(), 1);
            assert_eq!(q.journal_append(&row(1, "1@1")).unwrap(), 2);
            assert_eq!(q.journal_append(&row(2, "2@0")).unwrap(), 1);
            assert_eq!(q.journal_append(&row(1, "1@2")).unwrap(), 3);
            let latest = q.journal_latest(doc(1)).unwrap().unwrap();
            assert_eq!((latest.seq, latest.version.as_str()), (3, "1@2"));
            assert_eq!(latest.doc_id, doc(1));
            assert!(q.journal_latest(doc(9)).unwrap().is_none());
        });
    }

    #[test]
    fn a_row_round_trips_with_and_without_author_and_title() {
        on_db_and_tx(|q, conn| {
            make_doc(conn, 1);
            let full = row(1, "1@0,2@3");
            q.journal_append(&full).unwrap();
            let unknown = NewJournalRow {
                kind: JournalKind::Restore,
                base_version: "1@0".to_owned(),
                device_id: None,
                account_id: None,
                this_device: false,
                title: None,
                started_at: 5,
                ended_at: 9,
                ..row(1, "1@4")
            };
            q.journal_append(&unknown).unwrap();
            let rows = q.journal_list(doc(1), None, None).unwrap();
            assert_eq!(rows.len(), 2);
            let (newest, oldest) = (&rows[0], &rows[1]);
            assert_eq!(oldest.seq, 1);
            assert_eq!(oldest.kind, JournalKind::Edit);
            assert_eq!(oldest.version, "1@0,2@3");
            assert_eq!(oldest.base_version, "");
            assert_eq!(oldest.device_id, full.device_id);
            assert_eq!(oldest.account_id, full.account_id);
            assert!(oldest.this_device);
            assert_eq!(oldest.title.as_deref(), Some("Plan"));
            assert_eq!((oldest.started_at, oldest.ended_at), (100, 100));
            assert_eq!(newest.seq, 2);
            assert_eq!(newest.kind, JournalKind::Restore);
            assert_eq!(newest.base_version, "1@0");
            assert_eq!((newest.device_id, newest.account_id), (None, None));
            assert!(!newest.this_device);
            assert_eq!(newest.title, None);
            assert_eq!((newest.started_at, newest.ended_at), (5, 9));
        });
    }

    #[test]
    fn extend_moves_the_end_the_version_and_the_title_of_one_row_only() {
        on_db_and_tx(|q, conn| {
            make_doc(conn, 1);
            q.journal_append(&row(1, "1@0")).unwrap();
            let seq = q.journal_append(&row(1, "1@1")).unwrap();
            assert!(
                q.journal_extend(doc(1), seq, 250, "1@5", Some("Renamed"))
                    .unwrap()
            );
            let rows = q.journal_list(doc(1), None, None).unwrap();
            assert_eq!(versions(&rows), ["1@5", "1@0"]);
            let extended = &rows[0];
            assert_eq!(extended.ended_at, 250);
            assert_eq!(extended.started_at, 100, "the start stays");
            assert_eq!(extended.base_version, "");
            assert_eq!(extended.title.as_deref(), Some("Renamed"));
            assert_eq!(extended.kind, JournalKind::Edit);
            assert_eq!(extended.device_id, Some(DeviceId::from_bytes([0xd1; 16])));
            assert_eq!(rows[1].ended_at, 100, "the other row is untouched");
            // A title can be cleared; a row that is not there is not extended.
            assert!(q.journal_extend(doc(1), seq, 260, "1@6", None).unwrap());
            assert_eq!(q.journal_latest(doc(1)).unwrap().unwrap().title, None);
            assert!(!q.journal_extend(doc(1), 99, 1, "x", None).unwrap());
            assert!(!q.journal_extend(doc(7), 1, 1, "x", None).unwrap());
        });
    }

    #[test]
    fn list_is_newest_first_and_pages_by_seq() {
        on_db_and_tx(|q, conn| {
            make_doc(conn, 1);
            make_doc(conn, 2);
            for n in 1..=5 {
                q.journal_append(&row(1, &format!("1@{n}"))).unwrap();
            }
            q.journal_append(&row(2, "2@0")).unwrap();
            let all = q.journal_list(doc(1), None, None).unwrap();
            assert_eq!(versions(&all), ["1@5", "1@4", "1@3", "1@2", "1@1"]);
            // A page of two, then the next ones from the last seq of the page.
            let first = q.journal_list(doc(1), None, Some(2)).unwrap();
            assert_eq!(versions(&first), ["1@5", "1@4"]);
            let second = q.journal_list(doc(1), Some(first[1].seq), Some(2)).unwrap();
            assert_eq!(versions(&second), ["1@3", "1@2"]);
            let third = q
                .journal_list(doc(1), Some(second[1].seq), Some(2))
                .unwrap();
            assert_eq!(versions(&third), ["1@1"]);
            assert!(
                q.journal_list(doc(1), Some(third[0].seq), Some(2))
                    .unwrap()
                    .is_empty()
            );
            assert!(q.journal_list(doc(1), None, Some(0)).unwrap().is_empty());
            assert!(q.journal_list(doc(9), None, None).unwrap().is_empty());
            assert_eq!(q.journal_list(doc(2), None, None).unwrap().len(), 1);
        });
    }

    #[test]
    fn find_version_answers_the_newest_row_with_that_version() {
        on_db_and_tx(|q, conn| {
            make_doc(conn, 1);
            make_doc(conn, 2);
            q.journal_append(&NewJournalRow {
                title: Some("Old".into()),
                ..row(1, "1@3")
            })
            .unwrap();
            q.journal_append(&row(1, "1@4")).unwrap();
            q.journal_append(&NewJournalRow {
                title: Some("New".into()),
                ..row(1, "1@3")
            })
            .unwrap();
            q.journal_append(&row(2, "1@3")).unwrap();
            let found = q.journal_find_version(doc(1), "1@3").unwrap().unwrap();
            assert_eq!((found.seq, found.title.as_deref()), (3, Some("New")));
            assert_eq!(
                q.journal_find_version(doc(1), "1@4").unwrap().unwrap().seq,
                2
            );
            assert!(q.journal_find_version(doc(1), "1@5").unwrap().is_none());
            assert!(q.journal_find_version(doc(3), "1@3").unwrap().is_none());
        });
    }

    #[test]
    fn delete_removes_the_rows_of_the_given_documents_only() {
        on_db_and_tx(|q, conn| {
            for n in 1..=3 {
                make_doc(conn, n);
                q.journal_append(&row(n, "1@0")).unwrap();
                q.journal_append(&row(n, "1@1")).unwrap();
            }
            q.journal_delete_docs(&[doc(1), doc(3), doc(77)]).unwrap();
            assert!(q.journal_list(doc(1), None, None).unwrap().is_empty());
            assert_eq!(q.journal_list(doc(2), None, None).unwrap().len(), 2);
            assert!(q.journal_list(doc(3), None, None).unwrap().is_empty());
            q.journal_delete_docs(&[]).unwrap();
            // A document that starts again numbers from one.
            assert_eq!(q.journal_append(&row(1, "9@0")).unwrap(), 1);
        });
    }

    #[test]
    fn a_recorded_op_rides_the_callers_transaction_and_a_failure_undoes_only_itself() {
        on_db_and_tx(|q, conn| {
            make_doc(conn, 1);
            q.journal_record(&JournalOp::Append(row(1, "1@0"))).unwrap();
            q.journal_record(&JournalOp::Extend {
                doc_id: doc(1),
                seq: 1,
                ended_at: 500,
                version: "1@3".into(),
                title: Some("Renamed".into()),
            })
            .unwrap();
            let rows = q.journal_list(doc(1), None, None).unwrap();
            assert_eq!(versions(&rows), ["1@3"]);
            assert_eq!(
                (rows[0].ended_at, rows[0].title.as_deref()),
                (500, Some("Renamed"))
            );

            // A document that is not there: the foreign key refuses, the journal statements are undone, the error
            // is answered, and the transaction goes on (what was written before and after stays).
            make_doc(conn, 2);
            assert!(q.journal_record(&JournalOp::Append(row(7, "1@0"))).is_err());
            make_doc(conn, 3);
            q.journal_record(&JournalOp::Append(row(3, "3@0"))).unwrap();
            assert!(q.journal_list(doc(7), None, None).unwrap().is_empty());
            assert_eq!(q.journal_list(doc(3), None, None).unwrap().len(), 1);
            let docs: i64 = conn
                .query_row("SELECT COUNT(*) FROM docs", [], |r| r.get(0))
                .unwrap();
            assert_eq!(docs, 3);
            // Extending a row that is not there changes nothing and is no error.
            q.journal_record(&JournalOp::Extend {
                doc_id: doc(2),
                seq: 9,
                ended_at: 1,
                version: "x".into(),
                title: None,
            })
            .unwrap();
            assert!(q.journal_list(doc(2), None, None).unwrap().is_empty());
        });
    }
    #[test]
    fn the_rows_go_with_their_document_and_a_row_needs_one() {
        let db = new_db();
        make_doc(db.connection(), 1);
        db.journal_append(&row(1, "1@0")).unwrap();
        db.connection()
            .execute("DELETE FROM docs WHERE doc_id = ?1", [doc(1).to_bytes()])
            .unwrap();
        assert!(db.journal_list(doc(1), None, None).unwrap().is_empty());
        // No document, no row.
        assert!(db.journal_append(&row(5, "1@0")).is_err());
    }

    #[test]
    fn the_table_refuses_a_kind_or_a_flag_it_does_not_know() {
        let db = new_db();
        make_doc(db.connection(), 1);
        let insert = |kind: &str, this_device: i64| {
            db.connection().execute(
                "INSERT INTO page_history_journal \
                 (doc_id, seq, kind, started_at, ended_at, base_version, version, this_device) \
                 VALUES (?1, 1, ?2, 0, 0, '', '1@0', ?3)",
                params![doc(1).to_bytes(), kind, this_device],
            )
        };
        assert!(insert("edit", 0).is_ok());
        assert!(insert("rename", 0).is_err());
        assert!(
            db.connection()
                .execute(
                    "INSERT INTO page_history_journal \
                     (doc_id, seq, kind, started_at, ended_at, base_version, version, this_device) \
                     VALUES (?1, 2, 'edit', 0, 0, '', '1@0', 2)",
                    [doc(1).to_bytes()],
                )
                .is_err()
        );
    }

    #[test]
    fn a_damaged_row_is_a_typed_error_and_never_a_panic() {
        let db = new_db();
        make_doc(db.connection(), 1);
        db.connection()
            .execute(
                "INSERT INTO page_history_journal \
                 (doc_id, seq, kind, started_at, ended_at, base_version, version, device_id, this_device) \
                 VALUES (?1, 1, 'edit', 0, 0, '', '1@0', x'0102', 0)",
                [doc(1).to_bytes()],
            )
            .unwrap();
        assert!(matches!(
            db.journal_latest(doc(1)),
            Err(StorageError::InvalidData(_))
        ));
        assert!(matches!(
            db.journal_list(doc(1), None, None),
            Err(StorageError::InvalidData(_))
        ));
    }

    #[test]
    fn the_lookups_go_through_the_primary_key() {
        let db = new_db();
        let plan_of = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> Vec<String> {
            db.connection()
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(params, |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let blob = vec![1u8; 16];
        let plans = [
            ("latest", plan_of(LATEST_SQL, &[&blob])),
            ("list", plan_of(LIST_SQL, &[&blob, &i64::MAX, &-1i64])),
            ("find", plan_of(FIND_SQL, &[&blob, &"1@0"])),
        ];
        for (name, plan) in plans {
            assert!(
                plan.iter().any(|step| step.contains("PRIMARY KEY")),
                "{name}: {plan:?}"
            );
        }
    }
}
