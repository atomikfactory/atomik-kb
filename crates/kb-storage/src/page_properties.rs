//! The property values of the pages (schema v10, ADR-0026). Derived data: the values live in the
//! structure documents, so the core rewrites the rows of a page in the same transaction as the
//! update row of the document that changed (exactly like [`crate::DateMentionQueries`] for the
//! date mentions), `IndexRebuild` recreates the table from scratch, and it is neither synced nor
//! part of a backup.
//!
//! Same conventions as [`crate::docs`]: every method exists on [`Db`] and [`Tx`] through
//! [`PagePropertyQueries`], both running the same private functions over a connection (a
//! transaction derefs to one), so a method behaves identically in either place; the
//! multi-statement methods of a [`Db`] run as one atomic unit. Ids are bound as 16-byte blobs.
//! Rows are read as raw column values first and validated afterwards, so a damaged row is a
//! typed `InvalidData` error and never a panic or a silently wrong value. Values (the user's
//! content) never appear in an error.
//!
//! # The table
//!
//! One row per scalar value, one per selected option and one per attached file of a page's
//! property: `value_text` (`text`, `url`, `email`, `phone`), `value_number`, `value_bool`
//! (`checkbox`), `value_date` and `value_time` (`date`), `option_id` (`select` and `status`: one
//! row; `multi_select`: one row per option), `attachment_id` (`files`: one row per file, in
//! order), `related_page_id` (`relation`: one row per related page, in order, schema v13),
//! `account_id` (`person`: one row per listed account, in order, schema v18) or
//! `value_number` (the items done) with `value_total` (the items) for a `checklist`.
//! A page has at most one row per scalar property, one per option, one per file, one per
//! related page and one per listed account (five partial unique indexes); a repeated row in one
//! replacement keeps the first. This crate stores
//! what it is given; the model decides what a value is
//! (`kb_model::StructureDoc::property_index`).
//!
//! The `attachment_id` rows are attachment references: [`crate::AttachmentQueries`] counts them
//! with the blocks that show an attachment, so a file attached only to a property is kept by the
//! retention of attachments.
//!
//! # Queries
//!
//! [`PagePropertyQueries::page_properties_pages`] answers `PagesByProperty`: the live pages of a
//! space (per `pages_index`; trashed pages keep their rows but are never listed) whose rows of one
//! property match, ordered by title (ASCII case-insensitive, then exact, then page id). The
//! simple matches run in SQL over the indexes `(space_id, property_id, option_id)` and
//! `(space_id, property_id, value_text)`; a text search folds case with Unicode lowercase, which
//! SQLite cannot do, so it reads the property's text rows in title order and filters them here.

use kb_types::{AccountId, AttachmentId, OptionId, PageId, PropertyId, PropertyKind, SpaceId};
use rusqlite::{Connection, params, params_from_iter, types::Value};

use crate::docs::{atomically, id_from};
use crate::{Db, StorageError, Tx};

/// A `page_properties` row without its page and space (they are the arguments of the methods).
#[derive(Debug, Clone, PartialEq)]
pub struct PagePropertyRow {
    /// The property.
    pub property_id: PropertyId,
    /// Its kind.
    pub kind: PropertyKind,
    /// A `text`, `url`, `email` or `phone` value.
    pub value_text: Option<String>,
    /// A `number` value.
    pub value_number: Option<f64>,
    /// A `checkbox` value.
    pub value_bool: Option<bool>,
    /// A `date` value: the day, `YYYY-MM-DD`.
    pub value_date: Option<String>,
    /// A `date` value: the time of day, `HH:mm`.
    pub value_time: Option<String>,
    /// A `select`, `status` or `multi_select` option.
    pub option_id: Option<OptionId>,
    /// A `files` value: one attached file (one row per file).
    pub attachment_id: Option<AttachmentId>,
    /// A `checklist` value: the number of items (`value_number` is the number done).
    pub value_total: Option<u32>,
    /// A `relation` value: one related page (one row per page).
    pub related_page_id: Option<PageId>,
    /// A `person` value: one listed account (one row per account, in order; M13 slice 2,
    /// ADR-0049).
    pub account_id: Option<AccountId>,
}

/// What [`PagePropertyQueries::page_properties_pages`] looks for in the rows of one property.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropertyMatch<'a> {
    /// The page has any row of the property.
    HasValue,
    /// The page has this option.
    Option(OptionId),
    /// The page's options are exactly these (as a set; at least one).
    OptionSet(&'a [OptionId]),
    /// The page's text value is exactly this.
    Text(&'a str),
    /// The page's text value contains this, ignoring case (Unicode lowercase).
    TextContains(&'a str),
    /// The page's number is this.
    Number(f64),
    /// The page's checkbox is this.
    Bool(bool),
    /// Every live page of the space without a checkbox that is on (off or unset).
    NotChecked,
    /// The page's date value is this day, with this time (`None` matches only a date without
    /// one).
    Date {
        /// The day, `YYYY-MM-DD`.
        date: &'a str,
        /// The time of day, `HH:mm`.
        time: Option<&'a str>,
    },
    /// The page's date is on or after `from` and on or before `to` (a missing end is open);
    /// the time of day is ignored.
    DateBetween {
        /// First day, inclusive.
        from: Option<&'a str>,
        /// Last day, inclusive.
        to: Option<&'a str>,
    },
    /// The page has a checklist with every item done (`true`), or with an item not done
    /// (`false`). A page without a checklist is in neither.
    ChecklistComplete(bool),
    /// The page's `relation` value lists `page` (`Some`), or any page of the space (`None`).
    /// `paired` is the paired property of a two-way relation: a page also matches when `page`
    /// stores itself in the paired property (the symmetric view of ADR-0027). Rows that name a
    /// page the space does not have are ignored.
    Related {
        /// The related page asked for, or any.
        page: Option<PageId>,
        /// The paired property of a two-way relation.
        paired: Option<PropertyId>,
    },
    /// The page's `person` value lists this account (ADR-0049). A page that lists accounts that
    /// are no longer members still matches for each of them.
    Person(AccountId),
}

/// Queries of the `page_properties` table, available on a connection or inside a transaction.
/// On a [`Db`] every method is atomic by itself.
pub trait PagePropertyQueries {
    /// Replaces the rows of a page with `rows`, in the order given (a page's rows are read back
    /// in that order). A row that repeats an earlier scalar property or option of the page is
    /// dropped. The page does not have to be in `pages_index`.
    fn page_properties_replace(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        rows: &[PagePropertyRow],
    ) -> Result<(), StorageError>;

    /// The rows of a page in the order they were written; empty for a page without values.
    fn page_properties_of(&self, page_id: PageId) -> Result<Vec<PagePropertyRow>, StorageError>;

    /// Removes the rows of these pages (purge). Succeeds for pages without rows.
    fn page_properties_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError>;

    /// Removes the rows of every page of a space (before a rebuild of that space).
    fn page_properties_clear_space(&self, space_id: SpaceId) -> Result<(), StorageError>;

    /// Removes every row (before a full rebuild).
    fn page_properties_clear(&self) -> Result<(), StorageError>;

    /// The number of rows of a space.
    fn page_properties_count(&self, space_id: SpaceId) -> Result<u64, StorageError>;

    /// The attachments the rows of these pages attach, each once, ordered by attachment id.
    fn page_properties_attachments_of(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<AttachmentId>, StorageError>;

    /// The attachments the rows of a space attach, each once, ordered by attachment id.
    fn page_properties_attachments_in_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<AttachmentId>, StorageError>;

    /// The pages whose rows attach an attachment, each with its space, ordered by page id.
    fn page_properties_pages_with_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Vec<(SpaceId, PageId)>, StorageError>;

    /// The pages whose rows list `page_id` as a related page, in any property (live or trashed,
    /// each once, ordered by page id): the pages whose search text holds its title.
    fn page_properties_pages_relating_to(
        &self,
        page_id: PageId,
    ) -> Result<Vec<PageId>, StorageError>;

    /// The live pages of a space whose rows of `property_id` match, ordered by title (ASCII
    /// case-insensitive, then exact, then page id). Trashed pages, and pages missing from
    /// `pages_index`, are never listed.
    fn page_properties_pages(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        matching: &PropertyMatch<'_>,
    ) -> Result<Vec<PageId>, StorageError>;
}

impl PagePropertyQueries for Db {
    fn page_properties_replace(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        rows: &[PagePropertyRow],
    ) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            page_properties_replace_on(conn, space_id, page_id, rows)
        })
    }
    fn page_properties_of(&self, page_id: PageId) -> Result<Vec<PagePropertyRow>, StorageError> {
        page_properties_of_on(&self.conn, page_id)
    }
    fn page_properties_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        atomically(&self.conn, |conn| {
            page_properties_delete_pages_on(conn, pages)
        })
    }
    fn page_properties_clear_space(&self, space_id: SpaceId) -> Result<(), StorageError> {
        page_properties_clear_space_on(&self.conn, space_id)
    }
    fn page_properties_clear(&self) -> Result<(), StorageError> {
        page_properties_clear_on(&self.conn)
    }
    fn page_properties_count(&self, space_id: SpaceId) -> Result<u64, StorageError> {
        page_properties_count_on(&self.conn, space_id)
    }
    fn page_properties_attachments_of(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<AttachmentId>, StorageError> {
        page_properties_attachments_of_on(&self.conn, pages)
    }
    fn page_properties_attachments_in_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<AttachmentId>, StorageError> {
        page_properties_attachments_in_space_on(&self.conn, space_id)
    }
    fn page_properties_pages_with_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Vec<(SpaceId, PageId)>, StorageError> {
        page_properties_pages_with_attachment_on(&self.conn, attachment_id)
    }
    fn page_properties_pages_relating_to(
        &self,
        page_id: PageId,
    ) -> Result<Vec<PageId>, StorageError> {
        page_properties_pages_relating_to_on(&self.conn, page_id)
    }
    fn page_properties_pages(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        matching: &PropertyMatch<'_>,
    ) -> Result<Vec<PageId>, StorageError> {
        page_properties_pages_on(&self.conn, space_id, property_id, matching)
    }
}

