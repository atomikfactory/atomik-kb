//! The frame protocol between the desktop shell and the `kb-speech` sidecar (ADR-0053,
//! `docs/architecture/dictation.md` section 3): the sidecar's stdin carries [`ToSidecar`] frames, its
//! stdout [`FromSidecar`] frames.
//!
//! A frame is `tag: u8`, `length: u32` little-endian, then `length` bytes of payload, at most
//! [`MAX_FRAME_BYTES`] (1 MiB). Control payloads are JSON; audio is raw 16 kHz mono `i16`
//! little-endian. [`FrameReader`] turns any slicing of the byte stream back into frames; a malformed
//! frame is an error that cannot be recovered from (the stream has lost its place), so the reader keeps
//! answering it and the owner ends the session.
//!
//! Nothing here holds text the person said except [`FromSidecar::Text`], and no error message holds any.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::policy;

/// The most payload bytes one frame carries (1 MiB).
pub const MAX_FRAME_BYTES: usize = 1 << 20;

/// The bytes before the payload: the tag and the length.
pub const HEADER_BYTES: usize = 5;

/// The sample rate of every audio payload.
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// The bytes of the start and end times (milliseconds, `u32` little-endian each) a
/// [`FromSidecar::SegmentAudio`] payload begins with.
pub const SEGMENT_HEADER_BYTES: usize = 8;

/// The longest hard segment cap a [`StartConfig`] may ask for, in milliseconds. With the longest
/// pre-roll, a segment of that length still fits in one frame (see the test).
pub const MAX_HARD_CAP_MS: u32 = 30_000;

/// The longest pre-roll a [`StartConfig`] may ask for, in milliseconds.
pub const MAX_PREROLL_MS: u32 = 1_000;

/// Frame tags. Shell to sidecar below `0x80`, sidecar to shell from `0x80`.
pub mod tag {
    /// `ToSidecar::Start`: JSON [`StartConfig`](super::StartConfig).
    pub const START: u8 = 0x01;
    /// `ToSidecar::Audio`: raw `i16` little-endian samples.
    pub const AUDIO: u8 = 0x02;
    /// `ToSidecar::Flush`: empty.
    pub const FLUSH: u8 = 0x03;
    /// `ToSidecar::Quit`: empty.
    pub const QUIT: u8 = 0x04;
    /// `FromSidecar::Ready`: JSON `{ "load_ms" }`.
    pub const READY: u8 = 0x81;
    /// `FromSidecar::SpeechStarted`: empty.
    pub const SPEECH_STARTED: u8 = 0x82;
    /// `FromSidecar::SpeechEnded`: empty.
    pub const SPEECH_ENDED: u8 = 0x83;
    /// `FromSidecar::Text`: JSON `{ "text", "start_ms", "end_ms" }`.
    pub const TEXT: u8 = 0x84;
    /// `FromSidecar::SegmentAudio`: `start_ms` and `end_ms` as `u32` little-endian, then raw `i16` samples.
    pub const SEGMENT_AUDIO: u8 = 0x85;
    /// `FromSidecar::Flushed`: empty.
    pub const FLUSHED: u8 = 0x86;
    /// `FromSidecar::Error`: JSON `{ "code", "message" }`.
    pub const ERROR: u8 = 0x87;
    /// `FromSidecar::Empty`: JSON `{ "start_ms", "end_ms" }`.
    pub const EMPTY: u8 = 0x88;

    /// Whether `tag` is one of the tags above.
    #[must_use]
    pub const fn is_known(tag: u8) -> bool {
        matches!(
            tag,
            START
                | AUDIO
                | FLUSH
                | QUIT
                | READY
                | SPEECH_STARTED
                | SPEECH_ENDED
                | TEXT
                | SEGMENT_AUDIO
                | FLUSHED
                | ERROR
                | EMPTY
        )
    }
}

/// What went wrong with a frame. Never carries what was in it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    /// A frame announces more than [`MAX_FRAME_BYTES`] of payload, or a message is too big to send.
    #[error("a frame payload of {len} bytes is over the {MAX_FRAME_BYTES} byte limit")]
    Oversize {
        /// The announced or attempted payload length.
        len: usize,
    },
    /// The tag is not one of [`tag`], or it belongs to the other direction.
    #[error("unknown frame tag {0:#04x}")]
    UnknownTag(u8),
    /// A control payload is not the JSON its tag stands for.
    #[error("a control frame did not hold the JSON its tag needs")]
    BadPayload,
    /// An audio payload has an odd number of bytes.
    #[error("an audio payload has an odd number of bytes")]
    OddAudio,
    /// A frame that carries nothing carried a payload.
    #[error("a frame that has no payload carried one")]
    UnexpectedPayload,
    /// A segment-audio payload is shorter than its 8-byte header.
    #[error("a segment-audio payload is shorter than its header")]
    ShortSegment,
}

