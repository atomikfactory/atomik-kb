//! View links in the server's SQLite file (M13 slice 4, ADR-0051; the rules are the module
//! documentation of `crate::links`). Two tables, both created by `v2::SCHEMA`: `links` (the row, the
//! sealed publication as it came, `pub_seq`, the optional expiry) and `link_chunks` (the declared chunk
//! hashes). A chunk is served when the link declares its hash **and** the link's feed holds it, so the
//! declaration is a permission and not a copy. The publication's bytes count against the feed's billing
//! account like a chunk does, and every path that removes links gives them back.

use kb_sync::v2::LinkPut;
use kb_sync::v2::guard::Access;
use kb_types::sharing::LINK_EXPIRY_MAX_DAYS;
use kb_types::{AccountId, FeedId, LinkId, MemberRole};
use rusqlite::{OptionalExtension, ToSql, Transaction, params};

use super::{DAY_MS, Store};
use crate::error::HttpError;
use crate::store::{array, len_i64, storage, to_u64};

/// How far past the 365-day bound a client clock that runs ahead of the server's may put an expiry.
const EXPIRY_SKEW_MS: i64 = 60 * 60 * 1000;

/// What a public read of a link found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LinkRead {
    /// Unknown, expired or deleted: one answer.
    Missing,
    /// The caller already holds this `pub_seq`.
    NotModified(u64),
    /// The current publication.
    Found { pub_seq: u64, bytes: Vec<u8> },
}

