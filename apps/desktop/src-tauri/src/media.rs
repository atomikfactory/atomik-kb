//! The media protocol (ADR-0032): a custom URI scheme through which the webview's own `<video>`
//! and `<audio>` elements stream an attachment, a range at a time, whatever its size.
//!
//! The page sees `http://kbmedia.localhost/<attachment id>` on Windows and Android and
//! `kbmedia://localhost/<attachment id>` elsewhere. [`serve`] is the whole policy and knows nothing
//! of Tauri: a method, a request target and the `Range` header go in, a status, headers and a body
//! come out. [`register`] is the thin adapter that hands it the real request and runs it off the
//! main thread.
//!
//! - Only `GET` and `HEAD`. The target must be exactly `/<attachment id as UUID text>` (no query,
//!   no extra segment): anything else is 404.
//! - Only `video/*` and `audio/*` attachments are served (415 otherwise); an attachment that is
//!   not all here is 404; a locked core is 403 and nothing is read; a starting core is 503.
//! - One range per request. `bytes=a-b`, `a-` and `-n` are answered with 206 and `Content-Range`;
//!   several ranges (`a-b,c-d`) are answered as the first one; a header that is not valid `bytes=`
//!   syntax is ignored, as RFC 9110 allows; a range that starts past the end (or `-0`) is 416 with
//!   `Content-Range: bytes */size`.
//! - Every body is at most [`MAX_BODY`] bytes (one attachment chunk), so memory stays bounded and
//!   a request decrypts at most two chunks. A request without a range gets the whole file when it
//!   fits and otherwise 206 for the first [`MAX_BODY`] bytes, with the full length in
//!   `Content-Range`: the browser's media stack asks for the rest itself.
//! - The plaintext never goes to the webview's disk cache (`Cache-Control: no-store`, ADR-0007),
//!   the type is not sniffed, and no CORS header is sent (a media element needs none).
//! - Nothing is logged but the status and the number of bytes: never an id, a name or content.

use std::borrow::Cow;
use std::sync::Arc;

use kb_types::attachments::CHUNK_SIZE;
use kb_types::{
    ApiError, AttachmentAvailability, AttachmentId, AttachmentInfo, ByteRange, ErrorCode, Request,
    Response,
};
use tauri::Runtime;

use crate::SharedState;
use crate::attachments;
use crate::state::CoreHost;

/// The URI scheme name.
pub const SCHEME: &str = "kbmedia";

/// The most one response carries: one attachment chunk (4 MiB).
pub const MAX_BODY: u64 = CHUNK_SIZE as u64;

/// What [`serve`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaResponse {
    pub status: u16,
    /// Lower-case names, each once.
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl MediaResponse {
    /// The value of a header.
    #[cfg(test)]
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// The headers every answer has.
fn base_headers() -> Vec<(&'static str, String)> {
    vec![
        ("cache-control", "no-store".to_owned()),
        ("x-content-type-options", "nosniff".to_owned()),
    ]
}

/// An answer without a body.
fn bare(status: u16, extra: &[(&'static str, &str)]) -> MediaResponse {
    let mut headers = base_headers();
    headers.push(("content-length", "0".to_owned()));
    headers.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_owned())),
    );
    MediaResponse {
        status,
        headers,
        body: Vec::new(),
    }
}

/// What a `Range` header asks of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wanted {
    /// No usable header: from the start.
    Whole,
    /// A satisfiable range, inclusive and clamped to the file (not yet capped).
    Bytes { first: u64, last: u64 },
    /// Starts past the end of the file.
    Unsatisfiable,
}

fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok()
}

/// Reads a `Range` header for a file of `size` bytes (`size > 0`). Only the first range of a list
/// counts; a header that is not valid `bytes=` syntax is ignored (RFC 9110, section 14.2).
fn parse_range(header: Option<&str>, size: u64) -> Wanted {
    let Some(spec) = header.and_then(|text| text.trim().strip_prefix("bytes=")) else {
        return Wanted::Whole;
    };
    let first_spec = spec.split(',').next().unwrap_or("").trim();
    let Some((from, to)) = first_spec.split_once('-') else {
        return Wanted::Whole;
    };
    let last_byte = size - 1;
    match (from, to) {
        // `-n`: the last n bytes.
        ("", suffix) => match number(suffix) {
            Some(0) => Wanted::Unsatisfiable,
            Some(count) => Wanted::Bytes {
                first: size.saturating_sub(count),
                last: last_byte,
            },
            None => Wanted::Whole,
        },
        // `a-`: from a to the end.
        (from, "") => match number(from) {
            Some(first) if first >= size => Wanted::Unsatisfiable,
            Some(first) => Wanted::Bytes {
                first,
                last: last_byte,
            },
            None => Wanted::Whole,
        },
        // `a-b`.
        (from, to) => match (number(from), number(to)) {
            (Some(first), Some(last)) if last < first => Wanted::Whole,
            (Some(first), Some(_)) if first >= size => Wanted::Unsatisfiable,
            (Some(first), Some(last)) => Wanted::Bytes {
                first,
                last: last.min(last_byte),
            },
            _ => Wanted::Whole,
        },
    }
}

