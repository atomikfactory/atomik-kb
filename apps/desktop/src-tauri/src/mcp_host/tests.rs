//! Tests of the MCP host: the HTTP checks against a real listener on 127.0.0.1 with a scripted core,
//! the listener's life with the settings and the lock, and the grants confirmation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;

use kb_types::ai::McpSettings;
use kb_types::{ErrorCode, SpaceId};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::ai_test_support::{
    FakeConfirm, SharedScript, client, closed_port, eventually, put_setting, ready_host,
    requests_of, space,
};
use crate::state::{AppState, all_of};
use crate::test_support::captured_logs_through;

const TOKEN: &str = "kbm_TESTTOKEN0123456789abcdefghijklmnopqrstuv";
const MESSAGE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
const REPLY: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;

/// Runs a future on a runtime of its own.
fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// A listener on any port, serving a scripted core.
async fn serve() -> (
    Running,
    Arc<dyn CoreHost>,
    SharedScript,
    Arc<AppState<crate::ai_test_support::AiFake>>,
) {
    let (state, host, script) = ready_host();
    script.lock().unwrap().mcp_reply = Some(Ok(Some(REPLY.to_owned())));
    let running = start(Arc::downgrade(&host), 0).await.unwrap();
    (running, host, script, state)
}

fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// A well-formed request.
fn post(client: &reqwest::Client, port: u16) -> reqwest::RequestBuilder {
    client
        .post(url(port))
        .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(CONTENT_TYPE, "application/json")
        .body(MESSAGE)
}

// --- the checks ---

#[test]
fn a_well_formed_request_reaches_the_core_and_its_answer_is_returned() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let response = post(&http(), running.port).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(response.text().await.unwrap(), REPLY);
        match &requests_of(&script, "McpCall")[0] {
            Request::McpCall { token, message } => {
                assert_eq!(token, TOKEN);
                assert_eq!(message, MESSAGE);
            }
            other => panic!("{other:?}"),
        }
        // The content type may carry parameters, and the bearer scheme any case.
        let response = http()
            .post(url(running.port))
            .header(AUTHORIZATION, format!("bEaReR {TOKEN}"))
            .header(CONTENT_TYPE, "Application/JSON; charset=utf-8")
            .body(MESSAGE)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    });
}

#[test]
fn a_notification_is_202_with_no_body() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        script.lock().unwrap().mcp_reply = Some(Ok(None));
        let response = post(&http(), running.port).send().await.unwrap();
        assert_eq!(response.status(), 202);
        assert!(response.bytes().await.unwrap().is_empty());
    });
}

#[test]
fn the_host_header_must_be_127_0_0_1_or_localhost_with_the_port() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let port = running.port;
        for (host, status) in [
            (format!("127.0.0.1:{port}"), 200),
            (format!("localhost:{port}"), 200),
            (format!("LOCALHOST:{port}"), 200),
            ("evil.example".to_owned(), 403),
            (format!("evil.example:{port}"), 403),
            ("127.0.0.1".to_owned(), 403),
            ("localhost".to_owned(), 403),
            (format!("127.0.0.1:{}", port.wrapping_add(1)), 403),
            (format!("localhost.evil.example:{port}"), 403),
            (format!("127.0.0.1.evil.example:{port}"), 403),
            (format!("[::1]:{port}"), 403),
            (format!("192.168.1.2:{port}"), 403),
        ] {
            let response = post(&http(), port)
                .header(HOST, host.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{host}");
        }
        // The 403s never reached the core.
        assert_eq!(requests_of(&script, "McpCall").len(), 3);
    });
}

#[test]
fn a_request_without_a_host_header_is_refused() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", running.port))
            .await
            .unwrap();
        let body = MESSAGE;
        let request = format!(
            "POST /mcp HTTP/1.0\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(
            answer.starts_with("HTTP/1.0 403") || answer.starts_with("HTTP/1.1 403"),
            "{answer}"
        );
        assert!(requests_of(&script, "McpCall").is_empty());
    });
}

