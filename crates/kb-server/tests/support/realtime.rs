//! A real WebSocket client for the realtime tests (M13 slice 3): [`ServerWs`] is the
//! `kb_sync::v2::conformance::WsHarness` of the in-process server, and [`ServerSocket`] one
//! connection to `GET /v2/ws` with the frames of `kb_sync::v2::ws`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use kb_sync::SyncError;
use kb_sync::v2::conformance::{Person, WsConnection, WsHarness};
use kb_sync::v2::{
    PresenceFrame, ServerFrame, decode_server_binary, decode_server_text, encode_presence_frame,
    encode_subscribe,
};
use kb_types::FeedId;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::TestServer;

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn unavailable(what: &str, error: impl core::fmt::Display) -> SyncError {
    SyncError::Unavailable(format!("{what}: {error}"))
}

/// The server under test as a [`WsHarness`].
pub struct ServerWs<'a> {
    pub server: &'a TestServer,
}

impl ServerWs<'_> {
    /// A socket of `person`'s first active device (a session of its own), not subscribed to anything.
    pub async fn connect(&self, person: &Person) -> Result<ServerSocket, SyncError> {
        self.connect_device(person, person.active_device()).await
    }

    /// A socket of `person`'s device number `device`: `Unauthorized` when the server refuses it a
    /// session (it is not active in the account's log).
    pub async fn connect_device(
        &self,
        person: &Person,
        device: usize,
    ) -> Result<ServerSocket, SyncError> {
        let (status, _) = self
            .server
            .challenge(person.account, person.device_id(device))
            .await;
        if status != reqwest::StatusCode::OK {
            return Err(SyncError::Unauthorized);
        }
        let token = self.server.login(person, device).await;
        self.connect_with(&token).await
    }

    /// A socket on the session of `token`.
    pub async fn connect_with(&self, token: &str) -> Result<ServerSocket, SyncError> {
        let url = format!(
            "{}/v2/ws?token={token}",
            self.server.base.replace("http://", "ws://")
        );
        let (stream, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(response)
                    if response.status() == reqwest::StatusCode::UNAUTHORIZED =>
                {
                    SyncError::Unauthorized
                }
                other => unavailable("connect", other),
            })?;
        Ok(ServerSocket {
            stream: tokio::sync::Mutex::new(stream),
            server: self.server.server.clone(),
        })
    }
}

impl WsHarness for ServerWs<'_> {
    type Socket = ServerSocket;

    async fn open_device_socket(
        &self,
        person: &Person,
        device: usize,
    ) -> Result<ServerSocket, SyncError> {
        self.connect_device(person, device).await
    }

    async fn wait_ms(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// One WebSocket to the server.
pub struct ServerSocket {
    stream: tokio::sync::Mutex<Stream>,
    server: kb_server::Server,
}

impl ServerSocket {
    async fn send(&self, message: Message) -> Result<(), SyncError> {
        self.stream
            .lock()
            .await
            .send(message)
            .await
            .map_err(|e| unavailable("send", e))
    }

    /// Sends `subscribe` and waits until the server handled it (it has no acknowledgement).
    pub async fn subscribe_and_wait(&self, feeds: &[FeedId]) -> Result<(), SyncError> {
        let text = encode_subscribe(feeds)?;
        let before = self.server.realtime_subscribes_handled();
        self.send(Message::text(text)).await?;
        let started = Instant::now();
        while self.server.realtime_subscribes_handled() == before {
            if started.elapsed() > Duration::from_secs(10) {
                return Err(SyncError::Unavailable(
                    "the server did not handle the subscription".to_owned(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        Ok(())
    }

    /// Whether the server closes the connection within `wait_ms` (a close frame, an error or the end
    /// of the stream); frames that arrive meanwhile are read and ignored.
    pub async fn closed_within(&self, wait_ms: u64) -> bool {
        let mut stream = self.stream.lock().await;
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, stream.next()).await {
                Err(_) => return false,
                Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => return true,
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    /// Every frame that arrives within `wait_ms` of the previous one (or of the call).
    pub async fn drain(&self, wait_ms: u64) -> Vec<ServerFrame> {
        let mut out = Vec::new();
        while let Some(frame) = WsConnection::recv(self, wait_ms).await {
            out.push(frame);
        }
        out
    }
}

impl WsConnection for ServerSocket {
    async fn subscribe(&self, feeds: &[FeedId]) -> Result<(), SyncError> {
        self.subscribe_and_wait(feeds).await
    }

    async fn send_presence(&self, frame: &PresenceFrame) -> Result<(), SyncError> {
        let bytes = encode_presence_frame(frame)?;
        self.send(Message::binary(bytes)).await
    }

    async fn send_binary(&self, bytes: &[u8]) -> Result<(), SyncError> {
        self.send(Message::binary(bytes.to_vec())).await
    }

    async fn send_text(&self, text: &str) -> Result<(), SyncError> {
        self.send(Message::text(text.to_owned())).await
    }

    async fn is_closed(&self, wait_ms: u64) -> bool {
        self.closed_within(wait_ms).await
    }

    async fn recv(&self, wait_ms: u64) -> Option<ServerFrame> {
        let mut stream = self.stream.lock().await;
        loop {
            match tokio::time::timeout(Duration::from_millis(wait_ms), stream.next()).await {
                Err(_) | Ok(None | Some(Err(_))) => return None,
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Some(
                        decode_server_text(text.as_str()).expect("a frame of the protocol"),
                    );
                }
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    return Some(decode_server_binary(&bytes).expect("a frame of the protocol"));
                }
                Ok(Some(Ok(Message::Close(_)))) => return None,
                // A ping (answered by the protocol layer) or a pong.
                Ok(Some(Ok(_))) => {}
            }
        }
    }
}
