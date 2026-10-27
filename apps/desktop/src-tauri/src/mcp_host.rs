//! The MCP server's loopback host (ADR-0037, `docs/architecture/ai.md`, sections 6.4 and 7): Streamable
//! HTTP on `127.0.0.1` while the app is open and unlocked and `app.mcp.settings.enabled` is on. The
//! protocol, the clients, the grants and the audit are the core's (`Request::McpCall`); the shell is the
//! transport and the checks a browser cannot be trusted to make.
//!
//! **The listener.** A background task like the sync driver's: while the core is ready it reads
//! `app.mcp.settings` and, when MCP is on, binds `127.0.0.1:<port>` (and only that address). It reads the
//! setting again on every `SettingChanged` of `app.mcp.settings` and rebinds when the port changed or
//! stops when MCP was turned off; a port that cannot be bound is the `error` of `mcp_status` and is tried
//! again every 30 seconds. Dropping the task (the core locked) closes the listener, and a connection a
//! client still holds open is refused from then on.
//!
//! **The checks**, in this order, each with its status: the `Host` header must be `127.0.0.1:<port>` or
//! `localhost:<port>` (403: a web page that rebinds its own name to 127.0.0.1 is refused here); an
//! `Origin`, when there is one, must be `http://127.0.0.1` or `http://localhost` with any port (403); the
//! path is `/mcp` (404) and the method `POST` (405, `Allow: POST`; `GET` and `DELETE` are not offered);
//! an `Authorization: Bearer <token>` is present (401, `WWW-Authenticate: Bearer`); the content type is
//! `application/json` (415); the body is at most 1 MiB (413) and arrives within 15 s (408). Then the
//! shell dispatches `McpCall { token, message }`: an answer is 200 with `application/json`, no answer (a
//! notification) is 202 without a body, `InvalidInput` for the field `token` is 401 and for the field `message`
//! 413 (the core's own limit), a core that is
//! locked or starting is 503, anything else 500. Neither the token nor the body is ever logged.
//!
//! **Commands.** `mcp_status` answers `McpStatus` (the setting, whether the listener runs, the URL, the
//! path of `kb-mcp-bridge(.exe)` when it sits next to the app's executable, the bind error).
//! `mcp_client_grants_set` asks the person in a native dialog ([`Confirm`]) when the new grants add any
//! read or write access ("Allow <client> to read <spaces> and change <spaces>?"); declining is
//! `InvalidInput` for `grants` with `details.reason = "declined"` and nothing changes; removing access
//! needs no dialog. Then it dispatches `McpClientGrantsSet`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request as HttpRequest, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response as HttpResponse};
use kb_types::ai::{MCP_SETTINGS_SETTING, McpClientInfo, McpGrant, McpSettings, McpStatus};
use kb_types::{ApiError, ErrorCode, Event, Request, Response, SpaceId};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::MCP_STOPPED;
use crate::ai_client::{Reason, failure_for};
use crate::ai_host::{core, read_setting};
use crate::blocking;
use crate::dialogs::{Confirm, Question};
use crate::shell_labels::{Labels, fill};
use crate::state::{BackgroundStarter, BackgroundTask, CoreHost};

/// The largest request body: 1 MiB.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

/// How long a request has to deliver its body.
const BODY_TIMEOUT: Duration = Duration::from_secs(15);

/// How often a listener that could not be bound is tried again.
const BIND_RETRY: Duration = Duration::from_secs(30);

/// The lowest port a person may choose.
const MIN_PORT: u16 = 1024;

/// The name of the stdio bridge's executable next to the app's.
pub const BRIDGE_NAME: &str = if cfg!(windows) {
    "kb-mcp-bridge.exe"
} else {
    "kb-mcp-bridge"
};

/// The longest token the shell hands to the core; anything longer is not one of ours.
const MAX_TOKEN_CHARS: usize = 256;

// --- what the listener reports ---

/// What the listener is doing, for `mcp_status`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listening {
    /// The listener is bound.
    pub running: bool,
    /// The port it is bound to.
    pub port: Option<u16>,
    /// Why it is not (a short sentence, never an address).
    pub error: Option<String>,
}

/// Managed by Tauri inside an `Arc`; the listener's task holds the same one.
#[derive(Default)]
pub struct McpShell {
    listening: Mutex<Listening>,
}

