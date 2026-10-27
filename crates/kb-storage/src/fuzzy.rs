//! Fuzzy matching of page titles (ADR-0040, item 9): what the search adds after the full-text
//! hits when they do not fill the answer, so that "mtg" finds "Meeting notes" and "meetign" finds
//! it too.
//!
//! A pure function over a query and a title, with no database in it: the core reads the live titles
//! of the pages and hands them to [`rank_fuzzy_titles`]. The TypeScript mirror of the core
//! (`packages/core-client/src/search-fuzzy.ts`) is the same algorithm, held to the same results
//! by the shared fixture `fixtures/search/fuzzy-titles-v1.json` (written by the test of this module,
//! read by the mirror's).
//!
//! # What matches
//!
//! Query and title are folded the way the full-text index folds them: lower case, accents removed
//! (a table of the Latin letters stands in for the canonical decomposition, so `é` is `e`), and cut
//! into words at everything that is not a letter, a digit or a combining mark. A title matches when
//! one of two things holds.
//!
//! - **Letters in order.** The letters of the query (its words joined) appear in the title as a
//!   subsequence: `rcp` in "Recipes", `mtg nts` in "Meeting notes". The best alignment is chosen
//!   (see the score).
//! - **Words with one typo.** Every query word matches the start of some word of the title: exactly,
//!   or, for a query word of 4 or more characters, with one edit (a letter changed, added, left out
//!   or swapped with its neighbour) against a prefix of the title word: `meetign`, `meeteng` and
//!   `meetting` find "Meeting", `mneu` finds "Menu".
//!
//! When both hold, the words reading is the one used (it explains the match best: its highlights are
//! the start of whole words, and `meetign` is "Meeting", not an m, an e, ... scattered over "Meeting
//! notes"). A query of fewer than [`MIN_FUZZY_LETTERS`] letters matches nothing: one letter is in every
//! second title, and the full-text search already finds the words that start with it.
//!
//! # The score
//!
//! Higher is better. A matched letter is worth 1, 5 more at the start of a word, and each letter
//! that directly follows the previous matched one inside a word is worth 4 more (contiguous runs).
//! A word matched with a typo scores as the same run minus [`TYPO_PENALTY`]. [`rank_fuzzy_titles`]
//! orders by score, then by the shorter title, then by the order the titles came in.

use std::ops::Range;
use std::sync::OnceLock;

use crate::text::query_words;

/// Fewer letters than this in the query and nothing is matched (see the module docs).
pub const MIN_FUZZY_LETTERS: usize = 2;
/// A query word needs at least this many characters to be matched with one typo.
pub const TYPO_MIN_CHARS: usize = 4;
/// Only the first this many letters of a title are looked at (titles are short; this bounds the work).
const MAX_TITLE_LETTERS: usize = 400;

/// Score of any matched letter.
const SCORE_LETTER: i32 = 1;
/// Extra score of a matched letter that starts a word.
const SCORE_WORD_START: i32 = 5;
/// Extra score of a matched letter that directly follows the previous matched one in the same word.
const SCORE_CONTIGUOUS: i32 = 4;
/// What a word matched with a typo loses against the same word matched exactly.
pub const TYPO_PENALTY: i32 = 6;

/// "No alignment" in the table of the letters-in-order search.
const NONE: i32 = i32::MIN;

/// Accented lower-case Latin letters, grouped by the letter they fold to: the characters of
/// U+00C0..U+024F and U+1E00..U+1EFF whose canonical decomposition is a base letter and marks
/// (generated from `String.prototype.normalize("NFD")`; the letters with no such decomposition,
/// such as `ø`, `ł` and `đ`, stay as they are).
const FOLDS: [(char, &str); 25] = [
    ('a', "àáâãäåāăąǎǟǡǻȁȃȧḁạảấầẩẫậắằẳẵặ"),
    ('b', "ḃḅḇ"),
    ('c', "çćĉċčḉ"),
    ('d', "ďḋḍḏḑḓ"),
    ('e', "èéêëēĕėęěȅȇȩḕḗḙḛḝẹẻẽếềểễệ"),
    ('f', "ḟ"),
    ('g', "ĝğġģǧǵḡ"),
    ('h', "ĥȟḣḥḧḩḫẖ"),
    ('i', "ìíîïĩīĭįǐȉȋḭḯỉị"),
    ('j', "ĵǰ"),
    ('k', "ķǩḱḳḵ"),
    ('l', "ĺļľḷḹḻḽ"),
    ('m', "ḿṁṃ"),
    ('n', "ñńņňǹṅṇṉṋ"),
    ('o', "òóôõöōŏőơǒǫǭȍȏȫȭȯȱṍṏṑṓọỏốồổỗộớờởỡợ"),
    ('p', "ṕṗ"),
    ('r', "ŕŗřȑȓṙṛṝṟ"),
    ('s', "śŝşšșṡṣṥṧṩ"),
    ('t', "ţťțṫṭṯṱẗ"),
    ('u', "ùúûüũūŭůűųưǔǖǘǚǜȕȗṳṵṷṹṻụủứừửữự"),
    ('v', "ṽṿ"),
    ('w', "ŵẁẃẅẇẉẘ"),
    ('x', "ẋẍ"),
    ('y', "ýÿŷȳẏẙỳỵỷỹ"),
    ('z', "źżžẑẓẕ"),
];

