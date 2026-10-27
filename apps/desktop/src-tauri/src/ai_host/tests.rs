//! Tests of the AI commands' logic against a scripted core and a stub Ollama.

// The live tests (ignored; run by hand) print what the real Ollama answered.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]

use std::future::Future;
use std::sync::atomic::Ordering;

use kb_types::ai::{AiChatMessage, AiRole};
use kb_types::{ErrorCode, SpaceId};

use super::*;
use crate::ai_test_support::{
    ChatBehaviour, FakeConfirm, Stub, ai_on, closed_port, done_line, eventually, line, page_info,
    passage, put_setting, ready_host, requests_of, start_stub, wav_bytes,
};
use crate::test_support::captured_logs_through;

/// Runs a future on a runtime of its own.
fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

fn quick() -> crate::ai_client::Timeouts {
    crate::ai_client::Timeouts {
        connect: Duration::from_secs(1),
        plain: Duration::from_secs(5),
        meta: Duration::from_secs(5),
        first_byte: Duration::from_secs(5),
        between_chunks: Duration::from_secs(5),
        show_cache: Duration::from_secs(60),
    }
}

fn shell() -> Arc<AiShell> {
    Arc::new(AiShell::with_client(AiClient::with_timeouts(quick())))
}

/// What a stream sent, collected.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<AiStreamEvent>>>);

impl Events {
    fn emitter(&self) -> impl Fn(AiStreamEvent) -> bool + Send + Sync + 'static {
        let inner = Arc::clone(&self.0);
        move |event| {
            inner.lock().unwrap().push(event);
            true
        }
    }

    fn all(&self) -> Vec<AiStreamEvent> {
        self.0.lock().unwrap().clone()
    }

    fn deltas(&self) -> Vec<String> {
        self.all()
            .into_iter()
            .filter_map(|event| match event {
                AiStreamEvent::Delta { text } => Some(text),
                _ => None,
            })
            .collect()
    }

    fn last(&self) -> AiStreamEvent {
        self.all().pop().unwrap()
    }
}

fn reason(error: &ApiError) -> String {
    reason_of(error).to_owned()
}

fn field(error: &ApiError) -> String {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("field")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn writing(action: AiAction, text: &str) -> AiGenerateInput {
    AiGenerateInput {
        action,
        text: text.to_owned(),
        context: None,
        language: None,
        tone: None,
        prompt: None,
    }
}

fn question(text: &str, scope: AiScope) -> AiChatInput {
    AiChatInput {
        messages: vec![AiChatMessage {
            role: AiRole::User,
            content: text.to_owned(),
        }],
        scope,
    }
}

fn streamed(pieces: &[&str]) -> ChatBehaviour {
    let mut lines: Vec<Vec<u8>> = pieces.iter().map(|piece| line(piece)).collect();
    lines.push(done_line());
    ChatBehaviour::Pieces(lines, Duration::ZERO)
}

/// A ready core with AI on against `stub`, the models the stub lists, and the shell.
async fn setup(
    stub: &Stub,
) -> (
    Arc<dyn CoreHost>,
    crate::ai_test_support::SharedScript,
    Arc<AiShell>,
) {
    let (_state, host, script) = ready_host();
    ai_on(
        &script,
        &stub.url(),
        Some("chat-model"),
        Some("embed-model"),
    );
    (host, script, shell())
}

async fn generate_events(
    host: &Arc<dyn CoreHost>,
    shell: &Arc<AiShell>,
    id: &str,
    input: AiGenerateInput,
) -> (Result<(), ApiError>, Events) {
    let events = Events::default();
    let emit = events.emitter();
    let outcome = generate(
        Arc::clone(host),
        Arc::clone(shell),
        id.to_owned(),
        input,
        &emit,
    )
    .await;
    (outcome, events)
}

// --- the consent rules ---

#[test]
fn a_local_endpoint_needs_no_consent_and_a_remote_one_needs_the_consent_to_that_exact_url() {
    let endpoint = |url: &str, consent: bool| AiEndpoint {
        url: url.to_owned(),
        remote_consent: consent,
    };
    for local in [
        "http://127.0.0.1:11434",
        "http://127.0.0.1:11434/",
        "http://localhost:11434",
        "http://LOCALHOST",
        "http://[::1]:11434",
        "http://127.1.2.3:8080",
    ] {
        assert!(allowed_url(&endpoint(local, false)).is_ok(), "{local}");
    }
    assert_eq!(
        allowed_url(&endpoint("http://127.0.0.1:11434/", false)).unwrap(),
        "http://127.0.0.1:11434"
    );
    for remote in [
        "http://192.168.1.20:11434",
        "http://10.0.0.5",
        "https://ollama.example.com",
        "http://localhost.evil.com:11434",
        "http://127.0.0.1.evil.com",
        "http://my-nas:11434",
    ] {
        let error = allowed_url(&endpoint(remote, false)).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "{remote}");
        assert_eq!(field(&error), "endpoint", "{remote}");
        assert_eq!(error.message, "consent required", "{remote}");
        assert_eq!(reason(&error), "consent", "{remote}");
        assert!(allowed_url(&endpoint(remote, true)).is_ok(), "{remote}");
    }
    // A stored value that is not an endpoint is refused, not guessed at.
    for broken in [
        "",
        "ftp://x",
        "http://user:pw@host",
        "http://h/path?q=1",
        "not a url",
    ] {
        let error = allowed_url(&endpoint(broken, true)).unwrap_err();
        assert_eq!(field(&error), "endpoint", "{broken:?}");
    }
}

#[test]
fn the_default_endpoint_is_the_local_ollama_without_consent() {
    let (_state, host, script) = ready_host();
    let stored = read_endpoint(&*host).unwrap();
    assert_eq!(stored.url, DEFAULT_AI_ENDPOINT);
    assert!(!stored.remote_consent);
    put_setting(&script, AI_ENDPOINT_SETTING, json!("garbage"));
    assert_eq!(read_endpoint(&*host).unwrap().url, DEFAULT_AI_ENDPOINT);
}

// --- AI off ---

#[test]
fn with_ai_off_every_command_but_the_status_and_the_endpoint_refuses_for_the_field_enabled() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (_state, host, script) = ready_host();
        // The endpoint is set, AI is not on (nothing stored).
        put_setting(
            &script,
            AI_ENDPOINT_SETTING,
            json!({ "url": stub.url(), "remoteConsent": false }),
        );
        let shell = shell();

        let (outcome, events) =
            generate_events(&host, &shell, "g1", writing(AiAction::Improve, "text")).await;
        let error = outcome.unwrap_err();
        assert_eq!(
            (error.code, field(&error)),
            (ErrorCode::InvalidInput, "enabled".into())
        );
        assert_eq!(error.message, "AI is turned off");
        assert!(events.all().is_empty(), "nothing is streamed");

        let events = Events::default();
        let emit = events.emitter();
        let error = chat(
            Arc::clone(&host),
            Arc::clone(&shell),
            "c1".into(),
            question("why?", AiScope::All),
            &emit,
        )
        .await
        .unwrap_err();
        assert_eq!(field(&error), "enabled");

        let error = search(
            Arc::clone(&host),
            Arc::clone(&shell),
            "q".into(),
            AiScope::All,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(field(&error), "enabled");

        let error = transcribe(Arc::clone(&host), Arc::clone(&shell), wav_bytes(10), None)
            .await
            .unwrap_err();
        assert_eq!(field(&error), "enabled");

        // Nothing reached Ollama and no embedding was asked for.
        assert!(stub.state.requests.lock().unwrap().is_empty());
        assert!(requests_of(&script, "AiRetrieve").is_empty());

        // The status works with AI off, and says what Ollama is.
        let status = status(Arc::clone(&host), Arc::clone(&shell)).await.unwrap();
        assert!(status.reachable && status.local);
        assert_eq!(status.version.as_deref(), Some("0.35.0"));
        assert_eq!(status.models.len(), 3);
    });
}

