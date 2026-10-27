//! Log hygiene of the view-link routes and `/view` (M13 slice 4, `kb-server/src/links.rs`, "Logs"): the log
//! holds the 8 hex characters of a link id and of a chunk hash and nothing more of either, no publication,
//! no body, no token, no `ETag` value and no query string; the key of a link is in a URL fragment and never
//! reaches the server at all. Its own test binary, because it installs the global tracing subscriber.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};

use reqwest::StatusCode;
use support::base64;
use support::links::LinkWorld;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_of_a_link_flow_holds_no_full_ids_publications_tokens_or_queries() {
    let capture = Capture::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish(),
    )
    .unwrap();

    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let (chunk, chunk_bytes) = w.chunk_in(w.feed(), &w.owner_token, 80).await;
    let publication = LinkWorld::publication(&key, "private page text of the link");
    let body = LinkWorld::put_body(w.feed(), None, &[chunk], publication.clone());
    assert_eq!(
        w.put(Some(&w.owner_token), &key, body).await.0,
        StatusCode::OK
    );
    assert_eq!(w.get(&key, None).await.status(), StatusCode::OK);
    assert_eq!(
        w.get(&key, Some("\"1\"")).await.status(),
        StatusCode::NOT_MODIFIED
    );
    assert_eq!(w.chunk(&key, chunk).await.status(), StatusCode::OK);
    assert_eq!(
        w.get(&LinkWorld::key(2), None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        w.delete(Some(&w.viewer_token), &key).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        w.delete(Some(&w.owner_token), &key).await,
        StatusCode::NO_CONTENT
    );
    // The viewer's page, with a query a client might add (the fragment is never sent).
    let page = w
        .server
        .http
        .get(w.url("/view?token=not-for-the-log"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        page.status(),
        StatusCode::NOT_FOUND,
        "no viewer is configured"
    );
    w.server.stop().await;

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("/v2/links/"), "paths are logged:\n{log}");
    assert!(log.contains("link published"));
    assert!(log.contains("link revoked"));
    assert!(log.contains("/view"), "the viewer's path is logged");
    let link_id = key.link_id();
    let secret_key = key.to_fragment();
    let forbidden = vec![
        link_id.to_string(),
        chunk.to_hex(),
        secret_key,
        "private page text of the link".to_owned(),
        base64(&publication),
        base64(&chunk_bytes),
        w.owner_token.clone(),
        w.viewer_token.clone(),
        "not-for-the-log".to_owned(),
    ];
    for needle in &forbidden {
        assert!(!log.contains(needle.as_str()), "the log holds {needle}");
    }
    assert!(
        log.contains(&link_id.short_hex()),
        "the link's 8 hex characters are what is logged"
    );
    assert!(
        log.contains(&chunk.to_hex()[..8]),
        "the chunk's 8 hex characters are what is logged"
    );
    assert!(!log.contains('?'), "no query string reaches the log");
    assert!(
        !log.to_lowercase().contains("etag"),
        "no ETag value or header reaches the log"
    );
}
