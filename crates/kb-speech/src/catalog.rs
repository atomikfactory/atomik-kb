//! The speech engines and the models behind them (ADR-0053, `docs/architecture/dictation.md` section 2):
//! what each engine is, which files a downloadable model is made of (pinned URL, size, SHA-256), the
//! languages and the attribution the Settings screen shows, and the threads and the hard segment cap.
//!
//! Every URL pins a Hugging Face commit, so a model never changes under an installed copy, and every file
//! is checked against its SHA-256 after the download (or the import). The numbers were measured and
//! hashed on 2026-10-16.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::policy;
use crate::protocol::{MoonshinePaths, ParakeetPaths, Recognizer, StartConfig, VadParams};

/// Which engine turns speech into text. Chosen in Settings, per device (`dictation_select`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum SpeechEngine {
    /// sherpa-onnx with NVIDIA Parakeet TDT 0.6B v3 (int8): multilingual, punctuated, the most accurate.
    Accurate,
    /// sherpa-onnx with Moonshine v1 tiny (int8): English only, very light on the processor.
    Light,
    /// The AI transcription model of Ollama, through the shell's AI client (`ai_host::transcribe`).
    Ollama,
}

impl SpeechEngine {
    /// Every engine, in the order Settings lists them.
    pub const ALL: [Self; 3] = [Self::Accurate, Self::Light, Self::Ollama];

    /// The name on the wire (`accurate`, `light`, `ollama`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accurate => "accurate",
            Self::Light => "light",
            Self::Ollama => "ollama",
        }
    }

    /// The engine a wire name stands for.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.as_str() == name)
    }

    /// The downloadable model of the engine; `None` for Ollama, whose model lives in Ollama.
    #[must_use]
    pub const fn model(self) -> Option<&'static ModelSpec> {
        match self {
            Self::Accurate => Some(&PARAKEET),
            Self::Light => Some(&MOONSHINE_TINY),
            Self::Ollama => None,
        }
    }

    /// The hard cap on the audio of one recognition, milliseconds.
    #[must_use]
    pub const fn hard_cap_ms(self) -> u32 {
        match self {
            Self::Accurate => policy::HARD_CAP_PARAKEET_MS,
            Self::Light => policy::HARD_CAP_MOONSHINE_MS,
            Self::Ollama => policy::HARD_CAP_OLLAMA_MS,
        }
    }

    /// The first configuration the shell sends the sidecar for this engine: the model's files under
    /// `model_dir` (ignored for Ollama, whose recognizer is [`Recognizer::VadOnly`]) and the voice
    /// activity model at `vad_path`, with the measured settings.
    #[must_use]
    pub fn start_config(self, model_dir: &Path, vad_path: &Path) -> StartConfig {
        let path = |name: &str| model_dir.join(name).to_string_lossy().into_owned();
        let (recognizer, threads) = match self.model() {
            Some(model) => match model.kind {
                RecognizerKind::Parakeet => (
                    Recognizer::Parakeet {
                        paths: ParakeetPaths {
                            encoder: path("encoder.int8.onnx"),
                            decoder: path("decoder.int8.onnx"),
                            joiner: path("joiner.int8.onnx"),
                            tokens: path("tokens.txt"),
                        },
                    },
                    model.threads,
                ),
                RecognizerKind::Moonshine => (
                    Recognizer::Moonshine {
                        paths: MoonshinePaths {
                            preprocessor: path("preprocess.onnx"),
                            encoder: path("encode.int8.onnx"),
                            uncached_decoder: path("uncached_decode.int8.onnx"),
                            cached_decoder: path("cached_decode.int8.onnx"),
                            tokens: path("tokens.txt"),
                        },
                    },
                    model.threads,
                ),
            },
            None => (Recognizer::VadOnly, 1),
        };
        StartConfig {
            recognizer,
            vad_path: vad_path.to_string_lossy().into_owned(),
            vad: VadParams::default(),
            threads,
            hard_cap_ms: self.hard_cap_ms(),
            preroll_ms: policy::PREROLL_MS,
        }
    }
}

/// The family of a downloadable model, which decides how the sidecar builds its recognizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognizerKind {
    /// NeMo transducer (encoder, decoder, joiner).
    Parakeet,
    /// Moonshine v1 (preprocessor, encoder, two decoders).
    Moonshine,
}

/// One file of a model, or the voice activity model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    /// The name in the model's folder.
    pub name: &'static str,
    /// Where it is downloaded from: `https://huggingface.co/<repo>/resolve/<40-hex commit>/<name>`.
    pub url: &'static str,
    /// Its size in bytes.
    pub bytes: u64,
    /// Its SHA-256, lower-case hex.
    pub sha256: &'static str,
}