#[test]
fn a_locked_or_starting_core_answers_before_anything_else() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (state, host, script) = ready_host();
        ai_on(
            &script,
            &stub.url(),
            Some("chat-model"),
            Some("embed-model"),
        );
        host.lock().unwrap();
        let shell = shell();
        let (outcome, _) =
            generate_events(&host, &shell, "g1", writing(AiAction::Improve, "x")).await;
        assert_eq!(outcome.unwrap_err().code, ErrorCode::Locked);
        assert_eq!(
            status(Arc::clone(&host), Arc::clone(&shell))
                .await
                .unwrap_err()
                .code,
            ErrorCode::Locked
        );
        assert_eq!(
            search(
                Arc::clone(&host),
                Arc::clone(&shell),
                "q".into(),
                AiScope::All,
                None
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::Locked
        );
        assert!(stub.state.requests.lock().unwrap().is_empty());

        let confirm = FakeConfirm::saying(&[true]);
        let error = endpoint_set(
            Arc::clone(&host),
            shell,
            confirm.clone(),
            "http://nas:11434".into(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Locked);
        assert_eq!(confirm.count(), 0, "no dialog while locked");
        drop(state);
    });
}

// --- status ---

#[test]
fn the_status_has_the_models_the_index_and_the_indexer() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, script, shell) = setup(&stub).await;
        script.lock().unwrap().index_info = kb_types::ai::AiIndexInfo {
            model: Some("embed-model".into()),
            pages_indexed: 3,
            pages_total: 5,
            chunks: 9,
            dirty: 2,
            dimensions: Some(16),
        };
        shell.update_indexer(|state| {
            state.phase = kb_types::ai::AiIndexerPhase::Indexing;
            state.last_run_at = Some(42);
        });
        let status = status(host, shell).await.unwrap();
        assert!(status.reachable && status.local && !status.remote_consent);
        assert_eq!(status.endpoint, stub.url());
        assert_eq!(status.version.as_deref(), Some("0.35.0"));
        let names: Vec<&str> = status.models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["chat-model", "embed-model", "asr-model"]);
        assert!(status.error.is_none());
        assert_eq!(status.index.unwrap().pages_indexed, 3);
        assert_eq!(status.indexer.phase, kb_types::ai::AiIndexerPhase::Indexing);
        assert_eq!(status.indexer.last_run_at, Some(42));
    });
}

#[test]
fn an_unreachable_ollama_is_a_status_with_an_error_not_a_failure() {
    run(async {
        let (_state, host, script) = ready_host();
        let port = closed_port();
        ai_on(
            &script,
            &format!("http://127.0.0.1:{port}"),
            Some("m"),
            None,
        );
        let status = status(host, shell()).await.unwrap();
        assert!(!status.reachable && status.models.is_empty() && status.version.is_none());
        let error = status.error.unwrap();
        assert_eq!(
            (error.code, field(&error), reason(&error)),
            (
                ErrorCode::InvalidInput,
                "endpoint".into(),
                "unreachable".into()
            )
        );
    });
}

#[test]
fn the_status_of_a_remote_endpoint_without_consent_makes_no_call_and_says_so() {
    run(async {
        let (_state, host, script) = ready_host();
        put_setting(
            &script,
            AI_ENDPOINT_SETTING,
            json!({ "url": "http://ollama-nas.invalid:11434", "remoteConsent": false }),
        );
        let started = Instant::now();
        let status = status(host, shell()).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "no name lookup, no connection"
        );
        assert!(!status.local && !status.remote_consent && !status.reachable);
        assert_eq!(status.endpoint, "http://ollama-nas.invalid:11434");
        let error = status.error.unwrap();
        assert_eq!(reason(&error), "consent");
    });
}

// --- the endpoint ---

#[test]
fn a_local_endpoint_is_stored_at_once_with_no_dialog() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (_state, host, script) = ready_host();
        let confirm = FakeConfirm::saying(&[]);
        let status = endpoint_set(
            Arc::clone(&host),
            shell(),
            confirm.clone(),
            format!("  {}/  ", stub.url()),
        )
        .await
        .unwrap();
        assert_eq!(confirm.count(), 0);
        assert_eq!(status.endpoint, stub.url(), "normalised");
        assert!(status.reachable);
        let stored = script.lock().unwrap().settings[AI_ENDPOINT_SETTING].clone();
        assert_eq!(stored, json!({ "url": stub.url(), "remoteConsent": false }));
        // The settings event was emitted, so the indexer is woken.
        assert!(
            script
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|request| matches!(request, Request::SettingsSet { key, .. } if key == AI_ENDPOINT_SETTING))
        );
    });
}

#[test]
fn a_remote_endpoint_asks_in_the_native_dialog_and_is_stored_with_consent_only_on_allow() {
    let (_state, host, script) = ready_host();
    let confirm = FakeConfirm::saying(&[true]);
    store_endpoint(&*host, &*confirm, "https://Ollama.Example.com:11434/").unwrap();
    let asked = confirm.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].title, "Send text to another computer?");
    assert_eq!(
        asked[0].message,
        "AI features will send the text of your pages, your questions and your recordings to ollama.example.com. Anyone who controls that computer can read them."
    );
    assert_eq!(
        (asked[0].accept.as_str(), asked[0].cancel.as_str()),
        ("Allow", "Cancel")
    );
    assert_eq!(
        script.lock().unwrap().settings[AI_ENDPOINT_SETTING],
        json!({ "url": "https://ollama.example.com:11434", "remoteConsent": true })
    );
}

