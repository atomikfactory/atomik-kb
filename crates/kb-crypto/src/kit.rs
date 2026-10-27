//! Recovery kit encodings (ADR-0017): 24 BIP-39 English words with the standard
//! checksum, or 52 base32 characters plus a 2-character BLAKE3 checksum. Both
//! encode the same 32-byte recovery key.
//!
//! - Words: the 256 key bits followed by the first 8 bits of SHA-256(key), cut
//!   into 24 big-endian groups of 11 bits, each an index into the word list. Any
//!   BIP-39 tool can check a transcription.
//! - Code: the 256 key bits plus 4 zero bits as 52 RFC 4648 base32 characters,
//!   then 2 characters holding the first 10 bits of
//!   BLAKE3(`atomik-kb/v1/recovery-kit` || key); shown as 13 groups of four and the
//!   checksum group, joined by `-`.
//!
//! Decoders accept what people type (any case, extra spaces, line breaks, unique
//! 4-letter word prefixes) and never put the kit text into an error message.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{CryptoError, Secret32};

/// Number of words in the kit.
pub const KIT_WORDS: usize = 24;
/// Data characters in the code form.
pub const KIT_CODE_DATA_CHARS: usize = 52;
/// Checksum characters in the code form.
pub const KIT_CODE_CHECK_CHARS: usize = 2;
/// Domain of the code checksum: BLAKE3(`atomik-kb/v1/recovery-kit` || key).
pub const KIT_CHECKSUM_DOMAIN: &[u8] = b"atomik-kb/v1/recovery-kit";
/// Characters per group in the code form (the checksum group has two).
pub const KIT_CODE_GROUP_LEN: usize = 4;
/// Length of the code form as [`code`] prints it: 54 characters and 13 dashes.
pub const KIT_CODE_LEN: usize =
    KIT_CODE_DATA_CHARS + KIT_CODE_CHECK_CHARS + KIT_CODE_DATA_CHARS / KIT_CODE_GROUP_LEN;
/// Number of words in the BIP-39 English list.
pub const WORDLIST_LEN: usize = 2048;
/// Letters that identify a word of the list on their own.
pub const WORD_PREFIX_LEN: usize = 4;

/// The BIP-39 English word list (2048 words, MIT), one per line.
const WORDLIST_TEXT: &str = include_str!("../wordlists/english.txt");

// The vendored list must have exactly 2048 words; checked when the crate compiles.
const _: () = assert!(count_words(WORDLIST_TEXT) == WORDLIST_LEN);

/// RFC 4648 base32 alphabet.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Bits per word index.
const WORD_BITS: u32 = 11;
/// Bits per base32 character.
const BASE32_BITS: u32 = 5;
/// Recovery key length.
const KEY_LEN: usize = 32;
/// Key plus the 8-bit BIP-39 checksum: 264 bits, exactly 24 words.
const MNEMONIC_BYTES: usize = KEY_LEN + 1;
/// Characters of the code form without separators.
const CODE_SYMBOLS: usize = KIT_CODE_DATA_CHARS + KIT_CODE_CHECK_CHARS;

/// Lines of `text` with at least one character other than spaces, tabs and `\r`.
const fn count_words(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0;
    let mut in_word = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                if in_word {
                    count += 1;
                }
                in_word = false;
            }
            b'\r' | b' ' | b'\t' => {}
            _ => in_word = true,
        }
        i += 1;
    }
    if in_word {
        count += 1;
    }
    count
}

/// The parsed list, built on first use.
fn word_array() -> &'static [&'static str; WORDLIST_LEN] {
    static WORDS: OnceLock<[&'static str; WORDLIST_LEN]> = OnceLock::new();
    WORDS.get_or_init(|| {
        let mut words = [""; WORDLIST_LEN];
        let lines = WORDLIST_TEXT
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        for (slot, word) in words.iter_mut().zip(lines) {
            *slot = word;
        }
        words
    })
}

/// The 2048 BIP-39 English words in order.
#[must_use]
pub fn wordlist() -> &'static [&'static str] {
    word_array()
}

