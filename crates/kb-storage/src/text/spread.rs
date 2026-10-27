//! The page-level pass of a search (ADR-0040, item 9): the pages whose query words are spread over
//! several rows of the text index, which the statement of `fts_search` (every word in one row) does
//! not find.
//!
//! For each word, one full-text query lists the live rows that match it (with their page and their
//! `bm25` over that word). The pages that have every word are the intersection of those lists; the
//! pages the first pass already found are left out. For each remaining page the row that holds the
//! most of the words is its hit (ties: the best summed `bm25`, then the first row in the index), and
//! the hits are ordered by that count, then that rank, then the page id. Only the best `room` are
//! read back with their snippets, one small query each, over the words the row holds.

use std::collections::{HashMap, HashSet};

use kb_types::{PageId, SpaceId};
use rusqlite::{Connection, params};

use super::{
    FtsHit, HIGHLIGHT_CLOSE_SQL, HIGHLIGHT_OPEN_SQL, HitColumns, SNIPPET_ELLIPSIS, SearchParams,
    push_prefix_phrase,
};
use crate::StorageError;
use crate::docs::id_from;

/// Rows read for one word at most. A word in more rows than this (in a workspace of hundreds of
/// thousands of blocks) misses the pages beyond them, which only costs a spread-out match.
const MAX_ROWS_PER_WORD: i64 = 250_000;
/// Words the pass looks at: one bit each in the mask of a row. A query has at most 100 (the longest
/// query is 200 characters, and words are separated).
const MAX_WORDS: usize = 128;

/// The live rows that match one word: `?1` the prefix phrase, `?2` the space id or NULL, `?3` the
/// oldest `updated_at` or NULL, `?4` the page id or NULL, `?5` the most rows. The same join order as
/// the main statement: the match drives, the content row comes by rowid, the page by primary key.
const WORD_ROWS_SQL: &str = r"
SELECT fts.rowid, b.page_id, bm25(fts, 5.0, 1.0)
  FROM fts
 CROSS JOIN fts_blocks AS b ON b.id = fts.rowid
 CROSS JOIN pages_index AS p ON p.page_id = b.page_id
 WHERE fts MATCH ?1
   AND p.trashed_at IS NULL
   AND p.in_template = 0
   AND (?2 IS NULL OR p.space_id = ?2)
   AND (?3 IS NULL OR p.updated_at >= ?3)
   AND (?4 IS NULL OR b.page_id = ?4)
 LIMIT ?5";

/// One row as a hit, for the words it holds: `?1` the FTS5 query (those words OR-ed), `?2` and `?3`
/// the highlight markers, `?4` the ellipsis, `?5` the row. Same columns as the main statement.
const ROW_HIT_SQL: &str = r"
SELECT b.page_id,
       b.block_id,
       snippet(fts, CASE WHEN b.block_id IS NULL THEN 0 ELSE 1 END, ?2, ?3, ?4, 12),
       bm25(fts, 5.0, 1.0),
       b.kind
  FROM fts
 CROSS JOIN fts_blocks AS b ON b.id = fts.rowid
 WHERE fts MATCH ?1
   AND fts.rowid = ?5";

/// What is known of a row that matches some of the words.
struct RowWords {
    page: [u8; 16],
    /// Bit `i` is set when the row matches word `i`.
    words: u128,
    /// The sum of the `bm25` of the row for each word it matches (more negative is better).
    score: f64,
}

/// The best row of a page.
struct BestRow {
    page: [u8; 16],
    rowid: i64,
    words: u128,
    score: f64,
}

impl BestRow {
    fn count(&self) -> u32 {
        self.words.count_ones()
    }
}

