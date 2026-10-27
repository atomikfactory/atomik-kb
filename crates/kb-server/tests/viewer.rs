//! The viewer's files (M13 slice 4, ADR-0051, `kb-server/src/viewer.rs`): the policy and the other headers on
//! every answer of `/view`, content types, a plain 404 for everything that is not a file below the directory
//! (no listing, no traversal by any spelling), and the server without a viewer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::Path;

use kb_types::sharing::VIEWER_CSP;
use reqwest::StatusCode;
use support::{TestServer, config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SECRET: &str = "the secret outside the viewer";

/// A viewer build in `<dir>/dist` and a file next to it that must never be served.
fn build(dir: &Path) -> std::path::PathBuf {
    let dist = dir.join("dist");
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(
        dist.join("index.html"),
        "<!doctype html><title>viewer</title>",
    )
    .unwrap();
    std::fs::write(dist.join("assets/index-abc123.js"), "export default 1;").unwrap();
    std::fs::write(
        dist.join("assets/kb_wasm_bg-def456.wasm"),
        b"\0asm\x01\0\0\0",
    )
    .unwrap();
    std::fs::write(dist.join("assets/index-abc123.css"), "body{margin:0}").unwrap();
    std::fs::write(
        dist.join("favicon.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    std::fs::write(dir.join("secret.txt"), SECRET).unwrap();
    std::fs::write(dist.join(".hidden"), SECRET).unwrap();
    dist
}

async fn start(dist: Option<std::path::PathBuf>) -> (tempfile::TempDir, TestServer) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let mut config = config(&data);
    config.viewer_dir = dist;
    let server = TestServer::start(&config).await;
    (dir, server)
}

async fn started() -> (tempfile::TempDir, TestServer) {
    let dir = tempfile::tempdir().unwrap();
    let dist = build(dir.path());
    let data = dir.path().join("data");
    let mut config = config(&data);
    config.viewer_dir = Some(dist);
    let server = TestServer::start(&config).await;
    (dir, server)
}

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Every answer of `/view`, the 404 too, carries the policy and the other two headers.
fn assert_hardened(response: &reqwest::Response) {
    assert_eq!(header(response, "content-security-policy"), VIEWER_CSP);
    assert_eq!(header(response, "referrer-policy"), "no-referrer");
    assert_eq!(header(response, "x-content-type-options"), "nosniff");
}

/// One request line written as is (a client library would normalise the path): the status line and
/// everything after it, as text.
async fn raw(server: &TestServer, target: &str) -> String {
    let address = server.base.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    String::from_utf8_lossy(&answer).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_viewer_is_served_with_the_policy_and_the_right_content_types() {
    let (_dir, server) = started().await;
    let get = |path: &str| server.http.get(server.url(path)).send();

    for path in ["/view", "/view/", "/view/index.html", "/view?x=1"] {
        let response = get(path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_hardened(&response);
        assert_eq!(
            header(&response, "content-type"),
            "text/html; charset=utf-8"
        );
        assert_eq!(header(&response, "cache-control"), "no-store");
        assert_eq!(
            response.text().await.unwrap(),
            "<!doctype html><title>viewer</title>"
        );
    }
    let cases = [
        (
            "/view/assets/index-abc123.js",
            "text/javascript; charset=utf-8",
        ),
        ("/view/assets/kb_wasm_bg-def456.wasm", "application/wasm"),
        ("/view/assets/index-abc123.css", "text/css; charset=utf-8"),
        ("/view/favicon.svg", "image/svg+xml"),
    ];
    for (path, content_type) in cases {
        let response = get(path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_hardened(&response);
        assert_eq!(header(&response, "content-type"), content_type, "{path}");
        let cache = header(&response, "cache-control");
        if path.contains("/assets/") {
            assert_eq!(cache, "public, max-age=31536000, immutable");
        } else {
            assert_eq!(cache, "no-cache");
        }
    }
    let wasm = get("/view/assets/kb_wasm_bg-def456.wasm").await.unwrap();
    assert_eq!(wasm.bytes().await.unwrap().as_ref(), b"\0asm\x01\0\0\0");

    // Not a file, not there, a dotfile, a directory and a method that is not routed: one 404 for the
    // reads, with the same headers and the same body.
    let mut bodies = Vec::new();
    for path in [
        "/view/missing.js",
        "/view/assets",
        "/view/assets/",
        "/view/.hidden",
        "/view//index.html",
    ] {
        let response = get(path).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_hardened(&response);
        bodies.push(response.text().await.unwrap());
    }
    assert!(bodies.iter().all(|body| body == &bodies[0]), "{bodies:?}");
    assert!(!bodies[0].contains(SECRET));
    let posted = server.http.post(server.url("/view")).send().await.unwrap();
    assert_eq!(posted.status(), StatusCode::METHOD_NOT_ALLOWED);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_spelling_of_a_path_leaves_the_viewer_directory() {
    let (_dir, server) = started().await;
    let attempts = [
        "/view/../secret.txt",
        "/view/%2e%2e/secret.txt",
        "/view/%2E%2E%2Fsecret.txt",
        "/view/..%2fsecret.txt",
        "/view/assets/../../secret.txt",
        "/view/assets/%2e%2e/%2e%2e/secret.txt",
        "/view/%2e%2e%5csecret.txt",
        "/view/..%5csecret.txt",
        "/view/%5c..%5csecret.txt",
        "/view/assets%5c..%5c..%5csecret.txt",
        "/view/C:%5cWindows%5cwin.ini",
        "/view/C:/Windows/win.ini",
        "/view/%00",
        "/view/index.html%00.js",
        "/view/index.html::$DATA",
        "/view/index.html.",
        "/view/index.html%20",
        "/view/./index.html",
        "/view/.%2fsecret.txt",
        "/view//etc/passwd",
        "/view/%252e%252e/secret.txt",
    ];
    for target in attempts {
        let answer = raw(&server, target).await;
        assert!(
            answer.starts_with("HTTP/1.1 404") || answer.starts_with("HTTP/1.1 400"),
            "{target}: {}",
            answer.lines().next().unwrap_or_default()
        );
        assert!(!answer.contains(SECRET), "{target} served the secret");
        assert!(!answer.contains("<title>viewer"), "{target} found a file");
    }
    server.stop().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_symlink_out_of_the_directory_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let dist = build(dir.path());
    std::os::unix::fs::symlink(dir.path().join("secret.txt"), dist.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(dir.path(), dist.join("up")).unwrap();
    let (_other, server) = start(Some(dist)).await;
    for path in ["/view/link.txt", "/view/up/secret.txt"] {
        let response = server.http.get(server.url(path)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert!(!response.text().await.unwrap().contains(SECRET));
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_a_viewer_answers_a_plain_404_and_nothing_else_changes() {
    let empty = tempfile::tempdir().unwrap();
    let missing = empty.path().join("nowhere");
    for dist in [None, Some(missing), Some(empty.path().to_path_buf())] {
        let (_dir, server) = start(dist).await;
        for path in ["/view", "/view/", "/view/index.html", "/view/assets/a.js"] {
            let response = server.http.get(server.url(path)).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_hardened(&response);
            assert_eq!(
                header(&response, "content-type"),
                "text/plain; charset=utf-8"
            );
            assert_eq!(response.text().await.unwrap(), "not found\n");
        }
        // The rest of the server is as before.
        let info = server
            .http
            .get(server.url("/v2/server"))
            .send()
            .await
            .unwrap();
        assert_eq!(info.status(), StatusCode::OK);
        server.stop().await;
    }
}