#[test]
fn the_consent_dialog_is_written_in_the_words_the_ui_sent_and_still_names_the_computer() {
    let (_state, host, script) = ready_host();
    let french = Labels {
        dialog_ai_title: "Envoyer du texte \u{e0} un autre ordinateur ?".to_owned(),
        dialog_ai_message: "Les fonctions d'IA enverront le texte de vos pages \u{e0} {host}. Toute personne qui contr\u{f4}le cet ordinateur peut le lire.".to_owned(),
        dialog_allow: "Autoriser".to_owned(),
        dialog_cancel: "Annuler".to_owned(),
        ..Labels::default()
    };
    let confirm = FakeConfirm::saying_in(&[true], french);
    store_endpoint(&*host, &*confirm, "https://Ollama.Example.com:11434/").unwrap();
    let asked = confirm.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].title,
        "Envoyer du texte \u{e0} un autre ordinateur ?"
    );
    assert_eq!(
        asked[0].message,
        "Les fonctions d'IA enverront le texte de vos pages \u{e0} ollama.example.com. Toute personne qui contr\u{f4}le cet ordinateur peut le lire."
    );
    assert_eq!(
        (asked[0].accept.as_str(), asked[0].cancel.as_str()),
        ("Autoriser", "Annuler")
    );
    // The consent is the same: stored with the URL it was given for.
    assert_eq!(
        script.lock().unwrap().settings[AI_ENDPOINT_SETTING],
        json!({ "url": "https://ollama.example.com:11434", "remoteConsent": true })
    );
}

#[test]
fn declining_changes_nothing_and_is_invalid_input_for_the_url_with_the_reason_declined() {
    let (_state, host, script) = ready_host();
    // A local endpoint is in place.
    store_endpoint(&*host, &*FakeConfirm::saying(&[]), "http://127.0.0.1:11434").unwrap();
    let before = script.lock().unwrap().settings.clone();
    let events_before = script.lock().unwrap().events.len();

    let confirm = FakeConfirm::saying(&[false]);
    let error = store_endpoint(&*host, &*confirm, "http://192.168.1.20:11434").unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(field(&error), "url");
    assert_eq!(reason(&error), "declined");
    assert_eq!(script.lock().unwrap().settings, before);
    assert_eq!(
        script.lock().unwrap().events.len(),
        events_before,
        "no change, no event"
    );
    assert!(!error.message.contains("192.168"), "{}", error.message);
}

#[test]
fn consent_belongs_to_the_exact_url() {
    let (_state, host, script) = ready_host();
    let confirm = FakeConfirm::saying(&[true, true, true]);
    store_endpoint(&*host, &*confirm, "http://nas:11434").unwrap();
    assert_eq!(confirm.count(), 1);
    // The same URL again, with consent: not asked again.
    store_endpoint(&*host, &*confirm, "http://nas:11434/").unwrap();
    assert_eq!(confirm.count(), 1);
    // Another URL, even the same host on another port, is a new question.
    store_endpoint(&*host, &*confirm, "http://nas:11435").unwrap();
    assert_eq!(confirm.count(), 2);
    store_endpoint(&*host, &*confirm, "https://nas:11435").unwrap();
    assert_eq!(confirm.count(), 3);
    // Going back to the first one asks again: it is no longer the stored one.
    let confirm = FakeConfirm::saying(&[false]);
    assert!(store_endpoint(&*host, &*confirm, "http://nas:11434").is_err());
    assert_eq!(confirm.count(), 1);
    // A local URL forgets the consent altogether.
    store_endpoint(&*host, &*confirm, "http://localhost:11434").unwrap();
    assert_eq!(
        script.lock().unwrap().settings[AI_ENDPOINT_SETTING],
        json!({ "url": "http://localhost:11434", "remoteConsent": false })
    );
    // After that the earlier remote URL has no consent any more.
    let confirm = FakeConfirm::saying(&[false]);
    assert!(store_endpoint(&*host, &*confirm, "https://nas:11435").is_err());
    assert_eq!(confirm.count(), 1);
}

#[test]
fn what_is_not_an_endpoint_is_refused_before_any_dialog() {
    let (_state, host, script) = ready_host();
    let confirm = FakeConfirm::saying(&[true]);
    for bad in [
        "",
        "   ",
        "ftp://nas",
        "nas:11434",
        "http://user:SECRETPW@nas:11434",
        "http://nas:11434/api",
        "http://nas:11434?x=1",
        "http://nas:11434#top",
        "javascript:alert(1)",
    ] {
        let error = store_endpoint(&*host, &*confirm, bad).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
        assert_eq!(field(&error), "url", "{bad:?}");
        assert!(!error.message.contains("SECRETPW"), "{}", error.message);
    }
    assert_eq!(confirm.count(), 0);
    assert!(
        !script
            .lock()
            .unwrap()
            .settings
            .contains_key(AI_ENDPOINT_SETTING)
    );
}

// --- the writing assistant ---

#[test]
fn a_writing_action_streams_its_deltas_and_ends_with_the_cleaned_answer() {
    run(async {
        let stub = start_stub(streamed(&[
            "<think>hmm</think>",
            "```markdown\n",
            "Better ",
            "text",
            "\n```",
        ]))
        .await;
        let (host, _script, shell) = setup(&stub).await;
        let (outcome, events) = generate_events(
            &host,
            &shell,
            "req-1",
            writing(AiAction::Improve, "Bad text"),
        )
        .await;
        outcome.unwrap();
        // The tokens are raw; the end carries the cleaned text and the model.
        assert_eq!(
            events.deltas(),
            [
                "<think>hmm</think>",
                "```markdown\n",
                "Better ",
                "text",
                "\n```"
            ]
        );
        assert_eq!(
            events.last(),
            AiStreamEvent::Done {
                text: "Better text".into(),
                model: "chat-model".into()
            }
        );
        let sent = &stub.state.received("/api/chat")[0].body;
        assert_eq!(sent["model"], "chat-model");
        assert_eq!(sent["stream"], true);
        assert!(
            sent["messages"][1]["content"]
                .as_str()
                .unwrap()
                .contains("Bad text")
        );
        // The model thinks, so the setting is sent (off); the context is sized from `show`.
        assert_eq!(sent["think"], false);
        assert!(sent["options"]["num_ctx"].as_u64().unwrap() >= 4096);
        // The stream is forgotten when it ends.
        assert!(shell.streams().is_empty());
    });
}

#[test]
fn think_is_sent_only_to_a_model_that_can_and_with_the_persons_setting() {
    run(async {
        let stub = start_stub(streamed(&["ok"])).await;
        let (host, script, shell) = setup(&stub).await;
        let mut settings = AiSettings {
            enabled: true,
            chat_model: Some("chat-model".into()),
            embedding_model: Some("embed-model".into()),
            think: true,
            ..AiSettings::default()
        };
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            serde_json::to_value(&settings).unwrap(),
        );
        generate_events(&host, &shell, "t1", writing(AiAction::Improve, "x"))
            .await
            .0
            .unwrap();
        assert_eq!(stub.state.received("/api/chat")[0].body["think"], true);

        // A model without the capability gets no `think`, whatever the setting says.
        settings.chat_model = Some("asr-model".into());
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            serde_json::to_value(&settings).unwrap(),
        );
        generate_events(&host, &shell, "t2", writing(AiAction::Improve, "x"))
            .await
            .0
            .unwrap();
        assert!(
            stub.state.received("/api/chat")[1]
                .body
                .get("think")
                .is_none()
        );
    });
}

