//! The sherpa-onnx side of the sidecar's pipeline: the voice activity detector (Silero) and the recognizer
//! (Parakeet TDT or Moonshine v1, or none for the Ollama engine) behind the [`Detector`] and [`Recognizer`] traits of
//! `kb_speech_native::segmenter`, which holds the logic (pre-roll, hard cap, empty results, flush) and its tests.
//! The native types stay here, in the binary, so the library and the shell never link the DLLs.
//!
//! [`start`] loads the models named by the [`StartConfig`]: a VAD that does not load is `VadLoadFailed`, a
//! recognizer that does is `ModelLoadFailed`, each with a fixed sentence (no path, no model name). A model file
//! that is not there is refused before sherpa-onnx sees it, because the native library may print the path or end
//! the process on a bad file. The detector and the recognizer are moved into the loop's thread and dropped there
//! (they are `Send` in the crate).
//!
//! **Never** log or print the text, the audio or a path.

use std::path::Path;

use kb_speech::protocol::{
    MoonshinePaths, ParakeetPaths, Recognizer as RecognizerConfig, SAMPLE_RATE_HZ,
    SidecarErrorCode, StartConfig, VadParams,
};
use kb_speech_native::{
    Detector, Pipeline, RecognitionFailed, Recognizer, Segment, SegmentPipeline, StartFailure,
};
use sherpa_onnx::{
    OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};

/// The versions of the native libraries, which also proves they load.
pub fn version_line() -> String {
    format!(
        "kb-speech (sherpa-onnx {}, onnxruntime {})",
        sherpa_onnx::version(),
        sherpa_onnx::onnxruntime_version()
    )
}

/// The sample rate as sherpa-onnx takes it.
fn sample_rate() -> i32 {
    i32::try_from(SAMPLE_RATE_HZ).unwrap_or(16_000)
}

/// The silero voice activity detector.
struct SherpaDetector {
    vad: VoiceActivityDetector,
}

impl Detector for SherpaDetector {
    fn accept(&mut self, samples: &[f32]) {
        self.vad.accept_waveform(samples);
    }

    fn detected(&self) -> bool {
        self.vad.detected()
    }

    fn next_segment(&mut self) -> Option<Segment> {
        let segment = self.vad.front()?;
        let found = Segment {
            start: u64::try_from(segment.start()).unwrap_or(0),
            samples: segment.samples().to_vec(),
        };
        drop(segment);
        self.vad.pop();
        Some(found)
    }

    fn flush(&mut self) {
        self.vad.flush();
    }
}

/// An offline recognizer (one decode per piece of audio).
struct SherpaRecognizer {
    recognizer: OfflineRecognizer,
}

impl Recognizer for SherpaRecognizer {
    fn recognise(&mut self, samples: &[f32]) -> Result<String, RecognitionFailed> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(sample_rate(), samples);
        self.recognizer.decode(&stream);
        stream
            .get_result()
            .map(|result| result.text.trim().to_owned())
            .ok_or(RecognitionFailed)
    }
}

fn failure(code: SidecarErrorCode, message: &'static str) -> StartFailure {
    StartFailure { code, message }
}

fn vad_failure() -> StartFailure {
    failure(
        SidecarErrorCode::VadLoadFailed,
        "the voice activity model could not be loaded",
    )
}

fn model_failure() -> StartFailure {
    failure(
        SidecarErrorCode::ModelLoadFailed,
        "a speech model file could not be loaded",
    )
}

fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

fn text(path: &str) -> Option<String> {
    Some(path.to_owned())
}

/// The voice activity detector of `vad_path` with the settings of `vad`.
fn make_vad(vad_path: &str, vad: &VadParams) -> Result<SherpaDetector, StartFailure> {
    if !is_file(vad_path) {
        return Err(vad_failure());
    }
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: text(vad_path),
            threshold: vad.threshold,
            min_silence_duration: vad.min_silence_s,
            min_speech_duration: vad.min_speech_s,
            window_size: i32::try_from(vad.window_size).map_err(|_| vad_failure())?,
            max_speech_duration: vad.max_speech_s,
        },
        sample_rate: sample_rate(),
        num_threads: i32::try_from(vad.threads).map_err(|_| vad_failure())?,
        provider: Some("cpu".into()),
        debug: false,
        ..Default::default()
    };
    VoiceActivityDetector::create(&config, vad.buffer_s)
        .map(|vad| SherpaDetector { vad })
        .ok_or_else(vad_failure)
}

/// The recognizer config of Parakeet TDT (a NeMo transducer, greedy search).
fn parakeet_config(paths: &ParakeetPaths, threads: i32) -> OfflineRecognizerConfig {
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer = OfflineTransducerModelConfig {
        encoder: text(&paths.encoder),
        decoder: text(&paths.decoder),
        joiner: text(&paths.joiner),
    };
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.tokens = text(&paths.tokens);
    config.model_config.num_threads = threads;
    config.model_config.provider = Some("cpu".into());
    config.model_config.debug = false;
    config.decoding_method = Some("greedy_search".into());
    config
}

/// The recognizer config of Moonshine v1 (four model files and the tokens, greedy search).
fn moonshine_config(paths: &MoonshinePaths, threads: i32) -> OfflineRecognizerConfig {
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.moonshine = OfflineMoonshineModelConfig {
        preprocessor: text(&paths.preprocessor),
        encoder: text(&paths.encoder),
        uncached_decoder: text(&paths.uncached_decoder),
        cached_decoder: text(&paths.cached_decoder),
        merged_decoder: None,
    };
    config.model_config.tokens = text(&paths.tokens);
    config.model_config.num_threads = threads;
    config.model_config.provider = Some("cpu".into());
    config.model_config.debug = false;
    config.decoding_method = Some("greedy_search".into());
    config
}

/// The recognizer the configuration names; `None` for `VadOnly`.
fn make_recognizer(config: &StartConfig) -> Result<Option<SherpaRecognizer>, StartFailure> {
    let threads = i32::try_from(config.threads).map_err(|_| model_failure())?;
    let recognizer_config = match &config.recognizer {
        RecognizerConfig::VadOnly => return Ok(None),
        RecognizerConfig::Parakeet { paths } => {
            let files = [&paths.encoder, &paths.decoder, &paths.joiner, &paths.tokens];
            if !files.into_iter().all(|path| is_file(path)) {
                return Err(model_failure());
            }
            parakeet_config(paths, threads)
        }
        RecognizerConfig::Moonshine { paths } => {
            let files = [
                &paths.preprocessor,
                &paths.encoder,
                &paths.uncached_decoder,
                &paths.cached_decoder,
                &paths.tokens,
            ];
            if !files.into_iter().all(|path| is_file(path)) {
                return Err(model_failure());
            }
            moonshine_config(paths, threads)
        }
    };
    OfflineRecognizer::create(&recognizer_config)
        .map(|recognizer| Some(SherpaRecognizer { recognizer }))
        .ok_or_else(model_failure)
}

/// Loads the models and returns the pipeline.
pub fn start(config: &StartConfig) -> Result<Box<dyn Pipeline>, StartFailure> {
    let detector = make_vad(&config.vad_path, &config.vad)?;
    let recognizer = make_recognizer(config)?;
    Ok(Box::new(SegmentPipeline::new(
        Box::new(detector),
        recognizer.map(|recognizer| Box::new(recognizer) as Box<dyn Recognizer>),
        config,
    )))
}