/// Why a [`StartConfig`] is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// A path is empty.
    #[error("a path in the start configuration is empty")]
    EmptyPath,
    /// The hard segment cap is outside 1 s to [`MAX_HARD_CAP_MS`].
    #[error("the hard segment cap is out of range")]
    HardCap,
    /// The pre-roll is over [`MAX_PREROLL_MS`].
    #[error("the pre-roll is out of range")]
    PreRoll,
    /// A thread count is not 1 to 8.
    #[error("a thread count is out of range")]
    Threads,
    /// A voice-activity parameter is out of range.
    #[error("a voice-activity parameter is out of range")]
    Vad,
}

// ----- configuration ------------------------------------------------------------------------

/// The files of a Parakeet TDT (NeMo transducer) model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParakeetPaths {
    /// `encoder.int8.onnx`.
    pub encoder: String,
    /// `decoder.int8.onnx`.
    pub decoder: String,
    /// `joiner.int8.onnx`.
    pub joiner: String,
    /// `tokens.txt`.
    pub tokens: String,
}

/// The files of a Moonshine v1 model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoonshinePaths {
    /// `preprocess.onnx`.
    pub preprocessor: String,
    /// `encode.int8.onnx`.
    pub encoder: String,
    /// `uncached_decode.int8.onnx`.
    pub uncached_decoder: String,
    /// `cached_decode.int8.onnx`.
    pub cached_decoder: String,
    /// `tokens.txt`.
    pub tokens: String,
}

/// What turns a speech segment into text inside the sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recognizer {
    /// NVIDIA Parakeet TDT (sherpa-onnx `nemo_transducer`, greedy search).
    Parakeet {
        /// Where the model files are.
        paths: ParakeetPaths,
    },
    /// Moonshine v1 (sherpa-onnx offline Moonshine).
    Moonshine {
        /// Where the model files are.
        paths: MoonshinePaths,
    },
    /// No recognizer: the sidecar only finds the speech and sends it back as
    /// [`FromSidecar::SegmentAudio`] (the Ollama engine).
    VadOnly,
}

/// The voice activity detector's settings (Silero, 16 kHz). The defaults are the measured ones
/// ([`policy`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VadParams {
    /// Speech probability above which a window counts as speech (0 to 1, exclusive of both).
    pub threshold: f32,
    /// Seconds of silence that end a segment.
    pub min_silence_s: f32,
    /// Seconds of speech below which a segment is dropped.
    pub min_speech_s: f32,
    /// Seconds after which a segment is cut at the next brief dip (a soft limit).
    pub max_speech_s: f32,
    /// Samples per window; Silero at 16 kHz takes 512.
    pub window_size: u32,
    /// Seconds of audio the detector's buffer holds.
    pub buffer_s: f32,
    /// onnxruntime threads of the detector.
    pub threads: u32,
}

impl Default for VadParams {
    fn default() -> Self {
        Self {
            threshold: policy::VAD_THRESHOLD,
            min_silence_s: policy::VAD_MIN_SILENCE_S,
            min_speech_s: policy::VAD_MIN_SPEECH_S,
            max_speech_s: policy::VAD_MAX_SPEECH_S,
            window_size: policy::VAD_WINDOW_SIZE,
            buffer_s: policy::VAD_BUFFER_S,
            threads: policy::VAD_THREADS,
        }
    }
}

impl VadParams {
    /// Whether every value is in range.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let seconds =
            |value: f32, min: f32, max: f32| value.is_finite() && value >= min && value <= max;
        self.threshold > 0.0
            && self.threshold < 1.0
            && seconds(self.min_silence_s, 0.05, 5.0)
            && seconds(self.min_speech_s, 0.0, 5.0)
            && seconds(self.max_speech_s, 1.0, 120.0)
            && self.window_size == policy::VAD_WINDOW_SIZE
            && seconds(self.buffer_s, 5.0, 120.0)
            && (1..=8).contains(&self.threads)
    }
}

/// The first frame the shell sends: what to load and how to cut the speech. The sidecar answers
/// [`FromSidecar::Ready`] when it has loaded it, or [`FromSidecar::Error`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartConfig {
    /// The recognizer, with the paths of its model files.
    pub recognizer: Recognizer,
    /// The path of `silero_vad.onnx`.
    pub vad_path: String,
    /// The detector's settings.
    pub vad: VadParams,
    /// onnxruntime threads of the recognizer (1 to 8).
    pub threads: u32,
    /// The longest audio one recognition (or one [`FromSidecar::SegmentAudio`]) covers, milliseconds. A
    /// longer segment is cut at its quietest moment near the cap.
    pub hard_cap_ms: u32,
    /// Audio before the detector's start of speech that is added to the segment, milliseconds.
    pub preroll_ms: u32,
}

