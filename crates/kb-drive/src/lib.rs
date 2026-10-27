//! The Google Drive backup protocol (ADR-0059, `docs/architecture/drive-backup.md` section 2), sans-IO:
//! every request is a value the desktop shell sends and every response a value it received. No I/O,
//! no clock and no randomness of its own: the caller passes the random bytes and the times in. The
//! crate compiles to wasm32 like the rest of `crates/*`.
//!
//! The flow, as the shell drives it:
//!
//! 1. sign-in: [`pkce`], [`authorize_url`] (opened in the browser), [`parse_redirect`] on the
//!    loopback request's query, [`token_request`] and [`parse_token`];
//! 2. every later call: [`refresh_request`] when the access token ends, then the Drive requests
//!    below, each answer read by its `parse_*` function (all of them go through [`classify`], so
//!    the error kinds are the same everywhere);
//! 3. an upload: [`upload_start_request`] and [`parse_upload_session`], then [`upload_chunk_request`]
//!    and [`parse_upload_step`] until `Done`; after a transient failure [`upload_status_request`]
//!    says where to resume.
//!
//! Nothing in an error text, and nothing in the `Debug` of the values that carry secrets (client,
//! PKCE pair, tokens, requests and responses), names a token, code, id or name.

mod time;
mod wire;

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::wire::{
    constant_time_eq, encode, form_pairs, header, parse_query, q_quote, query_value,
};

/// Google's authorisation page (opened in the system browser).
pub const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// The token endpoint (code exchange and refresh).
pub const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// The revocation endpoint.
pub const REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";
/// The Drive API's origin.
pub const API: &str = "https://www.googleapis.com";
/// The only scope asked: the files this app created.
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
/// The folder the backups go to, at the root of the person's Drive.
pub const FOLDER_NAME: &str = "atomik-kb backups";
/// Drive's MIME type of a folder.
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
/// The MIME type a backup is uploaded with.
pub const BACKUP_MIME: &str = "application/octet-stream";
/// The size of an upload chunk: 8 MiB, a multiple of the 256 KiB Drive requires.
pub const CHUNK_SIZE: usize = 8 * 1024 * 1024;
/// Most backups a listing returns.
pub const MAX_LISTED: usize = 200;
/// The extension of a backup file (the listing's filter; see also [`is_backup_name`]).
pub const BACKUP_EXTENSION: &str = ".atomikkb";

/// The prefix of a scheduled backup's name (`kb-types` `BACKUP_AUTO_PREFIX`).
const SCHEDULED_PREFIX: &str = "atomik-kb-auto-";

/// The fields a file listing asks for.
const LIST_FIELDS: &str = "nextPageToken,files(id,name,size,modifiedTime)";

/// An HTTP request to send as it is.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET`, `POST`, `PUT` or `DELETE`.
    pub method: &'static str,
    /// The full URL, query included.
    pub url: String,
    /// Header names and values (no `Host`, no `Content-Length`: the transport sets them).
    pub headers: Vec<(String, String)>,
    /// The body (empty for none).
    pub body: Vec<u8>,
}

/// A request's `Debug` shows the method, the origin, the header names and the body's length: the
/// rest holds tokens, codes, ids and session URLs.
impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("origin", &origin(&self.url))
            .field("headers", &names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// An HTTP response as received.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The status code.
    pub status: u16,
    /// Header names (any case) and values.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

/// A response's `Debug` shows the status, the header names and the body's length: the rest holds
/// tokens and session URLs.
impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// `scheme://host` of a URL (what is left when the path and the query are cut).
fn origin(url: &str) -> &str {
    let start = url.find("://").map_or(0, |at| at + 3);
    let end = url
        .get(start..)
        .and_then(|rest| rest.find(['/', '?']))
        .map_or(url.len(), |at| start + at);
    url.get(..end).unwrap_or("")
}

/// The OAuth client the app signs in with.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthClient {
    /// The client id.
    pub client_id: String,
    /// The client secret of an installed app (not confidential; Google still asks for it). An
    /// empty secret is sent as none.
    pub client_secret: Option<String>,
}

impl fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthClient")
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

