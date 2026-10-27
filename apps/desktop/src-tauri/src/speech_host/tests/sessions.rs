//! The session state machine against a fake sidecar.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kb_speech::api::DictationState;
use kb_speech::catalog::{MOONSHINE_TINY, PARAKEET};
use std::path::{Path, PathBuf};

use kb_speech::protocol::{FromSidecar, Recognizer, SidecarErrorCode, ToSidecar};

use super::super::*;
use super::harness::{
    Events, FakeLauncher, Next, QUICK, Script, error, is_reason, phrase, quiet, run, state,
    talkative, world, world_with,
};
use super::{field, reason};
use crate::ai_test_support::{eventually, put_setting};

#[test]
fn a_session_loads_listens_recognises_and_ends_in_order() {
    run(async {
        let world = world(vec![talkative(16_000, &["Hello there."])]);
        let events = world.begin("s1").await;
        assert_eq!(events.all(), [state(DictationState::Loading)]);
        events.listening().await;
        world.push("s1", 16_000);
        events
            .wait("the phrase", |events| {
                events.contains(&phrase("Hello there."))
            })
            .await;
        stop(&world.shell, "s1");
        let all = events.ended().await;
        assert_eq!(
            all,
            [
                state(DictationState::Loading),
                state(DictationState::Listening),
                state(DictationState::Recognising),
                phrase("Hello there."),
                state(DictationState::Listening),
                DictationEvent::Ended,
            ]
        );
        // The sidecar was configured for the engine, fed, flushed and asked to quit; the slot is free.
        let received = world.launcher.wire.received();
        match &received[0] {
            ToSidecar::Start(config) => {
                assert!(matches!(config.recognizer, Recognizer::Moonshine { .. }));
                assert_eq!(config.hard_cap_ms, 25_000);
                assert_eq!(config.preroll_ms, 150);
                assert!(config.vad_path.ends_with("silero_vad.onnx"));
            }
            other => panic!("{other:?}"),
        }
        assert!(received.contains(&ToSidecar::Flush));
        assert!(world.launcher.wire.got_quit());
        assert!(!world.shell.session_running());
        assert!(world.launcher.wire.left().await);
        assert!(!world.launcher.wire.was_killed());
    });
}

#[test]
fn the_accurate_engine_is_started_with_parakeets_files_and_threads() {
    run(async {
        let world = world(vec![quiet()]);
        let events = world.begin_as("s1", SpeechEngine::Accurate).await.unwrap();
        events.listening().await;
        match &world.launcher.wire.received()[0] {
            ToSidecar::Start(config) => {
                assert!(matches!(config.recognizer, Recognizer::Parakeet { .. }));
                assert_eq!(config.hard_cap_ms, 15_000);
                assert_eq!(config.threads, 2);
            }
            other => panic!("{other:?}"),
        }
        cancel(&world.shell, "s1");
        events.ended().await;
    });
}

#[test]
fn audio_is_cut_into_frames_of_a_second_and_kept_in_order() {
    run(async {
        let world = world(vec![quiet()]);
        let events = world.begin("s1").await;
        events.listening().await;
        // 2.5 s as one push, then 299 samples, then 1 sample; a ramp so the order shows.
        let ramp: Vec<i16> = (0..40_300_i32).map(|n| (n % 30_000) as i16).collect();
        audio(&world.shell, "s1", ramp[..40_000].to_vec()).unwrap();
        audio(&world.shell, "s1", ramp[40_000..40_299].to_vec()).unwrap();
        audio(&world.shell, "s1", ramp[40_299..].to_vec()).unwrap();
        stop(&world.shell, "s1");
        events.ended().await;
        let frames = world.launcher.wire.audio_frames();
        let sizes: Vec<usize> = frames.iter().map(Vec::len).collect();
        assert_eq!(sizes, [16_000, 16_000, 8_000, 299, 1]);
        let all: Vec<i16> = frames.into_iter().flatten().collect();
        assert_eq!(all, ramp);
    });
}