impl McpShell {
    /// A shell whose listener is not running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What the listener is doing.
    #[must_use]
    pub fn listening(&self) -> Listening {
        self.listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, listening: Listening) {
        *self
            .listening
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = listening;
    }
}

// --- the settings ---

/// The MCP preferences; off on the default port when none are stored or the stored value is not one.
fn read_mcp_settings(host: &dyn CoreHost) -> Result<McpSettings, ApiError> {
    Ok(read_setting(host, MCP_SETTINGS_SETTING)?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

/// `http://127.0.0.1:<port>/mcp`.
#[must_use]
pub fn url_of(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// The stdio bridge in `dir` (the folder of the app's executable), when it is there, as text.
#[must_use]
pub fn bridge_in(dir: &Path) -> Option<String> {
    let path = dir.join(BRIDGE_NAME);
    path.is_file().then(|| path.to_str().map(str::to_owned))?
}

/// The stdio bridge next to the running executable.
fn bridge_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    bridge_in(exe.parent()?)
}

/// `mcp_status`.
///
/// # Errors
///
/// `Locked` or "core is starting" while the core is not ready (the setting is in it).
pub async fn status(host: Arc<dyn CoreHost>, shell: Arc<McpShell>) -> Result<McpStatus, ApiError> {
    let settings = core(&host, |host| {
        host.ensure_ready()?;
        read_mcp_settings(host)
    })
    .await?;
    let listening = shell.listening();
    let port = listening.port.unwrap_or(settings.port);
    Ok(McpStatus {
        enabled: settings.enabled,
        running: settings.enabled && listening.running,
        port,
        url: url_of(port),
        bridge_path: bridge_path(),
        error: if settings.enabled {
            listening.error
        } else {
            None
        },
    })
}

// --- grants ---

/// The spaces a new set of grants gives access to that the client did not have: those it may newly
/// read (and not write), and those it may newly write.
#[must_use]
pub fn added_access(current: &[McpGrant], new: &[McpGrant]) -> (Vec<SpaceId>, Vec<SpaceId>) {
    // (read, write) a list of grants amounts to for one space; write implies read.
    fn access(grants: &[McpGrant], space: SpaceId) -> (bool, bool) {
        grants
            .iter()
            .filter(|grant| grant.space_id == space)
            .fold((false, false), |(read, write), grant| {
                (read || grant.read || grant.write, write || grant.write)
            })
    }
    let mut read_only = Vec::new();
    let mut write = Vec::new();
    let mut seen = Vec::new();
    for grant in new {
        if seen.contains(&grant.space_id) {
            continue;
        }
        seen.push(grant.space_id);
        let (now_read, now_write) = access(new, grant.space_id);
        let (was_read, was_write) = access(current, grant.space_id);
        if now_write && !was_write {
            write.push(grant.space_id);
        } else if now_read && !was_read {
            read_only.push(grant.space_id);
        }
    }
    (read_only, write)
}

/// A name as it goes in a dialog: control characters dropped, at most 60 characters, `Untitled` (in the words of
/// `labels`) when nothing is left.
fn shown(labels: &Labels, name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        labels.dialog_untitled.clone()
    } else {
        cleaned.to_owned()
    }
}

/// "A, B and 2 more": at most five names, the rest counted (the words are `labels`').
fn name_list(labels: &Labels, names: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut listed: Vec<String> = names.iter().take(SHOWN).cloned().collect();
    if names.len() > SHOWN {
        let more = (names.len() - SHOWN).to_string();
        listed.push(fill(&labels.dialog_list_more, &[("count", &more)]));
    }
    match listed.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => fill(
            &labels.dialog_list_and,
            &[("names", &init.join(", ")), ("last", last)],
        ),
    }
}

/// The question of `mcp_client_grants_set`: "Allow <client> to read <spaces> and change <spaces>?", in the words
/// of `labels` (the UI's language; the text always names the client and the spaces, `shell_labels` refuses a
/// translation that does not). `client` is cleaned here, the spaces' names by the caller (`shown`).
#[must_use]
pub fn grants_question(
    labels: &Labels,
    client: &str,
    read: &[String],
    write: &[String],
) -> Question {
    let client = shown(labels, client);
    let (read_names, write_names) = (name_list(labels, read), name_list(labels, write));
    let message = match (read.is_empty(), write.is_empty()) {
        (false, true) => fill(
            &labels.dialog_mcp_read,
            &[("client", &client), ("spaces", &read_names)],
        ),
        (true, false) => fill(
            &labels.dialog_mcp_write,
            &[("client", &client), ("spaces", &write_names)],
        ),
        _ => fill(
            &labels.dialog_mcp_both,
            &[
                ("client", &client),
                ("read", &read_names),
                ("write", &write_names),
            ],
        ),
    };
    Question {
        title: labels.dialog_mcp_title.clone(),
        message,
        accept: labels.dialog_allow.clone(),
        cancel: labels.dialog_cancel.clone(),
    }
}