/// A downloadable speech model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    /// The folder under `<app data>/speech/`.
    pub id: &'static str,
    /// The engine that uses it.
    pub engine: SpeechEngine,
    /// Its name for the person.
    pub name: &'static str,
    /// How the sidecar builds a recognizer from it.
    pub kind: RecognizerKind,
    /// The Hugging Face repository the files come from.
    pub repo: &'static str,
    /// The pinned commit of that repository.
    pub commit: &'static str,
    /// The files, all of them required.
    pub files: &'static [ModelFile],
    /// The languages it understands, BCP-47 codes; the UI shows their names.
    pub languages: &'static [&'static str],
    /// The attribution Settings shows next to the model.
    pub attribution: &'static str,
    /// The licence as an SPDX identifier.
    pub licence: &'static str,
    /// onnxruntime threads of the recognizer.
    pub threads: u32,
    /// The hard cap on the audio of one recognition, milliseconds.
    pub hard_cap_ms: u32,
}

impl ModelSpec {
    /// The bytes of every file together: what a download transfers.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }

    /// The file called `name`.
    #[must_use]
    pub fn file(&self, name: &str) -> Option<&'static ModelFile> {
        self.files.iter().find(|file| file.name == name)
    }

    /// The host the files come from (what the consent dialog names).
    pub const HOST: &'static str = "huggingface.co";
}

/// `https://huggingface.co/<repo>/resolve/<commit>/<name>` as a constant.
macro_rules! hf_file {
    ($repo:literal, $commit:literal, $name:literal, $bytes:literal, $sha256:literal) => {
        ModelFile {
            name: $name,
            url: concat!(
                "https://huggingface.co/",
                $repo,
                "/resolve/",
                $commit,
                "/",
                $name
            ),
            bytes: $bytes,
            sha256: $sha256,
        }
    };
}

macro_rules! parakeet_file {
    ($name:literal, $bytes:literal, $sha256:literal) => {
        hf_file!(
            "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
            "2bda32ec70b097a55adaa07d9a7173915b43cc78",
            $name,
            $bytes,
            $sha256
        )
    };
}

macro_rules! moonshine_file {
    ($name:literal, $bytes:literal, $sha256:literal) => {
        hf_file!(
            "csukuangfj/sherpa-onnx-moonshine-tiny-en-int8",
            "bf2b762c076d8ea61e2af0b3851c9564fb77552e",
            $name,
            $bytes,
            $sha256
        )
    };
}

/// NVIDIA Parakeet TDT 0.6B v3, int8, for sherpa-onnx (the Accurate engine). About 670 MB.
pub const PARAKEET: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v3-int8",
    engine: SpeechEngine::Accurate,
    name: "NVIDIA Parakeet TDT 0.6B v3 (int8)",
    kind: RecognizerKind::Parakeet,
    repo: "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    commit: "2bda32ec70b097a55adaa07d9a7173915b43cc78",
    files: &[
        parakeet_file!(
            "encoder.int8.onnx",
            652_184_281,
            "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247"
        ),
        parakeet_file!(
            "decoder.int8.onnx",
            11_845_275,
            "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e"
        ),
        parakeet_file!(
            "joiner.int8.onnx",
            6_355_277,
            "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3"
        ),
        parakeet_file!(
            "tokens.txt",
            93_939,
            "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"
        ),
    ],
    // The 25 European languages of Parakeet TDT 0.6B v3.
    languages: &[
        "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt",
        "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
    ],
    attribution: "NVIDIA Parakeet TDT 0.6B v3, licensed CC-BY-4.0 (https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3); int8 conversion for sherpa-onnx by k2-fsa.",
    licence: "CC-BY-4.0",
    threads: 2,
    hard_cap_ms: policy::HARD_CAP_PARAKEET_MS,
};