#[test]
fn the_core_lock_is_not_held_while_ollama_streams() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("start ")])).await;
        let (host, _script, shell) = setup(&stub).await;
        let events = Events::default();
        let emit = events.emitter();
        let task = {
            let (host, shell) = (Arc::clone(&host), Arc::clone(&shell));
            tokio::spawn(async move {
                generate(
                    host,
                    shell,
                    "hold".into(),
                    writing(AiAction::Continue, "x"),
                    &emit,
                )
                .await
            })
        };
        let started = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(3), move || started
                .chat_started
                .load(Ordering::SeqCst)
                == 1)
            .await
        );
        // The stream is in progress: the core answers at once (its lock is free).
        let before = Instant::now();
        let answered = tokio::task::spawn_blocking({
            let host = Arc::clone(&host);
            move || {
                host.dispatch(Request::SettingsGet {
                    key: "ui.theme".into(),
                })
            }
        })
        .await
        .unwrap();
        assert!(answered.is_ok());
        assert!(before.elapsed() < Duration::from_millis(500));
        assert_eq!(shell.streams().len(), 1);
        shell.streams().cancel("hold");
        task.await.unwrap().unwrap();
        assert_eq!(events.deltas(), ["start "]);
    });
}

#[test]
fn cancelling_ends_the_stream_with_an_error_event_for_the_reason_cancelled() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("a "), line("b ")])).await;
        let (host, _script, shell) = setup(&stub).await;
        let events = Events::default();
        let emit = events.emitter();
        let task = {
            let (host, shell) = (Arc::clone(&host), Arc::clone(&shell));
            tokio::spawn(async move {
                generate(
                    host,
                    shell,
                    "stop-me".into(),
                    writing(AiAction::Continue, "x"),
                    &emit,
                )
                .await
            })
        };
        let seen = events.clone();
        assert!(eventually(Duration::from_secs(3), move || seen.deltas().len() == 2).await);
        cancel(&shell, "stop-me");
        // The command answers Ok; the outcome is the last event.
        task.await.unwrap().unwrap();
        match events.last() {
            AiStreamEvent::Error { error } => {
                assert_eq!(reason(&error), "cancelled");
                assert_eq!(error.code, ErrorCode::InvalidInput);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(events.deltas(), ["a ", "b "]);
        assert!(shell.streams().is_empty());
        let state = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(3), move || state
                .chat_dropped
                .load(Ordering::SeqCst)
                == 1)
            .await
        );
        // An id that is not running is fine.
        cancel(&shell, "nobody");
        cancel(&shell, "");
    });
}

#[test]
fn at_most_four_streams_run_and_a_request_id_is_used_once() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("x")])).await;
        let (host, _script, shell) = setup(&stub).await;
        let mut tasks = Vec::new();
        let mut all = Vec::new();
        for index in 0..4 {
            let events = Events::default();
            all.push(events.clone());
            let emit = events.emitter();
            let (host, shell) = (Arc::clone(&host), Arc::clone(&shell));
            tasks.push(tokio::spawn(async move {
                generate(
                    host,
                    shell,
                    format!("s{index}"),
                    writing(AiAction::Continue, "x"),
                    &emit,
                )
                .await
            }));
        }
        let state = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(3), move || state
                .chat_started
                .load(Ordering::SeqCst)
                == 4)
            .await
        );
        assert_eq!(shell.streams().len(), MAX_STREAMS);

        // A fifth is refused without an event, for the field requestId, reason busy.
        let (outcome, events) =
            generate_events(&host, &shell, "s4", writing(AiAction::Continue, "x")).await;
        let error = outcome.unwrap_err();
        assert_eq!(
            (field(&error), reason(&error)),
            ("requestId".into(), "busy".into())
        );
        assert!(events.all().is_empty());
        // The same id twice is refused too, even below the limit.
        let (outcome, _) =
            generate_events(&host, &shell, "s0", writing(AiAction::Continue, "x")).await;
        assert_eq!(field(&outcome.unwrap_err()), "requestId");

        // A lock (all streams cancelled) ends them all, each with an error event.
        shell.cancel_all();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        for events in all {
            match events.last() {
                AiStreamEvent::Error { error } => assert_eq!(reason(&error), "cancelled"),
                other => panic!("{other:?}"),
            }
        }
        assert!(shell.streams().is_empty());
        // Room again.
        let stub2 = start_stub(streamed(&["fine"])).await;
        ai_on(
            &_script,
            &stub2.url(),
            Some("chat-model"),
            Some("embed-model"),
        );
        let (outcome, _) =
            generate_events(&host, &shell, "s4", writing(AiAction::Continue, "x")).await;
        outcome.unwrap();
    });
}

#[test]
fn a_request_id_is_one_to_64_letters_digits_underscores_and_hyphens() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, _script, shell) = setup(&stub).await;
        for good in ["a", "A-b_9", &"x".repeat(64)] {
            let (outcome, _) =
                generate_events(&host, &shell, good, writing(AiAction::Continue, "x")).await;
            outcome.unwrap_or_else(|error| panic!("{good}: {error}"));
        }
        for bad in ["", "a b", "a/b", "é", "a.b", &"x".repeat(65), "a\n"] {
            let (outcome, events) =
                generate_events(&host, &shell, bad, writing(AiAction::Continue, "x")).await;
            let error = outcome.unwrap_err();
            assert_eq!(
                (error.code, field(&error)),
                (ErrorCode::InvalidInput, "requestId".into()),
                "{bad:?}"
            );
            assert!(events.all().is_empty());
        }
    });
}

#[test]
fn no_chat_model_chosen_is_a_rejection_for_the_field_model_with_the_reason_missing() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (_state, host, script) = ready_host();
        ai_on(&script, &stub.url(), None, Some("embed-model"));
        let (outcome, events) =
            generate_events(&host, &shell(), "m1", writing(AiAction::Continue, "x")).await;
        let error = outcome.unwrap_err();
        assert_eq!(
            (field(&error), reason(&error)),
            ("model".into(), "missing".into())
        );
        assert!(events.all().is_empty());
        // A blank name is no name.
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "chatModel": "  " }),
        );
        let (outcome, _) =
            generate_events(&host, &shell(), "m2", writing(AiAction::Continue, "x")).await;
        assert_eq!(reason(&outcome.unwrap_err()), "missing");
    });
}

