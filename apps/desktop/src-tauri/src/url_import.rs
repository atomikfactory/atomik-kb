//! "Image from URL" (ADR-0028, item 7): `attachment_import_url { spaceId, url }`. The shell
//! downloads the picture a link points to, once, on the person's explicit action, and hands
//! it to the core as an ordinary attachment (encrypted at rest, synced like any other). The
//! page never holds the link and the webview never loads it: its CSP does not change, and a
//! remote image is never fetched behind the person's back.
//!
//! - **The link** ([`parse_url`]): `https` only, no user name or password, at most
//!   [`MAX_URL_CHARS`] characters. Anything else is `InvalidInput` for the field `url`.
//! - **The request** ([`download`]): one `GET` with no cookies (the client has no cookie
//!   store), no `Referer`, `Accept: image/*` and the user agent `atomik-kb/<version>`; at most
//!   [`MAX_REDIRECTS`] redirects, each to an `https` address without credentials; 10 s to
//!   connect and 60 s in all. The answer is read piece by piece and never exceeds
//!   `MAX_PREVIEW_BYTES` (32 MiB, what the UI can show): checked against `Content-Length`
//!   first and again as the body arrives (`InvalidInput` for the field `size`, with
//!   `details.limit`, the shape of `limit_exceeded`).
//! - **The type** ([`sniff_image`]) comes from the bytes, never from the server: PNG, JPEG,
//!   GIF, WebP, BMP, AVIF, and SVG when the server says `image/svg+xml` and the body is an SVG
//!   document (an `<img>` never runs a script in an SVG). Anything else is
//!   `InvalidInput` for the field `url`, reason `notImage`. A body that cannot be an image is
//!   refused as soon as its first bytes are in, without reading the rest.
//! - **The name** ([`derive_name`]) is the last segment of the link's path, percent-decoded
//!   and sanitised with [`names::sanitised_name`] (the rules of "Save as"), with the extension
//!   of the sniffed type; `image.<ext>` when the path leaves no name.
//! - **Failures** are `InvalidInput` for the field `url` with `details.reason`: `status` (and
//!   `details.status`, the HTTP code), `network`, `timeout`, `redirect` or `notImage`, and a
//!   short message that never repeats the link. **Neither the link nor its host is ever
//!   logged or put in a message** (a link may carry a token; `reqwest`'s own errors name the
//!   URL, so they are never formatted); the log says the outcome, the reason and the size.
//!
//! The bytes then go through the same `AttachmentBegin`, `attachment_write` and
//! `AttachmentFinish` the file import uses ([`crate::attachments`]), the lock released between
//! pieces, and any failure aborts the upload. The download is held in memory (at most 32 MiB)
//! while it is imported; the core lock is never held while the network is used.
//!
//! The scheme and host policy is [`FetchPolicy`]: the commands use [`FetchPolicy::production`]
//! (https only, the system's proxy settings); the tests build one that allows plain `http` and
//! talks to a stub server on `127.0.0.1` without a proxy.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use kb_types::attachments::MAX_PREVIEW_BYTES;
use kb_types::{ApiError, AttachmentInfo, SpaceId};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::redirect::Policy;
use reqwest::{Client, Url};
use serde_json::json;

use crate::attachment_ipc::limit_exceeded;
use crate::attachment_names as names;
use crate::attachments::{abort, begin, finish, stream_pieces};
use crate::state::CoreHost;
use crate::{ATTACHMENT_STOPPED, SharedState, blocking};

/// The longest link accepted, in characters.
pub const MAX_URL_CHARS: usize = 2048;

/// How many redirects a download follows (each must be to an `https` address).
pub const MAX_REDIRECTS: usize = 5;

/// How long the connection (DNS, TCP and TLS) may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the whole download, body included, may take.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

/// What the shell calls itself to the server: `atomik-kb/<version>`.
const USER_AGENT: &str = concat!("atomik-kb/", env!("CARGO_PKG_VERSION"));

/// The most the buffer is reserved for in advance from a `Content-Length` (the header is the
/// server's word, and the buffer grows as the body arrives).
const RESERVE_BYTES: u64 = 4 * 1024 * 1024;

/// How many bytes of the body are needed to tell that it is not any binary image format (the
/// longest signature checked, BMP's, is 18 bytes).
const SIGNATURE_BYTES: usize = 32;

/// The field of every error of this module.
const URL_FIELD: &str = "url";

/// What the shell's messages say for each failure; none of them names the link.
const NOT_AN_IMAGE: &str = "the link is not an image";
const NETWORK_FAILED: &str = "the server could not be reached";
const TIMED_OUT: &str = "the server took too long to answer";
const BAD_REDIRECT: &str = "the link leads somewhere that is not allowed";
const TOO_LARGE: &str = "the image is larger than the app can show";

/// The sorts of failure `details.reason` names (ADR-0028).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    Status,
    Network,
    Timeout,
    Redirect,
    NotImage,
}

impl Reason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Redirect => "redirect",
            Self::NotImage => "notImage",
        }
    }
}

/// `InvalidInput` for the field `url`, with `details.reason` (and the HTTP code for a status).
fn failure(reason: Reason, message: &str, status: Option<u16>) -> ApiError {
    let mut details = json!({ "field": URL_FIELD, "reason": reason.as_str() });
    if let Some(code) = status {
        details["status"] = json!(code);
    }
    ApiError::invalid(URL_FIELD, message).with_details(details)
}

/// The `details.reason` of an error of this module, for the log.
fn reason_of(error: &ApiError) -> &str {
    error
        .details
        .as_ref()
        .and_then(|details| details.get("reason"))
        .and_then(|reason| reason.as_str())
        .unwrap_or("none")
}

// --- the policy ---

/// What a download may do, and the limits it keeps. [`FetchPolicy::production`] is what the
/// commands use.
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    /// Whether a link, and a redirect, may use plain `http`. Tests only.
    allow_http: bool,
    /// Whether the system's proxy settings apply (they do for the app; the tests talk to
    /// `127.0.0.1` directly).
    use_system_proxy: bool,
    /// The most bytes a download may have.
    max_bytes: u64,
    connect_timeout: Duration,
    total_timeout: Duration,
}

impl FetchPolicy {
    /// `https` only, the system's proxy settings, at most `MAX_PREVIEW_BYTES`, 10 s to connect
    /// and 60 s in all.
    pub const fn production() -> Self {
        Self {
            allow_http: false,
            use_system_proxy: true,
            max_bytes: MAX_PREVIEW_BYTES,
            connect_timeout: CONNECT_TIMEOUT,
            total_timeout: TOTAL_TIMEOUT,
        }
    }

