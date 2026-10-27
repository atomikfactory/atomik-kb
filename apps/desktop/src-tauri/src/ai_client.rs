//! The shell's HTTP client for Ollama (ADR-0037, `docs/architecture/ai.md`, section 6.1). Every call
//! the AI features make to Ollama goes through here, from the shell and never from the webview (its CSP
//! allows no network).
//!
//! - **The client** has no proxy (`no_proxy()`: the environment's proxy settings must never catch a
//!   loopback address, and a remote Ollama is an explicit choice of the person), no cookies, no
//!   redirects, no `Referer`, the user agent `atomik-kb/<version>` and a connect timeout of 5 s. Plain
//!   calls (`embed`, a chat that is not streamed) time out at 300 s, since the first call loads a model;
//!   the metadata calls (`version`, `tags`, `show`) at 30 s. A streamed chat waits at most 300 s for the
//!   first byte and 120 s between two chunks.
//! - **The models** (`models`, `traits_of`): `/api/tags` lists them, and `/api/show` says what each one
//!   can do (`capabilities`) and how long a context it has, when the tags do not. The answers of `show`
//!   are cached per endpoint for 60 s; the list itself is read every time.
//! - **Errors** are `InvalidInput` for the field `endpoint` with `details.reason`: `unreachable`
//!   (connection refused, a timeout, a connection that broke), `status` (Ollama or something else
//!   answered with an HTTP error: `details.status`), `model` (Ollama says the model is not installed:
//!   `details.model`), `remote` (Ollama's own error text, at most 300 characters; `details.status` when
//!   it came with an HTTP error) or `cancelled`.
//! - **Logging** says the outcome, the model, sizes and durations. **Never** the prompt, the answer, the
//!   text of a page, the endpoint's host or port (`reqwest`'s own errors name the URL, so they are never
//!   formatted: only what kind of failure it was is kept).
//!
//! Nothing here touches the core: callers dispatch their core requests, release the lock, and only then
//! come here.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use kb_ai::ollama::{
    AiWireError, ChatChunk, ChatRequest, EmbedRequest, NdjsonDecoder, VersionResponse,
    context_length_of_show, parse_chat_chunk, parse_embed, parse_tags,
};
use kb_ai::prompts::ModelTraits;
use kb_types::ApiError;
use kb_types::ai::AiModelInfo;
use reqwest::Client;
use reqwest::redirect::Policy;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::watch;

/// The field of every error of this module.
pub const ENDPOINT_FIELD: &str = "endpoint";

/// What the shell calls itself to Ollama: `atomik-kb/<version>`.
const USER_AGENT: &str = concat!("atomik-kb/", env!("CARGO_PKG_VERSION"));

/// The longest error text kept from Ollama, in characters.
const MAX_ERROR_CHARS: usize = 300;

/// The most of an error body that is read.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// The most of a successful plain answer that is read (an embedding batch of 16 chunks of a
/// 8,192-dimension model is about 2 MiB of JSON).
const MAX_PLAIN_BODY_BYTES: usize = 32 * 1024 * 1024;

/// The context length assumed for a model whose `show` does not say (`kb_ai::prompts::ModelTraits`).
const DEFAULT_NUM_CTX_MAX: u32 = 16_384;

/// What a model's `/api/show` is cached for.
const SHOW_CACHE_TTL: Duration = Duration::from_secs(60);

/// How long each kind of call may take.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// Connecting (DNS and TCP; TLS for a remote `https` host).
    pub connect: Duration,
    /// A call that is not streamed and may load a model: `embed`, a transcription.
    pub plain: Duration,
    /// The metadata calls: `version`, `tags`, `show`.
    pub meta: Duration,
    /// A streamed chat: the wait for the first byte (the model may have to load).
    pub first_byte: Duration,
    /// A streamed chat: the wait between two chunks.
    pub between_chunks: Duration,
    /// How long the answers of `show` are kept.
    pub show_cache: Duration,
}

