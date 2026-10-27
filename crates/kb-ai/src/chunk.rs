//! Chunking a page for the semantic index (`docs/architecture/ai.md`, section 3.3).
//!
//! All lengths count characters (`chars()`), and text is only ever cut on character boundaries.

use kb_types::BlockId;

use crate::util::{byte_index_of_char, char_len, longer_than, truncate_chars};

/// Target length of a chunk, characters.
pub const CHUNK_TARGET_CHARS: usize = 1_000;
/// A block longer than this is split at sentence ends; the hard cut length.
pub const CHUNK_MAX_CHARS: usize = 1_500;
/// The title line is truncated to this many characters.
pub const TITLE_MAX_CHARS: usize = 200;
/// Most chunks of one page; the rest are dropped.
pub const MAX_CHUNKS_PER_PAGE: usize = 400;

/// One chunk of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position in the page, from 0.
    pub seq: u32,
    /// The blocks it covers, in order (empty: the title alone).
    pub block_ids: Vec<BlockId>,
    /// The text to embed: `"{title}\n{blocks joined with \n}"` (no model prefix).
    pub text: String,
    /// `blake3(model + "\0" + text)`.
    pub hash: [u8; 32],
}

/// Chunks a page: `blocks` are the page's body blocks in document order with their plain text.
///
/// - Blocks with no text (empty or white space only) are skipped.
/// - Consecutive blocks are joined with `\n` until adding the next one would pass
///   [`CHUNK_TARGET_CHARS`] characters; a block that alone passes it (up to
///   [`CHUNK_MAX_CHARS`]) is a chunk of its own.
/// - A block longer than [`CHUNK_MAX_CHARS`] is split at sentence ends (`. `, `? `, `! `, a new
///   line) into pieces of at most [`CHUNK_TARGET_CHARS`] characters, or hard-cut at
///   [`CHUNK_MAX_CHARS`] characters when its next [`CHUNK_TARGET_CHARS`] characters have no
///   break. Every piece keeps the block's id and is packed like a block.
/// - The embedded text is the title (white space trimmed, at most [`TITLE_MAX_CHARS`]
///   characters), a new line, and the chunk text; without a title it is the chunk text alone.
/// - A page without body text is one chunk holding the title alone (no block ids); with no title
///   either there is no chunk.
/// - At most [`MAX_CHUNKS_PER_PAGE`] chunks; the rest of the page is dropped.
#[must_use]
pub fn chunk_page(model: &str, title: &str, blocks: &[(BlockId, String)]) -> Vec<Chunk> {
    let title = truncate_chars(title.trim(), TITLE_MAX_CHARS);
    let mut packer = Packer::default();
    'blocks: for (id, text) in blocks {
        if text.trim().is_empty() {
            continue;
        }
        if longer_than(text, CHUNK_MAX_CHARS) {
            for piece in split_long_block(text) {
                packer.add(*id, &piece);
            }
        } else {
            packer.add(*id, text);
        }
        if packer.is_full() {
            break 'blocks;
        }
    }
    let bodies = packer.finish();
    if bodies.is_empty() {
        if title.is_empty() {
            return Vec::new();
        }
        return vec![make_chunk(model, 0, Vec::new(), title.to_owned())];
    }
    bodies
        .into_iter()
        .enumerate()
        .map(|(seq, (block_ids, body))| {
            let text = if title.is_empty() {
                body
            } else {
                format!("{title}\n{body}")
            };
            make_chunk(model, seq, block_ids, text)
        })
        .collect()
}

/// `blake3` over the chunk hashes in order: changes when any chunk changes.
///
/// The block ids of the chunks are not part of it (only their text and the model are).
#[must_use]
pub fn source_hash(chunks: &[Chunk]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for chunk in chunks {
        hasher.update(&chunk.hash);
    }
    *hasher.finalize().as_bytes()
}

/// The hash of a chunk's embedded text for a model: `blake3(model + "\0" + text)`.
#[must_use]
pub fn chunk_hash(model: &str, text: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(model.as_bytes());
    hasher.update(&[0]);
    hasher.update(text.as_bytes());
    *hasher.finalize().as_bytes()
}

fn make_chunk(model: &str, seq: usize, block_ids: Vec<BlockId>, text: String) -> Chunk {
    Chunk {
        seq: u32::try_from(seq).unwrap_or(u32::MAX),
        block_ids,
        hash: chunk_hash(model, &text),
        text,
    }
}