#[test]
fn failures_after_the_stream_started_are_events_and_the_command_still_answers_ok() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, script, shell) = setup(&stub).await;

        // The model is not installed: its show is a 404.
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "chatModel": "ghost" }),
        );
        let (outcome, events) =
            generate_events(&host, &shell, "e1", writing(AiAction::Improve, "x")).await;
        outcome.unwrap();
        match events.last() {
            AiStreamEvent::Error { error } => {
                assert_eq!(reason(&error), "model");
                assert_eq!(error.details.as_ref().unwrap()["model"], "ghost");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(events.all().len(), 1);

        // The text is too long for a writing action: the pure crate's limit, as an event.
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "chatModel": "chat-model" }),
        );
        let long = "x".repeat(32_001);
        let (outcome, events) =
            generate_events(&host, &shell, "e2", writing(AiAction::Improve, &long)).await;
        outcome.unwrap();
        match events.last() {
            AiStreamEvent::Error { error } => {
                assert_eq!(
                    (error.code, field(&error)),
                    (ErrorCode::InvalidInput, "text".into())
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(stub.state.received("/api/chat").is_empty());

        // Ollama stops answering.
        let down = closed_port();
        ai_on(
            &script,
            &format!("http://127.0.0.1:{down}"),
            Some("chat-model"),
            None,
        );
        let (outcome, events) =
            generate_events(&host, &shell, "e3", writing(AiAction::Improve, "x")).await;
        outcome.unwrap();
        match events.last() {
            AiStreamEvent::Error { error } => assert_eq!(reason(&error), "unreachable"),
            other => panic!("{other:?}"),
        }
        assert!(shell.streams().is_empty());
    });
}

#[test]
fn an_error_inside_the_stream_is_an_error_event_after_the_deltas_that_came_before() {
    run(async {
        let stub = start_stub(ChatBehaviour::Pieces(
            vec![line("par"), b"{\"error\":\"out of memory\"}\n".to_vec()],
            Duration::ZERO,
        ))
        .await;
        let (host, _script, shell) = setup(&stub).await;
        let (outcome, events) =
            generate_events(&host, &shell, "i1", writing(AiAction::Improve, "x")).await;
        outcome.unwrap();
        assert_eq!(events.deltas(), ["par"]);
        match events.last() {
            AiStreamEvent::Error { error } => {
                assert_eq!(reason(&error), "remote");
                assert_eq!(error.message, "out of memory");
            }
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn a_channel_that_cannot_be_reached_ends_the_stream() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("a "), line("b ")])).await;
        let (host, _script, shell) = setup(&stub).await;
        let calls = Arc::new(Mutex::new(0_usize));
        let counter = Arc::clone(&calls);
        // The webview went away: every send fails.
        let emit = move |_: AiStreamEvent| {
            *counter.lock().unwrap() += 1;
            false
        };
        generate(
            Arc::clone(&host),
            Arc::clone(&shell),
            "gone".into(),
            writing(AiAction::Continue, "x"),
            &emit,
        )
        .await
        .unwrap();
        // One delta was attempted, then the stream was dropped (and the final event tried).
        assert_eq!(*calls.lock().unwrap(), 2);
        assert!(shell.streams().is_empty());
        let state = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(3), move || state
                .chat_dropped
                .load(Ordering::SeqCst)
                == 1)
            .await
        );
    });
}

#[test]
fn a_short_meeting_is_summarised_in_one_streamed_call() {
    run(async {
        let stub = start_stub(streamed(&["### Summary\n", "Short."])).await;
        let (host, _script, shell) = setup(&stub).await;
        let (outcome, events) = generate_events(
            &host,
            &shell,
            "mt1",
            writing(AiAction::MeetingSummary, "**[00:00]** We met."),
        )
        .await;
        outcome.unwrap();
        assert_eq!(stub.state.received("/api/chat").len(), 1);
        assert_eq!(events.deltas(), ["### Summary\n", "Short."]);
    });
}

#[test]
fn a_long_meeting_is_summarised_in_parts_and_merged_with_a_streamed_custom_call() {
    run(async {
        let stub = start_stub(streamed(&["part ", "summary"])).await;
        let (host, _script, shell) = setup(&stub).await;
        // Five paragraphs of 10,000 characters: 50,000 in all, so three parts of at most 24,000.
        let paragraph = |letter: char| letter.to_string().repeat(10_000);
        let transcript = ['a', 'b', 'c', 'd', 'e']
            .iter()
            .map(|letter| paragraph(*letter))
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(summary_parts(&transcript).len(), 3);
        // The transcript is longer than any writing input may be: it is cut before that limit applies.
        assert!(transcript.chars().count() > MAX_TEXT_CHARS);
        let (outcome, events) = generate_events(
            &host,
            &shell,
            "mt2",
            writing(AiAction::MeetingSummary, &transcript),
        )
        .await;
        outcome.unwrap();
        let chats = stub.state.received("/api/chat");
        assert_eq!(chats.len(), 4, "three parts and the merge");
        // The parts are summarised with the meeting instruction, the merge is a custom call.
        for part in &chats[..3] {
            let user = part.body["messages"][1]["content"].as_str().unwrap();
            assert!(
                user.contains("Summary") && user.contains("Key points"),
                "{user}"
            );
        }
        let merge = chats[3].body["messages"][1]["content"].as_str().unwrap();
        assert!(merge.contains(MERGE_SUMMARIES_PROMPT), "{merge}");
        assert!(
            merge.contains("### Part 1") && merge.contains("### Part 3"),
            "{merge}"
        );
        // Progress is an empty delta after each part; only the merge streams its text.
        assert_eq!(events.deltas(), ["", "", "", "part ", "summary"]);
        assert_eq!(
            events.last(),
            AiStreamEvent::Done {
                text: "part summary".into(),
                model: "chat-model".into()
            }
        );
    });
}

#[test]
fn a_transcript_beyond_the_limit_is_an_error_event() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, _script, shell) = setup(&stub).await;
        let huge = "word ".repeat(60_000);
        assert!(huge.chars().count() > MAX_MEETING_CHARS);
        let (outcome, events) = generate_events(
            &host,
            &shell,
            "mt3",
            writing(AiAction::MeetingSummary, &huge),
        )
        .await;
        outcome.unwrap();
        match events.last() {
            AiStreamEvent::Error { error } => assert_eq!(field(&error), "text"),
            other => panic!("{other:?}"),
        }
        assert!(stub.state.received("/api/chat").is_empty());
    });
}

#[test]
fn the_summaries_that_are_merged_fit_a_writing_input() {
    let summaries: Vec<String> = (0..12).map(|_| "s".repeat(10_000)).collect();
    let merged = join_summaries(&summaries);
    assert!(
        merged.chars().count() <= MAX_TEXT_CHARS,
        "{}",
        merged.chars().count()
    );
    assert!(merged.starts_with("### Part 1\n"));
    assert!(merged.contains("### Part 12\n"));
    let few = join_summaries(&["one".into(), "two".into()]);
    assert_eq!(few, "### Part 1\none\n\n### Part 2\ntwo");
}

// --- the chat ---