    /// Plain `http`, no proxy, the given limits: for a stub server on `127.0.0.1`.
    #[cfg(test)]
    pub(crate) fn local_test(max_bytes: u64, total_timeout: Duration) -> Self {
        Self {
            allow_http: true,
            use_system_proxy: false,
            max_bytes,
            connect_timeout: CONNECT_TIMEOUT,
            total_timeout,
        }
    }
}

// --- the link ---

/// The link the person gave, checked: surrounding whitespace dropped, at most
/// [`MAX_URL_CHARS`] characters, a well-formed absolute address with a host, the `https` scheme
/// (`http` too when `allow_http`), and no user name or password. Anything else is
/// `InvalidInput` for the field `url`; the messages never repeat the link.
pub fn parse_url(text: &str, allow_http: bool) -> Result<Url, ApiError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(ApiError::invalid(URL_FIELD, "the link is empty"));
    }
    if text.chars().count() > MAX_URL_CHARS {
        return Err(ApiError::invalid(
            URL_FIELD,
            format!("the link may be at most {MAX_URL_CHARS} characters"),
        ));
    }
    let url = Url::parse(text)
        .map_err(|_| ApiError::invalid(URL_FIELD, "the link is not a valid web address"))?;
    if !scheme_allowed(url.scheme(), allow_http) {
        return Err(ApiError::invalid(
            URL_FIELD,
            "only https links can be used: the link must start with https://",
        ));
    }
    if has_credentials(&url) {
        return Err(ApiError::invalid(
            URL_FIELD,
            "the link must not contain a user name or a password",
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(ApiError::invalid(URL_FIELD, "the link has no host"));
    }
    Ok(url)
}

fn scheme_allowed(scheme: &str, allow_http: bool) -> bool {
    scheme == "https" || (allow_http && scheme == "http")
}

fn has_credentials(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

// --- the type ---

/// The image formats the shell accepts, as its bytes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Avif,
    Svg,
}

impl ImageKind {
    /// The extension a file of this kind is given (lowercase, no dot).
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Avif => "avif",
            Self::Svg => "svg",
        }
    }

    /// The MIME type stored with the attachment.
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Bmp => "image/bmp",
            Self::Avif => "image/avif",
            Self::Svg => "image/svg+xml",
        }
    }

    /// Whether `extension` (any case) already names this kind, so that a name keeps it.
    fn names_extension(self, extension: &str) -> bool {
        let known: &[&str] = match self {
            Self::Jpeg => &["jpg", "jpeg", "jpe", "jfif"],
            Self::Png => &["png"],
            Self::Gif => &["gif"],
            Self::Webp => &["webp"],
            Self::Bmp => &["bmp"],
            Self::Avif => &["avif"],
            Self::Svg => &["svg"],
        };
        known
            .iter()
            .any(|name| extension.eq_ignore_ascii_case(name))
    }
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The DIB header sizes a BMP file has (core, info, v2 to v5), at offset 14.
const BMP_HEADER_SIZES: [u32; 7] = [12, 40, 52, 56, 64, 108, 124];

/// The kind of image the `bytes` are, from their magic numbers alone; `None` for anything
/// else. SVG has no magic number: it counts only when `content_type` (the server's header,
/// parameters allowed) is `image/svg+xml` **and** the body is an SVG document
/// ([`is_svg_document`]), so a server cannot make an HTML page pass for a picture and a
/// mislabelled picture is still read for what it is.
pub fn sniff_image(bytes: &[u8], content_type: Option<&str>) -> Option<ImageKind> {
    sniff_binary(bytes).or_else(|| {
        (is_svg_content_type(content_type) && is_svg_document(bytes)).then_some(ImageKind::Svg)
    })
}

/// [`sniff_image`] for the formats with a signature.
fn sniff_binary(bytes: &[u8]) -> Option<ImageKind> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        Some(ImageKind::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageKind::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageKind::Gif)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some(ImageKind::Webp)
    } else if is_bmp(bytes) {
        Some(ImageKind::Bmp)
    } else if is_avif(bytes) {
        Some(ImageKind::Avif)
    } else {
        None
    }
}

/// `BM` and a DIB header size of a known kind (two letters alone would take any text that
/// starts with "BM" for a bitmap).
fn is_bmp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"BM")
        && bytes
            .get(14..18)
            .and_then(|size| <[u8; 4]>::try_from(size).ok())
            .is_some_and(|size| BMP_HEADER_SIZES.contains(&u32::from_le_bytes(size)))
}

/// An ISO base media file (`ftyp` box at the start) whose major brand is `avif` or `avis`.
fn is_avif(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp".as_slice())
        && matches!(bytes.get(8..12), Some(b"avif" | b"avis"))
}

/// Whether the server's `Content-Type` is `image/svg+xml` (any case, parameters ignored).
fn is_svg_content_type(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("image/svg+xml"))
}

/// Whether `bytes` are an SVG document: after an optional UTF-8 byte order mark, white space,
/// processing instructions (the XML declaration), comments and a document type declaration
/// (with an internal subset), the first thing is an `<svg` element. Case-sensitive, as XML is:
/// the webview parses an `<img>`'s SVG as XML.
pub fn is_svg_document(bytes: &[u8]) -> bool {
    svg_root(bytes).is_some()
}

fn svg_root(bytes: &[u8]) -> Option<()> {
    let mut rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    loop {
        rest = rest.trim_ascii_start();
        if let Some(after) = rest.strip_prefix(b"<?") {
            rest = after_marker(after, b"?>")?;
        } else if let Some(after) = rest.strip_prefix(b"<!--") {
            rest = after_marker(after, b"-->")?;
        } else if let Some(after) = rest.strip_prefix(b"<!DOCTYPE") {
            rest = after_doctype(after)?;
        } else {
            let after = rest.strip_prefix(b"<svg")?;
            return matches!(
                after.first(),
                Some(b'>' | b'/' | b' ' | b'\t' | b'\r' | b'\n')
            )
            .then_some(());
        }
    }
}

/// What follows the first `marker` in `bytes`.
fn after_marker<'a>(bytes: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    let at = bytes
        .windows(marker.len())
        .position(|window| window == marker)?;
    bytes.get(at + marker.len()..)
}

/// What follows the `>` that ends a document type declaration, skipping an internal subset
/// (`[ ... ]`, where entity values may hold a `>`).
fn after_doctype(bytes: &[u8]) -> Option<&[u8]> {
    let mut depth = 0_u32;
    for (at, byte) in bytes.iter().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            b'>' if depth == 0 => return bytes.get(at + 1..),
            _ => {}
        }
    }
    None
}

// --- the name ---

/// What an image is called when the link's path leaves no name: `image.<ext>`.
fn fallback_name(kind: ImageKind) -> String {
    format!("image.{}", kind.extension())
}

