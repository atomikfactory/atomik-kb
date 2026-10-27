//! What the desktop shell and the webview exchange about dictation (ADR-0053,
//! `docs/architecture/dictation.md` section 5): the events of a session and of an installation, the
//! status of the engines, and the error codes. Exported to `packages/core-client/src/generated/` with
//! `cargo test -p kb-speech --features ts`.
//!
//! The wire form is camelCase, events are tagged `{ "type": ... }` like the AI stream's. A command that
//! cannot start answers `ApiError` `InvalidInput` with `details.reason` set to [`DictationErrorCode::as_str`];
//! what goes wrong later is an event with the same code.

use serde::{Deserialize, Serialize};

use crate::catalog::SpeechEngine;

/// What a running session is doing, for the status line of the dictation bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DictationState {
    /// The engine is starting and loading its model; audio pushed meanwhile is kept.
    Loading,
    /// Listening: waiting for speech or hearing it.
    Listening,
    /// The person paused and the phrase is being recognised.
    Recognising,
}

/// Why dictation or an installation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DictationErrorCode {
    /// The engine's model is not installed (download it or import it first).
    ModelMissing,
    /// The sidecar or the voice activity model is not where the app expects it.
    EngineMissing,
    /// The engine reported an error (a model that does not load, a recognition that failed).
    EngineFailed,
    /// The sidecar process ended without being asked to.
    EngineCrashed,
    /// The Ollama engine cannot be used: AI is off, no transcription model, no consent, or Ollama does not
    /// answer.
    OllamaUnavailable,
    /// Another dictation session or another installation is running.
    Busy,
    /// The download failed (no connection, an unexpected answer, a redirect off Hugging Face).
    DownloadFailed,
    /// A file's SHA-256 or size is not the pinned one; nothing was kept.
    HashMismatch,
    /// The person cancelled.
    Cancelled,
    /// The person declined the download in the native dialog.
    ConsentDeclined,
    /// The folder chosen for "Use model files" is missing a file or holds a wrong one.
    ImportInvalid,
    /// The model store could not be written (disk full, no permission).
    StoreFailed,
}

impl DictationErrorCode {
    /// The name on the wire, also the `details.reason` of an `ApiError`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ModelMissing => "modelMissing",
            Self::EngineMissing => "engineMissing",
            Self::EngineFailed => "engineFailed",
            Self::EngineCrashed => "engineCrashed",
            Self::OllamaUnavailable => "ollamaUnavailable",
            Self::Busy => "busy",
            Self::DownloadFailed => "downloadFailed",
            Self::HashMismatch => "hashMismatch",
            Self::Cancelled => "cancelled",
            Self::ConsentDeclined => "consentDeclined",
            Self::ImportInvalid => "importInvalid",
            Self::StoreFailed => "storeFailed",
        }
    }
}

/// What a dictation session sends over its channel. `Ended` is always the last event; an `Error` is
/// followed by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DictationEvent {
    /// The session moved to another state.
    State {
        /// The new state.
        state: DictationState,
    },
    /// A recognised phrase, as the engine wrote it; the editor joins it with `joinPhrase`.
    Phrase {
        /// The text; never empty.
        text: String,
    },
    /// The session failed.
    Error {
        /// What went wrong.
        code: DictationErrorCode,
    },
    /// The session is over; nothing follows.
    Ended,
}

/// Why an engine cannot be used right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum UnavailableReason {
    /// The sidecar or the voice activity model is not there (a build or packaging without them).
    EngineMissing,
    /// Ollama: AI features are off.
    AiOff,
    /// Ollama: no transcription model is chosen in the AI settings.
    NoTranscriptionModel,
    /// Ollama: the endpoint is on another computer and the person has not agreed to send text there.
    ConsentRequired,
}

/// One engine as Settings and the first-use dialog show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    /// Which engine.
    pub engine: SpeechEngine,
    /// Nothing more to download: the model files are in the store (always true for Ollama).
    pub installed: bool,
    /// What a download transfers, bytes (0 for Ollama).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub download_bytes: u64,
    /// BCP-47 codes of the languages it understands (empty for Ollama: it depends on the model).
    pub languages: Vec<String>,
    /// The attribution to show (empty for Ollama).
    pub attribution: String,
    /// The engine can run on this machine now (or, for a downloadable one, as soon as it is installed).
    pub available: bool,
    /// Why not, when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub unavailable_reason: Option<UnavailableReason>,
}