/// Cuts `bytes` into big-endian groups of `width` bits written to `out`; a last,
/// incomplete group is padded with zero bits on the right.
fn bit_groups(bytes: &[u8], width: u32, out: &mut [u16]) {
    let mask = (1u32 << width) - 1;
    let mut slots = out.iter_mut();
    let mut acc = 0u32;
    let mut acc_bits = 0u32;
    for &byte in bytes {
        acc = (acc << 8) | u32::from(byte);
        acc_bits += 8;
        while acc_bits >= width {
            acc_bits -= width;
            if let Some(slot) = slots.next() {
                *slot = ((acc >> acc_bits) & mask) as u16;
            }
        }
        acc &= (1u32 << acc_bits) - 1;
    }
    if acc_bits > 0
        && let Some(slot) = slots.next()
    {
        *slot = ((acc << (width - acc_bits)) & mask) as u16;
    }
}

/// Joins big-endian groups of `width` bits into `out`; returns the bits left over
/// after the last full byte (they must be zero in a canonical encoding).
fn join_groups(groups: &[u16], width: u32, out: &mut [u8]) -> u32 {
    let mut slots = out.iter_mut();
    let mut acc = 0u32;
    let mut acc_bits = 0u32;
    for &group in groups {
        acc = (acc << width) | (u32::from(group) & ((1u32 << width) - 1));
        acc_bits += width;
        while acc_bits >= 8 {
            acc_bits -= 8;
            if let Some(slot) = slots.next() {
                *slot = ((acc >> acc_bits) & 0xff) as u8;
            }
        }
        acc &= (1u32 << acc_bits) - 1;
    }
    acc
}

/// The BIP-39 checksum of 256 bits of entropy: the first byte of SHA-256.
fn bip39_checksum(entropy: &[u8; KEY_LEN]) -> u8 {
    Sha256::digest(entropy)[0]
}

/// The 24 word indices of `rck`.
fn word_indices(rck: &Secret32) -> Zeroizing<[u16; KIT_WORDS]> {
    let mut bits = Zeroizing::new([0u8; MNEMONIC_BYTES]);
    bits[..KEY_LEN].copy_from_slice(rck.expose());
    bits[KEY_LEN] = bip39_checksum(rck.expose());
    let mut indices = Zeroizing::new([0u16; KIT_WORDS]);
    bit_groups(&bits[..], WORD_BITS, &mut indices[..]);
    indices
}

/// The 24 words of `rck` (BIP-39: 256 entropy bits + 8 checksum bits of SHA-256).
#[must_use]
pub fn words(rck: &Secret32) -> [&'static str; KIT_WORDS] {
    let list = word_array();
    let mut out = [""; KIT_WORDS];
    for (slot, &index) in out.iter_mut().zip(word_indices(rck).iter()) {
        // An 11-bit index always falls inside the 2048 words.
        *slot = list[usize::from(index) & (WORDLIST_LEN - 1)];
    }
    out
}

/// The index of a typed word: the word itself in any case, or a prefix of at least
/// [`WORD_PREFIX_LEN`] letters that starts exactly one word of the list.
fn word_index(typed: &str) -> Option<u16> {
    let word = Zeroizing::new(typed.trim().to_ascii_lowercase());
    if word.is_empty() || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    let list = word_array();
    let start = list.partition_point(|candidate| *candidate < word.as_str());
    let first = list.get(start)?;
    if !first.starts_with(word.as_str()) {
        return None;
    }
    let exact = *first == word.as_str();
    let unique_prefix = word.len() >= WORD_PREFIX_LEN
        && list
            .get(start + 1)
            .is_none_or(|next| !next.starts_with(word.as_str()));
    if exact || unique_prefix {
        u16::try_from(start).ok()
    } else {
        None
    }
}

