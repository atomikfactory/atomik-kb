//! The frame loop of the `kb-speech` sidecar (ADR-0053, `docs/architecture/dictation.md` section 3).
//!
//! The sidecar is a process the desktop shell starts for one dictation session. It reads
//! [`kb_speech::protocol::ToSidecar`] frames on stdin and writes [`kb_speech::protocol::FromSidecar`] frames
//! on stdout; diagnostics go to stderr and never hold what the person said. This library is the part with no
//! sherpa-onnx in it: [`run`] reads and decodes the frames on a thread, enforces the order of the messages and
//! the idle exit, and drives a [`Pipeline`]; [`SegmentPipeline`] is that pipeline's logic (pre-roll, hard cap,
//! empty results, flush), written against the [`Detector`] and [`Recognizer`] traits that the binary
//! (`src/sherpa_pipeline.rs`) implements over sherpa-onnx. Keeping both apart lets their tests run without the
//! native DLLs.

#![forbid(unsafe_code)]

pub mod host_loop;
pub mod segmenter;

pub use host_loop::{Exit, Pipeline, Sink, SinkClosed, StartFailure, run};
pub use segmenter::{
    Detector, RecognitionFailed, Recognizer, Segment, SegmentPipeline, f32_to_pcm, pcm_to_f32,
};