impl Store {
    /// Creates or refreshes a link (`PUT /v2/links/{id}`): the session's account must be an Editor or
    /// Owner of `put.feed` (a stranger is 404, a lower role 403, a closed feed 409 `feed_closed`). The
    /// first upload fixes `created_by` and `expires_at` and starts `pub_seq` at 1; a later one needs the
    /// same feed (409 `conflict` otherwise), replaces the publication and the declared chunks and adds 1.
    /// Answers the new `pub_seq`.
    pub(crate) fn link_put(
        &mut self,
        account: AccountId,
        link: LinkId,
        put: &LinkPut,
        quota: u64,
        now_ms: i64,
    ) -> Result<u64, HttpError> {
        let standing = self.standing(account, put.feed, now_ms)?;
        standing.require(MemberRole::Editor)?;
        if standing.closed {
            return Err(HttpError::feed_closed());
        }
        let latest = i64::from(LINK_EXPIRY_MAX_DAYS)
            .saturating_mul(DAY_MS)
            .saturating_add(EXPIRY_SKEW_MS);
        if put
            .expires_at
            .is_some_and(|at| at > now_ms.saturating_add(latest))
        {
            return Err(HttpError::bad_request(
                "bad_request",
                &format!("a link expires within {LINK_EXPIRY_MAX_DAYS} days"),
            ));
        }
        let feed = put.feed;
        let size = len_i64(&put.publication);
        let tx = self.conn.transaction().map_err(storage)?;
        let existing: Option<(Vec<u8>, i64, i64)> = tx
            .query_row(
                "SELECT feed, pub_seq, length(publication) FROM links WHERE link_id = ?1",
                [link.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        let pub_seq = if let Some((stored_feed, seq, old_size)) = existing {
            if stored_feed.as_slice() != feed.as_bytes().as_slice() {
                return Err(HttpError::conflict("the link belongs to another space"));
            }
            let delta = size.saturating_sub(old_size);
            Self::check_billing(&tx, standing.billing, delta, quota)?;
            tx.execute(
                "UPDATE links SET publication = ?2, pub_seq = pub_seq + 1 WHERE link_id = ?1",
                params![link.as_bytes().as_slice(), put.publication],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM link_chunks WHERE link_id = ?1",
                [link.as_bytes().as_slice()],
            )
            .map_err(storage)?;
            Self::add_feed_bytes(&tx, feed, standing.billing, delta)?;
            to_u64(seq).saturating_add(1)
        } else {
            Self::check_billing(&tx, standing.billing, size, quota)?;
            tx.execute(
                "INSERT INTO links (link_id, feed, created_by, publication, pub_seq, expires_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![
                    link.as_bytes().as_slice(),
                    feed.as_bytes().as_slice(),
                    account.as_bytes().as_slice(),
                    put.publication,
                    put.expires_at
                ],
            )
            .map_err(storage)?;
            Self::add_feed_bytes(&tx, feed, standing.billing, size)?;
            1
        };
        for hash in &put.chunks {
            tx.execute(
                "INSERT OR IGNORE INTO link_chunks (link_id, hash) VALUES (?1, ?2)",
                params![link.as_bytes().as_slice(), hash.as_bytes().as_slice()],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)?;
        Ok(pub_seq)
    }

    /// The publication of a link that exists and has not expired on the server's clock. `unchanged` is
    /// asked with the stored `pub_seq` (the caller's `If-None-Match`); when it says yes the bytes are not
    /// read.
    pub(crate) fn link_get(
        &self,
        link: LinkId,
        now_ms: i64,
        unchanged: impl FnOnce(u64) -> bool,
    ) -> Result<LinkRead, HttpError> {
        let seq: Option<i64> = self
            .conn
            .query_row(
                "SELECT pub_seq FROM links
                 WHERE link_id = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                params![link.as_bytes().as_slice(), now_ms],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let Some(seq) = seq else {
            return Ok(LinkRead::Missing);
        };
        let pub_seq = to_u64(seq);
        if unchanged(pub_seq) {
            return Ok(LinkRead::NotModified(pub_seq));
        }
        let bytes: Vec<u8> = self
            .conn
            .query_row(
                "SELECT publication FROM links WHERE link_id = ?1",
                [link.as_bytes().as_slice()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        Ok(LinkRead::Found { pub_seq, bytes })
    }

    /// A chunk the link declares and its feed holds, while the link is live; `None` for everything else.
    pub(crate) fn link_chunk(
        &self,
        link: LinkId,
        hash: &[u8; 32],
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>, HttpError> {
        self.conn
            .query_row(
                "SELECT c.bytes FROM links l
                 JOIN link_chunks d ON d.link_id = l.link_id
                 JOIN feed_chunks c ON c.feed = l.feed AND c.hash = d.hash
                 WHERE l.link_id = ?1 AND d.hash = ?2
                   AND (l.expires_at IS NULL OR l.expires_at > ?3)",
                params![link.as_bytes().as_slice(), hash.as_slice(), now_ms],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    /// Revokes a link: its creator while still a member of the feed, or an Owner. Another member is 403,
    /// a stranger or an unknown link 404.
    pub(crate) fn link_delete(
        &mut self,
        account: AccountId,
        link: LinkId,
        now_ms: i64,
    ) -> Result<(), HttpError> {
        let row: Option<(Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT feed, created_by FROM links WHERE link_id = ?1",
                [link.as_bytes().as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?;
        let Some((feed, creator)) = row else {
            return Err(HttpError::not_found("not found"));
        };
        let feed = FeedId::from_bytes(array(&feed)?);
        match self.standing(account, feed, now_ms)?.access {
            Access::Member(role)
                if role == MemberRole::Owner
                    || creator.as_slice() == account.as_bytes().as_slice() => {}
            Access::Member(_) => return Err(HttpError::role_forbidden()),
            Access::Removed { .. } | Access::Stranger => {
                return Err(HttpError::not_found("not found"));
            }
        }
        let tx = self.conn.transaction().map_err(storage)?;
        drop_links(&tx, "links.link_id = ?1", &[&link.as_bytes().as_slice()])?;
        tx.commit().map_err(storage)
    }
}

/// Deletes the links the condition selects (a `WHERE` over `links` columns with `links.` prefixes): the
/// rows, their declared chunks (cascade), and gives the publications' bytes back to the feeds and their
/// billing accounts. Answers how many links went.
fn drop_links(
    tx: &Transaction<'_>,
    condition: &str,
    args: &[&dyn ToSql],
) -> Result<usize, HttpError> {
    let owed: Vec<(Vec<u8>, Vec<u8>, i64)> = {
        let mut stmt = tx
            .prepare(&format!(
                "SELECT links.feed, feeds.billing_account, COALESCE(SUM(length(links.publication)), 0)
                 FROM links JOIN feeds ON feeds.feed_id = links.feed
                 WHERE {condition} GROUP BY links.feed, feeds.billing_account"
            ))
            .map_err(storage)?;
        let rows = stmt
            .query_map(args, |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(storage)?;
        rows.collect::<Result<_, _>>().map_err(storage)?
    };
    for (feed, billing, bytes) in owed {
        Store::add_feed_bytes(
            tx,
            FeedId::from_bytes(array(&feed)?),
            AccountId::from_bytes(array(&billing)?),
            bytes.saturating_neg(),
        )?;
    }
    tx.execute(&format!("DELETE FROM links WHERE {condition}"), args)
        .map_err(storage)
}

/// Deletes every link of a feed (the feed was closed).
pub(super) fn drop_feed_links(tx: &Transaction<'_>, feed: FeedId) -> Result<(), HttpError> {
    drop_links(tx, "links.feed = ?1", &[&feed.as_bytes().as_slice()]).map(drop)
}

/// Deletes the links whose expiry has passed (the hourly pass); answers how many.
pub(in crate::store) fn purge_expired_links(
    tx: &Transaction<'_>,
    now_ms: i64,
) -> Result<usize, HttpError> {
    drop_links(
        tx,
        "links.expires_at IS NOT NULL AND links.expires_at <= ?1",
        &[&now_ms],
    )
}
