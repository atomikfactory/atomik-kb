//! Tests of the Ollama client against a stub on 127.0.0.1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use kb_ai::ollama::{ChatOptions, WireMessage};

use super::*;
use crate::ai_test_support::{
    ChatBehaviour, closed_port, default_tags, done_line, eventually, line, start_stub,
};
use crate::test_support::captured_logs_through;

/// Runs a future on a runtime of its own (the log capture is per thread).
fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

fn quick() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(1),
        plain: Duration::from_secs(5),
        meta: Duration::from_secs(5),
        first_byte: Duration::from_secs(5),
        between_chunks: Duration::from_secs(5),
        show_cache: Duration::from_secs(60),
    }
}

fn chat_request(model: &str, prompt: &str) -> ChatRequest {
    ChatRequest {
        model: model.to_owned(),
        messages: vec![WireMessage::text("user", prompt)],
        stream: true,
        think: None,
        options: ChatOptions::default(),
    }
}

fn details(error: &ApiError, key: &str) -> Option<Value> {
    error.details.as_ref()?.get(key).cloned()
}

/// Streams a chat and collects the pieces the callback saw.
async fn collect(
    client: &AiClient,
    url: &str,
    request: &ChatRequest,
) -> (Result<String, ApiError>, Vec<String>) {
    let (_source, mut token) = cancel_pair();
    let mut seen = Vec::new();
    let outcome = client
        .stream_chat(url, request, &mut token, &mut |piece: &str| {
            seen.push(piece.to_owned());
            true
        })
        .await;
    (outcome, seen)
}

#[test]
fn the_version_and_the_models_come_with_their_capabilities_and_the_shows_are_cached() {
    run(async {
        let stub = start_stub(ChatBehaviour::Json(String::new())).await;
        let client = AiClient::with_timeouts(quick());
        assert_eq!(client.version(&stub.url()).await.unwrap(), "0.35.0");
        let models = client.models(&stub.url()).await.unwrap();
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["chat-model", "embed-model", "asr-model"]);
        // The capabilities are the ones `show` listed, not the ones the name implies.
        assert_eq!(models[0].capabilities, ["completion", "thinking", "tools"]);
        assert_eq!(models[1].capabilities, ["embedding"]);
        assert_eq!(models[2].capabilities, ["completion", "audio"]);
        assert_eq!(stub.state.received("/api/show").len(), 3);

        // The list is read again, the shows are not (cached for a minute).
        client.models(&stub.url()).await.unwrap();
        assert_eq!(stub.state.received("/api/tags").len(), 2);
        assert_eq!(stub.state.received("/api/show").len(), 3);

        // Forgetting them (a new endpoint) asks again.
        client.forget_models();
        client.models(&stub.url()).await.unwrap();
        assert_eq!(stub.state.received("/api/show").len(), 6);

        // The shell says who it is, and does it the same way every time.
        let agents: Vec<String> = stub
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.user_agent.clone())
            .collect();
        assert!(agents.iter().all(|agent| agent == USER_AGENT), "{agents:?}");
        assert!(USER_AGENT.starts_with("atomik-kb/"));
    });
}

#[test]
fn the_shows_are_cached_per_endpoint_and_for_a_limited_time() {
    run(async {
        let first = start_stub(ChatBehaviour::Json(String::new())).await;
        let second = start_stub(ChatBehaviour::Json(String::new())).await;
        let client = AiClient::with_timeouts(Timeouts {
            show_cache: Duration::from_millis(250),
            ..quick()
        });
        client.models(&first.url()).await.unwrap();
        client.models(&second.url()).await.unwrap();
        assert_eq!(first.state.received("/api/show").len(), 3);
        assert_eq!(second.state.received("/api/show").len(), 3);
        // The cache holds one endpoint: going back to the first asks it again.
        client.models(&first.url()).await.unwrap();
        assert_eq!(first.state.received("/api/show").len(), 6);
        // And what is older than the limit is asked again.
        tokio::time::sleep(Duration::from_millis(400)).await;
        client.models(&first.url()).await.unwrap();
        assert_eq!(first.state.received("/api/show").len(), 9);
    });
}