#[test]
fn a_flush_is_queued_behind_the_audio_and_audio_after_a_stop_is_refused() {
    run(async {
        let world = world(vec![quiet()]);
        let events = world.begin("s1").await;
        world.push("s1", 1_000);
        stop(&world.shell, "s1");
        let error = audio(&world.shell, "s1", vec![1; 10]).unwrap_err();
        assert!(is_reason(&error, "sessionId", "ended"), "{error:?}");
        // A second stop changes nothing.
        stop(&world.shell, "s1");
        events.ended().await;
        let kinds: Vec<&str> = world
            .launcher
            .wire
            .received()
            .iter()
            .map(|message| match message {
                ToSidecar::Start(_) => "start",
                ToSidecar::Audio(_) => "audio",
                ToSidecar::Flush => "flush",
                ToSidecar::Quit => "quit",
            })
            .collect();
        assert_eq!(kinds, ["start", "audio", "flush", "quit"]);
    });
}

#[test]
fn only_changes_of_state_are_sent_and_listening_returns_when_nothing_is_pending() {
    run(async {
        // Two segments end back to back; the text of one and the "nothing" of the other come after both ended.
        let mut sent = false;
        let script: Script = Box::new(move |message, out| {
            match message {
                ToSidecar::Start(_) => out.send(FromSidecar::Ready { load_ms: 1 }),
                ToSidecar::Audio(_) if !sent => {
                    sent = true;
                    out.send(FromSidecar::SpeechStarted);
                    out.send(FromSidecar::SpeechEnded);
                    out.send(FromSidecar::SpeechEnded);
                    out.send(FromSidecar::Text {
                        text: " One. ".into(),
                        start_ms: 0,
                        end_ms: 1,
                    });
                    out.send(FromSidecar::Empty {
                        start_ms: 1,
                        end_ms: 2,
                    });
                }
                ToSidecar::Flush => out.send(FromSidecar::Flushed),
                _ => {}
            }
            Next::Go
        });
        let world = world(vec![script]);
        let events = world.begin("s1").await;
        events.listening().await;
        world.push("s1", 1_600);
        events
            .wait("back to listening", |events| events.len() >= 5)
            .await;
        stop(&world.shell, "s1");
        assert_eq!(
            events.ended().await,
            [
                state(DictationState::Loading),
                state(DictationState::Listening),
                state(DictationState::Recognising),
                phrase("One."),
                state(DictationState::Listening),
                DictationEvent::Ended,
            ]
        );
    });
}

#[test]
fn a_blank_text_is_no_phrase() {
    run(async {
        let world = world(vec![talkative(1_600, &["   ", "Real."])]);
        let events = world.begin("s1").await;
        events.listening().await;
        world.push("s1", 1_600);
        world.push("s1", 1_600);
        events
            .wait("the second text", |events| {
                events.contains(&phrase("Real."))
            })
            .await;
        stop(&world.shell, "s1");
        events.ended().await;
        assert_eq!(events.phrases(), ["Real."]);
    });
}

#[test]
fn a_stop_waits_for_the_last_phrases_that_the_flush_brings() {
    run(async {
        let world = world(vec![talkative(1_000_000, &["unused"])]);
        let events = world.begin("s1").await;
        world.push("s1", 3_000);
        stop(&world.shell, "s1");
        let all = events.ended().await;
        // The phrase of the flush comes before the end (then listening again, then the end).
        let at_phrase = all
            .iter()
            .position(|event| *event == phrase("Last."))
            .unwrap();
        assert_eq!(at_phrase, all.len() - 3, "{all:?}");
        assert!(world.launcher.wire.got_quit());
    });
}