/// Moonshine v1 tiny (English), int8, for sherpa-onnx (the Light engine). About 124 MB.
pub const MOONSHINE_TINY: ModelSpec = ModelSpec {
    id: "moonshine-tiny-en-int8",
    engine: SpeechEngine::Light,
    name: "Moonshine tiny, English (int8)",
    kind: RecognizerKind::Moonshine,
    repo: "csukuangfj/sherpa-onnx-moonshine-tiny-en-int8",
    commit: "bf2b762c076d8ea61e2af0b3851c9564fb77552e",
    files: &[
        moonshine_file!(
            "preprocess.onnx",
            6_800_738,
            "f33addce61a143460fe753b5ee5b7db255e5140b5b779c065b94f6c83ff0bf4e"
        ),
        moonshine_file!(
            "encode.int8.onnx",
            18_249_187,
            "8774dfba578de027ec6595c2c654a0836434489bc963a0db124a7f181f571acb"
        ),
        moonshine_file!(
            "uncached_decode.int8.onnx",
            53_216_096,
            "216737000dd5881a17aa043f6bbd286add33e4c3b0ae257153e2ec15438bdc41"
        ),
        moonshine_file!(
            "cached_decode.int8.onnx",
            45_264_830,
            "2aff28bba6a03d8dcf5c9feac45462629bae37317442299f28115ad09da773f6"
        ),
        moonshine_file!(
            "tokens.txt",
            436_688,
            "1165c2aeb9f72f457a83be2d459a09054f27490acd9b41bd43794dfd25e296ea"
        ),
    ],
    languages: &["en"],
    attribution: "Moonshine tiny by Useful Sensors, MIT licence (https://github.com/usefulsensors/moonshine); int8 conversion for sherpa-onnx by k2-fsa.",
    licence: "MIT",
    threads: 1,
    hard_cap_ms: policy::HARD_CAP_MOONSHINE_MS,
};

/// Every downloadable model.
pub const MODELS: [&ModelSpec; 2] = [&PARAKEET, &MOONSHINE_TINY];

/// The Silero voice activity model (MIT). Bundled in the installer and never downloaded by the app; the
/// development script fetches it. The release tag is mutable, so the file is pinned by its hash only.
pub const VAD_FILE: ModelFile = ModelFile {
    name: "silero_vad.onnx",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
    bytes: 643_854,
    sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
};