/// The hits for the pages that have every one of `words` (two or more, none repeated) somewhere,
/// without a row that holds them all, best first, at most `room`.
///
/// `found` holds the pages the first pass already found. The space, date and page filters are those
/// of `search` and `page`; the page must be live.
pub(super) fn spread_hits(
    conn: &Connection,
    words: &[String],
    search: &SearchParams<'_>,
    page: Option<PageId>,
    found: &HashSet<[u8; 16]>,
    room: usize,
) -> Result<Vec<FtsHit>, StorageError> {
    // The longest words first: they are the rarest, so a page set shrinks (or is empty) soonest.
    let mut words: Vec<&String> = words.iter().take(MAX_WORDS).collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.chars().count()));
    let space = search.space_id.map(SpaceId::to_bytes);
    let page = page.map(PageId::to_bytes);

    // The rows that match each word, kept only for pages that have all the words read so far.
    let mut rows: HashMap<i64, RowWords> = HashMap::new();
    let mut candidates: Option<HashSet<[u8; 16]>> = None;
    let mut word_rows = conn.prepare(WORD_ROWS_SQL)?;
    for (index, word) in words.iter().enumerate() {
        let mut phrase = String::new();
        push_prefix_phrase(&mut phrase, word);
        let listed = word_rows
            .query_map(
                params![phrase, space, search.edited_after, page, MAX_ROWS_PER_WORD],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let mut pages: HashSet<[u8; 16]> = HashSet::new();
        for (rowid, page_bytes, score) in listed {
            let page_id = id_from(&page_bytes, "indexed page id")?;
            let wanted = !found.contains(&page_id)
                && candidates
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&page_id));
            if !wanted {
                continue;
            }
            pages.insert(page_id);
            let row = rows.entry(rowid).or_insert(RowWords {
                page: page_id,
                words: 0,
                score: 0.0,
            });
            row.words |= 1u128 << index;
            row.score += score;
        }
        if pages.is_empty() {
            return Ok(Vec::new()); // a word that no candidate page has: no page has them all
        }
        candidates = Some(pages);
    }
    let Some(candidates) = candidates else {
        return Ok(Vec::new());
    };

    // The row of each page that holds the most words.
    let mut best: HashMap<[u8; 16], BestRow> = HashMap::new();
    for (&rowid, row) in &rows {
        if !candidates.contains(&row.page) {
            continue;
        }
        let this = BestRow {
            page: row.page,
            rowid,
            words: row.words,
            score: row.score,
        };
        match best.get_mut(&row.page) {
            Some(current) if !is_better(&this, current) => {}
            Some(current) => *current = this,
            None => {
                best.insert(row.page, this);
            }
        }
    }
    let mut ranked: Vec<BestRow> = best.into_values().collect();
    ranked.sort_by(|a, b| {
        b.count()
            .cmp(&a.count())
            .then(a.score.total_cmp(&b.score))
            .then(a.page.cmp(&b.page))
            .then(a.rowid.cmp(&b.rowid))
    });
    ranked.truncate(room);

    // Read each chosen row back as a hit over the words it holds.
    let mut row_hit = conn.prepare(ROW_HIT_SQL)?;
    let mut hits = Vec::with_capacity(ranked.len());
    for chosen in &ranked {
        let mut expression = String::new();
        for (index, word) in words.iter().enumerate() {
            if (chosen.words >> index) & 1 == 1 {
                if !expression.is_empty() {
                    expression.push_str(" OR ");
                }
                push_prefix_phrase(&mut expression, word);
            }
        }
        let columns = row_hit
            .query_map(
                params![
                    expression,
                    HIGHLIGHT_OPEN_SQL,
                    HIGHLIGHT_CLOSE_SQL,
                    SNIPPET_ELLIPSIS,
                    chosen.rowid
                ],
                |row| {
                    Ok(HitColumns {
                        page_id: row.get(0)?,
                        block_id: row.get(1)?,
                        snippet: row.get(2)?,
                        score: row.get(3)?,
                        kind: row.get(4)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for hit in columns {
            hits.push(hit.decode()?);
        }
    }
    Ok(hits)
}

/// True when `this` should replace `current` as the best row of a page: more words, then the better
/// rank, then the earlier row.
fn is_better(this: &BestRow, current: &BestRow) -> bool {
    this.count()
        .cmp(&current.count())
        .then(current.score.total_cmp(&this.score))
        .then(current.rowid.cmp(&this.rowid))
        .is_gt()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::Db;

    /// The query plan SQLite reports for a statement, as one string, with every parameter bound to a
    /// dummy value (`EXPLAIN` does not run the statement).
    fn plan_of(db: &Db, sql: &str) -> String {
        let mut stmt = db
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let dummies = vec![[1u8; 16]; stmt.parameter_count()];
        let rows: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(dummies.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        rows.join(" | ")
    }

    #[test]
    fn the_statements_start_from_the_full_text_match() {
        // As for the main statement: the match drives, the content row comes by rowid, the page by key.
        let db = Db::open_in_memory().unwrap();
        let rows = plan_of(&db, WORD_ROWS_SQL);
        assert!(
            rows.starts_with("SCAN fts VIRTUAL TABLE INDEX"),
            "the match drives: {rows}"
        );
        assert!(
            rows.contains("SEARCH b USING INTEGER PRIMARY KEY (rowid=?)"),
            "{rows}"
        );
        assert!(
            rows.contains("SEARCH p USING INDEX sqlite_autoindex_pages_index_1 (page_id=?)"),
            "{rows}"
        );
        // One row, read back by its id: no pass over the whole match.
        let one = plan_of(&db, ROW_HIT_SQL);
        assert!(one.starts_with("SCAN fts VIRTUAL TABLE INDEX"), "{one}");
        assert!(
            one.contains("SEARCH b USING INTEGER PRIMARY KEY (rowid=?)"),
            "{one}"
        );
    }
}
