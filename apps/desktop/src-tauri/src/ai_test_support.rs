//! Helpers for the unit tests of the AI and MCP modules: a scripted core, and a stub Ollama on
//! `127.0.0.1` (the way `url_import`'s tests stub a web server).

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response as HttpResponse};
use futures_util::stream;
use kb_types::ai::{
    AiChunkVector, AiIndexInfo, AiIndexWork, AiSettings, AiSource, DEFAULT_AI_ENDPOINT,
    McpClientInfo, McpGrant,
};
use kb_types::{
    ApiError, AttachmentId, AttachmentInfo, BackupSummary, ByteRange, Event, Json, PageId,
    PageInfo, Request, Response, SpaceId, SpaceInfo,
};
use serde_json::{Value, json};

use crate::dialogs::{Confirm, Question};
use crate::shell_labels::Labels;
use crate::state::{
    AppState, AttachmentSink, BackupSink, BackupSource, CoreApi, CoreHost, LockedApi,
};

// --- the scripted core ---

/// A hook that may answer a request before the defaults do.
pub type Answer = Box<dyn FnMut(&Request) -> Option<Result<Response, ApiError>> + Send>;

/// What the fake core holds and has seen; shared with the test.
#[derive(Default)]
pub struct Script {
    /// Every request the core received, in order.
    pub requests: Vec<Request>,
    /// The settings, by key.
    pub settings: HashMap<String, Json>,
    /// Events the core emitted and the shell has not drained.
    pub events: Vec<Event>,
    /// Hooks consulted first, in order.
    pub answers: Vec<Answer>,
    /// The MCP clients.
    pub clients: Vec<McpClientInfo>,
    /// The spaces.
    pub spaces: Vec<SpaceInfo>,
    /// What `AiIndexNext` answers, one per call; an empty work after them.
    pub index_work: VecDeque<AiIndexWork>,
    /// What `AiRetrieve` answers.
    pub passages: Vec<AiSource>,
    /// What `AiIndexStatus` answers.
    pub index_info: AiIndexInfo,
    /// The Markdown of each page.
    pub markdown: HashMap<PageId, String>,
    /// The page metadata of each page.
    pub pages: HashMap<PageId, PageInfo>,
    /// What `McpCall` answers.
    pub mcp_reply: Option<Result<Option<String>, ApiError>>,
    /// How many calls are being made right now (to see that the lock is not held across a wait).
    pub in_call: usize,
}

/// The script shared between the fake core and the test.
pub type SharedScript = Arc<Mutex<Script>>;

/// Locks a script.
pub fn script(shared: &SharedScript) -> MutexGuard<'_, Script> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A core that answers from a [`Script`].
pub struct AiFake {
    script: SharedScript,
}

/// The locked fake: it keeps the core, so unlocking gives the same one back.
pub struct AiFakeLocked(AiFake);

impl LockedApi for AiFakeLocked {
    type Core = AiFake;

    fn unlock(self, _: &str) -> Result<AiFake, (Self, ApiError)> {
        Ok(self.0)
    }
}

fn unused() -> ApiError {
    ApiError::internal("not used by these tests")
}

impl CoreApi for AiFake {
    type Locked = AiFakeLocked;

    fn dispatch(&mut self, request: Request) -> Result<Response, ApiError> {
        let mut script = script(&self.script);
        script.requests.push(request.clone());
        for answer in &mut script.answers {
            if let Some(result) = answer(&request) {
                return result;
            }
        }
        match request {
            Request::SettingsGet { key } => {
                Ok(Response::Setting(script.settings.get(&key).cloned()))
            }
            Request::SettingsSet { key, value } => {
                script.settings.insert(key.clone(), value);
                script.events.push(Event::SettingChanged { key });
                Ok(Response::Ok)
            }
            Request::AiIndexStatus => Ok(Response::AiIndex(script.index_info.clone())),
            Request::AiIndexNext { .. } => Ok(Response::AiIndexWork(
                script.index_work.pop_front().unwrap_or_default(),
            )),
            Request::AiIndexStore { .. } => Ok(Response::Ok),
            Request::AiRetrieve { .. } => Ok(Response::AiPassages(script.passages.clone())),
            Request::PageExportMarkdown { page_id } => Ok(Response::Markdown(
                script.markdown.get(&page_id).cloned().unwrap_or_default(),
            )),
            Request::PageGet { page_id } => script
                .pages
                .get(&page_id)
                .cloned()
                .map(Response::Page)
                .ok_or_else(|| ApiError::not_found("no such page")),
            Request::SpaceList => Ok(Response::Spaces(script.spaces.clone())),
            Request::McpClientList => Ok(Response::McpClients(script.clients.clone())),
            Request::McpClientGrantsSet { client_id, grants } => {
                let client = script
                    .clients
                    .iter_mut()
                    .find(|client| client.id == client_id)
                    .ok_or_else(|| ApiError::not_found("no such client"))?;
                client.grants = grants;
                let info = client.clone();
                script.events.push(Event::McpChanged);
                Ok(Response::McpClient(info))
            }
            Request::McpCall { .. } => match script.mcp_reply.clone() {
                Some(Ok(body)) => Ok(Response::McpReply(kb_types::ai::McpReply { body })),
                Some(Err(error)) => Err(error),
                None => Ok(Response::McpReply(kb_types::ai::McpReply { body: None })),
            },
            _ => Err(unused()),
        }
    }

    fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut script(&self.script).events)
    }

    fn page_doc_open(&mut self, _: PageId) -> Result<Vec<u8>, ApiError> {
        Err(unused())
    }

    fn page_doc_import(&mut self, _: PageId, _: &[u8]) -> Result<(), ApiError> {
        Err(unused())
    }

    fn page_doc_export(&mut self, _: PageId, _: Option<&[u8]>) -> Result<Vec<u8>, ApiError> {
        Err(unused())
    }

    fn page_doc_close(&mut self, _: PageId) -> Result<(), ApiError> {
        Err(unused())
    }

    fn lock(self) -> Result<AiFakeLocked, (Self, ApiError)> {
        Ok(AiFakeLocked(self))
    }

    fn backup_export(&mut self, _: BackupSink<'_>) -> Result<BackupSummary, ApiError> {
        Err(unused())
    }

    fn backup_import(
        &mut self,
        _: BackupSource<'_>,
        _: Option<&str>,
    ) -> Result<BackupSummary, ApiError> {
        Err(unused())
    }

    fn attachment_write(&mut self, _: AttachmentId, _: &[u8]) -> Result<(), ApiError> {
        Err(unused())
    }

    fn attachment_read(
        &mut self,
        _: AttachmentId,
        _: Option<ByteRange>,
        _: AttachmentSink<'_>,
    ) -> Result<AttachmentInfo, ApiError> {
        Err(unused())
    }
}

/// A ready state over a fresh fake core, as the managed `Arc<dyn CoreHost>`, with the script.
pub fn ready_host() -> (Arc<AppState<AiFake>>, Arc<dyn CoreHost>, SharedScript) {
    let script: SharedScript = Arc::default();
    let state = Arc::new(AppState::new());
    state.set_ready(AiFake {
        script: Arc::clone(&script),
    });
    let host: Arc<dyn CoreHost> = Arc::clone(&state) as Arc<dyn CoreHost>;
    (state, host, script)
}

/// Stores a setting in the script (no event).
pub fn put_setting(shared: &SharedScript, key: &str, value: Json) {
    script(shared).settings.insert(key.to_owned(), value);
}

/// AI on, with these models; the endpoint is the stub's.
pub fn ai_on(shared: &SharedScript, endpoint: &str, chat: Option<&str>, embedding: Option<&str>) {
    let settings = AiSettings {
        enabled: true,
        chat_model: chat.map(str::to_owned),
        embedding_model: embedding.map(str::to_owned),
        transcription_model: Some("asr-model".to_owned()),
        semantic_index: true,
        think: false,
    };
    put_setting(
        shared,
        kb_types::ai::AI_SETTINGS_SETTING,
        serde_json::to_value(settings).unwrap(),
    );
    put_setting(
        shared,
        kb_types::ai::AI_ENDPOINT_SETTING,
        json!({ "url": endpoint, "remoteConsent": false }),
    );
}

/// The requests of one kind the core received.
pub fn requests_of(shared: &SharedScript, kind: &str) -> Vec<Request> {
    script(shared)
        .requests
        .iter()
        .filter(|request| request.kind() == kind)
        .cloned()
        .collect()
}

/// A space.
pub fn space(id: u8, name: &str) -> SpaceInfo {
    SpaceInfo {
        id: SpaceId::from_bytes([id; 16]),
        name: name.to_owned(),
        icon: None,
        created_at: 0,
        page_count: 0,
        share_state: kb_types::SpaceShareState::Personal,
        role: None,
        archive_reason: None,
        member_count: 0,
        ai_enabled: true,
    }
}

