//! The AI commands of the shell (ADR-0037, `docs/architecture/ai.md`, sections 2 and 6): the
//! endpoint and its consent, the status, the writing assistant and the chat (streamed), the related
//! search and the transcription. The commands in `lib.rs` are thin; the rules are here.
//!
//! **The core lock is never held while the network is used.** Every core request is dispatched on a
//! blocking thread and answered before any call to Ollama starts (`core`), and the stream of a
//! generation holds nothing but its own state.
//!
//! - **AI off.** While `AiSettings.enabled` is false, `ai_generate`, `ai_chat`, `ai_search` and
//!   `ai_transcribe` answer `InvalidInput` for the field `enabled` ("AI is turned off") before anything
//!   else; `ai_status` and `ai_endpoint_set` work regardless. Every call also checks the consent of the
//!   stored endpoint (`remoteConsent` or a local host), else `InvalidInput` for `endpoint`
//!   ("consent required", `details.reason = "consent"`).
//! - **Endpoint** (`ai_endpoint_set`): a local URL is stored at once with no consent; a remote one first
//!   shows a native dialog ([`Confirm`]) with the host in it, and only "Allow" stores it with
//!   `remoteConsent: true`; "Cancel" is `InvalidInput` for `url` with `details.reason = "declined"` and
//!   nothing changes. The same remote URL that already has consent is not asked again; any other URL is
//!   a new question. The setting `app.ai.endpoint` is written only here (the webview's `SettingsSet` of
//!   it is refused by `guard`).
//! - **Streams.** `ai_generate` and `ai_chat` take a `requestId` (1 to 64 of `[A-Za-z0-9_-]`) and a
//!   channel. They answer `Err` only when nothing was started (a bad or busy `requestId`, AI off, no
//!   consent, no model chosen, a core that is locked or starting); from then on every outcome is an
//!   event on the channel (`sources`, `delta`*, then `done` or `error`) and the command answers `Ok`. At
//!   most four streams run at once (`InvalidInput` for `requestId`, reason `busy`); `ai_cancel` and a
//!   lock cancel them (the last event is `error` with reason `cancelled`); a channel that cannot be
//!   reached any more ends its stream too.
//! - **Nothing sensitive is logged**: the kind of request, the outcome, the model, sizes in characters
//!   and durations.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use kb_ai::chunk::{EmbedRole, embedding_input};
use kb_ai::endpoint;
use kb_ai::prompts::{
    MAX_TEXT_CHARS, MERGE_SUMMARIES_PROMPT, ModelTraits, build_chat, build_generate,
    build_transcription, clean_output, retrieval_query, summary_parts,
};
use kb_ai::transcript::{clean_transcript, is_pcm_wav};
use kb_types::ai::{
    AI_ENDPOINT_SETTING, AI_SETTINGS_SETTING, AiAction, AiChatInput, AiEndpoint, AiGenerateInput,
    AiIndexerState, AiScope, AiSettings, AiSource, AiStatus, AiStreamEvent, AiTranscribed,
    DEFAULT_AI_ENDPOINT,
};
use kb_types::{ApiError, Json, PageId, Request, Response};
use serde::Deserialize;
use serde_json::json;
use tauri::http::HeaderMap;

use crate::ai_client::{
    AiClient, CancelSource, CancelToken, Reason, cancel_pair, cancelled_error, consent_error,
    failure_for, reason_of,
};
use crate::dialogs::{Confirm, Question};
use crate::shell_labels::{Labels, fill};
use crate::state::CoreHost;
use crate::{AI_STOPPED, blocking};

/// The request header of `ai_transcribe`: JSON `{ "model"?: string }`.
pub const AI_HEADER: &str = "x-kb-ai";

/// How many streams may run at once.
pub const MAX_STREAMS: usize = 4;

/// The largest WAV segment `ai_transcribe` takes: 4 MiB (about two minutes at 16 kHz mono).
pub const MAX_WAV_BYTES: usize = 4 * 1024 * 1024;