/// Packs pieces of text into chunk bodies.
#[derive(Default)]
struct Packer {
    done: Vec<(Vec<BlockId>, String)>,
    text: String,
    chars: usize,
    ids: Vec<BlockId>,
}

impl Packer {
    /// Whether the page is at its chunk limit (nothing more is kept).
    fn is_full(&self) -> bool {
        self.done.len() >= MAX_CHUNKS_PER_PAGE
    }

    /// Adds a non-blank piece of at most [`CHUNK_MAX_CHARS`] characters.
    fn add(&mut self, id: BlockId, text: &str) {
        if self.is_full() {
            return;
        }
        let length = char_len(text);
        if self.chars > 0 && self.chars + 1 + length > CHUNK_TARGET_CHARS {
            self.flush();
            if self.is_full() {
                return;
            }
        }
        if self.chars > 0 {
            self.text.push('\n');
            self.chars += 1;
        }
        self.text.push_str(text);
        self.chars += length;
        if self.ids.last() != Some(&id) {
            self.ids.push(id);
        }
    }

    fn flush(&mut self) {
        if self.chars == 0 {
            return;
        }
        let body = std::mem::take(&mut self.text);
        let ids = std::mem::take(&mut self.ids);
        self.chars = 0;
        if self.done.len() < MAX_CHUNKS_PER_PAGE {
            self.done.push((ids, body));
        }
    }

    fn finish(mut self) -> Vec<(Vec<BlockId>, String)> {
        self.flush();
        self.done
    }
}

/// Splits a block of more than [`CHUNK_MAX_CHARS`] characters into trimmed, non-empty pieces.
fn split_long_block(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while longer_than(rest, CHUNK_TARGET_CHARS) {
        let cut = sentence_cut(rest).unwrap_or_else(|| byte_index_of_char(rest, CHUNK_MAX_CHARS));
        let head = rest.get(..cut).unwrap_or(rest).trim();
        if !head.is_empty() {
            pieces.push(head.to_owned());
        }
        rest = rest.get(cut..).unwrap_or("").trim_start();
    }
    let last = rest.trim();
    if !last.is_empty() {
        pieces.push(last.to_owned());
    }
    pieces
}

/// The byte index at which to cut `text` (more than [`CHUNK_TARGET_CHARS`] characters): at the
/// last sentence end (`. `, `? `, `! ` or a new line) that leaves at most
/// [`CHUNK_TARGET_CHARS`] characters before it. The cut falls on the space or new line, so the
/// punctuation stays with the sentence.
fn sentence_cut(text: &str) -> Option<usize> {
    let mut best = None;
    let mut previous = '\0';
    for (index, (byte, c)) in text.char_indices().take(CHUNK_TARGET_CHARS + 1).enumerate() {
        if index >= 1 && ((c == ' ' && matches!(previous, '.' | '?' | '!')) || c == '\n') {
            best = Some(byte);
        }
        previous = c;
    }
    best
}

/// What an embedding is for: a stored passage or a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedRole {
    /// A chunk of a page.
    Document,
    /// A question or a search.
    Query,
}