impl PagePropertyQueries for Tx<'_> {
    fn page_properties_replace(
        &self,
        space_id: SpaceId,
        page_id: PageId,
        rows: &[PagePropertyRow],
    ) -> Result<(), StorageError> {
        page_properties_replace_on(&self.tx, space_id, page_id, rows)
    }
    fn page_properties_of(&self, page_id: PageId) -> Result<Vec<PagePropertyRow>, StorageError> {
        page_properties_of_on(&self.tx, page_id)
    }
    fn page_properties_delete_pages(&self, pages: &[PageId]) -> Result<(), StorageError> {
        page_properties_delete_pages_on(&self.tx, pages)
    }
    fn page_properties_clear_space(&self, space_id: SpaceId) -> Result<(), StorageError> {
        page_properties_clear_space_on(&self.tx, space_id)
    }
    fn page_properties_clear(&self) -> Result<(), StorageError> {
        page_properties_clear_on(&self.tx)
    }
    fn page_properties_count(&self, space_id: SpaceId) -> Result<u64, StorageError> {
        page_properties_count_on(&self.tx, space_id)
    }
    fn page_properties_attachments_of(
        &self,
        pages: &[PageId],
    ) -> Result<Vec<AttachmentId>, StorageError> {
        page_properties_attachments_of_on(&self.tx, pages)
    }
    fn page_properties_attachments_in_space(
        &self,
        space_id: SpaceId,
    ) -> Result<Vec<AttachmentId>, StorageError> {
        page_properties_attachments_in_space_on(&self.tx, space_id)
    }
    fn page_properties_pages_with_attachment(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Vec<(SpaceId, PageId)>, StorageError> {
        page_properties_pages_with_attachment_on(&self.tx, attachment_id)
    }
    fn page_properties_pages_relating_to(
        &self,
        page_id: PageId,
    ) -> Result<Vec<PageId>, StorageError> {
        page_properties_pages_relating_to_on(&self.tx, page_id)
    }
    fn page_properties_pages(
        &self,
        space_id: SpaceId,
        property_id: PropertyId,
        matching: &PropertyMatch<'_>,
    ) -> Result<Vec<PageId>, StorageError> {
        page_properties_pages_on(&self.tx, space_id, property_id, matching)
    }
}

// ---- the SQL, once, over a plain connection ------------------------------------------

/// The rows of a page (`page_properties_page`).
const DELETE_PAGE_SQL: &str = "DELETE FROM page_properties WHERE page_id = ?1";

/// The rows of a space (the lookup indexes start with the space).
const DELETE_SPACE_SQL: &str = "DELETE FROM page_properties WHERE space_id = ?1";

/// A page's rows in the order they were written: `rowid` grows with every insert.
const OF_PAGE_SQL: &str = "SELECT property_id, kind, value_text, value_number, value_bool, \
            value_date, value_time, option_id, attachment_id, value_total, related_page_id, \
            account_id \
       FROM page_properties WHERE page_id = ?1 ORDER BY rowid";

/// The live pages of a space that relate to one page through a property, also through the
/// paired property (`?3` is the related page, `?4` the paired property or NULL).
const RELATED_TO_SQL: &str = "SELECT p.page_id FROM pages_index AS p \
      WHERE p.space_id = ?1 AND p.trashed_at IS NULL \
        AND (EXISTS (SELECT 1 FROM page_properties AS v \
                      WHERE v.page_id = p.page_id AND v.property_id = ?2 AND v.related_page_id = ?3) \
             OR (?4 IS NOT NULL AND EXISTS (SELECT 1 FROM page_properties AS v \
                      WHERE v.page_id = ?3 AND v.property_id = ?4 AND v.related_page_id = p.page_id))) \
      ORDER BY p.title COLLATE NOCASE, p.title, p.page_id";

/// The live pages of a space that relate to any page of the space through a property or its
/// paired property (`?3`, or NULL).
const HAS_RELATED_SQL: &str = "SELECT p.page_id FROM pages_index AS p \
      WHERE p.space_id = ?1 AND p.trashed_at IS NULL \
        AND (EXISTS (SELECT 1 FROM page_properties AS v \
                      CROSS JOIN pages_index AS r ON r.page_id = v.related_page_id \
                      WHERE v.page_id = p.page_id AND v.property_id = ?2 AND r.space_id = ?1) \
             OR (?3 IS NOT NULL AND EXISTS (SELECT 1 FROM page_properties AS v \
                      CROSS JOIN pages_index AS r ON r.page_id = v.page_id \
                      WHERE v.property_id = ?3 AND v.related_page_id = p.page_id AND r.space_id = ?1))) \
      ORDER BY p.title COLLATE NOCASE, p.title, p.page_id";

/// The attachments attached by the rows of a page (`page_properties_page`), once each.
const ATTACHMENTS_OF_PAGE_SQL: &str = "SELECT DISTINCT attachment_id FROM page_properties \
       WHERE page_id = ?1 AND attachment_id IS NOT NULL";

/// The attachments attached by the rows of a space (the lookup indexes start with the space).
const ATTACHMENTS_IN_SPACE_SQL: &str = "SELECT DISTINCT attachment_id FROM page_properties \
       WHERE space_id = ?1 AND attachment_id IS NOT NULL";

/// The pages that attach an attachment (`page_properties_by_attachment`).
const PAGES_WITH_ATTACHMENT_SQL: &str = "SELECT DISTINCT space_id, page_id FROM page_properties \
       WHERE attachment_id = ?1 ORDER BY page_id";