/// The name the attachment gets: the last segment of the link's path, percent-decoded, with
/// the extension of the sniffed `kind` (kept when it already names the kind, in whatever case;
/// replaced when it is a short word with a letter in it, as `.php` or `.v2`; else added), then
/// sanitised like the name of a saved file ([`names::sanitised_name`]: separators and control
/// and invisible characters removed, `< > : " | ? *` turned into `_`, device names prefixed,
/// at most 255 bytes). `image.<ext>` when nothing is left of the segment.
pub fn derive_name(url: &Url, kind: ImageKind) -> String {
    let segment = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let decoded = percent_decode(segment);
    let decoded = decoded.trim();
    let candidate = match decoded.rsplit_once('.') {
        Some((stem, extension)) if !stem.trim().is_empty() => {
            if kind.names_extension(extension) {
                decoded.to_owned()
            } else if is_replaceable_extension(extension) {
                format!("{}.{}", stem.trim_end(), kind.extension())
            } else {
                format!("{decoded}.{}", kind.extension())
            }
        }
        // `.png`, `.hidden`: an extension and no stem is not a name.
        Some(_) => String::new(),
        None if decoded.is_empty() => String::new(),
        None => format!("{decoded}.{}", kind.extension()),
    };
    let fallback = fallback_name(kind);
    match names::sanitised_name(&candidate) {
        // Nothing but the extension is left (the stem was all invisible characters).
        Some(name) if !name.starts_with('.') => name,
        _ => fallback,
    }
}

/// A short word with a letter in it: the extension a script or a version leaves in a path
/// (`image.php`, `photo.v2`), which the sniffed type then replaces. `2026.10.09` has none.
fn is_replaceable_extension(extension: &str) -> bool {
    (1..=5).contains(&extension.len())
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && extension.bytes().any(|byte| byte.is_ascii_alphabetic())
}

/// The text with its `%XX` escapes decoded as UTF-8 (a sequence that is not valid UTF-8 loses
/// the characters it breaks). `+` stays a plus: this is a path, not a form.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        let escaped = (byte == b'%')
            .then(|| {
                let high = bytes.get(at + 1).copied().and_then(hex_value)?;
                let low = bytes.get(at + 2).copied().and_then(hex_value)?;
                Some(high * 16 + low)
            })
            .flatten();
        match escaped {
            Some(value) => {
                decoded.push(value);
                at += 3;
            }
            None => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    match String::from_utf8(decoded) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes())
            .chars()
            .filter(|c| *c != char::REPLACEMENT_CHARACTER)
            .collect(),
    }
}