/// How many passages a chat asks the core for.
const RETRIEVE_LIMIT: u32 = 8;

/// The longest query of a retrieval, in characters (the core's limit).
const MAX_QUERY_CHARS: usize = 2_000;

/// A page shorter than this is given to the chat whole, as its first source.
const WHOLE_PAGE_CHARS: usize = 12_000;

/// The longest meeting transcript `meetingSummary` takes, in characters (about four and a half hours of
/// speech); it is summarised in parts.
const MAX_MEETING_CHARS: usize = 250_000;

/// How long a search waits for the embedding of its query before it falls back to the full-text search.
pub const SEARCH_EMBED_TIMEOUT: Duration = Duration::from_secs(15);

/// The longest model name the transcription header may carry.
const MAX_MODEL_CHARS: usize = 200;

/// What a stream of events is sent through; `false` means the receiver is gone.
pub type Emit<'a> = &'a (dyn Fn(AiStreamEvent) -> bool + Send + Sync);

// --- the shell's AI state ---

/// The streams that run, by `requestId`.
#[derive(Default)]
pub struct Streams {
    active: Mutex<HashMap<String, CancelSource>>,
}

impl Streams {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancelSource>> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts tracking a stream: at most [`MAX_STREAMS`], and each id once.
    pub(crate) fn register(self: &Arc<Self>, id: &str) -> Result<StreamGuard, ApiError> {
        let mut active = self.lock();
        if active.contains_key(id) {
            return Err(busy("duplicate", "that request is already running"));
        }
        if active.len() >= MAX_STREAMS {
            return Err(busy("busy", "too many AI requests are running"));
        }
        let (source, token) = cancel_pair();
        active.insert(id.to_owned(), source);
        Ok(StreamGuard {
            streams: Arc::clone(self),
            id: id.to_owned(),
            token,
        })
    }

    /// Cancels one stream; an unknown id is fine.
    pub fn cancel(&self, id: &str) {
        if let Some(source) = self.lock().get(id) {
            source.cancel();
        }
    }

    /// Cancels every stream (the core locked).
    pub fn cancel_all(&self) {
        for source in self.lock().values() {
            source.cancel();
        }
    }

    /// How many streams run.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no stream runs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A stream that is tracked; dropping it forgets the stream.
pub(crate) struct StreamGuard {
    streams: Arc<Streams>,
    id: String,
    pub(crate) token: CancelToken,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.streams.lock().remove(&self.id);
    }
}

fn busy(reason: &str, message: &str) -> ApiError {
    ApiError::invalid("requestId", message)
        .with_details(json!({ "field": "requestId", "reason": reason }))
}

/// What the AI features share: the client, the streams and what the indexer is doing. Managed by Tauri
/// inside an `Arc`; the indexer's background task holds the same one.
pub struct AiShell {
    /// The HTTP client of Ollama.
    pub client: AiClient,
    streams: Arc<Streams>,
    indexer: Mutex<AiIndexerState>,
}

impl Default for AiShell {
    fn default() -> Self {
        Self::new()
    }
}

impl AiShell {
    /// A shell with the production client.
    #[must_use]
    pub fn new() -> Self {
        Self::with_client(AiClient::new())
    }

    /// A shell with another client (the tests').
    #[must_use]
    pub fn with_client(client: AiClient) -> Self {
        Self {
            client,
            streams: Arc::default(),
            indexer: Mutex::new(AiIndexerState::default()),
        }
    }

    /// The streams that run.
    #[must_use]
    pub fn streams(&self) -> &Streams {
        &self.streams
    }

    /// Cancels every stream.
    pub fn cancel_all(&self) {
        self.streams.cancel_all();
    }

    /// Starts tracking a stream (the tests', which have no stream to run).
    #[cfg(test)]
    pub(crate) fn track(&self, id: &str) -> Result<StreamGuard, ApiError> {
        self.streams.register(id)
    }

