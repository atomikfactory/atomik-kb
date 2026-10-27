//! A small world for the view-link tests (M13 slice 4): a server with a clock, a shared space whose
//! members are an Owner, an Editor and a Viewer, a stranger, their sessions, and the link calls by
//! hand. A publication here is an unsigned envelope of kind `Publication` that carries the link
//! id in its header and any text as its sealed body: the server reads the header and nothing else.

use std::sync::Arc;

use kb_crypto::EnvelopeKind;
use kb_keys::link::LinkKey;
use kb_server::Config;
use kb_sync::v2::conformance::{DAY_MS, Person, Space};
use kb_sync::v2::{LinkPut, PublicationHeader, encode_link_put, encode_publication_header};
use kb_types::testing::{FixedClock, SeededRng};
use kb_types::{ChunkId, FeedId, MemberRole};
use reqwest::{Response, StatusCode};
use serde_json::{Value, json};

use super::{T0, TestServer, base64, config};

pub struct LinkWorld {
    pub dir: tempfile::TempDir,
    pub server: TestServer,
    pub clock: Arc<FixedClock>,
    pub space: Space,
    pub owner: Person,
    pub editor: Person,
    pub viewer: Person,
    pub stranger: Person,
    pub owner_token: String,
    pub editor_token: String,
    pub viewer_token: String,
    pub stranger_token: String,
}

impl LinkWorld {
    pub async fn start(adjust: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = config(dir.path());
        adjust(&mut config);
        let clock = Arc::new(FixedClock::new(T0));
        let server = TestServer::start_with_clock(&config, clock.clone()).await;
        let people: Vec<Person> = (0..4u64)
            .map(|i| Person::generate(4_400 + i, &format!("Link person {i}"), T0))
            .collect();
        let [owner, editor, viewer, stranger] = <[Person; 4]>::try_from(people).unwrap();
        let mut tokens = Vec::new();
        for person in [&owner, &editor, &viewer, &stranger] {
            assert_eq!(server.create_account(person).await, StatusCode::CREATED);
            tokens.push(server.login(person, person.active_device()).await);
        }
        let [owner_token, editor_token, viewer_token, stranger_token] =
            <[String; 4]>::try_from(tokens).unwrap();
        let mut world = Self {
            dir,
            server,
            clock,
            space: Space::new(4_477, "Link space", T0),
            owner,
            editor,
            viewer,
            stranger,
            owner_token,
            editor_token,
            viewer_token,
            stranger_token,
        };
        world.found().await;
        world
    }

    pub fn feed(&self) -> FeedId {
        self.space.feed
    }

    pub fn url(&self, path: &str) -> String {
        self.server.url(path)
    }

    async fn found(&mut self) {
        let create = self.space.create(&self.owner, 1);
        let response = self
            .server
            .http
            .post(self.url("/v2/feeds"))
            .bearer_auth(&self.owner_token)
            .header("content-type", "application/octet-stream")
            .body(create.encode())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        self.space.accept(&create);
        let (editor, viewer) = (self.editor.clone(), self.viewer.clone());
        let (editor_token, viewer_token) = (self.editor_token.clone(), self.viewer_token.clone());
        self.join(&editor, &editor_token, MemberRole::Editor).await;
        self.join(&viewer, &viewer_token, MemberRole::Viewer).await;
    }

    async fn join(&mut self, person: &Person, token: &str, role: MemberRole) {
        self.space.now = kb_types::Clock::now_ms(self.clock.as_ref());
        let (invite, secret) =
            self.space
                .invite(&self.owner, role, self.space.now + 7 * DAY_MS, None);
        let owner_token = self.owner_token.clone();
        assert_eq!(
            self.membership(&owner_token, &[invite.encode()]).await,
            StatusCode::OK
        );
        self.space.accept(&invite);
        let join = self.space.join(person, &secret);
        assert_eq!(
            self.membership(token, &[join.encode()]).await,
            StatusCode::OK
        );
        self.space.accept(&join);
    }

