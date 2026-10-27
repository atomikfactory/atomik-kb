//! The measured settings of dictation and the rules that follow from them (ADR-0053,
//! `docs/architecture/dictation.md` section 4): the voice-activity defaults, the pre-roll, the hard segment
//! caps, the sidecar's idle exit, how a segment longer than the cap is cut, the ring that keeps the
//! pre-roll audio, and [`join_phrase`], the one rule that decides what is inserted at the cursor for a
//! recognised phrase.
//!
//! The numbers come from the 2026-10-16 spike on a 3950X (`docs/architecture/dictation.md`, "Measured").

use std::collections::VecDeque;
use std::ops::Range;
use std::time::Duration;

use crate::protocol::SAMPLE_RATE_HZ;

// ----- voice activity (Silero) --------------------------------------------------------------

/// Speech probability above which a window counts as speech.
pub const VAD_THRESHOLD: f32 = 0.5;
/// Seconds of silence that end a segment: the pause after which a phrase appears.
pub const VAD_MIN_SILENCE_S: f32 = 0.5;
/// Seconds of speech below which a segment is dropped (a cough, a click).
pub const VAD_MIN_SPEECH_S: f32 = 0.25;
/// Seconds after which a segment is cut at the next brief dip. Soft: it never cuts through continuous
/// speech, which is why the hard cap exists.
pub const VAD_MAX_SPEECH_S: f32 = 20.0;
/// Samples per window (fixed for Silero at 16 kHz).
pub const VAD_WINDOW_SIZE: u32 = 512;
/// Seconds of audio the detector's buffer holds.
pub const VAD_BUFFER_S: f32 = 30.0;
/// onnxruntime threads of the detector.
pub const VAD_THREADS: u32 = 1;

/// Audio kept before the detector's start of speech and added to the segment (it saves a short first
/// word), milliseconds.
pub const PREROLL_MS: u32 = 150;

/// How much audio the sidecar's ring keeps for the pre-roll, milliseconds: longer than any segment
/// the detector's 30 s buffer can hold.
pub const PREROLL_RETAIN_MS: u32 = 45_000;

/// The size of the audio the webview pushes, milliseconds (100 to 200 are fine).
pub const CHUNK_MS: u32 = 100;

// ----- caps ---------------------------------------------------------------------------------

/// The hard segment cap of Parakeet: it drops sentences on longer inputs without pauses.
pub const HARD_CAP_PARAKEET_MS: u32 = 15_000;
/// The hard segment cap of Moonshine v1, which is tested up to 30 s.
pub const HARD_CAP_MOONSHINE_MS: u32 = 25_000;
/// The hard segment cap of the Ollama engine (a segment becomes one WAV and one request).
pub const HARD_CAP_OLLAMA_MS: u32 = 15_000;

/// The most samples one `Audio` frame the shell sends holds (1 s); it cuts a bigger push.
pub const MAX_AUDIO_FRAME_SAMPLES: usize = 16_000;

/// The most bytes one `dictation_audio` call may carry (2 s of 16 kHz `i16`).
pub const MAX_PUSH_BYTES: usize = 64_000;

/// How long the sidecar waits for a frame before it exits (it holds a model in memory).
pub const IDLE_EXIT: Duration = Duration::from_secs(60);

/// How long the shell waits for `Flushed` (and for the Ollama requests of the last segments) after the
/// person stops, before it ends the session anyway.
pub const FLUSH_TIMEOUT: Duration = Duration::from_secs(20);

// ----- joining a phrase to the text -----------------------------------------------------------

/// Characters that, at the start of a phrase, stay attached to the text before them: `, . ; : ! ? ) ] } …`.
pub const CLOSERS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '…'];

/// Characters after which no space is added: `( [ { ¿ ¡`.
pub const OPENERS: &[char] = &['(', '[', '{', '¿', '¡'];