/// A combining mark (the ranges of the diacritical marks): folded away.
fn is_combining_mark(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F
    )
}

/// A private-use character (the full-text tokenizer counts them as word characters).
fn is_private_use(c: char) -> bool {
    matches!(
        u32::from(c),
        0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD
    )
}

/// A character that belongs to a word: a letter, a digit, a combining mark or a private-use one.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_combining_mark(c) || is_private_use(c)
}

/// The base letter of an accented Latin letter, or the character itself.
fn base_letter(c: char) -> char {
    if c.is_ascii() {
        return c;
    }
    static TABLE: OnceLock<Vec<(char, char)>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut pairs: Vec<(char, char)> = FOLDS
            .iter()
            .flat_map(|&(base, group)| group.chars().map(move |accented| (accented, base)))
            .collect();
        pairs.sort_unstable();
        pairs
    });
    table
        .binary_search_by_key(&c, |&(accented, _)| accented)
        .ok()
        .and_then(|at| table.get(at))
        .map_or(c, |&(_, base)| base)
}

/// Appends the folded form of one character: its lower case with the accents removed. A combining
/// mark folds to nothing.
fn fold_char(c: char, out: &mut Vec<char>) {
    for lower in c.to_lowercase() {
        if !is_combining_mark(lower) {
            out.push(base_letter(lower));
        }
    }
}

/// One folded letter of a text, with where the character it came from sits in the text.
#[derive(Debug, Clone, Copy)]
struct Letter {
    ch: char,
    /// First UTF-16 code unit of the original character.
    start: u32,
    /// One past its last UTF-16 code unit.
    end: u32,
    /// The first letter of a word.
    word_start: bool,
}

/// A text folded into letters and cut into words.
struct Folded {
    letters: Vec<Letter>,
    /// The words, as ranges of `letters`.
    words: Vec<Range<usize>>,
}

fn fold_text(text: &str) -> Folded {
    let mut letters: Vec<Letter> = Vec::new();
    let mut words: Vec<Range<usize>> = Vec::new();
    let mut folded: Vec<char> = Vec::new();
    let mut offset: u32 = 0;
    let mut word_from: Option<usize> = None;
    for c in text.chars() {
        let units = u32::try_from(c.len_utf16()).unwrap_or(1);
        if is_word_char(c) {
            let from = *word_from.get_or_insert(letters.len());
            folded.clear();
            fold_char(c, &mut folded);
            for &ch in &folded {
                letters.push(Letter {
                    ch,
                    start: offset,
                    end: offset.saturating_add(units),
                    word_start: letters.len() == from,
                });
            }
        } else if let Some(from) = word_from.take()
            && from < letters.len()
        {
            words.push(from..letters.len());
        }
        offset = offset.saturating_add(units);
    }
    if let Some(from) = word_from
        && from < letters.len()
    {
        words.push(from..letters.len());
    }
    if letters.len() > MAX_TITLE_LETTERS {
        letters.truncate(MAX_TITLE_LETTERS);
        words.retain(|word| word.start < MAX_TITLE_LETTERS);
        for word in &mut words {
            word.end = word.end.min(MAX_TITLE_LETTERS);
        }
    }
    Folded { letters, words }
}

/// A search query prepared for matching titles: its words folded, with the same cutting and the same
/// dropped operators as the full-text query.
#[derive(Debug, Clone)]
pub struct FuzzyQuery {
    /// Every letter of the query, in order.
    letters: Vec<char>,
    /// The words, as ranges of `letters`.
    words: Vec<Range<usize>>,
}