fn hex_value(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

// --- the download ---

/// A picture that came back from a link, checked: what the core is then given.
#[derive(Debug)]
pub struct Downloaded {
    /// The attachment's name ([`derive_name`]).
    pub name: String,
    /// What the bytes are.
    pub kind: ImageKind,
    /// The picture itself, at most the policy's limit.
    pub bytes: Vec<u8>,
}

/// Fetches the picture at `link` as the module documentation says. Nothing here touches the
/// core.
pub async fn download(link: &str, policy: &FetchPolicy) -> Result<Downloaded, ApiError> {
    let url = parse_url(link, policy.allow_http)?;
    let client = build_client(policy)?;
    let response = client
        .get(url.clone())
        .header(ACCEPT, "image/*")
        .send()
        .await
        .map_err(|error| transport_failure(&error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(failure(
            Reason::Status,
            &format!(
                "the server answered with an error (HTTP {})",
                status.as_u16()
            ),
            Some(status.as_u16()),
        ));
    }
    if let Some(length) = response.content_length()
        && length > policy.max_bytes
    {
        return Err(limit_exceeded("size", TOO_LARGE, policy.max_bytes));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = read_body(response, content_type.as_deref(), policy.max_bytes).await?;
    let kind = sniff_image(&bytes, content_type.as_deref())
        .ok_or_else(|| failure(Reason::NotImage, NOT_AN_IMAGE, None))?;
    Ok(Downloaded {
        name: derive_name(&url, kind),
        kind,
        bytes,
    })
}

/// The client of one download: no cookies, no `Referer`, the shell's user agent, the
/// redirect rules and the timeouts of the policy.
fn build_client(policy: &FetchPolicy) -> Result<Client, ApiError> {
    let allow_http = policy.allow_http;
    let redirects = Policy::custom(move |attempt| {
        match redirect_check(attempt.previous().len(), attempt.url(), allow_http) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }
    });
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .redirect(redirects)
        .referer(false)
        .https_only(!allow_http)
        .connect_timeout(policy.connect_timeout)
        .timeout(policy.total_timeout);
    if !policy.use_system_proxy {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|_| {
        // The error can name the proxy settings of the machine: not formatted.
        ApiError::internal("the download could not be set up")
    })
}

/// Whether a redirect may be followed: `followed` is how many addresses the request has been
/// through (the one it began with included), `target` where the server now sends it. At most
/// [`MAX_REDIRECTS`] redirects, each to an address [`parse_url`] would accept (a scheme that is
/// allowed, no credentials); the reason it is not is a short text without the address.
fn redirect_check(followed: usize, target: &Url, allow_http: bool) -> Result<(), &'static str> {
    if followed > MAX_REDIRECTS {
        Err("too many redirects")
    } else if !scheme_allowed(target.scheme(), allow_http) {
        Err("a redirect to an address that is not https")
    } else if has_credentials(target) {
        Err("a redirect to an address with credentials")
    } else {
        Ok(())
    }
}

/// Reads the body piece by piece, refusing more than `max_bytes`, and a body that cannot be
/// an image as soon as its first bytes show it.
async fn read_body(
    mut response: reqwest::Response,
    content_type: Option<&str>,
    max_bytes: u64,
) -> Result<Vec<u8>, ApiError> {
    let reserve = response
        .content_length()
        .map_or(0, |length| length.min(RESERVE_BYTES).min(max_bytes));
    let mut bytes = Vec::with_capacity(usize::try_from(reserve).unwrap_or(0));
    let may_be_svg = is_svg_content_type(content_type);
    let mut checked = false;
    while let Some(piece) = response
        .chunk()
        .await
        .map_err(|error| transport_failure(&error))?
    {
        if (bytes.len() as u64).saturating_add(piece.len() as u64) > max_bytes {
            return Err(limit_exceeded("size", TOO_LARGE, max_bytes));
        }
        bytes.extend_from_slice(&piece);
        if !checked && bytes.len() >= SIGNATURE_BYTES {
            checked = true;
            if !may_be_svg && sniff_binary(&bytes).is_none() {
                return Err(failure(Reason::NotImage, NOT_AN_IMAGE, None));
            }
        }
    }
    Ok(bytes)
}

/// A failed request or a failed read of the body as an error. `reqwest`'s own messages name
/// the URL, so only what kind of failure it was is kept.
fn transport_failure(error: &reqwest::Error) -> ApiError {
    if error.is_redirect() {
        failure(Reason::Redirect, BAD_REDIRECT, None)
    } else if error.is_timeout() {
        failure(Reason::Timeout, TIMED_OUT, None)
    } else {
        failure(Reason::Network, NETWORK_FAILED, None)
    }
}

// --- into the core ---

/// Streams a download through `AttachmentBegin`, `attachment_write` (in `CHUNK_SIZE` pieces)
/// and `AttachmentFinish`; aborts the upload when anything fails.
pub fn import_downloaded(
    host: &dyn CoreHost,
    space_id: SpaceId,
    image: Downloaded,
    max_bytes: u64,
) -> Result<AttachmentInfo, ApiError> {
    let mime = image.kind.mime().to_owned();
    let attachment_id = begin(host, space_id, image.name, mime)?;
    let uploaded = stream_pieces(
        host,
        attachment_id,
        &mut Cursor::new(image.bytes),
        max_bytes,
    )
    .and_then(|()| finish(host, attachment_id));
    if uploaded.is_err() {
        abort(host, attachment_id);
    }
    uploaded
}

/// The whole command: a core that is not ready answers first (no request is made), then the
/// download, then the import on a blocking thread. The log says the outcome and the size,
/// never the link.
pub async fn import_url(
    state: SharedState,
    space_id: SpaceId,
    link: String,
    policy: FetchPolicy,
) -> Result<AttachmentInfo, ApiError> {
    let ready = Arc::clone(&state);
    blocking(move || ready.ensure_ready(), ATTACHMENT_STOPPED).await?;
    let max_bytes = policy.max_bytes;
    let outcome = match download(&link, &policy).await {
        Ok(image) => {
            blocking(
                move || import_downloaded(&*state, space_id, image, max_bytes),
                ATTACHMENT_STOPPED,
            )
            .await
        }
        Err(error) => Err(error),
    };
    match &outcome {
        Ok(info) => tracing::info!(bytes = info.size, "image imported from a link"),
        Err(error) => tracing::warn!(
            code = ?error.code,
            reason = reason_of(error),
            "image import from a link failed"
        ),
    }
    outcome
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::convert::Infallible;
    use std::sync::Mutex;

    use axum::Router;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::get;
    use kb_types::ErrorCode;
    use kb_types::attachments::CHUNK_SIZE;

    use super::*;
    use crate::test_support::{AttachCore, SharedSpy, captured_logs, ready_attachment_state};

    fn space() -> SpaceId {
        SpaceId::from_bytes([9; 16])
    }

    fn details(error: &ApiError, key: &str) -> Option<serde_json::Value> {
        error.details.as_ref()?.get(key).cloned()
    }

    fn reason(error: &ApiError) -> String {
        details(error, "reason")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn field(error: &ApiError) -> String {
        details(error, "field")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    // The smallest recognisable heads of each format.
    fn png() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(b"\0\0\0\rIHDR rest of the picture");
        bytes
    }

    fn jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0]
    }

    fn webp() -> Vec<u8> {
        b"RIFF\x24\0\0\0WEBPVP8 ".to_vec()
    }

    fn bmp() -> Vec<u8> {
        let mut bytes = b"BM\x46\0\0\0\0\0\0\0\x36\0\0\0".to_vec();
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 32]);
        bytes
    }

    fn avif(brand: &[u8; 4]) -> Vec<u8> {
        let mut bytes = b"\0\0\0\x1Cftyp".to_vec();
        bytes.extend_from_slice(brand);
        bytes.extend_from_slice(b"\0\0\0\0mif1avif");
        bytes
    }

    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>";

    // --- the link ---

    #[test]
    fn an_https_link_is_accepted_and_normalised() {
        let parsed = parse_url("  https://Example.com/a/b.png?x=1#top \n", false).unwrap();
        assert_eq!(parsed.as_str(), "https://example.com/a/b.png?x=1#top");
        assert!(parse_url("https://[2001:db8::1]:8443/p.png", false).is_ok());
    }

    #[test]
    fn only_https_links_are_accepted() {
        for link in [
            "http://example.com/a.png",
            "file:///C:/Users/me/a.png",
            "data:image/png;base64,iVBORw0KGgo=",
            "ftp://example.com/a.png",
            "javascript:alert(1)",
            "blob:https://example.com/1234",
            "mailto:me@example.com",
        ] {
            let error = parse_url(link, false).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{link}");
            assert_eq!(field(&error), URL_FIELD, "{link}");
            assert!(!error.message.contains("example.com"), "{link}");
        }
        // The policy of the tests allows plain http, and nothing else.
        assert!(parse_url("http://127.0.0.1:8080/a.png", true).is_ok());
        assert!(parse_url("ftp://127.0.0.1/a.png", true).is_err());
    }

    #[test]
    fn credentials_empty_and_malformed_links_are_refused() {
        for link in [
            "https://user@example.com/a.png",
            "https://user:secret@example.com/a.png",
            "https://:secret@example.com/a.png",
            "",
            "   ",
            "example.com/a.png",
            "//example.com/a.png",
            "https://",
            "https://exa mple.com/a.png",
            "not a link",
        ] {
            let error = parse_url(link, false).unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{link:?}");
            assert_eq!(field(&error), URL_FIELD, "{link:?}");
            assert!(!error.message.contains("secret"), "{link:?}");
        }
    }

    #[test]
    fn a_link_is_at_most_2048_characters() {
        let prefix = "https://example.com/";
        let at_limit = format!("{prefix}{}", "a".repeat(MAX_URL_CHARS - prefix.len()));
        assert_eq!(at_limit.chars().count(), MAX_URL_CHARS);
        assert!(parse_url(&at_limit, false).is_ok());
        let error = parse_url(&format!("{at_limit}a"), false).unwrap_err();
        assert_eq!(field(&error), URL_FIELD);
        assert!(error.message.contains("2048"), "{}", error.message);
        // Characters, not bytes.
        let wide = format!("{prefix}{}", "é".repeat(MAX_URL_CHARS - prefix.len()));
        assert!(parse_url(&wide, false).is_ok());
    }

    // --- sniffing ---

    #[test]
    fn each_binary_format_is_recognised_by_its_signature() {
        for (bytes, kind) in [
            (png(), ImageKind::Png),
            (jpeg(), ImageKind::Jpeg),
            (b"GIF87a\x01\0\x01\0".to_vec(), ImageKind::Gif),
            (b"GIF89a\x01\0\x01\0".to_vec(), ImageKind::Gif),
            (webp(), ImageKind::Webp),
            (bmp(), ImageKind::Bmp),
            (avif(b"avif"), ImageKind::Avif),
            (avif(b"avis"), ImageKind::Avif),
        ] {
            assert_eq!(sniff_image(&bytes, None), Some(kind), "{kind:?}");
            // Whatever the server says: the bytes decide.
            assert_eq!(
                sniff_image(&bytes, Some("text/html")),
                Some(kind),
                "{kind:?}"
            );
            assert_eq!(
                sniff_image(&bytes, Some("image/svg+xml")),
                Some(kind),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn things_that_are_not_images_are_not_recognised() {
        let mut heic = avif(b"heic");
        heic.truncate(20);
        let mut riff_wave = b"RIFF\x24\0\0\0WAVEfmt ".to_vec();
        riff_wave.truncate(16);
        for (what, bytes) in [
            ("empty", Vec::new()),
            (
                "html",
                b"<!DOCTYPE html><html><body>hi</body></html>".to_vec(),
            ),
            ("json", br#"{"error":"not found"}"#.to_vec()),
            (
                "text starting with BM",
                b"BMW is a car maker, not a bitmap".to_vec(),
            ),
            ("a short PNG head", PNG_SIGNATURE[..7].to_vec()),
            ("zip", b"PK\x03\x04\x14\0\0\0".to_vec()),
            ("pdf", b"%PDF-1.7\n".to_vec()),
            ("heic", heic),
            ("wave", riff_wave),
            ("svg without a type", SVG.to_vec()),
        ] {
            assert_eq!(sniff_image(&bytes, None), None, "{what}");
            assert_eq!(sniff_image(&bytes, Some("image/png")), None, "{what}");
        }
    }

    #[test]
    fn svg_needs_the_server_to_say_so_and_the_body_to_be_an_svg_document() {
        let svg = Some("image/svg+xml");
        assert_eq!(sniff_image(SVG, svg), Some(ImageKind::Svg));
        assert_eq!(
            sniff_image(SVG, Some("Image/SVG+XML; charset=utf-8")),
            Some(ImageKind::Svg)
        );
        assert_eq!(
            sniff_image(SVG, Some(" image/svg+xml ;x=y")),
            Some(ImageKind::Svg)
        );
        for wrong in ["image/png", "text/xml", "text/html", "application/xml", ""] {
            assert_eq!(sniff_image(SVG, Some(wrong)), None, "{wrong:?}");
        }
        assert_eq!(sniff_image(SVG, None), None);
        // Said to be an SVG, but it is a page.
        assert_eq!(
            sniff_image(b"<html><svg></svg></html>", svg),
            None,
            "an svg inside a page is a page"
        );
        assert_eq!(sniff_image(b"", svg), None);
        assert_eq!(sniff_image(b"<svgfoo/>", svg), None);
        assert_eq!(sniff_image(b"<SVG/>", svg), None, "XML is case-sensitive");
        assert_eq!(sniff_image(b"<svg", svg), None);
    }

    #[test]
    fn an_svg_may_start_with_a_byte_order_mark_a_declaration_comments_and_a_doctype() {
        let svg = Some("image/svg+xml");
        let ok = |bytes: &[u8]| sniff_image(bytes, svg) == Some(ImageKind::Svg);
        assert!(ok(b"\xEF\xBB\xBF<svg/>"));
        assert!(ok(b"  \r\n\t<svg>\n</svg>"));
        assert!(ok(
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg width=\"1\"/>"
        ));
        assert!(ok(b"<?xml version=\"1.0\"?><!-- made by hand -->\n<svg/>"));
        assert!(ok(b"<!-- one --> <!-- two -->\n<svg/>"));
        assert!(ok(
            b"<?xml version=\"1.0\"?>\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd\">\n<svg/>"
        ));
        assert!(ok(
            b"<!DOCTYPE svg [ <!ENTITY a \"x>y\"> <!ENTITY b \"z\"> ]>\n<svg/>"
        ));
        assert!(ok(b"<?xml-stylesheet href=\"a.css\"?><svg/>"));
        assert!(ok(b"<svg\txmlns=\"http://www.w3.org/2000/svg\"/>"));
        // Unterminated prologue pieces are not documents.
        assert!(!ok(b"<?xml version=\"1.0\""));
        assert!(!ok(b"<!-- never closed <svg/>"));
        assert!(!ok(b"<!DOCTYPE svg [ <svg/>"));
        // A UTF-16 document is not read.
        assert!(!ok(b"\xFF\xFE<\0s\0v\0g\0/\0>\0"));
        // Text before the element.
        assert!(!ok(b"hello <svg/>"));
    }

    // --- names ---

    #[test]
    fn the_name_is_the_last_path_segment_with_the_extension_of_the_sniffed_type() {
        for (link, kind, name) in [
            (
                "https://example.com/photos/sunset.png",
                ImageKind::Png,
                "sunset.png",
            ),
            // An extension of another type is replaced.
            (
                "https://example.com/photos/sunset.jpg",
                ImageKind::Png,
                "sunset.png",
            ),
            (
                "https://example.com/photos/sunset.gif?size=big",
                ImageKind::Webp,
                "sunset.webp",
            ),
            // One that names the kind stays, in its own case.
            (
                "https://example.com/Sunset.JPG",
                ImageKind::Jpeg,
                "Sunset.JPG",
            ),
            (
                "https://example.com/sunset.jpeg",
                ImageKind::Jpeg,
                "sunset.jpeg",
            ),
            // None: added.
            ("https://example.com/render", ImageKind::Avif, "render.avif"),
            (
                "https://example.com/render?id=5#x",
                ImageKind::Jpeg,
                "render.jpg",
            ),
            // A script or a version is replaced; digits are not an extension.
            ("https://example.com/image.php", ImageKind::Png, "image.png"),
            ("https://example.com/photo.v2", ImageKind::Png, "photo.png"),
            (
                "https://example.com/2026.10.09",
                ImageKind::Png,
                "2026.10.09.png",
            ),
            (
                "https://example.com/archive.tar.gz",
                ImageKind::Png,
                "archive.tar.png",
            ),
            // Percent-decoded.
            (
                "https://example.com/my%20holiday%20%28final%29.png",
                ImageKind::Png,
                "my holiday (final).png",
            ),
            (
                "https://example.com/caf%C3%A9.png",
                ImageKind::Png,
                "café.png",
            ),
            ("https://example.com/a+b.png", ImageKind::Png, "a+b.png"),
            // Sanitised like a saved file.
            (
                "https://example.com/a%3Cb%3Ec%3Ad%22e%7Cf%3Fg%2Ah.png",
                ImageKind::Png,
                "a_b_c_d_e_f_g_h.png",
            ),
            (
                "https://example.com/a%2Fb%5Cc.png",
                ImageKind::Png,
                "abc.png",
            ),
            (
                "https://example.com/%00evil%07.png",
                ImageKind::Png,
                "evil.png",
            ),
            (
                "https://example.com/invoice%E2%80%AEfdp.png",
                ImageKind::Png,
                "invoicefdp.png",
            ),
            ("https://example.com/CON", ImageKind::Png, "_CON.png"),
            ("https://example.com/nul.txt", ImageKind::Png, "_nul.png"),
            // A broken escape stays as typed; invalid UTF-8 loses what it breaks.
            ("https://example.com/100%.png", ImageKind::Png, "100%.png"),
            ("https://example.com/a%FFb.png", ImageKind::Png, "ab.png"),
        ] {
            assert_eq!(derive_name(&url(link), kind), name, "{link}");
        }
    }

    #[test]
    fn a_path_without_a_name_gives_image_dot_extension() {
        for link in [
            "https://example.com",
            "https://example.com/",
            "https://example.com/photos/",
            "https://example.com/.png",
            "https://example.com/.hidden",
            "https://example.com/%20%20",
            "https://example.com/%E2%80%AE.png",
            "https://example.com/..%2F..",
        ] {
            assert_eq!(
                derive_name(&url(link), ImageKind::Webp),
                "image.webp",
                "{link}"
            );
        }
        assert_eq!(
            derive_name(&url("https://example.com/"), ImageKind::Svg),
            "image.svg"
        );
        assert_eq!(
            derive_name(&url("https://example.com/"), ImageKind::Jpeg),
            "image.jpg"
        );
    }

    #[test]
    fn a_long_name_is_cut_to_the_limit_and_keeps_its_extension() {
        let long = "x".repeat(400);
        let name = derive_name(
            &url(&format!("https://example.com/{long}.jpg")),
            ImageKind::Png,
        );
        assert!(
            name.len() <= kb_types::attachments::MAX_NAME_BYTES,
            "{}",
            name.len()
        );
        assert!(name.ends_with(".png"), "{name}");
        assert!(name.starts_with("xxxx"), "{name}");
    }

    #[test]
    fn every_kind_has_a_mime_type_the_core_accepts() {
        for kind in [
            ImageKind::Png,
            ImageKind::Jpeg,
            ImageKind::Gif,
            ImageKind::Webp,
            ImageKind::Bmp,
            ImageKind::Avif,
            ImageKind::Svg,
        ] {
            assert!(kind.mime().starts_with("image/"), "{kind:?}");
            // The shell's own table agrees with the sniffed type.
            assert_eq!(
                names::mime_for_name(&format!("x.{}", kind.extension())),
                kind.mime(),
                "{kind:?}"
            );
        }
    }

    // --- a stub server ---

    /// What a stub route saw of the requests it got.
    #[derive(Default)]
    struct Seen {
        headers: Vec<HeaderMap>,
        hits: usize,
    }

    type SharedSeen = Arc<Mutex<Seen>>;

    /// Serves `app` on 127.0.0.1 and returns its base address (`http://127.0.0.1:<port>`).
    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}")
    }

    fn picture(content_type: &'static str, bytes: Vec<u8>) -> Response {
        ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
    }

    /// A stub with `/a.png` (a picture, recording what it was asked), `/page` (HTML), `/gone`
    /// (404), `/error` (500), `/redirect-ok` (to `/a.png`) and `/redirect-http` (to an `http`
    /// address that is not allowed, with a different scheme than the policy of the tests: an
    /// `ftp` one).
    async fn stub() -> (String, SharedSeen) {
        let seen = SharedSeen::default();
        let app = Router::new()
            .route(
                "/a.png",
                get(
                    |State(seen): State<SharedSeen>, headers: HeaderMap| async move {
                        let mut seen = seen.lock().unwrap();
                        seen.hits += 1;
                        seen.headers.push(headers);
                        picture("image/png", png())
                    },
                ),
            )
            .route(
                "/page",
                get(|| async { picture("text/html", vec![b'x'; 4096]) }),
            )
            .route("/gone", get(|| async { StatusCode::NOT_FOUND }))
            .route(
                "/error",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route(
                "/redirect-ok",
                get(|| async { Redirect::temporary("/a.png") }),
            )
            .route(
                "/redirect-ftp",
                get(|| async { Redirect::temporary("ftp://127.0.0.1/a.png") }),
            )
            .with_state(Arc::clone(&seen));
        (serve(app).await, seen)
    }

    fn policy() -> FetchPolicy {
        FetchPolicy::local_test(MAX_PREVIEW_BYTES, Duration::from_secs(20))
    }

    // --- the download ---

    #[tokio::test]
    async fn a_picture_is_downloaded_named_and_typed_from_its_bytes() {
        let (base, seen) = stub().await;
        let image = download(&format!("{base}/a.png"), &policy()).await.unwrap();
        assert_eq!(image.kind, ImageKind::Png);
        assert_eq!(image.name, "a.png");
        assert_eq!(image.bytes, png());

        let seen = seen.lock().unwrap();
        assert_eq!(seen.hits, 1);
        let headers = &seen.headers[0];
        assert_eq!(headers[header::ACCEPT], "image/*");
        assert_eq!(
            headers[header::USER_AGENT],
            concat!("atomik-kb/", env!("CARGO_PKG_VERSION"))
        );
        for absent in [header::REFERER, header::COOKIE, header::AUTHORIZATION] {
            assert!(headers.get(&absent).is_none(), "{absent} was sent");
        }
    }

    #[tokio::test]
    async fn the_type_comes_from_the_bytes_not_from_the_server() {
        let app = Router::new().route(
            "/photo.gif",
            get(|| async { picture("application/octet-stream", jpeg()) }),
        );
        let base = serve(app).await;
        let image = download(&format!("{base}/photo.gif"), &policy())
            .await
            .unwrap();
        assert_eq!(image.kind, ImageKind::Jpeg);
        // The extension of the link says gif; the bytes say jpeg.
        assert_eq!(image.name, "photo.jpg");
    }

    #[tokio::test]
    async fn an_svg_is_taken_only_when_the_server_says_so() {
        let app = Router::new()
            .route(
                "/logo.svg",
                get(|| async { picture("image/svg+xml; charset=utf-8", SVG.to_vec()) }),
            )
            .route(
                "/plain.svg",
                get(|| async { picture("text/plain", SVG.to_vec()) }),
            );
        let base = serve(app).await;
        let image = download(&format!("{base}/logo.svg"), &policy())
            .await
            .unwrap();
        assert_eq!(
            (image.kind, image.name.as_str()),
            (ImageKind::Svg, "logo.svg")
        );
        let error = download(&format!("{base}/plain.svg"), &policy())
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "notImage");
    }

    #[tokio::test]
    async fn a_page_is_not_an_image() {
        let (base, _) = stub().await;
        let error = download(&format!("{base}/page"), &policy())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), URL_FIELD);
        assert_eq!(reason(&error), "notImage");
        assert_eq!(error.message, NOT_AN_IMAGE);
    }

    #[tokio::test]
    async fn an_empty_answer_is_not_an_image() {
        let app = Router::new().route("/empty", get(|| async { StatusCode::OK }));
        let base = serve(app).await;
        let error = download(&format!("{base}/empty"), &policy())
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "notImage");
    }

    #[tokio::test]
    async fn a_body_that_cannot_be_an_image_is_refused_before_it_is_read_to_the_end() {
        // Chunked, with no Content-Length, far more than the limit and none of it a picture: the
        // refusal is "not an image", reached from the first piece, not "too large".
        let app = Router::new().route(
            "/endless",
            get(|| async {
                let piece = Bytes::from(vec![b'x'; 100]);
                let pieces = (0..1000).map(move |_| Ok::<_, Infallible>(piece.clone()));
                (
                    [(header::CONTENT_TYPE, "text/html")],
                    Body::from_stream(futures_util::stream::iter(pieces)),
                )
            }),
        );
        let base = serve(app).await;
        let tight = FetchPolicy::local_test(1024, Duration::from_secs(20));
        let error = download(&format!("{base}/endless"), &tight)
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "notImage");
    }

    #[tokio::test]
    async fn a_non_success_status_is_reported_with_its_code() {
        let (base, _) = stub().await;
        for (path, code) in [("/gone", 404), ("/error", 500)] {
            let error = download(&format!("{base}{path}"), &policy())
                .await
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidInput, "{path}");
            assert_eq!(field(&error), URL_FIELD, "{path}");
            assert_eq!(reason(&error), "status", "{path}");
            assert_eq!(details(&error, "status"), Some(json!(code)), "{path}");
            assert!(
                error.message.contains(&code.to_string()),
                "{}",
                error.message
            );
            assert!(!error.message.contains("127.0.0.1"), "{}", error.message);
        }
    }

    #[tokio::test]
    async fn a_redirect_to_an_allowed_address_is_followed() {
        let (base, seen) = stub().await;
        let image = download(&format!("{base}/redirect-ok"), &policy())
            .await
            .unwrap();
        assert_eq!(image.bytes, png());
        // The name is the one of the link that was given.
        assert_eq!(image.name, "redirect-ok.png");
        assert_eq!(seen.lock().unwrap().hits, 1);
    }

    #[tokio::test]
    async fn a_redirect_to_a_scheme_that_is_not_allowed_is_refused() {
        let (base, _) = stub().await;
        let error = download(&format!("{base}/redirect-ftp"), &policy())
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "redirect");
        assert_eq!(field(&error), URL_FIELD);
    }

    #[test]
    fn a_redirect_must_be_to_https_without_credentials_and_at_most_five_times() {
        let https = url("https://cdn.example.com/a.png");
        assert_eq!(redirect_check(1, &https, false), Ok(()));
        assert_eq!(redirect_check(MAX_REDIRECTS, &https, false), Ok(()));
        assert!(redirect_check(MAX_REDIRECTS + 1, &https, false).is_err());
        for target in [
            "http://cdn.example.com/a.png",
            "ftp://cdn.example.com/a.png",
            "file:///C:/a.png",
            "data:image/png;base64,AAAA",
            "https://user@cdn.example.com/a.png",
            "https://user:pw@cdn.example.com/a.png",
        ] {
            let why = redirect_check(1, &url(target), false).unwrap_err();
            assert!(!why.contains("cdn.example.com"), "{why}");
        }
        // The tests' own policy lets a stub server on plain http be a redirect target.
        assert_eq!(
            redirect_check(1, &url("http://127.0.0.1:1/a.png"), true),
            Ok(())
        );
        assert!(redirect_check(1, &url("ftp://127.0.0.1/a.png"), true).is_err());
    }

    #[tokio::test]
    async fn the_production_policy_refuses_a_plain_http_link_without_making_a_request() {
        let (base, seen) = stub().await;
        let error = download(&format!("{base}/a.png"), &FetchPolicy::production())
            .await
            .unwrap_err();
        assert_eq!(field(&error), URL_FIELD);
        assert_eq!(seen.lock().unwrap().hits, 0, "a request was made over http");
    }

    #[tokio::test]
    async fn more_than_five_redirects_are_refused() {
        let app = Router::new().route(
            "/hop/{n}",
            get(
                |axum::extract::Path(n): axum::extract::Path<u32>| async move {
                    if n == 0 {
                        picture("image/png", png())
                    } else {
                        Redirect::temporary(&format!("/hop/{}", n - 1)).into_response()
                    }
                },
            ),
        );
        let base = serve(app).await;
        // Five redirects are followed ...
        let image = download(&format!("{base}/hop/{MAX_REDIRECTS}"), &policy())
            .await
            .unwrap();
        assert_eq!(image.kind, ImageKind::Png);
        // ... the sixth is not.
        let error = download(&format!("{base}/hop/{}", MAX_REDIRECTS + 1), &policy())
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "redirect");
    }

    #[tokio::test]
    async fn an_answer_larger_than_the_limit_is_refused_from_its_content_length() {
        let app = Router::new().route(
            "/big.png",
            get(|| async {
                let mut bytes = png();
                bytes.resize(5000, 0);
                picture("image/png", bytes)
            }),
        );
        let base = serve(app).await;
        let small = FetchPolicy::local_test(4096, Duration::from_secs(20));
        let error = download(&format!("{base}/big.png"), &small)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(field(&error), "size");
        assert_eq!(details(&error, "limit"), Some(json!(4096)));
        // Exactly at the limit is fine.
        let exact = FetchPolicy::local_test(5000, Duration::from_secs(20));
        assert_eq!(
            download(&format!("{base}/big.png"), &exact)
                .await
                .unwrap()
                .bytes
                .len(),
            5000
        );
    }

    #[tokio::test]
    async fn an_answer_that_grows_past_the_limit_is_refused_while_it_is_read() {
        // Chunked: no Content-Length to look at.
        let app = Router::new().route(
            "/stream.png",
            get(|| async {
                let first = Bytes::from(png());
                let rest = Bytes::from(vec![0_u8; 1000]);
                let pieces = std::iter::once(first)
                    .chain((0..10).map(move |_| rest.clone()))
                    .map(Ok::<_, Infallible>);
                (
                    [(header::CONTENT_TYPE, "image/png")],
                    Body::from_stream(futures_util::stream::iter(pieces)),
                )
            }),
        );
        let base = serve(app).await;
        let small = FetchPolicy::local_test(4096, Duration::from_secs(20));
        let error = download(&format!("{base}/stream.png"), &small)
            .await
            .unwrap_err();
        assert_eq!(field(&error), "size");
        assert_eq!(details(&error, "limit"), Some(json!(4096)));
        // Large enough: the same stream is accepted whole.
        let roomy = FetchPolicy::local_test(1 << 20, Duration::from_secs(20));
        let image = download(&format!("{base}/stream.png"), &roomy)
            .await
            .unwrap();
        assert_eq!(image.bytes.len(), png().len() + 10_000);
    }

    #[tokio::test]
    async fn a_server_that_does_not_answer_in_time_is_a_timeout() {
        let app = Router::new().route(
            "/slow.png",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                picture("image/png", png())
            }),
        );
        let base = serve(app).await;
        let impatient = FetchPolicy::local_test(MAX_PREVIEW_BYTES, Duration::from_millis(250));
        let error = download(&format!("{base}/slow.png"), &impatient)
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "timeout");
        assert_eq!(field(&error), URL_FIELD);
    }

    #[tokio::test]
    async fn a_server_that_cannot_be_reached_is_a_network_error_that_does_not_name_it() {
        // A port nothing listens on: bind, note the port, close.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = download(&format!("http://127.0.0.1:{port}/a.png"), &policy())
            .await
            .unwrap_err();
        assert_eq!(reason(&error), "network");
        assert_eq!(field(&error), URL_FIELD);
        let text = format!("{error:?}");
        assert!(
            !text.contains("127.0.0.1") && !text.contains(&port.to_string()),
            "{text}"
        );
    }

    // --- into the core ---

    fn state_and_spy() -> (SharedState, SharedSpy) {
        let (state, spy) = ready_attachment_state();
        (Arc::new(state) as SharedState, spy)
    }

    fn calls(spy: &SharedSpy) -> Vec<String> {
        spy.lock().unwrap().calls.clone()
    }

    #[tokio::test]
    async fn an_image_from_a_link_becomes_an_attachment() {
        let (base, _) = stub().await;
        let (state, spy) = state_and_spy();

        let info = import_url(state, space(), format!("{base}/a.png"), policy())
            .await
            .unwrap();

        assert_eq!(info.name, "a.png");
        assert_eq!(info.mime, "image/png");
        assert_eq!(info.size, png().len() as u64);
        assert_eq!(info.space_id, space());
        assert_eq!(
            calls(&spy),
            [
                "begin".to_owned(),
                format!("write:{}", png().len()),
                "finish".to_owned()
            ]
        );
        let spy = spy.lock().unwrap();
        assert!(spy.uploads.is_empty());
        assert_eq!(spy.stored[&info.attachment_id].bytes, png());
    }

    #[tokio::test]
    async fn a_picture_larger_than_a_chunk_is_written_in_chunks() {
        let app = Router::new().route(
            "/big.png",
            get(|| async {
                let mut bytes = png();
                bytes.resize(CHUNK_SIZE + 10, 7);
                picture("image/png", bytes)
            }),
        );
        let base = serve(app).await;
        let (state, spy) = state_and_spy();
        let info = import_url(state, space(), format!("{base}/big.png"), policy())
            .await
            .unwrap();
        assert_eq!(info.size, (CHUNK_SIZE + 10) as u64);
        assert_eq!(
            calls(&spy),
            [
                "begin".to_owned(),
                format!("write:{CHUNK_SIZE}"),
                "write:10".to_owned(),
                "finish".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_download_never_reaches_the_core() {
        let (base, _) = stub().await;
        let (state, spy) = state_and_spy();
        for path in ["/gone", "/page"] {
            import_url(
                Arc::clone(&state),
                space(),
                format!("{base}{path}"),
                policy(),
            )
            .await
            .unwrap_err();
        }
        import_url(
            Arc::clone(&state),
            space(),
            "http://example.com/a.png".into(),
            FetchPolicy::production(),
        )
        .await
        .unwrap_err();
        assert!(calls(&spy).is_empty(), "{:?}", calls(&spy));
    }

    #[tokio::test]
    async fn a_failure_after_the_upload_began_aborts_it() {
        let (base, _) = stub().await;
        let (state, spy) = state_and_spy();
        spy.lock().unwrap().fail_write_on = Some(1);
        let error = import_url(state, space(), format!("{base}/a.png"), policy())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Storage);
        assert_eq!(
            calls(&spy),
            [
                "begin".to_owned(),
                format!("write:{}", png().len()),
                "abort".to_owned()
            ]
        );
        assert!(
            spy.lock().unwrap().uploads.is_empty(),
            "the upload was left behind"
        );
    }

    #[tokio::test]
    async fn a_core_that_is_not_ready_answers_before_any_request_is_made() {
        let (base, seen) = stub().await;
        // A state whose core has not opened yet.
        let starting: SharedState = Arc::new(crate::state::AppState::<AttachCore>::new());
        let error = import_url(starting, space(), format!("{base}/a.png"), policy())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.message.contains("starting"), "{}", error.message);
        assert_eq!(seen.lock().unwrap().hits, 0, "the server was asked");
    }

    // --- the log ---

    #[test]
    fn neither_the_link_nor_its_host_nor_the_content_reaches_the_log() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let logs = captured_logs(|| {
            runtime.block_on(async {
                let (base, _) = stub().await;
                let (state, _spy) = state_and_spy();
                // A success, a status, a page, a refused scheme and a refused credential.
                for tail in [
                    "/a.png?token=LINKSECRET",
                    "/gone?token=LINKSECRET",
                    "/page?token=LINKSECRET",
                ] {
                    let _ = import_url(
                        Arc::clone(&state),
                        space(),
                        format!("{base}{tail}"),
                        policy(),
                    )
                    .await;
                }
                let _ = import_url(
                    Arc::clone(&state),
                    space(),
                    "https://user:LINKSECRET@host.invalid/LINKSECRET.png".into(),
                    policy(),
                )
                .await;
            });
        });
        // The shell's own lines. The HTTP stack's (hyper-util logs the address it connects to at
        // `debug`) are held at `warn` by the app's log filter, which `lib.rs` tests.
        let ours: Vec<&str> = logs
            .lines()
            .filter(|line| line.contains("atomik_kb_desktop_lib"))
            .collect();
        let ours = ours.join("\n");
        assert!(ours.contains("image imported from a link"), "{logs}");
        assert!(ours.contains("image import from a link failed"), "{logs}");
        assert!(ours.contains("reason"), "{logs}");
        for secret in [
            "LINKSECRET",
            "127.0.0.1",
            "host.invalid",
            "a.png",
            "image/png",
        ] {
            assert!(!ours.contains(secret), "{secret} reached the log: {ours}");
        }
    }
}