#[test]
fn a_model_whose_show_fails_keeps_the_capabilities_its_name_implies() {
    run(async {
        let stub = start_stub(ChatBehaviour::Json(String::new())).await;
        stub.state.show.lock().unwrap().clear();
        let client = AiClient::with_timeouts(quick());
        let models = client.models(&stub.url()).await.unwrap();
        assert_eq!(models.len(), 3);
        for model in &models {
            let expected = if model.name.contains("embed") {
                "embedding"
            } else {
                "completion"
            };
            assert_eq!(model.capabilities, [expected], "{}", model.name);
        }
    });
}

#[test]
fn the_traits_of_a_model_come_from_its_show() {
    run(async {
        let stub = start_stub(ChatBehaviour::Json(String::new())).await;
        let client = AiClient::with_timeouts(quick());
        let chat = client.traits_of(&stub.url(), "chat-model").await.unwrap();
        assert_eq!(
            chat,
            ModelTraits {
                thinking: true,
                num_ctx_max: 40_960
            }
        );
        let embed = client.traits_of(&stub.url(), "embed-model").await.unwrap();
        assert!(!embed.thinking);
        assert_eq!(embed.num_ctx_max, 2_048);

        // A model Ollama does not have is the `model` reason, with the name.
        let error = client.traits_of(&stub.url(), "ghost").await.unwrap_err();
        assert_eq!(reason_of(&error), "model");
        assert_eq!(details(&error, "model"), Some(json!("ghost")));
        assert_eq!(details(&error, "field"), Some(json!("endpoint")));

        // An Ollama that cannot say gives the defaults.
        stub.state
            .show
            .lock()
            .unwrap()
            .insert("odd".into(), (500, "{}".into()));
        stub.state.show.lock().unwrap().insert(
            "plain".into(),
            (200, "{\"capabilities\":[\"completion\"]}".into()),
        );
        let odd = client.traits_of(&stub.url(), "odd").await.unwrap();
        assert_eq!(odd.num_ctx_max, DEFAULT_NUM_CTX_MAX);
        assert!(!odd.thinking);
        let plain = client.traits_of(&stub.url(), "plain").await.unwrap();
        assert_eq!(plain.num_ctx_max, DEFAULT_NUM_CTX_MAX);
    });
}

#[test]
fn nothing_listening_is_unreachable_and_the_error_does_not_name_the_address() {
    run(async {
        let port = closed_port();
        let url = format!("http://127.0.0.1:{port}");
        let client = AiClient::with_timeouts(quick());
        let error = client.version(&url).await.unwrap_err();
        assert_eq!(error.code, kb_types::ErrorCode::InvalidInput);
        assert_eq!(details(&error, "field"), Some(json!("endpoint")));
        assert_eq!(reason_of(&error), "unreachable");
        let text = format!("{error:?}");
        assert!(
            !text.contains("127.0.0.1") && !text.contains(&port.to_string()),
            "{text}"
        );
        assert_eq!(
            reason_of(&client.models(&url).await.unwrap_err()),
            "unreachable"
        );
        assert_eq!(
            reason_of(
                &client
                    .embed(&url, "m", vec!["x".into()], None)
                    .await
                    .unwrap_err()
            ),
            "unreachable"
        );
        let (outcome, seen) = collect(&client, &url, &chat_request("m", "hi")).await;
        assert_eq!(reason_of(&outcome.unwrap_err()), "unreachable");
        assert!(seen.is_empty());
    });
}