/// The text to send to the embedding model: the prefix its family expects (`nomic-embed`:
/// `search_document: ` or `search_query: `; `qwen3-embedding`: an instruction for queries), or
/// the text unchanged. The model name is matched ignoring case.
#[must_use]
pub fn embedding_input(model: &str, text: &str, role: EmbedRole) -> String {
    let model = model.to_ascii_lowercase();
    if model.contains("nomic-embed") {
        let prefix = match role {
            EmbedRole::Document => "search_document: ",
            EmbedRole::Query => "search_query: ",
        };
        return format!("{prefix}{text}");
    }
    if model.contains("qwen3-embedding") && role == EmbedRole::Query {
        return format!(
            "Instruct: Given a question, retrieve passages of the user's notes that answer it\nQuery: {text}"
        );
    }
    text.to_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const MODEL: &str = "nomic-embed-text:latest";

    fn id(n: u8) -> BlockId {
        BlockId::from_bytes([n; 16])
    }

    fn block(n: u8, text: &str) -> (BlockId, String) {
        (id(n), text.to_owned())
    }

    fn chars(chunk: &Chunk) -> usize {
        chunk.text.chars().count()
    }

    #[test]
    fn short_blocks_are_joined_with_a_new_line_under_the_title() {
        let chunks = chunk_page(
            MODEL,
            "Title",
            &[block(1, "First."), block(2, "Second."), block(3, "Third.")],
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].seq, 0);
        assert_eq!(chunks[0].text, "Title\nFirst.\nSecond.\nThird.");
        assert_eq!(chunks[0].block_ids, vec![id(1), id(2), id(3)]);
    }

    #[test]
    fn empty_and_blank_blocks_are_skipped() {
        let chunks = chunk_page(
            MODEL,
            "T",
            &[
                block(1, ""),
                block(2, "x"),
                block(3, "  \n\t "),
                block(4, "y"),
            ],
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "T\nx\ny");
        assert_eq!(chunks[0].block_ids, vec![id(2), id(4)]);
    }

    #[test]
    fn a_chunk_may_reach_exactly_1000_characters_and_no_more() {
        let a = "a".repeat(499);
        let b = "b".repeat(500); // 499 + 1 + 500 = 1000
        let c = "c".repeat(501); // 499 + 1 + 501 = 1001
        let joined = chunk_page(MODEL, "", &[block(1, &a), block(2, &b)]);
        assert_eq!(joined.len(), 1);
        assert_eq!(chars(&joined[0]), 1000);
        let split = chunk_page(MODEL, "", &[block(1, &a), block(2, &c)]);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].text, a);
        assert_eq!(split[1].text, c);
        assert_eq!(split[1].seq, 1);
        assert_eq!(split[1].block_ids, vec![id(2)]);
    }

    #[test]
    fn the_title_does_not_count_toward_the_thousand() {
        let body = "x".repeat(990);
        let chunks = chunk_page(MODEL, &"T".repeat(200), &[block(1, &body)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chars(&chunks[0]), 200 + 1 + 990);
    }

    #[test]
    fn a_block_of_up_to_1500_characters_is_one_chunk_of_its_own() {
        let small = "s".repeat(50);
        let big = "b".repeat(1500);
        let chunks = chunk_page(
            MODEL,
            "",
            &[block(1, &small), block(2, &big), block(3, &small)],
        );
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[1].text, big);
        assert_eq!(chunks[1].block_ids, vec![id(2)]);
        assert_eq!(chunks[2].text, small);
    }

    #[test]
    fn a_long_block_is_split_at_sentence_ends_into_pieces_of_at_most_1000() {
        let sentence = "This is a sentence of moderate length. ";
        let text = sentence.repeat(100); // about 3,900 characters
        assert!(text.chars().count() > 3_000);
        let chunks = chunk_page(MODEL, "", &[block(7, text.trim_end())]);
        assert!(chunks.len() >= 4);
        for chunk in &chunks {
            assert!(chars(chunk) <= 1000, "chunk of {} characters", chars(chunk));
            assert!(chunk.text.ends_with('.'), "{:?}", chunk.text);
            assert_eq!(chunk.block_ids, vec![id(7)]);
        }
        // Nothing lost: the same words in the same order.
        let words: Vec<&str> = chunks
            .iter()
            .flat_map(|c| c.text.split_whitespace())
            .collect();
        let expected: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(words, expected);
    }

    #[test]
    fn a_long_block_without_breaks_is_hard_cut_at_1500() {
        let text = "a".repeat(4000);
        let chunks = chunk_page(MODEL, "", &[block(1, &text)]);
        assert_eq!(
            chunks.iter().map(chars).collect::<Vec<_>>(),
            vec![1500, 1500, 1000]
        );
        assert!(chunks.iter().all(|c| c.block_ids == vec![id(1)]));
    }

    #[test]
    fn new_lines_are_sentence_ends_too() {
        let line = "x".repeat(400);
        let text = format!("{line}\n{line}\n{line}\n{line}"); // 1,603 characters
        let chunks = chunk_page(MODEL, "", &[block(1, &text)]);
        // Cut at the last new line before character 1000: three lines fit (1,201 > 1000), so two.
        assert_eq!(chunks[0].text, format!("{line}\n{line}"));
        assert_eq!(chunks[1].text, format!("{line}\n{line}"));
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn questions_and_exclamations_end_sentences() {
        let part = format!("{}? ", "q".repeat(500));
        let text = format!("{part}{}! {}", "e".repeat(400), "z".repeat(900));
        let chunks = chunk_page(MODEL, "", &[block(1, &text)]);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].text.ends_with('!') || chunks[0].text.ends_with('?'));
        assert!(chunks.iter().all(|c| chars(c) <= 1500));
    }

    #[test]
    fn splitting_never_cuts_inside_a_multi_byte_character() {
        for unit in ["é", "日", "😀", "a😀"] {
            let text = unit.repeat(5000);
            let chunks = chunk_page(MODEL, "Täg", &[block(1, &text)]);
            assert!(chunks.len() >= 4, "{unit}");
            for chunk in &chunks {
                // A String is always valid UTF-8; check the length is counted in characters.
                assert!(chunk.text.chars().count() <= "Täg\n".chars().count() + 1500);
            }
            let joined: String = chunks
                .iter()
                .map(|c| c.text.strip_prefix("Täg\n").unwrap())
                .collect();
            assert_eq!(joined, text, "{unit}");
        }
    }

    #[test]
    fn characters_are_counted_not_bytes() {
        // 600 two-byte characters per block: 1,200 bytes but 600 characters.
        let a = "é".repeat(450);
        let b = "è".repeat(450);
        let chunks = chunk_page(MODEL, "", &[block(1, &a), block(2, &b)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chars(&chunks[0]), 901);
        // And the limit is on characters: 500 + 1 + 500 = 1001 splits.
        let c = "é".repeat(500);
        let d = "è".repeat(500);
        assert_eq!(
            chunk_page(MODEL, "", &[block(1, &c), block(2, &d)]).len(),
            2
        );
    }

    #[test]
    fn pieces_of_a_split_block_are_packed_with_following_blocks() {
        let sentence = "Another fairly ordinary sentence. ";
        let long = sentence.repeat(50); // 1,700 characters: split in two
        let chunks = chunk_page(MODEL, "", &[block(1, long.trim_end()), block(2, "tail")]);
        let last = chunks.last().unwrap();
        assert!(last.text.ends_with("\ntail"));
        assert_eq!(last.block_ids, vec![id(1), id(2)]);
    }

    #[test]
    fn the_title_is_truncated_to_200_characters() {
        let title = "t".repeat(300);
        let chunks = chunk_page(MODEL, &title, &[block(1, "body")]);
        assert_eq!(chunks[0].text, format!("{}\nbody", "t".repeat(200)));
        // Characters, not bytes.
        let chunks = chunk_page(MODEL, &"é".repeat(300), &[block(1, "body")]);
        assert_eq!(chunks[0].text, format!("{}\nbody", "é".repeat(200)));
    }

    #[test]
    fn a_page_without_body_is_one_chunk_with_the_title_alone() {
        for blocks in [vec![], vec![block(1, ""), block(2, "  ")]] {
            let chunks = chunk_page(MODEL, "Only a title", &blocks);
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].seq, 0);
            assert_eq!(chunks[0].text, "Only a title");
            assert!(chunks[0].block_ids.is_empty());
        }
    }

    #[test]
    fn an_empty_page_has_no_chunk() {
        assert!(chunk_page(MODEL, "", &[]).is_empty());
        assert!(chunk_page(MODEL, "  ", &[block(1, ""), block(2, " ")]).is_empty());
    }

    #[test]
    fn a_page_without_a_title_embeds_the_body_alone() {
        let chunks = chunk_page(MODEL, "", &[block(1, "just body")]);
        assert_eq!(chunks[0].text, "just body");
    }

    #[test]
    fn at_most_400_chunks_are_kept() {
        let big = "w".repeat(1200);
        let blocks: Vec<_> = (0..600u16)
            .map(|n| (BlockId::from_bytes([(n % 251) as u8; 16]), big.clone()))
            .collect();
        let chunks = chunk_page(MODEL, "Big", &blocks);
        assert_eq!(chunks.len(), MAX_CHUNKS_PER_PAGE);
        assert_eq!(chunks.last().unwrap().seq, 399);
        assert_eq!(
            chunks.iter().map(|c| c.seq).collect::<Vec<_>>(),
            (0..400).collect::<Vec<u32>>()
        );
    }

    #[test]
    fn the_chunk_cap_also_applies_to_the_pieces_of_a_huge_block() {
        let huge = "x".repeat(2_000_000);
        let chunks = chunk_page(MODEL, "", &[block(1, &huge)]);
        assert_eq!(chunks.len(), MAX_CHUNKS_PER_PAGE);
    }

    #[test]
    fn the_hash_is_blake3_of_model_nul_and_embedded_text() {
        let chunks = chunk_page(MODEL, "T", &[block(1, "body")]);
        let expected = *blake3::hash(format!("{MODEL}\0T\nbody").as_bytes()).as_bytes();
        assert_eq!(chunks[0].hash, expected);
        assert_eq!(chunks[0].hash, chunk_hash(MODEL, &chunks[0].text));
    }

    #[test]
    fn hashes_are_stable_and_depend_on_model_and_text() {
        let blocks = [block(1, "alpha"), block(2, "beta")];
        let a = chunk_page(MODEL, "T", &blocks);
        let b = chunk_page(MODEL, "T", &blocks);
        assert_eq!(a, b);
        assert_eq!(source_hash(&a), source_hash(&b));
        let other_model = chunk_page("qwen3-embedding:0.6b", "T", &blocks);
        assert_ne!(a[0].hash, other_model[0].hash);
        assert_ne!(source_hash(&a), source_hash(&other_model));
        let edited = chunk_page(MODEL, "T", &[block(1, "alpha"), block(2, "beta!")]);
        assert_ne!(a[0].hash, edited[0].hash);
        assert_ne!(source_hash(&a), source_hash(&edited));
        let retitled = chunk_page(MODEL, "T2", &blocks);
        assert_ne!(a[0].hash, retitled[0].hash);
    }

    #[test]
    fn the_hash_ignores_block_ids() {
        let a = chunk_page(MODEL, "T", &[block(1, "same")]);
        let b = chunk_page(MODEL, "T", &[block(9, "same")]);
        assert_eq!(a[0].hash, b[0].hash);
        assert_ne!(a[0].block_ids, b[0].block_ids);
    }

    #[test]
    fn the_source_hash_covers_every_chunk_in_order() {
        let long = "k".repeat(1200);
        let two = chunk_page(MODEL, "", &[block(1, &long), block(2, &long)]);
        assert_eq!(two.len(), 2);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&two[0].hash);
        hasher.update(&two[1].hash);
        assert_eq!(source_hash(&two), *hasher.finalize().as_bytes());
        // The same chunks in another order hash differently when their hashes differ.
        let other = chunk_page(MODEL, "", &[block(1, &long), block(2, &"j".repeat(1200))]);
        let mut reversed = other.clone();
        reversed.reverse();
        assert_ne!(source_hash(&other), source_hash(&reversed));
        // The hash of nothing is a fixed value, not a failure.
        assert_eq!(
            source_hash(&[]),
            *blake3::Hasher::new().finalize().as_bytes()
        );
    }

    #[test]
    fn nomic_models_get_search_prefixes() {
        assert_eq!(
            embedding_input("nomic-embed-text:latest", "hello", EmbedRole::Document),
            "search_document: hello"
        );
        assert_eq!(
            embedding_input("nomic-embed-text", "hello", EmbedRole::Query),
            "search_query: hello"
        );
        assert_eq!(
            embedding_input("Nomic-Embed-Text:v1.5", "hi", EmbedRole::Query),
            "search_query: hi"
        );
    }

    #[test]
    fn qwen3_embedding_gets_an_instruction_for_queries_only() {
        assert_eq!(
            embedding_input("qwen3-embedding:0.6b", "what is x?", EmbedRole::Query),
            "Instruct: Given a question, retrieve passages of the user's notes that answer it\nQuery: what is x?"
        );
        assert_eq!(
            embedding_input("qwen3-embedding:0.6b", "a passage", EmbedRole::Document),
            "a passage"
        );
    }

    #[test]
    fn other_models_get_no_prefix() {
        for model in ["mxbai-embed-large", "bge-m3", "all-minilm", "qwen3:8b", ""] {
            assert_eq!(embedding_input(model, "t", EmbedRole::Document), "t");
            assert_eq!(embedding_input(model, "t", EmbedRole::Query), "t");
        }
    }
}