#[test]
fn the_origin_header_when_there_is_one_must_be_http_loopback_with_any_port() {
    run(async {
        let (running, _host, _script, _state) = serve().await;
        for (origin, status) in [
            ("http://localhost", 200),
            ("http://localhost:3000", 200),
            ("http://127.0.0.1", 200),
            ("http://127.0.0.1:51234", 200),
            ("HTTP://LOCALHOST:1", 403),
            ("https://localhost", 403),
            ("https://127.0.0.1:443", 403),
            ("http://evil.example", 403),
            ("http://localhost.evil.example", 403),
            ("http://127.0.0.1.evil.example:80", 403),
            ("http://localhost:abc", 403),
            ("http://localhost:", 403),
            ("http://localhost/path", 403),
            ("http://user@localhost", 403),
            ("null", 403),
            ("file://", 403),
            ("", 403),
        ] {
            let response = post(&http(), running.port)
                .header(ORIGIN, origin)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{origin:?}");
        }
        // No Origin at all is fine (the bridge and the desktop clients send none).
        assert_eq!(
            post(&http(), running.port).send().await.unwrap().status(),
            200
        );
    });
}

#[test]
fn only_post_to_mcp_is_served() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let client = http();
        for method in ["GET", "DELETE", "PUT", "PATCH", "OPTIONS"] {
            let response = client
                .request(method.parse().unwrap(), url(running.port))
                .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 405, "{method}");
            assert_eq!(response.headers()["allow"], "POST", "{method}");
        }
        for path in ["/", "/mcp/", "/mcp/x", "/MCP", "/sse", "/message"] {
            let wrong = client
                .post(format!("http://127.0.0.1:{}{path}", running.port))
                .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE)
                .send()
                .await
                .unwrap();
            assert_eq!(wrong.status(), 404, "{path}");
        }
        assert!(requests_of(&script, "McpCall").is_empty());
    });
}

#[test]
fn a_missing_or_unusable_bearer_token_is_401_with_www_authenticate_and_never_reaches_the_core() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let client = http();
        let requests = [
            client
                .post(url(running.port))
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE),
            client
                .post(url(running.port))
                .header(AUTHORIZATION, "Basic dXNlcjpwYXNz")
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE),
            client
                .post(url(running.port))
                .header(AUTHORIZATION, "Bearer")
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE),
            client
                .post(url(running.port))
                .header(AUTHORIZATION, "Bearer ")
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE),
            client
                .post(url(running.port))
                .header(AUTHORIZATION, "Bearer two words")
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE),
            client
                .post(url(running.port))
                .header(AUTHORIZATION, format!("Bearer {}", "k".repeat(300)))
                .header(CONTENT_TYPE, "application/json")
                .body(MESSAGE),
        ];
        for request in requests {
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), 401);
            assert_eq!(response.headers()["www-authenticate"], "Bearer");
        }
        assert!(requests_of(&script, "McpCall").is_empty());
    });
}

#[test]
fn a_token_the_core_does_not_know_is_401_too() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        script.lock().unwrap().mcp_reply = Some(Err(ApiError::invalid("token", "unknown client")));
        let response = post(&http(), running.port).send().await.unwrap();
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()["www-authenticate"], "Bearer");
        // The refusal does not say more than that.
        assert!(!response.text().await.unwrap().contains("unknown"));
    });
}

#[test]
fn the_body_must_be_json_and_at_most_a_mebibyte() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let client = http();
        for content_type in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "application/jsonx",
            "text/json",
        ] {
            let response = client
                .post(url(running.port))
                .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(CONTENT_TYPE, content_type)
                .body(MESSAGE)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 415, "{content_type}");
        }
        let none = client
            .post(url(running.port))
            .header(AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(MESSAGE)
            .send()
            .await
            .unwrap();
        assert_eq!(none.status(), 415);
        assert!(requests_of(&script, "McpCall").is_empty());

        // Exactly 1 MiB passes, one byte more is 413.
        let exactly = "x".repeat(MAX_BODY_BYTES);
        let response = post(&client, running.port)
            .body(exactly)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let over = "x".repeat(MAX_BODY_BYTES + 1);
        // The server answers 413 from the length and closes; under load the client can still be
        // sending and see the connection cut instead of the answer. Either is a refusal; the
        // count below proves the body never reached the core.
        match post(&client, running.port).body(over).send().await {
            Ok(response) => assert_eq!(response.status(), 413),
            Err(error) => assert!(error.is_request() || error.is_body(), "{error}"),
        }
        assert_eq!(requests_of(&script, "McpCall").len(), 1);
    });
}