#[test]
fn a_second_session_is_busy_and_nothing_is_started_for_it() {
    run(async {
        let world = world(vec![quiet(), quiet()]);
        let first = world.begin("s1").await;
        let error = world.begin_as("s2", SpeechEngine::Light).await.unwrap_err();
        assert!(is_reason(&error, "sessionId", "busy"), "{error:?}");
        assert_eq!(world.launcher.launches(), 1);
        // The first goes on, and once it ended a new one may start at once.
        stop(&world.shell, "s1");
        first.ended().await;
        let second = world.begin("s2").await;
        assert_eq!(second.all(), [state(DictationState::Loading)]);
        cancel(&world.shell, "s2");
        second.ended().await;
        assert_eq!(world.launcher.launches(), 2);
    });
}

#[test]
fn the_checks_of_a_start_run_in_the_contracts_order() {
    run(async {
        // An invalid id.
        let world = world(vec![quiet()]);
        let error = world
            .begin_as("bad id", SpeechEngine::Light)
            .await
            .unwrap_err();
        assert_eq!(field(&error), Some("sessionId"));
        assert_eq!(world.launcher.launches(), 0);
        // A core that is not ready answers first.
        let starting: Arc<dyn CoreHost> =
            Arc::new(crate::state::AppState::<crate::ai_test_support::AiFake>::new());
        let error = start(
            starting,
            Arc::clone(&world.shell),
            Arc::clone(&world.ai),
            "bad id".into(),
            SpeechEngine::Light,
            Events::default().emitter(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, kb_types::ErrorCode::Internal);
        assert_eq!(error.message, "core is starting");
        // The model is not installed.
        let root = world.shell.store_root().unwrap();
        std::fs::remove_dir_all(model_dir(&root, &MOONSHINE_TINY)).unwrap();
        let error = world.begin_as("s1", SpeechEngine::Light).await.unwrap_err();
        assert!(is_reason(&error, "engine", "modelMissing"), "{error:?}");
        // The sidecar or the voice activity model is missing (the other model is installed).
        world.launcher.present.store(false, Ordering::SeqCst);
        let error = world
            .begin_as("s1", SpeechEngine::Accurate)
            .await
            .unwrap_err();
        assert!(is_reason(&error, "engine", "engineMissing"), "{error:?}");
        assert_eq!(world.launcher.launches(), 0);
        // Busy comes before the model's absence.
        world.launcher.present.store(true, Ordering::SeqCst);
        let running = world.begin_as("s1", SpeechEngine::Accurate).await.unwrap();
        let error = world.begin_as("s2", SpeechEngine::Light).await.unwrap_err();
        assert!(is_reason(&error, "sessionId", "busy"), "{error:?}");
        cancel(&world.shell, "s1");
        running.ended().await;
    });
}

#[test]
fn a_sidecar_that_cannot_be_started_is_engine_failed() {
    run(async {
        let world = world_with(FakeLauncher::with(vec![], false, true));
        let error = world.begin_as("s1", SpeechEngine::Light).await.unwrap_err();
        assert!(is_reason(&error, "engine", "engineFailed"), "{error:?}");
        assert!(!world.shell.session_running());
        assert_eq!(world.launcher.launches(), 1);
    });
}

#[test]
fn ollama_needs_ai_a_transcription_model_and_consent() {
    run(async {
        let world = world(vec![]);
        let unavailable = |error: &ApiError| is_reason(error, "engine", "ollamaUnavailable");
        // AI off.
        let error = world
            .begin_as("s1", SpeechEngine::Ollama)
            .await
            .unwrap_err();
        assert!(unavailable(&error), "{error:?}");
        // On, but no transcription model.
        put_setting(
            &world.core,
            kb_types::ai::AI_SETTINGS_SETTING,
            serde_json::json!({ "enabled": true }),
        );
        let error = world
            .begin_as("s1", SpeechEngine::Ollama)
            .await
            .unwrap_err();
        assert!(unavailable(&error), "{error:?}");
        // A model, but the endpoint is another computer nobody agreed to.
        put_setting(
            &world.core,
            kb_types::ai::AI_SETTINGS_SETTING,
            serde_json::json!({ "enabled": true, "transcriptionModel": "asr-model" }),
        );
        put_setting(
            &world.core,
            kb_types::ai::AI_ENDPOINT_SETTING,
            serde_json::json!({ "url": "http://nas:11434", "remoteConsent": false }),
        );
        let error = world
            .begin_as("s1", SpeechEngine::Ollama)
            .await
            .unwrap_err();
        assert!(unavailable(&error), "{error:?}");
        assert_eq!(world.launcher.launches(), 0);
    });
}

#[test]
fn a_cancel_ends_at_once_kills_the_process_and_drops_the_audio() {
    run(async {
        let world = world(vec![talkative(1_000_000, &["unused"])]);
        let events = world.begin("s1").await;
        events.listening().await;
        world.push("s1", 8_000);
        cancel(&world.shell, "s1");
        let all = events.ended().await;
        // No phrase after a cancel, and the session is gone.
        assert!(events.phrases().is_empty(), "{all:?}");
        assert!(world.launcher.wire.was_killed());
        assert!(!world.shell.session_running());
        let error = audio(&world.shell, "s1", vec![1; 10]).unwrap_err();
        assert!(is_reason(&error, "sessionId", "ended"));
        // An unknown session is fine for stop and cancel.
        stop(&world.shell, "s1");
        cancel(&world.shell, "nobody");
        // The sidecar process leaves.
        assert!(world.launcher.wire.left().await);
    });
}

#[test]
fn a_lock_ends_the_session() {
    run(async {
        let world = world(vec![quiet(), quiet()]);
        let events = world.begin("s1").await;
        world.shell.cancel_all();
        let all = events.ended().await;
        assert!(
            !all.iter()
                .any(|event| matches!(event, DictationEvent::Error { .. }))
        );
        assert!(world.launcher.wire.was_killed());
        assert!(!world.shell.session_running());
        // The same through the background task that a lock drops.
        let events = world.begin("s2").await;
        drop(speech_starter(Arc::clone(&world.shell))().unwrap());
        events.ended().await;
        assert!(!world.shell.session_running());
    });
}

#[test]
fn a_session_whose_channel_is_gone_ends_like_a_cancel() {
    run(async {
        let world = world(vec![talkative(1_000, &["x"])]);
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        // The receiver takes the first two events and then is gone.
        let emit: Arc<dyn Fn(DictationEvent) -> bool + Send + Sync> =
            Arc::new(move |_| counter.fetch_add(1, Ordering::SeqCst) < 2);
        start(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            Arc::clone(&world.ai),
            "s1".into(),
            SpeechEngine::Light,
            emit,
        )
        .await
        .unwrap();
        // Loading was the first event; the engine's `Ready` is the second; the third finds nobody.
        let _ = eventually(Duration::from_secs(5), || seen.load(Ordering::SeqCst) >= 2).await;
        world.push("s1", 1_000);
        assert!(eventually(Duration::from_secs(10), || !world.shell.session_running()).await);
        assert!(world.launcher.wire.killed().await);
    });
}

// ----- how a session fails -----

#[test]
fn a_process_that_ends_after_ready_is_engine_crashed() {
    run(async {
        let script: Script = Box::new(|message, out| match message {
            ToSidecar::Start(_) => {
                out.send(FromSidecar::Ready { load_ms: 1 });
                Next::Go
            }
            ToSidecar::Audio(_) => Next::Exit,
            _ => Next::Go,
        });
        let world = world(vec![script]);
        let events = world.begin("s1").await;
        events.listening().await;
        world.push("s1", 1_600);
        assert_eq!(
            events.ended().await,
            [
                state(DictationState::Loading),
                state(DictationState::Listening),
                error(DictationErrorCode::EngineCrashed),
                DictationEvent::Ended,
            ]
        );
        assert!(!world.shell.session_running());
    });
}

#[test]
fn a_process_that_ends_while_loading_is_engine_failed() {
    run(async {
        let script: Script = Box::new(|_message, _out| Next::Exit);
        let world = world(vec![script]);
        let events = world.begin("s1").await;
        assert_eq!(
            events.ended().await,
            [
                state(DictationState::Loading),
                error(DictationErrorCode::EngineFailed),
                DictationEvent::Ended,
            ]
        );
    });
}

#[test]
fn an_error_of_the_sidecar_is_engine_failed_but_a_failed_segment_is_not() {
    run(async {
        let script: Script = Box::new(|message, out| {
            match message {
                ToSidecar::Start(_) => out.send(FromSidecar::Ready { load_ms: 1 }),
                ToSidecar::Audio(pcm) if pcm.len() == 1 => {
                    out.send(FromSidecar::Error {
                        code: SidecarErrorCode::RecognizerFailed,
                        message: "a speech segment could not be recognised".into(),
                    });
                    out.send(FromSidecar::Empty {
                        start_ms: 0,
                        end_ms: 1,
                    });
                }
                ToSidecar::Audio(_) => out.send(FromSidecar::Error {
                    code: SidecarErrorCode::ModelLoadFailed,
                    message: "a speech model file could not be loaded".into(),
                }),
                _ => {}
            }
            Next::Go
        });
        let world = world(vec![script]);
        let events = world.begin("s1").await;
        events.listening().await;
        world.push("s1", 1);
        // The failed segment changes nothing the webview hears.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(world.shell.session_running());
        assert_eq!(events.all().len(), 2);
        world.push("s1", 2);
        assert_eq!(
            events.ended().await,
            [
                state(DictationState::Loading),
                state(DictationState::Listening),
                error(DictationErrorCode::EngineFailed),
                DictationEvent::Ended,
            ]
        );
    });
}

#[test]
fn garbage_from_the_sidecar_is_engine_failed() {
    run(async {
        let script: Script = Box::new(|message, out| {
            if matches!(message, ToSidecar::Start(_)) {
                out.send(FromSidecar::Ready { load_ms: 1 });
                out.raw(b"this is not a frame at all");
            }
            Next::Go
        });
        let world = world(vec![script]);
        let events = world.begin("s1").await;
        let all = events.ended().await;
        assert!(
            all.contains(&error(DictationErrorCode::EngineFailed)),
            "{all:?}"
        );
        assert!(world.launcher.wire.killed().await);
    });
}

#[test]
fn a_sidecar_that_never_says_ready_is_engine_failed_after_the_load_timeout() {
    run(async {
        let script: Script = Box::new(|_message, _out| Next::Go);
        let world = world(vec![script]);
        world.shell.set_timeouts(Timeouts {
            load: Duration::from_millis(200),
            ..QUICK
        });
        let began = Instant::now();
        let events = world.begin("s1").await;
        assert_eq!(
            events.ended().await,
            [
                state(DictationState::Loading),
                error(DictationErrorCode::EngineFailed),
                DictationEvent::Ended,
            ]
        );
        assert!(began.elapsed() >= Duration::from_millis(200));
        assert!(world.launcher.wire.killed().await);
    });
}

#[test]
fn a_stop_that_the_sidecar_never_answers_ends_after_the_flush_timeout_without_an_error() {
    run(async {
        let script: Script = Box::new(|message, out| {
            if matches!(message, ToSidecar::Start(_)) {
                out.send(FromSidecar::Ready { load_ms: 1 });
            }
            // Never answers the flush.
            Next::Go
        });
        let world = world(vec![script]);
        world.shell.set_timeouts(Timeouts {
            flush: Duration::from_millis(300),
            quit_grace: Duration::from_millis(100),
            ..QUICK
        });
        let events = world.begin("s1").await;
        events.listening().await;
        let began = Instant::now();
        stop(&world.shell, "s1");
        let all = events.ended().await;
        assert!(began.elapsed() >= Duration::from_millis(250));
        assert!(
            !all.iter()
                .any(|event| matches!(event, DictationEvent::Error { .. }))
        );
        // The process that did not answer is killed.
        assert!(world.launcher.wire.killed().await);
    });
}

#[test]
fn a_process_that_has_quit_is_not_killed() {
    run(async {
        let world = world(vec![quiet()]);
        let events = world.begin("s1").await;
        stop(&world.shell, "s1");
        events.ended().await;
        assert!(world.launcher.wire.left().await);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!world.launcher.wire.was_killed());
    });
}

