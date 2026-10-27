//! The WebSocket client of [`HttpRemote`] (M13 slice 3, ADR-0050, [`crate::realtime`]): one connection
//! to `GET /v2/ws` per device, owned by a background task that
//!
//! - connects with the session's bearer token (waiting for one while there is none), reconnects with
//!   backoff (1 s to 1 min, back to 1 s after a connection that lasted) and with the current token;
//! - sends the **latest `Subscribe` after every (re)connection** (it keeps the last one the driver gave
//!   it, also while it is not connected), then reports [`RealtimeEvent::Connected`];
//! - decodes the server's frames (`kb_sync::v2::ws`) into [`RealtimeEvent`]s, in order, and ignores
//!   what it does not understand (an unknown text frame, a frame that does not decode);
//! - encodes the driver's presence frames (dropped while there is no connection: presence is best
//!   effort) and subscriptions;
//! - treats a refused upgrade (401, 403) as an authentication failure: the token is not tried again (a 401 also
//!   drops the session, so that the driver authenticates anew), [`RealtimeEvent::Refused`] tells the driver, and
//!   the task waits for a new session instead of retrying;
//! - ends, closing the connection, when the driver drops its commands or its events.
//!
//! Nothing here logs a token, a frame, an entry or an id.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use kb_sync::v2::ws::CLIENT_MESSAGE_MAX_BYTES;
use kb_sync::v2::{
    ServerFrame, decode_server_binary, decode_server_text, encode_presence_frame, encode_subscribe,
};
use kb_types::FeedId;
use reqwest::header::AUTHORIZATION;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::{
    HttpRemote, REQUEST_TIMEOUT, SharedSession, WS_BACKOFF_MAX, WS_BACKOFF_MIN, lock, ws_url,
};
use crate::NetError;
use crate::realtime::{
    COMMAND_QUEUE, EVENT_QUEUE, MAX_WS_MESSAGE, Realtime, RealtimeCommand, RealtimeEvent,
};

/// A connection that lasted this long was a working one: the next reconnection is not delayed.
const WORKED: Duration = Duration::from_secs(10);

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

impl HttpRemote {
    /// Opens the realtime socket: a background task keeps one WebSocket to `GET /v2/ws` open (see the
    /// [module documentation](self)). Needs a session (the task waits for one) and a tokio runtime.
    /// Dropping the returned commands, or the events, ends the task and closes the connection.
    pub fn open_realtime(&self) -> Result<Realtime, NetError> {
        let url = ws_url(&self.base_url)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| NetError::Transport("no async runtime for the WebSocket".to_owned()))?;
        let (event_sender, events) = mpsc::channel(EVENT_QUEUE);
        let (commands, command_receiver) = mpsc::channel(COMMAND_QUEUE);
        runtime.spawn(socket_loop(
            url,
            Arc::clone(&self.session),
            event_sender,
            command_receiver,
        ));
        Ok(Realtime { events, commands })
    }
}

/// How one connection ended.
enum Ended {
    /// The driver dropped its end: stop for good.
    Gone,
    /// The connection closed or broke; `lasted` says whether it was a working one.
    Dropped { lasted: bool },
}

/// Keeps a connection open for as long as the driver holds its ends.
async fn socket_loop(
    url: String,
    session: SharedSession,
    events: mpsc::Sender<RealtimeEvent>,
    mut commands: mpsc::Receiver<RealtimeCommand>,
) {
    let mut subscription: Option<Vec<FeedId>> = None;
    let mut delay = WS_BACKOFF_MIN;
    // The token the server refused: it is never tried again, so a device the server turned away (revoked, or
    // its session gone) does not hammer it. A new session brings a new token.
    let mut refused: Option<String> = None;
    loop {
        let token = lock(&session)
            .token
            .clone()
            .filter(|token| refused.as_ref() != Some(token));
        let waiting = token.is_none();
        if let Some(token) = token {
            match connect(&url, &token).await {
                Attempt::Open(stream) => {
                    match serve(*stream, &mut subscription, &events, &mut commands).await {
                        Ended::Gone => return,
                        Ended::Dropped { lasted } => {
                            if lasted {
                                delay = WS_BACKOFF_MIN;
                            }
                        }
                    }
                }
                Attempt::Refused { unauthorized } => {
                    // An authentication failure, not a connection failure: the session is gone (401) or
                    // the device is refused (403). The driver is told so that its next cycle
                    // authenticates again, or reports why it cannot.
                    if unauthorized {
                        let mut shared = lock(&session);
                        if shared.token.as_deref() == Some(token.as_str()) {
                            shared.token = None;
                            shared.identity = None;
                        }
                    }
                    refused = Some(token);
                    if events.send(RealtimeEvent::Refused).await.is_err() {
                        return;
                    }
                }
                Attempt::Failed => {}
            }
        }
        if !pause(delay, &mut subscription, &mut commands).await {
            return;
        }
        // Waiting for a session costs nothing, so the pause stays short; a server that cannot be reached
        // is asked less and less often.
        delay = if waiting {
            WS_BACKOFF_MIN
        } else {
            (delay * 2).min(WS_BACKOFF_MAX)
        };
    }
}

/// How an attempt to connect ended.
enum Attempt {
    Open(Box<Stream>),
    /// The server answered the upgrade with 401 (`unauthorized`) or 403.
    Refused {
        unauthorized: bool,
    },
    /// Anything else: no route, a timeout, a server that is down.
    Failed,
}