#[test]
fn a_stream_is_read_across_chunks_that_split_lines_and_characters() {
    run(async {
        // "Hé" and "llo wörld" are in two lines; the pieces cut a line, then a 2-byte character.
        let mut all = line("Hé");
        all.extend(line("llo wörld"));
        all.extend(done_line());
        let cut_in_e = all.iter().position(|b| *b == 0xC3).unwrap() + 1;
        let cut_in_o = all.iter().rposition(|b| *b == 0xC3).unwrap() + 1;
        let pieces = vec![
            all[..7].to_vec(),
            all[7..cut_in_e].to_vec(),
            all[cut_in_e..cut_in_o].to_vec(),
            all[cut_in_o..].to_vec(),
        ];
        let stub = start_stub(ChatBehaviour::Pieces(pieces, Duration::from_millis(5))).await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, seen) =
            collect(&client, &stub.url(), &chat_request("chat-model", "hi")).await;
        assert_eq!(outcome.unwrap(), "Héllo wörld");
        assert_eq!(seen, ["Hé", "llo wörld"]);
        // The request was what was asked: streaming, the model, the prompt.
        let sent = &stub.state.received("/api/chat")[0].body;
        assert_eq!(sent["model"], "chat-model");
        assert_eq!(sent["stream"], true);
        assert_eq!(sent["messages"][0]["content"], "hi");
    });
}

#[test]
fn the_last_line_may_have_no_newline() {
    run(async {
        let mut done = done_line();
        done.pop();
        let stub = start_stub(ChatBehaviour::Pieces(
            vec![line("ok"), done],
            Duration::ZERO,
        ))
        .await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, _) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        assert_eq!(outcome.unwrap(), "ok");
    });
}

#[test]
fn an_error_line_ends_the_stream_with_ollamas_text() {
    run(async {
        let stub = start_stub(ChatBehaviour::Pieces(
            vec![line("a"), b"{\"error\":\"the model crashed\"}\n".to_vec()],
            Duration::ZERO,
        ))
        .await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, seen) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        let error = outcome.unwrap_err();
        assert_eq!(reason_of(&error), "remote");
        assert_eq!(error.message, "the model crashed");
        assert_eq!(seen, ["a"], "what came before was delivered");
    });
}

#[test]
fn a_long_error_text_is_cut_to_300_characters() {
    run(async {
        let long = "x".repeat(900);
        let stub = start_stub(ChatBehaviour::Error(
            500,
            json!({ "error": long }).to_string(),
        ))
        .await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, _) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        let error = outcome.unwrap_err();
        assert_eq!(reason_of(&error), "remote");
        assert_eq!(error.message.chars().count(), 300);
        assert_eq!(details(&error, "status"), Some(json!(500)));
    });
}

#[test]
fn a_missing_model_is_the_model_reason_with_its_name() {
    run(async {
        let stub = start_stub(ChatBehaviour::Error(
            404,
            json!({ "error": "model 'qwen9' not found" }).to_string(),
        ))
        .await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, _) = collect(&client, &stub.url(), &chat_request("qwen9", "hi")).await;
        let error = outcome.unwrap_err();
        assert_eq!(reason_of(&error), "model");
        assert_eq!(details(&error, "model"), Some(json!("qwen9")));
        assert_eq!(details(&error, "field"), Some(json!("endpoint")));
        // The same for the other calls that name a model.
        *stub.state.embed_status.lock().unwrap() = 404;
        let embed_error = client
            .embed(&stub.url(), "qwen9", vec!["x".into()], None)
            .await
            .unwrap_err();
        assert_eq!(reason_of(&embed_error), "model");
    });
}

#[test]
fn an_http_error_without_text_is_the_status_reason() {
    run(async {
        let stub = start_stub(ChatBehaviour::Error(502, "<html>bad gateway</html>".into())).await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, _) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        let error = outcome.unwrap_err();
        assert_eq!(reason_of(&error), "status");
        assert_eq!(details(&error, "status"), Some(json!(502)));
        assert!(!error.message.contains("gateway"), "{}", error.message);
    });
}

#[test]
fn a_stream_that_ends_before_its_last_chunk_is_an_error() {
    run(async {
        let stub = start_stub(ChatBehaviour::Pieces(vec![line("half")], Duration::ZERO)).await;
        let client = AiClient::with_timeouts(quick());
        let (outcome, seen) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        assert_eq!(reason_of(&outcome.unwrap_err()), "remote");
        assert_eq!(seen, ["half"]);
    });
}