/// An MCP client.
pub fn client(id: &str, name: &str, grants: Vec<McpGrant>) -> McpClientInfo {
    McpClientInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        grants,
        created_at: 0,
        last_used_at: None,
    }
}

/// A passage.
pub fn passage(n: u32, page: u8, text: &str) -> AiSource {
    AiSource {
        n,
        page_id: PageId::from_bytes([page; 16]),
        space_id: SpaceId::from_bytes([9; 16]),
        block_id: None,
        title: format!("Page {page}"),
        icon: None,
        text: text.to_owned(),
        score: 1.0 / f32::from(u8::try_from(n).unwrap_or(1)),
    }
}

/// A page's metadata.
pub fn page_info(id: PageId, title: &str) -> PageInfo {
    PageInfo {
        id,
        space_id: SpaceId::from_bytes([9; 16]),
        parent_id: None,
        title: title.to_owned(),
        icon: None,
        cover: None,
        cover_y: None,
        font: None,
        small_text: false,
        locked: false,
        created_at: 0,
        updated_at: 0,
        trashed_at: None,
        created_by: None,
        edited_by: None,
        properties: Vec::new(),
        template: false,
        template_placeholders: Vec::new(),
    }
}

/// A vector of the work an indexer test expects (16 dimensions, as Ollama would give, any length is
/// fine for the stub).
pub fn vector_of(chunk: &AiChunkVector) -> usize {
    chunk.vector.len()
}

// --- the stub Ollama ---

/// What the stub's `/api/chat` does.
#[derive(Clone)]
pub enum ChatBehaviour {
    /// Streams these pieces of bytes with a pause between them, then ends the body.
    Pieces(Vec<Vec<u8>>, Duration),
    /// Streams the pieces, then holds the connection open without a word until the client goes away.
    Hang(Vec<Vec<u8>>),
    /// Says nothing for this long, then streams the pieces.
    SlowStart(Duration, Vec<Vec<u8>>),
    /// An HTTP error with a body.
    Error(u16, String),
    /// One JSON body, for a request with `stream: false`.
    Json(String),
}

/// A request the stub received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub body: Value,
    pub user_agent: String,
}

/// What the stub serves and what it saw.
pub struct StubState {
    pub requests: Mutex<Vec<Recorded>>,
    pub version: Mutex<String>,
    pub tags: Mutex<String>,
    /// `/api/show` answers by model: status and body; a model not listed is a 404 "not found".
    pub show: Mutex<HashMap<String, (u16, String)>>,
    pub chat: Mutex<ChatBehaviour>,
    /// `/api/embed`: the status and, for 200, the dimension of the vectors made.
    pub embed_status: Mutex<u16>,
    pub embed_dim: AtomicUsize,
    /// A body that replaces the embed answer when set.
    pub embed_body: Mutex<Option<String>>,
    /// How long `/api/embed` thinks before it answers.
    pub embed_delay: Mutex<Duration>,
    /// How many chat bodies were dropped by the client before they ended.
    pub chat_dropped: AtomicUsize,
    pub chat_started: AtomicUsize,
    pub chat_in_flight: AtomicUsize,
}

impl StubState {
    /// What the stub received at `path`.
    pub fn received(&self, path: &str) -> Vec<Recorded> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path == path)
            .cloned()
            .collect()
    }
}

/// A running stub: its address and state. The server stops with the runtime of the test.
pub struct Stub {
    pub addr: SocketAddr,
    pub state: Arc<StubState>,
}

