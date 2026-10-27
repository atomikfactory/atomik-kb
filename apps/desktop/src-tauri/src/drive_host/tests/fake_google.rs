//! A fake Google on `127.0.0.1` for the tests of the Drive host: the token endpoint (code exchange, refresh,
//! `invalid_grant`), revocation, Drive's `about`, files (list, find the folder, create a folder, delete, download)
//! and resumable uploads (a 308 after each chunk, the status query, faults injected per request).
//!
//! It runs on a thread and a runtime of its own, so that the tests' flows (which run on theirs) never share a
//! reactor with it. It speaks only what `kb-drive` sends: the routes are the paths of Drive's API v3, the queries
//! are percent-decoded, an API call without a valid `Authorization: Bearer` is a 401. Every request is recorded.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::oneshot;

pub const REFRESH_TOKEN: &str = "refresh-1";
pub const GOOD_CODE: &str = "good-code";
pub const ACCOUNT: &str = "person@example.com";

/// One request the fake received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// The query, percent-decoded, in order.
    pub query: Vec<(String, String)>,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn query_value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// A form body, decoded.
    pub fn form(&self) -> Vec<(String, String)> {
        decode_pairs(&String::from_utf8_lossy(&self.body))
    }

    pub fn form_value(&self, name: &str) -> Option<String> {
        self.form()
            .into_iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// A file or a folder the fake holds.
#[derive(Debug, Clone)]
pub struct Stored {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub folder: bool,
    pub trashed: bool,
    pub bytes: Vec<u8>,
    pub modified_ms: i64,
}

/// What goes wrong with one upload request (the next `PUT` of a session takes the first fault).
#[derive(Debug, Clone)]
pub enum UploadFault {
    /// Answers this status and stores nothing.
    Fail(u16),
    /// Stores the chunk, then answers this status: the answer was lost.
    StoreThenFail(u16),
    /// Sits silent this long, then answers 503 and stores nothing (the client gives up before).
    Silent(Duration),
}

/// What goes wrong with a download.
#[derive(Debug, Clone)]
pub enum DownloadFault {
    /// Answers this status and an error body.
    Status(u16, String),
    /// Sends the first bytes of the file, then the connection breaks.
    Break(usize),
}

/// How the token endpoint answers a refresh.
#[derive(Debug, Clone)]
pub enum RefreshMode {
    Ok,
    InvalidGrant,
    ServerError,
}

/// What the tests change.
pub struct Script {
    pub refresh_token: String,
    pub refresh: RefreshMode,
    pub exchange_invalid_grant: bool,
    /// What a code exchange answers as the refresh token (none: it gives no refresh token).
    pub issue_refresh_token: bool,
    pub expires_in: u64,
    pub tokens_issued: u32,
    pub valid_tokens: HashSet<String>,
    pub files: Vec<Stored>,
    pub next_id: u32,
    pub about_status: u16,
    pub page_size: Option<usize>,
    pub create_folder_status: u16,
    pub upload_start_status: Option<(u16, String)>,
    pub upload_faults: VecDeque<UploadFault>,
    pub fail_every_upload: Option<u16>,
    pub delete_status: u16,
    pub download_fault: Option<DownloadFault>,
    /// When set, a download is sent in pieces of this many bytes with this pause before each.
    pub download_pace: Option<(usize, Duration)>,
    pub revoke_status: u16,
    pub revoked: Vec<String>,
    pub deleted: Vec<String>,
    sessions: HashMap<String, Session>,
}

struct Session {
    name: String,
    parent: Option<String>,
    total: u64,
    received: Vec<u8>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            refresh_token: REFRESH_TOKEN.to_owned(),
            refresh: RefreshMode::Ok,
            exchange_invalid_grant: false,
            issue_refresh_token: true,
            expires_in: 3600,
            tokens_issued: 0,
            valid_tokens: HashSet::new(),
            files: Vec::new(),
            next_id: 1,
            about_status: 200,
            page_size: None,
            create_folder_status: 200,
            upload_start_status: None,
            upload_faults: VecDeque::new(),
            fail_every_upload: None,
            delete_status: 204,
            download_fault: None,
            download_pace: None,
            revoke_status: 200,
            revoked: Vec::new(),
            deleted: Vec::new(),
            sessions: HashMap::new(),
        }
    }
}