/// Line breaks: a phrase after one starts a line, so it is not the continuation of a sentence. (`\n`, `\v`,
/// `\f`, `\r`, NEL, LINE SEPARATOR, PARAGRAPH SEPARATOR.)
const LINE_BREAKS: &[char] = &[
    '\n', '\u{b}', '\u{c}', '\r', '\u{85}', '\u{2028}', '\u{2029}',
];

/// What to insert at the cursor for a recognised phrase.
///
/// `before` is the text before the cursor in the same text block, empty at the start of the block (only
/// its end matters). `text` is what the recognizer said. The result is `text` trimmed and with every run of
/// whitespace collapsed to one space, with a leading space unless the phrase is at the start of a block,
/// follows whitespace or an opening bracket (`( [ {` and the Spanish `¿ ¡`), or begins with a closing
/// mark (`, . ; : ! ? ) ] } …`). `None` when `text` has nothing but whitespace.
///
/// **Mid-sentence capital.** Parakeet starts every phrase with a capital, even when the speaker only paused.
/// When the text before the cursor leaves its sentence open (see [`continues_sentence`]), the phrase's first
/// letter is lowered, but only when it is an uppercase letter followed by a lowercase one and the rest of the
/// first word has no capital: `The`, `Then`, `Élan` become `the`, `then`, `élan`, while `I`, `I'm`, `NASA`,
/// `OK`, `McDonald` and `Jean-Pierre` are kept. A name such as `Paris` looks like any sentence's first word
/// and is lowered too (a limit). The phrase's full stop stays.
///
/// Whitespace is Unicode `White_Space` (`char::is_whitespace`); the TypeScript mirror
/// (`packages/core-client/src/dictation.ts`) spells that set out and shares the test vectors in
/// `crates/kb-speech/testdata/join_phrase.json`.
#[must_use]
pub fn join_phrase(before: &str, text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let attached = match before.chars().next_back() {
        None => true,
        Some(c) => c.is_whitespace() || OPENERS.contains(&c),
    } || first.chars().next().is_some_and(|c| CLOSERS.contains(&c));
    let mut joined = String::with_capacity(text.len() + 1);
    if !attached {
        joined.push(' ');
    }
    let lowered = continues_sentence(before)
        .then(|| lowered_initial(first))
        .flatten();
    joined.push_str(lowered.as_deref().unwrap_or(first));
    for word in words {
        joined.push(' ');
        joined.push_str(word);
    }
    Some(joined)
}

/// Whether the text before the cursor leaves its sentence open, so that a capital at the cursor is
/// not a sentence's start: the last mark that is not white space is a lowercase or uppercase letter, a
/// digit, a comma or a semicolon, and no line break lies between it and the cursor. Nothing before the
/// cursor, only white space, a full stop or any other end mark, a closing quote or bracket, a dash, a
/// colon, a letter of a script without case (Han, Kana) and an inline atom all say no: the rule is
/// conservative, a capital is only lowered where the sentence plainly goes on.
fn continues_sentence(before: &str) -> bool {
    for c in before.chars().rev() {
        if LINE_BREAKS.contains(&c) {
            return false;
        }
        if c.is_whitespace() {
            continue;
        }
        return c.is_lowercase() || c.is_uppercase() || c.is_numeric() || c == ',' || c == ';';
    }
    false
}

/// `word` with its capital lowered, `None` when it is not a word to lower: its first character must be an
/// uppercase letter, its second a lowercase letter, and no later character of the word an uppercase letter
/// (so a single letter, `I'm`, `NASA`, `McDonald` and `Jean-Pierre` stay).
fn lowered_initial(word: &str) -> Option<String> {
    let mut chars = word.chars();
    let first = chars.next()?;
    let rest = chars.as_str();
    let mut after = rest.chars();
    let second = after.next()?;
    if !first.is_uppercase() || !second.is_lowercase() || after.any(char::is_uppercase) {
        return None;
    }
    let mut lowered: String = first.to_lowercase().collect();
    lowered.push_str(rest);
    Some(lowered)
}

