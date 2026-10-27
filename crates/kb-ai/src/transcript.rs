//! Transcripts (`docs/architecture/ai.md`, section 3.5): cleaning what an audio model answers and
//! laying segments out as Markdown.

use crate::util::{char_len, strip_prefix_ignore_case};

/// A cleaned transcription of one segment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transcribed {
    /// The speech (empty for silence).
    pub text: String,
    /// The language an ASR header named.
    pub language: Option<String>,
}

const ASR_OPEN: &str = "<asr_text>";
const ASR_CLOSE: &str = "</asr_text>";

/// Longest language name an ASR header may carry.
const MAX_LANGUAGE_CHARS: usize = 60;

/// Lead-ins a chat model puts before a transcript, longest first.
const LEAD_INS: [&str; 6] = [
    "here is the transcript:",
    "here's the transcript:",
    "here is the transcription:",
    "here's the transcription:",
    "transcript:",
    "transcription:",
];

/// Cleans a model's answer: ASR header `language <Name><asr_text>` (language kept), closing tag,
/// "Transcript:" lead-ins, quotes and fences, runs of spaces; silence markers become empty.
///
/// A language of `None` or `unknown` (what an ASR model writes for silence) is not a language.
/// Line breaks are kept; each line is trimmed and its runs of spaces and tabs become one space.
#[must_use]
pub fn clean_transcript(raw: &str) -> Transcribed {
    let mut language = None;
    let mut text = raw.trim().to_owned();
    if let Some((before, after)) = text.split_once(ASR_OPEN) {
        match asr_header_language(before) {
            Some(found) => {
                language = found;
                text = after.to_owned();
            }
            // Something else before the tag: keep it, drop the tag.
            None => text = format!("{before}{after}"),
        }
    }
    let mut text = text.replace(ASR_OPEN, "").replace(ASR_CLOSE, "");
    // A wrapper may hide another (a quoted lead-in inside a fence): peel until nothing changes.
    for _ in 0..4 {
        let trimmed = text.trim();
        let peeled = strip_fence(trimmed)
            .or_else(|| strip_lead_in(trimmed))
            .or_else(|| strip_quotes(trimmed));
        match peeled {
            Some(inner) => text = inner.to_owned(),
            None => break,
        }
    }
    let text = collapse_spaces(&text);
    if is_silence_marker(&text) {
        return Transcribed {
            text: String::new(),
            language,
        };
    }
    Transcribed { text, language }
}

/// The language of a header such as `language English`: `None` when `header` is not one,
/// `Some(None)` for a header without a usable language.
fn asr_header_language(header: &str) -> Option<Option<String>> {
    let rest = strip_prefix_ignore_case(header.trim(), "language")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest.trim();
    if name.contains('\n') || char_len(name) > MAX_LANGUAGE_CHARS {
        return None;
    }
    if name.is_empty() || name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("unknown")
    {
        return Some(None);
    }
    Some(Some(name.to_owned()))
}

/// The text inside a fence that wraps all of `text` (the opening line may name a language).
fn strip_fence(text: &str) -> Option<&str> {
    let after_open = text.strip_prefix("```")?;
    let newline = after_open.find('\n')?;
    let inner = after_open.get(newline + 1..)?;
    let inner = inner.trim_end().strip_suffix("```")?;
    // Another fence inside means several blocks, not one wrapper.
    if inner.contains("```") {
        return None;
    }
    Some(inner)
}

/// `text` without a leading "Transcript:" style lead-in.
fn strip_lead_in(text: &str) -> Option<&str> {
    LEAD_INS
        .iter()
        .find_map(|lead_in| strip_prefix_ignore_case(text, lead_in))
        .map(str::trim_start)
}

/// The text inside quotes that wrap all of it (and contain no other quote of that kind).
fn strip_quotes(text: &str) -> Option<&str> {
    const PAIRS: [(char, char); 4] = [
        ('"', '"'),
        ('\u{201c}', '\u{201d}'),
        ('\u{ab}', '\u{bb}'),
        ('\'', '\''),
    ];
    for (open, close) in PAIRS {
        let Some(inner) = text
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
        else {
            continue;
        };
        if !inner.contains(open) && !inner.contains(close) {
            return Some(inner);
        }
    }
    None
}