    /// Posts membership entries (already encoded); the status.
    pub async fn membership(&self, token: &str, entries: &[Vec<u8>]) -> StatusCode {
        let entries: Vec<String> = entries.iter().map(|e| base64(e)).collect();
        self.server
            .http
            .post(self.url(&format!("/v2/feeds/{}/membership", self.feed())))
            .bearer_auth(token)
            .json(&json!({ "entries": entries }))
            .send()
            .await
            .unwrap()
            .status()
    }

    /// Stores a chunk of `len` random bytes in `feed` as `token`'s account; the chunk and its bytes.
    pub async fn chunk_in(&self, feed: FeedId, token: &str, len: usize) -> (ChunkId, Vec<u8>) {
        let (id, bytes) = self.space.random_chunk(len);
        let status = self
            .server
            .http
            .put(self.url(&format!("/v2/feeds/{feed}/chunks/{}", id.to_hex())))
            .bearer_auth(token)
            .body(bytes.clone())
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::NO_CONTENT);
        (id, bytes)
    }

    // ----- links ---------------------------------------------------------------------------

    /// The `n`th link key of this world.
    pub fn key(n: u64) -> LinkKey {
        LinkKey::generate(&SeededRng::new(9_000 + n))
    }

    /// A sealed publication for the key's link carrying `text`.
    pub fn publication(key: &LinkKey, text: &str) -> Vec<u8> {
        let header = encode_publication_header(&PublicationHeader {
            link_id: key.link_id(),
        });
        kb_crypto::seal_envelope(
            EnvelopeKind::Publication,
            &header,
            &key.publication_key(),
            text.as_bytes(),
            None,
            &SeededRng::new(5),
        )
    }

    /// The body of `PUT /v2/links/{id}`.
    pub fn put_body(
        feed: FeedId,
        expires_at: Option<i64>,
        chunks: &[ChunkId],
        publication: Vec<u8>,
    ) -> Vec<u8> {
        encode_link_put(&LinkPut {
            feed,
            expires_at,
            chunks: chunks.to_vec(),
            publication,
        })
        .unwrap()
    }

    /// `PUT /v2/links/{id}`; the status and the JSON body.
    pub async fn put(
        &self,
        token: Option<&str>,
        key: &LinkKey,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .server
            .http
            .put(self.url(&format!("/v2/links/{}", key.link_id())))
            .header("content-type", "application/octet-stream")
            .body(body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A first upload by `token` into the shared feed: `text` as the publication, the declared chunks.
    pub async fn publish(
        &self,
        token: &str,
        key: &LinkKey,
        text: &str,
        chunks: &[ChunkId],
        expires_at: Option<i64>,
    ) -> (StatusCode, Value) {
        let body = Self::put_body(
            self.feed(),
            expires_at,
            chunks,
            Self::publication(key, text),
        );
        self.put(Some(token), key, body).await
    }

    /// `GET /v2/links/{id}` without a session.
    pub async fn get(&self, key: &LinkKey, if_none_match: Option<&str>) -> Response {
        let mut request = self
            .server
            .http
            .get(self.url(&format!("/v2/links/{}", key.link_id())));
        if let Some(value) = if_none_match {
            request = request.header("if-none-match", value);
        }
        request.send().await.unwrap()
    }

    /// `GET /v2/links/{id}/chunks/{hash}` without a session.
    pub async fn chunk(&self, key: &LinkKey, hash: ChunkId) -> Response {
        self.server
            .http
            .get(self.url(&format!(
                "/v2/links/{}/chunks/{}",
                key.link_id(),
                hash.to_hex()
            )))
            .send()
            .await
            .unwrap()
    }

    /// `DELETE /v2/links/{id}`; the status.
    pub async fn delete(&self, token: Option<&str>, key: &LinkKey) -> StatusCode {
        let mut request = self
            .server
            .http
            .delete(self.url(&format!("/v2/links/{}", key.link_id())));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().await.unwrap().status()
    }

    /// How many rows `links` and `link_chunks` hold, read from the server's file.
    pub fn rows(&self) -> (i64, i64) {
        let conn = rusqlite::Connection::open(self.dir.path().join("server.db")).unwrap();
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        (count("links"), count("link_chunks"))
    }
}