#[test]
fn cancelling_stops_the_stream_and_closes_the_connection() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("one "), line("two ")])).await;
        let client = AiClient::with_timeouts(quick());
        let (source, mut token) = cancel_pair();
        let mut seen = Vec::new();
        let request = chat_request("m", "hi");
        let outcome = client
            .stream_chat(&stub.url(), &request, &mut token, &mut |piece: &str| {
                seen.push(piece.to_owned());
                if seen.len() == 2 {
                    source.cancel();
                }
                true
            })
            .await;
        let error = outcome.unwrap_err();
        assert_eq!(reason_of(&error), "cancelled");
        assert_eq!(seen, ["one ", "two "]);
        // Ollama sees the client go: that is what makes it stop generating.
        let state = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(3), move || {
                state.chat_dropped.load(Ordering::SeqCst) == 1
            })
            .await
        );
    });
}

#[test]
fn cancelling_before_the_first_byte_gives_up_at_once() {
    run(async {
        let stub = start_stub(ChatBehaviour::SlowStart(
            Duration::from_secs(3),
            vec![done_line()],
        ))
        .await;
        let client = AiClient::with_timeouts(quick());
        let (source, mut token) = cancel_pair();
        let request = chat_request("m", "hi");
        let started = Instant::now();
        let url = stub.url();
        let mut ignore = |_: &str| true;
        let canceller = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            source.cancel();
        };
        let streaming = client.stream_chat(&url, &request, &mut token, &mut ignore);
        let (outcome, ()) = tokio::join!(streaming, canceller);
        assert_eq!(reason_of(&outcome.unwrap_err()), "cancelled");
        assert!(started.elapsed() < Duration::from_secs(2));
    });
}

#[test]
fn a_receiver_that_is_gone_ends_the_stream_as_cancelled() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("a"), line("b")])).await;
        let client = AiClient::with_timeouts(quick());
        let (_source, mut token) = cancel_pair();
        let request = chat_request("m", "hi");
        let outcome = client
            .stream_chat(&stub.url(), &request, &mut token, &mut |_: &str| false)
            .await;
        assert_eq!(reason_of(&outcome.unwrap_err()), "cancelled");
        let state = Arc::clone(&stub.state);
        assert!(
            eventually(Duration::from_secs(3), move || {
                state.chat_dropped.load(Ordering::SeqCst) == 1
            })
            .await
        );
    });
}

#[test]
fn a_server_that_does_not_start_in_time_is_unreachable() {
    run(async {
        let stub = start_stub(ChatBehaviour::SlowStart(
            Duration::from_secs(3),
            vec![done_line()],
        ))
        .await;
        let client = AiClient::with_timeouts(Timeouts {
            first_byte: Duration::from_millis(150),
            ..quick()
        });
        let (outcome, _) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        assert_eq!(reason_of(&outcome.unwrap_err()), "unreachable");
    });
}

#[test]
fn a_server_that_goes_quiet_between_chunks_is_unreachable() {
    run(async {
        let stub = start_stub(ChatBehaviour::Hang(vec![line("a")])).await;
        let client = AiClient::with_timeouts(Timeouts {
            between_chunks: Duration::from_millis(150),
            ..quick()
        });
        let (outcome, seen) = collect(&client, &stub.url(), &chat_request("m", "hi")).await;
        assert_eq!(reason_of(&outcome.unwrap_err()), "unreachable");
        assert_eq!(seen, ["a"]);
    });
}

