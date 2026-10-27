//! The Ollama engine: a `VadOnly` sidecar sends the speech back and the shell transcribes it through
//! `ai_host::transcribe` (the stub Ollama of the AI tests).

use std::time::{Duration, Instant};

use kb_speech::api::DictationState;
use kb_speech::protocol::{FromSidecar, Recognizer, ToSidecar};
use kb_speech::wav::encode_wav;
use serde_json::{Value, json};

use super::super::*;
use super::harness::{Events, Next, Script, World, error, phrase, run, state, world};
use crate::ai_test_support::{ChatBehaviour, Stub, ai_on, eventually, start_stub};

fn asr_body(text: &str) -> String {
    json!({ "model": "asr-model", "message": { "role": "assistant", "content": text }, "done": true })
        .to_string()
}

fn asr_answer(text: &str) -> ChatBehaviour {
    ChatBehaviour::Json(asr_body(text))
}

/// An answer that takes a while.
fn slow_answer(text: &str, delay: Duration) -> ChatBehaviour {
    ChatBehaviour::SlowStart(delay, vec![asr_body(text).into_bytes()])
}

/// The samples of a piece of speech that the fake sends back: `value` all through.
const PIECE_SAMPLES: usize = 4_000;

/// A `VadOnly` sidecar: every push of audio ends a segment that comes back as `pieces` pieces of speech, the
/// n-th made of the value n.
fn vad_only(pieces: usize) -> Script {
    Box::new(move |message, out| {
        match message {
            ToSidecar::Start(config) => {
                assert_eq!(config.recognizer, Recognizer::VadOnly);
                out.send(FromSidecar::Ready { load_ms: 1 });
            }
            ToSidecar::Audio(_) => {
                out.send(FromSidecar::SpeechStarted);
                out.send(FromSidecar::SpeechEnded);
                for piece in 1..=pieces {
                    out.send(FromSidecar::SegmentAudio {
                        pcm: vec![i16::try_from(piece).unwrap(); PIECE_SAMPLES],
                        start_ms: 0,
                        end_ms: 250,
                    });
                }
            }
            ToSidecar::Flush => out.send(FromSidecar::Flushed),
            _ => {}
        }
        Next::Go
    })
}

async fn ollama_world(stub: &Stub, scripts: Vec<Script>) -> (World, Events) {
    let world = world(scripts);
    ai_on(&world.core, &stub.url(), None, None);
    let events = world.begin_as("s1", SpeechEngine::Ollama).await.unwrap();
    events.listening().await;
    (world, events)
}