/// Answer of `dictation_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DictationStatus {
    /// The engine chosen on this device, `null` before the first choice.
    pub selected: Option<SpeechEngine>,
    /// Every engine, in the order of [`SpeechEngine::ALL`].
    pub engines: Vec<EngineStatus>,
}

/// What an installation sends over its channel. `Done` and `Error` are final.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum InstallEvent {
    /// Bytes downloaded so far of the whole model, at most ten times a second.
    Progress {
        /// Bytes done.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        done: u64,
        /// Bytes in all.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        total: u64,
    },
    /// Every file is downloaded and is being checked against its SHA-256.
    Verifying,
    /// The model is installed.
    Done,
    /// The installation failed; nothing was kept.
    Error {
        /// What went wrong.
        code: DictationErrorCode,
    },
}

/// What the shell knows when it builds a [`DictationStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// The sidecar executable and the voice activity model were found.
    pub engine_present: bool,
    /// The Accurate model is in the store, complete.
    pub accurate_installed: bool,
    /// The Light model is in the store, complete.
    pub light_installed: bool,
    /// `AiSettings.enabled`.
    pub ai_enabled: bool,
    /// `AiSettings.transcriptionModel` is set.
    pub transcription_model_set: bool,
    /// The AI endpoint is this computer or the person agreed to send text to it.
    pub endpoint_allowed: bool,
}