/// `mcp_client_grants_set`: replaces a client's grants; asks first when the change adds access.
///
/// # Errors
///
/// `InvalidInput` for `grants` when the person declined (`details.reason = "declined"`, nothing
/// changes); `Locked` or "core is starting"; the core's (`NotFound` for an unknown client).
pub async fn grants_set(
    host: Arc<dyn CoreHost>,
    confirm: Arc<dyn Confirm>,
    client_id: String,
    grants: Vec<McpGrant>,
) -> Result<McpClientInfo, ApiError> {
    core(&host, move |host| {
        host.ensure_ready()?;
        let clients = match host.dispatch(Request::McpClientList)? {
            Response::McpClients(clients) => clients,
            _ => {
                return Err(ApiError::internal(
                    "the core answered with something unexpected",
                ));
            }
        };
        // An unknown client is the core's to refuse (`NotFound`), below.
        if let Some(client) = clients.iter().find(|client| client.id == client_id) {
            let (read, write) = added_access(&client.grants, &grants);
            if !read.is_empty() || !write.is_empty() {
                let spaces = match host.dispatch(Request::SpaceList)? {
                    Response::Spaces(spaces) => spaces,
                    _ => {
                        return Err(ApiError::internal(
                            "the core answered with something unexpected",
                        ));
                    }
                };
                // The words of the dialog are the UI's now (`shell_labels`).
                let labels = confirm.labels();
                let named = |ids: &[SpaceId]| -> Vec<String> {
                    ids.iter()
                        .filter_map(|id| spaces.iter().find(|space| space.id == *id))
                        .map(|space| shown(&labels, &space.name))
                        .collect()
                };
                // A grant of a space that does not exist is ignored by the core: nothing to ask.
                let (read, write) = (named(&read), named(&write));
                if (!read.is_empty() || !write.is_empty())
                    && !confirm.ask(&grants_question(&labels, &client.name, &read, &write))
                {
                    return Err(failure_for(
                        "grants",
                        Reason::Declined,
                        "the access was not changed",
                    ));
                }
            }
        }
        match host.dispatch(Request::McpClientGrantsSet { client_id, grants })? {
            Response::McpClient(info) => Ok(info),
            _ => Err(ApiError::internal(
                "the core answered with something unexpected",
            )),
        }
    })
    .await
}

// --- the HTTP side ---

/// What a request handler knows.
struct McpState {
    host: Weak<dyn CoreHost>,
    port: u16,
    /// Cleared when the listener stops: a connection that outlives it is refused.
    active: Arc<AtomicBool>,
}

fn router(state: Arc<McpState>) -> Router {
    Router::new().fallback(handle).with_state(state)
}

/// A response with a status and a short plain-text reason.
fn reply(status: StatusCode, reason: &'static str) -> HttpResponse {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        reason,
    )
        .into_response()
}

fn unauthorized() -> HttpResponse {
    let mut response = reply(StatusCode::UNAUTHORIZED, "a valid bearer token is required");
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

/// The `Host` the request was sent to: the header, or the authority of an HTTP/2 request.
fn request_host(request: &HttpRequest) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(|a| a.as_str().to_owned()))
}

/// Whether `host` is `127.0.0.1:<port>` or `localhost:<port>` (any case).
#[must_use]
pub fn host_allowed(host: &str, port: u16) -> bool {
    let host = host.to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

/// Whether an `Origin` is `http://127.0.0.1` or `http://localhost`, with any port and nothing else.
#[must_use]
pub fn origin_allowed(origin: &str) -> bool {
    let Some(rest) = origin.strip_prefix("http://") else {
        return false;
    };
    let (name, port) = match rest.split_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (rest, None),
    };
    let port_ok = port.is_none_or(|port| {
        !port.is_empty() && port.len() <= 5 && port.bytes().all(|byte| byte.is_ascii_digit())
    });
    port_ok && (name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost"))
}

/// The bearer token of an `Authorization` header.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer")
        && !token.is_empty()
        && token.chars().count() <= MAX_TOKEN_CHARS
        && !token.chars().any(char::is_whitespace))
    .then(|| token.to_owned())
}