#[test]
fn embedding_returns_one_vector_per_input_with_truncation_on() {
    run(async {
        let stub = start_stub(ChatBehaviour::Json(String::new())).await;
        let client = AiClient::with_timeouts(quick());
        let vectors = client
            .embed(
                &stub.url(),
                "embed-model",
                vec!["a".into(), "b".into(), "c".into()],
                None,
            )
            .await
            .unwrap();
        assert_eq!(vectors.len(), 3);
        assert_eq!(vectors[0].len(), 16);
        let sent = &stub.state.received("/api/embed")[0].body;
        assert_eq!(sent["model"], "embed-model");
        assert_eq!(sent["truncate"], true);
        assert_eq!(sent["input"], json!(["a", "b", "c"]));

        // A count that is not the count of inputs is not trusted.
        *stub.state.embed_body.lock().unwrap() =
            Some(json!({ "embeddings": [[0.5, 0.5]] }).to_string());
        let error = client
            .embed(
                &stub.url(),
                "embed-model",
                vec!["a".into(), "b".into()],
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(reason_of(&error), "remote");
        // Neither is an answer that is not an embedding.
        *stub.state.embed_body.lock().unwrap() = Some("[]".into());
        assert_eq!(
            reason_of(
                &client
                    .embed(&stub.url(), "embed-model", vec!["a".into()], None)
                    .await
                    .unwrap_err()
            ),
            "remote"
        );
    });
}

#[test]
fn a_plain_chat_returns_the_one_chunk() {
    run(async {
        let stub = start_stub(ChatBehaviour::Json(
            json!({ "model": "m", "message": { "role": "assistant", "content": "spoken words" }, "done": true })
                .to_string(),
        ))
        .await;
        let client = AiClient::with_timeouts(quick());
        let mut request = chat_request("asr-model", "x");
        request.stream = false;
        let chunk = client.chat_once(&stub.url(), &request).await.unwrap();
        assert_eq!(chunk.content, "spoken words");
        assert!(chunk.done);
        assert_eq!(stub.state.received("/api/chat")[0].body["stream"], false);
    });
}

#[test]
fn the_stubs_default_list_parses_with_the_pure_crate() {
    // Guards the stub itself: the other tests rely on its three models.
    let models = kb_ai::ollama::parse_tags(&default_tags()).unwrap();
    assert_eq!(models.len(), 3);
}

#[test]
fn the_log_says_the_outcome_and_never_the_prompt_the_answer_or_the_address() {
    // Through the app's own filter: the HTTP stack's targets stay at `warn` (they log the addresses
    // they connect to), whatever level the log is asked for.
    let filter = crate::log_filter(tracing_subscriber::EnvFilter::new("debug"));
    let logs = captured_logs_through(filter, || {
        run(async {
            let stub = start_stub(ChatBehaviour::Pieces(
                vec![line("SECRET-ANSWER"), done_line()],
                Duration::ZERO,
            ))
            .await;
            let client = AiClient::with_timeouts(quick());
            let url = stub.url();
            let (outcome, _) =
                collect(&client, &url, &chat_request("chat-model", "SECRET-PROMPT")).await;
            assert!(outcome.is_ok());
            client
                .embed(&url, "embed-model", vec!["SECRET-PAGE".into()], None)
                .await
                .unwrap();
            // The failing ones too.
            let down = format!("http://127.0.0.1:{}", closed_port());
            let _ = client.version(&down).await;
            let _ = client
                .embed(&down, "embed-model", vec!["SECRET-PAGE".into()], None)
                .await;
            let _ = collect(&client, &down, &chat_request("m", "SECRET-PROMPT")).await;
        });
    });
    assert!(logs.contains("ollama chat stream"), "{logs}");
    assert!(logs.contains("chat-model"), "{logs}");
    for secret in [
        "SECRET-PROMPT",
        "SECRET-ANSWER",
        "SECRET-PAGE",
        "127.0.0.1",
        "localhost",
    ] {
        assert!(!logs.contains(secret), "{secret} leaked: {logs}");
    }
}

#[test]
fn the_client_is_built_without_a_proxy() {
    // `no_proxy()` is what keeps `HTTP_PROXY` from catching a loopback address; a client that was
    // built has no way to be told otherwise.
    let source = include_str!("../ai_client.rs");
    assert!(source.contains(".no_proxy()"));
    assert!(AiClient::new().http.is_some());
}