impl StartConfig {
    /// Checks the ranges (the sidecar refuses a configuration that fails; the shell builds only valid ones).
    ///
    /// # Errors
    ///
    /// The first [`ConfigError`] found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let paths_empty = match &self.recognizer {
            Recognizer::Parakeet { paths } => {
                [&paths.encoder, &paths.decoder, &paths.joiner, &paths.tokens]
                    .iter()
                    .any(|path| path.is_empty())
            }
            Recognizer::Moonshine { paths } => [
                &paths.preprocessor,
                &paths.encoder,
                &paths.uncached_decoder,
                &paths.cached_decoder,
                &paths.tokens,
            ]
            .iter()
            .any(|path| path.is_empty()),
            Recognizer::VadOnly => false,
        };
        if paths_empty || self.vad_path.is_empty() {
            return Err(ConfigError::EmptyPath);
        }
        if !(1_000..=MAX_HARD_CAP_MS).contains(&self.hard_cap_ms) {
            return Err(ConfigError::HardCap);
        }
        if self.preroll_ms > MAX_PREROLL_MS {
            return Err(ConfigError::PreRoll);
        }
        if !(1..=8).contains(&self.threads) {
            return Err(ConfigError::Threads);
        }
        if !self.vad.is_valid() {
            return Err(ConfigError::Vad);
        }
        Ok(())
    }
}

// ----- messages -----------------------------------------------------------------------------

/// A message from the shell to the sidecar.
#[derive(Debug, Clone, PartialEq)]
pub enum ToSidecar {
    /// Load the models; must be the first message.
    Start(StartConfig),
    /// Audio: 16 kHz mono samples, in order.
    Audio(Vec<i16>),
    /// The person stopped: finish every segment, send what they said, then [`FromSidecar::Flushed`].
    Flush,
    /// Exit now.
    Quit,
}

/// Why the sidecar reports an [`FromSidecar::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarErrorCode {
    /// The first message was not a valid [`ToSidecar::Start`], or a message came before it.
    BadConfig,
    /// A model file could not be loaded (missing, damaged or the wrong kind).
    ModelLoadFailed,
    /// The voice activity detector could not be loaded.
    VadLoadFailed,
    /// The recognizer failed on a segment.
    RecognizerFailed,
    /// A frame could not be read; the stream is lost.
    BadFrame,
}

/// A message from the sidecar to the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum FromSidecar {
    /// The models are loaded; audio is now being cut and recognised.
    Ready {
        /// How long loading took, milliseconds.
        load_ms: u32,
    },
    /// The detector heard speech begin.
    SpeechStarted,
    /// The detector heard speech end (a pause); a segment follows at once.
    SpeechEnded,
    /// What a segment says. One per segment; never empty.
    Text {
        /// The recognised text (as the model wrote it; the shell joins phrases, see [`policy::join_phrase`]).
        text: String,
        /// Where the segment starts, milliseconds of audio since the first [`ToSidecar::Audio`].
        start_ms: u32,
        /// Where it ends.
        end_ms: u32,
    },
    /// A segment of speech for the shell to transcribe (the recognizer is [`Recognizer::VadOnly`]), at most
    /// the hard cap plus the pre-roll long.
    SegmentAudio {
        /// 16 kHz mono samples.
        pcm: Vec<i16>,
        /// Where the segment starts, as in [`FromSidecar::Text`].
        start_ms: u32,
        /// Where it ends.
        end_ms: u32,
    },
    /// A segment that came out as no text (noise); nothing to insert.
    Empty {
        /// Where the segment starts.
        start_ms: u32,
        /// Where it ends.
        end_ms: u32,
    },
    /// The answer to [`ToSidecar::Flush`]: everything before it has been sent.
    Flushed,
    /// Something failed; after `BadConfig`, `ModelLoadFailed`, `VadLoadFailed` and `BadFrame` the sidecar
    /// exits.
    Error {
        /// What failed.
        code: SidecarErrorCode,
        /// A fixed English sentence for diagnostics; never the person's words or a path.
        message: String,
    },
}

// ----- encoding -----------------------------------------------------------------------------

/// One frame's tag and payload, before it is read as a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    /// The tag.
    pub tag: u8,
    /// The payload.
    pub payload: Vec<u8>,
}