#[test]
fn a_chat_sends_the_sources_first_then_streams_the_answer_from_them() {
    run(async {
        let stub = start_stub(streamed(&["It is ", "so [1]."])).await;
        let (host, script, shell) = setup(&stub).await;
        script.lock().unwrap().passages = vec![
            passage(1, 1, "The launch is on Friday."),
            passage(2, 2, "Budget approved."),
        ];
        let events = Events::default();
        let emit = events.emitter();
        let space = SpaceId::from_bytes([3; 16]);
        chat(
            Arc::clone(&host),
            Arc::clone(&shell),
            "chat-1".into(),
            question("When is the launch?", AiScope::Space { space_id: space }),
            &emit,
        )
        .await
        .unwrap();
        let all = events.all();
        match &all[0] {
            AiStreamEvent::Sources { sources } => {
                assert_eq!(sources.len(), 2);
                assert_eq!(sources[0].n, 1);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(events.deltas(), ["It is ", "so [1]."]);
        assert_eq!(
            events.last(),
            AiStreamEvent::Done {
                text: "It is so [1].".into(),
                model: "chat-model".into()
            }
        );
        // The core was asked for eight passages, in the scope, with the question's embedding.
        let retrieve = requests_of(&script, "AiRetrieve");
        assert_eq!(retrieve.len(), 1);
        match &retrieve[0] {
            Request::AiRetrieve {
                query,
                vector,
                scope,
                limit,
            } => {
                assert_eq!(query, "When is the launch?");
                assert_eq!(vector.as_ref().map(Vec::len), Some(16));
                assert_eq!(*scope, AiScope::Space { space_id: space });
                assert_eq!(*limit, Some(8));
            }
            other => panic!("{other:?}"),
        }
        // The question was embedded as a query, without a document prefix, by the embedding model.
        let embedded = &stub.state.received("/api/embed")[0].body;
        assert_eq!(embedded["model"], "embed-model");
        assert_eq!(embedded["input"][0], "When is the launch?");
        // The prompt carries the numbered sources.
        let prompt = &stub.state.received("/api/chat")[0].body["messages"][0]["content"];
        let prompt = prompt.as_str().unwrap();
        assert!(
            prompt.contains("[1] Page 1\nThe launch is on Friday."),
            "{prompt}"
        );
        assert!(prompt.contains("[2] Page 2\nBudget approved."), "{prompt}");
    });
}

#[test]
fn a_follow_up_embeds_the_previous_question_with_it() {
    run(async {
        let stub = start_stub(streamed(&["ok"])).await;
        let (host, script, shell) = setup(&stub).await;
        let input = AiChatInput {
            messages: vec![
                AiChatMessage {
                    role: AiRole::User,
                    content: "What is Atlas?".into(),
                },
                AiChatMessage {
                    role: AiRole::Assistant,
                    content: "A project.".into(),
                },
                AiChatMessage {
                    role: AiRole::User,
                    content: "Who leads it?".into(),
                },
            ],
            scope: AiScope::All,
        };
        let events = Events::default();
        let emit = events.emitter();
        chat(Arc::clone(&host), shell, "chat-2".into(), input, &emit)
            .await
            .unwrap();
        match &requests_of(&script, "AiRetrieve")[0] {
            Request::AiRetrieve { query, .. } => {
                assert!(
                    query.contains("What is Atlas?") && query.contains("Who leads it?"),
                    "{query}"
                );
            }
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn when_the_question_cannot_be_embedded_the_chat_goes_on_with_full_text_alone() {
    run(async {
        let stub = start_stub(streamed(&["answer"])).await;
        let (host, script, shell) = setup(&stub).await;
        *stub.state.embed_status.lock().unwrap() = 500;
        script.lock().unwrap().passages = vec![passage(1, 1, "text")];
        let events = Events::default();
        let emit = events.emitter();
        chat(
            Arc::clone(&host),
            Arc::clone(&shell),
            "c3".into(),
            question("q?", AiScope::All),
            &emit,
        )
        .await
        .unwrap();
        match &requests_of(&script, "AiRetrieve")[0] {
            Request::AiRetrieve { vector, .. } => assert!(vector.is_none()),
            other => panic!("{other:?}"),
        }
        assert!(matches!(events.last(), AiStreamEvent::Done { .. }));

        // The semantic index switched off: no embedding is asked for at all.
        let before = stub.state.received("/api/embed").len();
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "chatModel": "chat-model", "embeddingModel": "embed-model", "semanticIndex": false }),
        );
        let events = Events::default();
        let emit = events.emitter();
        chat(
            Arc::clone(&host),
            Arc::clone(&shell),
            "c4".into(),
            question("q?", AiScope::All),
            &emit,
        )
        .await
        .unwrap();
        assert_eq!(stub.state.received("/api/embed").len(), before);
        match &requests_of(&script, "AiRetrieve")[1] {
            Request::AiRetrieve { vector, .. } => assert!(vector.is_none()),
            other => panic!("{other:?}"),
        }

        // And with no embedding model chosen.
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "chatModel": "chat-model" }),
        );
        let events = Events::default();
        let emit = events.emitter();
        chat(
            host,
            shell,
            "c5".into(),
            question("q?", AiScope::All),
            &emit,
        )
        .await
        .unwrap();
        assert_eq!(stub.state.received("/api/embed").len(), before);
    });
}

#[test]
fn a_short_page_in_scope_page_is_the_one_source_and_nothing_is_retrieved() {
    run(async {
        let stub = start_stub(streamed(&["the answer"])).await;
        let (host, script, shell) = setup(&stub).await;
        let page = PageId::from_bytes([5; 16]);
        script
            .lock()
            .unwrap()
            .markdown
            .insert(page, "# Notes\nBuy milk.".into());
        script
            .lock()
            .unwrap()
            .pages
            .insert(page, page_info(page, "Shopping"));
        let events = Events::default();
        let emit = events.emitter();
        chat(
            Arc::clone(&host),
            Arc::clone(&shell),
            "p1".into(),
            question("what do I buy?", AiScope::Page { page_id: page }),
            &emit,
        )
        .await
        .unwrap();
        match &events.all()[0] {
            AiStreamEvent::Sources { sources } => {
                assert_eq!(sources.len(), 1);
                assert_eq!((sources[0].n, sources[0].page_id), (1, page));
                assert_eq!(sources[0].title, "Shopping");
                assert_eq!(sources[0].text, "# Notes\nBuy milk.");
            }
            other => panic!("{other:?}"),
        }
        assert!(requests_of(&script, "AiRetrieve").is_empty());
        assert!(stub.state.received("/api/embed").is_empty());
        let prompt = stub.state.received("/api/chat")[0].body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            prompt.contains("[1] Shopping\n# Notes\nBuy milk."),
            "{prompt}"
        );

        // A page of 12,000 characters or more is retrieved from instead.
        script
            .lock()
            .unwrap()
            .markdown
            .insert(page, "x".repeat(12_000));
        let events = Events::default();
        let emit = events.emitter();
        chat(
            host,
            shell,
            "p2".into(),
            question("what?", AiScope::Page { page_id: page }),
            &emit,
        )
        .await
        .unwrap();
        match &requests_of(&script, "AiRetrieve")[0] {
            Request::AiRetrieve { scope, .. } => {
                assert_eq!(*scope, AiScope::Page { page_id: page })
            }
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn an_empty_question_is_an_error_event_and_nothing_is_asked_of_ollama() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, _script, shell) = setup(&stub).await;
        let events = Events::default();
        let emit = events.emitter();
        chat(
            host,
            shell,
            "q0".into(),
            question("   ", AiScope::All),
            &emit,
        )
        .await
        .unwrap();
        match events.last() {
            AiStreamEvent::Error { error } => assert_eq!(error.code, ErrorCode::InvalidInput),
            other => panic!("{other:?}"),
        }
        assert!(stub.state.received("/api/chat").is_empty());
    });
}