#[test]
fn a_body_over_a_mebibyte_with_no_content_length_is_413_too() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", running.port))
            .await
            .unwrap();
        let head = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            running.port
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..17 {
            let framed = [
                format!("{:x}\r\n", chunk.len()).into_bytes(),
                chunk.clone(),
                b"\r\n".to_vec(),
            ]
            .concat();
            if stream.write_all(&framed).await.is_err() {
                break;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer).await;
        let answer = String::from_utf8_lossy(&answer);
        // The server answers 413 and closes while the body is still coming; on Windows the reset
        // that follows can discard the answer before it is read, leaving nothing. Either is a
        // refusal (never a 200), and the core was never asked.
        assert!(
            answer.is_empty() || answer.starts_with("HTTP/1.1 413"),
            "{answer}"
        );
        assert!(requests_of(&script, "McpCall").is_empty());
    });
}

#[test]
fn a_body_that_is_not_utf8_is_400() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        let response = post(&http(), running.port)
            .body(vec![0xFF_u8, 0xFE, b'{'])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert!(requests_of(&script, "McpCall").is_empty());
    });
}

#[test]
fn what_the_core_cannot_do_is_503_or_500_and_says_nothing_about_why() {
    run(async {
        let (running, _host, script, _state) = serve().await;
        for (error, status) in [
            (ApiError::locked(), 503),
            (ApiError::internal("core is starting"), 503),
            (
                ApiError::internal("disk on fire at C:\\Users\\ada\\notes"),
                500,
            ),
            (ApiError::storage("SECRET sqlite error"), 500),
            // The core's own limit on a message is the same 1 MiB.
            (ApiError::invalid("message", "too large"), 413),
            (ApiError::invalid("other", "bad"), 500),
        ] {
            script.lock().unwrap().mcp_reply = Some(Err(error.clone()));
            let response = post(&http(), running.port).send().await.unwrap();
            assert_eq!(response.status(), status, "{error:?}");
            let text = response.text().await.unwrap();
            assert!(!text.contains("ada") && !text.contains("SECRET"), "{text}");
        }
    });
}

#[test]
fn a_listener_that_was_stopped_refuses_what_a_connection_still_holds() {
    run(async {
        let (running, _host, _script, _state) = serve().await;
        let client = http();
        let port = running.port;
        assert_eq!(post(&client, port).send().await.unwrap().status(), 200);
        // The same client (a kept-alive connection) after the listener stopped.
        running.stop().await;
        // (A connection that was closed or refused is as good.)
        if let Ok(response) = post(&client, port).send().await {
            assert_eq!(response.status(), 503);
        }
        // And the port is free.
        assert!(
            tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .is_ok()
        );
    });
}

