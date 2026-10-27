//! The logic of the sidecar's speech pipeline (ADR-0053, `docs/architecture/dictation.md` section 3.4), written
//! against two small traits so that it runs, and is tested, without the sherpa-onnx DLLs: a [`Detector`] (the
//! voice activity detector, Silero in the binary) and a [`Recognizer`] (Parakeet or Moonshine in the binary).
//! `src/sherpa_pipeline.rs` implements the traits over sherpa-onnx and hands the result to [`run`](crate::run).
//!
//! What [`SegmentPipeline`] does with the audio it is given, in 100 ms steps:
//!
//! - It keeps every sample in a [`PreRollRing`] (45 s), counts the samples fed, and feeds the detector.
//! - **`SpeechStarted`** is sent when the detector says speech began (a rising edge). A blip shorter than the
//!   detector's minimum speech produces this and nothing else.
//! - For each segment the detector finishes, **`SpeechEnded`** is sent right before it is recognised, then the
//!   segment is given the `preroll_ms` of audio that came just before its start (the detector's start can clip
//!   a short first word), cut by [`split_for_cap`] at the quietest moment near the hard cap when it is longer,
//!   and either recognised piece by piece (the texts of the pieces joined with one space into **one** `Text`,
//!   or one `Empty` when every piece is empty) or, with no recognizer, sent back as one `SegmentAudio` per
//!   piece (the Ollama engine).
//! - A recognition that fails is an `Error { RecognizerFailed }` (a fixed sentence) and the session goes on;
//!   the segment then still ends in an `Empty` when nothing was recognised, so the shell's state never sticks.
//! - **`flush`** is the detector's flush (the speech in progress ends where the audio ends) and then the same
//!   draining. Measured with sherpa-onnx 1.13.8: the segment ends at the last sample fed, so a last word spoken
//!   just before the person pressed Stop is kept; no silence has to be added.
//!
//! The times of the messages are milliseconds of audio since the first sample after `Start`.
//!
//! Nothing here logs or holds what was said except the text on its way out, and no message of this module holds
//! a path or a model name.

use kb_speech::policy::{
    CHUNK_MS, PREROLL_RETAIN_MS, PreRollRing, ms_for_samples, samples_for_ms, split_for_cap,
};
use kb_speech::protocol::{FromSidecar, SidecarErrorCode, StartConfig};

use crate::host_loop::{Pipeline, Sink, SinkClosed};

/// One finished piece of speech, as the detector hands it over.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// Where the detector says the speech started: an absolute sample index since the first sample fed.
    pub start: u64,
    /// The samples of the speech (16 kHz mono, `-1.0..1.0`).
    pub samples: Vec<f32>,
}

/// The voice activity detector.
pub trait Detector {
    /// Takes more audio (any length).
    fn accept(&mut self, samples: &[f32]);

    /// Whether speech is in progress right now.
    fn detected(&self) -> bool;

    /// The oldest finished segment, removed from the detector; `None` when there is none.
    fn next_segment(&mut self) -> Option<Segment>;

    /// Ends the speech in progress and makes it a segment, as far as the detector can tell it ended.
    fn flush(&mut self);
}

/// A recognition that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecognitionFailed;

/// The recognizer: one piece of audio in, its text out.
pub trait Recognizer {
    /// The text of `samples`, trimmed; an empty string for no speech.
    ///
    /// # Errors
    ///
    /// [`RecognitionFailed`] when the recognizer could not decode the audio.
    fn recognise(&mut self, samples: &[f32]) -> Result<String, RecognitionFailed>;
}

/// The fixed sentence of `Error { RecognizerFailed }`.
const RECOGNITION_FAILED: &str = "a speech segment could not be recognised";

/// `i16` samples as `f32` in `-1.0..1.0`.
#[must_use]
pub fn pcm_to_f32(pcm: &[i16]) -> Vec<f32> {
    pcm.iter()
        .map(|sample| f32::from(*sample) / 32_768.0)
        .collect()
}