#[test]
fn each_segment_is_transcribed_and_comes_back_as_a_phrase() {
    run(async {
        let stub = start_stub(asr_answer("First phrase.")).await;
        let (world, events) = ollama_world(&stub, vec![vad_only(1)]).await;
        world.push("s1", 1_600);
        events
            .wait("the first phrase", |events| {
                events.contains(&phrase("First phrase."))
            })
            .await;
        *stub.state.chat.lock().unwrap() = asr_answer("[silence]");
        world.push("s1", 1_600);
        // The second segment was blank: no phrase, back to listening.
        events
            .wait("listening after the blank", |events| events.len() >= 7)
            .await;
        *stub.state.chat.lock().unwrap() = asr_answer("language English<asr_text> Third one. ");
        world.push("s1", 1_600);
        events
            .wait("the third phrase", |events| {
                events.contains(&phrase("Third one."))
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
                phrase("First phrase."),
                state(DictationState::Listening),
                state(DictationState::Recognising),
                state(DictationState::Listening),
                state(DictationState::Recognising),
                phrase("Third one."),
                state(DictationState::Listening),
                DictationEvent::Ended,
            ]
        );
        // The sidecar only found the speech.
        let received = stub.state.received("/api/chat");
        assert_eq!(received.len(), 3);
        assert_eq!(received[0].body["model"], "asr-model");
        assert_eq!(received[0].body["stream"], false);
        assert_eq!(
            received[0].body["messages"][0]["images"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn the_pieces_of_a_segment_are_sent_as_wav_files_one_at_a_time_in_order() {
    run(async {
        let stub = start_stub(asr_answer("Piece.")).await;
        let (world, events) = ollama_world(&stub, vec![vad_only(3)]).await;
        world.push("s1", 1_600);
        events
            .wait("three phrases", |events| {
                events
                    .iter()
                    .filter(|event| matches!(event, DictationEvent::Phrase { .. }))
                    .count()
                    == 3
            })
            .await;
        stop(&world.shell, "s1");
        let all = events.ended().await;
        assert_eq!(events.phrases(), ["Piece.", "Piece.", "Piece."]);
        // One recognising, one listening at the end.
        assert_eq!(
            all.iter()
                .filter(|event| **event == state(DictationState::Recognising))
                .count(),
            1
        );
        assert_eq!(all[all.len() - 2], state(DictationState::Listening));
        // The requests carry the WAV of each piece, in order.
        let received = stub.state.received("/api/chat");
        assert_eq!(received.len(), 3);
        for (index, request) in received.iter().enumerate() {
            let wav = encode_wav(&vec![i16::try_from(index + 1).unwrap(); PIECE_SAMPLES]);
            let expected: Value =
                serde_json::to_value(kb_ai::prompts::build_transcription("asr-model", &wav))
                    .unwrap();
            assert_eq!(
                request.body["messages"][0]["images"], expected["messages"][0]["images"],
                "request {index}"
            );
        }
    });
}

#[test]
fn three_failed_requests_in_a_row_end_the_session_and_a_success_resets_the_count() {
    run(async {
        let stub = start_stub(ChatBehaviour::Error(500, "{\"error\":\"boom\"}".into())).await;
        let (world, events) = ollama_world(&stub, vec![vad_only(1)]).await;
        // Two failures, a success, two more failures: the session is still there.
        for answer in [
            ChatBehaviour::Error(500, "{\"error\":\"boom\"}".into()),
            ChatBehaviour::Error(500, "{\"error\":\"boom\"}".into()),
            asr_answer("Fine."),
            ChatBehaviour::Error(500, "{\"error\":\"boom\"}".into()),
            ChatBehaviour::Error(500, "{\"error\":\"boom\"}".into()),
        ] {
            let before = stub.state.received("/api/chat").len();
            *stub.state.chat.lock().unwrap() = answer;
            world.push("s1", 1_600);
            let state_ref = Arc::clone(&stub.state);
            assert!(
                eventually(Duration::from_secs(10), move || state_ref
                    .received("/api/chat")
                    .len()
                    > before)
                .await
            );
            // Let the answer be handled before the next one is asked for.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(world.shell.session_running());
        assert_eq!(events.phrases(), ["Fine."]);
        // The third in a row.
        *stub.state.chat.lock().unwrap() = ChatBehaviour::Error(500, "{\"error\":\"boom\"}".into());
        world.push("s1", 1_600);
        let all = events.ended().await;
        assert_eq!(
            all[all.len() - 2],
            error(DictationErrorCode::OllamaUnavailable)
        );
        assert_eq!(all[all.len() - 1], DictationEvent::Ended);
        assert!(world.launcher.wire.killed().await);
    });
}

#[test]
fn a_stop_waits_for_the_requests_of_the_last_segments() {
    run(async {
        let stub = start_stub(slow_answer("Slow phrase.", Duration::from_millis(400))).await;
        let (world, events) = ollama_world(&stub, vec![vad_only(2)]).await;
        world.push("s1", 1_600);
        let began = Instant::now();
        stop(&world.shell, "s1");
        let all = events.ended().await;
        // Both phrases came before the end, which waited for the two requests (400 ms each, one at a time).
        assert_eq!(events.phrases(), ["Slow phrase.", "Slow phrase."]);
        assert!(
            began.elapsed() >= Duration::from_millis(700),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(all[all.len() - 1], DictationEvent::Ended);
        assert!(world.launcher.wire.got_quit());
    });
}

#[test]
fn a_cancel_drops_the_requests_that_wait() {
    run(async {
        let stub = start_stub(slow_answer("Never.", Duration::from_secs(30))).await;
        let (world, events) = ollama_world(&stub, vec![vad_only(2)]).await;
        world.push("s1", 1_600);
        let state_ref = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(10), move || !state_ref
                .received("/api/chat")
                .is_empty())
            .await
        );
        cancel(&world.shell, "s1");
        let began = Instant::now();
        events.ended().await;
        assert!(began.elapsed() < Duration::from_secs(5));
        assert!(events.phrases().is_empty());
        assert!(!world.shell.session_running());
    });
}

#[test]
fn a_person_who_talks_faster_than_ollama_answers_ends_the_session() {
    run(async {
        let stub = start_stub(slow_answer("Late.", Duration::from_secs(30))).await;
        let (world, events) = ollama_world(&stub, vec![vad_only(1)]).await;
        // One request in flight and eight waiting is all that is kept; the tenth segment is too much.
        for _ in 0..12 {
            if audio(&world.shell, "s1", vec![1; 1_600]).is_err() {
                break;
            }
        }
        let all = events.ended().await;
        assert!(
            all.contains(&error(DictationErrorCode::OllamaUnavailable)),
            "{all:?}"
        );
        assert!(world.launcher.wire.killed().await);
        assert!(!world.shell.session_running());
    });
}

#[test]
fn the_engine_is_not_the_sidecars_recognizer() {
    // Parakeet and Moonshine have models in the store; the Ollama engine has none and starts VadOnly.
    let config = SpeechEngine::Ollama.start_config(
        std::path::Path::new("ignored"),
        std::path::Path::new("vad.onnx"),
    );
    assert_eq!(config.recognizer, Recognizer::VadOnly);
    assert_eq!(config.hard_cap_ms, 15_000);
}