#[test]
fn neither_the_token_nor_the_message_nor_the_reply_is_logged() {
    let filter = crate::log_filter(tracing_subscriber::EnvFilter::new("debug"));
    let logs = captured_logs_through(filter, || {
        run(async {
            let (running, _host, script, _state) = serve().await;
            script.lock().unwrap().mcp_reply =
                Some(Ok(Some(r#"{"result":"SECRET-REPLY"}"#.into())));
            let response = http()
                .post(url(running.port))
                .header(AUTHORIZATION, "Bearer kbm_SECRET-TOKEN")
                .header(CONTENT_TYPE, "application/json")
                .body(r#"{"method":"SECRET-MESSAGE"}"#)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            script.lock().unwrap().mcp_reply = Some(Err(ApiError::invalid("token", "no")));
            let response = http()
                .post(url(running.port))
                .header(AUTHORIZATION, "Bearer kbm_SECRET-TOKEN")
                .header(CONTENT_TYPE, "application/json")
                .body(r#"{"method":"SECRET-MESSAGE"}"#)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 401);
        });
    });
    assert!(logs.contains("mcp request"), "{logs}");
    for secret in ["SECRET-TOKEN", "SECRET-MESSAGE", "SECRET-REPLY", "kbm_"] {
        assert!(!logs.contains(secret), "{secret} leaked: {logs}");
    }
}

// --- the pure checks ---

#[test]
fn host_and_origin_checks_as_functions() {
    assert!(host_allowed("127.0.0.1:47811", 47811));
    assert!(host_allowed("Localhost:47811", 47811));
    assert!(!host_allowed("127.0.0.1:47812", 47811));
    assert!(!host_allowed("127.0.0.1:47811 ", 47811));
    assert!(!host_allowed("", 47811));
    assert!(origin_allowed("http://localhost"));
    assert!(origin_allowed("http://127.0.0.1:1"));
    assert!(!origin_allowed("http://127.0.0.1:123456"));
    assert!(!origin_allowed("http://localhost:80:80"));
}

#[test]
fn the_url_and_the_bridge_path() {
    assert_eq!(url_of(47_811), "http://127.0.0.1:47811/mcp");
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(bridge_in(dir.path()), None);
    std::fs::write(dir.path().join(BRIDGE_NAME), b"MZ").unwrap();
    let found = bridge_in(dir.path()).unwrap();
    assert!(found.ends_with(BRIDGE_NAME), "{found}");
    assert_eq!(BRIDGE_NAME.ends_with(".exe"), cfg!(windows));
    // A directory with that name is not the bridge.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join(BRIDGE_NAME)).unwrap();
    assert_eq!(bridge_in(other.path()), None);
}

// --- the listener's life ---

fn mcp_settings(host: &Arc<dyn CoreHost>, enabled: bool, port: u16) {
    host.dispatch(Request::SettingsSet {
        key: MCP_SETTINGS_SETTING.into(),
        value: serde_json::to_value(McpSettings { enabled, port }).unwrap(),
    })
    .unwrap();
}

fn listening(shell: &Arc<McpShell>) -> bool {
    shell.listening().running
}

async fn answers(port: u16) -> bool {
    match post(&http(), port).send().await {
        Ok(response) => response.status() == 200,
        Err(_) => false,
    }
}

async fn closed(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err()
}

fn start_host() -> (
    Arc<AppState<crate::ai_test_support::AiFake>>,
    Arc<dyn CoreHost>,
    SharedScript,
    Arc<McpShell>,
) {
    let (state, host, script) = ready_host();
    script.lock().unwrap().mcp_reply = Some(Ok(Some(REPLY.to_owned())));
    let shell = Arc::new(McpShell::new());
    state.set_background(all_of(vec![mcp_starter_with(
        Arc::downgrade(&host),
        Arc::clone(&shell),
        Duration::from_millis(100),
    )]));
    (state, host, script, shell)
}

#[test]
fn the_listener_follows_the_setting_binds_loopback_only_and_rebinds_on_a_new_port() {
    run(async {
        let (state, host, _script, shell) = start_host();
        // Off by default: nothing listens.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(shell.listening(), Listening::default());

        let first = closed_port();
        mcp_settings(&host, true, first);
        let watched = Arc::clone(&shell);
        assert!(eventually(Duration::from_secs(5), move || listening(&watched)).await);
        assert_eq!(shell.listening().port, Some(first));
        assert!(answers(first).await);

        // Only the loopback address: the same port on the machine's other addresses is not served.
        // (`0.0.0.0` would answer on 127.0.0.1 too, so what is checked is the bound address.)
        let bound = std::net::TcpListener::bind(("127.0.0.1", first));
        assert!(bound.is_err(), "the port is taken on 127.0.0.1");

        // A new port: the old one is closed, the new one answers.
        let second = closed_port();
        mcp_settings(&host, true, second);
        let watched = Arc::clone(&shell);
        assert!(
            eventually(Duration::from_secs(5), move || watched.listening().port
                == Some(second))
            .await
        );
        assert!(answers(second).await);
        assert!(closed(first).await);

        // Turned off: closed, and nothing is reported.
        mcp_settings(&host, false, second);
        let watched = Arc::clone(&shell);
        assert!(eventually(Duration::from_secs(5), move || !listening(&watched)).await);
        assert!(closed(second).await);
        assert_eq!(shell.listening(), Listening::default());
        drop(state);
    });
}

#[test]
fn a_port_in_use_is_reported_and_tried_again() {
    run(async {
        let (_state, host, _script, shell) = start_host();
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        mcp_settings(&host, true, port);
        let watched = Arc::clone(&shell);
        assert!(
            eventually(Duration::from_secs(5), move || watched
                .listening()
                .error
                .is_some())
            .await
        );
        let state = shell.listening();
        assert!(!state.running);
        let error = state.error.unwrap();
        assert!(
            !error.contains("127.0.0.1") && !error.contains(&port.to_string()),
            "{error}"
        );

        // The port is freed: the next try (every 100 ms here) binds it.
        drop(taken);
        let watched = Arc::clone(&shell);
        assert!(eventually(Duration::from_secs(5), move || listening(&watched)).await);
        assert!(shell.listening().error.is_none());
        assert!(answers(port).await);
    });
}

#[test]
fn a_port_below_1024_is_refused_with_a_message_and_nothing_listens() {
    run(async {
        let (_state, host, _script, shell) = start_host();
        mcp_settings(&host, true, 80);
        let watched = Arc::clone(&shell);
        assert!(
            eventually(Duration::from_secs(5), move || watched
                .listening()
                .error
                .is_some())
            .await
        );
        assert!(!shell.listening().running);
        assert!(shell.listening().error.unwrap().contains("1024"));
    });
}

#[test]
fn locking_closes_the_listener_and_unlocking_opens_it_again() {
    run(async {
        let (state, host, _script, shell) = start_host();
        let port = closed_port();
        mcp_settings(&host, true, port);
        let watched = Arc::clone(&shell);
        assert!(eventually(Duration::from_secs(5), move || listening(&watched)).await);
        assert!(answers(port).await);
        assert_eq!(state.listener_count(), 1);

        host.lock().unwrap();
        assert_eq!(state.listener_count(), 0);
        assert_eq!(shell.listening(), Listening::default());
        let wait = Duration::from_secs(5);
        assert!(
            eventually(wait, || std::net::TcpStream::connect(("127.0.0.1", port))
                .is_err())
            .await
        );

        host.unlock("passphrase").unwrap();
        let watched = Arc::clone(&shell);
        assert!(eventually(Duration::from_secs(5), move || listening(&watched)).await);
        assert!(answers(port).await);
    });
}

#[test]
fn the_status_combines_the_setting_and_the_listener() {
    run(async {
        let (_state, host, script, shell) = start_host();
        // Nothing stored: off, the default port.
        let off = status(Arc::clone(&host), Arc::clone(&shell)).await.unwrap();
        assert!(!off.enabled && !off.running);
        assert_eq!(off.port, kb_types::ai::DEFAULT_MCP_PORT);
        assert_eq!(off.url, "http://127.0.0.1:47811/mcp");
        assert!(off.error.is_none());

        let port = closed_port();
        mcp_settings(&host, true, port);
        let watched = Arc::clone(&shell);
        assert!(eventually(Duration::from_secs(5), move || listening(&watched)).await);
        let on = status(Arc::clone(&host), Arc::clone(&shell)).await.unwrap();
        assert!(on.enabled && on.running && on.error.is_none());
        assert_eq!((on.port, on.url.as_str()), (port, url(port).as_str()));

        // A stored value that is not settings is off.
        put_setting(&script, MCP_SETTINGS_SETTING, json!("junk"));
        let junk = status(Arc::clone(&host), Arc::clone(&shell)).await.unwrap();
        assert!(!junk.enabled && !junk.running);

        // Locked: the setting is not readable, so neither is the status.
        host.lock().unwrap();
        let error = status(host, shell).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Locked);
    });
}

// --- the grants confirmation ---

fn grant(space: u8, read: bool, write: bool) -> McpGrant {
    McpGrant {
        space_id: SpaceId::from_bytes([space; 16]),
        read,
        write,
    }
}

#[test]
fn what_a_new_set_of_grants_adds() {
    let ids = |list: &[u8]| -> Vec<SpaceId> {
        list.iter()
            .map(|id| SpaceId::from_bytes([*id; 16]))
            .collect()
    };
    // Nothing before: reading adds read, writing adds write (and not read as well).
    assert_eq!(
        added_access(&[], &[grant(1, true, false)]),
        (ids(&[1]), vec![])
    );
    assert_eq!(
        added_access(&[], &[grant(1, false, true)]),
        (vec![], ids(&[1]))
    );
    assert_eq!(
        added_access(&[], &[grant(1, true, true), grant(2, true, false)]),
        (ids(&[2]), ids(&[1]))
    );
    // Write implies read: a grant of write alone from a client that reads adds only the write.
    assert_eq!(
        added_access(&[grant(1, true, false)], &[grant(1, true, true)]),
        (vec![], ids(&[1]))
    );
    // Nothing is added when nothing grows: the same, less, none, another order.
    assert_eq!(
        added_access(&[grant(1, true, true)], &[grant(1, true, true)]),
        (vec![], vec![])
    );
    assert_eq!(
        added_access(&[grant(1, true, true)], &[grant(1, true, false)]),
        (vec![], vec![])
    );
    assert_eq!(
        added_access(&[grant(1, true, true), grant(2, true, false)], &[]),
        (vec![], vec![])
    );
    assert_eq!(
        added_access(&[grant(1, true, false)], &[grant(1, false, false)]),
        (vec![], vec![])
    );
    assert_eq!(
        added_access(&[grant(1, false, true)], &[grant(1, true, false)]),
        (vec![], vec![])
    );
    // A space listed twice counts once, with both entries' access together.
    assert_eq!(
        added_access(&[], &[grant(1, true, false), grant(1, false, true)]),
        (vec![], ids(&[1]))
    );
    // A grant with neither flag adds nothing.
    assert_eq!(
        added_access(&[], &[grant(1, false, false)]),
        (vec![], vec![])
    );
}

#[test]
fn the_question_names_the_client_and_the_spaces() {
    let names =
        |list: &[&str]| -> Vec<String> { list.iter().map(|name| (*name).to_owned()).collect() };
    let english = Labels::default();
    let q = grants_question(&english, "Claude Desktop", &names(&["Work"]), &[]);
    assert_eq!(q.title, "Allow access to your pages?");
    assert_eq!(q.message, "Allow Claude Desktop to read Work?");
    assert_eq!((q.accept.as_str(), q.cancel.as_str()), ("Allow", "Cancel"));
    let q = grants_question(&english, "Claude", &[], &names(&["Journal"]));
    assert_eq!(q.message, "Allow Claude to change Journal?");
    let q = grants_question(
        &english,
        "Claude",
        &names(&["Work", "Home"]),
        &names(&["Journal"]),
    );
    assert_eq!(
        q.message,
        "Allow Claude to read Work and Home and change Journal?"
    );
    let q = grants_question(
        &english,
        "Claude",
        &names(&["A", "B", "C", "D", "E", "F", "G"]),
        &[],
    );
    assert_eq!(q.message, "Allow Claude to read A, B, C, D, E and 2 more?");
    // What the person typed is shown as text: controls dropped, long names cut, blank is "Untitled".
    // (The command cleans the names of the spaces with `shown` before it asks; the client's name is
    // cleaned by the question itself.)
    let cleaned = [
        shown(&english, &format!("{}\u{0}", "N".repeat(100))),
        shown(&english, "   "),
    ];
    let q = grants_question(&english, "Cla\u{7}ude\n", &cleaned, &[]);
    assert_eq!(
        q.message,
        format!("Allow Claude to read {} and Untitled?", "N".repeat(60))
    );
}

#[test]
fn the_question_is_written_in_the_words_the_ui_sent() {
    // A translation changes the words and the order of the parts, and the question still names the client and
    // the spaces: the placeholders are the shell's, filled in one pass.
    let french = Labels {
        dialog_mcp_title: "Autoriser l'acc\u{e8}s \u{e0} vos pages ?".to_owned(),
        dialog_mcp_read: "Autoriser {client} \u{e0} lire {spaces} ?".to_owned(),
        dialog_mcp_write: "Autoriser {client} \u{e0} modifier {spaces} ?".to_owned(),
        dialog_mcp_both: "Autoriser {client} \u{e0} lire {read} et \u{e0} modifier {write} ?"
            .to_owned(),
        dialog_list_and: "{names} et {last}".to_owned(),
        dialog_list_more: "{count} autres".to_owned(),
        dialog_untitled: "Sans titre".to_owned(),
        dialog_allow: "Autoriser".to_owned(),
        dialog_cancel: "Annuler".to_owned(),
        ..Labels::default()
    };
    let names =
        |list: &[&str]| -> Vec<String> { list.iter().map(|name| (*name).to_owned()).collect() };
    let q = grants_question(&french, "Claude", &names(&["Travail", "Maison"]), &[]);
    assert_eq!(q.title, "Autoriser l'acc\u{e8}s \u{e0} vos pages ?");
    assert_eq!(
        q.message,
        "Autoriser Claude \u{e0} lire Travail et Maison ?"
    );
    assert_eq!(
        (q.accept.as_str(), q.cancel.as_str()),
        ("Autoriser", "Annuler")
    );
    let q = grants_question(
        &french,
        "Claude",
        &names(&["A", "B", "C", "D", "E", "F", "G"]),
        &names(&["Journal"]),
    );
    assert_eq!(
        q.message,
        "Autoriser Claude \u{e0} lire A, B, C, D, E et 2 autres et \u{e0} modifier Journal ?"
    );
    // A blank name is called what the UI calls it, and a name that looks like a placeholder is just a name.
    let tricky = [shown(&french, "  "), shown(&french, "{client}")];
    let q = grants_question(&french, "{spaces}", &tricky, &[]);
    assert_eq!(
        q.message,
        "Autoriser {spaces} \u{e0} lire Sans titre et {client} ?"
    );
}

fn host_with_clients() -> (Arc<dyn CoreHost>, SharedScript) {
    let (_state, host, script) = ready_host();
    {
        let mut core = script.lock().unwrap();
        core.spaces = vec![space(1, "Work"), space(2, "Journal")];
        core.clients = vec![client("c1", "Claude", vec![grant(1, true, false)])];
    }
    (host, script)
}

fn grants_of(script: &SharedScript, id: &str) -> Vec<McpGrant> {
    script
        .lock()
        .unwrap()
        .clients
        .iter()
        .find(|c| c.id == id)
        .unwrap()
        .grants
        .clone()
}

#[test]
fn adding_access_asks_first_and_applies_it_only_when_allowed() {
    run(async {
        let (host, script) = host_with_clients();
        // Allowed: read of Journal and write of Work are both added.
        let confirm = FakeConfirm::saying(&[true]);
        let info = grants_set(
            Arc::clone(&host),
            confirm.clone(),
            "c1".into(),
            vec![grant(1, true, true), grant(2, true, false)],
        )
        .await
        .unwrap();
        assert_eq!(
            info.grants,
            vec![grant(1, true, true), grant(2, true, false)]
        );
        let asked = confirm.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(
            asked[0].message,
            "Allow Claude to read Journal and change Work?"
        );
        assert_eq!(grants_of(&script, "c1"), info.grants);

        // Declined: invalid input for the grants, reason declined, and the core is not asked.
        let confirm = FakeConfirm::saying(&[false]);
        let before = grants_of(&script, "c1");
        let calls = requests_of(&script, "McpClientGrantsSet").len();
        let error = grants_set(
            Arc::clone(&host),
            confirm.clone(),
            "c1".into(),
            vec![grant(1, true, true), grant(2, true, true)],
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        let details = error.details.unwrap();
        assert_eq!(
            (details["field"].as_str(), details["reason"].as_str()),
            (Some("grants"), Some("declined"))
        );
        assert_eq!(grants_of(&script, "c1"), before);
        assert_eq!(requests_of(&script, "McpClientGrantsSet").len(), calls);
        assert_eq!(
            confirm.asked.lock().unwrap()[0].message,
            "Allow Claude to change Journal?"
        );
    });
}

#[test]
fn removing_or_keeping_access_asks_nothing() {
    run(async {
        let (host, script) = host_with_clients();
        let confirm = FakeConfirm::saying(&[]);
        // Less access, the same, none.
        for grants in [
            vec![grant(1, true, false)],
            vec![],
            vec![grant(1, false, false)],
        ] {
            grants_set(
                Arc::clone(&host),
                confirm.clone(),
                "c1".into(),
                grants.clone(),
            )
            .await
            .unwrap();
            assert_eq!(grants_of(&script, "c1"), grants);
        }
        assert_eq!(confirm.count(), 0);
        // And from a client with write access, a reduction to read.
        script.lock().unwrap().clients[0].grants = vec![grant(1, true, true)];
        grants_set(
            Arc::clone(&host),
            confirm.clone(),
            "c1".into(),
            vec![grant(1, true, false)],
        )
        .await
        .unwrap();
        assert_eq!(confirm.count(), 0);
    });
}

#[test]
fn an_unknown_client_is_the_cores_answer_and_a_space_that_does_not_exist_asks_nothing() {
    run(async {
        let (host, _script) = host_with_clients();
        let confirm = FakeConfirm::saying(&[true]);
        let error = grants_set(
            Arc::clone(&host),
            confirm.clone(),
            "nobody".into(),
            vec![grant(1, true, true)],
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(confirm.count(), 0);
        // Space 9 is not there: the core ignores a grant of it, so there is nothing to confirm.
        grants_set(
            host,
            confirm.clone(),
            "c1".into(),
            vec![grant(1, true, false), grant(9, true, true)],
        )
        .await
        .unwrap();
        assert_eq!(confirm.count(), 0);
    });
}

#[test]
fn no_dialog_while_the_workspace_is_locked() {
    run(async {
        let (host, _script) = host_with_clients();
        host.lock().unwrap();
        let confirm = FakeConfirm::saying(&[true]);
        let error = grants_set(
            host,
            confirm.clone(),
            "c1".into(),
            vec![grant(2, true, true)],
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Locked);
        assert_eq!(confirm.count(), 0);
    });
}

// --- the stdio bridge ---

/// A request shaped exactly as `kb-mcp-bridge` sends it: HTTP/1.1, `Connection: close`, `Accept` for
/// both kinds of answer, no `Origin`.
#[test]
fn a_request_in_the_shape_the_bridge_sends_is_answered_and_the_connection_closed() {
    run(async {
        let (running, _host, _script, _state) = serve().await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", running.port))
            .await
            .unwrap();
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{MESSAGE}",
            running.port,
            MESSAGE.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(
            answer
                .to_lowercase()
                .contains("content-type: application/json")
        );
        assert!(answer.ends_with(REPLY), "{answer}");
    });
}

/// By hand: `cargo build -p kb-mcp --bin kb-mcp-bridge`, then
/// `cargo test -p atomik-kb-desktop --lib live_ -- --ignored`.
#[test]
#[ignore = "needs target/debug/kb-mcp-bridge built"]
fn live_the_real_bridge_talks_to_the_listener() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().unwrap();
    let bridge = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(BRIDGE_NAME);
    assert!(bridge.is_file(), "{} is not built", bridge.display());
    run(async {
        let (running, _host, script, _state) = serve().await;
        let port = running.port;
        let talk = |token: &'static str| {
            let bridge = bridge.clone();
            tokio::task::spawn_blocking(move || {
                let mut child = Command::new(bridge)
                    .args(["--port", &port.to_string()])
                    .env("ATOMIK_KB_MCP_TOKEN", token)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap();
                let mut stdin = child.stdin.take().unwrap();
                writeln!(stdin, "{MESSAGE}").unwrap();
                // A notification: no answer. Then the call that is answered.
                writeln!(
                    stdin,
                    r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
                )
                .unwrap();
                drop(stdin);
                let lines: Vec<String> = BufReader::new(child.stdout.take().unwrap())
                    .lines()
                    .map(Result::unwrap)
                    .collect();
                child.wait().unwrap();
                lines
            })
        };
        // The core answers the first message and has nothing for the notification.
        script
            .lock()
            .unwrap()
            .answers
            .push(Box::new(|request| match request {
                Request::McpCall { message, .. } if message.contains("tools/list") => {
                    Some(Ok(Response::McpReply(kb_types::ai::McpReply {
                        body: Some(REPLY.to_owned()),
                    })))
                }
                Request::McpCall { .. } => Some(Ok(Response::McpReply(kb_types::ai::McpReply {
                    body: None,
                }))),
                _ => None,
            }));
        let lines = talk("kbm_right").await.unwrap();
        assert_eq!(lines, [REPLY]);
        // A token the core refuses becomes a JSON-RPC error for the message that had an id.
        script.lock().unwrap().answers.clear();
        script.lock().unwrap().mcp_reply = Some(Err(ApiError::invalid("token", "unknown")));
        let lines = talk("kbm_wrong").await.unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("-32001"), "{}", lines[0]);
        // The listener stopped: the bridge says the app is not there.
        running.stop().await;
        let lines = talk("kbm_right").await.unwrap();
        assert!(
            lines.iter().any(|line| line.contains("-32002")),
            "{lines:?}"
        );
    });
}