/// A PKCE pair (RFC 7636, S256).
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    /// Sent with the code exchange.
    pub verifier: String,
    /// Sent with the authorisation request.
    pub challenge: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce").finish_non_exhaustive()
    }
}

/// Tokens from the token endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    /// The access token.
    pub access_token: String,
    /// Seconds it stays valid.
    pub expires_in_s: u64,
    /// The refresh token (the code exchange gives one; a refresh usually does not).
    pub refresh_token: Option<String>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in_s", &self.expires_in_s)
            .field("has_refresh_token", &self.refresh_token.is_some())
            .finish_non_exhaustive()
    }
}

/// A file or folder in Drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveFile {
    /// The Drive id.
    pub id: String,
    /// The name.
    pub name: String,
    /// Bytes (folders have none).
    pub size: Option<u64>,
    /// Last modified, Unix ms.
    pub modified_time_ms: Option<i64>,
}

/// Where a resumable upload stands after a chunk or a status query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadStep {
    /// Send the bytes from `next_offset` on.
    Continue {
        /// The first byte Drive has not stored.
        next_offset: u64,
    },
    /// The file is complete.
    Done(DriveFile),
}

/// What went wrong. The texts name the step, never a token, code, id or name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DriveError {
    /// The person refused access in the browser.
    #[error("access was refused")]
    Denied,
    /// The redirect's `state` was not ours.
    #[error("the sign-in answer does not match the request")]
    State,
    /// Google refused the stored token or the client: connect again.
    #[error("the connection to Google Drive must be made again")]
    Reconnect,
    /// The Drive is full.
    #[error("the Google Drive is full")]
    Quota,
    /// The file or folder is gone.
    #[error("not found in Google Drive")]
    NotFound,
    /// Too many requests: wait and retry.
    #[error("Google Drive asks to slow down")]
    RateLimited,
    /// Google's side failed: retry later.
    #[error("Google Drive failed")]
    Server,
    /// An answer this version does not understand.
    #[error("unexpected answer from Google: {0}")]
    Protocol(String),
}

// ---------------------------------------------------------------------------------------------
// OAuth
// ---------------------------------------------------------------------------------------------

/// A PKCE pair from 32 random bytes: the verifier is their base64url (no padding, 43 characters),
/// the challenge the base64url of the SHA-256 of the verifier's ASCII (RFC 7636, S256).
pub fn pkce(random: [u8; 32]) -> Pkce {
    let verifier = URL_SAFE_NO_PAD.encode(random);
    let hash: [u8; 32] = Sha256::digest(verifier.as_bytes()).into();
    Pkce {
        challenge: URL_SAFE_NO_PAD.encode(hash),
        verifier,
    }
}

/// The URL to open in the browser.
pub fn authorize_url(
    client: &OAuthClient,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> String {
    let query = form_pairs(&[
        ("client_id", &client.client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ]);
    format!("{AUTH_ENDPOINT}?{query}")
}

/// The authorisation code of the loopback redirect's query (without the `?`, which is tolerated).
/// The `state` must be the expected one (also on a refusal), else [`DriveError::State`];
/// `error=access_denied` is [`DriveError::Denied`], any other `error` a protocol error.
pub fn parse_redirect(query: &str, expected_state: &str) -> Result<String, DriveError> {
    let pairs = parse_query(query);
    // A missing or empty expectation never matches: failing closed.
    let state_ok = !expected_state.is_empty()
        && query_value(&pairs, "state").is_some_and(|got| constant_time_eq(got, expected_state));
    if !state_ok {
        return Err(DriveError::State);
    }
    if let Some(error) = query_value(&pairs, "error") {
        return Err(if error == "access_denied" {
            DriveError::Denied
        } else {
            DriveError::Protocol("sign-in: the browser reported an error".to_owned())
        });
    }
    match query_value(&pairs, "code") {
        Some(code) if !code.is_empty() => Ok(code.to_owned()),
        _ => Err(DriveError::Protocol(
            "sign-in: the redirect carries no code".to_owned(),
        )),
    }
}

const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// A POST of a form body.
fn form_post(url: &str, pairs: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        method: "POST",
        url: url.to_owned(),
        headers: vec![("Content-Type".to_owned(), FORM_CONTENT_TYPE.to_owned())],
        body: form_pairs(pairs).into_bytes(),
    }
}

/// The client's form fields: its id, then its secret when it has a non-empty one.
fn client_fields<'a>(
    client: &'a OAuthClient,
    rest: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    let mut fields = vec![("client_id", client.client_id.as_str())];
    if let Some(secret) = client.client_secret.as_deref().filter(|s| !s.is_empty()) {
        fields.push(("client_secret", secret));
    }
    fields.extend_from_slice(rest);
    fields
}