/// The bytes of a frame: header and payload.
///
/// # Errors
///
/// [`ProtocolError::Oversize`] when the payload is over [`MAX_FRAME_BYTES`].
pub fn frame(tag: u8, payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::Oversize { len: payload.len() });
    }
    // `MAX_FRAME_BYTES` fits a `u32`, checked above.
    let length =
        u32::try_from(payload.len()).map_err(|_| ProtocolError::Oversize { len: payload.len() })?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
    bytes.push(tag);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

/// `i16` samples as little-endian bytes.
#[must_use]
pub fn pcm_to_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Little-endian bytes as `i16` samples.
///
/// # Errors
///
/// [`ProtocolError::OddAudio`] when the length is odd.
pub fn bytes_to_pcm(bytes: &[u8]) -> Result<Vec<i16>, ProtocolError> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err(ProtocolError::OddAudio);
    }
    Ok(pairs.iter().map(|pair| i16::from_le_bytes(*pair)).collect())
}

fn json<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    serde_json::to_vec(value).map_err(|_| ProtocolError::BadPayload)
}

fn from_json<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Result<T, ProtocolError> {
    serde_json::from_slice(payload).map_err(|_| ProtocolError::BadPayload)
}

fn require_empty(frame: &RawFrame) -> Result<(), ProtocolError> {
    if frame.payload.is_empty() {
        Ok(())
    } else {
        Err(ProtocolError::UnexpectedPayload)
    }
}

#[derive(Serialize, Deserialize)]
struct ReadyBody {
    load_ms: u32,
}

#[derive(Serialize, Deserialize)]
struct TextBody {
    text: String,
    start_ms: u32,
    end_ms: u32,
}

#[derive(Serialize, Deserialize)]
struct EmptyBody {
    start_ms: u32,
    end_ms: u32,
}

#[derive(Serialize, Deserialize)]
struct ErrorBody {
    code: SidecarErrorCode,
    message: String,
}

impl ToSidecar {
    /// The message as bytes of one frame.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Oversize`] for an `Audio` message over [`MAX_FRAME_BYTES`] (split it, see
    /// [`policy::MAX_AUDIO_FRAME_SAMPLES`]).
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        match self {
            Self::Start(config) => frame(tag::START, &json(config)?),
            Self::Audio(pcm) => frame(tag::AUDIO, &pcm_to_bytes(pcm)),
            Self::Flush => frame(tag::FLUSH, &[]),
            Self::Quit => frame(tag::QUIT, &[]),
        }
    }

    /// The message a frame holds.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnknownTag`] for a tag of the other direction or none, and the payload's own errors.
    pub fn decode(frame: &RawFrame) -> Result<Self, ProtocolError> {
        match frame.tag {
            tag::START => Ok(Self::Start(from_json(&frame.payload)?)),
            tag::AUDIO => Ok(Self::Audio(bytes_to_pcm(&frame.payload)?)),
            tag::FLUSH => require_empty(frame).map(|()| Self::Flush),
            tag::QUIT => require_empty(frame).map(|()| Self::Quit),
            other => Err(ProtocolError::UnknownTag(other)),
        }
    }
}