/// The status of every engine from what the shell found out, and the stored choice.
#[must_use]
pub fn build_status(selected: Option<SpeechEngine>, facts: &Facts) -> DictationStatus {
    let engines = SpeechEngine::ALL
        .into_iter()
        .map(|engine| {
            let (installed, reason) = match engine {
                SpeechEngine::Accurate => (facts.accurate_installed, None),
                SpeechEngine::Light => (facts.light_installed, None),
                SpeechEngine::Ollama => (
                    true,
                    if !facts.ai_enabled {
                        Some(UnavailableReason::AiOff)
                    } else if !facts.transcription_model_set {
                        Some(UnavailableReason::NoTranscriptionModel)
                    } else if !facts.endpoint_allowed {
                        Some(UnavailableReason::ConsentRequired)
                    } else {
                        None
                    },
                ),
            };
            // Every engine needs the sidecar and the voice activity model; that comes first.
            let unavailable_reason = if facts.engine_present {
                reason
            } else {
                Some(UnavailableReason::EngineMissing)
            };
            let model = engine.model();
            EngineStatus {
                engine,
                installed,
                download_bytes: model.map_or(0, crate::catalog::ModelSpec::total_bytes),
                languages: model
                    .map(|model| {
                        model
                            .languages
                            .iter()
                            .map(|code| (*code).to_owned())
                            .collect()
                    })
                    .unwrap_or_default(),
                attribution: model
                    .map(|model| model.attribution.to_owned())
                    .unwrap_or_default(),
                available: unavailable_reason.is_none(),
                unavailable_reason,
            }
        })
        .collect();
    DictationStatus { selected, engines }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts() -> Facts {
        Facts {
            engine_present: true,
            accurate_installed: false,
            light_installed: false,
            ai_enabled: false,
            transcription_model_set: false,
            endpoint_allowed: true,
        }
    }

    #[test]
    fn events_have_the_wire_form_the_webview_reads() {
        let value = |event: &DictationEvent| serde_json::to_value(event).unwrap();
        assert_eq!(
            value(&DictationEvent::State {
                state: DictationState::Recognising
            }),
            json!({ "type": "state", "state": "recognising" })
        );
        assert_eq!(
            value(&DictationEvent::Phrase { text: "Hi.".into() }),
            json!({ "type": "phrase", "text": "Hi." })
        );
        assert_eq!(
            value(&DictationEvent::Error {
                code: DictationErrorCode::ModelMissing
            }),
            json!({ "type": "error", "code": "modelMissing" })
        );
        assert_eq!(value(&DictationEvent::Ended), json!({ "type": "ended" }));
        let install = |event: &InstallEvent| serde_json::to_value(event).unwrap();
        assert_eq!(
            install(&InstallEvent::Progress { done: 5, total: 10 }),
            json!({ "type": "progress", "done": 5, "total": 10 })
        );
        assert_eq!(
            install(&InstallEvent::Verifying),
            json!({ "type": "verifying" })
        );
        assert_eq!(install(&InstallEvent::Done), json!({ "type": "done" }));
        assert_eq!(
            install(&InstallEvent::Error {
                code: DictationErrorCode::HashMismatch
            }),
            json!({ "type": "error", "code": "hashMismatch" })
        );
        assert_eq!(
            serde_json::to_value(DictationState::Loading).unwrap(),
            json!("loading")
        );
        assert_eq!(
            serde_json::to_value(SpeechEngine::Accurate).unwrap(),
            json!("accurate")
        );
    }

    #[test]
    fn as_str_is_the_serde_name_of_every_code() {
        for code in [
            DictationErrorCode::ModelMissing,
            DictationErrorCode::EngineMissing,
            DictationErrorCode::EngineFailed,
            DictationErrorCode::EngineCrashed,
            DictationErrorCode::OllamaUnavailable,
            DictationErrorCode::Busy,
            DictationErrorCode::DownloadFailed,
            DictationErrorCode::HashMismatch,
            DictationErrorCode::Cancelled,
            DictationErrorCode::ConsentDeclined,
            DictationErrorCode::ImportInvalid,
            DictationErrorCode::StoreFailed,
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
            let back: DictationErrorCode = serde_json::from_value(json!(code.as_str())).unwrap();
            assert_eq!(back, code);
        }
    }

    #[test]
    fn events_round_trip() {
        for event in [
            DictationEvent::State {
                state: DictationState::Listening,
            },
            DictationEvent::Phrase { text: "é".into() },
            DictationEvent::Error {
                code: DictationErrorCode::EngineCrashed,
            },
            DictationEvent::Ended,
        ] {
            let back: DictationEvent =
                serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn the_status_lists_every_engine_in_order() {
        let status = build_status(None, &facts());
        assert_eq!(status.selected, None);
        let order: Vec<_> = status.engines.iter().map(|engine| engine.engine).collect();
        assert_eq!(order, SpeechEngine::ALL);
        let accurate = &status.engines[0];
        assert!(!accurate.installed);
        assert!(accurate.available);
        assert_eq!(accurate.unavailable_reason, None);
        assert_eq!(accurate.download_bytes, 670_478_772);
        assert_eq!(accurate.languages.len(), 25);
        assert!(accurate.attribution.contains("CC-BY-4.0"));
        let light = &status.engines[1];
        assert_eq!(light.download_bytes, 123_967_539);
        assert_eq!(light.languages, ["en"]);
        let ollama = &status.engines[2];
        assert!(ollama.installed);
        assert_eq!(ollama.download_bytes, 0);
        assert!(ollama.languages.is_empty());
        assert!(ollama.attribution.is_empty());
    }

    #[test]
    fn installed_follows_the_store() {
        let status = build_status(
            Some(SpeechEngine::Light),
            &Facts {
                accurate_installed: true,
                ..facts()
            },
        );
        assert_eq!(status.selected, Some(SpeechEngine::Light));
        assert!(status.engines[0].installed);
        assert!(!status.engines[1].installed);
    }

    #[test]
    fn ollama_needs_ai_a_model_and_consent_in_that_order() {
        let reason = |facts: &Facts| build_status(None, facts).engines[2].unavailable_reason;
        assert_eq!(reason(&facts()), Some(UnavailableReason::AiOff));
        assert_eq!(
            reason(&Facts {
                ai_enabled: true,
                ..facts()
            }),
            Some(UnavailableReason::NoTranscriptionModel)
        );
        assert_eq!(
            reason(&Facts {
                ai_enabled: true,
                transcription_model_set: true,
                endpoint_allowed: false,
                ..facts()
            }),
            Some(UnavailableReason::ConsentRequired)
        );
        let ready = Facts {
            ai_enabled: true,
            transcription_model_set: true,
            ..facts()
        };
        let status = build_status(None, &ready);
        assert!(status.engines[2].available);
        assert_eq!(status.engines[2].unavailable_reason, None);
    }

    #[test]
    fn without_the_sidecar_no_engine_is_available() {
        let status = build_status(
            None,
            &Facts {
                engine_present: false,
                ai_enabled: true,
                transcription_model_set: true,
                ..facts()
            },
        );
        for engine in &status.engines {
            assert!(!engine.available);
            assert_eq!(
                engine.unavailable_reason,
                Some(UnavailableReason::EngineMissing)
            );
        }
    }

    #[test]
    fn the_status_json_leaves_out_an_absent_reason() {
        let status = build_status(None, &facts());
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(value["selected"], json!(null));
        assert!(value["engines"][0].get("unavailableReason").is_none());
        assert_eq!(value["engines"][0]["downloadBytes"], json!(670_478_772));
        assert_eq!(value["engines"][2]["unavailableReason"], json!("aiOff"));
        assert_eq!(value["engines"][0]["engine"], json!("accurate"));
    }
}