impl Timeouts {
    /// 5 s, 300 s, 30 s, 300 s, 120 s and a cache of 60 s.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            connect: Duration::from_secs(5),
            plain: Duration::from_secs(300),
            meta: Duration::from_secs(30),
            first_byte: Duration::from_secs(300),
            between_chunks: Duration::from_secs(120),
            show_cache: SHOW_CACHE_TTL,
        }
    }
}

// --- errors ---

/// Why a call to Ollama failed (`details.reason` of the `ApiError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Nothing answered: refused, timed out or the connection broke.
    Unreachable,
    /// An HTTP error that says nothing more.
    Status,
    /// The model is not installed.
    Model,
    /// Ollama's own error text.
    Remote,
    /// The person (or a lock) stopped it.
    Cancelled,
    /// The endpoint is remote and the person has not agreed to send text there.
    Consent,
    /// The person said no in the native dialog.
    Declined,
}

impl Reason {
    /// The word that goes in `details.reason`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Status => "status",
            Self::Model => "model",
            Self::Remote => "remote",
            Self::Cancelled => "cancelled",
            Self::Consent => "consent",
            Self::Declined => "declined",
        }
    }
}

/// `InvalidInput` for `field` with `details.reason`.
#[must_use]
pub fn failure_for(field: &str, reason: Reason, message: &str) -> ApiError {
    ApiError::invalid(field, message)
        .with_details(json!({ "field": field, "reason": reason.as_str() }))
}

/// `InvalidInput` for `endpoint` with `details.reason`.
fn failure(reason: Reason, message: &str) -> ApiError {
    failure_for(ENDPOINT_FIELD, reason, message)
}

/// Adds one member to the `details` of an error.
fn with_detail(mut error: ApiError, key: &str, value: Value) -> ApiError {
    if let Some(Value::Object(details)) = error.details.as_mut() {
        details.insert(key.to_owned(), value);
    }
    error
}

/// Nothing answered.
#[must_use]
pub fn unreachable_error() -> ApiError {
    failure(Reason::Unreachable, "Ollama could not be reached")
}

/// An HTTP error with nothing more to say.
#[must_use]
pub fn status_error(code: u16) -> ApiError {
    with_detail(
        failure(
            Reason::Status,
            &format!("Ollama answered with an error (HTTP {code})"),
        ),
        "status",
        json!(code),
    )
}

/// The model is not installed.
#[must_use]
pub fn model_error(model: &str) -> ApiError {
    with_detail(
        failure(Reason::Model, "the model is not installed in Ollama"),
        "model",
        json!(model),
    )
}

/// Ollama's own error text (cut to 300 characters), with the HTTP status when it had one.
#[must_use]
pub fn remote_error(text: &str, status: Option<u16>) -> ApiError {
    let error = failure(
        Reason::Remote,
        &truncate_chars(text.trim(), MAX_ERROR_CHARS),
    );
    match status {
        Some(code) => with_detail(error, "status", json!(code)),
        None => error,
    }
}

/// The stream was stopped.
#[must_use]
pub fn cancelled_error() -> ApiError {
    failure(Reason::Cancelled, "cancelled")
}

/// The endpoint is remote and has no consent.
#[must_use]
pub fn consent_error() -> ApiError {
    failure(Reason::Consent, "consent required")
}

/// The `details.reason` of an error, `"none"` when it has none (for the log).
#[must_use]
pub fn reason_of(error: &ApiError) -> &str {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or("none")
}

/// The first `max` characters of `text`.
fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A failure of `reqwest` as an error. `reqwest`'s own messages name the URL, so they are never
/// formatted and only the kind of failure is logged.
fn transport_failure(error: &reqwest::Error) -> ApiError {
    tracing::debug!(
        connect = error.is_connect(),
        timeout = error.is_timeout(),
        "a call to Ollama failed"
    );
    unreachable_error()
}

/// An answer that could not be read, as an error.
fn wire_failure(error: AiWireError) -> ApiError {
    match error {
        AiWireError::Remote(text) => remote_error(&text, None),
        AiWireError::Malformed | AiWireError::LineTooLong => {
            remote_error("Ollama answered something unexpected", None)
        }
    }
}