#[test]
fn more_than_sixty_seconds_waiting_for_a_stuck_sidecar_ends_the_session() {
    run(async {
        let world = world_with(FakeLauncher::with(vec![quiet()], true, false));
        let events = world.begin("s1").await;
        // 61 s in pushes of a second: the sidecar reads nothing, so they pile up.
        let mut refused_at = None;
        for second in 0..70 {
            if audio(&world.shell, "s1", vec![5; 16_000]).is_err() {
                refused_at = Some(second);
                break;
            }
        }
        // The pushes up to 60 s are taken (a little of the first may already have been written).
        assert!(matches!(refused_at, Some(60..=61)), "{refused_at:?}");
        assert_eq!(
            events.ended().await,
            [
                state(DictationState::Loading),
                error(DictationErrorCode::EngineFailed),
                DictationEvent::Ended,
            ]
        );
        assert!(world.launcher.wire.was_killed());
        assert!(!world.shell.session_running());
    });
}

// ----- privacy -----

#[test]
fn the_sidecars_stderr_is_counted_and_only_its_own_line_is_logged() {
    let text = b"C:\\Users\\someone\\secret\\model.onnx could not be opened\nkb-speech: ended (Quit)\nwhat the person said\nkb-speech: ended (Whatever it likes)\n";
    let logs = crate::test_support::captured_logs(|| {
        session::stderr_loop(Box::new(std::io::Cursor::new(text.to_vec())));
    });
    assert!(logs.contains("the dictation sidecar ended"), "{logs}");
    assert!(logs.contains("diagnostics (not logged)"), "{logs}");
    for secret in [
        "someone",
        "secret",
        "model.onnx",
        "what the person said",
        "Whatever",
    ] {
        assert!(!logs.contains(secret), "{secret} leaked: {logs}");
    }
}