/// `/<uuid>` and nothing else.
fn attachment_of(target: &str) -> Option<AttachmentId> {
    let text = target.strip_prefix('/')?;
    if text.len() != 36
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return None;
    }
    text.parse::<AttachmentId>().ok()
}

fn is_media(mime: &str) -> bool {
    let lower = mime.trim().to_ascii_lowercase();
    lower.starts_with("video/") || lower.starts_with("audio/")
}

/// The status a core error becomes.
fn error_status(error: &ApiError) -> u16 {
    match error.code {
        ErrorCode::Locked => 403,
        ErrorCode::NotFound => 404,
        // "core is starting" and the like: try again.
        ErrorCode::Internal => 503,
        _ => 500,
    }
}

fn info_of(host: &dyn CoreHost, attachment_id: AttachmentId) -> Result<AttachmentInfo, ApiError> {
    match host.dispatch(Request::AttachmentGet { attachment_id })? {
        Response::Attachment(info) => Ok(info),
        _ => Err(ApiError::internal("unexpected answer")),
    }
}

/// Answers one request. `target` is the request's path and query, `range` the value of the `Range`
/// header. Reads the core only for the bytes of the range, through the same `attachment_read` the
/// IPC command uses.
pub fn serve(
    host: &dyn CoreHost,
    method: &str,
    target: &str,
    range: Option<&str>,
) -> MediaResponse {
    let head = method.eq_ignore_ascii_case("HEAD");
    if !head && !method.eq_ignore_ascii_case("GET") {
        return bare(405, &[("allow", "GET, HEAD")]);
    }
    let Some(attachment_id) = attachment_of(target) else {
        return bare(404, &[]);
    };
    // A locked, starting or failed core refuses here, before anything is looked up.
    let info = match host
        .ensure_ready()
        .and_then(|()| info_of(host, attachment_id))
    {
        Ok(info) => info,
        Err(error) => return bare(error_status(&error), &[]),
    };
    if !is_media(&info.mime) {
        return bare(415, &[]);
    }
    if info.availability != AttachmentAvailability::Complete {
        return bare(404, &[]);
    }
    let size = info.size;
    let mut headers = base_headers();
    headers.push(("accept-ranges", "bytes".to_owned()));
    headers.push(("content-type", info.mime.trim().to_owned()));

    if size == 0 {
        if range.is_some_and(|text| text.trim().starts_with("bytes=")) {
            return bare(416, &[("content-range", "bytes */0")]);
        }
        headers.push(("content-length", "0".to_owned()));
        return MediaResponse {
            status: 200,
            headers,
            body: Vec::new(),
        };
    }

    let (first, last, partial) = match parse_range(range, size) {
        Wanted::Unsatisfiable => {
            return bare(416, &[("content-range", &format!("bytes */{size}"))]);
        }
        Wanted::Whole => (0, (size - 1).min(MAX_BODY - 1), size > MAX_BODY),
        Wanted::Bytes { first, last } => (first, last.min(first + MAX_BODY - 1), true),
    };
    headers.push(("content-length", (last - first + 1).to_string()));
    if partial {
        headers.push(("content-range", format!("bytes {first}-{last}/{size}")));
    }
    let status = if partial { 206 } else { 200 };
    if head {
        return MediaResponse {
            status,
            headers,
            body: Vec::new(),
        };
    }
    let wanted = ByteRange {
        start: first,
        end: last + 1,
    };
    match attachments::read_bytes(host, attachment_id, Some(wanted)) {
        Ok(body) => MediaResponse {
            status,
            headers,
            body,
        },
        // The range was checked against the size: a refusal now is the core's, not the caller's.
        Err(error) if error.code == ErrorCode::InvalidInput => bare(500, &[]),
        Err(error) => bare(error_status(&error), &[]),
    }
}

