//! Tests of the dictation host: the pure helpers, the session and installation state machines against an
//! in-process fake sidecar and a stub download server, and the Ollama path against the AI stub.

// The tests that run by hand (`--ignored --nocapture`) print what they measured.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use super::*;
use kb_speech::catalog::{MOONSHINE_TINY, PARAKEET};
use tauri::http::HeaderValue;

mod harness;
mod installs;
mod ollama;
mod sessions;

pub(super) fn reason(error: &ApiError) -> Option<&str> {
    error.details.as_ref()?.get("reason")?.as_str()
}

pub(super) fn field(error: &ApiError) -> Option<&str> {
    error.details.as_ref()?.get("field")?.as_str()
}

/// A model folder with every file of the spec at its pinned size (sparse content: zeros).
pub(super) fn install_fake(dir: &Path, spec: &ModelSpec) {
    std::fs::create_dir_all(dir).unwrap();
    for file in spec.files {
        let handle = std::fs::File::create(dir.join(file.name)).unwrap();
        handle.set_len(file.bytes).unwrap();
    }
    write_installed_marker(dir, spec).unwrap();
}

#[test]
fn a_session_id_is_one_to_sixty_four_plain_characters() {
    assert!(validate_session_id("a").is_ok());
    assert!(validate_session_id("dict-1_X").is_ok());
    assert!(validate_session_id(&"a".repeat(64)).is_ok());
    for bad in ["", "a b", "a/b", "é", "a.b"] {
        let error = validate_session_id(bad).unwrap_err();
        assert_eq!(field(&error), Some("sessionId"), "{bad:?}");
    }
    assert!(validate_session_id(&"a".repeat(65)).is_err());
}

#[test]
fn the_session_header_is_read_and_checked() {
    let mut headers = HeaderMap::new();
    assert_eq!(
        field(&session_from_header(&headers).unwrap_err()),
        Some("sessionId")
    );
    headers.insert(SESSION_HEADER, HeaderValue::from_static("session-1"));
    assert_eq!(session_from_header(&headers).unwrap(), "session-1");
    headers.insert(SESSION_HEADER, HeaderValue::from_static("bad id"));
    assert!(session_from_header(&headers).is_err());
}

#[test]
fn an_audio_body_is_whole_samples_and_at_most_two_seconds() {
    assert_eq!(pcm_from_body(&[1, 0, 0xff, 0xff]).unwrap(), [1, -1]);
    assert_eq!(field(&pcm_from_body(&[]).unwrap_err()), Some("audio"));
    assert_eq!(
        field(&pcm_from_body(&[1, 2, 3]).unwrap_err()),
        Some("audio")
    );
    assert!(pcm_from_body(&vec![0; MAX_PUSH_BYTES]).is_ok());
    assert!(pcm_from_body(&vec![0; MAX_PUSH_BYTES + 2]).is_err());
}

#[test]
fn a_dictation_error_names_its_reason_and_field() {
    let error = dictation_error("engine", DictationErrorCode::ConsentDeclined, "declined");
    assert_eq!(reason(&error), Some("consentDeclined"));
    assert_eq!(field(&error), Some("engine"));
    assert_eq!(error.code, kb_types::ErrorCode::InvalidInput);
}

#[test]
fn the_selection_is_a_file_that_survives_and_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("speech");
    assert_eq!(read_selection(&root), None);
    write_selection(&root, SpeechEngine::Light).unwrap();
    assert_eq!(read_selection(&root), Some(SpeechEngine::Light));
    write_selection(&root, SpeechEngine::Ollama).unwrap();
    assert_eq!(read_selection(&root), Some(SpeechEngine::Ollama));
    assert_eq!(
        std::fs::read_to_string(root.join(SELECTION_FILE)).unwrap(),
        r#"{"engine":"ollama"}"#
    );
    assert!(!root.join("selection.tmp").exists());
    // A damaged file is no choice.
    std::fs::write(root.join(SELECTION_FILE), b"{nope").unwrap();
    assert_eq!(read_selection(&root), None);
    std::fs::write(root.join(SELECTION_FILE), br#"{"engine":"cloud"}"#).unwrap();
    assert_eq!(read_selection(&root), None);
}

#[test]
fn a_selection_that_cannot_be_written_is_a_store_error() {
    let dir = tempfile::tempdir().unwrap();
    // The "store" is a file, so no folder can be made under it.
    let blocker = dir.path().join("speech");
    std::fs::write(&blocker, b"x").unwrap();
    let error = write_selection(&blocker, SpeechEngine::Accurate).unwrap_err();
    assert_eq!(reason(&error), Some("storeFailed"));
}