struct Shared {
    base: String,
    requests: Mutex<Vec<Recorded>>,
    script: Mutex<Script>,
}

/// A running fake Google. It stops when dropped.
pub struct Fake {
    pub base: String,
    shared: Arc<Shared>,
    stop: Option<oneshot::Sender<()>>,
}

impl Fake {
    pub fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let shared = Arc::new(Shared {
            base: base.clone(),
            requests: Mutex::new(Vec::new()),
            script: Mutex::new(Script::default()),
        });
        let (stop, stopped) = oneshot::channel::<()>();
        let served = Arc::clone(&shared);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let app = Router::new().fallback(handle).with_state(served);
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await;
            });
        });
        Self {
            base,
            shared,
            stop: Some(stop),
        }
    }

    pub fn script(&self) -> MutexGuard<'_, Script> {
        self.shared
            .script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Everything received so far.
    pub fn requests(&self) -> Vec<Recorded> {
        self.shared
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The requests with this method and path.
    pub fn requests_to(&self, method: &str, path: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == method && request.path == path)
            .collect()
    }

    pub fn count(&self, method: &str, path: &str) -> usize {
        self.requests_to(method, path).len()
    }

    /// Adds a folder at the root; answers its id.
    pub fn add_folder(&self, name: &str) -> String {
        let mut script = self.script();
        let id = format!("folder-{}", script.next_id);
        script.next_id += 1;
        let modified_ms = i64::from(script.next_id) * 1_000;
        script.files.push(Stored {
            id: id.clone(),
            name: name.to_owned(),
            parent: None,
            folder: true,
            trashed: false,
            bytes: Vec::new(),
            modified_ms,
        });
        id
    }

    /// Adds a file to a folder; answers its id.
    pub fn add_file(&self, parent: &str, name: &str, bytes: &[u8], modified_ms: i64) -> String {
        let mut script = self.script();
        let id = format!("file-{}", script.next_id);
        script.next_id += 1;
        script.files.push(Stored {
            id: id.clone(),
            name: name.to_owned(),
            parent: Some(parent.to_owned()),
            folder: false,
            trashed: false,
            bytes: bytes.to_vec(),
            modified_ms,
        });
        id
    }

    /// The live files of a folder, by name.
    pub fn names_in(&self, parent: &str) -> Vec<String> {
        let script = self.script();
        let mut names: Vec<String> = script
            .files
            .iter()
            .filter(|file| file.parent.as_deref() == Some(parent) && !file.trashed)
            .map(|file| file.name.clone())
            .collect();
        names.sort();
        names
    }

    /// A stored file by name.
    pub fn file_named(&self, name: &str) -> Option<Stored> {
        self.script()
            .files
            .iter()
            .find(|file| file.name == name && !file.trashed)
            .cloned()
    }

    /// Makes every access token issued so far worthless (as Google does when one expires early).
    pub fn invalidate_access(&self) {
        self.script().valid_tokens.clear();
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn decode_pairs(text: &str) -> Vec<(String, String)> {
    Url::parse(&format!("http://x/?{text}"))
        .map(|url| {
            url.query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn json_response(status: u16, value: &Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

fn error_body(status: u16, reason: &str) -> Response {
    json_response(
        status,
        &json!({ "error": { "code": status, "message": "x", "errors": [{ "reason": reason }] } }),
    )
}

fn file_json(file: &Stored) -> Value {
    let mut value = json!({ "id": file.id, "name": file.name });
    if !file.folder {
        value["size"] = json!(file.bytes.len().to_string());
        let seconds = file.modified_ms / 1000;
        let millis = file.modified_ms % 1000;
        // 1970-01-01T00:00:00Z plus the seconds, enough for a test's order.
        let (day, rest) = (seconds / 86_400, seconds % 86_400);
        value["modifiedTime"] = json!(format!(
            "{}T{:02}:{:02}:{:02}.{:03}Z",
            civil(day),
            rest / 3600,
            (rest % 3600) / 60,
            rest % 60,
            millis
        ));
    }
    value
}

/// `YYYY-MM-DD` of a day count since 1970-01-01.
fn civil(days: i64) -> String {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn new_token(script: &mut Script) -> String {
    script.tokens_issued += 1;
    let token = format!("access-{}", script.tokens_issued);
    script.valid_tokens.insert(token.clone());
    token
}

fn authorised(script: &Script, headers: &HashMap<String, String>) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| script.valid_tokens.contains(token))
}

async fn handle(
    axum::extract::State(shared): axum::extract::State<Arc<Shared>>,
    request: axum::extract::Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let headers: HashMap<String, String> = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect();
    let recorded = Recorded {
        method: parts.method.to_string(),
        path: parts.uri.path().to_owned(),
        query: decode_pairs(parts.uri.query().unwrap_or_default()),
        headers,
        body: if bytes.len() <= 64 * 1024 {
            bytes.to_vec()
        } else {
            Vec::new()
        },
    };
    shared.requests.lock().unwrap().push(recorded.clone());
    route(&shared, &recorded, &bytes).await
}

async fn route(shared: &Shared, request: &Recorded, body: &Bytes) -> Response {
    let path = request.path.as_str();
    match (request.method.as_str(), path) {
        ("POST", "/token") => token(shared, request),
        ("POST", "/revoke") => {
            let mut script = shared.script.lock().unwrap();
            script
                .revoked
                .push(request.form_value("token").unwrap_or_default());
            let status = script.revoke_status;
            json_response(status, &json!({}))
        }
        ("GET", "/drive/v3/about") => with_auth(shared, request, |script| {
            if script.about_status != 200 {
                return error_body(script.about_status, "backendError");
            }
            json_response(200, &json!({ "user": { "emailAddress": ACCOUNT } }))
        }),
        ("GET", "/drive/v3/files") => with_auth(shared, request, |script| list(script, request)),
        ("POST", "/drive/v3/files") => {
            with_auth(shared, request, |script| create_folder(script, request))
        }
        ("POST", "/upload/drive/v3/files") => with_auth(shared, request, |script| {
            start_upload(shared, script, request)
        }),
        ("PUT", _) if path.starts_with("/upload/session/") => {
            upload_chunk(shared, request, body).await
        }
        ("GET", _) if path.starts_with("/drive/v3/files/") => download(shared, request).await,
        ("DELETE", _) if path.starts_with("/drive/v3/files/") => {
            with_auth(shared, request, |script| {
                let id = path.trim_start_matches("/drive/v3/files/");
                if script.delete_status >= 300 {
                    return error_body(script.delete_status, "backendError");
                }
                script.deleted.push(id.to_owned());
                script.files.retain(|file| file.id != id);
                StatusCode::NO_CONTENT.into_response()
            })
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Runs `answer` with the script locked when the request carries a good token; a 401 otherwise.
fn with_auth(
    shared: &Shared,
    request: &Recorded,
    answer: impl FnOnce(&mut Script) -> Response,
) -> Response {
    let mut script = shared.script.lock().unwrap();
    if !authorised(&script, &request.headers) {
        return error_body(401, "authError");
    }
    answer(&mut script)
}

fn token(shared: &Shared, request: &Recorded) -> Response {
    let mut script = shared.script.lock().unwrap();
    let grant = request.form_value("grant_type").unwrap_or_default();
    match grant.as_str() {
        "authorization_code" => {
            let code_ok = request.form_value("code").as_deref() == Some(GOOD_CODE)
                && request
                    .form_value("code_verifier")
                    .is_some_and(|v| v.len() == 43)
                && !script.exchange_invalid_grant;
            if !code_ok {
                return json_response(400, &json!({ "error": "invalid_grant" }));
            }
            let access = new_token(&mut script);
            let mut value = json!({ "access_token": access, "expires_in": script.expires_in, "token_type": "Bearer" });
            if script.issue_refresh_token {
                value["refresh_token"] = json!(script.refresh_token);
            }
            json_response(200, &value)
        }
        "refresh_token" => {
            if request.form_value("refresh_token").as_deref() != Some(script.refresh_token.as_str())
            {
                return json_response(400, &json!({ "error": "invalid_grant" }));
            }
            match script.refresh {
                RefreshMode::InvalidGrant => {
                    json_response(400, &json!({ "error": "invalid_grant" }))
                }
                RefreshMode::ServerError => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                RefreshMode::Ok => {
                    let access = new_token(&mut script);
                    json_response(
                        200,
                        &json!({ "access_token": access, "expires_in": script.expires_in, "token_type": "Bearer" }),
                    )
                }
            }
        }
        _ => json_response(400, &json!({ "error": "unsupported_grant_type" })),
    }
}

/// The folder id between the first pair of quotes of `'<id>' in parents`.
fn parent_of(query: &str) -> Option<String> {
    let start = query.find('\'')? + 1;
    let end = start + query[start..].find('\'')?;
    Some(query[start..end].to_owned())
}

fn list(script: &mut Script, request: &Recorded) -> Response {
    let q = request.query_value("q").unwrap_or_default().to_owned();
    let finding_folder = q.contains("mimeType");
    let mut found: Vec<Stored> = if finding_folder {
        script
            .files
            .iter()
            .filter(|file| file.folder && !file.trashed && file.name == "atomik-kb backups")
            .cloned()
            .collect()
    } else {
        let Some(parent) = parent_of(&q) else {
            return error_body(400, "invalid");
        };
        if !script
            .files
            .iter()
            .any(|file| file.folder && file.id == parent && !file.trashed)
        {
            return error_body(404, "notFound");
        }
        script
            .files
            .iter()
            .filter(|file| {
                file.parent.as_deref() == Some(parent.as_str())
                    && !file.trashed
                    && file.name.contains(".atomikkb")
            })
            .cloned()
            .collect()
    };
    if finding_folder {
        found.sort_by_key(|file| file.modified_ms);
    } else {
        found.sort_by_key(|file| std::cmp::Reverse(file.modified_ms));
    }
    let size = script
        .page_size
        .or_else(|| request.query_value("pageSize").and_then(|v| v.parse().ok()))
        .unwrap_or(100)
        .max(1);
    let from: usize = request
        .query_value("pageToken")
        .and_then(|token| token.parse().ok())
        .unwrap_or(0);
    let page: Vec<Value> = found.iter().skip(from).take(size).map(file_json).collect();
    let mut value = json!({ "files": page });
    if from + size < found.len() {
        value["nextPageToken"] = json!((from + size).to_string());
    }
    json_response(200, &value)
}

fn create_folder(script: &mut Script, request: &Recorded) -> Response {
    if script.create_folder_status != 200 {
        return error_body(script.create_folder_status, "backendError");
    }
    let body = request.json();
    let id = format!("folder-{}", script.next_id);
    script.next_id += 1;
    let name = body["name"].as_str().unwrap_or_default().to_owned();
    let folder = Stored {
        id,
        name,
        parent: None,
        folder: true,
        trashed: false,
        bytes: Vec::new(),
        modified_ms: i64::from(script.next_id) * 1_000,
    };
    let answer = file_json(&folder);
    script.files.push(folder);
    json_response(200, &answer)
}

fn start_upload(shared: &Shared, script: &mut Script, request: &Recorded) -> Response {
    if let Some((status, reason)) = script.upload_start_status.clone() {
        return error_body(status, &reason);
    }
    let body = request.json();
    let total: u64 = request
        .header("x-upload-content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let id = format!("session-{}", script.next_id);
    script.next_id += 1;
    script.sessions.insert(
        id.clone(),
        Session {
            name: body["name"].as_str().unwrap_or_default().to_owned(),
            parent: body["parents"][0].as_str().map(str::to_owned),
            total,
            received: Vec::new(),
        },
    );
    (
        StatusCode::OK,
        [(
            header::LOCATION,
            HeaderValue::from_str(&format!("{}/upload/session/{id}", shared.base)).unwrap(),
        )],
    )
        .into_response()
}

/// `bytes a-b/total` or `bytes */total`.
fn parse_range(value: &str) -> Option<(Option<(u64, u64)>, u64)> {
    let rest = value.strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let total = total.parse().ok()?;
    if span == "*" {
        return Some((None, total));
    }
    let (from, to) = span.split_once('-')?;
    Some((Some((from.parse().ok()?, to.parse().ok()?)), total))
}

fn resume_incomplete(received: usize) -> Response {
    let mut response = StatusCode::PERMANENT_REDIRECT.into_response();
    if received > 0 {
        response.headers_mut().insert(
            header::RANGE,
            HeaderValue::from_str(&format!("bytes=0-{}", received - 1)).unwrap(),
        );
    }
    response
}

async fn upload_chunk(shared: &Shared, request: &Recorded, body: &Bytes) -> Response {
    let id = request
        .path
        .trim_start_matches("/upload/session/")
        .to_owned();
    let fault = {
        let mut script = shared.script.lock().unwrap();
        match script.fail_every_upload {
            Some(status) => Some(UploadFault::Fail(status)),
            None => script.upload_faults.pop_front(),
        }
    };
    if let Some(UploadFault::Silent(pause)) = &fault {
        tokio::time::sleep(*pause).await;
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut script = shared.script.lock().unwrap();
    if let Some(UploadFault::Fail(status)) = &fault {
        return StatusCode::from_u16(*status).unwrap().into_response();
    }
    let Some(range) = request.header("content-range").and_then(parse_range) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(session) = script.sessions.get_mut(&id) else {
        return error_body(404, "notFound");
    };
    let (span, total) = range;
    if total != session.total {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if let Some((from, _)) = span {
        // Drive stores a chunk only where the stored bytes end.
        if usize::try_from(from).ok() == Some(session.received.len()) {
            session.received.extend_from_slice(body);
        }
    }
    let received = session.received.len();
    let complete = u64::try_from(received).ok() == Some(session.total);
    if let Some(UploadFault::StoreThenFail(status)) = &fault {
        // The chunk is stored (the file too, when it was the last one), the answer is lost.
        if complete {
            finish_upload(&mut script, &id);
        }
        return StatusCode::from_u16(*status).unwrap().into_response();
    }
    if complete {
        let file = finish_upload(&mut script, &id);
        return json_response(200, &file_json(&file));
    }
    resume_incomplete(received)
}

/// Makes the file of a finished session (once), answering it.
fn finish_upload(script: &mut Script, id: &str) -> Stored {
    let session = script.sessions.get(id).unwrap();
    let already = script
        .files
        .iter()
        .find(|file| file.id == format!("uploaded-{id}"))
        .cloned();
    if let Some(file) = already {
        return file;
    }
    let file = Stored {
        id: format!("uploaded-{id}"),
        name: session.name.clone(),
        parent: session.parent.clone(),
        folder: false,
        trashed: false,
        bytes: session.received.clone(),
        modified_ms: 4_000_000_000 + i64::from(script.next_id),
    };
    script.files.push(file.clone());
    file
}

async fn download(shared: &Shared, request: &Recorded) -> Response {
    let (file, fault, pace) = {
        let script = shared.script.lock().unwrap();
        if !authorised(&script, &request.headers) {
            return error_body(401, "authError");
        }
        let id = request.path.trim_start_matches("/drive/v3/files/");
        let Some(file) = script
            .files
            .iter()
            .find(|file| file.id == id && !file.folder)
            .cloned()
        else {
            return error_body(404, "notFound");
        };
        (file, script.download_fault.clone(), script.download_pace)
    };
    if let Some(DownloadFault::Status(status, reason)) = &fault {
        return error_body(*status, reason);
    }
    let bytes = file.bytes;
    let length = bytes.len();
    let (piece, pause) = match (&fault, pace) {
        (_, Some((piece, pause))) => (piece.max(1), pause),
        _ => (64 * 1024, Duration::ZERO),
    };
    let cut = match &fault {
        Some(DownloadFault::Break(after)) => Some(*after),
        _ => None,
    };
    let pieces: Vec<Result<Bytes, std::io::Error>> = {
        let limit = cut.unwrap_or(length).min(length);
        let mut out: Vec<Result<Bytes, std::io::Error>> = bytes
            .get(..limit)
            .unwrap_or_default()
            .chunks(piece)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect();
        if cut.is_some() {
            out.push(Err(std::io::Error::other("the connection broke")));
        }
        out
    };
    let body = Body::from_stream(stream::unfold(
        (pieces.into_iter(), true),
        move |(mut pieces, first)| async move {
            let next = pieces.next()?;
            if !first && !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
            Some((next, (pieces, false)))
        },
    ));
    let mut response = Response::new(body);
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
}