/// `f32` samples as `i16` (clamped), the inverse of [`pcm_to_f32`] for what it produces.
#[must_use]
pub fn f32_to_pcm(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        // The clamp keeps the value in range, so the cast loses nothing but the fraction.
        .map(|sample| (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)
        .collect()
}

/// The detector and the recognizer of one session, and what turns their output into protocol messages.
pub struct SegmentPipeline {
    detector: Box<dyn Detector>,
    recognizer: Option<Box<dyn Recognizer>>,
    ring: PreRollRing,
    hard_cap_ms: u32,
    preroll_samples: usize,
    /// How many samples were fed (the pre-roll ring's `end`, kept apart for the times).
    fed: u64,
    /// Whether the last look at the detector said speech was in progress.
    speaking: bool,
}

impl SegmentPipeline {
    /// A pipeline over `detector`, with `recognizer` (`None` for the Ollama engine: segments go back as audio),
    /// cutting and padding as `config` says.
    #[must_use]
    pub fn new(
        detector: Box<dyn Detector>,
        recognizer: Option<Box<dyn Recognizer>>,
        config: &StartConfig,
    ) -> Self {
        Self {
            detector,
            recognizer,
            ring: PreRollRing::new(PREROLL_RETAIN_MS),
            hard_cap_ms: config.hard_cap_ms,
            preroll_samples: samples_for_ms(config.preroll_ms),
            fed: 0,
            speaking: false,
        }
    }

    /// Feeds `samples` to the ring and the detector in steps of [`CHUNK_MS`], reporting each start of speech and
    /// recognising each segment as soon as the detector finishes it.
    fn feed(&mut self, samples: &[f32], sink: &mut dyn Sink) -> Result<(), SinkClosed> {
        for chunk in samples.chunks(samples_for_ms(CHUNK_MS)) {
            self.ring.push(chunk);
            self.detector.accept(chunk);
            self.fed += chunk.len() as u64;
            let speaking = self.detector.detected();
            if speaking && !self.speaking {
                sink.send(FromSidecar::SpeechStarted)?;
            }
            self.speaking = speaking;
            self.drain(sink)?;
        }
        Ok(())
    }

    /// Takes every finished segment from the detector, in order.
    fn drain(&mut self, sink: &mut dyn Sink) -> Result<(), SinkClosed> {
        while let Some(segment) = self.detector.next_segment() {
            self.process(&segment, sink)?;
        }
        Ok(())
    }

    /// Recognises (or sends back) one segment.
    fn process(&mut self, segment: &Segment, sink: &mut dyn Sink) -> Result<(), SinkClosed> {
        let mut audio = self.ring.before(segment.start, self.preroll_samples);
        // Where the audio (pre-roll included) starts in the stream.
        let base = segment.start.saturating_sub(audio.len() as u64);
        audio.extend_from_slice(&segment.samples);
        if audio.is_empty() {
            return Ok(());
        }
        sink.send(FromSidecar::SpeechEnded)?;
        let pieces = split_for_cap(&audio, self.hard_cap_ms);
        let Some(recognizer) = self.recognizer.as_mut() else {
            for piece in pieces {
                sink.send(FromSidecar::SegmentAudio {
                    pcm: f32_to_pcm(&audio[piece.clone()]),
                    start_ms: ms_for_samples(base + piece.start as u64),
                    end_ms: ms_for_samples(base + piece.end as u64),
                })?;
            }
            return Ok(());
        };
        let start_ms = ms_for_samples(segment.start);
        let end_ms = ms_for_samples(segment.start + segment.samples.len() as u64);
        let mut text = String::new();
        let mut failed = false;
        for piece in pieces {
            match recognizer.recognise(&audio[piece]) {
                Ok(piece_text) => {
                    let piece_text = piece_text.trim();
                    if !piece_text.is_empty() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(piece_text);
                    }
                }
                Err(RecognitionFailed) => failed = true,
            }
        }
        if failed {
            sink.send(FromSidecar::Error {
                code: SidecarErrorCode::RecognizerFailed,
                message: RECOGNITION_FAILED.to_owned(),
            })?;
        }
        if text.is_empty() {
            sink.send(FromSidecar::Empty { start_ms, end_ms })
        } else {
            sink.send(FromSidecar::Text {
                text,
                start_ms,
                end_ms,
            })
        }
    }
}

impl Pipeline for SegmentPipeline {
    fn audio(&mut self, pcm: &[i16], sink: &mut dyn Sink) -> Result<(), SinkClosed> {
        self.feed(&pcm_to_f32(pcm), sink)
    }