impl FuzzyQuery {
    /// The query as a matcher, or `None` when it has fewer than [`MIN_FUZZY_LETTERS`] letters.
    #[must_use]
    pub fn new(query: &str) -> Option<Self> {
        let folded = fold_text(&query_words(query, false).join(" "));
        if folded.letters.len() < MIN_FUZZY_LETTERS {
            return None;
        }
        Some(Self {
            letters: folded.letters.iter().map(|letter| letter.ch).collect(),
            words: folded.words,
        })
    }

    fn word(&self, word: &Range<usize>) -> &[char] {
        self.letters.get(word.clone()).unwrap_or_default()
    }
}

/// How a title matched a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// The score (higher is better, see the module docs).
    pub score: i32,
    /// The matched parts of the title as UTF-16 code unit ranges `(start, end)`: sorted, not
    /// overlapping, none empty. The letters matched in order, merged where they touch; or the
    /// start of each title word that matched a query word.
    pub highlights: Vec<(u32, u32)>,
}

/// The score of a run of `length` letters that starts a word and runs on without a break.
fn run_score(length: usize) -> i32 {
    let length = i32::try_from(length).unwrap_or(i32::MAX / 16);
    SCORE_WORD_START + SCORE_LETTER * length + SCORE_CONTIGUOUS * (length - 1).max(0)
}

/// The best way to find `query` as a subsequence of `title`: its score and the position in `title` of
/// each query letter. Best is the highest score; on a tie the earliest alignment.
fn best_subsequence(query: &[char], title: &[Letter]) -> Option<(i32, Vec<usize>)> {
    let (m, n) = (query.len(), title.len());
    if m == 0 || n < m {
        return None;
    }
    // Cheap test first: most titles are not a match at all.
    let mut next = 0;
    for letter in title {
        if query.get(next) == Some(&letter.ch) {
            next += 1;
        }
    }
    if next < m {
        return None;
    }
    // best[i * n + j]: the best score with query letter i matched to title letter j.
    let mut best = vec![NONE; m * n];
    let mut from = vec![usize::MAX; m * n];
    for (i, &wanted) in query.iter().enumerate() {
        // The best earlier alignment of letter i - 1 strictly before j: its score and position.
        let mut before: Option<(i32, usize)> = None;
        for (j, letter) in title.iter().enumerate() {
            if i > 0 && j > 0 {
                let previous = best[(i - 1) * n + j - 1];
                if previous != NONE && before.is_none_or(|(value, _)| previous > value) {
                    before = Some((previous, j - 1));
                }
            }
            if letter.ch != wanted {
                continue;
            }
            let base = SCORE_LETTER
                + if letter.word_start {
                    SCORE_WORD_START
                } else {
                    0
                };
            if i == 0 {
                best[j] = base;
                continue;
            }
            let mut chosen = before;
            if j > 0 && !letter.word_start {
                let previous = best[(i - 1) * n + j - 1];
                if previous != NONE {
                    let run = previous + SCORE_CONTIGUOUS;
                    if chosen.is_none_or(|(value, _)| run > value) {
                        chosen = Some((run, j - 1));
                    }
                }
            }
            if let Some((value, at)) = chosen {
                best[i * n + j] = base + value;
                from[i * n + j] = at;
            }
        }
    }
    let last = (m - 1) * n;
    let mut end: Option<(i32, usize)> = None;
    for j in 0..n {
        let value = best[last + j];
        if value != NONE && end.is_none_or(|(top, _)| value > top) {
            end = Some((value, j));
        }
    }
    let (score, mut at) = end?;
    let mut positions = vec![0; m];
    for i in (0..m).rev() {
        positions[i] = at;
        if i > 0 {
            at = from[i * n + at];
        }
    }
    Some((score, positions))
}

/// True when `a` and `b` are equal or one edit apart: a letter changed, one added or left out, or two
/// neighbours swapped.
fn within_one_edit(a: &[char], b: &[char]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let common = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if common == a.len() && common == b.len() {
        return true;
    }
    let rest = |text: &[char], skip: usize| text.get(skip..).map(<[char]>::to_vec);
    if a.len() == b.len() {
        if rest(a, common + 1) == rest(b, common + 1) {
            return true; // one letter changed
        }
        let swapped = a.get(common) == b.get(common + 1)
            && a.get(common + 1) == b.get(common)
            && a.get(common + 2..) == b.get(common + 2..);
        return swapped;
    }
    if a.len() < b.len() {
        rest(a, common) == rest(b, common + 1) // one letter added to b
    } else {
        rest(a, common + 1) == rest(b, common) // one letter left out of b
    }
}