// ----- the hard cap ---------------------------------------------------------------------------

/// The samples in `ms` milliseconds at 16 kHz.
#[must_use]
pub fn samples_for_ms(ms: u32) -> usize {
    usize::try_from(u64::from(ms) * u64::from(SAMPLE_RATE_HZ) / 1_000).unwrap_or(usize::MAX)
}

/// The milliseconds of `samples` at 16 kHz (saturating).
#[must_use]
pub fn ms_for_samples(samples: u64) -> u32 {
    u32::try_from(samples.saturating_mul(1_000) / u64::from(SAMPLE_RATE_HZ)).unwrap_or(u32::MAX)
}

/// Where to cut `head` (at most one cap of audio): the centre of its quietest 50 ms window within the last
/// second, searched every 10 ms; `head.len()` for audio shorter than a window. The same rule the spike
/// measured.
#[must_use]
pub fn quietest_split(head: &[f32]) -> usize {
    let window = samples_for_ms(50);
    let hop = samples_for_ms(10);
    let from = head.len().saturating_sub(samples_for_ms(1_000));
    let mut best = head.len();
    let mut best_energy = f32::MAX;
    let mut at = from;
    while at + window <= head.len() {
        let energy: f32 = head[at..at + window]
            .iter()
            .map(|sample| sample * sample)
            .sum();
        if energy < best_energy {
            best_energy = energy;
            best = at + window / 2;
        }
        at += hop;
    }
    best
}

/// How to cut a segment that is longer than `cap_ms` into pieces of at most that long: ranges that cover
/// `0..samples.len()` in order, each cut at the quietest moment near the cap. A segment within the cap is
/// one range; an empty one has none. A cap under one second counts as one second.
#[must_use]
pub fn split_for_cap(samples: &[f32], cap_ms: u32) -> Vec<Range<usize>> {
    let cap = samples_for_ms(cap_ms.max(1_000));
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let rest = &samples[start..];
        let take = if rest.len() > cap {
            // Never a sliver: the cut is in the second half of the cap at least.
            quietest_split(&rest[..cap]).max(cap / 2)
        } else {
            rest.len()
        };
        ranges.push(start..start + take);
        start += take;
    }
    ranges
}

// ----- the pre-roll ring ----------------------------------------------------------------------

/// The last audio the sidecar heard, addressed by absolute sample index (the detector's segment starts
/// are such indices), so a segment can be given the audio just before it started.
#[derive(Debug)]
pub struct PreRollRing {
    capacity: usize,
    samples: VecDeque<f32>,
    /// The absolute index one past the newest sample.
    end: u64,
}

impl PreRollRing {
    /// A ring that keeps `retain_ms` milliseconds.
    #[must_use]
    pub fn new(retain_ms: u32) -> Self {
        Self {
            capacity: samples_for_ms(retain_ms).max(1),
            samples: VecDeque::new(),
            end: 0,
        }
    }

    /// Adds audio at the end, dropping the oldest beyond the capacity.
    pub fn push(&mut self, audio: &[f32]) {
        self.end += audio.len() as u64;
        if audio.len() >= self.capacity {
            self.samples.clear();
            self.samples.extend(&audio[audio.len() - self.capacity..]);
            return;
        }
        let overflow = (self.samples.len() + audio.len()).saturating_sub(self.capacity);
        self.samples.drain(..overflow);
        self.samples.extend(audio);
    }