/// The code exchange.
pub fn token_request(
    client: &OAuthClient,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> HttpRequest {
    let fields = client_fields(
        client,
        &[
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ],
    );
    form_post(TOKEN_ENDPOINT, &fields)
}

/// A refresh of the access token.
pub fn refresh_request(client: &OAuthClient, refresh_token: &str) -> HttpRequest {
    let fields = client_fields(
        client,
        &[
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ],
    );
    form_post(TOKEN_ENDPOINT, &fields)
}

/// Revokes a token (the refresh token revokes the whole grant).
pub fn revoke_request(token: &str) -> HttpRequest {
    form_post(REVOKE_ENDPOINT, &[("token", token)])
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: Option<String>,
    expires_in: Option<Value>,
    refresh_token: Option<String>,
}

/// Tokens from the token endpoint's answer. A 400 or 401 whose `error` is `invalid_grant`,
/// `invalid_client` or `unauthorized_client` is [`DriveError::Reconnect`].
pub fn parse_token(response: &HttpResponse) -> Result<Tokens, DriveError> {
    if matches!(response.status, 400 | 401) && token_error_means_reconnect(&response.body) {
        return Err(DriveError::Reconnect);
    }
    let body: TokenBody = read_json(response, "token")?;
    let access_token = body
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or_else(|| protocol("token: no access token"))?;
    let expires_in_s = body
        .expires_in
        .as_ref()
        .and_then(lenient_u64)
        .ok_or_else(|| protocol("token: no lifetime"))?;
    Ok(Tokens {
        access_token,
        expires_in_s,
        refresh_token: body.refresh_token.filter(|token| !token.is_empty()),
    })
}

/// Whether a token endpoint's error body names a grant or a client that is no longer good.
fn token_error_means_reconnect(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    matches!(
        value.get("error").and_then(Value::as_str),
        Some("invalid_grant" | "invalid_client" | "unauthorized_client")
    )
}

// ---------------------------------------------------------------------------------------------
// Drive
// ---------------------------------------------------------------------------------------------

fn bearer(access_token: &str) -> (String, String) {
    ("Authorization".to_owned(), format!("Bearer {access_token}"))
}

fn authorized(method: &'static str, url: String, access_token: &str) -> HttpRequest {
    HttpRequest {
        method,
        url,
        headers: vec![bearer(access_token)],
        body: Vec::new(),
    }
}

fn json_post(
    url: String,
    access_token: &str,
    mut extra: Vec<(String, String)>,
    body: &Value,
) -> HttpRequest {
    let mut headers = vec![
        bearer(access_token),
        (
            "Content-Type".to_owned(),
            "application/json; charset=UTF-8".to_owned(),
        ),
    ];
    headers.append(&mut extra);
    HttpRequest {
        method: "POST",
        url,
        headers,
        body: body.to_string().into_bytes(),
    }
}

fn files_url(query: &[(&str, &str)]) -> String {
    format!("{API}/drive/v3/files?{}", form_pairs(query))
}

/// The account's address (`about`, `fields=user(emailAddress)`).
pub fn about_request(access_token: &str) -> HttpRequest {
    let query = form_pairs(&[("fields", "user(emailAddress)")]);
    authorized("GET", format!("{API}/drive/v3/about?{query}"), access_token)
}

#[derive(Deserialize)]
struct AboutBody {
    user: Option<AboutUser>,
}

#[derive(Deserialize)]
struct AboutUser {
    #[serde(rename = "emailAddress")]
    email_address: Option<String>,
}

/// The address from `about`'s answer, when there is one.
pub fn parse_about(response: &HttpResponse) -> Result<Option<String>, DriveError> {
    let body: AboutBody = read_json(response, "about")?;
    Ok(body
        .user
        .and_then(|user| user.email_address)
        .filter(|address| !address.is_empty()))
}

/// Looks for the backups folder among the files this app created (oldest first, so that the first
/// match is stable; read the answer with [`parse_file_list`]).
pub fn find_folder_request(access_token: &str) -> HttpRequest {
    let q = format!(
        "name = {} and mimeType = {} and trashed = false",
        q_quote(FOLDER_NAME),
        q_quote(FOLDER_MIME)
    );
    let url = files_url(&[
        ("q", &q),
        ("spaces", "drive"),
        ("orderBy", "createdTime"),
        ("pageSize", "10"),
        ("fields", "nextPageToken,files(id,name)"),
    ]);
    authorized("GET", url, access_token)
}

/// Creates the backups folder at the root (read the answer with [`parse_file`]).
pub fn create_folder_request(access_token: &str) -> HttpRequest {
    let body = json!({ "name": FOLDER_NAME, "mimeType": FOLDER_MIME });
    json_post(
        format!("{API}/drive/v3/files"),
        access_token,
        Vec::new(),
        &body,
    )
}

/// The backups of the folder, newest first; `page_token` continues a listing. Drive's `contains`
/// on a name is a loose match: keep the files for which [`is_backup_name`] holds.
pub fn list_backups_request(
    access_token: &str,
    folder_id: &str,
    page_token: Option<&str>,
) -> HttpRequest {
    let q = format!(
        "{} in parents and trashed = false and name contains {}",
        q_quote(folder_id),
        q_quote(BACKUP_EXTENSION)
    );
    let page_size = MAX_LISTED.to_string();
    let mut query = vec![
        ("q", q.as_str()),
        ("orderBy", "modifiedTime desc"),
        ("pageSize", page_size.as_str()),
        ("fields", LIST_FIELDS),
        ("spaces", "drive"),
    ];
    if let Some(token) = page_token.filter(|token| !token.is_empty()) {
        query.push(("pageToken", token));
    }
    authorized("GET", files_url(&query), access_token)
}

#[derive(Deserialize)]
struct FileBody {
    id: Option<String>,
    name: Option<String>,
    size: Option<Value>,
    #[serde(rename = "modifiedTime")]
    modified_time: Option<String>,
}

impl FileBody {
    /// A file needs an id and a name; a size or a time that cannot be read is left out rather than
    /// failing a whole listing.
    fn into_file(self, step: &str) -> Result<DriveFile, DriveError> {
        let id = self
            .id
            .filter(|id| !id.is_empty())
            .ok_or_else(|| protocol(&format!("{step}: a file without an id")))?;
        let name = self
            .name
            .ok_or_else(|| protocol(&format!("{step}: a file without a name")))?;
        Ok(DriveFile {
            id,
            name,
            size: self.size.as_ref().and_then(lenient_u64),
            modified_time_ms: self
                .modified_time
                .as_deref()
                .and_then(time::parse_rfc3339_ms),
        })
    }
}

#[derive(Deserialize)]
struct ListBody {
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    files: Option<Vec<FileBody>>,
}

/// Files and the next page's token from a listing's answer.
pub fn parse_file_list(
    response: &HttpResponse,
) -> Result<(Vec<DriveFile>, Option<String>), DriveError> {
    let body: ListBody = read_json(response, "list")?;
    let files = body
        .files
        .unwrap_or_default()
        .into_iter()
        .map(|file| file.into_file("list"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        files,
        body.next_page_token.filter(|token| !token.is_empty()),
    ))
}

/// One file from an answer (a folder created, an upload completed).
pub fn parse_file(response: &HttpResponse) -> Result<DriveFile, DriveError> {
    parse_file_step(response, "file")
}

fn parse_file_step(response: &HttpResponse, step: &str) -> Result<DriveFile, DriveError> {
    let body: FileBody = read_json(response, step)?;
    body.into_file(step)
}

/// Starts a resumable upload of `total` bytes named `name` into the folder.
pub fn upload_start_request(
    access_token: &str,
    folder_id: &str,
    name: &str,
    total: u64,
) -> HttpRequest {
    let body = json!({ "name": name, "parents": [folder_id], "mimeType": BACKUP_MIME });
    let extra = vec![
        ("X-Upload-Content-Type".to_owned(), BACKUP_MIME.to_owned()),
        ("X-Upload-Content-Length".to_owned(), total.to_string()),
    ];
    json_post(
        format!("{API}/upload/drive/v3/files?uploadType=resumable"),
        access_token,
        extra,
        &body,
    )
}

/// The session URL (`Location`) of a started upload.
pub fn parse_upload_session(response: &HttpResponse) -> Result<String, DriveError> {
    classify_step(response, "upload start")?;
    match header(&response.headers, "location") {
        Some(url) if !url.is_empty() => Ok(url.to_owned()),
        _ => Err(protocol("upload start: no session URL")),
    }
}

fn put(session_url: &str, content_range: String, body: Vec<u8>) -> HttpRequest {
    HttpRequest {
        method: "PUT",
        url: session_url.to_owned(),
        headers: vec![("Content-Range".to_owned(), content_range)],
        body,
    }
}

/// Sends `bytes` at `offset` of `total` (`Content-Range: bytes a-b/total`; no bytes is the status
/// query `bytes */total`, which also finishes an empty file).
pub fn upload_chunk_request(
    session_url: &str,
    offset: u64,
    bytes: Vec<u8>,
    total: u64,
) -> HttpRequest {
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length == 0 {
        return upload_status_request(session_url, total);
    }
    let last = offset.saturating_add(length - 1);
    put(session_url, format!("bytes {offset}-{last}/{total}"), bytes)
}

/// Asks where an interrupted upload stands.
pub fn upload_status_request(session_url: &str, total: u64) -> HttpRequest {
    put(session_url, format!("bytes */{total}"), Vec::new())
}

/// The step after a chunk or a status query: 308 is `Continue` (from after the `Range`'s last
/// byte, or from 0 without a `Range`), 200 and 201 are `Done`.
pub fn parse_upload_step(response: &HttpResponse) -> Result<UploadStep, DriveError> {
    match response.status {
        308 => {
            let next_offset = match header(&response.headers, "range") {
                None => 0,
                Some(range) => next_offset_of(range)?,
            };
            Ok(UploadStep::Continue { next_offset })
        }
        200 | 201 => Ok(UploadStep::Done(parse_file_step(response, "upload")?)),
        status => {
            classify_step(response, "upload")?;
            Err(protocol(&format!("upload: unexpected status {status}")))
        }
    }
}

/// `bytes=0-N` is stored up to and including byte N, so the next one is N + 1.
fn next_offset_of(range: &str) -> Result<u64, DriveError> {
    let unreadable = || protocol("upload: unreadable Range");
    let span = range.strip_prefix("bytes=").ok_or_else(unreadable)?;
    let (from, to) = span.split_once('-').ok_or_else(unreadable)?;
    if from.trim().parse::<u64>().ok() != Some(0) {
        return Err(unreadable());
    }
    to.trim()
        .parse::<u64>()
        .ok()
        .and_then(|last| last.checked_add(1))
        .ok_or_else(unreadable)
}

/// Downloads a file's bytes (`alt=media`); check the answer's status with [`classify`].
pub fn download_request(access_token: &str, file_id: &str) -> HttpRequest {
    let url = format!("{API}/drive/v3/files/{}?alt=media", encode(file_id));
    authorized("GET", url, access_token)
}

/// Deletes a file.
pub fn delete_request(access_token: &str, file_id: &str) -> HttpRequest {
    let url = format!("{API}/drive/v3/files/{}", encode(file_id));
    authorized("DELETE", url, access_token)
}

/// A 2xx answer with nothing to read (delete, revoke).
pub fn parse_empty(response: &HttpResponse) -> Result<(), DriveError> {
    classify_step(response, "request")
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// The error an answer means, or `Ok` for a 2xx: 401 is `Reconnect`; 403 is `Quota`
/// (`storageQuotaExceeded`), `RateLimited` (`userRateLimitExceeded`, `rateLimitExceeded`) or a
/// protocol error; 404 is `NotFound`; 429 is `RateLimited`; 5xx is `Server`; the rest is a
/// protocol error.
pub fn classify(response: &HttpResponse) -> Result<(), DriveError> {
    classify_step(response, "request")
}

fn classify_step(response: &HttpResponse, step: &str) -> Result<(), DriveError> {
    match response.status {
        200..=299 => Ok(()),
        401 => Err(DriveError::Reconnect),
        403 => Err(match forbidden_reason(&response.body) {
            Some(Forbidden::Quota) => DriveError::Quota,
            Some(Forbidden::RateLimited) => DriveError::RateLimited,
            None => protocol(&format!("{step}: status 403")),
        }),
        404 => Err(DriveError::NotFound),
        429 => Err(DriveError::RateLimited),
        500..=599 => Err(DriveError::Server),
        status => Err(protocol(&format!("{step}: status {status}"))),
    }
}

enum Forbidden {
    Quota,
    RateLimited,
}

/// What Drive's 403 body says, from `error.errors[].reason` and `error.details[].reason` (compared
/// without case and underscores: `rateLimitExceeded` and `RATE_LIMIT_EXCEEDED` are the same).
fn forbidden_reason(body: &[u8]) -> Option<Forbidden> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let error = value.get("error")?;
    ["errors", "details"]
        .iter()
        .filter_map(|key| error.get(key)?.as_array())
        .flatten()
        .filter_map(|entry| entry.get("reason")?.as_str())
        .find_map(|reason| {
            let normal: String = reason
                .chars()
                .filter(|c| *c != '_')
                .map(|c| c.to_ascii_lowercase())
                .collect();
            match normal.as_str() {
                "storagequotaexceeded" => Some(Forbidden::Quota),
                "userratelimitexceeded" | "ratelimitexceeded" => Some(Forbidden::RateLimited),
                _ => None,
            }
        })
}

fn protocol(text: &str) -> DriveError {
    DriveError::Protocol(text.to_owned())
}

/// A successful answer's body as `T`; the step names where it failed (the parser's own message is
/// dropped: it may quote the body).
fn read_json<T: DeserializeOwned>(response: &HttpResponse, step: &str) -> Result<T, DriveError> {
    classify_step(response, step)?;
    serde_json::from_slice(&response.body)
        .map_err(|_| protocol(&format!("{step}: unreadable answer")))
}

/// A JSON number, or a decimal string (Drive sends `size` as one).
fn lenient_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// The folder's address in Drive's web interface.
pub fn folder_url(folder_id: &str) -> String {
    format!(
        "https://drive.google.com/drive/folders/{}",
        encode(folder_id)
    )
}

/// Whether a name is a backup file's (ends with [`BACKUP_EXTENSION`]).
pub fn is_backup_name(name: &str) -> bool {
    name.len() > BACKUP_EXTENSION.len() && name.ends_with(BACKUP_EXTENSION)
}

/// Whether a name is a scheduled backup's: `atomik-kb-auto-YYYY-MM-DD-HHMM.atomikkb`, digits and
/// real calendar numbers, as `kb-core::backup_schedule::to_prune` reads it.
pub fn is_scheduled_name(name: &str) -> bool {
    let Some(stamp) = name
        .strip_prefix(SCHEDULED_PREFIX)
        .and_then(|rest| rest.strip_suffix(BACKUP_EXTENSION))
    else {
        return false;
    };
    let bytes = stamp.as_bytes();
    // YYYY-MM-DD-HHMM
    if bytes.len() != 15 || [4, 7, 10].iter().any(|at| bytes.get(*at) != Some(&b'-')) {
        return false;
    }
    let digits = |from: usize, to: usize| -> Option<i64> {
        let part = bytes.get(from..to)?;
        if !part.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(
            part.iter()
                .fold(0i64, |acc, d| acc * 10 + i64::from(d - b'0')),
        )
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute)) = (
        digits(0, 4),
        digits(5, 7),
        digits(8, 10),
        digits(11, 13),
        digits(13, 15),
    ) else {
        return false;
    };
    (1..=12).contains(&month)
        && (1..=time::days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
}

#[cfg(test)]
mod tests;