impl FromSidecar {
    /// The message as bytes of one frame.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Oversize`] for a segment over [`MAX_FRAME_BYTES`] (the hard cap prevents it).
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        match self {
            Self::Ready { load_ms } => frame(tag::READY, &json(&ReadyBody { load_ms: *load_ms })?),
            Self::SpeechStarted => frame(tag::SPEECH_STARTED, &[]),
            Self::SpeechEnded => frame(tag::SPEECH_ENDED, &[]),
            Self::Text {
                text,
                start_ms,
                end_ms,
            } => frame(
                tag::TEXT,
                &json(&TextBody {
                    text: text.clone(),
                    start_ms: *start_ms,
                    end_ms: *end_ms,
                })?,
            ),
            Self::SegmentAudio {
                pcm,
                start_ms,
                end_ms,
            } => {
                let mut payload = Vec::with_capacity(SEGMENT_HEADER_BYTES + pcm.len() * 2);
                payload.extend_from_slice(&start_ms.to_le_bytes());
                payload.extend_from_slice(&end_ms.to_le_bytes());
                payload.extend_from_slice(&pcm_to_bytes(pcm));
                frame(tag::SEGMENT_AUDIO, &payload)
            }
            Self::Empty { start_ms, end_ms } => frame(
                tag::EMPTY,
                &json(&EmptyBody {
                    start_ms: *start_ms,
                    end_ms: *end_ms,
                })?,
            ),
            Self::Flushed => frame(tag::FLUSHED, &[]),
            Self::Error { code, message } => frame(
                tag::ERROR,
                &json(&ErrorBody {
                    code: *code,
                    message: message.clone(),
                })?,
            ),
        }
    }

    /// The message a frame holds.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnknownTag`] for a tag of the other direction or none, and the payload's own errors.
    pub fn decode(frame: &RawFrame) -> Result<Self, ProtocolError> {
        match frame.tag {
            tag::READY => {
                let body: ReadyBody = from_json(&frame.payload)?;
                Ok(Self::Ready {
                    load_ms: body.load_ms,
                })
            }
            tag::SPEECH_STARTED => require_empty(frame).map(|()| Self::SpeechStarted),
            tag::SPEECH_ENDED => require_empty(frame).map(|()| Self::SpeechEnded),
            tag::TEXT => {
                let body: TextBody = from_json(&frame.payload)?;
                Ok(Self::Text {
                    text: body.text,
                    start_ms: body.start_ms,
                    end_ms: body.end_ms,
                })
            }
            tag::SEGMENT_AUDIO => {
                let header = frame
                    .payload
                    .get(..SEGMENT_HEADER_BYTES)
                    .ok_or(ProtocolError::ShortSegment)?;
                let (start, end) = header.split_at(4);
                let read = |bytes: &[u8]| {
                    <[u8; 4]>::try_from(bytes)
                        .map(u32::from_le_bytes)
                        .map_err(|_| ProtocolError::ShortSegment)
                };
                Ok(Self::SegmentAudio {
                    start_ms: read(start)?,
                    end_ms: read(end)?,
                    pcm: bytes_to_pcm(&frame.payload[SEGMENT_HEADER_BYTES..])?,
                })
            }
            tag::EMPTY => {
                let body: EmptyBody = from_json(&frame.payload)?;
                Ok(Self::Empty {
                    start_ms: body.start_ms,
                    end_ms: body.end_ms,
                })
            }
            tag::FLUSHED => require_empty(frame).map(|()| Self::Flushed),
            tag::ERROR => {
                let body: ErrorBody = from_json(&frame.payload)?;
                Ok(Self::Error {
                    code: body.code,
                    message: body.message,
                })
            }
            other => Err(ProtocolError::UnknownTag(other)),
        }
    }
}

// ----- reading a stream ---------------------------------------------------------------------

/// Cuts a byte stream into frames, however the bytes arrive (a pipe read returns any number of them).
///
/// Feed what a read returned to [`push`](Self::push), then call [`next_frame`](Self::next_frame) until it
/// answers `Ok(None)`. The reader checks the tag and the announced length as soon as the five header bytes
/// are there, so garbage is refused before a huge payload is waited for.
#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
    /// Where the unread bytes of `buffer` begin.
    start: usize,
    failed: Option<ProtocolError>,
}