    /// The absolute index one past the newest sample: how many samples were pushed.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.end
    }

    /// Up to `want` samples that came just before the absolute index `index`; fewer (even none) when the
    /// stream began later or the ring no longer holds them.
    #[must_use]
    pub fn before(&self, index: u64, want: usize) -> Vec<f32> {
        let oldest = self.end - self.samples.len() as u64;
        let to = index.min(self.end);
        let from = to.saturating_sub(want as u64).max(oldest);
        if from >= to {
            return Vec::new();
        }
        let skip = usize::try_from(from - oldest).unwrap_or(0);
        let count = usize::try_from(to - from).unwrap_or(0);
        self.samples
            .iter()
            .skip(skip)
            .take(count)
            .copied()
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Vector {
        name: String,
        before: String,
        text: String,
        expected: Option<String>,
    }

    #[test]
    fn join_phrase_follows_the_shared_vectors() {
        let vectors: Vec<Vector> =
            serde_json::from_str(include_str!("../testdata/join_phrase.json")).unwrap();
        assert!(vectors.len() >= 100);
        for vector in vectors {
            assert_eq!(
                join_phrase(&vector.before, &vector.text),
                vector.expected,
                "{}",
                vector.name
            );
        }
    }

    #[test]
    fn a_capital_is_lowered_only_where_the_sentence_plainly_goes_on() {
        assert_eq!(
            join_phrase("I went to the", "Store and more."),
            Some(" store and more.".to_owned())
        );
        // A sentence that ended, a new line, the start of the block: the capital stays.
        for before in [
            "Done.", "Done. ", "Really?", "Wow!", "Well…", "end\n", "", "  ",
        ] {
            let joined = join_phrase(before, "Then we left.").unwrap();
            assert!(joined.trim_start().starts_with("Then"), "{before:?}");
        }
        // Words the rule leaves alone, even mid-sentence.
        for word in ["I", "I'm", "NASA", "OK", "McDonald", "Jean-Pierre", "42"] {
            assert_eq!(
                join_phrase("we saw", word),
                Some(format!(" {word}")),
                "{word}"
            );
        }
    }

    #[test]
    fn the_caps_and_defaults_are_the_measured_ones() {
        assert_eq!(HARD_CAP_PARAKEET_MS, 15_000);
        assert_eq!(HARD_CAP_MOONSHINE_MS, 25_000);
        assert_eq!(HARD_CAP_OLLAMA_MS, 15_000);
        assert_eq!(PREROLL_MS, 150);
        assert_eq!(IDLE_EXIT, Duration::from_secs(60));
        let vad = crate::protocol::VadParams::default();
        assert!(vad.is_valid());
        assert_eq!(vad.min_silence_s, 0.5);
        assert_eq!(vad.max_speech_s, 20.0);
        assert_eq!(MAX_AUDIO_FRAME_SAMPLES, samples_for_ms(1_000));
        assert_eq!(MAX_PUSH_BYTES, 2 * samples_for_ms(2_000));
        const { assert!(PREROLL_RETAIN_MS > 30_000) };
    }

    #[test]
    fn sample_and_millisecond_conversions_agree() {
        assert_eq!(samples_for_ms(1_000), 16_000);
        assert_eq!(samples_for_ms(150), 2_400);
        assert_eq!(ms_for_samples(16_000), 1_000);
        assert_eq!(ms_for_samples(2_400), 150);
        assert_eq!(ms_for_samples(0), 0);
        assert_eq!(ms_for_samples(u64::MAX), u32::MAX);
    }

    /// `seconds` of a tone with a quiet 100 ms gap centred at `gap_at` seconds.
    fn speech_with_gap(seconds: f32, gap_at: f32) -> Vec<f32> {
        let total = (seconds * 16_000.0) as usize;
        let gap = (gap_at * 16_000.0) as usize;
        (0..total)
            .map(|i| {
                if i.abs_diff(gap) < 800 {
                    0.0
                } else {
                    0.5 * ((i as f32) * 0.05).sin()
                }
            })
            .collect()
    }

    #[test]
    fn a_segment_within_the_cap_is_not_cut() {
        let audio = speech_with_gap(10.0, 5.0);
        assert_eq!(split_for_cap(&audio, 15_000), vec![0..audio.len()]);
        assert_eq!(split_for_cap(&audio, 10_000), vec![0..audio.len()]);
        assert!(split_for_cap(&[], 15_000).is_empty());
    }

    #[test]
    fn a_long_segment_is_cut_at_the_quiet_moment_before_the_cap() {
        // The gap is 14.5 s in, inside the last second before the 15 s cap.
        let audio = speech_with_gap(20.0, 14.5);
        let ranges = split_for_cap(&audio, 15_000);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges[1].end, audio.len());
        assert_eq!(ranges[0].end, ranges[1].start);
        let cut = ranges[0].end;
        assert!(
            cut.abs_diff(14_500 * 16) <= 800,
            "cut at sample {cut}, the gap is at {}",
            14_500 * 16
        );
    }

    #[test]
    fn the_pieces_cover_the_segment_and_none_is_over_the_cap() {
        for (seconds, cap_ms) in [
            (16.0, 15_000),
            (31.0, 15_000),
            (60.0, 25_000),
            (100.0, 1_000),
            (7.3, 3_000),
        ] {
            let audio = speech_with_gap(seconds, 3.0);
            let ranges = split_for_cap(&audio, cap_ms);
            let cap = samples_for_ms(cap_ms.max(1_000));
            let mut at = 0;
            for range in &ranges {
                assert_eq!(range.start, at);
                assert!(range.end > range.start);
                assert!(
                    range.len() <= cap,
                    "{seconds} s at {cap_ms}: piece of {}",
                    range.len()
                );
                at = range.end;
            }
            assert_eq!(at, audio.len(), "{seconds} s at {cap_ms}");
        }
    }

    #[test]
    fn continuous_speech_is_still_cut_in_the_second_half_of_the_cap_at_most() {
        let audio: Vec<f32> = (0..16_000 * 40)
            .map(|i| 0.4 * ((i as f32) * 0.07).sin())
            .collect();
        let ranges = split_for_cap(&audio, 15_000);
        assert!(ranges.len() >= 3);
        for range in &ranges[..ranges.len() - 1] {
            assert!(range.len() >= samples_for_ms(7_500));
        }
    }

    #[test]
    fn the_ring_keeps_the_newest_audio_by_absolute_index() {
        let mut ring = PreRollRing::new(1_000); // 16,000 samples
        let audio: Vec<f32> = (0..40_000).map(|i| i as f32).collect();
        for piece in audio.chunks(1_600) {
            ring.push(piece);
        }
        assert_eq!(ring.end(), 40_000);
        // Held: 24,000..40,000.
        assert_eq!(
            ring.before(30_000, 3).as_slice(),
            &[29_997.0, 29_998.0, 29_999.0]
        );
        assert_eq!(ring.before(40_000, 2).as_slice(), &[39_998.0, 39_999.0]);
        // Partly gone: only what is still there.
        let partly = ring.before(24_002, 10);
        assert_eq!(partly.as_slice(), &[24_000.0, 24_001.0]);
        // Gone, or not yet heard.
        assert!(ring.before(23_000, 100).is_empty());
        assert!(ring.before(0, 100).is_empty());
        assert_eq!(ring.before(99_999, 1).as_slice(), &[39_999.0]);
        assert!(ring.before(30_000, 0).is_empty());
    }

    #[test]
    fn the_ring_handles_a_push_bigger_than_itself_and_an_empty_start() {
        let mut ring = PreRollRing::new(1_000);
        assert!(ring.before(10, 5).is_empty());
        let big: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        ring.push(&big);
        assert_eq!(ring.end(), 20_000);
        assert_eq!(ring.before(20_000, 1).as_slice(), &[19_999.0]);
        assert!(ring.before(4_000, 10).is_empty());
        assert_eq!(ring.before(4_001, 10).as_slice(), &[4_000.0]);
    }

    #[test]
    fn a_pre_roll_before_the_start_of_the_stream_is_short() {
        let mut ring = PreRollRing::new(5_000);
        ring.push(&[1.0, 2.0, 3.0]);
        assert_eq!(ring.before(2, 2400).as_slice(), &[1.0, 2.0]);
    }
}