/// The rows of a property of a space, joined to their live pages. `page_properties` drives
/// through the `(space_id, property_id, ...)` indexes (`CROSS JOIN` pins the order), each hit
/// looks its page up by primary key in `pages_index`. `?1` is the space, `?2` the property.
const PAGES_HEAD: &str = "SELECT p.page_id FROM page_properties AS v \
      CROSS JOIN pages_index AS p ON p.page_id = v.page_id \
      WHERE v.space_id = ?1 AND v.property_id = ?2 AND p.space_id = ?1 AND p.trashed_at IS NULL";

/// The order of the pages of a query: by title (ASCII case-insensitive, then exact), then id.
const PAGES_ORDER: &str = " ORDER BY p.title COLLATE NOCASE, p.title, p.page_id";

/// Every live page of the space whose checkbox of the property is not on.
const NOT_CHECKED_SQL: &str = "SELECT p.page_id FROM pages_index AS p \
      WHERE p.space_id = ?1 AND p.trashed_at IS NULL \
        AND NOT EXISTS (SELECT 1 FROM page_properties AS v \
                         WHERE v.page_id = p.page_id AND v.property_id = ?2 AND v.value_bool = 1) \
      ORDER BY p.title COLLATE NOCASE, p.title, p.page_id";

/// The text rows of a property, with their live pages, in page order (the text search filters
/// them in Rust).
const TEXT_ROWS_SQL: &str = "SELECT p.page_id, v.value_text FROM page_properties AS v \
      CROSS JOIN pages_index AS p ON p.page_id = v.page_id \
      WHERE v.space_id = ?1 AND v.property_id = ?2 AND v.value_text IS NOT NULL \
        AND p.space_id = ?1 AND p.trashed_at IS NULL \
      ORDER BY p.title COLLATE NOCASE, p.title, p.page_id";

/// The statement of a query that is one condition on the rows of the property, with its
/// parameters after `?1` (space) and `?2` (property).
fn pages_sql(matching: &PropertyMatch<'_>) -> Option<(String, Vec<Value>)> {
    let (condition, extra): (String, Vec<Value>) = match matching {
        PropertyMatch::HasValue => (String::new(), Vec::new()),
        PropertyMatch::Option(option) => (
            " AND v.option_id = ?3".to_owned(),
            vec![Value::Blob(option.to_bytes().to_vec())],
        ),
        PropertyMatch::Person(account) => (
            " AND v.account_id = ?3".to_owned(),
            vec![Value::Blob(account.to_bytes().to_vec())],
        ),
        PropertyMatch::Text(text) => (
            " AND v.value_text = ?3".to_owned(),
            vec![Value::Text((*text).to_owned())],
        ),
        PropertyMatch::Number(number) => (
            " AND v.value_number = ?3".to_owned(),
            vec![Value::Real(*number)],
        ),
        PropertyMatch::Bool(on) => (
            " AND v.value_bool = ?3".to_owned(),
            vec![Value::Integer(i64::from(*on))],
        ),
        PropertyMatch::Date { date, time } => (
            " AND v.value_date = ?3 AND v.value_time IS ?4".to_owned(),
            vec![
                Value::Text((*date).to_owned()),
                time.map_or(Value::Null, |time| Value::Text(time.to_owned())),
            ],
        ),
        PropertyMatch::DateBetween { from, to } => (
            " AND v.value_date IS NOT NULL \
              AND (?3 IS NULL OR v.value_date >= ?3) AND (?4 IS NULL OR v.value_date <= ?4)"
                .to_owned(),
            vec![
                from.map_or(Value::Null, |day| Value::Text(day.to_owned())),
                to.map_or(Value::Null, |day| Value::Text(day.to_owned())),
            ],
        ),
        PropertyMatch::ChecklistComplete(complete) => (
            if *complete {
                " AND v.value_total IS NOT NULL AND v.value_number >= v.value_total".to_owned()
            } else {
                " AND v.value_total IS NOT NULL AND v.value_number < v.value_total".to_owned()
            },
            Vec::new(),
        ),
        PropertyMatch::OptionSet(options) => {
            let mut distinct: Vec<[u8; 16]> = options.iter().map(|id| id.to_bytes()).collect();
            distinct.sort_unstable();
            distinct.dedup();
            if distinct.is_empty() {
                return None;
            }
            let holders: Vec<String> = (0..distinct.len()).map(|i| format!("?{}", i + 4)).collect();
            let mut params = vec![Value::Integer(i64::try_from(distinct.len()).ok()?)];
            params.extend(distinct.iter().map(|id| Value::Blob(id.to_vec())));
            let tail = format!(
                " AND v.option_id IS NOT NULL GROUP BY p.page_id \
                  HAVING count(*) = ?3 AND sum(v.option_id IN ({})) = ?3",
                holders.join(", ")
            );
            return Some((format!("{PAGES_HEAD}{tail}{PAGES_ORDER}"), params));
        }
        PropertyMatch::NotChecked
        | PropertyMatch::TextContains(_)
        | PropertyMatch::Related { .. } => return None,
    };
    Some((
        format!("{PAGES_HEAD}{condition} GROUP BY p.page_id{PAGES_ORDER}"),
        extra,
    ))
}

fn page_properties_replace_on(
    conn: &Connection,
    space_id: SpaceId,
    page_id: PageId,
    rows: &[PagePropertyRow],
) -> Result<(), StorageError> {
    conn.execute(DELETE_PAGE_SQL, [page_id.to_bytes()])?;
    if rows.is_empty() {
        return Ok(());
    }
    // A repeated scalar property or option of one page (a damaged document) has one row: the
    // first wins.
    let mut insert = conn.prepare(
        "INSERT INTO page_properties \
           (space_id, page_id, property_id, kind, value_text, value_number, value_bool, \
            value_date, value_time, option_id, attachment_id, value_total, related_page_id, \
            account_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
         ON CONFLICT DO NOTHING",
    )?;
    for row in rows {
        insert.execute(params![
            space_id.to_bytes(),
            page_id.to_bytes(),
            row.property_id.to_bytes(),
            row.kind.as_str(),
            row.value_text,
            row.value_number,
            row.value_bool.map(i64::from),
            row.value_date,
            row.value_time,
            row.option_id.map(OptionId::to_bytes),
            row.attachment_id.map(AttachmentId::to_bytes),
            row.value_total,
            row.related_page_id.map(PageId::to_bytes),
            row.account_id.map(AccountId::to_bytes),
        ])?;
    }
    Ok(())
}