    /// What the indexer is doing.
    #[must_use]
    pub fn indexer_state(&self) -> AiIndexerState {
        self.indexer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Changes what the indexer reports.
    pub(crate) fn update_indexer(&self, change: impl FnOnce(&mut AiIndexerState)) {
        change(&mut self.indexer.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// The indexer stopped (the core locked): nothing is going on.
    pub(crate) fn reset_indexer(&self) {
        self.update_indexer(|state| *state = AiIndexerState::default());
    }
}

// --- the core, off the async runtime ---

/// Runs `work` on a blocking thread with the shell's handle on the core. The lock is taken and released
/// inside each call `work` makes; nothing here is awaited while it is held.
pub(crate) async fn core<T, F>(host: &Arc<dyn CoreHost>, work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&dyn CoreHost) -> Result<T, ApiError> + Send + 'static,
{
    let host = Arc::clone(host);
    blocking(move || work(&*host), AI_STOPPED).await
}

fn unexpected() -> ApiError {
    ApiError::internal("the core answered with something unexpected")
}

/// A setting's value, `None` when unset.
pub(crate) fn read_setting(host: &dyn CoreHost, key: &str) -> Result<Option<Json>, ApiError> {
    match host.dispatch(Request::SettingsGet {
        key: key.to_owned(),
    })? {
        Response::Setting(value) => Ok(value),
        _ => Err(unexpected()),
    }
}

/// Stores a setting through the shell's own handle on the core (the webview's `dispatch` refuses the
/// keys only the shell writes; this does not go through there).
pub(crate) fn write_setting(host: &dyn CoreHost, key: &str, value: Json) -> Result<(), ApiError> {
    match host.dispatch(Request::SettingsSet {
        key: key.to_owned(),
        value,
    })? {
        Response::Ok => Ok(()),
        _ => Err(unexpected()),
    }
}

/// The AI preferences; the default (AI off) when none are stored or the stored value is not one.
pub(crate) fn read_ai_settings(host: &dyn CoreHost) -> Result<AiSettings, ApiError> {
    Ok(read_setting(host, AI_SETTINGS_SETTING)?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

/// The stored endpoint; the local default with no consent when none is stored or it is not readable.
pub(crate) fn read_endpoint(host: &dyn CoreHost) -> Result<AiEndpoint, ApiError> {
    Ok(read_setting(host, AI_ENDPOINT_SETTING)?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_else(|| AiEndpoint {
            url: DEFAULT_AI_ENDPOINT.to_owned(),
            remote_consent: false,
        }))
}

/// The endpoint's normalised URL when text may be sent there: it is this machine or the person
/// consented to this exact URL.
///
/// # Errors
///
/// `InvalidInput` for `endpoint`: "consent required" (`details.reason = "consent"`), or the stored URL is
/// not an endpoint.
pub fn allowed_url(endpoint: &AiEndpoint) -> Result<String, ApiError> {
    let parsed = endpoint::parse(&endpoint.url)
        .map_err(|error| ApiError::invalid("endpoint", error.to_string()))?;
    if endpoint.remote_consent || parsed.local {
        Ok(parsed.url)
    } else {
        Err(consent_error())
    }
}

/// What every AI call that needs Ollama starts from.
struct Ctx {
    settings: AiSettings,
    /// The normalised, consented endpoint.
    endpoint: String,
}

/// The state check of the AI commands: the core is ready, AI is on and the endpoint may be used.
async fn prepare(host: &Arc<dyn CoreHost>) -> Result<Ctx, ApiError> {
    core(host, |host| {
        host.ensure_ready()?;
        let settings = read_ai_settings(host)?;
        if !settings.enabled {
            return Err(ApiError::invalid("enabled", "AI is turned off"));
        }
        let endpoint = allowed_url(&read_endpoint(host)?)?;
        Ok(Ctx { settings, endpoint })
    })
    .await
}

/// A model name that is set and not blank.
fn chosen(model: Option<&String>) -> Option<String> {
    model
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// No model of this kind is chosen.
fn missing_model(what: &str) -> ApiError {
    ApiError::invalid("model", format!("choose a {what} model in the AI settings"))
        .with_details(json!({ "field": "model", "reason": "missing" }))
}

fn input_error(error: kb_ai::prompts::AiInputError) -> ApiError {
    ApiError::invalid(error.field, error.message)
}

/// `requestId`: 1 to 64 characters of `[A-Za-z0-9_-]`.
fn validate_request_id(id: &str) -> Result<(), ApiError> {
    let good = (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if good {
        Ok(())
    } else {
        Err(ApiError::invalid(
            "requestId",
            "the request id must be 1 to 64 letters, digits, '_' or '-'",
        ))
    }
}

/// Waits for `work`, or stops at once when the stream is cancelled.
async fn cancellable<T>(
    token: &mut CancelToken,
    work: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    tokio::select! {
        biased;
        () = token.cancelled() => Err(cancelled_error()),
        outcome = work => outcome,
    }
}

// --- status and endpoint ---

/// `ai_status`: the endpoint, whether Ollama answers, its models, the semantic index (when the core
/// answers) and the indexer. Works with AI off; makes no call to a remote endpoint that has no consent.
///
/// # Errors
///
/// `Locked`, or `Internal: "core is starting"`, while the core is not ready (the endpoint is stored in
/// it).
pub async fn status(host: Arc<dyn CoreHost>, shell: Arc<AiShell>) -> Result<AiStatus, ApiError> {
    let (stored, index) = core(&host, |host| {
        host.ensure_ready()?;
        let stored = read_endpoint(host)?;
        let index = match host.dispatch(Request::AiIndexStatus) {
            Ok(Response::AiIndex(info)) => Some(info),
            _ => None,
        };
        Ok((stored, index))
    })
    .await?;
    let (shown, local) = match endpoint::parse(&stored.url) {
        Ok(parsed) => (parsed.url, parsed.local),
        Err(_) => (stored.url.clone(), false),
    };
    let mut status = AiStatus {
        endpoint: shown,
        local,
        remote_consent: stored.remote_consent,
        reachable: false,
        version: None,
        models: Vec::new(),
        error: None,
        index,
        indexer: shell.indexer_state(),
    };
    match allowed_url(&stored) {
        Err(error) => status.error = Some(error),
        Ok(url) => match shell.client.version(&url).await {
            Err(error) => status.error = Some(error),
            Ok(version) => {
                status.reachable = true;
                status.version = Some(version);
                match shell.client.models(&url).await {
                    Ok(models) => status.models = models,
                    Err(error) => status.error = Some(error),
                }
            }
        },
    }
    Ok(status)
}

/// The question a remote endpoint has to be answered "Allow" to, in the words of `labels` (the UI's language;
/// the text always names `host`, `shell_labels` refuses a translation that does not).
#[must_use]
pub fn consent_question(labels: &Labels, host: &str) -> Question {
    Question {
        title: labels.dialog_ai_title.clone(),
        message: fill(&labels.dialog_ai_message, &[("host", host)]),
        accept: labels.dialog_allow.clone(),
        cancel: labels.dialog_cancel.clone(),
    }
}

/// Stores the endpoint, asking the person first when it is not this machine (blocking: the native
/// dialog waits for the answer, so this runs on a blocking thread).
///
/// # Errors
///
/// `InvalidInput` for `url` when it is not an endpoint, or declined (`details.reason = "declined"`,
/// nothing changes); `Locked` or "core is starting" while the core is not ready; the core's.
pub(crate) fn store_endpoint(
    host: &dyn CoreHost,
    confirm: &dyn Confirm,
    url: &str,
) -> Result<(), ApiError> {
    host.ensure_ready()?;
    let parsed =
        endpoint::parse(url.trim()).map_err(|error| ApiError::invalid("url", error.to_string()))?;
    let consent = if parsed.local {
        false
    } else {
        let current = read_endpoint(host)?;
        if current.url == parsed.url && current.remote_consent {
            // Consent belongs to this exact URL, and it was given.
            true
        } else if confirm.ask(&consent_question(&confirm.labels(), &parsed.host)) {
            true
        } else {
            return Err(failure_for(
                "url",
                Reason::Declined,
                "the endpoint was not changed",
            ));
        }
    };
    let value = serde_json::to_value(AiEndpoint {
        url: parsed.url,
        remote_consent: consent,
    })
    .map_err(|_| ApiError::internal("the endpoint could not be stored"))?;
    write_setting(host, AI_ENDPOINT_SETTING, value)?;
    tracing::info!(remote = !parsed.local, consent, "AI endpoint stored");
    Ok(())
}

/// `ai_endpoint_set`: stores the endpoint (asking for consent first when it is remote) and answers the
/// new status.
///
/// # Errors
///
/// As [`store_endpoint`].
pub async fn endpoint_set(
    host: Arc<dyn CoreHost>,
    shell: Arc<AiShell>,
    confirm: Arc<dyn Confirm>,
    url: String,
) -> Result<AiStatus, ApiError> {
    core(&host, move |host| store_endpoint(host, &*confirm, &url)).await?;
    shell.client.forget_models();
    status(host, shell).await
}

// --- the writing assistant ---

/// `ai_generate`: runs a writing action and streams its result into `emit`. See the module
/// documentation for what is an `Err` and what is an event.
///
/// # Errors
///
/// `InvalidInput` for `requestId`, `enabled`, `endpoint` or `model`; `Locked` or "core is starting".
pub async fn generate(
    host: Arc<dyn CoreHost>,
    shell: Arc<AiShell>,
    request_id: String,
    input: AiGenerateInput,
    emit: Emit<'_>,
) -> Result<(), ApiError> {
    validate_request_id(&request_id)?;
    let ctx = prepare(&host).await?;
    let model = chosen(ctx.settings.chat_model.as_ref()).ok_or_else(|| missing_model("chat"))?;
    let mut guard = shell.streams.register(&request_id)?;
    let started = Instant::now();
    let outcome = run_generate(&shell.client, &ctx, &model, &input, &mut guard.token, emit).await;
    conclude("generate", &model, started, outcome, emit);
    Ok(())
}

async fn run_generate(
    client: &AiClient,
    ctx: &Ctx,
    model: &str,
    input: &AiGenerateInput,
    token: &mut CancelToken,
    emit: Emit<'_>,
) -> Result<String, ApiError> {
    // A meeting transcript may be longer than any other input: it is cut into parts before the
    // limits of a single call are checked.
    if input.action == AiAction::MeetingSummary && input.text.chars().count() > MAX_MEETING_CHARS {
        return Err(ApiError::invalid("text", "the transcript is too long"));
    }
    let traits = cancellable(token, client.traits_of(&ctx.endpoint, model)).await?;
    if input.action == AiAction::MeetingSummary {
        return meeting_summary(client, ctx, model, traits, input, token, emit).await;
    }
    let request = build_generate(model, traits, ctx.settings.think, input).map_err(input_error)?;
    stream_text(client, &ctx.endpoint, &request, token, Some(emit)).await
}

/// One streamed chat; each piece is sent to `emit` when there is one. The answer is returned cleaned.
async fn stream_text(
    client: &AiClient,
    endpoint: &str,
    request: &kb_ai::ollama::ChatRequest,
    token: &mut CancelToken,
    emit: Option<Emit<'_>>,
) -> Result<String, ApiError> {
    let mut on_delta = |piece: &str| -> bool {
        emit.is_none_or(|emit| {
            emit(AiStreamEvent::Delta {
                text: piece.to_owned(),
            })
        })
    };
    let raw = client
        .stream_chat(endpoint, request, token, &mut on_delta)
        .await?;
    Ok(clean_output(&raw))
}

/// The summary of a meeting transcript. A transcript that fits one call is summarised in one; a longer
/// one is cut into parts at paragraph boundaries, each part is summarised without being streamed (the
/// receiver gets an empty `delta` after each, as progress), and the part summaries are merged by one
/// `custom` call whose text is streamed.
async fn meeting_summary(
    client: &AiClient,
    ctx: &Ctx,
    model: &str,
    traits: ModelTraits,
    input: &AiGenerateInput,
    token: &mut CancelToken,
    emit: Emit<'_>,
) -> Result<String, ApiError> {
    let think = ctx.settings.think;
    let parts = summary_parts(&input.text);
    if parts.len() <= 1 {
        let request = build_generate(model, traits, think, input).map_err(input_error)?;
        return stream_text(client, &ctx.endpoint, &request, token, Some(emit)).await;
    }
    let mut summaries = Vec::with_capacity(parts.len());
    for part in parts {
        let part_input = AiGenerateInput {
            text: part,
            ..input.clone()
        };
        let request = build_generate(model, traits, think, &part_input).map_err(input_error)?;
        summaries.push(stream_text(client, &ctx.endpoint, &request, token, None).await?);
        if !emit(AiStreamEvent::Delta {
            text: String::new(),
        }) {
            return Err(cancelled_error());
        }
    }
    let merge = AiGenerateInput {
        action: AiAction::Custom,
        text: join_summaries(&summaries),
        context: None,
        language: None,
        tone: None,
        prompt: Some(MERGE_SUMMARIES_PROMPT.to_owned()),
    };
    let request = build_generate(model, traits, think, &merge).map_err(input_error)?;
    stream_text(client, &ctx.endpoint, &request, token, Some(emit)).await
}

/// The part summaries as one text for the merge call, each cut so that all of them fit the limit of a
/// writing input.
fn join_summaries(summaries: &[String]) -> String {
    let room = MAX_TEXT_CHARS.saturating_sub(1_000);
    let each = room / summaries.len().max(1);
    summaries
        .iter()
        .enumerate()
        .map(|(index, summary)| {
            let cut: String = summary.chars().take(each).collect();
            format!("### Part {}\n{}", index + 1, cut.trim())
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Sends the last event of a stream and logs its outcome.
fn conclude(
    kind: &'static str,
    model: &str,
    started: Instant,
    outcome: Result<String, ApiError>,
    emit: Emit<'_>,
) {
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(text) => {
            tracing::info!(
                kind,
                model,
                chars = text.chars().count(),
                ms,
                "AI request finished"
            );
            let _ = emit(AiStreamEvent::Done {
                text,
                model: model.to_owned(),
            });
        }
        Err(error) => {
            tracing::info!(kind, model, reason = reason_of(&error), code = ?error.code, ms, "AI request failed");
            let _ = emit(AiStreamEvent::Error { error });
        }
    }
}

// --- the chat and the search ---

/// The query of a retrieval: at most 2,000 characters, and not blank.
fn bounded_query(text: &str) -> Result<String, ApiError> {
    let query: String = text.trim().chars().take(MAX_QUERY_CHARS).collect();
    if query.trim().is_empty() {
        Err(ApiError::invalid("query", "the question is empty"))
    } else {
        Ok(query)
    }
}

/// The embedding of a question, when the index is on and an embedding model is chosen; `None` (so the
/// retrieval is full-text only) when it is not, or when the embedding fails.
async fn embed_query(
    client: &AiClient,
    ctx: &Ctx,
    text: &str,
    timeout: Option<Duration>,
) -> Option<Vec<f32>> {
    if !ctx.settings.semantic_index {
        return None;
    }
    let model = chosen(ctx.settings.embedding_model.as_ref())?;
    let input = embedding_input(&model, text, EmbedRole::Query);
    let vectors = client
        .embed(&ctx.endpoint, &model, vec![input], timeout)
        .await
        .ok()?;
    vectors
        .into_iter()
        .next()
        .filter(|vector| !vector.is_empty())
}

/// `AiRetrieve`: the passages for a question.
async fn retrieve(
    host: &Arc<dyn CoreHost>,
    query: String,
    vector: Option<Vec<f32>>,
    scope: AiScope,
    limit: Option<u32>,
) -> Result<Vec<AiSource>, ApiError> {
    core(host, move |host| {
        match host.dispatch(Request::AiRetrieve {
            query,
            vector,
            scope,
            limit,
        })? {
            Response::AiPassages(sources) => Ok(sources),
            _ => Err(unexpected()),
        }
    })
    .await
}

/// A page shorter than [`WHOLE_PAGE_CHARS`] as the one source of a chat about it; `None` for a longer
/// page (the chat then retrieves passages of it) or an empty one.
async fn whole_page(
    host: &Arc<dyn CoreHost>,
    page_id: PageId,
) -> Result<Option<AiSource>, ApiError> {
    core(host, move |host| {
        let Response::Markdown(text) = host.dispatch(Request::PageExportMarkdown { page_id })?
        else {
            return Err(unexpected());
        };
        if text.trim().is_empty() || text.chars().count() >= WHOLE_PAGE_CHARS {
            return Ok(None);
        }
        let Response::Page(page) = host.dispatch(Request::PageGet { page_id })? else {
            return Err(unexpected());
        };
        Ok(Some(AiSource {
            n: 1,
            page_id,
            space_id: page.space_id,
            block_id: None,
            title: page.title,
            icon: page.icon,
            text,
            score: 1.0,
        }))
    })
    .await
}

/// What a chat answers from: the whole page for a short page in scope `page`, else the passages the
/// core finds for the question (semantic when the question could be embedded, always full-text).
async fn chat_sources(
    host: &Arc<dyn CoreHost>,
    client: &AiClient,
    ctx: &Ctx,
    input: &AiChatInput,
) -> Result<Vec<AiSource>, ApiError> {
    if let AiScope::Page { page_id } = input.scope
        && let Some(source) = whole_page(host, page_id).await?
    {
        return Ok(vec![source]);
    }
    let query = bounded_query(&retrieval_query(input))?;
    let vector = embed_query(client, ctx, &query, None).await;
    retrieve(host, query, vector, input.scope, Some(RETRIEVE_LIMIT)).await
}

/// `ai_chat`: answers the last message of the conversation from the person's pages and streams the
/// answer into `emit` (`sources` first). See the module documentation for what is an `Err`.
///
/// # Errors
///
/// As [`generate`].
pub async fn chat(
    host: Arc<dyn CoreHost>,
    shell: Arc<AiShell>,
    request_id: String,
    input: AiChatInput,
    emit: Emit<'_>,
) -> Result<(), ApiError> {
    validate_request_id(&request_id)?;
    let ctx = prepare(&host).await?;
    let model = chosen(ctx.settings.chat_model.as_ref()).ok_or_else(|| missing_model("chat"))?;
    let mut guard = shell.streams.register(&request_id)?;
    let started = Instant::now();
    let outcome = run_chat(
        &host,
        &shell.client,
        &ctx,
        &model,
        &input,
        &mut guard.token,
        emit,
    )
    .await;
    conclude("chat", &model, started, outcome, emit);
    Ok(())
}

async fn run_chat(
    host: &Arc<dyn CoreHost>,
    client: &AiClient,
    ctx: &Ctx,
    model: &str,
    input: &AiChatInput,
    token: &mut CancelToken,
    emit: Emit<'_>,
) -> Result<String, ApiError> {
    let sources = cancellable(token, chat_sources(host, client, ctx, input)).await?;
    let traits = cancellable(token, client.traits_of(&ctx.endpoint, model)).await?;
    let request =
        build_chat(model, traits, ctx.settings.think, input, &sources).map_err(input_error)?;
    if !emit(AiStreamEvent::Sources { sources }) {
        return Err(cancelled_error());
    }
    stream_text(client, &ctx.endpoint, &request, token, Some(emit)).await
}

/// `ai_cancel`: stops a stream; an id that is not running is fine.
pub fn cancel(shell: &AiShell, request_id: &str) {
    shell.streams.cancel(request_id);
}

/// `ai_search`: related passages for a query, semantic when the query can be embedded in time and the
/// index is on, full-text only otherwise.
///
/// # Errors
///
/// `InvalidInput` for `enabled`, `endpoint` or `query`; `Locked` or "core is starting"; the core's.
pub async fn search(
    host: Arc<dyn CoreHost>,
    shell: Arc<AiShell>,
    query: String,
    scope: AiScope,
    limit: Option<u32>,
) -> Result<Vec<AiSource>, ApiError> {
    let ctx = prepare(&host).await?;
    let query = bounded_query(&query)?;
    let vector = embed_query(&shell.client, &ctx, &query, Some(SEARCH_EMBED_TIMEOUT)).await;
    retrieve(&host, query, vector, scope, limit).await
}

// --- transcription ---

/// The model a `ai_transcribe` header names, when it names one.
///
/// # Errors
///
/// `InvalidInput` for the field `x-kb-ai` when the header is not `{ "model"?: string }` JSON.
pub fn model_from_header(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    #[derive(Deserialize)]
    struct Header {
        model: Option<String>,
    }
    let Some(value) = headers.get(AI_HEADER) else {
        return Ok(None);
    };
    let bad = || {
        ApiError::invalid(
            AI_HEADER,
            "the header must be JSON like {\"model\":\"name\"}",
        )
    };
    let text = value.to_str().map_err(|_| bad())?;
    let parsed: Header = serde_json::from_str(text).map_err(|_| bad())?;
    match parsed.model {
        None => Ok(None),
        Some(model) => {
            let model = model.trim().to_owned();
            if model.chars().count() > MAX_MODEL_CHARS || model.chars().any(char::is_control) {
                return Err(bad());
            }
            Ok((!model.is_empty()).then_some(model))
        }
    }
}

/// The size of the audio of `ai_transcribe`: at most [`MAX_WAV_BYTES`].
///
/// # Errors
///
/// `InvalidInput` for `audio` with `details.limit`.
pub fn check_audio_size(wav: &[u8]) -> Result<(), ApiError> {
    if wav.len() > MAX_WAV_BYTES {
        return Err(ApiError::invalid("audio", "the audio segment is too large")
            .with_details(json!({ "field": "audio", "limit": MAX_WAV_BYTES })));
    }
    Ok(())
}

/// The audio of `ai_transcribe`: a PCM WAV file of at most [`MAX_WAV_BYTES`].
fn check_audio(wav: &[u8]) -> Result<(), ApiError> {
    check_audio_size(wav)?;
    if !is_pcm_wav(wav) {
        return Err(ApiError::invalid(
            "audio",
            "the audio must be a PCM WAV file",
        ));
    }
    Ok(())
}

/// `ai_transcribe`: the speech in one WAV segment, through an audio-capable model. Not streamed.
///
/// # Errors
///
/// `InvalidInput` for `enabled`, `endpoint`, `audio`, or `model` (reason `missing` when none is named
/// by the header or chosen in the settings); Ollama's failures.
pub async fn transcribe(
    host: Arc<dyn CoreHost>,
    shell: Arc<AiShell>,
    wav: Vec<u8>,
    header_model: Option<String>,
) -> Result<AiTranscribed, ApiError> {
    let ctx = prepare(&host).await?;
    check_audio(&wav)?;
    let model = header_model
        .or_else(|| chosen(ctx.settings.transcription_model.as_ref()))
        .ok_or_else(|| missing_model("transcription"))?;
    let started = Instant::now();
    let request = build_transcription(&model, &wav);
    let outcome = shell.client.chat_once(&ctx.endpoint, &request).await;
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(chunk) => {
            let cleaned = clean_transcript(&chunk.content);
            tracing::info!(
                model = model.as_str(),
                bytes = wav.len(),
                chars = cleaned.text.chars().count(),
                ms,
                "AI transcription finished"
            );
            Ok(AiTranscribed {
                text: cleaned.text,
                language: cleaned.language,
            })
        }
        Err(error) => {
            tracing::info!(
                model = model.as_str(),
                reason = reason_of(&error),
                ms,
                "AI transcription failed"
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests;
