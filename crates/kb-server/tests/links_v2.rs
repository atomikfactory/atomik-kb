//! View links on the server (M13 slice 4, ADR-0051, `kb-server/src/links.rs`): every route with its
//! authorization (a stranger, a Viewer, no session, the creator, an Owner), the allow-list of chunks, the
//! ETag and 304, expiry, revocation and what a removal or a close leaves, the read limit, and the four
//! calls of `kb_net::HttpRemote` against the same server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use kb_crypto::{EnvelopeKind, Nonce24};
use kb_net::HttpRemote;
use kb_sync::SyncError;
use kb_sync::v2::{LinkFetch, LinkPut, PublicationHeader, RemoteStore, encode_publication_header};
use kb_types::sharing::{LINK_EXPIRY_MAX_DAYS, LINK_GETS_PER_MINUTE_PER_IP};
use kb_types::sync::MAX_WIRE_BYTES;
use kb_types::{FeedId, LinkId};
use reqwest::StatusCode;
use serde_json::json;
use support::links::LinkWorld;
use support::{DAY_MS, T0};

fn header_of(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_is_read_without_a_session_and_its_etag_answers_304() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let first = LinkWorld::publication(&key, "first");
    let body = LinkWorld::put_body(w.feed(), None, &[], first.clone());
    let (status, answer) = w.put(Some(&w.owner_token), &key, body).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({ "pubSeq": 1 })));

    let response = w.get(&key, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_of(&response, "content-type"),
        "application/octet-stream"
    );
    assert_eq!(header_of(&response, "etag"), "\"1\"");
    assert_eq!(header_of(&response, "cache-control"), "no-store");
    assert_eq!(response.bytes().await.unwrap().as_ref(), first.as_slice());

    for matching in ["\"1\"", "W/\"1\"", "\"9\", \"1\"", "*"] {
        let response = w.get(&key, Some(matching)).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{matching}");
        assert_eq!(header_of(&response, "etag"), "\"1\"");
        assert!(response.bytes().await.unwrap().is_empty());
    }
    for other in ["\"2\"", "nonsense"] {
        let response = w.get(&key, Some(other)).await;
        assert_eq!(response.status(), StatusCode::OK, "{other}");
    }

    // An Editor refreshes it: the counter moves, the old ETag no longer matches.
    let second = LinkWorld::publication(&key, "second");
    let body = LinkWorld::put_body(w.feed(), None, &[], second.clone());
    let (status, answer) = w.put(Some(&w.editor_token), &key, body).await;
    assert_eq!((status, answer), (StatusCode::OK, json!({ "pubSeq": 2 })));
    let response = w.get(&key, Some("\"1\"")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "etag"), "\"2\"");
    assert_eq!(response.bytes().await.unwrap().as_ref(), second.as_slice());
    assert_eq!(
        w.get(&key, Some("\"2\"")).await.status(),
        StatusCode::NOT_MODIFIED
    );

    // Unknown and malformed ids are the same 404.
    assert_eq!(
        w.get(&LinkWorld::key(2), None).await.status(),
        StatusCode::NOT_FOUND
    );
    let malformed = w
        .server
        .http
        .get(w.url("/v2/links/not-a-link"))
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::NOT_FOUND);
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_put_needs_a_session_a_role_and_a_well_formed_upload() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let body = || LinkWorld::put_body(w.feed(), None, &[], LinkWorld::publication(&key, "page"));
    let code = |answer: &serde_json::Value| answer["error"].as_str().unwrap_or("").to_owned();

    let (status, _) = w.put(None, &key, body()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, answer) = w.put(Some(&w.stranger_token), &key, body()).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::NOT_FOUND, "not_found".into())
    );
    let (status, answer) = w.put(Some(&w.viewer_token), &key, body()).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::FORBIDDEN, "forbidden".into())
    );
    assert_eq!(w.rows(), (0, 0), "nothing was stored");

    // The publication must name the link of the path; garbage and a declaration over the limit are 400.
    let other = LinkWorld::key(2);
    let moved = LinkWorld::put_body(w.feed(), None, &[], LinkWorld::publication(&other, "page"));
    let (status, _) = w.put(Some(&w.owner_token), &key, moved).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = w.put(Some(&w.owner_token), &key, vec![1, 2, 3]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut over = body();
    let count_at = 1 + 16 + 1 + 8;
    over[count_at..count_at + 4].copy_from_slice(&1025u32.to_le_bytes());
    let (status, _) = w.put(Some(&w.owner_token), &key, over).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The expiry: 365 days is the most (an hour of clock skew is forgiven), beyond is 400.
    let far = T0 + i64::from(LINK_EXPIRY_MAX_DAYS) * DAY_MS;
    let (status, _) = w
        .publish(&w.owner_token, &key, "page", &[], Some(far + 2 * 3_600_000))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = w
        .publish(&w.owner_token, &key, "page", &[], Some(far))
        .await;
    assert_eq!(status, StatusCode::OK);

    // A later upload keeps the feed: the same link in another feed is a conflict.
    let personal = FeedId::from_account(w.owner.account);
    let elsewhere = LinkWorld::put_body(personal, None, &[], LinkWorld::publication(&key, "page"));
    let (status, answer) = w.put(Some(&w.owner_token), &key, elsewhere).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::CONFLICT, "conflict".into())
    );

    // A personal feed takes links from its own account only.
    let mine = LinkWorld::key(3);
    let body = LinkWorld::put_body(personal, None, &[], LinkWorld::publication(&mine, "mine"));
    let (status, _) = w.put(Some(&w.owner_token), &mine, body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let theirs = LinkWorld::key(4);
    let body = LinkWorld::put_body(personal, None, &[], LinkWorld::publication(&theirs, "x"));
    let (status, _) = w.put(Some(&w.editor_token), &theirs, body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publication_of_the_wire_limit_is_taken_and_one_byte_more_is_413() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let header = encode_publication_header(&PublicationHeader {
        link_id: key.link_id(),
    });
    let limit = usize::try_from(MAX_WIRE_BYTES).unwrap();
    // The server never opens it: zeros stand in for the ciphertext.
    let envelope = |ciphertext_len: usize| {
        kb_crypto::Envelope {
            kind: EnvelopeKind::Publication,
            header: header.clone(),
            nonce: Nonce24::from_bytes([0; 24]),
            ciphertext: vec![0; ciphertext_len],
            signature: None,
        }
        .encode()
    };
    let overhead = envelope(0).len();
    let exact = envelope(limit - overhead);
    assert_eq!(exact.len(), limit);
    let body = LinkWorld::put_body(w.feed(), None, &[], envelope(limit - overhead + 1));
    let (status, answer) = w.put(Some(&w.owner_token), &key, body).await;
    assert_eq!(
        (status, answer["error"].as_str().unwrap_or("")),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
    assert_eq!(w.rows(), (0, 0));
    // The framing and the declared hashes ride on top of the publication's limit.
    let body = LinkWorld::put_body(w.feed(), None, &[], exact);
    let (status, _) = w.put(Some(&w.owner_token), &key, body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(w.rows(), (1, 0));
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_declared_chunks_the_feed_holds_are_served() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let (declared, declared_bytes) = w.chunk_in(w.feed(), &w.owner_token, 64).await;
    let (undeclared, _) = w.chunk_in(w.feed(), &w.owner_token, 64).await;
    let (personal_chunk, _) = w
        .chunk_in(FeedId::from_account(w.owner.account), &w.owner_token, 64)
        .await;
    let (not_yet, not_yet_bytes) = w.space.random_chunk(64);
    let (status, _) = w
        .publish(
            &w.owner_token,
            &key,
            "page",
            &[declared, not_yet, personal_chunk],
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let response = w.chunk(&key, declared).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_of(&response, "cache-control"), "no-store");
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        declared_bytes.as_slice()
    );
    // Held by the feed but not declared; declared but not held; declared but held by another feed.
    for hash in [undeclared, not_yet, personal_chunk] {
        assert_eq!(
            w.chunk(&key, hash).await.status(),
            StatusCode::NOT_FOUND,
            "{hash:?}"
        );
    }
    // The declaration is a permission, not a copy: once the feed holds the chunk it is served.
    let status = w
        .server
        .http
        .put(w.url(&format!(
            "/v2/feeds/{}/chunks/{}",
            w.feed(),
            not_yet.to_hex()
        )))
        .bearer_auth(&w.owner_token)
        .body(not_yet_bytes.clone())
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(w.chunk(&key, not_yet).await.status(), StatusCode::OK);
    // Another link does not inherit the declaration, and a malformed hash is a 404 too.
    let other = LinkWorld::key(2);
    w.publish(&w.owner_token, &other, "other", &[], None).await;
    assert_eq!(
        w.chunk(&other, declared).await.status(),
        StatusCode::NOT_FOUND
    );
    let malformed = w
        .server
        .http
        .get(w.url(&format!("/v2/links/{}/chunks/zz", key.link_id())))
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::NOT_FOUND);
    // A refresh replaces the declaration; a chunk the Owner deletes is gone at once.
    w.publish(&w.owner_token, &key, "page 2", &[not_yet], None)
        .await;
    assert_eq!(
        w.chunk(&key, declared).await.status(),
        StatusCode::NOT_FOUND
    );
    let status = w
        .server
        .http
        .delete(w.url(&format!(
            "/v2/feeds/{}/chunks/{}",
            w.feed(),
            not_yet.to_hex()
        )))
        .bearer_auth(&w.owner_token)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(w.chunk(&key, not_yet).await.status(), StatusCode::NOT_FOUND);
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_link_is_a_404_at_once_and_the_pass_deletes_it() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let baseline = w.server.server.account_bytes_used(w.owner.account).unwrap();
    let (chunk, _) = w.chunk_in(w.feed(), &w.owner_token, 64).await;
    let with_chunk = w.server.server.account_bytes_used(w.owner.account).unwrap();
    let (status, _) = w
        .publish(&w.owner_token, &key, "page", &[chunk], Some(T0 + DAY_MS))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(w.server.server.account_bytes_used(w.owner.account).unwrap() > with_chunk);
    assert_eq!(w.get(&key, None).await.status(), StatusCode::OK);
    // A refresh cannot move the expiry.
    let refreshed = LinkWorld::put_body(
        w.feed(),
        Some(T0 + 300 * DAY_MS),
        &[chunk],
        LinkWorld::publication(&key, "page 2"),
    );
    assert_eq!(
        w.put(Some(&w.editor_token), &key, refreshed).await.0,
        StatusCode::OK
    );

    w.clock.advance(DAY_MS - 1);
    assert_eq!(w.get(&key, None).await.status(), StatusCode::OK);
    w.clock.advance(1);
    assert_eq!(w.get(&key, None).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(w.chunk(&key, chunk).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(w.rows(), (1, 1), "the row waits for the pass");
    let report = w.server.server.prune_now().unwrap();
    assert!(report.expired >= 1, "{report:?}");
    assert_eq!(w.rows(), (0, 0));
    assert_eq!(
        w.server.server.account_bytes_used(w.owner.account).unwrap(),
        with_chunk,
        "the publication's bytes went back to the account"
    );
    assert!(with_chunk > baseline);
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_creator_or_an_owner_revokes_and_revoking_removes_everything() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let baseline = w.server.server.account_bytes_used(w.owner.account).unwrap();
    let (chunk, _) = w.chunk_in(w.feed(), &w.editor_token, 64).await;
    let with_chunk = w.server.server.account_bytes_used(w.owner.account).unwrap();
    let (status, _) = w
        .publish(&w.editor_token, &key, "page", &[chunk], None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(w.rows(), (1, 1));

    assert_eq!(w.delete(None, &key).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        w.delete(Some(&w.stranger_token), &key).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        w.delete(Some(&w.viewer_token), &key).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        w.delete(Some(&w.owner_token), &LinkWorld::key(9)).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(w.get(&key, None).await.status(), StatusCode::OK);

    // The creator (an Editor) revokes; a second revocation finds nothing.
    assert_eq!(
        w.delete(Some(&w.editor_token), &key).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(w.get(&key, None).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(w.chunk(&key, chunk).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(w.rows(), (0, 0));
    assert_eq!(
        w.delete(Some(&w.editor_token), &key).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        w.server.server.account_bytes_used(w.owner.account).unwrap(),
        with_chunk
    );
    assert!(with_chunk > baseline);

    // An Owner revokes a link somebody else made; the link id can be used again afterwards.
    w.publish(&w.editor_token, &key, "again", &[], None).await;
    assert_eq!(
        w.delete(Some(&w.owner_token), &key).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(w.get(&key, None).await.status(), StatusCode::NOT_FOUND);
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_creator_cannot_revoke_and_closing_the_feed_deletes_the_links() {
    let mut w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let (chunk, _) = w.chunk_in(w.feed(), &w.editor_token, 64).await;
    w.publish(&w.editor_token, &key, "page", &[chunk], None)
        .await;

    // Removing the Editor does not revoke the link, and the Editor can no longer touch it.
    let (owner, viewer, editor) = (w.owner.clone(), w.viewer.clone(), w.editor.clone());
    let remove = w.space.remove(&owner, editor.account, &[&owner, &viewer]);
    assert_eq!(
        w.membership(&w.owner_token, &[remove.encode()]).await,
        StatusCode::OK
    );
    w.space.accept(&remove);
    assert_eq!(
        w.delete(Some(&w.editor_token), &key).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(w.get(&key, None).await.status(), StatusCode::OK);

    // An Owner closes the space: its links go, and nothing new is taken.
    let close = w.space.close(&owner);
    assert_eq!(
        w.membership(&w.owner_token, &[close.encode()]).await,
        StatusCode::OK
    );
    w.space.accept(&close);
    assert_eq!(w.get(&key, None).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(w.chunk(&key, chunk).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(w.rows(), (0, 0));
    let (status, answer) = w.publish(&w.owner_token, &key, "late", &[], None).await;
    assert_eq!(
        (status, answer["error"].as_str().unwrap_or("")),
        (StatusCode::CONFLICT, "feed_closed")
    );
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn public_link_reads_are_limited_per_address_and_minute() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let (chunk, _) = w.chunk_in(w.feed(), &w.owner_token, 64).await;
    w.publish(&w.owner_token, &key, "page", &[chunk], None)
        .await;
    let limit = usize::try_from(LINK_GETS_PER_MINUTE_PER_IP).unwrap();
    // Publication reads and chunk reads share the count; the answer does not matter (404 counts too).
    for i in 0..limit {
        let status = if i % 2 == 0 {
            w.get(&key, None).await.status()
        } else {
            w.chunk(&key, chunk).await.status()
        };
        assert_eq!(status, StatusCode::OK, "read {i}");
    }
    let refused = w.get(&key, None).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let answer: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(answer["error"], "rate_limited");
    assert_eq!(
        w.chunk(&key, chunk).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // Sessions and the other routes are not part of it.
    assert_eq!(
        w.delete(Some(&w.stranger_token), &key).await,
        StatusCode::NOT_FOUND
    );
    w.clock.advance(60_000);
    assert_eq!(w.get(&key, None).await.status(), StatusCode::OK);
    w.server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn http_remote_speaks_the_four_link_calls() {
    let w = LinkWorld::start(|_| {}).await;
    let key = LinkWorld::key(1);
    let link_id: LinkId = key.link_id();
    let (chunk, chunk_bytes) = w.chunk_in(w.feed(), &w.owner_token, 64).await;

    let member = |person: kb_sync::v2::conformance::Person| {
        let remote = HttpRemote::new(&w.server.base).unwrap();
        async move {
            let device = person.active_device();
            remote
                .authenticate(
                    person.account,
                    person.device_id(device),
                    &person.signer(device),
                )
                .await
                .unwrap();
            remote
        }
    };
    let owner = member(w.owner.clone()).await;
    let viewer = member(w.viewer.clone()).await;
    let stranger = member(w.stranger.clone()).await;
    let anonymous = HttpRemote::new(&w.server.base).unwrap();

    let publication = LinkWorld::publication(&key, "page");
    let put = LinkPut {
        feed: w.feed(),
        expires_at: Some(T0 + DAY_MS),
        chunks: vec![chunk],
        publication: publication.clone(),
    };
    assert_eq!(owner.link_put(link_id, &put).await.unwrap(), 1);
    assert!(matches!(
        viewer.link_put(link_id, &put).await,
        Err(SyncError::Forbidden)
    ));
    assert!(matches!(
        stranger.link_put(link_id, &put).await,
        Err(SyncError::NotFound)
    ));
    // Public reads need no session and do not send one.
    assert_eq!(
        anonymous.link_get(link_id, None).await.unwrap(),
        LinkFetch::Publication {
            pub_seq: 1,
            bytes: publication
        }
    );
    assert_eq!(
        anonymous.link_get(link_id, Some(1)).await.unwrap(),
        LinkFetch::NotModified
    );
    assert!(matches!(
        anonymous.link_get(link_id, Some(0)).await.unwrap(),
        LinkFetch::Publication { pub_seq: 1, .. }
    ));
    assert_eq!(
        anonymous.link_chunk_get(link_id, chunk).await.unwrap(),
        chunk_bytes
    );
    let (unknown, _) = w.space.random_chunk(8);
    assert!(matches!(
        anonymous.link_chunk_get(link_id, unknown).await,
        Err(SyncError::NotFound)
    ));
    // Revocation: the viewer may not, the creator may, the second time finds nothing.
    assert!(matches!(
        viewer.link_delete(link_id).await,
        Err(SyncError::Forbidden)
    ));
    owner.link_delete(link_id).await.unwrap();
    assert!(matches!(
        owner.link_delete(link_id).await,
        Err(SyncError::NotFound)
    ));
    assert!(matches!(
        anonymous.link_get(link_id, None).await,
        Err(SyncError::NotFound)
    ));
    // A publication over the limit is `Quota`, before anything leaves the machine.
    let big = LinkPut {
        publication: vec![0; usize::try_from(MAX_WIRE_BYTES).unwrap() + 1],
        ..put
    };
    assert!(matches!(
        owner.link_put(link_id, &big).await,
        Err(SyncError::Quota)
    ));
    w.server.stop().await;
}