impl FrameReader {
    /// An empty reader.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes read from the stream.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.failed.is_some() {
            return;
        }
        // Drop what was consumed before it grows: the buffer holds a frame or two at most.
        if self.start > 0 && (self.start == self.buffer.len() || self.start >= 64 * 1024) {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        self.buffer.extend_from_slice(bytes);
    }

    /// How many bytes wait for a frame to complete.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len() - self.start
    }

    /// The next complete frame, `Ok(None)` when more bytes are needed.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnknownTag`] or [`ProtocolError::Oversize`] for a header that cannot be a frame.
    /// The reader has lost its place then: every later call answers the same error.
    pub fn next_frame(&mut self) -> Result<Option<RawFrame>, ProtocolError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let unread = &self.buffer[self.start..];
        let Some(header) = unread.get(..HEADER_BYTES) else {
            return Ok(None);
        };
        let tag = header[0];
        let announced = <[u8; 4]>::try_from(&header[1..HEADER_BYTES])
            .map(u32::from_le_bytes)
            .map_err(|_| ProtocolError::BadPayload)?;
        let length = usize::try_from(announced).unwrap_or(usize::MAX);
        if !tag::is_known(tag) {
            return Err(self.fail(ProtocolError::UnknownTag(tag)));
        }
        if length > MAX_FRAME_BYTES {
            return Err(self.fail(ProtocolError::Oversize { len: length }));
        }
        let Some(payload) = unread.get(HEADER_BYTES..HEADER_BYTES + length) else {
            return Ok(None);
        };
        let frame = RawFrame {
            tag,
            payload: payload.to_vec(),
        };
        self.start += HEADER_BYTES + length;
        Ok(Some(frame))
    }

    fn fail(&mut self, error: ProtocolError) -> ProtocolError {
        self.failed = Some(error.clone());
        self.buffer.clear();
        self.start = 0;
        error
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn config() -> StartConfig {
        StartConfig {
            recognizer: Recognizer::Parakeet {
                paths: ParakeetPaths {
                    encoder: "m/encoder.int8.onnx".into(),
                    decoder: "m/decoder.int8.onnx".into(),
                    joiner: "m/joiner.int8.onnx".into(),
                    tokens: "m/tokens.txt".into(),
                },
            },
            vad_path: "silero_vad.onnx".into(),
            vad: VadParams::default(),
            threads: 2,
            hard_cap_ms: 15_000,
            preroll_ms: 150,
        }
    }

    fn all_to_sidecar() -> Vec<ToSidecar> {
        vec![
            ToSidecar::Start(config()),
            ToSidecar::Start(StartConfig {
                recognizer: Recognizer::Moonshine {
                    paths: MoonshinePaths {
                        preprocessor: "p".into(),
                        encoder: "e".into(),
                        uncached_decoder: "u".into(),
                        cached_decoder: "c".into(),
                        tokens: "t".into(),
                    },
                },
                threads: 1,
                hard_cap_ms: 25_000,
                ..config()
            }),
            ToSidecar::Start(StartConfig {
                recognizer: Recognizer::VadOnly,
                ..config()
            }),
            ToSidecar::Audio(vec![0, 1, -1, i16::MAX, i16::MIN, 300]),
            ToSidecar::Audio(Vec::new()),
            ToSidecar::Flush,
            ToSidecar::Quit,
        ]
    }

    fn all_from_sidecar() -> Vec<FromSidecar> {
        vec![
            FromSidecar::Ready { load_ms: 2_100 },
            FromSidecar::SpeechStarted,
            FromSidecar::SpeechEnded,
            FromSidecar::Text {
                text: "Hello, wörld — « ça va » ?".into(),
                start_ms: 1_000,
                end_ms: 3_500,
            },
            FromSidecar::SegmentAudio {
                pcm: vec![5, -5, 1_000, -1_000],
                start_ms: 10,
                end_ms: 20,
            },
            FromSidecar::SegmentAudio {
                pcm: Vec::new(),
                start_ms: 0,
                end_ms: 0,
            },
            FromSidecar::Empty {
                start_ms: 7,
                end_ms: 9,
            },
            FromSidecar::Flushed,
            FromSidecar::Error {
                code: SidecarErrorCode::ModelLoadFailed,
                message: "a model file could not be loaded".into(),
            },
        ]
    }

    fn read_all(bytes: &[u8]) -> Vec<RawFrame> {
        let mut reader = FrameReader::new();
        reader.push(bytes);
        let mut frames = Vec::new();
        while let Some(frame) = reader.next_frame().unwrap() {
            frames.push(frame);
        }
        assert_eq!(reader.buffered(), 0);
        frames
    }

    #[test]
    fn every_message_to_the_sidecar_round_trips() {
        for message in all_to_sidecar() {
            let bytes = message.encode().unwrap();
            let frames = read_all(&bytes);
            assert_eq!(frames.len(), 1, "{message:?}");
            assert_eq!(ToSidecar::decode(&frames[0]).unwrap(), message);
        }
    }

    #[test]
    fn every_message_from_the_sidecar_round_trips() {
        for message in all_from_sidecar() {
            let bytes = message.encode().unwrap();
            let frames = read_all(&bytes);
            assert_eq!(frames.len(), 1, "{message:?}");
            assert_eq!(FromSidecar::decode(&frames[0]).unwrap(), message);
        }
    }

    #[test]
    fn the_frame_layout_is_tag_length_payload() {
        let bytes = ToSidecar::Audio(vec![1, 2]).encode().unwrap();
        assert_eq!(bytes, [tag::AUDIO, 4, 0, 0, 0, 1, 0, 2, 0]);
        assert_eq!(ToSidecar::Flush.encode().unwrap(), [tag::FLUSH, 0, 0, 0, 0]);
        let segment = FromSidecar::SegmentAudio {
            pcm: vec![-2],
            start_ms: 0x0102,
            end_ms: 0x0304,
        }
        .encode()
        .unwrap();
        assert_eq!(
            segment,
            [
                tag::SEGMENT_AUDIO,
                10,
                0,
                0,
                0,
                2,
                1,
                0,
                0,
                4,
                3,
                0,
                0,
                0xfe,
                0xff
            ]
        );
    }

    #[test]
    fn the_control_json_uses_the_documented_field_names() {
        let ready = FromSidecar::Ready { load_ms: 5 }.encode().unwrap();
        assert_eq!(&ready[HEADER_BYTES..], br#"{"load_ms":5}"#);
        let text = FromSidecar::Text {
            text: "hi".into(),
            start_ms: 1,
            end_ms: 2,
        }
        .encode()
        .unwrap();
        assert_eq!(
            &text[HEADER_BYTES..],
            br#"{"text":"hi","start_ms":1,"end_ms":2}"#
        );
        let error = FromSidecar::Error {
            code: SidecarErrorCode::BadFrame,
            message: "m".into(),
        }
        .encode()
        .unwrap();
        assert_eq!(
            &error[HEADER_BYTES..],
            br#"{"code":"bad_frame","message":"m"}"#
        );
        let start = ToSidecar::Start(StartConfig {
            recognizer: Recognizer::VadOnly,
            ..config()
        })
        .encode()
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&start[HEADER_BYTES..]).unwrap();
        assert_eq!(
            value["recognizer"],
            serde_json::json!({ "kind": "vad_only" })
        );
        assert_eq!(value["hard_cap_ms"], 15_000);
        assert_eq!(value["preroll_ms"], 150);
        assert_eq!(value["vad"]["window_size"], 512);
    }

    #[test]
    fn a_stream_split_anywhere_reads_the_same_frames() {
        let mut bytes = Vec::new();
        let messages = all_to_sidecar();
        for message in &messages {
            bytes.extend_from_slice(&message.encode().unwrap());
        }
        for step in [1_usize, 2, 3, 5, 7, 64, 1_000] {
            let mut reader = FrameReader::new();
            let mut decoded = Vec::new();
            for piece in bytes.chunks(step) {
                reader.push(piece);
                while let Some(frame) = reader.next_frame().unwrap() {
                    decoded.push(ToSidecar::decode(&frame).unwrap());
                }
            }
            assert_eq!(decoded, messages, "step {step}");
            assert_eq!(reader.buffered(), 0);
        }
    }

    #[test]
    fn a_partial_frame_waits() {
        let bytes = ToSidecar::Audio(vec![9; 100]).encode().unwrap();
        let mut reader = FrameReader::new();
        reader.push(&bytes[..4]);
        assert_eq!(reader.next_frame().unwrap(), None);
        reader.push(&bytes[4..bytes.len() - 1]);
        assert_eq!(reader.next_frame().unwrap(), None);
        assert_eq!(reader.buffered(), bytes.len() - 1);
        reader.push(&bytes[bytes.len() - 1..]);
        assert!(reader.next_frame().unwrap().is_some());
    }

    #[test]
    fn an_oversize_announcement_is_refused_before_its_payload_arrives() {
        let mut reader = FrameReader::new();
        let announced = u32::try_from(MAX_FRAME_BYTES + 1).unwrap();
        let mut header = vec![tag::AUDIO];
        header.extend_from_slice(&announced.to_le_bytes());
        reader.push(&header);
        assert_eq!(
            reader.next_frame().unwrap_err(),
            ProtocolError::Oversize {
                len: MAX_FRAME_BYTES + 1
            }
        );
        // The stream is lost for good.
        reader.push(&ToSidecar::Flush.encode().unwrap());
        assert!(matches!(
            reader.next_frame(),
            Err(ProtocolError::Oversize { .. })
        ));
    }

    #[test]
    fn a_frame_of_exactly_the_limit_is_accepted() {
        let payload = vec![0_u8; MAX_FRAME_BYTES];
        let bytes = frame(tag::AUDIO, &payload).unwrap();
        let frames = read_all(&bytes);
        assert_eq!(frames[0].payload.len(), MAX_FRAME_BYTES);
        assert_eq!(
            frame(tag::AUDIO, &vec![0_u8; MAX_FRAME_BYTES + 1]).unwrap_err(),
            ProtocolError::Oversize {
                len: MAX_FRAME_BYTES + 1
            }
        );
        assert!(matches!(
            ToSidecar::Audio(vec![0; MAX_FRAME_BYTES / 2 + 1]).encode(),
            Err(ProtocolError::Oversize { .. })
        ));
    }

    #[test]
    fn garbage_is_refused() {
        let mut reader = FrameReader::new();
        reader.push(&[0x7f, 0, 0, 0, 0]);
        assert_eq!(
            reader.next_frame().unwrap_err(),
            ProtocolError::UnknownTag(0x7f)
        );
        assert_eq!(
            reader.next_frame().unwrap_err(),
            ProtocolError::UnknownTag(0x7f)
        );

        let mut text = FrameReader::new();
        text.push(b"hello, this is not a frame stream");
        assert!(matches!(
            text.next_frame(),
            Err(ProtocolError::UnknownTag(_) | ProtocolError::Oversize { .. })
        ));
        assert_eq!(text.buffered(), 0);
    }

    #[test]
    fn a_tag_of_the_other_direction_does_not_decode() {
        let frames = read_all(&FromSidecar::Flushed.encode().unwrap());
        assert_eq!(
            ToSidecar::decode(&frames[0]).unwrap_err(),
            ProtocolError::UnknownTag(tag::FLUSHED)
        );
        let frames = read_all(&ToSidecar::Quit.encode().unwrap());
        assert_eq!(
            FromSidecar::decode(&frames[0]).unwrap_err(),
            ProtocolError::UnknownTag(tag::QUIT)
        );
    }

    #[test]
    fn bad_payloads_are_refused() {
        let raw = |tag, payload: &[u8]| RawFrame {
            tag,
            payload: payload.to_vec(),
        };
        assert_eq!(
            ToSidecar::decode(&raw(tag::START, b"{not json")).unwrap_err(),
            ProtocolError::BadPayload
        );
        assert_eq!(
            ToSidecar::decode(&raw(tag::START, br#"{"recognizer":{"kind":"nope"}}"#)).unwrap_err(),
            ProtocolError::BadPayload
        );
        assert_eq!(
            ToSidecar::decode(&raw(tag::AUDIO, &[1, 2, 3])).unwrap_err(),
            ProtocolError::OddAudio
        );
        assert_eq!(
            ToSidecar::decode(&raw(tag::FLUSH, &[0])).unwrap_err(),
            ProtocolError::UnexpectedPayload
        );
        assert_eq!(
            FromSidecar::decode(&raw(tag::SEGMENT_AUDIO, &[0; 7])).unwrap_err(),
            ProtocolError::ShortSegment
        );
        assert_eq!(
            FromSidecar::decode(&raw(tag::SEGMENT_AUDIO, &[0; 9])).unwrap_err(),
            ProtocolError::OddAudio
        );
        assert_eq!(
            FromSidecar::decode(&raw(tag::TEXT, br#"{"text":1}"#)).unwrap_err(),
            ProtocolError::BadPayload
        );
        assert_eq!(
            FromSidecar::decode(&raw(tag::SPEECH_ENDED, &[1])).unwrap_err(),
            ProtocolError::UnexpectedPayload
        );
    }

    #[test]
    fn the_longest_segment_still_fits_in_one_frame() {
        let samples =
            usize::try_from((MAX_HARD_CAP_MS + MAX_PREROLL_MS) * SAMPLE_RATE_HZ / 1_000).unwrap();
        assert!(SEGMENT_HEADER_BYTES + samples * 2 <= MAX_FRAME_BYTES);
        let message = FromSidecar::SegmentAudio {
            pcm: vec![0; samples],
            start_ms: 0,
            end_ms: MAX_HARD_CAP_MS,
        };
        assert!(message.encode().is_ok());
        // The shell's audio pushes are cut well under the limit.
        const { assert!(policy::MAX_AUDIO_FRAME_SAMPLES * 2 <= MAX_FRAME_BYTES) };
    }

    #[test]
    fn the_start_configuration_is_checked() {
        assert_eq!(config().validate(), Ok(()));
        let with = |change: &dyn Fn(&mut StartConfig)| {
            let mut c = config();
            change(&mut c);
            c.validate()
        };
        assert_eq!(with(&|c| c.vad_path.clear()), Err(ConfigError::EmptyPath));
        assert_eq!(
            with(&|c| {
                c.recognizer = Recognizer::Moonshine {
                    paths: MoonshinePaths {
                        preprocessor: "p".into(),
                        encoder: String::new(),
                        uncached_decoder: "u".into(),
                        cached_decoder: "c".into(),
                        tokens: "t".into(),
                    },
                };
            }),
            Err(ConfigError::EmptyPath)
        );
        assert_eq!(with(&|c| c.hard_cap_ms = 999), Err(ConfigError::HardCap));
        assert_eq!(
            with(&|c| c.hard_cap_ms = MAX_HARD_CAP_MS + 1),
            Err(ConfigError::HardCap)
        );
        assert_eq!(
            with(&|c| c.preroll_ms = MAX_PREROLL_MS + 1),
            Err(ConfigError::PreRoll)
        );
        assert_eq!(with(&|c| c.threads = 0), Err(ConfigError::Threads));
        assert_eq!(with(&|c| c.threads = 9), Err(ConfigError::Threads));
        assert_eq!(with(&|c| c.vad.threshold = 1.0), Err(ConfigError::Vad));
        assert_eq!(with(&|c| c.vad.window_size = 256), Err(ConfigError::Vad));
        assert_eq!(with(&|c| c.vad.buffer_s = f32::NAN), Err(ConfigError::Vad));
        assert_eq!(with(&|c| c.recognizer = Recognizer::VadOnly), Ok(()));
    }

    #[test]
    fn pcm_bytes_round_trip() {
        let pcm = vec![0, 1, -1, 12_345, i16::MIN, i16::MAX];
        assert_eq!(bytes_to_pcm(&pcm_to_bytes(&pcm)).unwrap(), pcm);
        assert_eq!(bytes_to_pcm(&[1]).unwrap_err(), ProtocolError::OddAudio);
    }
}