// --- the search ---

#[test]
fn a_search_embeds_the_query_and_asks_the_core_with_the_vector() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, script, shell) = setup(&stub).await;
        script.lock().unwrap().passages = vec![passage(1, 1, "found")];
        let scope = AiScope::All;
        let found = search(
            Arc::clone(&host),
            Arc::clone(&shell),
            "  launch plan  ".into(),
            scope,
            Some(6),
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 1);
        match &requests_of(&script, "AiRetrieve")[0] {
            Request::AiRetrieve {
                query,
                vector,
                scope,
                limit,
            } => {
                assert_eq!(query, "launch plan");
                assert_eq!(vector.as_ref().map(Vec::len), Some(16));
                assert_eq!((*scope, *limit), (AiScope::All, Some(6)));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            stub.state.received("/api/embed")[0].body["input"][0],
            "launch plan"
        );
    });
}

#[test]
fn a_search_falls_back_to_full_text_when_embedding_fails_or_the_index_is_off() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, script, shell) = setup(&stub).await;
        *stub.state.embed_status.lock().unwrap() = 500;
        search(
            Arc::clone(&host),
            Arc::clone(&shell),
            "q".into(),
            AiScope::All,
            None,
        )
        .await
        .unwrap();
        match &requests_of(&script, "AiRetrieve")[0] {
            Request::AiRetrieve { vector, limit, .. } => {
                assert!(vector.is_none() && limit.is_none())
            }
            other => panic!("{other:?}"),
        }
        // Ollama gone altogether: still an answer (the core's), not an error.
        let down = closed_port();
        ai_on(
            &script,
            &format!("http://127.0.0.1:{down}"),
            Some("chat-model"),
            Some("embed-model"),
        );
        search(
            Arc::clone(&host),
            Arc::clone(&shell),
            "q".into(),
            AiScope::All,
            None,
        )
        .await
        .unwrap();
        // The index off: no embedding at all.
        let before = stub.state.received("/api/embed").len();
        ai_on(
            &script,
            &stub.url(),
            Some("chat-model"),
            Some("embed-model"),
        );
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "embeddingModel": "embed-model", "semanticIndex": false }),
        );
        search(host, shell, "q".into(), AiScope::All, None)
            .await
            .unwrap();
        assert_eq!(stub.state.received("/api/embed").len(), before);
    });
}