/// Decodes 24 words (any case; a unique 4-letter prefix is accepted for each word);
/// a bad count, unknown word or checksum mismatch is [`CryptoError::InvalidKit`].
///
/// Surrounding whitespace of each word is ignored. Error messages name positions,
/// never the words.
pub fn from_words(words: &[&str]) -> Result<Secret32, CryptoError> {
    if words.len() != KIT_WORDS {
        return Err(CryptoError::InvalidKit(format!(
            "expected {KIT_WORDS} words, got {}",
            words.len()
        )));
    }
    let mut indices = Zeroizing::new([0u16; KIT_WORDS]);
    for (position, (slot, word)) in indices.iter_mut().zip(words).enumerate() {
        *slot = word_index(word).ok_or_else(|| {
            CryptoError::InvalidKit(format!(
                "word {} is neither in the word list nor a unique prefix of \
                 {WORD_PREFIX_LEN} or more letters",
                position + 1
            ))
        })?;
    }
    let mut bits = Zeroizing::new([0u8; MNEMONIC_BYTES]);
    join_groups(&indices[..], WORD_BITS, &mut bits[..]);
    let mut entropy = Zeroizing::new([0u8; KEY_LEN]);
    entropy.copy_from_slice(&bits[..KEY_LEN]);
    if bits[KEY_LEN] != bip39_checksum(&entropy) {
        return Err(CryptoError::InvalidKit(
            "the checksum does not match: a word is wrong or out of order".to_owned(),
        ));
    }
    Ok(Secret32::from_bytes(*entropy))
}

/// The two checksum characters (as alphabet indices): the first 10 bits of
/// BLAKE3(`KIT_CHECKSUM_DOMAIN` || key).
fn code_checksum(key: &[u8; KEY_LEN]) -> [u16; KIT_CODE_CHECK_CHARS] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(KIT_CHECKSUM_DOMAIN);
    hasher.update(key);
    let hash = hasher.finalize();
    let [first, second, ..] = *hash.as_bytes();
    let bits = (u16::from(first) << 2) | (u16::from(second) >> 6);
    [bits >> 5, bits & 0x1f]
}

/// The code form: 13 groups of 4 base32 characters (RFC 4648 alphabet, uppercase)
/// followed by a 2-character checksum group, joined by `-`: `xxxx-xxxx-…-xxxx-cc`.
#[must_use]
pub fn code(rck: &Secret32) -> String {
    let mut symbols = Zeroizing::new([0u16; CODE_SYMBOLS]);
    let (data, check) = symbols.split_at_mut(KIT_CODE_DATA_CHARS);
    bit_groups(rck.expose(), BASE32_BITS, data);
    check.copy_from_slice(&code_checksum(rck.expose()));
    // Exact capacity: no reallocation leaves a partial copy behind.
    let mut out = String::with_capacity(KIT_CODE_LEN);
    for (i, &symbol) in symbols.iter().enumerate() {
        if i > 0 && i % KIT_CODE_GROUP_LEN == 0 {
            out.push('-');
        }
        out.push(char::from(BASE32_ALPHABET[usize::from(symbol) & 0x1f]));
    }
    out
}

/// The alphabet index of a base32 character, either case.
fn base32_value(c: char) -> Option<u16> {
    let byte = u8::try_from(c).ok()?;
    match byte {
        b'A'..=b'Z' => Some(u16::from(byte - b'A')),
        b'a'..=b'z' => Some(u16::from(byte - b'a')),
        b'2'..=b'7' => Some(u16::from(byte - b'2') + 26),
        _ => None,
    }
}

/// Decodes the code form (any case, separators and whitespace ignored); a wrong
/// length or checksum is [`CryptoError::InvalidKit`].
///
/// Also refused: characters outside the alphabet (`0`, `1`, `8`, `9` included) and a
/// last data character with non-zero padding bits (each key has exactly one code).
pub fn from_code(text: &str) -> Result<Secret32, CryptoError> {
    let mut symbols = Zeroizing::new([0u16; CODE_SYMBOLS]);
    let mut count = 0usize;
    for c in text.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        let value = base32_value(c).ok_or_else(|| {
            CryptoError::InvalidKit(
                "the code may only contain the letters A-Z, the digits 2-7, dashes and spaces"
                    .to_owned(),
            )
        })?;
        if let Some(slot) = symbols.get_mut(count) {
            *slot = value;
        }
        count += 1;
    }
    if count != CODE_SYMBOLS {
        return Err(CryptoError::InvalidKit(format!(
            "expected {CODE_SYMBOLS} code characters, got {count}"
        )));
    }
    let (data, check) = symbols.split_at(KIT_CODE_DATA_CHARS);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    if join_groups(data, BASE32_BITS, &mut key[..]) != 0 {
        return Err(CryptoError::InvalidKit(
            "the last data character of the code is not valid in that position".to_owned(),
        ));
    }
    if check != code_checksum(&key).as_slice() {
        return Err(CryptoError::InvalidKit(
            "the checksum does not match: a character is wrong".to_owned(),
        ));
    }
    Ok(Secret32::from_bytes(*key))
}