/// Waits `delay`, remembering the latest subscription and dropping presence meanwhile; `false` when the
/// driver dropped its commands.
async fn pause(
    delay: Duration,
    subscription: &mut Option<Vec<FeedId>>,
    commands: &mut mpsc::Receiver<RealtimeCommand>,
) -> bool {
    let timer = tokio::time::sleep(delay);
    tokio::pin!(timer);
    loop {
        tokio::select! {
            () = &mut timer => return true,
            command = commands.recv() => match command {
                None => return false,
                Some(RealtimeCommand::Subscribe(feeds)) => *subscription = Some(feeds),
                Some(RealtimeCommand::Presence(_)) => {}
            },
        }
    }
}

async fn connect(url: &str, token: &str) -> Attempt {
    let Ok(mut request) = url.into_client_request() else {
        return Attempt::Failed;
    };
    let Ok(authorization) = format!("Bearer {token}").parse() else {
        return Attempt::Failed;
    };
    request.headers_mut().insert(AUTHORIZATION, authorization);
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MESSAGE))
        .max_frame_size(Some(MAX_WS_MESSAGE));
    let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
    match tokio::time::timeout(REQUEST_TIMEOUT, connect).await {
        Ok(Ok((stream, _))) => {
            tracing::debug!("realtime socket open");
            Attempt::Open(Box::new(stream))
        }
        Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
            if matches!(response.status().as_u16(), 401 | 403) =>
        {
            tracing::debug!("the realtime socket was refused");
            Attempt::Refused {
                unauthorized: response.status().as_u16() == 401,
            }
        }
        Ok(Err(_)) | Err(_) => {
            tracing::debug!("the realtime socket could not be opened");
            Attempt::Failed
        }
    }
}

/// Sends the subscription as the text frame; `false` when the connection is gone.
async fn send_subscription(stream: &mut Stream, feeds: &[FeedId]) -> bool {
    match encode_subscribe(feeds) {
        // The server closes a connection that sends a message over its limit: never send one.
        Ok(text) if text.len() > CLIENT_MESSAGE_MAX_BYTES => {
            tracing::debug!(feeds = feeds.len(), "the subscription is too large to send");
            true
        }
        Ok(text) => stream.send(Message::text(text)).await.is_ok(),
        // More feeds than the protocol allows: nothing to send, the connection is fine.
        Err(_) => {
            tracing::debug!(feeds = feeds.len(), "the subscription could not be encoded");
            true
        }
    }
}

/// One connection: the subscription, `Connected`, then the frames both ways until it ends.
async fn serve(
    mut stream: Stream,
    subscription: &mut Option<Vec<FeedId>>,
    events: &mpsc::Sender<RealtimeEvent>,
    commands: &mut mpsc::Receiver<RealtimeCommand>,
) -> Ended {
    let opened = Instant::now();
    // What the driver said while the connection was being made: the latest subscription counts, the
    // presence is stale.
    loop {
        match commands.try_recv() {
            Ok(RealtimeCommand::Subscribe(feeds)) => *subscription = Some(feeds),
            Ok(RealtimeCommand::Presence(_)) => {}
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                let _ = stream.close(None).await;
                return Ended::Gone;
            }
        }
    }
    let lasted = || opened.elapsed() >= WORKED;
    if let Some(feeds) = subscription.as_deref()
        && !send_subscription(&mut stream, feeds).await
    {
        return Ended::Dropped { lasted: false };
    }
    if events.send(RealtimeEvent::Connected).await.is_err() {
        let _ = stream.close(None).await;
        return Ended::Gone;
    }
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => {
                    let _ = stream.close(None).await;
                    return Ended::Gone;
                }
                Some(RealtimeCommand::Subscribe(feeds)) => {
                    let sent = send_subscription(&mut stream, &feeds).await;
                    *subscription = Some(feeds);
                    if !sent {
                        return disconnected(events, lasted()).await;
                    }
                }
                Some(RealtimeCommand::Presence(frame)) => {
                    // A frame over the limit is not sent (the handle checks it too).
                    if let Ok(bytes) = encode_presence_frame(&frame)
                        && stream.send(Message::Binary(bytes.into())).await.is_err()
                    {
                        return disconnected(events, lasted()).await;
                    }
                }
            },
            message = stream.next() => {
                let event = match message {
                    Some(Ok(Message::Text(text))) => match decode_server_text(text.as_str()) {
                        Ok(ServerFrame::Changed { feed, cursor }) => {
                            Some(RealtimeEvent::Changed { feed, cursor })
                        }
                        _ => None,
                    },
                    Some(Ok(Message::Binary(bytes))) => match decode_server_binary(&bytes) {
                        Ok(ServerFrame::Entry(inline)) => Some(RealtimeEvent::Entry(inline)),
                        Ok(ServerFrame::Presence(frame)) => Some(RealtimeEvent::Presence(frame)),
                        _ => None,
                    },
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        tracing::debug!("realtime socket closed");
                        return disconnected(events, lasted()).await;
                    }
                    Some(Ok(_)) => None,
                };
                if let Some(event) = event
                    && events.send(event).await.is_err()
                {
                    let _ = stream.close(None).await;
                    return Ended::Gone;
                }
            }
        }
    }
}

/// Reports the end of a connection that had been reported as connected.
async fn disconnected(events: &mpsc::Sender<RealtimeEvent>, lasted: bool) -> Ended {
    if events.send(RealtimeEvent::Disconnected).await.is_err() {
        return Ended::Gone;
    }
    Ended::Dropped { lasted }
}