/// The `error` text of an Ollama error body (`{"error": "..."}`), when there is one.
fn error_text(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let text = value.get("error")?.as_str()?.trim();
    (!text.is_empty()).then(|| truncate_chars(text, MAX_ERROR_CHARS))
}

/// What an HTTP error means: a 404 about a missing model is `model` (when the caller named one), any
/// other error with Ollama's text is `remote`, and one without is `status`.
fn http_failure(code: u16, body: &[u8], model: Option<&str>) -> ApiError {
    match (error_text(body), model) {
        (Some(text), Some(model)) if code == 404 && text.to_lowercase().contains("not found") => {
            model_error(model)
        }
        (Some(text), _) => remote_error(&text, Some(code)),
        (None, _) => status_error(code),
    }
}

// --- cancellation ---

/// The half that cancels a stream.
#[derive(Debug)]
pub struct CancelSource(watch::Sender<bool>);

/// The half a stream watches.
#[derive(Debug, Clone)]
pub struct CancelToken(watch::Receiver<bool>);

/// A cancellation switch and the token that watches it.
#[must_use]
pub fn cancel_pair() -> (CancelSource, CancelToken) {
    let (sender, receiver) = watch::channel(false);
    (CancelSource(sender), CancelToken(receiver))
}

impl CancelSource {
    /// Cancels; whoever watches the token finds out at its next wait.
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }
}

impl CancelToken {
    /// Whether the stream was cancelled already.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves when the stream is cancelled; never when the switch is dropped without cancelling.
    pub async fn cancelled(&mut self) {
        loop {
            if *self.0.borrow_and_update() {
                return;
            }
            if self.0.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

// --- the client ---

/// What `/api/show` said about a model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShowInfo {
    /// Ollama's capabilities (`completion`, `embedding`, `audio`, `vision`, `thinking`, `tools`), when
    /// the answer lists them.
    pub capabilities: Option<Vec<String>>,
    /// The context length in tokens, when it is there.
    pub context_length: Option<u32>,
}

/// The cached `show` answers of one endpoint.
struct ShowCache {
    endpoint: String,
    entries: HashMap<String, (Instant, ShowInfo)>,
}

/// The client of the shell's AI features. One per app, shared behind an `Arc`.
pub struct AiClient {
    http: Option<Client>,
    timeouts: Timeouts,
    shows: Mutex<Option<ShowCache>>,
}

impl Default for AiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AiClient {
    /// The production client ([`Timeouts::production`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_timeouts(Timeouts::production())
    }

    /// A client with other timeouts (the tests use short ones).
    #[must_use]
    pub fn with_timeouts(timeouts: Timeouts) -> Self {
        let http = Client::builder()
            // The environment's proxy must never catch a loopback address (it would send the text of
            // the person's pages through whatever the machine is configured with).
            .no_proxy()
            .user_agent(USER_AGENT)
            .redirect(Policy::none())
            .referer(false)
            .connect_timeout(timeouts.connect)
            .build()
            .inspect_err(|_| {
                tracing::error!("the HTTP client of the AI features could not be built")
            })
            .ok();
        Self {
            http,
            timeouts,
            shows: Mutex::new(None),
        }
    }

    /// The timeouts in force.
    #[must_use]
    pub const fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    fn http(&self) -> Result<&Client, ApiError> {
        self.http
            .as_ref()
            .ok_or_else(|| ApiError::internal("the AI client could not be set up"))
    }

    /// Forgets what `show` said (the endpoint changed).
    pub fn forget_models(&self) {
        *self.shows.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    // --- plain calls ---

    /// A `GET` answered with 2xx, as bytes.
    async fn get_bytes(&self, url: String, timeout: Duration) -> Result<Vec<u8>, ApiError> {
        let response = self
            .http()?
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|error| transport_failure(&error))?;
        Self::body_of(response, None, MAX_PLAIN_BODY_BYTES).await
    }

    /// A `POST` of JSON answered with 2xx, as bytes. `model` names the model a 404 would be about.
    async fn post_json<B: Serialize + Sync>(
        &self,
        url: String,
        body: &B,
        timeout: Duration,
        model: Option<&str>,
    ) -> Result<Vec<u8>, ApiError> {
        let response = self
            .http()?
            .post(url)
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|error| transport_failure(&error))?;
        Self::body_of(response, model, MAX_PLAIN_BODY_BYTES).await
    }