/// Decodes either form (words when the text contains letters-only tokens of the
/// word list, the code otherwise).
///
/// Precisely: when the text is 24 whitespace-separated tokens made of ASCII
/// letters only, it is read as words; anything else is read as a code. When the
/// code reading fails and the text looked like a word list of the wrong length,
/// the error says so (`expected 24 words, got N`).
pub fn parse(text: &str) -> Result<Secret32, CryptoError> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let alphabetic = !tokens.is_empty()
        && tokens
            .iter()
            .all(|token| token.bytes().all(|b| b.is_ascii_alphabetic()));
    if alphabetic && tokens.len() == KIT_WORDS {
        return from_words(&tokens);
    }
    let result = from_code(text);
    let letters: usize = tokens.iter().map(|token| token.len()).sum();
    if result.is_err() && alphabetic && tokens.len() > 1 && letters != CODE_SYMBOLS {
        return Err(CryptoError::InvalidKit(format!(
            "expected {KIT_WORDS} words, got {}",
            tokens.len()
        )));
    }
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::test_support::{hex, kat, unhex32};
    use crate::{LABEL_PREFIX, Label};
    use kb_types::testing::SeededRng;

    fn random_key(seed: u64) -> Secret32 {
        Secret32::generate(&SeededRng::new(seed))
    }

    fn invalid_kit_message(result: Result<Secret32, CryptoError>) -> String {
        match result {
            Err(CryptoError::InvalidKit(message)) => message,
            other => panic!("expected InvalidKit, got {other:?}"),
        }
    }

    #[test]
    fn the_vendored_word_list_is_bip39_english() {
        let digest = Sha256::digest(WORDLIST_TEXT.as_bytes());
        assert_eq!(
            hex(&digest),
            "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda"
        );
        let list = wordlist();
        assert_eq!(list.len(), WORDLIST_LEN);
        assert_eq!(list[0], "abandon");
        assert_eq!(list[2047], "zoo");
        for word in list {
            assert!((3..=8).contains(&word.len()), "{word}");
            assert!(word.bytes().all(|b| b.is_ascii_lowercase()), "{word}");
        }
        for pair in list.windows(2) {
            assert!(
                pair[0] < pair[1],
                "sorted and unique: {} {}",
                pair[0],
                pair[1]
            );
            let a = &pair[0][..pair[0].len().min(WORD_PREFIX_LEN)];
            let b = &pair[1][..pair[1].len().min(WORD_PREFIX_LEN)];
            assert_ne!(a, b, "four letters identify a word");
        }
        assert_eq!(count_words(WORDLIST_TEXT), WORDLIST_LEN);
        assert_eq!(count_words("a\r\n\r\nb\n  \nc"), 3);
    }

    #[test]
    fn checksum_domain_is_the_recovery_kit_label() {
        assert_eq!(
            KIT_CHECKSUM_DOMAIN,
            format!("{LABEL_PREFIX}{}", Label::RECOVERY_KIT.purpose()).as_bytes()
        );
    }

    #[test]
    fn matches_the_trezor_bip39_vectors() {
        let file = kat("kat-bip39.json");
        let vectors = file["vectors"].as_array().unwrap();
        assert_eq!(
            vectors.len(),
            8,
            "every 256-bit vector of the reference set"
        );
        for vector in vectors {
            let entropy = Secret32::from_bytes(unhex32(vector["entropy_hex"].as_str().unwrap()));
            let mnemonic = vector["mnemonic"].as_str().unwrap();
            assert_eq!(words(&entropy).join(" "), mnemonic);
            let typed: Vec<&str> = mnemonic.split(' ').collect();
            assert_eq!(from_words(&typed).unwrap(), entropy);
            assert_eq!(parse(mnemonic).unwrap(), entropy);
        }
    }

    #[test]
    fn words_round_trip_with_prefixes_case_and_whitespace() {
        for seed in 0..20 {
            let key = random_key(seed);
            let kit = words(&key);
            assert_eq!(from_words(&kit).unwrap(), key);

            let prefixes: Vec<&str> = kit
                .iter()
                .map(|w| &w[..w.len().min(WORD_PREFIX_LEN)])
                .collect();
            assert_eq!(from_words(&prefixes).unwrap(), key, "4-letter prefixes");

            let shouted: Vec<String> = kit
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    if i % 2 == 0 {
                        w.to_uppercase()
                    } else {
                        format!("  {}\t", w[..1].to_uppercase() + &w[1..])
                    }
                })
                .collect();
            let shouted: Vec<&str> = shouted.iter().map(String::as_str).collect();
            assert_eq!(from_words(&shouted).unwrap(), key, "mixed case and spaces");

            let text = format!("\n  {}\n", kit.join(" \n\t "));
            assert_eq!(parse(&text).unwrap(), key, "free-form whitespace");
        }
    }

    #[test]
    fn longer_prefixes_count_but_wrong_letters_do_not() {
        let key = random_key(100);
        let kit = words(&key);
        let mut typed: Vec<String> = kit.iter().map(|w| (*w).to_owned()).collect();
        // A prefix longer than four letters is still a prefix.
        let long_word = kit.iter().position(|w| w.len() >= 6).unwrap();
        typed[long_word] = kit[long_word][..5].to_owned();
        let as_refs: Vec<&str> = typed.iter().map(String::as_str).collect();
        assert_eq!(from_words(&as_refs).unwrap(), key);
        // A wrong letter after the prefix is not.
        typed[long_word] = format!("{}x", &kit[long_word][..5]);
        let as_refs: Vec<&str> = typed.iter().map(String::as_str).collect();
        let message = invalid_kit_message(from_words(&as_refs));
        assert!(
            message.contains(&format!("word {}", long_word + 1)),
            "{message}"
        );
    }

    #[test]
    fn short_words_and_short_prefixes() {
        // "act" is a word and the prefix of four others: exact matches win.
        assert_eq!(
            word_index("act"),
            wordlist()
                .iter()
                .position(|w| *w == "act")
                .map(|i| i as u16)
        );
        assert_eq!(word_index("ACT "), word_index("act"));
        assert_eq!(word_index("acti"), word_index("action"));
        // Three letters that are not a word are too short to be a prefix.
        assert_eq!(word_index("aba"), None);
        assert_eq!(word_index("ab"), None);
        assert_eq!(word_index(""), None);
        assert_eq!(word_index("   "), None);
        assert_eq!(word_index("zoo"), Some(2047));
        assert_eq!(word_index("zooo"), None);
        assert_eq!(word_index("abandon1"), None);
        assert_eq!(word_index("ab and"), None);
        assert_eq!(word_index("\u{e9}tude"), None);
    }

    #[test]
    fn word_errors() {
        let key = random_key(200);
        let kit = words(&key);
        let message = invalid_kit_message(from_words(&kit[..23]));
        assert_eq!(message, "expected 24 words, got 23");
        let mut longer = kit.to_vec();
        longer.push("zoo");
        assert_eq!(
            invalid_kit_message(from_words(&longer)),
            "expected 24 words, got 25"
        );
        assert_eq!(
            invalid_kit_message(from_words(&[])),
            "expected 24 words, got 0"
        );

        let mut unknown = kit;
        unknown[5] = "qqqqzzzz";
        let message = invalid_kit_message(from_words(&unknown));
        assert!(message.starts_with("word 6 "), "{message}");
        assert!(!message.contains("qqqq"));

        // Swapping two different words breaks the checksum.
        let mut swapped = kit;
        let other = (1..KIT_WORDS).find(|&i| kit[i] != kit[0]).unwrap();
        swapped.swap(0, other);
        assert!(invalid_kit_message(from_words(&swapped)).contains("checksum"));

        // The last word holds 3 key bits and the 8 checksum bits: any other word
        // with the same key bits fails the checksum.
        let last = word_index(kit[23]).unwrap();
        let mut wrong_last = kit;
        wrong_last[23] = wordlist()[usize::from(last ^ 0x01)];
        assert!(invalid_kit_message(from_words(&wrong_last)).contains("checksum"));
    }

    #[test]
    fn errors_never_contain_the_kit() {
        let distinctive = [
            "zebra", "zero", "zone", "youth", "yellow", "wisdom", "whale", "vapor", "umbrella",
            "tuition", "theory", "symptom", "stadium", "spider", "shrimp", "quantum", "purple",
            "pumpkin", "oxygen", "orbit", "nephew", "mushroom", "lobster", "jaguar",
        ];
        assert_eq!(distinctive.len(), KIT_WORDS);
        let mut cases: Vec<Vec<&str>> = vec![distinctive.to_vec(), distinctive[..20].to_vec()];
        let mut typo = distinctive.to_vec();
        typo[3] = "youthh";
        cases.push(typo);
        for case in cases {
            let message = invalid_kit_message(from_words(&case));
            for word in &case {
                assert!(!message.contains(word), "{message} leaks {word}");
            }
            let text = case.join(" ");
            let message = invalid_kit_message(parse(&text));
            for word in &case {
                assert!(!message.contains(word), "{message} leaks {word}");
            }
        }
        let code_text = code(&random_key(300));
        let mut broken = code_text.clone().into_bytes();
        broken[0] = if broken[0] == b'A' { b'B' } else { b'A' };
        let broken = String::from_utf8(broken).unwrap();
        let message = invalid_kit_message(from_code(&broken));
        for group in broken.split('-') {
            assert!(!message.contains(group), "{message} leaks {group}");
        }
    }

    #[test]
    fn code_format() {
        for seed in 0..10 {
            let key = random_key(seed);
            let text = code(&key);
            assert_eq!(text.len(), KIT_CODE_LEN);
            assert_eq!(KIT_CODE_LEN, 67);
            let groups: Vec<&str> = text.split('-').collect();
            assert_eq!(groups.len(), 14);
            for group in &groups[..13] {
                assert_eq!(group.len(), 4);
            }
            assert_eq!(groups[13].len(), KIT_CODE_CHECK_CHARS);
            assert!(
                text.bytes()
                    .all(|b| b == b'-' || BASE32_ALPHABET.contains(&b))
            );
            assert_eq!(from_code(&text).unwrap(), key);
            assert_eq!(parse(&text).unwrap(), key);
        }
    }

    #[test]
    fn code_known_values() {
        // All-zero key: 52 'A', then the checksum.
        let zero = code(&Secret32::from_bytes([0; 32]));
        assert!(
            zero.starts_with("AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-")
        );
        // All-ones key: 51 '7' (31), then one key bit and four zero bits (0b10000 = 'Q').
        let ones = code(&Secret32::from_bytes([0xff; 32]));
        let data: String = ones.split('-').take(13).collect();
        assert_eq!(data, format!("{}Q", "7".repeat(51)));
        // The checksum is the first 10 bits of BLAKE3(domain || key), independently.
        for key in [[0u8; 32], [0xff; 32], *random_key(7).expose()] {
            let mut input = KIT_CHECKSUM_DOMAIN.to_vec();
            input.extend_from_slice(&key);
            let hash = blake3::hash(&input);
            let bits = (u16::from(hash.as_bytes()[0]) << 2) | (u16::from(hash.as_bytes()[1]) >> 6);
            let expected: String = [bits >> 5, bits & 31]
                .iter()
                .map(|&v| char::from(BASE32_ALPHABET[usize::from(v)]))
                .collect();
            let text = code(&Secret32::from_bytes(key));
            assert_eq!(&text[text.len() - 2..], expected);
        }
    }

    #[test]
    fn code_input_is_forgiving_about_case_and_separators() {
        let key = random_key(400);
        let text = code(&key);
        let variants = [
            text.to_lowercase(),
            text.replace('-', ""),
            text.replace('-', " "),
            text.replace('-', "\n"),
            format!("  {}\r\n", text.replace('-', " - ")),
            text.chars()
                .enumerate()
                .map(|(i, c)| {
                    if i % 3 == 0 {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    }
                })
                .collect(),
        ];
        for variant in variants {
            assert_eq!(from_code(&variant).unwrap(), key, "{variant:?}");
            assert_eq!(parse(&variant).unwrap(), key, "{variant:?}");
        }
    }

    #[test]
    fn code_errors() {
        let key = random_key(500);
        let text = code(&key);
        let compact = text.replace('-', "");
        assert_eq!(
            invalid_kit_message(from_code(&compact[..53])),
            "expected 54 code characters, got 53"
        );
        assert_eq!(
            invalid_kit_message(from_code(&format!("{compact}A"))),
            "expected 54 code characters, got 55"
        );
        assert_eq!(
            invalid_kit_message(from_code("")),
            "expected 54 code characters, got 0"
        );
        for bad in ['0', '1', '8', '9', '!', '_', '\u{e9}'] {
            let mut changed: Vec<char> = compact.chars().collect();
            changed[10] = bad;
            let changed: String = changed.into_iter().collect();
            assert!(
                invalid_kit_message(from_code(&changed)).contains("A-Z"),
                "{bad:?}"
            );
        }
        // Any single changed data character is caught by the 10-bit checksum (or,
        // for the last one, by the padding rule).
        for position in 0..KIT_CODE_DATA_CHARS {
            let mut changed = compact.clone().into_bytes();
            let value = base32_value(char::from(changed[position])).unwrap();
            changed[position] = BASE32_ALPHABET[usize::from((value + 1) % 32)];
            let changed = String::from_utf8(changed).unwrap();
            let message = invalid_kit_message(from_code(&changed));
            assert!(
                message.contains("checksum") || message.contains("last data character"),
                "position {position}: {message}"
            );
        }
        // A changed checksum character.
        let mut changed = compact.clone().into_bytes();
        changed[53] = if changed[53] == b'A' { b'B' } else { b'A' };
        let changed = String::from_utf8(changed).unwrap();
        assert!(invalid_kit_message(from_code(&changed)).contains("checksum"));
    }

    #[test]
    fn padding_bits_must_be_zero() {
        // The zero key ends its data with 'A' (0b00000): one key bit, four padding
        // bits. 'B' (0b00001) sets a padding bit and is refused even though the key
        // bits and the checksum are unchanged.
        let text = code(&Secret32::from_bytes([0; 32]));
        let mut compact = text.replace('-', "").into_bytes();
        assert_eq!(compact[51], b'A');
        compact[51] = b'B';
        let changed = String::from_utf8(compact).unwrap();
        assert!(invalid_kit_message(from_code(&changed)).contains("last data character"));
    }

    #[test]
    fn parse_picks_the_form_and_explains_wrong_word_counts() {
        let key = random_key(600);
        let kit = words(&key);
        assert_eq!(parse(&kit.join(" ")).unwrap(), key);
        assert_eq!(parse(&code(&key)).unwrap(), key);
        assert_eq!(
            invalid_kit_message(parse(&kit[..23].join(" "))),
            "expected 24 words, got 23"
        );
        assert_eq!(
            invalid_kit_message(parse("hello world")),
            "expected 24 words, got 2"
        );
        assert_eq!(
            invalid_kit_message(parse("")),
            "expected 54 code characters, got 0"
        );
        // Words with a digit inside are not a word list: read as a code.
        let mut with_digit = kit.join(" ");
        with_digit.push('2');
        assert!(invalid_kit_message(parse(&with_digit)).contains("code"));
    }

    #[test]
    fn both_encodings_carry_the_same_key() {
        for seed in 700..720 {
            let key = random_key(seed);
            assert_eq!(
                parse(&words(&key).join(" ")).unwrap(),
                parse(&code(&key)).unwrap()
            );
        }
    }

    #[test]
    fn bit_group_helpers_are_inverse() {
        let bytes: Vec<u8> = (0u8..32)
            .map(|i| i.wrapping_mul(37).wrapping_add(11))
            .collect();
        let mut groups = [0u16; 52];
        bit_groups(&bytes, 5, &mut groups);
        let mut back = [0u8; 32];
        assert_eq!(join_groups(&groups, 5, &mut back), 0);
        assert_eq!(back.as_slice(), bytes.as_slice());
        let mut groups = [0u16; 24];
        let mut padded = bytes.clone();
        padded.push(0xa5);
        bit_groups(&padded, 11, &mut groups);
        let mut back = [0u8; 33];
        assert_eq!(join_groups(&groups, 11, &mut back), 0);
        assert_eq!(back.as_slice(), padded.as_slice());
    }
}