#[test]
fn a_query_is_not_blank_and_at_most_2000_characters() {
    run(async {
        let stub = start_stub(streamed(&["x"])).await;
        let (host, script, shell) = setup(&stub).await;
        let error = search(
            Arc::clone(&host),
            Arc::clone(&shell),
            "  \n ".into(),
            AiScope::All,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(
            (error.code, field(&error)),
            (ErrorCode::InvalidInput, "query".into())
        );
        let long = "é".repeat(2_500);
        search(host, shell, long, AiScope::All, None).await.unwrap();
        match &requests_of(&script, "AiRetrieve")[0] {
            Request::AiRetrieve { query, .. } => assert_eq!(query.chars().count(), 2_000),
            other => panic!("{other:?}"),
        }
    });
}

// --- transcription ---

fn asr_answer(text: &str) -> ChatBehaviour {
    ChatBehaviour::Json(
        json!({ "model": "asr-model", "message": { "role": "assistant", "content": text }, "done": true }).to_string(),
    )
}

#[test]
fn a_wav_segment_is_sent_as_audio_and_the_answer_is_cleaned() {
    run(async {
        let stub = start_stub(asr_answer(
            "language English<asr_text>Hello there, everyone.",
        ))
        .await;
        let (host, _script, shell) = setup(&stub).await;
        let wav = wav_bytes(16_000);
        let answer = transcribe(Arc::clone(&host), Arc::clone(&shell), wav.clone(), None)
            .await
            .unwrap();
        assert_eq!(answer.text, "Hello there, everyone.");
        assert_eq!(answer.language.as_deref(), Some("English"));
        let sent = &stub.state.received("/api/chat")[0].body;
        // The configured transcription model, not streamed, the audio in `images`.
        assert_eq!(sent["model"], "asr-model");
        assert_eq!(sent["stream"], false);
        assert_eq!(sent["messages"][0]["images"].as_array().unwrap().len(), 1);

        // The header's model wins.
        let stub = start_stub(asr_answer("Bonjour")).await;
        let (host, _script, shell) = setup(&stub).await;
        let answer = transcribe(host, shell, wav, Some("gemma4:12b".into()))
            .await
            .unwrap();
        assert_eq!(answer.text, "Bonjour");
        assert_eq!(
            stub.state.received("/api/chat")[0].body["model"],
            "gemma4:12b"
        );
    });
}

#[test]
fn silence_is_an_empty_text() {
    run(async {
        let stub = start_stub(asr_answer("[silence]")).await;
        let (host, _script, shell) = setup(&stub).await;
        let answer = transcribe(host, shell, wav_bytes(100), None).await.unwrap();
        assert_eq!(answer.text, "");
    });
}

#[test]
fn audio_that_is_not_a_small_pcm_wav_is_refused_for_the_field_audio() {
    run(async {
        let stub = start_stub(asr_answer("x")).await;
        let (host, _script, shell) = setup(&stub).await;
        for bad in [
            vec![],
            b"not audio at all".to_vec(),
            b"RIFF\0\0\0\0WAVE".to_vec(),
        ] {
            let error = transcribe(Arc::clone(&host), Arc::clone(&shell), bad, None)
                .await
                .unwrap_err();
            assert_eq!(
                (error.code, field(&error)),
                (ErrorCode::InvalidInput, "audio".into())
            );
        }
        let mut huge = wav_bytes(10);
        huge.resize(MAX_WAV_BYTES + 1, 0);
        let error = transcribe(Arc::clone(&host), Arc::clone(&shell), huge, None)
            .await
            .unwrap_err();
        assert_eq!(field(&error), "audio");
        assert_eq!(error.details.as_ref().unwrap()["limit"], MAX_WAV_BYTES);
        // Exactly 4 MiB passes.
        let mut exact = wav_bytes(10);
        exact.resize(MAX_WAV_BYTES, 0);
        transcribe(host, shell, exact, None).await.unwrap();
        assert!(check_audio_size(&vec![0; MAX_WAV_BYTES + 1]).is_err());
    });
}

#[test]
fn no_transcription_model_is_the_field_model_with_the_reason_missing() {
    run(async {
        let stub = start_stub(asr_answer("x")).await;
        let (_state, host, script) = ready_host();
        put_setting(
            &script,
            AI_ENDPOINT_SETTING,
            json!({ "url": stub.url(), "remoteConsent": false }),
        );
        put_setting(
            &script,
            AI_SETTINGS_SETTING,
            json!({ "enabled": true, "chatModel": "chat-model" }),
        );
        let error = transcribe(host, shell(), wav_bytes(10), None)
            .await
            .unwrap_err();
        assert_eq!(
            (field(&error), reason(&error)),
            ("model".into(), "missing".into())
        );
        assert!(stub.state.requests.lock().unwrap().is_empty());
    });
}

#[test]
fn the_x_kb_ai_header_is_json_with_an_optional_model() {
    use tauri::http::HeaderValue;
    let headers = |value: Option<&str>| {
        let mut map = HeaderMap::new();
        if let Some(value) = value {
            map.insert(AI_HEADER, HeaderValue::from_str(value).unwrap());
        }
        map
    };
    assert_eq!(model_from_header(&headers(None)).unwrap(), None);
    assert_eq!(model_from_header(&headers(Some("{}"))).unwrap(), None);
    assert_eq!(
        model_from_header(&headers(Some(r#"{"model":"r2t2"}"#)))
            .unwrap()
            .as_deref(),
        Some("r2t2")
    );
    assert_eq!(
        model_from_header(&headers(Some(r#"{"model":"  gemma4:12b "}"#)))
            .unwrap()
            .as_deref(),
        Some("gemma4:12b")
    );
    assert_eq!(
        model_from_header(&headers(Some(r#"{"model":""}"#))).unwrap(),
        None
    );
    // The escapes of a client that keeps its header ASCII are read.
    assert_eq!(
        model_from_header(&headers(Some(&format!(
            r#"{{"model":"caf{}u00e9"}}"#,
            char::from(92_u8)
        ))))
        .unwrap()
        .as_deref(),
        Some("café")
    );
    for bad in ["not json", "[]", r#"{"model":3}"#, r#"{"model":"a\nb"}"#] {
        let error = model_from_header(&headers(Some(bad))).unwrap_err();
        assert_eq!(
            (error.code, field(&error)),
            (ErrorCode::InvalidInput, AI_HEADER.into()),
            "{bad}"
        );
    }
    let long = format!(r#"{{"model":"{}"}}"#, "m".repeat(201));
    assert!(model_from_header(&headers(Some(&long))).is_err());
}

// --- logging ---

#[test]
fn the_logs_never_hold_text_prompts_answers_or_the_endpoint() {
    let filter = crate::log_filter(tracing_subscriber::EnvFilter::new("debug"));
    let logs = captured_logs_through(filter, || {
        run(async {
            let stub = start_stub(streamed(&["SECRET-ANSWER"])).await;
            let (host, script, shell) = setup(&stub).await;
            script.lock().unwrap().passages = vec![passage(1, 1, "SECRET-PASSAGE")];
            let (outcome, _) = generate_events(
                &host,
                &shell,
                "l1",
                writing(AiAction::Improve, "SECRET-TEXT"),
            )
            .await;
            outcome.unwrap();
            let events = Events::default();
            let emit = events.emitter();
            chat(
                Arc::clone(&host),
                Arc::clone(&shell),
                "l2".into(),
                question("SECRET-QUESTION?", AiScope::All),
                &emit,
            )
            .await
            .unwrap();
            search(
                Arc::clone(&host),
                Arc::clone(&shell),
                "SECRET-QUERY".into(),
                AiScope::All,
                None,
            )
            .await
            .unwrap();
            let stub = start_stub(asr_answer("SECRET-SPEECH")).await;
            ai_on(
                &script,
                &stub.url(),
                Some("chat-model"),
                Some("embed-model"),
            );
            transcribe(Arc::clone(&host), Arc::clone(&shell), wav_bytes(100), None)
                .await
                .unwrap();
            // A failure too.
            let down = closed_port();
            ai_on(
                &script,
                &format!("http://127.0.0.1:{down}"),
                Some("chat-model"),
                None,
            );
            let _ = generate_events(
                &host,
                &shell,
                "l3",
                writing(AiAction::Improve, "SECRET-TEXT"),
            )
            .await;
            // And an endpoint with a remote host.
            let confirm = FakeConfirm::saying(&[true]);
            store_endpoint(&*host, &*confirm, "https://secret-host.example:11434").unwrap();
        });
    });
    assert!(logs.contains("AI request finished"), "{logs}");
    assert!(logs.contains("AI request failed"), "{logs}");
    for secret in [
        "SECRET-ANSWER",
        "SECRET-PASSAGE",
        "SECRET-TEXT",
        "SECRET-QUESTION",
        "SECRET-QUERY",
        "SECRET-SPEECH",
        "secret-host",
        "127.0.0.1",
    ] {
        assert!(!logs.contains(secret), "{secret} leaked: {logs}");
    }
}

// --- a live Ollama (run by hand: `cargo test -p atomik-kb-desktop --lib live_ -- --ignored --nocapture`) ---

/// A ready core with AI on against the Ollama of this machine and the models the developer has.
fn live() -> (Arc<dyn CoreHost>, Arc<AiShell>) {
    let (_state, host, script) = ready_host();
    put_setting(
        &script,
        AI_SETTINGS_SETTING,
        json!({
            "enabled": true,
            "chatModel": "qwen3:4b",
            "embeddingModel": "nomic-embed-text",
            "transcriptionModel": "r2t2",
        }),
    );
    (host, Arc::new(AiShell::new()))
}

#[test]
#[ignore = "needs the Ollama of this machine on 127.0.0.1:11434"]
fn live_status_lists_the_installed_models_with_their_capabilities() {
    run(async {
        let (host, shell) = live();
        let status = status(host, shell).await.unwrap();
        assert!(status.reachable, "{:?}", status.error);
        println!("version {:?}", status.version);
        for model in &status.models {
            println!("{} {:?}", model.name, model.capabilities);
        }
        assert!(
            status
                .models
                .iter()
                .any(|model| model.name.starts_with("qwen3"))
        );
    });
}

#[test]
#[ignore = "needs the Ollama of this machine on 127.0.0.1:11434"]
fn live_a_writing_action_streams_and_ends_with_the_cleaned_answer() {
    run(async {
        let (host, shell) = live();
        let (outcome, events) = generate_events(
            &host,
            &shell,
            "live-1",
            writing(
                AiAction::FixGrammar,
                "Ths is a tset of the wrting asistant.",
            ),
        )
        .await;
        outcome.unwrap();
        println!("{:?}", events.deltas().len());
        match events.last() {
            AiStreamEvent::Done { text, model } => {
                println!("{model}: {text}");
                assert!(!text.is_empty() && !text.contains("<think>"));
            }
            other => panic!("{other:?}"),
        }
    });
}

#[test]
#[ignore = "needs the Ollama of this machine on 127.0.0.1:11434"]
fn live_a_query_and_a_chunk_embed_with_their_prefixes() {
    run(async {
        let (host, shell) = live();
        let ctx = prepare(&host).await.unwrap();
        let vector = embed_query(&shell.client, &ctx, "what is the launch date?", None)
            .await
            .unwrap();
        println!("dimensions {}", vector.len());
        assert!(vector.len() >= 16);
    });
}