/// Where query word `word` matches the start of a title word: a range of title letters and the
/// score. An exact prefix of any title word wins over a typo (4+ characters only) against a prefix.
fn match_word(word: &[char], title: &Folded) -> Option<(Range<usize>, i32)> {
    for title_word in &title.words {
        let letters = title.letters.get(title_word.clone())?;
        let prefix = letters.get(..word.len());
        if prefix.is_some_and(|prefix| prefix.iter().map(|l| l.ch).eq(word.iter().copied())) {
            let end = title_word.start + word.len();
            return Some((title_word.start..end, run_score(word.len())));
        }
    }
    if word.len() < TYPO_MIN_CHARS {
        return None;
    }
    for title_word in &title.words {
        let letters: Vec<char> = title
            .letters
            .get(title_word.clone())?
            .iter()
            .map(|l| l.ch)
            .collect();
        for length in [word.len(), word.len() - 1, word.len() + 1] {
            if let Some(prefix) = letters.get(..length)
                && within_one_edit(word, prefix)
            {
                let end = title_word.start + length;
                return Some((title_word.start..end, run_score(length) - TYPO_PENALTY));
            }
        }
    }
    None
}

/// The UTF-16 range of title letters `from..to`.
fn units_of(title: &Folded, range: &Range<usize>) -> Option<(u32, u32)> {
    let first = title.letters.get(range.start)?;
    let last = title.letters.get(range.end.checked_sub(1)?)?;
    Some((first.start, last.end))
}