impl Stub {
    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

/// An NDJSON chat line with a piece of the answer.
pub fn line(content: &str) -> Vec<u8> {
    let mut text = json!({ "model": "m", "message": { "role": "assistant", "content": content }, "done": false }).to_string();
    text.push('\n');
    text.into_bytes()
}

/// The last NDJSON chat line.
pub fn done_line() -> Vec<u8> {
    let mut text = json!({ "model": "m", "message": { "role": "assistant", "content": "" }, "done": true, "done_reason": "stop" }).to_string();
    text.push('\n');
    text.into_bytes()
}

/// The models the stub lists by default: a chat model that thinks, an embedding model and an audio model.
pub fn default_tags() -> String {
    json!({ "models": [
        { "name": "chat-model", "size": 5_000_000_000_u64, "details": { "family": "qwen3", "parameter_size": "8.2B" } },
        { "name": "embed-model", "size": 274_000_000_u64, "details": { "family": "nomic-bert", "parameter_size": "137M" } },
        { "name": "asr-model", "size": 3_000_000_000_u64, "details": { "family": "asr", "parameter_size": "2B" } },
    ] })
    .to_string()
}

fn default_show() -> HashMap<String, (u16, String)> {
    let mut shows = HashMap::new();
    shows.insert(
        "chat-model".to_owned(),
        (
            200,
            json!({ "capabilities": ["completion", "thinking", "tools"], "model_info": { "qwen3.context_length": 40960 } }).to_string(),
        ),
    );
    shows.insert(
        "embed-model".to_owned(),
        (
            200,
            json!({ "capabilities": ["embedding"], "model_info": { "nomic-bert.context_length": 2048 } }).to_string(),
        ),
    );
    shows.insert(
        "asr-model".to_owned(),
        (
            200,
            json!({ "capabilities": ["completion", "audio"], "model_info": { "asr.context_length": 8192 } }).to_string(),
        ),
    );
    shows
}

/// Starts a stub Ollama on `127.0.0.1` with these chat pieces.
pub async fn start_stub(chat: ChatBehaviour) -> Stub {
    let state = Arc::new(StubState {
        requests: Mutex::new(Vec::new()),
        version: Mutex::new("0.35.0".to_owned()),
        tags: Mutex::new(default_tags()),
        show: Mutex::new(default_show()),
        chat: Mutex::new(chat),
        embed_status: Mutex::new(200),
        embed_dim: AtomicUsize::new(16),
        embed_body: Mutex::new(None),
        embed_delay: Mutex::new(Duration::ZERO),
        chat_dropped: AtomicUsize::new(0),
        chat_started: AtomicUsize::new(0),
        chat_in_flight: AtomicUsize::new(0),
    });
    let app = Router::new()
        .fallback(stub_handler)
        .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Stub { addr, state }
}

/// Sets a flag when it is dropped: held by a streamed body, it says the client went away.
struct DropFlag {
    state: Arc<StubState>,
    finished: AtomicBool,
}

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.state.chat_in_flight.fetch_sub(1, Ordering::SeqCst);
        if !self.finished.load(Ordering::SeqCst) {
            self.state.chat_dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn ndjson(body: Body) -> HttpResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
        .into_response()
}

/// A body that sends `pieces` (pausing `pause` before each but the first), then `then`.
fn body_of(state: Arc<StubState>, pieces: Vec<Vec<u8>>, pause: Duration, hang: bool) -> Body {
    state.chat_in_flight.fetch_add(1, Ordering::SeqCst);
    let flag = DropFlag {
        state,
        finished: AtomicBool::new(false),
    };
    let items: VecDeque<Vec<u8>> = pieces.into();
    let stream = stream::unfold(
        (items, flag, true),
        move |(mut items, flag, first)| async move {
            match items.pop_front() {
                Some(piece) => {
                    if !first && !pause.is_zero() {
                        tokio::time::sleep(pause).await;
                    }
                    Some((
                        Ok::<Bytes, std::convert::Infallible>(Bytes::from(piece)),
                        (items, flag, false),
                    ))
                }
                None if hang => {
                    std::future::pending::<()>().await;
                    None
                }
                None => {
                    flag.finished.store(true, Ordering::SeqCst);
                    None
                }
            }
        },
    );
    Body::from_stream(stream)
}

async fn stub_handler(
    State(state): State<Arc<StubState>>,
    request: axum::extract::Request,
) -> HttpResponse {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let path = parts.uri.path().to_owned();
    state.requests.lock().unwrap().push(Recorded {
        method: parts.method.to_string(),
        path: path.clone(),
        body: body.clone(),
        user_agent: parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    });
    match path.as_str() {
        "/api/version" => {
            let version = state.version.lock().unwrap().clone();
            axum::Json(json!({ "version": version })).into_response()
        }
        "/api/tags" => {
            let tags = state.tags.lock().unwrap().clone();
            ([(header::CONTENT_TYPE, "application/json")], tags).into_response()
        }
        "/api/show" => {
            let model = body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let shows = state.show.lock().unwrap();
            match shows.get(&model) {
                Some((status, text)) => (
                    StatusCode::from_u16(*status).unwrap(),
                    [(header::CONTENT_TYPE, "application/json")],
                    text.clone(),
                )
                    .into_response(),
                None => (
                    StatusCode::NOT_FOUND,
                    axum::Json(json!({ "error": format!("model '{model}' not found") })),
                )
                    .into_response(),
            }
        }
        "/api/embed" => {
            let delay = *state.embed_delay.lock().unwrap();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let status = *state.embed_status.lock().unwrap();
            if status != 200 {
                let text = if status == 404 {
                    json!({ "error": "model not found, try pulling it first" })
                } else {
                    json!({ "error": "embed failed" })
                };
                return (StatusCode::from_u16(status).unwrap(), axum::Json(text)).into_response();
            }
            if let Some(text) = state.embed_body.lock().unwrap().clone() {
                return ([(header::CONTENT_TYPE, "application/json")], text).into_response();
            }
            let inputs = body
                .get("input")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let dim = state.embed_dim.load(Ordering::SeqCst);
            let vectors: Vec<Vec<f32>> = (0..inputs)
                .map(|index| {
                    (0..dim)
                        .map(|d| if d == index % dim { 1.0 } else { 0.0 })
                        .collect()
                })
                .collect();
            axum::Json(json!({ "model": "m", "embeddings": vectors })).into_response()
        }
        "/api/chat" => {
            state.chat_started.fetch_add(1, Ordering::SeqCst);
            let behaviour = state.chat.lock().unwrap().clone();
            match behaviour {
                ChatBehaviour::Pieces(pieces, pause) => {
                    ndjson(body_of(Arc::clone(&state), pieces, pause, false))
                }
                ChatBehaviour::Hang(pieces) => {
                    ndjson(body_of(Arc::clone(&state), pieces, Duration::ZERO, true))
                }
                ChatBehaviour::SlowStart(delay, pieces) => {
                    tokio::time::sleep(delay).await;
                    ndjson(body_of(Arc::clone(&state), pieces, Duration::ZERO, false))
                }
                ChatBehaviour::Error(status, text) => (
                    StatusCode::from_u16(status).unwrap(),
                    [(header::CONTENT_TYPE, "application/json")],
                    text,
                )
                    .into_response(),
                ChatBehaviour::Json(text) => {
                    ([(header::CONTENT_TYPE, "application/json")], text).into_response()
                }
            }
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A TCP port nothing listens on (bound once and released).
pub fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// A `Default` for [`DEFAULT_AI_ENDPOINT`], kept here so the tests name it in one place.
pub const DEFAULT_ENDPOINT: &str = DEFAULT_AI_ENDPOINT;

/// A flag a test can wait on.
#[derive(Default)]
pub struct Flag(AtomicBool);

impl Flag {
    pub fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Polls `check` until it is true or `within` has passed.
pub async fn eventually(within: Duration, mut check: impl FnMut() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < within {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    check()
}

// --- the person, answering a dialog ---

/// Answers the questions the shell asks in a native dialog, from a script, and keeps them.
#[derive(Default)]
pub struct FakeConfirm {
    answers: Mutex<VecDeque<bool>>,
    /// The questions asked, in order.
    pub asked: Mutex<Vec<Question>>,
    /// The words the questions are written in (English unless a test gives others).
    words: Labels,
}

impl FakeConfirm {
    /// Says `answers` to the first questions, then no.
    pub fn saying(answers: &[bool]) -> Arc<Self> {
        Self::saying_in(answers, Labels::default())
    }

    /// Says `answers` to the first questions, then no, to questions written in the words of `words`.
    pub fn saying_in(answers: &[bool], words: Labels) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.iter().copied().collect()),
            asked: Mutex::new(Vec::new()),
            words,
        })
    }

    /// How many questions were asked.
    pub fn count(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

impl Confirm for FakeConfirm {
    fn labels(&self) -> Labels {
        self.words.clone()
    }

    fn ask(&self, question: &Question) -> bool {
        self.asked.lock().unwrap().push(question.clone());
        self.answers.lock().unwrap().pop_front().unwrap_or(false)
    }
}

/// A 16 kHz mono 16-bit PCM WAV file of `samples` samples of silence.
pub fn wav_bytes(samples: usize) -> Vec<u8> {
    let data_len = u32::try_from(samples * 2).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&16_000_u32.to_le_bytes());
    out.extend_from_slice(&32_000_u32.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.resize(out.len() + samples * 2, 0);
    out
}
