//! Shared harness of the integration tests: an in-process server on an ephemeral port
//! with an injected clock, and small JSON helpers. The `/v2` calls the tests make by hand
//! (accounts, challenge, session) use the people of `kb_sync::v2::conformance`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

pub mod links;
pub mod realtime;

use std::path::Path;
use std::sync::Arc;

use kb_crypto::Signature;
use kb_server::{Config, Server};
use kb_sync::v2::conformance::Person;
use kb_types::testing::FixedClock;
use kb_types::{AccountId, DeviceId};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// The clock every test server starts at (Unix ms).
pub const T0: i64 = 1_790_000_000_000;
/// One day in ms.
pub const DAY_MS: i64 = 86_400_000;

/// A configuration for a test server in `dir` with the invite token `dev`.
pub fn config(dir: &Path) -> Config {
    Config {
        listen: ([127, 0, 0, 1], 0).into(),
        data_dir: dir.to_path_buf(),
        invite_tokens: vec!["dev".to_owned()],
        account_quota_bytes: 10 * 1024 * 1024 * 1024,
        prune_grace_days: 7,
        idle_device_days: None,
        trusted_proxies: Vec::new(),
        share_signups: 20,
        share_signups_per_account: 5,
        viewer_dir: None,
    }
}

/// A server serving on `127.0.0.1:<ephemeral>`.
pub struct TestServer {
    pub base: String,
    pub server: Server,
    pub clock: Arc<FixedClock>,
    pub http: reqwest::Client,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl TestServer {
    pub async fn start(config: &Config) -> Self {
        Self::start_with_clock(config, Arc::new(FixedClock::new(T0))).await
    }

    pub async fn start_with_clock(config: &Config, clock: Arc<FixedClock>) -> Self {
        let server = Server::open_with_clock(config, clock.clone()).unwrap();
        let app = server
            .router()
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            base: format!("http://{addr}"),
            server,
            clock,
            http: reqwest::Client::new(),
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }

    /// Stops serving and waits for the server task to end.
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.await.unwrap();
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// `GET /v2/server`'s epoch.
    pub async fn epoch(&self) -> [u8; 16] {
        let info: Value = self
            .http
            .get(self.url("/v2/server"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        unhex(info["serverEpoch"].as_str().unwrap())
    }

    /// Creates the person's account with the operator token; answers the status.
    pub async fn create_account(&self, person: &Person) -> StatusCode {
        let log: Vec<String> = person.encoded_log().iter().map(|e| base64(e)).collect();
        self.http
            .post(self.url("/v2/accounts"))
            .json(&json!({ "inviteToken": "dev", "log": log }))
            .send()
            .await
            .unwrap()
            .status()
    }

    /// `POST /v2/auth/challenge`; status and body.
    pub async fn challenge(&self, account: AccountId, device: DeviceId) -> (StatusCode, Value) {
        let response = self
            .http
            .post(self.url("/v2/auth/challenge"))
            .json(&json!({ "accountId": account, "deviceId": device }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// `POST /v2/auth/session`; status and body.
    pub async fn session(
        &self,
        account: AccountId,
        device: DeviceId,
        nonce: &str,
        signature: &Signature,
    ) -> (StatusCode, Value) {
        let response = self
            .http
            .post(self.url("/v2/auth/session"))
            .json(&json!({
                "accountId": account,
                "deviceId": device,
                "nonce": nonce,
                "signature": hex(&signature.to_bytes()),
            }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// Challenge and session as the person's device `device`; the bearer token.
    pub async fn login(&self, person: &Person, device: usize) -> String {
        let (status, challenge) = self
            .challenge(person.account, person.device_id(device))
            .await;
        assert_eq!(status, StatusCode::OK, "{challenge}");
        let nonce_hex = challenge["nonce"].as_str().unwrap().to_owned();
        let nonce: [u8; 32] = unhex(&nonce_hex);
        let epoch: [u8; 16] = unhex(challenge["serverEpoch"].as_str().unwrap());
        let signature = person.signer(device)(&kb_sync::auth_message(&nonce, &epoch));
        let (status, session) = self
            .session(
                person.account,
                person.device_id(device),
                &nonce_hex,
                &signature,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{session}");
        session["token"].as_str().unwrap().to_owned()
    }
}

// ----- encodings ----------------------------------------------------------------------------

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex<const N: usize>(text: &str) -> [u8; N] {
    assert_eq!(text.len(), N * 2, "hex of {N} bytes expected");
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn unbase64(text: &str) -> Vec<u8> {
    let digits: Vec<u32> = text
        .bytes()
        .filter(|c| *c != b'=')
        .map(|c| B64.iter().position(|d| *d == c).unwrap() as u32)
        .collect();
    let mut out = Vec::new();
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, d) in chunk.iter().enumerate() {
            n |= d << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    out
}