/// Registers the scheme on a Tauri builder. The handler runs on the blocking pool (the core is
/// synchronous and may touch the disk and decrypt), never on the main thread.
pub fn register<R: Runtime>(builder: tauri::Builder<R>, state: SharedState) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(
        SCHEME,
        move |_context, request, responder| {
            let state = Arc::clone(&state);
            tauri::async_runtime::spawn_blocking(move || {
                let range = request
                    .headers()
                    .get(tauri::http::header::RANGE)
                    .and_then(|value| value.to_str().ok());
                let target = request
                    .uri()
                    .path_and_query()
                    .map_or("", |parts| parts.as_str());
                let answer = serve(&*state, request.method().as_str(), target, range);
                tracing::debug!(
                    status = answer.status,
                    bytes = answer.body.len(),
                    "media request"
                );
                let mut builder = tauri::http::Response::builder().status(answer.status);
                for (name, value) in &answer.headers {
                    builder = builder.header(*name, value.as_str());
                }
                let response = builder.body(Cow::Owned(answer.body)).unwrap_or_else(|_| {
                    let mut failed = tauri::http::Response::new(Cow::Owned(Vec::new()));
                    *failed.status_mut() = tauri::http::StatusCode::INTERNAL_SERVER_ERROR;
                    failed
                });
                responder.respond(response);
            });
        },
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::{captured_logs, ready_attachment_state, seed};

    fn id_target(id: AttachmentId) -> String {
        format!("/{id}")
    }

    /// Bytes where each is recognisable: the offset modulo 251.
    fn pattern(length: usize) -> Vec<u8> {
        (0..length).map(|offset| (offset % 251) as u8).collect()
    }

    fn setup(mime: &str, length: usize) -> (impl CoreHost, String, Vec<u8>) {
        let (state, spy) = ready_attachment_state();
        let bytes = pattern(length);
        let id = seed(&spy, "clip", mime, &bytes);
        (state, id_target(id), bytes)
    }

    #[test]
    fn a_range_is_answered_with_206_and_only_those_bytes() {
        let (host, target, bytes) = setup("audio/wav", 1000);
        let answer = serve(&host, "GET", &target, Some("bytes=10-19"));
        assert_eq!(answer.status, 206);
        assert_eq!(answer.body, bytes[10..20]);
        assert_eq!(answer.header("content-range"), Some("bytes 10-19/1000"));
        assert_eq!(answer.header("content-length"), Some("10"));
        assert_eq!(answer.header("accept-ranges"), Some("bytes"));
        assert_eq!(answer.header("content-type"), Some("audio/wav"));
        assert_eq!(answer.header("cache-control"), Some("no-store"));
        assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));
        assert!(answer.header("access-control-allow-origin").is_none());
    }

    #[test]
    fn open_ended_and_suffix_ranges() {
        let (host, target, bytes) = setup("video/mp4", 1000);
        let open = serve(&host, "GET", &target, Some("bytes=990-"));
        assert_eq!((open.status, open.body.as_slice()), (206, &bytes[990..]));
        assert_eq!(open.header("content-range"), Some("bytes 990-999/1000"));
        let suffix = serve(&host, "GET", &target, Some("bytes=-5"));
        assert_eq!(
            (suffix.status, suffix.body.as_slice()),
            (206, &bytes[995..])
        );
        assert_eq!(suffix.header("content-range"), Some("bytes 995-999/1000"));
        let big_suffix = serve(&host, "GET", &target, Some("bytes=-5000"));
        assert_eq!(big_suffix.body, bytes);
        assert_eq!(big_suffix.header("content-range"), Some("bytes 0-999/1000"));
        let past_end = serve(&host, "GET", &target, Some("bytes=900-5000"));
        assert_eq!(past_end.body, bytes[900..]);
        assert_eq!(past_end.header("content-range"), Some("bytes 900-999/1000"));
    }

    #[test]
    fn a_range_starting_past_the_end_is_416_with_the_size() {
        let (host, target, _) = setup("audio/mpeg", 1000);
        for range in ["bytes=1000-", "bytes=1000-1005", "bytes=5000-", "bytes=-0"] {
            let answer = serve(&host, "GET", &target, Some(range));
            assert_eq!(answer.status, 416, "{range}");
            assert_eq!(answer.header("content-range"), Some("bytes */1000"));
            assert!(answer.body.is_empty());
        }
    }

    #[test]
    fn several_ranges_are_answered_as_the_first_and_bad_syntax_is_ignored() {
        let (host, target, bytes) = setup("audio/mpeg", 1000);
        let many = serve(&host, "GET", &target, Some("bytes=0-3, 10-12"));
        assert_eq!((many.status, many.body.as_slice()), (206, &bytes[0..4]));
        for bad in [
            "bytes=abc",
            "items=0-5",
            "bytes=5-2",
            "bytes=",
            "0-5",
            "bytes=-",
        ] {
            let answer = serve(&host, "GET", &target, Some(bad));
            assert_eq!(answer.status, 200, "{bad}");
            assert_eq!(answer.body, bytes, "{bad}");
        }
    }

    #[test]
    fn without_a_range_a_small_file_is_whole_and_a_large_one_is_a_bounded_206() {
        let (host, target, bytes) = setup("audio/ogg", 1000);
        let whole = serve(&host, "GET", &target, None);
        assert_eq!(
            (whole.status, whole.body.as_slice()),
            (200, bytes.as_slice())
        );
        assert!(whole.header("content-range").is_none());
        assert_eq!(whole.header("content-length"), Some("1000"));

        let size = CHUNK_SIZE * 2 + 17;
        let (host, target, bytes) = setup("video/webm", size);
        let first = serve(&host, "GET", &target, None);
        assert_eq!(first.status, 206);
        assert_eq!(first.body.len(), CHUNK_SIZE);
        assert_eq!(first.body, bytes[..CHUNK_SIZE]);
        let expected = format!("bytes 0-{}/{size}", CHUNK_SIZE - 1);
        assert_eq!(first.header("content-range"), Some(expected.as_str()));
    }

    #[test]
    fn a_response_is_capped_at_one_chunk_and_reads_across_chunk_borders() {
        let size = CHUNK_SIZE * 2 + 17;
        let (host, target, bytes) = setup("video/mp4", size);
        let open = serve(&host, "GET", &target, Some("bytes=0-"));
        assert_eq!((open.status, open.body.len()), (206, CHUNK_SIZE));
        // A read that straddles the first border.
        let start = CHUNK_SIZE - 5;
        let straddle = serve(
            &host,
            "GET",
            &target,
            Some(&format!("bytes={start}-{}", start + 9)),
        );
        assert_eq!(straddle.body, bytes[start..start + 10]);
        // Exactly the border bytes, and the last bytes of the file.
        let last_chunk = serve(
            &host,
            "GET",
            &target,
            Some(&format!("bytes={}-", CHUNK_SIZE * 2)),
        );
        assert_eq!(last_chunk.body, bytes[CHUNK_SIZE * 2..]);
        assert_eq!(
            last_chunk.header("content-range"),
            Some(format!("bytes {}-{}/{size}", CHUNK_SIZE * 2, size - 1).as_str())
        );
        // A huge request is cut to a chunk, starting where asked.
        let from = CHUNK_SIZE + 3;
        let capped = serve(
            &host,
            "GET",
            &target,
            Some(&format!("bytes={from}-{}", size - 1)),
        );
        assert_eq!(capped.body.len(), CHUNK_SIZE);
        assert_eq!(capped.body, bytes[from..from + CHUNK_SIZE]);
    }

    #[test]
    fn a_range_read_asks_the_core_for_that_range_only() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "clip", "audio/wav", &pattern(1000));
        let target = id_target(id);
        spy.lock().unwrap().calls.clear();
        let _ = serve(&state, "GET", &target, Some("bytes=100-199"));
        assert_eq!(
            spy.lock().unwrap().calls.clone(),
            vec!["get".to_owned(), "read:100-200".to_owned()]
        );
    }

    #[test]
    fn head_answers_the_headers_and_reads_nothing() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "clip", "audio/wav", &pattern(1000));
        spy.lock().unwrap().calls.clear();
        let answer = serve(&state, "HEAD", &id_target(id), Some("bytes=0-99"));
        assert_eq!(answer.status, 206);
        assert!(answer.body.is_empty());
        assert_eq!(answer.header("content-length"), Some("100"));
        assert_eq!(answer.header("content-range"), Some("bytes 0-99/1000"));
        assert_eq!(spy.lock().unwrap().calls, vec!["get".to_owned()]);
        let plain = serve(&state, "HEAD", &id_target(id), None);
        assert_eq!(
            (plain.status, plain.header("content-length")),
            (200, Some("1000"))
        );
    }

    #[test]
    fn other_methods_are_refused() {
        let (host, target, _) = setup("audio/wav", 10);
        for method in ["POST", "PUT", "DELETE", "OPTIONS", "PATCH"] {
            let answer = serve(&host, method, &target, None);
            assert_eq!(answer.status, 405, "{method}");
            assert_eq!(answer.header("allow"), Some("GET, HEAD"));
        }
    }

    #[test]
    fn only_media_is_served() {
        for mime in [
            "application/pdf",
            "image/png",
            "text/html",
            "application/octet-stream",
        ] {
            let (host, target, _) = setup(mime, 100);
            let answer = serve(&host, "GET", &target, Some("bytes=0-9"));
            assert_eq!(answer.status, 415, "{mime}");
            assert!(answer.body.is_empty());
        }
        for mime in ["video/mp4", "AUDIO/MPEG", "audio/ogg"] {
            let (host, target, _) = setup(mime, 100);
            assert_eq!(serve(&host, "GET", &target, None).status, 200, "{mime}");
        }
    }

    #[test]
    fn an_unknown_or_malformed_id_is_404() {
        let (host, _, _) = setup("audio/wav", 10);
        let unknown = AttachmentId::from_bytes([200; 16]);
        assert_eq!(serve(&host, "GET", &id_target(unknown), None).status, 404);
        for target in [
            "",
            "/",
            "/abc",
            "/..",
            "/../x",
            "/12345678-9abc-4def-8123-456789abcdef/extra",
            "/12345678-9abc-4def-8123-456789abcdef?x=1",
            "/12345678-9abc-4def-8123-456789abcdeg",
            "12345678-9abc-4def-8123-456789abcdef",
            "/%31%32345678-9abc-4def-8123-456789abcdef",
        ] {
            assert_eq!(serve(&host, "GET", target, None).status, 404, "{target}");
        }
    }

    #[test]
    fn a_locked_core_serves_nothing() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "clip", "audio/wav", &pattern(100));
        let target = id_target(id);
        assert_eq!(serve(&state, "GET", &target, None).status, 200);
        state.lock().unwrap();
        spy.lock().unwrap().calls.clear();
        let answer = serve(&state, "GET", &target, Some("bytes=0-9"));
        assert_eq!(answer.status, 403);
        assert!(answer.body.is_empty());
        assert!(spy.lock().unwrap().calls.is_empty());
        let head = serve(&state, "HEAD", &target, None);
        assert_eq!(head.status, 403);
    }

    #[test]
    fn an_empty_attachment_is_200_and_any_range_of_it_is_416() {
        let (host, target, _) = setup("audio/wav", 0);
        let empty = serve(&host, "GET", &target, None);
        assert_eq!((empty.status, empty.body.len()), (200, 0));
        let ranged = serve(&host, "GET", &target, Some("bytes=0-"));
        assert_eq!(ranged.status, 416);
        assert_eq!(ranged.header("content-range"), Some("bytes */0"));
    }

    #[test]
    fn a_failing_read_is_a_server_error_and_the_log_has_no_id_or_bytes() {
        let (state, spy) = ready_attachment_state();
        let id = seed(&spy, "secret-name", "audio/wav", b"secret bytes here");
        spy.lock().unwrap().fail_read = Some(ApiError::storage("a chunk is missing"));
        let target = id_target(id);
        let mut status = 0;
        let logs = captured_logs(|| {
            status = serve(&state, "GET", &target, Some("bytes=0-5")).status;
            tracing::debug!("media request");
        });
        assert_eq!(status, 500);
        assert!(!logs.contains(&id.to_string()));
        assert!(!logs.contains("secret"));
    }

    #[test]
    fn the_range_parser() {
        assert_eq!(parse_range(None, 10), Wanted::Whole);
        assert_eq!(
            parse_range(Some("bytes=2-4"), 10),
            Wanted::Bytes { first: 2, last: 4 }
        );
        assert_eq!(
            parse_range(Some(" bytes=2-"), 10),
            Wanted::Bytes { first: 2, last: 9 }
        );
        assert_eq!(parse_range(Some("bytes=10-"), 10), Wanted::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=+1-3"), 10), Wanted::Whole);
        assert_eq!(parse_range(Some("bytes=1.5-3"), 10), Wanted::Whole);
        assert_eq!(
            parse_range(Some("bytes=99999999999999999999999-"), 10),
            Wanted::Whole
        );
    }
}