#[test]
fn nothing_the_person_said_reaches_the_log() {
    let world = world(vec![talkative(1_600, &["My secret sentence."])]);
    let events = Events::default();
    let logs = crate::test_support::captured_logs(|| {
        // The driver runs on this thread, where the subscriber is.
        run(async {
            let root = world.shell.store_root().unwrap();
            let (handle, rx) =
                prepare_session(&world.shell, &root, "s1", SpeechEngine::Light, None).unwrap();
            let driver = Driver::new(
                handle.clone(),
                rx,
                Arc::clone(&world.shell),
                events.emitter(),
                None,
                QUICK,
            );
            let task = tokio::spawn(driver.run());
            events.listening().await;
            world.push("s1", 1_600);
            events
                .wait("the phrase", |events| {
                    events.contains(&phrase("My secret sentence."))
                })
                .await;
            handle.stop();
            events.ended().await;
            task.await.unwrap();
        });
    });
    assert!(logs.contains("the dictation engine is ready"), "{logs}");
    assert!(!logs.contains("secret"), "{logs}");
    assert!(!logs.contains("sentence"), "{logs}");
}

#[test]
fn the_error_a_command_gives_never_carries_a_path_or_text() {
    run(async {
        let world = world(vec![]);
        let root = world.shell.store_root().unwrap();
        std::fs::remove_dir_all(model_dir(&root, &MOONSHINE_TINY)).unwrap();
        let error = world.begin_as("s1", SpeechEngine::Light).await.unwrap_err();
        let text = serde_json::to_string(&error).unwrap();
        assert!(
            !text.contains(world.dir.path().to_string_lossy().as_ref()),
            "{text}"
        );
        assert!(
            !text.contains(PARAKEET.id) && !text.contains(MOONSHINE_TINY.id),
            "{text}"
        );
        assert_eq!(reason(&error), Some("modelMissing"));
    });
}