    fn flush(&mut self, sink: &mut dyn Sink) -> Result<(), SinkClosed> {
        self.detector.flush();
        self.drain(sink)?;
        self.speaking = self.detector.detected();
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    // ----- fakes -----

    /// A detector that hears speech where the samples are loud (above 0.1), ends a segment after `min_silence`
    /// samples of quiet, and flushes the way sherpa-onnx 1.13.8's does: the open segment ends at the tail.
    struct EnergyDetector {
        min_silence: usize,
        pos: u64,
        history: Vec<f32>,
        start: Option<u64>,
        last_loud: u64,
        queue: VecDeque<Segment>,
    }

    impl EnergyDetector {
        fn new(min_silence: usize) -> Self {
            Self {
                min_silence,
                pos: 0,
                history: Vec::new(),
                start: None,
                last_loud: 0,
                queue: VecDeque::new(),
            }
        }

        fn close(&mut self, end: u64) {
            if let Some(start) = self.start.take() {
                let from = usize::try_from(start).unwrap();
                let to = usize::try_from(end).unwrap();
                if to > from {
                    self.queue.push_back(Segment {
                        start,
                        samples: self.history[from..to].to_vec(),
                    });
                }
            }
        }
    }

    impl Detector for EnergyDetector {
        fn accept(&mut self, samples: &[f32]) {
            for sample in samples {
                self.history.push(*sample);
                self.pos += 1;
                if sample.abs() > 0.1 {
                    if self.start.is_none() {
                        self.start = Some(self.pos - 1);
                    }
                    self.last_loud = self.pos;
                } else if self.start.is_some()
                    && self.pos - self.last_loud >= self.min_silence as u64
                {
                    // The segment ends where the speech did, the silence is not in it.
                    self.close(self.last_loud);
                }
            }
        }

        fn detected(&self) -> bool {
            self.start.is_some()
        }

        fn next_segment(&mut self) -> Option<Segment> {
            self.queue.pop_front()
        }

        fn flush(&mut self) {
            self.close(self.pos);
        }
    }

    /// What the recognizer was given, and what it answers.
    #[derive(Clone, Default)]
    struct Heard(Arc<Mutex<Vec<Vec<f32>>>>);

    struct Scripted {
        heard: Heard,
        answers: VecDeque<Result<String, RecognitionFailed>>,
    }

    impl Recognizer for Scripted {
        fn recognise(&mut self, samples: &[f32]) -> Result<String, RecognitionFailed> {
            self.heard.0.lock().unwrap().push(samples.to_vec());
            self.answers
                .pop_front()
                .unwrap_or_else(|| Ok(format!("{} samples", samples.len())))
        }
    }

    #[derive(Default)]
    struct Collect(Vec<FromSidecar>);

    impl Sink for Collect {
        fn send(&mut self, message: FromSidecar) -> Result<(), SinkClosed> {
            self.0.push(message);
            Ok(())
        }
    }

    struct Closed;

    impl Sink for Closed {
        fn send(&mut self, _message: FromSidecar) -> Result<(), SinkClosed> {
            Err(SinkClosed)
        }
    }

    fn config(hard_cap_ms: u32, preroll_ms: u32) -> StartConfig {
        let mut config = kb_speech::catalog::SpeechEngine::Accurate
            .start_config(std::path::Path::new("m"), std::path::Path::new("v.onnx"));
        config.hard_cap_ms = hard_cap_ms;
        config.preroll_ms = preroll_ms;
        config
    }

    const MIN_SILENCE: usize = 8_000; // 0.5 s

    fn pipeline_with(
        hard_cap_ms: u32,
        preroll_ms: u32,
        answers: Vec<Result<String, RecognitionFailed>>,
    ) -> (SegmentPipeline, Heard) {
        let heard = Heard::default();
        let recognizer = Scripted {
            heard: heard.clone(),
            answers: answers.into(),
        };
        let pipeline = SegmentPipeline::new(
            Box::new(EnergyDetector::new(MIN_SILENCE)),
            Some(Box::new(recognizer)),
            &config(hard_cap_ms, preroll_ms),
        );
        (pipeline, heard)
    }

    fn vad_only(hard_cap_ms: u32, preroll_ms: u32) -> SegmentPipeline {
        SegmentPipeline::new(
            Box::new(EnergyDetector::new(MIN_SILENCE)),
            None,
            &config(hard_cap_ms, preroll_ms),
        )
    }

    /// `ms` milliseconds of quiet.
    fn quiet(ms: u32) -> Vec<i16> {
        vec![0; samples_for_ms(ms)]
    }

    /// `ms` milliseconds of loud noise: every sample is about 0.3 of full scale, so each one counts as speech.
    fn loud(ms: u32) -> Vec<i16> {
        (0..samples_for_ms(ms))
            .map(|i| if i % 2 == 0 { 10_000 } else { -10_000 })
            .collect()
    }

    fn concat(parts: &[Vec<i16>]) -> Vec<i16> {
        parts.iter().flatten().copied().collect()
    }

    fn feed(pipeline: &mut dyn Pipeline, sink: &mut Collect, pcm: &[i16]) {
        pipeline.audio(pcm, sink).unwrap();
    }

    fn kinds(sink: &Collect) -> Vec<&'static str> {
        sink.0
            .iter()
            .map(|message| match message {
                FromSidecar::SpeechStarted => "started",
                FromSidecar::SpeechEnded => "ended",
                FromSidecar::Text { .. } => "text",
                FromSidecar::SegmentAudio { .. } => "audio",
                FromSidecar::Empty { .. } => "empty",
                FromSidecar::Error { .. } => "error",
                FromSidecar::Ready { .. } => "ready",
                FromSidecar::Flushed => "flushed",
            })
            .collect()
    }