/// The model a folder name stands for.
#[must_use]
pub fn model_by_id(id: &str) -> Option<&'static ModelSpec> {
    MODELS.into_iter().find(|model| model.id == id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn is_hex(text: &str, length: usize) -> bool {
        text.len() == length
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn every_url_pins_a_forty_hex_commit_of_its_repository() {
        for model in MODELS {
            assert!(is_hex(model.commit, 40), "{}: commit", model.id);
            for file in model.files {
                assert_eq!(
                    file.url,
                    format!(
                        "https://huggingface.co/{}/resolve/{}/{}",
                        model.repo, model.commit, file.name
                    ),
                    "{}/{}",
                    model.id,
                    file.name
                );
            }
        }
    }

    #[test]
    fn sizes_hashes_and_names_look_sane() {
        for model in MODELS {
            assert!(!model.files.is_empty());
            let mut names = HashSet::new();
            for file in model.files {
                assert!(names.insert(file.name), "{}: {} twice", model.id, file.name);
                assert!(
                    is_hex(file.sha256, 64),
                    "{}/{}: sha256",
                    model.id,
                    file.name
                );
                assert!(
                    file.bytes > 0 && file.bytes < 1 << 32,
                    "{}/{}",
                    model.id,
                    file.name
                );
                assert!(
                    !file.name.contains(['/', '\\', ':'])
                        && !file.name.starts_with('.')
                        && !file.name.is_empty(),
                    "{}: {} must be a plain file name",
                    model.id,
                    file.name
                );
            }
            let hashes: HashSet<_> = model.files.iter().map(|file| file.sha256).collect();
            assert_eq!(
                hashes.len(),
                model.files.len(),
                "{}: two files with one hash",
                model.id
            );
        }
        assert!(is_hex(VAD_FILE.sha256, 64));
        assert!(
            VAD_FILE
                .url
                .starts_with("https://github.com/k2-fsa/sherpa-onnx/releases/download/")
        );
        assert_eq!(VAD_FILE.bytes, 643_854);
    }

    #[test]
    fn the_pinned_totals_match_the_measured_downloads() {
        assert_eq!(
            PARAKEET.total_bytes(),
            652_184_281 + 11_845_275 + 6_355_277 + 93_939
        );
        assert_eq!(PARAKEET.total_bytes(), 670_478_772);
        assert_eq!(MOONSHINE_TINY.total_bytes(), 123_967_539);
        assert_eq!(PARAKEET.files.len(), 4);
        assert_eq!(MOONSHINE_TINY.files.len(), 5);
    }

    #[test]
    fn the_model_ids_are_distinct_plain_folder_names() {
        let ids: HashSet<_> = MODELS.iter().map(|model| model.id).collect();
        assert_eq!(ids.len(), MODELS.len());
        for model in MODELS {
            assert!(
                model
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'),
                "{}",
                model.id
            );
            assert_eq!(model_by_id(model.id), Some(model));
        }
        assert_eq!(model_by_id("../x"), None);
    }

    #[test]
    fn the_engines_and_their_models_agree() {
        assert_eq!(SpeechEngine::Accurate.model(), Some(&PARAKEET));
        assert_eq!(SpeechEngine::Light.model(), Some(&MOONSHINE_TINY));
        assert_eq!(SpeechEngine::Ollama.model(), None);
        for model in MODELS {
            assert_eq!(model.engine.model(), Some(model));
            assert_eq!(model.hard_cap_ms, model.engine.hard_cap_ms());
        }
        assert_eq!(SpeechEngine::Accurate.hard_cap_ms(), 15_000);
        assert_eq!(SpeechEngine::Light.hard_cap_ms(), 25_000);
        assert_eq!(SpeechEngine::Ollama.hard_cap_ms(), 15_000);
        for engine in SpeechEngine::ALL {
            assert_eq!(SpeechEngine::parse(engine.as_str()), Some(engine));
        }
        assert_eq!(SpeechEngine::parse("Accurate"), None);
        assert_eq!(SpeechEngine::parse(""), None);
    }

    #[test]
    fn languages_and_attribution_are_there_for_the_settings_screen() {
        assert_eq!(PARAKEET.languages.len(), 25);
        for code in ["en", "fr", "es"] {
            assert!(PARAKEET.languages.contains(&code));
        }
        assert_eq!(MOONSHINE_TINY.languages, ["en"]);
        let unique: HashSet<_> = PARAKEET.languages.iter().collect();
        assert_eq!(unique.len(), PARAKEET.languages.len());
        assert!(PARAKEET.attribution.contains("CC-BY-4.0"));
        assert!(MOONSHINE_TINY.attribution.contains("MIT"));
        assert_eq!(PARAKEET.licence, "CC-BY-4.0");
        assert_eq!(MOONSHINE_TINY.licence, "MIT");
        assert_eq!(ModelSpec::HOST, "huggingface.co");
    }

    #[test]
    fn the_start_configuration_names_the_models_files_and_is_valid() {
        let dir = PathBuf::from("models").join("m");
        let vad = PathBuf::from("res").join("silero_vad.onnx");
        for engine in SpeechEngine::ALL {
            let config = engine.start_config(&dir, &vad);
            assert_eq!(config.validate(), Ok(()), "{engine:?}");
            assert_eq!(config.hard_cap_ms, engine.hard_cap_ms());
            assert_eq!(config.preroll_ms, 150);
            assert!(config.vad_path.ends_with("silero_vad.onnx"));
        }
        let accurate = SpeechEngine::Accurate.start_config(&dir, &vad);
        assert_eq!(accurate.threads, 2);
        let Recognizer::Parakeet { paths } = &accurate.recognizer else {
            unreachable!("Accurate is Parakeet");
        };
        assert!(paths.encoder.ends_with("encoder.int8.onnx"));
        assert!(paths.tokens.ends_with("tokens.txt"));
        let light = SpeechEngine::Light.start_config(&dir, &vad);
        assert_eq!(light.threads, 1);
        let Recognizer::Moonshine { paths } = &light.recognizer else {
            unreachable!("Light is Moonshine");
        };
        assert!(paths.cached_decoder.ends_with("cached_decode.int8.onnx"));
        assert_eq!(
            SpeechEngine::Ollama.start_config(&dir, &vad).recognizer,
            Recognizer::VadOnly
        );
        // Every file the configuration names is a file of the catalog.
        for (engine, model) in [
            (SpeechEngine::Accurate, &PARAKEET),
            (SpeechEngine::Light, &MOONSHINE_TINY),
        ] {
            let config = engine.start_config(&dir, &vad);
            let named: Vec<String> = match config.recognizer {
                Recognizer::Parakeet { paths } => {
                    vec![paths.encoder, paths.decoder, paths.joiner, paths.tokens]
                }
                Recognizer::Moonshine { paths } => vec![
                    paths.preprocessor,
                    paths.encoder,
                    paths.uncached_decoder,
                    paths.cached_decoder,
                    paths.tokens,
                ],
                Recognizer::VadOnly => Vec::new(),
            };
            assert_eq!(named.len(), model.files.len());
            for path in named {
                let name = Path::new(&path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                assert!(model.file(&name).is_some(), "{engine:?}: {name}");
            }
        }
    }
}