// ----- the real sidecar -----

/// A launcher that starts the real `kb-speech` binary of this build (`target/debug`), with the real voice model
/// (`KB_SPEECH_VAD`).
struct RealLauncher {
    exe: PathBuf,
    vad: PathBuf,
}

impl Launcher for RealLauncher {
    fn locate(&self, _shell: &SpeechShell) -> Option<Located> {
        Some(Located {
            exe: self.exe.clone(),
            vad: self.vad.clone(),
        })
    }

    fn launch(&self, exe: &Path) -> std::io::Result<Sidecar> {
        ProcessLauncher.launch(exe)
    }
}

/// The samples of a 16 kHz mono 16-bit WAV file.
fn read_wav(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            return bytes[at + 8..at + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
        }
        at += 8 + size + size % 2;
    }
    panic!("no data chunk");
}

/// A whole dictation through the shell and the real sidecar with the real Moonshine model and a spoken WAV file:
/// `KB_TEST_MOONSHINE_DIR` (the model's folder), `KB_TEST_WAV` (16 kHz mono), `KB_SPEECH_VAD` (set by
/// `scripts/dev-env.ps1`); the binary is `target/debug/kb-speech.exe` (`cargo build -p kb-speech-native`). Run by hand
/// with `--ignored --nocapture`.
#[test]
#[ignore = "needs the real sidecar, the Moonshine files and a WAV file"]
fn a_real_session_dictates_a_spoken_file() {
    let model_source =
        PathBuf::from(std::env::var_os("KB_TEST_MOONSHINE_DIR").expect("KB_TEST_MOONSHINE_DIR"));
    let wav = read_wav(Path::new(
        &std::env::var_os("KB_TEST_WAV").expect("KB_TEST_WAV"),
    ));
    let vad = PathBuf::from(std::env::var_os(VAD_ENV).expect("KB_SPEECH_VAD"));
    let exe = std::env::current_exe().unwrap();
    let exe = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(if cfg!(windows) {
            "kb-speech.exe"
        } else {
            "kb-speech"
        });
    assert!(exe.is_file(), "build the sidecar first");
    run(async {
        let world = world_with(FakeLauncher::new(vec![]));
        world
            .shell
            .set_launcher(Arc::new(RealLauncher { exe, vad }) as Arc<dyn Launcher>);
        world.shell.set_timeouts(Timeouts {
            load: Duration::from_secs(60),
            flush: Duration::from_secs(20),
            quit_grace: Duration::from_secs(2),
        });
        let root = world.shell.store_root().unwrap();
        std::fs::remove_dir_all(model_dir(&root, &MOONSHINE_TINY)).unwrap();
        import_folder(
            &model_dir(&root, &MOONSHINE_TINY),
            &MOONSHINE_TINY,
            &model_source,
        )
        .unwrap();
        let began = Instant::now();
        let events = world.begin("real").await;
        events.listening().await;
        println!("ready after {:?}", began.elapsed());
        // The file, in pieces of 100 ms pushed a little faster than real time, then a second of room tone.
        let mut pcm = wav.clone();
        pcm.extend(std::iter::repeat_n(0, 16_000));
        for chunk in pcm.chunks(1_600) {
            audio(&world.shell, "real", chunk.to_vec()).unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        stop(&world.shell, "real");
        let all = events.ended().await;
        println!("events: {all:#?}");
        let phrases = events.phrases();
        assert!(!phrases.is_empty());
        println!("joined: {}", phrases.join(" "));
        assert!(!world.shell.session_running());
        assert!(
            !all.iter()
                .any(|event| matches!(event, DictationEvent::Error { .. }))
        );
    });
}