/// Each line trimmed, its runs of spaces and tabs one space, the whole trimmed.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut gap = false;
        for c in line.trim().chars() {
            if c == ' ' || c == '\t' {
                if !gap {
                    out.push(' ');
                }
                gap = true;
            } else {
                gap = false;
                out.push(c);
            }
        }
    }
    out.trim().to_owned()
}

/// Whether the whole answer is a marker for "nothing was said" (always in brackets or
/// parentheses: the bare word may be what a person said).
fn is_silence_marker(text: &str) -> bool {
    let marker = text.trim().trim_end_matches('.').trim().to_lowercase();
    matches!(
        marker.as_str(),
        "" | "[silence]"
            | "(silence)"
            | "[silent]"
            | "(silent)"
            | "[no speech]"
            | "(no speech)"
            | "[no speech detected]"
            | "(no speech detected)"
            | "[blank_audio]"
            | "[blank audio]"
            | "(blank audio)"
    )
}

/// `mm:ss`, or `h:mm:ss` from one hour.
#[must_use]
pub fn format_timestamp(ms: u64) -> String {
    let seconds = ms / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// One paragraph per non-empty segment: `**[mm:ss]** text`, separated by blank lines. A segment
/// is `(start in milliseconds, text)`; segments whose text is empty or white space are skipped.
#[must_use]
pub fn transcript_markdown(segments: &[(u64, String)]) -> String {
    segments
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(start, text)| format!("**[{}]** {}", format_timestamp(*start), text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Whether bytes are a RIFF/WAVE file with PCM format (what `ai_transcribe` accepts): the
/// `RIFF....WAVE` header and a `fmt ` chunk whose audio format is 1.
#[must_use]
pub fn is_pcm_wav(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WAVE".as_slice()) {
        return false;
    }
    let mut position = 12usize;
    loop {
        let Some(id) = bytes.get(position..position.saturating_add(4)) else {
            return false;
        };
        let Some(size) = bytes
            .get(position.saturating_add(4)..position.saturating_add(8))
            .and_then(|field| <[u8; 4]>::try_from(field).ok())
            .map(u32::from_le_bytes)
        else {
            return false;
        };
        let body = position.saturating_add(8);
        if id == b"fmt " {
            // Audio format, channels, rate, byte rate, block align, bits: 16 bytes at least.
            let Some(format) = bytes.get(body..body.saturating_add(16)) else {
                return false;
            };
            // Audio format 1 (PCM), little-endian.
            return size >= 16 && format.starts_with(&[1, 0]);
        }
        // Chunks are padded to an even size.
        let Ok(size) = usize::try_from(size) else {
            return false;
        };
        let Some(next) = body
            .checked_add(size)
            .and_then(|end| end.checked_add(size & 1))
        else {
            return false;
        };
        position = next;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn clean(raw: &str) -> (String, Option<String>) {
        let result = clean_transcript(raw);
        (result.text, result.language)
    }

    // ----- clean_transcript -----

    #[test]
    fn the_real_r2t2_answer_loses_its_header() {
        let raw = "language English<asr_text>Hello, Ben. The quarterly meeting moved to Thursday at 3 p.m.";
        let result = clean_transcript(raw);
        assert_eq!(
            result.text,
            "Hello, Ben. The quarterly meeting moved to Thursday at 3 p.m."
        );
        assert_eq!(result.language.as_deref(), Some("English"));
    }

    #[test]
    fn a_closing_tag_and_surrounding_white_space_are_removed() {
        let result =
            clean_transcript("  language French<asr_text> Bonjour tout le monde. </asr_text>\n");
        assert_eq!(result.text, "Bonjour tout le monde.");
        assert_eq!(result.language.as_deref(), Some("French"));
    }

    #[test]
    fn a_language_with_two_words_is_kept_whole() {
        let result = clean_transcript("language Chinese Mandarin<asr_text>你好");
        assert_eq!(result.language.as_deref(), Some("Chinese Mandarin"));
        assert_eq!(result.text, "你好");
    }

    #[test]
    fn a_none_language_is_no_language() {
        for header in [
            "language None",
            "language none",
            "language Unknown",
            "language",
        ] {
            let result = clean_transcript(&format!("{header}<asr_text>Hi there."));
            assert_eq!(result.language, None, "{header}");
            assert_eq!(result.text, "Hi there.", "{header}");
        }
    }

    #[test]
    fn silence_after_an_asr_header_is_empty_but_keeps_the_language() {
        let result = clean_transcript("language English<asr_text>");
        assert_eq!(result.text, "");
        assert_eq!(result.language.as_deref(), Some("English"));
        let result = clean_transcript("language None<asr_text></asr_text>");
        assert_eq!(result, Transcribed::default());
    }

    #[test]
    fn a_tag_without_a_header_is_just_removed() {
        assert_eq!(
            clean("<asr_text>Hello there</asr_text>"),
            ("Hello there".into(), None)
        );
        assert_eq!(clean("Hello <asr_text>there"), ("Hello there".into(), None));
        // "languages" is not the header "language".
        assert_eq!(
            clean("languages are fun<asr_text>yes"),
            ("languages are funyes".into(), None)
        );
    }

    #[test]
    fn a_header_name_that_is_a_sentence_is_not_a_language() {
        let long = format!("language {}<asr_text>text", "x".repeat(80));
        assert_eq!(clean_transcript(&long).language, None);
        let two_lines = "language English\nand more<asr_text>text";
        assert_eq!(clean_transcript(two_lines).language, None);
    }

    #[test]
    fn lead_ins_are_removed() {
        assert_eq!(clean("Transcript: Hello there.").0, "Hello there.");
        assert_eq!(clean("transcript:\nHello there.").0, "Hello there.");
        assert_eq!(
            clean("Here is the transcript:\n\nHello there.").0,
            "Hello there."
        );
        assert_eq!(
            clean("HERE IS THE TRANSCRIPT: Hello there.").0,
            "Hello there."
        );
        assert_eq!(
            clean("Here's the transcript:\nHello there.").0,
            "Hello there."
        );
        assert_eq!(clean("Transcription: Hello there.").0, "Hello there.");
    }

    #[test]
    fn only_a_leading_lead_in_is_removed() {
        let text = "He said: transcript: is a word.";
        assert_eq!(clean(text).0, text);
        assert_eq!(
            clean("The transcript: was long.").0,
            "The transcript: was long."
        );
    }

    #[test]
    fn surrounding_quotes_are_removed() {
        assert_eq!(clean("\"Hello there.\"").0, "Hello there.");
        assert_eq!(clean("\u{201c}Hello there.\u{201d}").0, "Hello there.");
        assert_eq!(clean("'Hello there.'").0, "Hello there.");
        assert_eq!(clean("Transcript: \"Hello there.\"").0, "Hello there.");
    }

    #[test]
    fn quotes_that_do_not_wrap_the_text_stay() {
        let text = "\"Hello,\" he said. \"Goodbye.\"";
        assert_eq!(clean(text).0, text);
        assert_eq!(clean("It's fine, he's here.").0, "It's fine, he's here.");
        assert_eq!(clean("\"Only the start").0, "\"Only the start");
    }

    #[test]
    fn fences_are_removed() {
        assert_eq!(clean("```\nHello there.\n```").0, "Hello there.");
        assert_eq!(clean("```text\nHello there.\n```\n").0, "Hello there.");
        assert_eq!(
            clean("```markdown\nLine one\nLine two\n```").0,
            "Line one\nLine two"
        );
        assert_eq!(clean("```\nTranscript: \"Hello.\"\n```").0, "Hello.");
        // Two separate blocks are not one wrapper.
        let two = "```\na\n```\nb\n```\nc\n```";
        assert_eq!(clean(two).0, two);
    }

    #[test]
    fn runs_of_spaces_collapse_and_lines_are_trimmed() {
        assert_eq!(clean("Hello   there,\t\tBen.").0, "Hello there, Ben.");
        assert_eq!(clean("  one  \n   two   words  \n").0, "one\ntwo words");
        assert_eq!(clean("a\n\nb").0, "a\n\nb");
        assert_eq!(clean("a\r\nb").0, "a\nb");
    }

    #[test]
    fn silence_markers_become_empty() {
        for raw in [
            "[silence]",
            "[Silence]",
            "[SILENCE].",
            "(silence)",
            "(no speech)",
            "[no speech]",
            "(No Speech Detected)",
            "[BLANK_AUDIO]",
            "  [silence]  ",
            "\"[silence]\"",
            "language None<asr_text>[silence]",
            "",
            "   \n ",
            "```\n```",
            "Transcript:",
            "<asr_text></asr_text>",
        ] {
            assert_eq!(clean(raw).0, "", "{raw:?}");
        }
    }

    #[test]
    fn speech_that_mentions_silence_is_kept() {
        for raw in [
            "silence",
            "There was a [silence] here.",
            "[silence] and then he spoke",
            "No speech was given.",
        ] {
            assert_eq!(clean(raw).0, raw, "{raw:?}");
        }
    }

    #[test]
    fn plain_text_is_only_trimmed() {
        assert_eq!(clean("  Just text.  "), ("Just text.".into(), None));
    }

    // ----- timestamps -----

    #[test]
    fn timestamps_are_mm_ss_then_h_mm_ss() {
        assert_eq!(format_timestamp(0), "00:00");
        assert_eq!(format_timestamp(999), "00:00");
        assert_eq!(format_timestamp(1_000), "00:01");
        assert_eq!(format_timestamp(65_000), "01:05");
        assert_eq!(format_timestamp(59 * 60_000 + 59_999), "59:59");
        assert_eq!(format_timestamp(3_600_000), "1:00:00");
        assert_eq!(format_timestamp(3_600_000 + 61_000), "1:01:01");
        assert_eq!(
            format_timestamp(10 * 3_600_000 + 5 * 60_000 + 9_000),
            "10:05:09"
        );
    }

    #[test]
    fn a_transcript_is_one_paragraph_per_segment() {
        let markdown = transcript_markdown(&[
            (0, "Welcome everyone.".into()),
            (28_000, "  Let us begin.  ".into()),
            (61_500, "".into()),
            (3_725_000, "Over an hour in.".into()),
        ]);
        assert_eq!(
            markdown,
            "**[00:00]** Welcome everyone.\n\n**[00:28]** Let us begin.\n\n**[1:02:05]** Over an hour in."
        );
    }

    #[test]
    fn no_segments_is_an_empty_transcript() {
        assert_eq!(transcript_markdown(&[]), "");
        assert_eq!(transcript_markdown(&[(0, "  ".into())]), "");
    }

    // ----- wav -----

    fn wav(format: u16, extra_chunk: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        if extra_chunk {
            // An odd-sized chunk is padded to an even length.
            out.extend_from_slice(b"LIST");
            out.extend_from_slice(&3u32.to_le_bytes());
            out.extend_from_slice(&[1, 2, 3, 0]);
        }
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&format.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // channels
        out.extend_from_slice(&16_000u32.to_le_bytes());
        out.extend_from_slice(&32_000u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out
    }

    #[test]
    fn a_pcm_wav_is_recognised() {
        assert!(is_pcm_wav(&wav(1, false)));
        assert!(is_pcm_wav(&wav(1, true)));
    }

    #[test]
    fn other_formats_and_other_files_are_refused() {
        assert!(!is_pcm_wav(&wav(3, false)), "float");
        assert!(!is_pcm_wav(&wav(0xfffe, false)), "extensible");
        assert!(!is_pcm_wav(&wav(0, false)));
        assert!(!is_pcm_wav(&[]));
        assert!(!is_pcm_wav(b"RIFF"));
        assert!(!is_pcm_wav(b"RIFF\0\0\0\0WAVE"));
        assert!(!is_pcm_wav(b"ID3\x03\0\0\0\0\0\0WAVEfmt "));
        let mut avi = wav(1, false);
        avi[8..12].copy_from_slice(b"AVI ");
        assert!(!is_pcm_wav(&avi));
        let mut rifx = wav(1, false);
        rifx[..4].copy_from_slice(b"RIFX");
        assert!(!is_pcm_wav(&rifx));
    }

    #[test]
    fn a_truncated_or_hostile_header_is_refused_without_panicking() {
        let good = wav(1, false);
        for cut in 0..36 {
            assert!(!is_pcm_wav(&good[..cut]), "cut at {cut}");
        }
        assert!(is_pcm_wav(&good[..36]));
        // A chunk size that runs past the end, or past the address space.
        let mut huge = Vec::new();
        huge.extend_from_slice(b"RIFF\0\0\0\0WAVEJUNK");
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        huge.extend_from_slice(b"fmt ");
        assert!(!is_pcm_wav(&huge));
        // A short fmt chunk.
        let mut short = Vec::new();
        short.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        short.extend_from_slice(&8u32.to_le_bytes());
        short.extend_from_slice(&1u16.to_le_bytes());
        short.extend_from_slice(&[0; 6]);
        assert!(!is_pcm_wav(&short));
    }
}
