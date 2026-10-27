//! Local dictation rules (ADR-0053, `docs/architecture/dictation.md`).
//!
//! Pure functions and types: the catalog of the speech models and the engines ([`catalog`]), the frame
//! protocol between the desktop shell and the `kb-speech` sidecar process ([`protocol`]), the voice-activity
//! and segment policy and the rule that joins a recognised phrase to the text before the cursor
//! ([`policy`]), the WAV a segment becomes for Ollama ([`wav`]) and the types the shell and the webview
//! exchange ([`api`], exported to TypeScript with the `ts` feature). No I/O, no clock, no sherpa-onnx;
//! compiles for `wasm32-unknown-unknown`. The sidecar (`adapters/kb-speech-native`) runs the recognizer, the
//! shell (`apps/desktop/src-tauri/src/speech_host.rs`) owns the process, the downloads and the consent.

#![forbid(unsafe_code)]

pub mod api;
pub mod catalog;
pub mod policy;
pub mod protocol;
pub mod wav;

pub use api::{
    DictationErrorCode, DictationEvent, DictationState, DictationStatus, EngineStatus,
    InstallEvent, UnavailableReason,
};
pub use catalog::{ModelFile, ModelSpec, RecognizerKind, SpeechEngine, VAD_FILE};