/// Whether the content type is `application/json` (parameters allowed).
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

/// The one handler: every request, whatever its path and method.
async fn handle(State(state): State<Arc<McpState>>, request: HttpRequest) -> HttpResponse {
    let response = respond(&state, request).await;
    tracing::debug!(status = response.status().as_u16(), "mcp request");
    response
}

async fn respond(state: &McpState, request: HttpRequest) -> HttpResponse {
    if !state.active.load(Ordering::SeqCst) {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            "the MCP server is not running",
        );
    }
    let host_ok = request_host(&request).is_some_and(|host| host_allowed(&host, state.port));
    if !host_ok {
        return reply(StatusCode::FORBIDDEN, "unexpected host");
    }
    if let Some(origin) = request.headers().get(header::ORIGIN)
        && !origin.to_str().is_ok_and(origin_allowed)
    {
        return reply(StatusCode::FORBIDDEN, "unexpected origin");
    }
    if request.uri().path() != "/mcp" {
        return reply(StatusCode::NOT_FOUND, "not found");
    }
    if request.method() != Method::POST {
        let mut response = reply(StatusCode::METHOD_NOT_ALLOWED, "use POST");
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return response;
    }
    let Some(token) = bearer(request.headers()) else {
        return unauthorized();
    };
    if !is_json(request.headers()) {
        return reply(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the content type must be application/json",
        );
    }
    let too_large = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_BODY_BYTES);
    if too_large {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "the message is too large");
    }
    let body = request.into_body();
    let bytes = match tokio::time::timeout(BODY_TIMEOUT, axum::body::to_bytes(body, MAX_BODY_BYTES))
        .await
    {
        Err(_) => return reply(StatusCode::REQUEST_TIMEOUT, "the message did not arrive"),
        Ok(Err(_)) => return reply(StatusCode::PAYLOAD_TOO_LARGE, "the message is too large"),
        Ok(Ok(bytes)) => bytes,
    };
    let Ok(message) = String::from_utf8(bytes.to_vec()) else {
        return reply(StatusCode::BAD_REQUEST, "the message is not UTF-8 text");
    };
    let Some(host) = state.host.upgrade() else {
        return reply(StatusCode::SERVICE_UNAVAILABLE, "the app is closing");
    };
    let outcome = blocking(
        move || host.dispatch(Request::McpCall { token, message }),
        MCP_STOPPED,
    )
    .await;
    match outcome {
        Ok(Response::McpReply(answer)) => match answer.body {
            Some(body) => (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "application/json"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                Body::from(body),
            )
                .into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
        Ok(_) => reply(StatusCode::INTERNAL_SERVER_ERROR, "unexpected answer"),
        Err(error) => match error.code {
            ErrorCode::InvalidInput if field_of(&error) == Some("token") => unauthorized(),
            // The core's own limit on a message is the same 1 MiB.
            ErrorCode::InvalidInput if field_of(&error) == Some("message") => {
                reply(StatusCode::PAYLOAD_TOO_LARGE, "the message is too large")
            }
            ErrorCode::Locked => reply(StatusCode::SERVICE_UNAVAILABLE, "the workspace is locked"),
            ErrorCode::Internal if error.message == "core is starting" => {
                reply(StatusCode::SERVICE_UNAVAILABLE, "the app is starting")
            }
            _ => reply(StatusCode::INTERNAL_SERVER_ERROR, "the request failed"),
        },
    }
}

/// `details.field` of an error.
fn field_of(error: &ApiError) -> Option<&str> {
    error.details.as_ref()?.get("field")?.as_str()
}

// --- the listener ---

/// A bound listener and the task serving it. Dropping it refuses what is in flight from then on and
/// closes the port (once the runtime has run the cancelled task).
struct Running {
    /// The port the settings asked for.
    requested: u16,
    /// The port that is bound (the same, except in the tests that ask for any).
    port: u16,
    active: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    /// Stops serving and waits until the port is free.
    async fn stop(mut self) {
        self.active.store(false, Ordering::SeqCst);
        self.task.abort();
        let _ = (&mut self.task).await;
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
        self.task.abort();
    }
}