#[test]
fn a_model_is_installed_when_the_marker_and_every_file_are_there() {
    let dir = tempfile::tempdir().unwrap();
    let folder = model_dir(dir.path(), &MOONSHINE_TINY);
    assert!(folder.ends_with(MOONSHINE_TINY.id));
    assert!(!is_installed(&folder, &MOONSHINE_TINY));
    install_fake(&folder, &MOONSHINE_TINY);
    assert!(is_installed(&folder, &MOONSHINE_TINY));
    // The marker of another model, or a file of the wrong size, or a missing file, is not an installation.
    assert!(!is_installed(&folder, &PARAKEET));
    let victim = folder.join("tokens.txt");
    std::fs::write(&victim, b"short").unwrap();
    assert!(!is_installed(&folder, &MOONSHINE_TINY));
    std::fs::remove_file(&victim).unwrap();
    assert!(!is_installed(&folder, &MOONSHINE_TINY));
}

#[test]
fn a_marker_of_an_older_commit_is_not_an_installation() {
    let dir = tempfile::tempdir().unwrap();
    let folder = model_dir(dir.path(), &MOONSHINE_TINY);
    install_fake(&folder, &MOONSHINE_TINY);
    std::fs::write(
        folder.join(INSTALLED_FILE),
        format!(
            r#"{{"id":"{}","commit":"0000000000000000000000000000000000000000"}}"#,
            MOONSHINE_TINY.id
        ),
    )
    .unwrap();
    assert!(!is_installed(&folder, &MOONSHINE_TINY));
}

#[test]
fn the_voice_activity_model_is_found_by_size_in_the_places_it_may_be() {
    let dir = tempfile::tempdir().unwrap();
    let resources = dir.path().join("res");
    let exe_dir = dir.path().join("exe");
    std::fs::create_dir_all(resources.join("resources")).unwrap();
    std::fs::create_dir_all(&exe_dir).unwrap();
    let candidates = vad_candidates(Some(&resources), Some(&exe_dir), None, false);
    assert_eq!(candidates.len(), 3);
    assert_eq!(locate_vad(&candidates), None);
    // A file of the wrong size is not the model.
    std::fs::write(resources.join(VAD_FILE.name), b"nope").unwrap();
    assert_eq!(locate_vad(&candidates), None);
    // The right size, under `resources/`.
    let good = resources.join("resources").join(VAD_FILE.name);
    std::fs::File::create(&good)
        .unwrap()
        .set_len(VAD_FILE.bytes)
        .unwrap();
    assert_eq!(locate_vad(&candidates), Some(good));
}

#[test]
fn the_vad_variable_counts_only_in_debug_builds_and_only_when_absolute() {
    let absolute = std::env::temp_dir().join("silero_vad.onnx");
    let env = Some(absolute.clone().into_os_string());
    assert_eq!(vad_candidates(None, None, env.clone(), true), [absolute]);
    assert!(vad_candidates(None, None, env, false).is_empty());
    assert!(vad_candidates(None, None, Some(OsString::from("relative.onnx")), true).is_empty());
    assert!(vad_candidates(None, None, None, true).is_empty());
}

#[test]
fn the_sidecar_sits_next_to_the_executable() {
    let path = sidecar_path(Path::new("dir"));
    assert!(path.starts_with("dir"));
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("kb-speech")
    );
    assert_eq!(path.extension().is_some(), cfg!(windows));
}

#[test]
fn the_store_needs_a_data_directory_and_lives_under_it() {
    let shell = SpeechShell::new();
    assert!(shell.store_root().is_err());
    shell.set_data_dir(Path::new("data"));
    assert_eq!(
        shell.store_root().unwrap(),
        Path::new("data").join(STORE_DIR)
    );
    // The first directory set stays.
    shell.set_data_dir(Path::new("other"));
    assert_eq!(
        shell.store_root().unwrap(),
        Path::new("data").join(STORE_DIR)
    );
}

#[test]
fn the_facts_follow_the_store_and_the_files_next_to_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let shell = SpeechShell::new();
    let root = dir.path().join("speech");
    let facts = gather_facts(&shell, &root);
    assert!(!facts.accurate_installed && !facts.light_installed);
    install_fake(&model_dir(&root, &PARAKEET), &PARAKEET);
    let facts = gather_facts(&shell, &root);
    assert!(facts.accurate_installed);
    assert!(!facts.light_installed);
}