/// Raw column values of one row.
type RawRow = (
    Vec<u8>,
    String,
    Option<String>,
    Option<f64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

fn page_properties_of_on(
    conn: &Connection,
    page_id: PageId,
) -> Result<Vec<PagePropertyRow>, StorageError> {
    let mut stmt = conn.prepare(OF_PAGE_SQL)?;
    let rows: Vec<RawRow> = stmt
        .query_map([page_id.to_bytes()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(
                property,
                kind,
                text,
                number,
                flag,
                date,
                time,
                option,
                file,
                total,
                related,
                account,
            )| {
                Ok(PagePropertyRow {
                    property_id: PropertyId::from_bytes(id_from(&property, "property id")?),
                    kind: PropertyKind::parse(&kind).ok_or_else(|| {
                        StorageError::InvalidData("a property row has an unknown kind".to_owned())
                    })?,
                    value_text: text,
                    value_number: number,
                    value_bool: match flag {
                        None => None,
                        Some(0) => Some(false),
                        Some(1) => Some(true),
                        Some(_) => {
                            return Err(StorageError::InvalidData(
                                "a property row has a checkbox that is neither 0 nor 1".to_owned(),
                            ));
                        }
                    },
                    value_date: date,
                    value_time: time,
                    option_id: option
                        .as_deref()
                        .map(|bytes| id_from(bytes, "property option id").map(OptionId::from_bytes))
                        .transpose()?,
                    attachment_id: file
                        .as_deref()
                        .map(|bytes| {
                            id_from(bytes, "property attachment id").map(AttachmentId::from_bytes)
                        })
                        .transpose()?,
                    value_total: total
                        .map(|total| {
                            u32::try_from(total).map_err(|_| {
                                StorageError::InvalidData(
                                    "a property row has a checklist size that is out of range"
                                        .to_owned(),
                                )
                            })
                        })
                        .transpose()?,
                    related_page_id: related.as_deref().map(page_id_from).transpose()?,
                    account_id: account
                        .as_deref()
                        .map(|bytes| {
                            id_from(bytes, "property account id").map(AccountId::from_bytes)
                        })
                        .transpose()?,
                })
            },
        )
        .collect()
}

fn page_properties_delete_pages_on(
    conn: &Connection,
    pages: &[PageId],
) -> Result<(), StorageError> {
    let mut delete = conn.prepare(DELETE_PAGE_SQL)?;
    for page in pages {
        delete.execute([page.to_bytes()])?;
    }
    Ok(())
}

fn page_properties_clear_space_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<(), StorageError> {
    conn.execute(DELETE_SPACE_SQL, [space_id.to_bytes()])?;
    Ok(())
}

fn page_properties_clear_on(conn: &Connection) -> Result<(), StorageError> {
    conn.execute("DELETE FROM page_properties", [])?;
    Ok(())
}

fn page_properties_count_on(conn: &Connection, space_id: SpaceId) -> Result<u64, StorageError> {
    let count: i64 = conn.query_one(
        "SELECT count(*) FROM page_properties WHERE space_id = ?1",
        [space_id.to_bytes()],
        |row| row.get(0),
    )?;
    u64::try_from(count)
        .map_err(|_| StorageError::InvalidData("a row count is out of range".to_owned()))
}

fn page_id_from(bytes: &[u8]) -> Result<PageId, StorageError> {
    id_from(bytes, "property page id").map(PageId::from_bytes)
}

fn attachment_ids_from(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<AttachmentId>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(args, |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = rows
        .iter()
        .map(|bytes| id_from(bytes, "property attachment id").map(AttachmentId::from_bytes))
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn page_properties_attachments_of_on(
    conn: &Connection,
    pages: &[PageId],
) -> Result<Vec<AttachmentId>, StorageError> {
    let mut ids = Vec::new();
    for page in pages {
        ids.extend(attachment_ids_from(
            conn,
            ATTACHMENTS_OF_PAGE_SQL,
            [page.to_bytes()],
        )?);
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

fn page_properties_attachments_in_space_on(
    conn: &Connection,
    space_id: SpaceId,
) -> Result<Vec<AttachmentId>, StorageError> {
    attachment_ids_from(conn, ATTACHMENTS_IN_SPACE_SQL, [space_id.to_bytes()])
}

fn page_properties_pages_with_attachment_on(
    conn: &Connection,
    attachment_id: AttachmentId,
) -> Result<Vec<(SpaceId, PageId)>, StorageError> {
    let mut stmt = conn.prepare(PAGES_WITH_ATTACHMENT_SQL)?;
    let rows = stmt
        .query_map([attachment_id.to_bytes()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.iter()
        .map(|(space, page)| {
            Ok((
                id_from(space, "property space id").map(SpaceId::from_bytes)?,
                page_id_from(page)?,
            ))
        })
        .collect()
}

/// The pages whose rows list a page (`page_properties_by_related`).
const PAGES_RELATING_TO_SQL: &str = "SELECT DISTINCT page_id FROM page_properties \n       WHERE related_page_id = ?1 ORDER BY page_id";

fn page_properties_pages_relating_to_on(
    conn: &Connection,
    page_id: PageId,
) -> Result<Vec<PageId>, StorageError> {
    let mut stmt = conn.prepare(PAGES_RELATING_TO_SQL)?;
    let rows = stmt
        .query_map([page_id.to_bytes()], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    rows.iter().map(|bytes| page_id_from(bytes)).collect()
}

fn page_properties_pages_on(
    conn: &Connection,
    space_id: SpaceId,
    property_id: PropertyId,
    matching: &PropertyMatch<'_>,
) -> Result<Vec<PageId>, StorageError> {
    let space = space_id.to_bytes();
    let property = property_id.to_bytes();
    match matching {
        PropertyMatch::NotChecked => {
            let mut stmt = conn.prepare(NOT_CHECKED_SQL)?;
            let rows = stmt
                .query_map(params![space, property], |row| row.get::<_, Vec<u8>>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows.iter().map(|bytes| page_id_from(bytes)).collect()
        }
        PropertyMatch::Related { page, paired } => {
            let paired = paired.map(PropertyId::to_bytes);
            let rows = match page {
                Some(page) => {
                    let mut stmt = conn.prepare(RELATED_TO_SQL)?;
                    stmt.query_map(params![space, property, page.to_bytes(), paired], |row| {
                        row.get::<_, Vec<u8>>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?
                }
                None => {
                    let mut stmt = conn.prepare(HAS_RELATED_SQL)?;
                    stmt.query_map(params![space, property, paired], |row| {
                        row.get::<_, Vec<u8>>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?
                }
            };
            rows.iter().map(|bytes| page_id_from(bytes)).collect()
        }
        PropertyMatch::TextContains(needle) => {
            let needle = needle.to_lowercase();
            let mut stmt = conn.prepare(TEXT_ROWS_SQL)?;
            let rows = stmt
                .query_map(params![space, property], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.iter()
                .filter(|(_, text)| text.to_lowercase().contains(&needle))
                .map(|(bytes, _)| page_id_from(bytes))
                .collect()
        }
        other => {
            let Some((sql, extra)) = pages_sql(other) else {
                return Ok(Vec::new());
            };
            let mut stmt = conn.prepare(&sql)?;
            let mut args: Vec<Value> =
                vec![Value::Blob(space.to_vec()), Value::Blob(property.to_vec())];
            args.extend(extra);
            let rows = stmt
                .query_map(params_from_iter(args.iter()), |row| {
                    row.get::<_, Vec<u8>>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.iter().map(|bytes| page_id_from(bytes)).collect()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{DocQueries, PageIndexRow};

    /// What a scenario needs of a connection or a transaction: both implement both.
    trait Queries: DocQueries + PagePropertyQueries {}
    impl<T: DocQueries + PagePropertyQueries> Queries for T {}

    fn new_db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// Runs `scenario` on a bare `Db` and again inside a transaction of a fresh one, so that
    /// both `PagePropertyQueries` impls are held to the same behaviour.
    fn on_db_and_tx(scenario: impl Fn(&dyn Queries)) {
        scenario(&new_db());
        new_db()
            .transaction(|tx| {
                scenario(tx);
                Ok(())
            })
            .unwrap();
    }

    const fn space(n: u8) -> SpaceId {
        SpaceId::from_bytes([n; 16])
    }

    const fn page(n: u8) -> PageId {
        PageId::from_bytes([n; 16])
    }

    const fn prop(n: u8) -> PropertyId {
        PropertyId::from_bytes([n; 16])
    }

    const fn option(n: u8) -> OptionId {
        OptionId::from_bytes([n; 16])
    }

    fn add_page(q: &dyn Queries, n: u8, space_id: SpaceId, title: &str, trashed_at: Option<i64>) {
        q.pages_index_upsert(&[PageIndexRow {
            page_id: page(n),
            space_id,
            parent_id: None,
            title: title.to_owned(),
            trashed_at,
            updated_at: 100,
            in_template: false,
        }])
        .unwrap();
    }

    fn empty(property: u8, kind: PropertyKind) -> PagePropertyRow {
        PagePropertyRow {
            property_id: prop(property),
            kind,
            value_text: None,
            value_number: None,
            value_bool: None,
            value_date: None,
            value_time: None,
            option_id: None,
            attachment_id: None,
            value_total: None,
            related_page_id: None,
            account_id: None,
        }
    }

    fn text(property: u8, value: &str) -> PagePropertyRow {
        PagePropertyRow {
            value_text: Some(value.to_owned()),
            ..empty(property, PropertyKind::Text)
        }
    }

    fn number(property: u8, value: f64) -> PagePropertyRow {
        PagePropertyRow {
            value_number: Some(value),
            ..empty(property, PropertyKind::Number)
        }
    }

    fn checkbox(property: u8, on: bool) -> PagePropertyRow {
        PagePropertyRow {
            value_bool: Some(on),
            ..empty(property, PropertyKind::Checkbox)
        }
    }

    fn date(property: u8, day: &str, time: Option<&str>) -> PagePropertyRow {
        PagePropertyRow {
            value_date: Some(day.to_owned()),
            value_time: time.map(str::to_owned),
            ..empty(property, PropertyKind::Date)
        }
    }

    fn selected(property: u8, kind: PropertyKind, n: u8) -> PagePropertyRow {
        PagePropertyRow {
            option_id: Some(option(n)),
            ..empty(property, kind)
        }
    }

    const fn file(n: u8) -> AttachmentId {
        AttachmentId::from_bytes([n; 16])
    }

    /// One row of a `files` property: attachment `n`.
    fn attached(property: u8, n: u8) -> PagePropertyRow {
        PagePropertyRow {
            attachment_id: Some(file(n)),
            ..empty(property, PropertyKind::Files)
        }
    }

    /// The one row of a `checklist` property: `done` of `total` items.
    fn checklist(property: u8, done: u32, total: u32) -> PagePropertyRow {
        PagePropertyRow {
            value_number: Some(f64::from(done)),
            value_total: Some(total),
            ..empty(property, PropertyKind::Checklist)
        }
    }

    /// One row of a `relation` property: related page `n`.
    fn related(property: u8, n: u8) -> PagePropertyRow {
        PagePropertyRow {
            related_page_id: Some(page(n)),
            account_id: None,
            ..empty(property, PropertyKind::Relation)
        }
    }

    const fn account(n: u8) -> AccountId {
        AccountId::from_bytes([n; 16])
    }

    /// One row of a `person` property: account `n`.
    fn person(property: u8, n: u8) -> PagePropertyRow {
        PagePropertyRow {
            account_id: Some(account(n)),
            ..empty(property, PropertyKind::Person)
        }
    }

    fn count(db: &Db, from: &str) -> i64 {
        db.connection()
            .query_row(&format!("SELECT count(*) FROM {from}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn found(q: &dyn Queries, property: u8, matching: &PropertyMatch<'_>) -> Vec<u8> {
        q.page_properties_pages(space(1), prop(property), matching)
            .unwrap()
            .iter()
            .map(|page| page.to_bytes()[0])
            .collect()
    }

    #[test]
    fn rows_round_trip_in_the_order_they_were_written() {
        on_db_and_tx(|q| {
            let rows = vec![
                selected(2, PropertyKind::MultiSelect, 9),
                text(1, "Quarterly plan"),
                selected(2, PropertyKind::MultiSelect, 3),
                number(3, -2.5),
                checkbox(4, false),
                date(5, "2026-10-07", Some("14:30")),
                date(6, "2026-10-08", None),
                selected(7, PropertyKind::Status, 4),
            ];
            q.page_properties_replace(space(1), page(1), &rows).unwrap();
            assert_eq!(q.page_properties_of(page(1)).unwrap(), rows);
            assert!(q.page_properties_of(page(2)).unwrap().is_empty());
            q.page_properties_replace(space(1), page(1), &[]).unwrap();
            assert!(q.page_properties_of(page(1)).unwrap().is_empty());
        });
    }

    #[test]
    fn a_person_value_is_one_row_per_account_found_by_account() {
        on_db_and_tx(|q| {
            for (n, title) in [(1, "Alpha"), (2, "Bravo"), (3, "Charlie"), (4, "Delta")] {
                add_page(q, n, space(1), title, None);
            }
            add_page(q, 5, space(1), "Echo", Some(9));
            add_page(q, 6, space(2), "Foxtrot", None);
            // Page 1 lists accounts 7 and 8 (in that order), page 2 only 8, page 3 has none, page 5 is
            // trashed and lists 7, page 6 is in another space and lists 7. Page 1 also repeats 7
            // (a damaged document): one row.
            q.page_properties_replace(
                space(1),
                page(1),
                &[person(1, 7), person(1, 8), person(1, 7), text(2, "x")],
            )
            .unwrap();
            q.page_properties_replace(space(1), page(2), &[person(1, 8)])
                .unwrap();
            q.page_properties_replace(space(1), page(3), &[text(2, "y")])
                .unwrap();
            q.page_properties_replace(space(1), page(5), &[person(1, 7)])
                .unwrap();
            q.page_properties_replace(space(2), page(6), &[person(1, 7)])
                .unwrap();

            // The rows come back in the order written, with the account.
            assert_eq!(
                q.page_properties_of(page(1)).unwrap(),
                [person(1, 7), person(1, 8), text(2, "x")]
            );
            // By account: live pages of the space, by title.
            assert_eq!(found(q, 1, &PropertyMatch::Person(account(7))), [1]);
            assert_eq!(found(q, 1, &PropertyMatch::Person(account(8))), [1, 2]);
            assert!(found(q, 1, &PropertyMatch::Person(account(9))).is_empty());
            // "Has a value" lists the pages once each, whatever the number of people.
            assert_eq!(found(q, 1, &PropertyMatch::HasValue), [1, 2]);
            // A person row does not count as the scalar value of its property: the property can
            // hold a person and the same account is refused twice, but another account is fine.
            q.page_properties_replace(space(1), page(4), &[person(1, 7), person(1, 7)])
                .unwrap();
            assert_eq!(q.page_properties_of(page(4)).unwrap(), [person(1, 7)]);
        });
    }

    #[test]
    fn ids_are_stored_as_16_byte_blobs() {
        let db = new_db();
        db.page_properties_replace(space(7), page(8), &[selected(9, PropertyKind::Select, 10)])
            .unwrap();
        let (space_id, page_id, property_id, option_id, kind): (
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
            String,
        ) = db
            .connection()
            .query_row(
                "SELECT space_id, page_id, property_id, option_id, kind FROM page_properties",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(space_id, vec![7; 16]);
        assert_eq!(page_id, vec![8; 16]);
        assert_eq!(property_id, vec![9; 16]);
        assert_eq!(option_id, vec![10; 16]);
        assert_eq!(kind, "select");
    }

    #[test]
    fn replacing_the_rows_of_a_page_leaves_the_other_pages_alone() {
        on_db_and_tx(|q| {
            q.page_properties_replace(space(1), page(1), &[text(1, "a"), number(2, 1.0)])
                .unwrap();
            q.page_properties_replace(space(1), page(2), &[text(1, "b")])
                .unwrap();
            q.page_properties_replace(space(1), page(1), &[text(1, "c")])
                .unwrap();
            assert_eq!(q.page_properties_of(page(1)).unwrap(), [text(1, "c")]);
            assert_eq!(
                q.page_properties_of(page(2)).unwrap(),
                [text(1, "b")],
                "the same property on another page is another row"
            );
        });
    }

    #[test]
    fn a_property_or_option_twice_in_a_page_keeps_the_first_row() {
        on_db_and_tx(|q| {
            q.page_properties_replace(
                space(1),
                page(1),
                &[
                    text(1, "first"),
                    text(1, "second"),
                    selected(2, PropertyKind::MultiSelect, 5),
                    selected(2, PropertyKind::MultiSelect, 5),
                    selected(2, PropertyKind::MultiSelect, 6),
                ],
            )
            .unwrap();
            assert_eq!(
                q.page_properties_of(page(1)).unwrap(),
                [
                    text(1, "first"),
                    selected(2, PropertyKind::MultiSelect, 5),
                    selected(2, PropertyKind::MultiSelect, 6),
                ]
            );
        });
    }

    #[test]
    fn delete_pages_clear_space_and_clear_remove_rows() {
        on_db_and_tx(|q| {
            q.page_properties_replace(space(1), page(1), &[text(1, "a")])
                .unwrap();
            q.page_properties_replace(space(1), page(2), &[text(1, "b")])
                .unwrap();
            q.page_properties_replace(space(2), page(3), &[text(1, "c")])
                .unwrap();
            assert_eq!(q.page_properties_count(space(1)).unwrap(), 2);
            q.page_properties_delete_pages(&[page(1), page(9)]).unwrap();
            q.page_properties_delete_pages(&[]).unwrap();
            assert!(q.page_properties_of(page(1)).unwrap().is_empty());
            assert_eq!(q.page_properties_count(space(1)).unwrap(), 1);
            q.page_properties_clear_space(space(1)).unwrap();
            assert_eq!(q.page_properties_count(space(1)).unwrap(), 0);
            assert_eq!(
                q.page_properties_count(space(2)).unwrap(),
                1,
                "other spaces stay"
            );
            q.page_properties_clear().unwrap();
            assert_eq!(q.page_properties_count(space(2)).unwrap(), 0);
            q.page_properties_replace(space(2), page(3), &[text(1, "c")])
                .unwrap();
            assert_eq!(q.page_properties_count(space(2)).unwrap(), 1);
        });
    }

    #[test]
    fn the_table_refuses_an_unknown_kind_and_a_checkbox_that_is_not_0_or_1() {
        let db = new_db();
        let bad = |sql: &str| db.connection().execute_batch(sql).is_err();
        assert!(bad(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind) \
             VALUES (zeroblob(16), zeroblob(16), zeroblob(16), 'rollup')"
        ));
        assert!(bad(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, value_bool) \
             VALUES (zeroblob(16), zeroblob(16), zeroblob(16), 'checkbox', 2)"
        ));
        assert_eq!(count(&db, "page_properties"), 0);
    }

    fn fill(q: &dyn Queries) {
        // Pages 1 to 6 in space 1 (titles out of id order), 7 in space 2, 8 trashed.
        for (n, title) in [
            (1, "banana"),
            (2, "Apple"),
            (3, "apple"),
            (4, "cherry"),
            (5, "Date"),
            (6, "elder"),
        ] {
            add_page(q, n, space(1), title, None);
        }
        add_page(q, 7, space(2), "Other space", None);
        add_page(q, 8, space(1), "Trashed", Some(9));
        for n in [1, 2, 3, 4, 5, 6, 8] {
            let rows = match n {
                1 => vec![
                    text(1, "Hello World"),
                    number(2, 42.0),
                    checkbox(3, true),
                    date(4, "2026-10-07", Some("14:30")),
                    selected(5, PropertyKind::Select, 1),
                    selected(6, PropertyKind::MultiSelect, 1),
                    selected(6, PropertyKind::MultiSelect, 2),
                ],
                2 => vec![
                    text(1, "hello"),
                    number(2, 42.0),
                    checkbox(3, false),
                    date(4, "2026-10-07", None),
                    selected(5, PropertyKind::Select, 2),
                    selected(6, PropertyKind::MultiSelect, 2),
                ],
                3 => vec![
                    text(1, "ÉCOLE élève"),
                    date(4, "2026-11-01", None),
                    selected(6, PropertyKind::MultiSelect, 1),
                    selected(6, PropertyKind::MultiSelect, 2),
                    selected(6, PropertyKind::MultiSelect, 3),
                ],
                4 => vec![
                    text(1, "goodbye"),
                    number(2, 7.5),
                    checkbox(3, true),
                    date(4, "2026-09-30", Some("23:59")),
                ],
                5 => vec![checkbox(3, false)],
                6 => vec![text(1, "world")],
                _ => vec![
                    text(1, "hello trashed"),
                    checkbox(3, true),
                    selected(6, PropertyKind::MultiSelect, 1),
                ],
            };
            q.page_properties_replace(space(1), page(n), &rows).unwrap();
        }
        q.page_properties_replace(
            space(2),
            page(7),
            &[text(1, "hello other"), checkbox(3, true)],
        )
        .unwrap();
    }

    #[test]
    fn has_value_lists_the_live_pages_of_the_space_by_title() {
        on_db_and_tx(|q| {
            fill(q);
            // Titles: Apple (2), apple (3), banana (1), cherry (4), Date (5), elder (6): "Apple"
            // before "apple" (case-insensitive tie, then exact: uppercase first), and so on.
            assert_eq!(found(q, 1, &PropertyMatch::HasValue), [2, 3, 1, 4, 6]);
            assert_eq!(found(q, 3, &PropertyMatch::HasValue), [2, 1, 4, 5]);
            assert_eq!(found(q, 9, &PropertyMatch::HasValue), Vec::<u8>::new());
            assert_eq!(
                q.page_properties_pages(space(2), prop(1), &PropertyMatch::HasValue)
                    .unwrap(),
                [page(7)],
                "the other space"
            );
        });
    }

    #[test]
    fn options_numbers_text_checkboxes_and_dates_match_exactly() {
        on_db_and_tx(|q| {
            fill(q);
            assert_eq!(found(q, 5, &PropertyMatch::Option(option(1))), [1]);
            assert_eq!(found(q, 5, &PropertyMatch::Option(option(2))), [2]);
            assert_eq!(found(q, 6, &PropertyMatch::Option(option(2))), [2, 3, 1]);
            assert_eq!(
                found(q, 6, &PropertyMatch::Option(option(9))),
                Vec::<u8>::new()
            );

            assert_eq!(found(q, 2, &PropertyMatch::Number(42.0)), [2, 1]);
            assert_eq!(found(q, 2, &PropertyMatch::Number(7.5)), [4]);
            assert_eq!(found(q, 2, &PropertyMatch::Number(1.0)), Vec::<u8>::new());

            assert_eq!(found(q, 1, &PropertyMatch::Text("hello")), [2]);
            assert_eq!(found(q, 1, &PropertyMatch::Text("Hello World")), [1]);
            assert_eq!(found(q, 1, &PropertyMatch::Text("Hello")), Vec::<u8>::new());

            assert_eq!(found(q, 3, &PropertyMatch::Bool(true)), [1, 4]);
            assert_eq!(found(q, 3, &PropertyMatch::Bool(false)), [2, 5]);

            assert_eq!(
                found(
                    q,
                    4,
                    &PropertyMatch::Date {
                        date: "2026-10-07",
                        time: Some("14:30")
                    }
                ),
                [1]
            );
            assert_eq!(
                found(
                    q,
                    4,
                    &PropertyMatch::Date {
                        date: "2026-10-07",
                        time: None
                    }
                ),
                [2],
                "no time matches only a date without one"
            );
        });
    }

    #[test]
    fn a_set_of_options_matches_exactly_that_set() {
        on_db_and_tx(|q| {
            fill(q);
            assert_eq!(
                found(q, 6, &PropertyMatch::OptionSet(&[option(1), option(2)])),
                [1],
                "page 3 also has option 3, page 2 lacks option 1"
            );
            assert_eq!(
                found(q, 6, &PropertyMatch::OptionSet(&[option(2), option(1)])),
                [1],
                "order does not matter"
            );
            assert_eq!(found(q, 6, &PropertyMatch::OptionSet(&[option(2)])), [2]);
            assert_eq!(
                found(
                    q,
                    6,
                    &PropertyMatch::OptionSet(&[option(1), option(2), option(2), option(3)])
                ),
                [3],
                "a repeated id counts once"
            );
            assert_eq!(
                found(q, 6, &PropertyMatch::OptionSet(&[option(1)])),
                Vec::<u8>::new()
            );
            assert_eq!(
                found(q, 6, &PropertyMatch::OptionSet(&[])),
                Vec::<u8>::new()
            );
        });
    }

    #[test]
    fn a_text_search_ignores_case_and_matches_inside_the_text() {
        on_db_and_tx(|q| {
            fill(q);
            assert_eq!(found(q, 1, &PropertyMatch::TextContains("HELLO")), [2, 1]);
            assert_eq!(found(q, 1, &PropertyMatch::TextContains("o w")), [1]);
            assert_eq!(
                found(q, 1, &PropertyMatch::TextContains("école")),
                [3],
                "Unicode lowercase, not ASCII only"
            );
            assert_eq!(found(q, 1, &PropertyMatch::TextContains("élève")), [3]);
            assert_eq!(
                found(q, 1, &PropertyMatch::TextContains("zzz")),
                Vec::<u8>::new()
            );
            assert_eq!(
                found(q, 1, &PropertyMatch::TextContains("")),
                [2, 3, 1, 4, 6]
            );
            assert_eq!(
                found(q, 2, &PropertyMatch::TextContains("4")),
                Vec::<u8>::new()
            );
        });
    }

    #[test]
    fn dates_between_include_both_ends_and_ignore_the_time() {
        on_db_and_tx(|q| {
            fill(q);
            let between = |from, to| found(q, 4, &PropertyMatch::DateBetween { from, to });
            assert_eq!(between(Some("2026-10-07"), Some("2026-10-07")), [2, 1]);
            assert_eq!(between(Some("2026-10-01"), Some("2026-10-31")), [2, 1]);
            assert_eq!(between(Some("2026-10-08"), None), [3]);
            assert_eq!(between(None, Some("2026-09-30")), [4]);
            assert_eq!(between(None, None), [2, 3, 1, 4]);
            assert_eq!(between(Some("2027-01-01"), None), Vec::<u8>::new());
            assert_eq!(
                between(Some("2026-10-31"), Some("2026-10-01")),
                Vec::<u8>::new()
            );
        });
    }

    #[test]
    fn not_checked_lists_every_live_page_without_a_checkbox_that_is_on() {
        on_db_and_tx(|q| {
            fill(q);
            // On: 1 and 4 (and the trashed page 8). Off or unset: 2, 3 (no value), 5, 6 (no
            // value); the other space's page is not of this space.
            assert_eq!(found(q, 3, &PropertyMatch::NotChecked), [2, 3, 5, 6]);
            assert_eq!(
                found(q, 9, &PropertyMatch::NotChecked),
                [2, 3, 1, 4, 5, 6],
                "a property nobody set: every live page"
            );
        });
    }

    #[test]
    fn trashed_pages_keep_their_rows_and_come_back_when_restored() {
        on_db_and_tx(|q| {
            fill(q);
            assert!(!found(q, 1, &PropertyMatch::HasValue).contains(&8));
            assert_eq!(
                q.page_properties_of(page(8)).unwrap().len(),
                3,
                "the rows stay"
            );
            add_page(q, 8, space(1), "Trashed", None);
            assert!(found(q, 1, &PropertyMatch::HasValue).contains(&8));
            // A page missing from the index is never listed.
            q.page_properties_replace(space(1), page(50), &[text(1, "ghost")])
                .unwrap();
            assert!(!found(q, 1, &PropertyMatch::Text("ghost")).contains(&50));
        });
    }

    #[test]
    fn titles_with_the_same_text_order_by_page_id() {
        on_db_and_tx(|q| {
            for n in [3, 1, 2] {
                add_page(q, n, space(1), "same", None);
                q.page_properties_replace(space(1), page(n), &[text(1, "x")])
                    .unwrap();
            }
            assert_eq!(found(q, 1, &PropertyMatch::HasValue), [1, 2, 3]);
        });
    }

    #[test]
    fn damaged_rows_are_invalid_data_and_never_a_panic() {
        let db = new_db();
        db.connection()
            .execute_batch(
                "INSERT INTO page_properties (space_id, page_id, property_id, kind, value_text) \
                 VALUES (zeroblob(16), zeroblob(16), x'01', 'text', 'secret value');",
            )
            .unwrap();
        let err = db.page_properties_of(page(0)).unwrap_err();
        assert!(matches!(err, StorageError::InvalidData(_)), "{err:?}");
        assert!(!err.to_string().contains("secret"), "{err}");
    }

    #[test]
    fn files_rows_are_one_per_attachment_in_order_and_a_file_twice_keeps_the_first() {
        on_db_and_tx(|q| {
            let rows = vec![
                attached(1, 9),
                text(2, "between"),
                attached(1, 3),
                attached(1, 9),
                attached(4, 9),
            ];
            q.page_properties_replace(space(1), page(1), &rows).unwrap();
            assert_eq!(
                q.page_properties_of(page(1)).unwrap(),
                [
                    attached(1, 9),
                    text(2, "between"),
                    attached(1, 3),
                    attached(4, 9),
                ],
                "the same file twice in one property is one row; another property has its own"
            );
            add_page(q, 1, space(1), "A", None);
            add_page(q, 2, space(1), "B", None);
            q.page_properties_replace(space(1), page(2), &[attached(1, 3)])
                .unwrap();
            assert_eq!(found(q, 1, &PropertyMatch::HasValue), [1, 2]);
            assert_eq!(
                found(q, 2, &PropertyMatch::HasValue),
                [1],
                "a text value of another property"
            );
        });
    }

    #[test]
    fn a_checklist_has_one_row_with_both_counts_and_matches_complete_and_open() {
        on_db_and_tx(|q| {
            for (n, title) in [
                (1, "All done"),
                (2, "Open"),
                (3, "Untouched"),
                (4, "Other kind"),
            ] {
                add_page(q, n, space(1), title, None);
            }
            q.page_properties_replace(space(1), page(1), &[checklist(1, 3, 3)])
                .unwrap();
            q.page_properties_replace(space(1), page(2), &[checklist(1, 1, 3)])
                .unwrap();
            q.page_properties_replace(space(1), page(3), &[checklist(1, 0, 2), text(2, "x")])
                .unwrap();
            q.page_properties_replace(space(1), page(4), &[text(2, "y")])
                .unwrap();
            assert_eq!(
                q.page_properties_of(page(2)).unwrap(),
                [checklist(1, 1, 3)],
                "both counts come back"
            );
            let complete = |on| found(q, 1, &PropertyMatch::ChecklistComplete(on));
            assert_eq!(complete(true), [1], "every item done");
            assert_eq!(
                complete(false),
                [2, 3],
                "an item is open (by title: Open, Untouched); no checklist is in neither answer"
            );
            assert_eq!(found(q, 1, &PropertyMatch::HasValue), [1, 2, 3]);
            assert_eq!(
                found(q, 2, &PropertyMatch::ChecklistComplete(true)),
                Vec::<u8>::new(),
                "a property that is no checklist matches nothing"
            );
        });
    }

    #[test]
    fn the_attachments_of_pages_and_spaces_are_listed_once_and_found_by_page() {
        on_db_and_tx(|q| {
            q.page_properties_replace(space(1), page(1), &[attached(1, 5), attached(1, 2)])
                .unwrap();
            q.page_properties_replace(space(1), page(2), &[attached(3, 5), text(1, "x")])
                .unwrap();
            q.page_properties_replace(space(2), page(3), &[attached(1, 7)])
                .unwrap();
            assert_eq!(
                q.page_properties_attachments_of(&[page(1), page(2)])
                    .unwrap(),
                [file(2), file(5)],
                "once each, by attachment id"
            );
            assert_eq!(
                q.page_properties_attachments_of(&[page(9)]).unwrap(),
                Vec::<AttachmentId>::new()
            );
            assert!(q.page_properties_attachments_of(&[]).unwrap().is_empty());
            assert_eq!(
                q.page_properties_attachments_in_space(space(1)).unwrap(),
                [file(2), file(5)]
            );
            assert_eq!(
                q.page_properties_attachments_in_space(space(2)).unwrap(),
                [file(7)]
            );
            assert_eq!(
                q.page_properties_pages_with_attachment(file(5)).unwrap(),
                [(space(1), page(1)), (space(1), page(2))]
            );
            assert!(
                q.page_properties_pages_with_attachment(file(9))
                    .unwrap()
                    .is_empty()
            );
            q.page_properties_delete_pages(&[page(2)]).unwrap();
            assert_eq!(
                q.page_properties_pages_with_attachment(file(5)).unwrap(),
                [(space(1), page(1))]
            );
        });
    }

    #[test]
    fn the_table_takes_the_new_kinds_and_refuses_a_second_row_of_a_file() {
        let db = new_db();
        db.page_properties_replace(space(1), page(1), &[attached(1, 4), checklist(2, 1, 2)])
            .unwrap();
        let bad = |sql: &str| db.connection().execute_batch(sql).is_err();
        // The unique indexes hold below the typed API too.
        assert!(bad(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, attachment_id) \
             SELECT space_id, page_id, property_id, kind, attachment_id FROM page_properties \
              WHERE kind = 'files'"
        ));
        assert!(bad(
            "INSERT INTO page_properties (space_id, page_id, property_id, kind, value_number) \
             SELECT space_id, page_id, property_id, kind, 1 FROM page_properties \
              WHERE kind = 'checklist'"
        ));
        assert_eq!(count(&db, "page_properties"), 2);
    }

    fn plan_of(db: &Db, sql: &str, args: usize) -> String {
        let mut stmt = db
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let blob = Value::Blob(vec![0; 16]);
        let values: Vec<&Value> = (0..args).map(|_| &blob).collect();
        let rows = stmt
            .query_map(params_from_iter(values), |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.join(" | ")
    }

    #[test]
    fn the_statements_are_driven_by_their_indexes() {
        let db = new_db();
        let (option_sql, _) = pages_sql(&PropertyMatch::Option(option(1))).unwrap();
        let plan = plan_of(&db, &option_sql, 3);
        assert!(
            plan.contains("USING INDEX page_properties_by_option"),
            "an option is found through its index: {plan}"
        );
        assert!(
            plan.contains("SEARCH p USING INDEX sqlite_autoindex_pages_index_1 (page_id=?)"),
            "each hit finds its page by primary key: {plan}"
        );
        let (text_sql, _) = pages_sql(&PropertyMatch::Text("x")).unwrap();
        let plan = plan_of(&db, &text_sql, 3);
        assert!(
            plan.contains("USING INDEX page_properties_by_text"),
            "a text value is found through its index: {plan}"
        );
        let plan = plan_of(&db, DELETE_PAGE_SQL, 1);
        assert!(
            plan.contains("USING INDEX page_properties_page"),
            "a page's rows are found through their index: {plan}"
        );
        let plan = plan_of(&db, OF_PAGE_SQL, 1);
        assert!(plan.contains("USING INDEX page_properties_page"), "{plan}");
        let plan = plan_of(&db, DELETE_SPACE_SQL, 1);
        assert!(
            plan.contains("USING INDEX"),
            "a space is cleared through an index: {plan}"
        );
        let plan = plan_of(&db, PAGES_WITH_ATTACHMENT_SQL, 1);
        assert!(
            plan.contains("USING INDEX page_properties_by_attachment"),
            "the pages of a file are found through its index: {plan}"
        );
        let plan = plan_of(&db, ATTACHMENTS_OF_PAGE_SQL, 1);
        assert!(
            plan.contains("INDEX page_properties_attachment_row (page_id=?)"),
            "the files of a page are found through an index on the page: {plan}"
        );
    }

    #[test]
    fn related_pages_are_rows_in_order_and_a_page_is_listed_once_per_property() {
        on_db_and_tx(|q| {
            let rows = vec![related(1, 5), related(1, 3), related(2, 3)];
            q.page_properties_replace(space(1), page(1), &rows).unwrap();
            assert_eq!(q.page_properties_of(page(1)).unwrap(), rows);
            // The same page twice in one property keeps the first row.
            q.page_properties_replace(space(1), page(1), &[related(1, 5), related(1, 5)])
                .unwrap();
            assert_eq!(q.page_properties_of(page(1)).unwrap(), [related(1, 5)]);
        });
    }

    /// Pages 1 to 5 of space 1 (page 5 trashed), 6 of space 2: 1 lists 2, 5 and 6 (property 1);
    /// 3 lists 1 in the paired property 2; 4 lists a page that does not exist.
    fn fill_relations(q: &dyn Queries) {
        for (n, title) in [(1, "One"), (2, "Two"), (3, "Three"), (4, "Four")] {
            add_page(q, n, space(1), title, None);
        }
        add_page(q, 5, space(1), "Five", Some(9));
        add_page(q, 6, space(2), "Six", None);
        q.page_properties_replace(
            space(1),
            page(1),
            &[related(1, 2), related(1, 5), related(1, 6)],
        )
        .unwrap();
        q.page_properties_replace(space(1), page(3), &[related(2, 1)])
            .unwrap();
        q.page_properties_replace(space(1), page(4), &[related(1, 99)])
            .unwrap();
    }

    #[test]
    fn relates_to_finds_the_live_pages_that_list_a_page_directly_or_through_the_paired_property() {
        on_db_and_tx(|q| {
            fill_relations(q);
            let related_to = |property, page_n, paired: Option<u8>| {
                found(
                    q,
                    property,
                    &PropertyMatch::Related {
                        page: Some(page(page_n)),
                        paired: paired.map(prop),
                    },
                )
            };
            // Directly: page 1 lists 2.
            assert_eq!(related_to(1, 2, None), [1]);
            // A trashed or foreign page is found as the target (it exists), but only live pages
            // are listed.
            assert_eq!(related_to(1, 5, None), [1]);
            assert_eq!(related_to(1, 6, None), [1]);
            // Through the paired property: page 3 lists page 1 on the back side, so page 1 relates
            // to 3 on the forward side, and nothing is stored on page 1 for it.
            assert_eq!(related_to(1, 3, None), Vec::<u8>::new());
            assert_eq!(related_to(1, 3, Some(2)), [1]);
            // The back side: who relates to page 1 through property 2? Page 3 stores it.
            assert_eq!(related_to(2, 1, None), [3]);
            assert_eq!(related_to(2, 3, Some(1)), Vec::<u8>::new());
            assert_eq!(related_to(2, 4, Some(1)), Vec::<u8>::new());
        });
    }

    #[test]
    fn a_relation_has_a_value_when_it_lists_an_existing_page_of_the_space() {
        on_db_and_tx(|q| {
            fill_relations(q);
            let has = |property, paired: Option<u8>| {
                found(
                    q,
                    property,
                    &PropertyMatch::Related {
                        page: None,
                        paired: paired.map(prop),
                    },
                )
            };
            // Page 1 lists live, trashed (listed) and foreign pages; page 4 only a missing one.
            assert_eq!(has(1, None), [1]);
            // Page 3 stores the back property; page 1 lists page 2 in the forward one, so page 2 has
            // a value of the back property too (and page 5 and 6 are not live pages of the space).
            assert_eq!(has(2, None), [3]);
            assert_eq!(has(1, Some(2)), [1]);
            assert_eq!(has(2, Some(1)), [3, 2]);
        });
    }

    #[test]
    fn the_pages_that_list_a_page_are_found_in_any_property() {
        on_db_and_tx(|q| {
            fill_relations(q);
            let relating = |n: u8| q.page_properties_pages_relating_to(page(n)).unwrap();
            assert_eq!(relating(2), [page(1)]);
            assert_eq!(relating(1), [page(3)]);
            assert_eq!(relating(99), [page(4)]);
            assert_eq!(relating(4), Vec::<PageId>::new());
        });
    }

    #[test]
    fn the_relation_queries_run_through_indexes() {
        let db = new_db();
        fill_relations(&db);
        let plan = plan_of(&db, PAGES_RELATING_TO_SQL, 1);
        assert!(
            plan.contains("USING INDEX page_properties_by_related"),
            "the pages that list a page are found through its index: {plan}"
        );
        let plan = plan_of(&db, RELATED_TO_SQL, 4);
        assert!(plan.contains("page_properties"), "{plan}");
        assert!(
            plan.contains("INDEX page_properties_related_row")
                || plan.contains("INDEX page_properties_by_related"),
            "a related page is found through an index: {plan}"
        );
    }
}