    // ----- tests -----

    #[test]
    fn a_segment_is_started_ended_and_recognised_in_that_order() {
        let (mut pipeline, heard) = pipeline_with(15_000, 0, vec![Ok("Hello there.".into())]);
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[quiet(300), loud(1_000), quiet(900)]),
        );
        assert_eq!(kinds(&sink), ["started", "ended", "text"]);
        assert_eq!(
            sink.0[2],
            FromSidecar::Text {
                text: "Hello there.".into(),
                start_ms: 300,
                end_ms: 1_300,
            }
        );
        assert_eq!(heard.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn two_segments_are_two_texts_in_order() {
        let (mut pipeline, _) =
            pipeline_with(15_000, 0, vec![Ok("One.".into()), Ok("Two.".into())]);
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[loud(700), quiet(900), loud(700), quiet(900)]),
        );
        assert_eq!(
            kinds(&sink),
            ["started", "ended", "text", "started", "ended", "text"]
        );
        let texts: Vec<_> = sink
            .0
            .iter()
            .filter_map(|message| match message {
                FromSidecar::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["One.", "Two."]);
    }

    #[test]
    fn the_pre_roll_is_added_before_the_start_of_the_speech() {
        let (mut pipeline, heard) = pipeline_with(15_000, 150, vec![]);
        let mut sink = Collect::default();
        // A little quiet "room tone" so the pre-roll is told apart from the tone and from zeros.
        let room: Vec<i16> = (0..samples_for_ms(500)).map(|i| (i % 50) as i16).collect();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[room.clone(), loud(800), quiet(900)]),
        );
        let heard = heard.0.lock().unwrap();
        assert_eq!(heard.len(), 1);
        let audio = &heard[0];
        let pre = samples_for_ms(150);
        assert_eq!(audio.len(), pre + samples_for_ms(800));
        // The first samples are the last 150 ms of the room tone.
        let expected = pcm_to_f32(&room[room.len() - pre..]);
        assert_eq!(&audio[..pre], expected.as_slice());
        // The text's times are the segment's own, without the pre-roll.
        match &sink.0[2] {
            FromSidecar::Text {
                start_ms, end_ms, ..
            } => assert_eq!((*start_ms, *end_ms), (500, 1_300)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_pre_roll_before_the_start_of_the_stream_is_only_what_exists() {
        let (mut pipeline, heard) = pipeline_with(15_000, 150, vec![]);
        let mut sink = Collect::default();
        // Speech from the very first sample, then silence: nothing before it to prepend.
        feed(&mut pipeline, &mut sink, &concat(&[loud(600), quiet(900)]));
        assert_eq!(heard.0.lock().unwrap()[0].len(), samples_for_ms(600));
        // And 50 ms of quiet before: only those 50 ms.
        let (mut pipeline, heard) = pipeline_with(15_000, 150, vec![]);
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[quiet(50), loud(600), quiet(900)]),
        );
        assert_eq!(heard.0.lock().unwrap()[0].len(), samples_for_ms(650));
    }

    #[test]
    fn a_segment_over_the_hard_cap_is_cut_and_its_texts_are_joined_with_one_space() {
        let (mut pipeline, heard) = pipeline_with(
            2_000,
            100,
            vec![
                Ok("first piece".into()),
                Ok(" second".into()),
                Ok("third ".into()),
            ],
        );
        let mut sink = Collect::default();
        // 5 s of continuous tone is cut into pieces of at most 2 s.
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[quiet(200), loud(5_000), quiet(900)]),
        );
        let heard = heard.0.lock().unwrap();
        assert!(heard.len() >= 3, "{} pieces", heard.len());
        assert!(
            heard
                .iter()
                .all(|piece| piece.len() <= samples_for_ms(2_000))
        );
        // The pre-roll belongs to the first piece: the pieces together are pre-roll + speech.
        assert_eq!(
            heard.iter().map(Vec::len).sum::<usize>(),
            samples_for_ms(100) + samples_for_ms(5_000)
        );
        // One ended, one text with the pieces joined by a single space.
        assert_eq!(kinds(&sink), ["started", "ended", "text"]);
        match &sink.0[2] {
            FromSidecar::Text { text, .. } => {
                assert!(text.starts_with("first piece second third"), "{text:?}");
                assert!(!text.contains("  "));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_pieces_are_dropped_and_an_all_empty_segment_is_one_empty() {
        // Pieces: "a", "", "b" -> "a b".
        let (mut pipeline, _) = pipeline_with(
            2_000,
            0,
            vec![
                Ok("a".into()),
                Ok(String::new()),
                Ok("   ".into()),
                Ok("b".into()),
            ],
        );
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[loud(5_000), quiet(900)]),
        );
        assert_eq!(kinds(&sink), ["started", "ended", "text"]);
        match &sink.0[2] {
            FromSidecar::Text { text, .. } => assert_eq!(text, "a b"),
            other => panic!("{other:?}"),
        }
        // Every piece empty: no text, exactly one Empty, never an empty Text.
        let (mut pipeline, _) = pipeline_with(2_000, 0, vec![Ok(String::new()); 4]);
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[loud(5_000), quiet(900)]),
        );
        assert_eq!(kinds(&sink), ["started", "ended", "empty"]);
    }

    #[test]
    fn a_failed_recognition_is_reported_and_the_session_goes_on() {
        let (mut pipeline, _) =
            pipeline_with(15_000, 0, vec![Err(RecognitionFailed), Ok("Fine.".into())]);
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[loud(700), quiet(900), loud(700), quiet(900)]),
        );
        assert_eq!(
            kinds(&sink),
            [
                "started", "ended", "error", "empty", "started", "ended", "text"
            ]
        );
        match &sink.0[2] {
            FromSidecar::Error { code, message } => {
                assert_eq!(*code, SidecarErrorCode::RecognizerFailed);
                assert_eq!(message, RECOGNITION_FAILED);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_piece_that_fails_among_good_ones_keeps_the_good_text() {
        let (mut pipeline, _) = pipeline_with(
            2_000,
            0,
            vec![
                Ok("kept".into()),
                Err(RecognitionFailed),
                Ok("also kept".into()),
            ],
        );
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[loud(4_500), quiet(900)]),
        );
        assert_eq!(kinds(&sink), ["started", "ended", "error", "text"]);
        match &sink.0[3] {
            FromSidecar::Text { text, .. } => assert!(text.starts_with("kept also kept")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn without_a_recognizer_each_piece_goes_back_as_audio_with_its_own_times() {
        let mut pipeline = vad_only(2_000, 100);
        let mut sink = Collect::default();
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[quiet(1_000), loud(5_000), quiet(900)]),
        );
        let pieces: Vec<(&Vec<i16>, u32, u32)> = sink
            .0
            .iter()
            .filter_map(|message| match message {
                FromSidecar::SegmentAudio {
                    pcm,
                    start_ms,
                    end_ms,
                } => Some((pcm, *start_ms, *end_ms)),
                _ => None,
            })
            .collect();
        assert!(pieces.len() >= 3);
        assert_eq!(kinds(&sink)[..2], ["started", "ended"]);
        assert!(!kinds(&sink).contains(&"text"));
        // The first piece starts at the pre-roll (900 ms = 1,000 - 100), the pieces are contiguous, none is over
        // the cap, and a piece's samples match its times.
        assert_eq!(pieces[0].1, 900);
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].2, pair[1].1);
        }
        for (pcm, start_ms, end_ms) in &pieces {
            assert!(pcm.len() <= samples_for_ms(2_000));
            assert_eq!(pcm.len(), samples_for_ms(end_ms - start_ms));
        }
        assert_eq!(pieces.last().unwrap().2, 6_000);
    }

    #[test]
    fn a_blip_the_detector_drops_is_a_start_and_nothing_else() {
        struct Blip {
            on: bool,
        }
        impl Detector for Blip {
            fn accept(&mut self, _samples: &[f32]) {
                self.on = !self.on;
            }
            fn detected(&self) -> bool {
                self.on
            }
            fn next_segment(&mut self) -> Option<Segment> {
                None
            }
            fn flush(&mut self) {}
        }
        let mut pipeline = SegmentPipeline::new(
            Box::new(Blip { on: false }),
            Some(Box::new(Scripted {
                heard: Heard::default(),
                answers: VecDeque::new(),
            })),
            &config(15_000, 150),
        );
        let mut sink = Collect::default();
        // Two chunks: speech seen, then gone, no segment.
        feed(&mut pipeline, &mut sink, &quiet(200));
        assert_eq!(kinds(&sink), ["started"]);
    }

    #[test]
    fn a_flush_ends_the_speech_in_progress_and_keeps_its_last_words() {
        let (mut pipeline, heard) = pipeline_with(15_000, 0, vec![Ok("Done.".into())]);
        let mut sink = Collect::default();
        // The person stops while still speaking (only 200 ms of quiet, less than the 500 ms the detector wants).
        feed(
            &mut pipeline,
            &mut sink,
            &concat(&[loud(1_000), quiet(200)]),
        );
        assert_eq!(kinds(&sink), ["started"]);
        pipeline.flush(&mut sink).unwrap();
        assert_eq!(kinds(&sink), ["started", "ended", "text"]);
        // Everything that was fed reached the recognizer, the last word included.
        assert_eq!(heard.0.lock().unwrap()[0].len(), samples_for_ms(1_200));
    }

    #[test]
    fn a_flush_with_no_speech_sends_nothing_and_the_session_goes_on() {
        let (mut pipeline, heard) = pipeline_with(15_000, 0, vec![]);
        let mut sink = Collect::default();
        feed(&mut pipeline, &mut sink, &quiet(500));
        pipeline.flush(&mut sink).unwrap();
        assert!(sink.0.is_empty());
        // The detector keeps working afterwards.
        feed(&mut pipeline, &mut sink, &concat(&[loud(600), quiet(900)]));
        assert_eq!(kinds(&sink), ["started", "ended", "text"]);
        assert_eq!(heard.0.lock().unwrap().len(), 1);
        // After a flush that ended speech, a second flush is a no-op and later speech still works.
        pipeline.flush(&mut sink).unwrap();
        feed(&mut pipeline, &mut sink, &concat(&[loud(300), quiet(100)]));
        pipeline.flush(&mut sink).unwrap();
        assert_eq!(
            kinds(&sink),
            ["started", "ended", "text", "started", "ended", "text"]
        );
    }

    #[test]
    fn the_audio_may_come_in_pieces_of_any_size() {
        let whole = concat(&[quiet(300), loud(900), quiet(900), loud(500), quiet(900)]);
        let run = |size: usize| {
            let (mut pipeline, _) = pipeline_with(15_000, 150, vec![]);
            let mut sink = Collect::default();
            for piece in whole.chunks(size) {
                pipeline.audio(piece, &mut sink).unwrap();
            }
            sink.0
        };
        let reference = run(1_600);
        assert_eq!(reference.len(), 6);
        for size in [1, 7, 512, 16_000, whole.len()] {
            assert_eq!(run(size), reference, "pieces of {size}");
        }
    }

    #[test]
    fn a_closed_sink_ends_the_call() {
        let (mut pipeline, _) = pipeline_with(15_000, 0, vec![]);
        let result = pipeline.audio(&concat(&[loud(500), quiet(900)]), &mut Closed);
        assert_eq!(result, Err(SinkClosed));
    }

    #[test]
    fn the_conversions_between_i16_and_f32_are_exact() {
        let pcm: Vec<i16> = vec![i16::MIN, -12_345, -1, 0, 1, 12_345, i16::MAX];
        assert_eq!(f32_to_pcm(&pcm_to_f32(&pcm)), pcm);
        assert_eq!(f32_to_pcm(&[2.0, -2.0]), [i16::MAX, i16::MIN]);
        assert_eq!(pcm_to_f32(&[16_384]), [0.5]);
    }
}
