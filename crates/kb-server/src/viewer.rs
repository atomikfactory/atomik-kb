//! The web viewer's files (M13 slice 4, ADR-0051, `docs/development/milestone-13.md` 4.1 and its contract
//! notes): `GET /view` and `GET /view/{*path}` serve the build of `apps/viewer` from the directory of
//! `KB_SERVER_VIEWER_DIR` (`Config::viewer_dir`), outside `/v2`.
//!
//! ```text
//! GET /view            index.html of the viewer directory (so does /view/)
//! GET /view/{*path}    a file below it (`assets/index-<hash>.js`, `kb_wasm_bg.wasm`, ...)
//! ```
//!
//! - **Why a directory** and not bytes in the binary: the build of the viewer needs node and the wasm
//!   toolchain, which `cargo build -p kb-server` must not. The Docker image builds the viewer in its own
//!   stage and sets the variable; `scripts/smoke/server.ps1` sets it when `apps/viewer/dist` exists.
//!   Unset, or a directory without `index.html`: every path of `/view` is a plain 404 ("not found") and
//!   nothing else changes.
//! - **Headers on every answer of `/view`**, the 404 too: `Content-Security-Policy: VIEWER_CSP` (the policy of
//!   ADR-0051, `kb_types::sharing::VIEWER_CSP`, word for word), `Referrer-Policy: no-referrer`,
//!   `X-Content-Type-Options: nosniff`, and a `Content-Type` from the extension (`text/html; charset=utf-8`,
//!   `text/javascript; charset=utf-8`, `text/css; charset=utf-8`, `application/wasm` (so the module can be
//!   streamed), `image/svg+xml`, `font/woff2`, `application/json`, `image/png` and a few more; anything else
//!   `application/octet-stream`). `Cache-Control: no-store` for HTML, `public, max-age=31536000, immutable`
//!   for the hashed files of `assets/`, `no-cache` for the rest.
//! - **Paths**: only files strictly below the directory are served. A segment that is empty or starts or
//!   ends with a dot, holds a backslash, a colon (a drive letter, a Windows stream), a NUL or another
//!   control or reserved character, a path that is not a file, and a symlink out of the directory are all
//!   the same 404 with the same body, never a different answer that tells them apart. The path is decoded
//!   by the router (`%2e%2e` is `..`) before it is checked, and the file found is canonicalised and must
//!   still lie below the canonical directory. Only `GET` (and `HEAD`) are routed; the query string is
//!   ignored and never logged.
//! - **The fragment never reaches the server**: the viewer reads `location.hash` in the browser, so the
//!   request line, the access log and the `loggable_path` hold `/view` and nothing of the key.
//! - **Rate limit**: none (static files); the link reads the viewer then makes are limited under
//!   `/v2/links`.

use std::path::Path as FsPath;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use kb_types::sharing::VIEWER_CSP;

use crate::api::AppState;

/// The largest file the viewer's directory may serve (the wasm module and the bundle are a few MiB).
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Hashed build output: the file name changes with its content.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// `GET /view` and `GET /view/`: the viewer's `index.html`.
pub(crate) async fn index(State(state): State<Arc<AppState>>) -> Response {
    serve(&state, "index.html".to_owned()).await
}

/// `GET /view/{*path}`: a file of the viewer's build.
pub(crate) async fn file(State(state): State<Arc<AppState>>, Path(path): Path<String>) -> Response {
    serve(&state, path).await
}

/// A file ready to be answered.
struct Loaded {
    bytes: Vec<u8>,
    content_type: &'static str,
    cache_control: &'static str,
}

async fn serve(state: &AppState, path: String) -> Response {
    let Some(root) = state.settings.viewer_dir.clone() else {
        return not_found();
    };
    match tokio::task::spawn_blocking(move || load(&root, &path)).await {
        Ok(Some(loaded)) => answer(loaded),
        _ => not_found(),
    }
}

/// The segments of a request path, or `None` for anything that could leave the directory or that no build
/// produces.
fn segments(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.len() > 512 {
        return None;
    }
    let mut out = Vec::new();
    for segment in path.split('/') {
        let bad = segment.is_empty()
            || segment.starts_with('.')
            || segment.ends_with(['.', ' '])
            || segment.chars().any(|c| {
                c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            });
        if bad {
            return None;
        }
        out.push(segment);
    }
    Some(out)
}

fn load(root: &FsPath, path: &str) -> Option<Loaded> {
    let segments = segments(path)?;
    let root = std::fs::canonicalize(root).ok()?;
    // A directory without its `index.html` is no viewer at all.
    if !root.join("index.html").is_file() {
        return None;
    }
    let mut target = root.clone();
    target.extend(segments.iter());
    // Symbolic links are resolved here: a file that lands outside the directory is not served.
    let target = std::fs::canonicalize(target).ok()?;
    if !target.starts_with(&root) {
        return None;
    }
    let meta = std::fs::metadata(&target).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(&target).ok()?;
    let extension = target
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let content_type = content_type(&extension);
    let cache_control = if content_type.starts_with("text/html") {
        "no-store"
    } else if segments.first() == Some(&"assets") {
        IMMUTABLE
    } else {
        "no-cache"
    };
    Some(Loaded {
        bytes,
        content_type,
        cache_control,
    })
}

fn content_type(extension: &str) -> &'static str {
    match extension {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" | "map" | "webmanifest" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// What every answer of `/view` carries: the policy of ADR-0051, no referrer, no sniffing.
fn hardened(
    status: StatusCode,
    content_type: &'static str,
    cache_control: &'static str,
) -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(VIEWER_CSP),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

fn answer(loaded: Loaded) -> Response {
    let mut response = hardened(StatusCode::OK, loaded.content_type, loaded.cache_control);
    *response.body_mut() = axum::body::Body::from(loaded.bytes);
    response
}

/// The one answer for everything that is not served.
fn not_found() -> Response {
    let mut response = hardened(
        StatusCode::NOT_FOUND,
        "text/plain; charset=utf-8",
        "no-store",
    );
    *response.body_mut() = axum::body::Body::from("not found\n");
    response.into_response()
}