    /// The body of a 2xx answer, or the error an HTTP error makes.
    async fn body_of(
        response: reqwest::Response,
        model: Option<&str>,
        max: usize,
    ) -> Result<Vec<u8>, ApiError> {
        let code = response.status();
        if !code.is_success() {
            let body = read_limited(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            return Err(http_failure(code.as_u16(), &body, model));
        }
        read_limited(response, max).await
    }

    /// Ollama's version (`GET /api/version`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `endpoint`, see the module documentation.
    pub async fn version(&self, endpoint: &str) -> Result<String, ApiError> {
        let body = self
            .get_bytes(format!("{endpoint}/api/version"), self.timeouts.meta)
            .await?;
        serde_json::from_slice::<VersionResponse>(&body)
            .map(|answer| answer.version)
            .map_err(|_| wire_failure(AiWireError::Malformed))
    }

    /// The installed models with their capabilities: `/api/tags` for the list (read every time) and
    /// `/api/show` for what each can do, from the cache when it is under a minute old. A model whose
    /// `show` fails keeps the capabilities the tags (or its name) imply.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `endpoint` when the list cannot be read.
    pub async fn models(&self, endpoint: &str) -> Result<Vec<AiModelInfo>, ApiError> {
        let body = self
            .get_bytes(format!("{endpoint}/api/tags"), self.timeouts.meta)
            .await?;
        let text = String::from_utf8_lossy(&body);
        let mut models = parse_tags(&text).map_err(wire_failure)?;
        for model in &mut models {
            if let Ok(info) = self.show_info(endpoint, &model.name).await
                && let Some(capabilities) = info.capabilities
            {
                model.capabilities = capabilities;
            }
        }
        Ok(models)
    }

    /// What `/api/show` says about `model`, from the cache when it is fresh.
    ///
    /// # Errors
    ///
    /// `model` when Ollama has no such model, the other reasons for the other failures.
    pub async fn show_info(&self, endpoint: &str, model: &str) -> Result<ShowInfo, ApiError> {
        if let Some(info) = self.cached_show(endpoint, model) {
            return Ok(info);
        }
        let body = self
            .post_json(
                format!("{endpoint}/api/show"),
                &json!({ "model": model }),
                self.timeouts.meta,
                Some(model),
            )
            .await?;
        let text = String::from_utf8_lossy(&body);
        let info = ShowInfo {
            capabilities: capabilities_of_show(&text),
            context_length: context_length_of_show(&text),
        };
        self.remember_show(endpoint, model, &info);
        Ok(info)
    }

    fn cached_show(&self, endpoint: &str, model: &str) -> Option<ShowInfo> {
        let shows = self.shows.lock().unwrap_or_else(PoisonError::into_inner);
        let cache = shows.as_ref().filter(|cache| cache.endpoint == endpoint)?;
        let (at, info) = cache.entries.get(model)?;
        (at.elapsed() < self.timeouts.show_cache).then(|| info.clone())
    }

    fn remember_show(&self, endpoint: &str, model: &str, info: &ShowInfo) {
        let mut shows = self.shows.lock().unwrap_or_else(PoisonError::into_inner);
        let cache = match shows.as_mut() {
            Some(cache) if cache.endpoint == endpoint => cache,
            _ => shows.insert(ShowCache {
                endpoint: endpoint.to_owned(),
                entries: HashMap::new(),
            }),
        };
        cache
            .entries
            .insert(model.to_owned(), (Instant::now(), info.clone()));
    }

    /// What the prompt builders need to know of `model`: whether it thinks and how long a context it
    /// has. A model Ollama does not have, or an Ollama that cannot be reached, is the error; an Ollama
    /// that cannot say (an old one, an odd answer) gives the defaults.
    ///
    /// # Errors
    ///
    /// `model` or `unreachable`.
    pub async fn traits_of(&self, endpoint: &str, model: &str) -> Result<ModelTraits, ApiError> {
        match self.show_info(endpoint, model).await {
            Ok(info) => Ok(ModelTraits {
                thinking: info
                    .capabilities
                    .as_ref()
                    .is_some_and(|all| all.iter().any(|name| name == "thinking")),
                num_ctx_max: info.context_length.unwrap_or(DEFAULT_NUM_CTX_MAX),
            }),
            Err(error) if matches!(reason_of(&error), "model" | "unreachable") => Err(error),
            Err(_) => Ok(ModelTraits {
                thinking: false,
                num_ctx_max: DEFAULT_NUM_CTX_MAX,
            }),
        }
    }

    /// Embeds `inputs` (the model's prefixes already applied) with `/api/embed`: one vector per input,
    /// in order. `timeout` replaces the plain-call timeout (a search gives up sooner than the indexer).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `endpoint`; a count of vectors that is not the count of inputs is `remote`.
    pub async fn embed(
        &self,
        endpoint: &str,
        model: &str,
        inputs: Vec<String>,
        timeout: Option<Duration>,
    ) -> Result<Vec<Vec<f32>>, ApiError> {
        let started = Instant::now();
        let count = inputs.len();
        let request = EmbedRequest {
            model: model.to_owned(),
            input: inputs,
            truncate: true,
        };
        let body = self
            .post_json(
                format!("{endpoint}/api/embed"),
                &request,
                timeout.unwrap_or(self.timeouts.plain),
                Some(model),
            )
            .await;
        let outcome = body.and_then(|body| {
            let vectors = parse_embed(&String::from_utf8_lossy(&body)).map_err(wire_failure)?;
            if vectors.len() == count {
                Ok(vectors)
            } else {
                Err(remote_error("Ollama answered something unexpected", None))
            }
        });
        match &outcome {
            Ok(_) => tracing::debug!(
                model,
                inputs = count,
                ms = elapsed_ms(started),
                "ollama embed"
            ),
            Err(error) => tracing::debug!(
                model,
                inputs = count,
                reason = reason_of(error),
                "ollama embed failed"
            ),
        }
        outcome
    }

    /// A chat that is not streamed (the transcription): the one chunk Ollama answers with.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `endpoint`.
    pub async fn chat_once(
        &self,
        endpoint: &str,
        request: &ChatRequest,
    ) -> Result<ChatChunk, ApiError> {
        let started = Instant::now();
        let body = self
            .post_json(
                format!("{endpoint}/api/chat"),
                request,
                self.timeouts.plain,
                Some(&request.model),
            )
            .await;
        let outcome = body.and_then(|body| {
            parse_chat_chunk(String::from_utf8_lossy(&body).trim()).map_err(wire_failure)
        });
        match &outcome {
            Ok(chunk) => tracing::debug!(
                model = request.model.as_str(),
                chars = chunk.content.chars().count(),
                ms = elapsed_ms(started),
                "ollama chat"
            ),
            Err(error) => tracing::debug!(
                model = request.model.as_str(),
                reason = reason_of(error),
                "ollama chat failed"
            ),
        }
        outcome
    }

    // --- the stream ---

    /// A streamed chat. `on_delta` receives each piece of the answer as it arrives (a `false` from it
    /// means nobody is listening any more: the stream is dropped and ends as cancelled); the whole
    /// answer is returned. Waits at most `first_byte` for Ollama to start and `between_chunks` for the
    /// next chunk. Dropping the response closes the connection, which makes Ollama stop generating.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `endpoint`: `cancelled` when `cancel` fired, `remote` for an error line (or a
    /// stream that ended before its last chunk), the other reasons for the other failures.
    pub async fn stream_chat(
        &self,
        endpoint: &str,
        request: &ChatRequest,
        cancel: &mut CancelToken,
        on_delta: &mut (dyn FnMut(&str) -> bool + Send),
    ) -> Result<String, ApiError> {
        let started = Instant::now();
        let outcome = self
            .stream_chat_inner(endpoint, request, cancel, on_delta)
            .await;
        match &outcome {
            Ok(text) => tracing::debug!(
                model = request.model.as_str(),
                chars = text.chars().count(),
                ms = elapsed_ms(started),
                "ollama chat stream"
            ),
            Err(error) => tracing::debug!(
                model = request.model.as_str(),
                reason = reason_of(error),
                ms = elapsed_ms(started),
                "ollama chat stream failed"
            ),
        }
        outcome
    }

    async fn stream_chat_inner(
        &self,
        endpoint: &str,
        request: &ChatRequest,
        cancel: &mut CancelToken,
        on_delta: &mut (dyn FnMut(&str) -> bool + Send),
    ) -> Result<String, ApiError> {
        let sending = self
            .http()?
            .post(format!("{endpoint}/api/chat"))
            .json(request)
            .send();
        let mut response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(cancelled_error()),
            sent = tokio::time::timeout(self.timeouts.first_byte, sending) => match sent {
                Err(_) => return Err(unreachable_error()),
                Ok(Err(error)) => return Err(transport_failure(&error)),
                Ok(Ok(response)) => response,
            },
        };
        if !response.status().is_success() {
            let code = response.status().as_u16();
            let body = read_limited(response, MAX_ERROR_BODY_BYTES)
                .await
                .unwrap_or_default();
            return Err(http_failure(code, &body, Some(&request.model)));
        }
        let mut decoder = NdjsonDecoder::new();
        let mut text = String::new();
        let mut finished = false;
        loop {
            let piece = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(cancelled_error()),
                next = tokio::time::timeout(self.timeouts.between_chunks, response.chunk()) => match next {
                    Err(_) => return Err(unreachable_error()),
                    Ok(Err(error)) => return Err(transport_failure(&error)),
                    Ok(Ok(piece)) => piece,
                },
            };
            let Some(piece) = piece else {
                break;
            };
            decoder.push(&piece).map_err(wire_failure)?;
            while let Some(line) = decoder.next_line().map_err(wire_failure)? {
                absorb(&line, &mut text, &mut finished, on_delta)?;
            }
        }
        if let Some(line) = decoder.finish().map_err(wire_failure)? {
            absorb(&line, &mut text, &mut finished, on_delta)?;
        }
        if finished {
            Ok(text)
        } else {
            Err(remote_error("the model stopped before it finished", None))
        }
    }
}

/// Reads one line of a chat stream into the answer so far.
fn absorb(
    line: &str,
    text: &mut String,
    finished: &mut bool,
    on_delta: &mut (dyn FnMut(&str) -> bool + Send),
) -> Result<(), ApiError> {
    let chunk = parse_chat_chunk(line).map_err(wire_failure)?;
    if !chunk.content.is_empty() {
        text.push_str(&chunk.content);
        if !on_delta(&chunk.content) {
            return Err(cancelled_error());
        }
    }
    if chunk.done {
        *finished = true;
    }
    Ok(())
}

/// The capabilities `/api/show` lists, when it does.
fn capabilities_of_show(body: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let list = value.get("capabilities")?.as_array()?;
    Some(
        list.iter()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect(),
    )
}

/// The body of an answer, at most `max` bytes (more is an error).
async fn read_limited(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    while let Some(piece) = response
        .chunk()
        .await
        .map_err(|error| transport_failure(&error))?
    {
        if out.len().saturating_add(piece.len()) > max {
            return Err(remote_error("Ollama's answer is too large", None));
        }
        out.extend_from_slice(&piece);
    }
    Ok(out)
}

/// Milliseconds since `started`, saturating.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