/// Binds `127.0.0.1:<port>` and serves `host` there. `port` 0 asks the system for one (tests).
async fn start(host: Weak<dyn CoreHost>, requested: u16) -> std::io::Result<Running> {
    let listener = TcpListener::bind(("127.0.0.1", requested)).await?;
    let port = listener.local_addr()?.port();
    let active = Arc::new(AtomicBool::new(true));
    let state = Arc::new(McpState {
        host,
        port,
        active: Arc::clone(&active),
    });
    let app = router(state);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::warn!(%error, "the MCP listener stopped");
        }
    });
    Ok(Running {
        requested,
        port,
        active,
        task,
    })
}

/// The listener a setting asks for: none when MCP is off.
async fn follow_settings(
    host: &Weak<dyn CoreHost>,
    shell: &McpShell,
    server: &mut Option<Running>,
    settings: McpSettings,
) {
    if !settings.enabled {
        if let Some(old) = server.take() {
            old.stop().await;
        }
        shell.set(Listening::default());
        return;
    }
    if settings.port < MIN_PORT {
        if let Some(old) = server.take() {
            old.stop().await;
        }
        shell.set(Listening {
            running: false,
            port: None,
            error: Some(format!("the port must be between {MIN_PORT} and 65535")),
        });
        return;
    }
    if server
        .as_ref()
        .is_some_and(|running| running.requested == settings.port)
    {
        return;
    }
    if let Some(old) = server.take() {
        old.stop().await;
    }
    match start(host.clone(), settings.port).await {
        Ok(running) => {
            tracing::info!("the MCP server is listening");
            shell.set(Listening {
                running: true,
                port: Some(running.port),
                error: None,
            });
            *server = Some(running);
        }
        Err(error) => {
            tracing::info!(kind = ?error.kind(), "the MCP server could not bind its port");
            shell.set(Listening {
                running: false,
                port: None,
                error: Some("the port is in use or cannot be opened".to_owned()),
            });
        }
    }
}

/// Keeps the listener in line with the settings until the task is dropped.
async fn supervise(
    host: Weak<dyn CoreHost>,
    shell: Arc<McpShell>,
    wake: Arc<Notify>,
    retry: Duration,
) {
    let mut server: Option<Running> = None;
    loop {
        let Some(strong) = host.upgrade() else {
            return;
        };
        let settings = core(&strong, read_mcp_settings).await;
        drop(strong);
        let mut failed = false;
        if let Ok(settings) = settings {
            follow_settings(&host, &shell, &mut server, settings).await;
            failed = settings.enabled && server.is_none();
        }
        if failed {
            tokio::select! {
                () = wake.notified() => {}
                () = tokio::time::sleep(retry) => {}
            }
        } else {
            wake.notified().await;
        }
    }
}

/// The running supervisor: dropping it stops it, which closes the listener.
struct McpTask {
    handle: tauri::async_runtime::JoinHandle<()>,
    unlisten: Option<Box<dyn FnOnce() + Send>>,
    shell: Arc<McpShell>,
}

impl Drop for McpTask {
    fn drop(&mut self) {
        self.handle.abort();
        if let Some(unlisten) = self.unlisten.take() {
            unlisten();
        }
        self.shell.set(Listening::default());
    }
}

/// The background work `lib.rs` registers next to the sync driver: runs the listener while the core is
/// ready. It holds the state only weakly: when the state is gone the task ends.
pub fn mcp_starter(host: Weak<dyn CoreHost>, shell: Arc<McpShell>) -> BackgroundStarter {
    mcp_starter_with(host, shell, BIND_RETRY)
}

/// [`mcp_starter`] with another retry interval for a port that could not be bound (the tests').
pub fn mcp_starter_with(
    host: Weak<dyn CoreHost>,
    shell: Arc<McpShell>,
    retry: Duration,
) -> BackgroundStarter {
    Box::new(move || {
        let strong = host.upgrade()?;
        let wake = Arc::new(Notify::new());
        let listener = Arc::clone(&wake);
        let unlisten = strong.add_listener(Arc::new(move |event: &Event| {
            if let Event::SettingChanged { key } = event
                && key == MCP_SETTINGS_SETTING
            {
                listener.notify_one();
            }
        }));
        drop(strong);
        let handle =
            tauri::async_runtime::spawn(supervise(host.clone(), Arc::clone(&shell), wake, retry));
        let task: BackgroundTask = Box::new(McpTask {
            handle,
            unlisten: Some(unlisten),
            shell: Arc::clone(&shell),
        });
        Some(task)
    })
}

#[cfg(test)]
mod tests;