/// Merges ranges that touch or overlap; the input is sorted by start.
fn merge(ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// The reading of a match as words: every query word is the start of some title word, exactly or with
/// one typo.
fn match_as_words(query: &FuzzyQuery, title: &Folded) -> Option<FuzzyMatch> {
    let mut score = 0;
    let mut ranges = Vec::new();
    for word in &query.words {
        let (matched, gained) = match_word(query.word(word), title)?;
        score += gained;
        ranges.push(units_of(title, &matched)?);
    }
    ranges.sort_unstable();
    Some(FuzzyMatch {
        score,
        highlights: merge(ranges),
    })
}

/// The reading of a match as the query's letters found in order in the title.
fn match_as_letters(query: &FuzzyQuery, title: &Folded) -> Option<FuzzyMatch> {
    let (score, positions) = best_subsequence(&query.letters, &title.letters)?;
    let ranges = positions
        .iter()
        .filter_map(|&at| {
            title
                .letters
                .get(at)
                .map(|letter| (letter.start, letter.end))
        })
        .collect();
    Some(FuzzyMatch {
        score,
        highlights: merge(ranges),
    })
}

/// How `title` matches `query`, or `None`. See the module docs for what matches and the score: the
/// words reading when it holds (its highlights are whole word starts), else the letters in order.
#[must_use]
pub fn fuzzy_title(query: &FuzzyQuery, title: &str) -> Option<FuzzyMatch> {
    let folded = fold_text(title);
    match_as_words(query, &folded).or_else(|| match_as_letters(query, &folded))
}

/// The titles that match `query`, best first, at most `limit`: by score, then by the shorter title
/// (in characters), then in the order the titles came in. Each is returned with its key and how it
/// matched. A query with fewer than [`MIN_FUZZY_LETTERS`] letters matches nothing.
#[must_use]
pub fn rank_fuzzy_titles<'a, K>(
    query: &str,
    titles: impl IntoIterator<Item = (K, &'a str)>,
    limit: usize,
) -> Vec<(K, FuzzyMatch)> {
    let Some(query) = FuzzyQuery::new(query) else {
        return Vec::new();
    };
    let mut found: Vec<(K, FuzzyMatch, usize)> = titles
        .into_iter()
        .filter_map(|(key, title)| {
            let matched = fuzzy_title(&query, title)?;
            Some((key, matched, title.chars().count()))
        })
        .collect();
    found.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.2.cmp(&b.2)));
    found.truncate(limit);
    found
        .into_iter()
        .map(|(key, matched, _)| (key, matched))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn matched(query: &str, title: &str) -> Option<FuzzyMatch> {
        fuzzy_title(&FuzzyQuery::new(query)?, title)
    }

    /// The highlighted parts of a title, cut by UTF-16 offsets as a JavaScript client does.
    fn parts(title: &str, found: &FuzzyMatch) -> Vec<String> {
        let units: Vec<u16> = title.encode_utf16().collect();
        found
            .highlights
            .iter()
            .map(|&(start, end)| String::from_utf16(&units[start as usize..end as usize]).unwrap())
            .collect()
    }

    #[test]
    fn letters_in_order_find_a_title() {
        let found = matched("rcp", "Recipes").unwrap();
        assert_eq!(parts("Recipes", &found), ["R", "c", "p"]);
        // r at a word start 1 + 5, c and p scattered 1 + 1.
        assert_eq!(found.score, 8);

        let found = matched("mtg", "Meeting notes").unwrap();
        assert_eq!(parts("Meeting notes", &found), ["M", "t", "g"]);

        assert_eq!(matched("gm", "Meeting"), None, "the order matters");
        assert_eq!(matched("xyz", "Meeting"), None);
        assert_eq!(
            matched("meetingsx", "Meeting"),
            None,
            "more letters than the title has, and two edits from it"
        );
    }

    #[test]
    fn the_words_of_a_query_are_one_run_of_letters() {
        let found = matched("mtg nts", "Meeting notes").unwrap();
        assert_eq!(
            parts("Meeting notes", &found),
            ["M", "t", "g", "n", "t", "s"]
        );
        // Letters only: the spaces and punctuation of the query do not count.
        assert_eq!(matched("m-t-g", "Meeting"), matched("mtg", "Meeting"));
        // Whole words of the query and the title still work.
        assert!(matched("meeting notes", "Meeting notes").is_some());
    }

    #[test]
    fn contiguous_runs_and_word_starts_score_higher() {
        let contiguous = matched("note", "Notes").unwrap();
        let scattered = matched("note", "Nxoxtxex").unwrap();
        assert!(
            contiguous.score > scattered.score,
            "{contiguous:?} {scattered:?}"
        );
        // Same letters, but the second title has them at word starts.
        let starts = matched("np", "New plan").unwrap();
        let inside = matched("np", "Snap").unwrap();
        assert!(starts.score > inside.score, "{starts:?} {inside:?}");
        // The best alignment is used, not the first: the word start of "plan" wins.
        let found = matched("np", "Another plan").unwrap();
        assert_eq!(parts("Another plan", &found), ["n", "p"]);
        let found = matched("pl", "Open plan").unwrap();
        assert_eq!(parts("Open plan", &found), ["pl"], "a run, at a word start");
    }

    #[test]
    fn a_contiguous_run_does_not_cross_a_word_break() {
        // The "n" that starts a word earns the word-start bonus, not the bonus of a run: 1 + (1 + 5).
        let across = matched("gn", "Meeting notes").unwrap();
        assert_eq!(across.score, 7);
        assert_eq!(
            across.highlights,
            [(6, 7), (8, 9)],
            "two ranges, the space between"
        );
        // Inside a word the second letter earns the run bonus instead: 1 + (1 + 4).
        assert_eq!(matched("gn", "Signal").unwrap().score, 6);
    }

    #[test]
    fn one_typo_is_tolerated_in_a_word_of_four_or_more_characters() {
        // Swapped neighbours, a letter changed, a letter added, a letter left out: the word of the
        // title is highlighted.
        for query in ["meetign", "meeteng", "meetting", "meetng", "meetings"] {
            let found = matched(query, "Meeting notes").unwrap_or_else(|| panic!("{query}"));
            assert_eq!(parts("Meeting notes", &found), ["Meeting"], "{query}");
        }
        let found = matched("mneu", "Menu").unwrap();
        assert_eq!(parts("Menu", &found), ["Menu"]);
        let found = matched("emnu", "Menu").unwrap();
        assert_eq!(parts("Menu", &found), ["Menu"]);
        // Two typos are too many; a short word has no tolerance.
        assert_eq!(matched("mneux", "Menu"), None);
        assert_eq!(matched("mnue", "Menu"), None);
        assert_eq!(matched("mxn", "Menu"), None, "three characters: no typo");
    }

    #[test]
    fn a_typo_is_matched_against_a_prefix_of_a_title_word() {
        // "agend" is a prefix of "agenda" with a letter changed.
        let found = matched("agxnd", "Agenda for monday").unwrap();
        assert_eq!(parts("Agenda for monday", &found), ["Agend"]);
        // Both words of the query have to match something.
        assert!(matched("meetign notes", "Meeting notes").is_some());
        assert_eq!(matched("meetign zebra", "Meeting notes"), None);
    }

    #[test]
    fn a_typo_scores_less_than_the_same_word_typed_right() {
        let right = matched("meeting", "Meeting").unwrap();
        let typo = matched("meetign", "Meeting").unwrap();
        assert!(right.score > typo.score, "{right:?} {typo:?}");
    }

    #[test]
    fn case_and_accents_are_folded() {
        let found = matched("cafe", "Caf\u{e9} noir").unwrap();
        assert_eq!(parts("Caf\u{e9} noir", &found), ["Caf\u{e9}"]);
        assert!(matched("caf\u{e9}", "CAFE").is_some());
        assert!(matched("resume", "R\u{c9}SUM\u{c9}").is_some());
        // A combining mark of a decomposed title folds away too.
        assert!(matched("cafe", "cafe\u{301}").is_some());
        assert!(matched("nino", "Ni\u{f1}o").is_some());
        // A letter with no decomposition stays itself.
        assert_eq!(matched("bro", "Br\u{f8}"), None);
    }

    #[test]
    fn highlights_are_utf16_ranges_of_the_original_title() {
        let title = "\u{1F600} Recipes";
        let found = matched("rcp", title).unwrap();
        // The emoji is two code units and the space one: the matches start at 3.
        assert_eq!(found.highlights, [(3, 4), (5, 6), (7, 8)]);
        assert_eq!(parts(title, &found), ["R", "c", "p"]);
        // An accented letter is one unit: its range covers the original character.
        let found = matched("cafe", "Caf\u{e9}").unwrap();
        assert_eq!(found.highlights, [(0, 4)]);
    }

    #[test]
    fn short_queries_and_nothing_to_match_give_no_match() {
        assert!(FuzzyQuery::new("").is_none());
        assert!(FuzzyQuery::new("a").is_none(), "one letter is too few");
        assert!(FuzzyQuery::new("!?").is_none());
        assert!(
            FuzzyQuery::new("ANDROID").is_some(),
            "a word that merely starts like an operator"
        );
        assert!(
            FuzzyQuery::new("AND OR").is_none(),
            "bare operators are dropped"
        );
        assert!(FuzzyQuery::new("ab").is_some());
        assert_eq!(matched("ab", ""), None, "an untitled page");
        assert_eq!(matched("ab", "!!!"), None);
    }

    #[test]
    fn digits_and_unicode_words_are_letters() {
        assert!(matched("q4", "Q4 2026 plan").is_some());
        assert!(matched("2026", "Plan 2026").is_some());
        let title = "\u{65e5}\u{672c}\u{8a9e}";
        assert_eq!(
            matched("\u{65e5}\u{8a9e}", title).map(|f| f.highlights),
            Some(vec![(0, 1), (2, 3)])
        );
    }

    #[test]
    fn very_long_titles_are_cut() {
        let title = format!("{} z", "a".repeat(MAX_TITLE_LETTERS + 50));
        assert_eq!(
            matched("az", &title),
            None,
            "the end is beyond what is looked at"
        );
        assert!(matched("aa", &title).is_some());
    }

    #[test]
    fn one_edit_between_words() {
        let edit = |a: &str, b: &str| {
            let a: Vec<char> = a.chars().collect();
            let b: Vec<char> = b.chars().collect();
            (within_one_edit(&a, &b), within_one_edit(&b, &a))
        };
        assert_eq!(edit("abcd", "abcd"), (true, true));
        assert_eq!(edit("abcd", "abxd"), (true, true));
        assert_eq!(edit("abcd", "abd"), (true, true));
        assert_eq!(edit("abcd", "abcde"), (true, true));
        assert_eq!(edit("abcd", "bacd"), (true, true));
        assert_eq!(edit("abcd", "abdc"), (true, true));
        assert_eq!(edit("abcd", "xbcd"), (true, true));
        assert_eq!(edit("abcd", "abcx"), (true, true));
        assert_eq!(edit("abcd", "bcd"), (true, true));
        assert_eq!(edit("abcd", "dcba"), (false, false));
        assert_eq!(edit("abcd", "axyd"), (false, false));
        assert_eq!(edit("abcd", "ab"), (false, false));
        assert_eq!(edit("abcd", "acbe"), (false, false));
        assert_eq!(edit("", "a"), (true, true));
    }

    #[test]
    fn ranking_orders_by_score_then_shorter_titles_then_input_order() {
        let titles = [
            (0, "Scattered notes of every kind"),
            (1, "Notes"),
            (2, "Notes again"),
            (3, "Another one"),
            (4, "Notes!"),
        ];
        let ranked = rank_fuzzy_titles("note", titles.iter().map(|&(k, t)| (k, t)), 10);
        let keys: Vec<i32> = ranked.iter().map(|(key, _)| *key).collect();
        // Titles 1, 4, 2 and 0 have a word that starts with "note" (the same score): the shorter title
        // first. Title 3 only has the letters, spread over "Another".
        assert_eq!(keys, [1, 4, 2, 0, 3]);
        // Same score and same length keep the order they came in.
        let tied = rank_fuzzy_titles("note", [(7, "Notes"), (5, "Notes"), (6, "Notes")], 10);
        assert_eq!(tied.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [7, 5, 6]);
        assert_eq!(
            rank_fuzzy_titles("note", titles.iter().map(|&(k, t)| (k, t)), 2).len(),
            2
        );
        assert!(rank_fuzzy_titles("n", titles.iter().map(|&(k, t)| (k, t)), 2).is_empty());
    }

    /// The cases the TypeScript mirror is held to: `fixtures/search/fuzzy-titles-v1.json`. The test
    /// compares the file with what this module computes, so a change of the algorithm fails here until
    /// the file is written again (`KB_WRITE_FIXTURES=1 cargo test -p kb-storage fuzzy_fixture`) and the
    /// mirror follows.
    const MATCH_CASES: &[(&str, &str)] = &[
        ("rcp", "Recipes"),
        ("rcp", "Recipes and more"),
        ("mtg", "Meeting notes"),
        ("mtg nts", "Meeting notes"),
        ("gm", "Meeting"),
        ("meetign", "Meeting notes"),
        ("meeteng", "Meeting notes"),
        ("meetting", "Meeting notes"),
        ("meetng", "Meeting notes"),
        ("mneu", "Menu"),
        ("mnue", "Menu"),
        ("mneux", "Menu"),
        ("mxn", "Menu"),
        ("agxnd", "Agenda for monday"),
        ("meetign notes", "Meeting notes"),
        ("meetign zebra", "Meeting notes"),
        ("note", "Notes"),
        ("note", "Nxoxtxex"),
        ("np", "New plan"),
        ("np", "Snap"),
        ("np", "Another plan"),
        ("pl", "Open plan"),
        ("gn", "Meeting notes"),
        ("gn", "Signal"),
        ("cafe", "Caf\u{e9} noir"),
        ("caf\u{e9}", "CAFE"),
        ("resume", "R\u{c9}SUM\u{c9}"),
        ("cafe", "cafe\u{301}"),
        ("nino", "Ni\u{f1}o"),
        ("rcp", "\u{1F600} Recipes"),
        ("q4", "Q4 2026 plan"),
        ("2026", "Plan 2026"),
        ("\u{65e5}\u{8a9e}", "\u{65e5}\u{672c}\u{8a9e}"),
        ("a", "Anything"),
        ("ab", ""),
        ("ab", "!!!"),
        ("AND OR", "Android"),
        ("andr", "Android"),
        ("meet-ing", "Meeting"),
        ("pln", "Project plan notes"),
        ("pln", "Open plan"),
        ("budget", "Q3 budget review"),
        ("bugdet", "Q3 budget review"),
        ("review budget", "Q3 budget review"),
        ("xyz", "Meeting"),
    ];

    /// Titles ranked for a query: the keys are the positions in the list.
    const RANK_CASES: &[(&str, &[&str])] = &[
        (
            "note",
            &[
                "Scattered notes of every kind",
                "Notes",
                "Notes again",
                "Another one",
                "Notes!",
                "Nxoxtxex",
            ],
        ),
        (
            "mtg",
            &[
                "Meeting",
                "My team goals",
                "Meeting notes",
                "Mixed everything tags inventory",
                "Notes",
            ],
        ),
        (
            "rcp",
            &["Recipes", "Recipe book", "Rec", "Wrap chips", "R c p"],
        ),
    ];

    /// The text of the fixture: one case to a line.
    fn fixture_text() -> String {
        let lines = |cases: Vec<serde_json::Value>| {
            cases
                .iter()
                .map(|case| format!("    {}", serde_json::to_string(case).unwrap()))
                .collect::<Vec<_>>()
                .join(
                    ",
",
                )
        };
        let matches: Vec<serde_json::Value> = MATCH_CASES
            .iter()
            .map(|&(query, title)| {
                let found = matched(query, title);
                serde_json::json!({
                    "query": query,
                    "title": title,
                    "match": found.map(|f| serde_json::json!({
                        "score": f.score,
                        "highlights": f.highlights.iter().map(|&(s, e)| [s, e]).collect::<Vec<_>>(),
                    })),
                })
            })
            .collect();
        let rankings: Vec<serde_json::Value> = RANK_CASES
            .iter()
            .map(|&(query, titles)| {
                let ranked = rank_fuzzy_titles(query, titles.iter().copied().enumerate(), 100);
                serde_json::json!({
                    "query": query,
                    "titles": titles,
                    "ranked": ranked.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
                })
            })
            .collect();
        format!(
            "{{
  \"comment\": \"Written by the test fuzzy_fixture_is_current of kb-storage (crates/kb-storage/src/fuzzy.rs); read by packages/core-client/src/search-fuzzy.test.ts. Do not edit by hand.\",
  \"matches\": [
{}
  ],
  \"rankings\": [
{}
  ]
}}
",
            lines(matches),
            lines(rankings)
        )
    }

    #[test]
    fn fuzzy_fixture_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/search/fuzzy-titles-v1.json");
        let text = fixture_text();
        if std::env::var_os("KB_WRITE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("{}: {e}; write it with KB_WRITE_FIXTURES=1", path.display())
        });
        assert_eq!(
            on_disk.replace("\r\n", "\n"),
            text,
            "fixtures/search/fuzzy-titles-v1.json is stale: KB_WRITE_FIXTURES=1 cargo test -p kb-storage fuzzy_fixture"
        );
        // The file is JSON.
        serde_json::from_str::<serde_json::Value>(&text).unwrap();
    }

    #[test]
    fn the_fixture_cases_say_what_they_should() {
        // A few of the vectors, spelled out, so the fixture is not only a snapshot.
        let case =
            |query: &str, title: &str| matched(query, title).map(|f| (f.score, f.highlights));
        assert_eq!(
            case("rcp", "Recipes"),
            Some((8, vec![(0, 1), (2, 3), (4, 5)]))
        );
        assert_eq!(case("note", "Notes"), Some((run_score(4), vec![(0, 4)])));
        assert_eq!(case("gm", "Meeting"), None);
        assert_eq!(case("a", "Anything"), None);
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn ten_thousand_titles_are_ranked_without_trouble() {
        // Every page of a large workspace is a candidate: titles of a few words, most of them no match.
        let titles: Vec<String> = (0..10_000)
            .map(|i| {
                format!(
                    "Project {i} notes on the quarterly review of item {}",
                    i * 7
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let ranked = rank_fuzzy_titles(
            "qrtly 77",
            titles
                .iter()
                .enumerate()
                .map(|(i, title)| (i, title.as_str())),
            30,
        );
        eprintln!("10,000 titles ranked in {:?}", started.elapsed());
        assert_eq!(ranked.len(), 30);
        assert!(
            ranked
                .windows(2)
                .all(|pair| pair[0].1.score >= pair[1].1.score)
        );
        let none = rank_fuzzy_titles(
            "zzzz",
            titles
                .iter()
                .enumerate()
                .map(|(i, title)| (i, title.as_str())),
            30,
        );
        assert!(none.is_empty());
    }

    #[test]
    fn the_fold_table_covers_every_letter_once() {
        let mut seen = std::collections::BTreeSet::new();
        for (base, group) in FOLDS {
            assert!(base.is_ascii_lowercase());
            for c in group.chars() {
                assert!(seen.insert(c), "{c} is listed twice");
                assert!(!c.is_ascii());
                assert_eq!(base_letter(c), base);
            }
        }
        assert_eq!(base_letter('a'), 'a');
        assert_eq!(base_letter('\u{f8}'), '\u{f8}');
    }
}
