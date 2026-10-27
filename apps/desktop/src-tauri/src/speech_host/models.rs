//! The files of the model store: downloading a model, importing one from a folder, finishing an installation
//! and removing one (`docs/architecture/dictation.md` sections 4.5 and 4.6).
//!
//! - **Download** ([`download_model`]): every file of the catalog is fetched with `reqwest` (rustls) to
//!   `<model id>/<name>.part`. The URL is the catalog's; a redirect must be `https` and to Hugging Face
//!   ([`allowed_redirect`]); the answer must be 200; the SHA-256 is computed on the stream and the byte count
//!   checked against the catalog (a longer body is cut and fails as [`HashMismatch`], a shorter one is a
//!   [`DownloadFailed`]). The client has no cookies, the user agent `atomik-kb/<version>`, 10 s to connect and
//!   60 s without a byte, and the system's proxy settings (`HTTPS_PROXY`). Nothing is logged of the host, the URL
//!   or the redirects.
//! - **Progress** is throttled ([`Throttle`]) to ten events a second, plus one at the end.
//! - **Finishing** ([`commit_download`]): the old marker goes first, each `.part` is renamed into place, the
//!   marker is written last. A failure or a cancel removes every `.part` ([`clean_partials`]) and the folder when
//!   nothing else is in it.
//! - **Import** ([`import_folder`]): every file of the catalog must be in the chosen folder (a regular file, not
//!   a link) with its pinned size; each is then copied to `.part` while its SHA-256 is computed from the very
//!   bytes that are copied, and checked. Nothing is read outside the folder; the path never leaves the shell.
//! - **Removal** ([`remove_model`]) deletes the model's folder (a sidecar that has just quit may still hold a file
//!   for a moment, so it is tried again for three seconds).
//!
//! [`HashMismatch`]: DictationErrorCode::HashMismatch
//! [`DownloadFailed`]: DictationErrorCode::DownloadFailed

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kb_speech::api::DictationErrorCode;
use kb_speech::catalog::{ModelFile, ModelSpec};
use kb_types::ApiError;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode, Url};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::process::hex;
use super::{INSTALLED_FILE, dictation_error, write_installed_marker};

/// Redirects a download follows at most (Hugging Face needs one or two).
const MAX_REDIRECTS: usize = 6;

/// What the shell calls itself to the server: `atomik-kb/<version>`.
const USER_AGENT: &str = concat!("atomik-kb/", env!("CARGO_PKG_VERSION"));

/// The most progress events a second.
pub const PROGRESS_PER_SECOND: u32 = 10;

/// The suffix of a file that is still being written.
const PART: &str = "part";

/// How a download is made: where each file comes from, where a redirect may lead, and the timings. The commands
/// use [`DownloadPolicy::production`]; the tests point it at a stub server on `127.0.0.1`.
#[derive(Clone)]
pub struct DownloadPolicy {
    /// The URL of a file (the catalog's in production).
    pub url_for: Arc<dyn Fn(&ModelFile) -> String + Send + Sync>,
    /// Whether a redirect may be followed.
    pub allow_redirect: Arc<dyn Fn(&Url) -> bool + Send + Sync>,
    /// Whether the system's proxy settings apply (they do for the app; the tests talk to `127.0.0.1` directly).
    pub use_system_proxy: bool,
    /// How long the connection (DNS, TCP, TLS) may take.
    pub connect_timeout: Duration,
    /// How long a read may wait for the next bytes.
    pub read_timeout: Duration,
    /// The least time between two progress events.
    pub progress_interval: Duration,
}

impl DownloadPolicy {
    /// The catalog's URLs, redirects only to Hugging Face over `https`, the system's proxy settings.
    #[must_use]
    pub fn production() -> Self {
        Self {
            url_for: Arc::new(|file: &ModelFile| file.url.to_owned()),
            allow_redirect: Arc::new(allowed_redirect),
            use_system_proxy: true,
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(60),
            progress_interval: Duration::from_millis(1_000 / u64::from(PROGRESS_PER_SECOND)),
        }
    }
}

impl std::fmt::Debug for DownloadPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DownloadPolicy")
            .field("use_system_proxy", &self.use_system_proxy)
            .finish_non_exhaustive()
    }
}

/// Whether a download may follow a redirect to `url`: `https`, no credentials, and `huggingface.co`, a host
/// ending in `.huggingface.co`, `hf.co` or a host ending in `.hf.co` (Hugging Face serves large files from such
/// CDN hosts).
#[must_use]
pub fn allowed_redirect(url: &Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    host == "huggingface.co"
        || host.ends_with(".huggingface.co")
        || host == "hf.co"
        || host.ends_with(".hf.co")
}

/// The client of a download.
fn build_client(policy: &DownloadPolicy) -> Result<Client, DictationErrorCode> {
    let allow = Arc::clone(&policy.allow_redirect);
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(policy.connect_timeout)
        .read_timeout(policy.read_timeout)
        .redirect(Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if allow(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("a redirect left the allowed hosts")
            }
        }));
    if !policy.use_system_proxy {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|_| DictationErrorCode::DownloadFailed)
}

/// Lets an event through at most once in `interval`.
#[derive(Debug)]
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    /// A throttle with no event yet.
    #[must_use]
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    /// Whether an event at `now` may be sent (the first always may); when it may, it counts as sent.
    pub fn allow(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Where a file is written until it is complete.
fn part_name(file: &ModelFile) -> String {
    format!("{}.{PART}", file.name)
}

/// Removes the `.part` files of `spec` from `dir`, and `dir` when nothing else is in it.
pub fn clean_partials(dir: &Path, spec: &ModelSpec) {
    for file in spec.files {
        let _ = std::fs::remove_file(dir.join(part_name(file)));
    }
    // Only an empty folder goes.
    let _ = std::fs::remove_dir(dir);
}

fn store_error(error: &std::io::Error) -> DictationErrorCode {
    tracing::info!(kind = ?error.kind(), "the speech model store could not be written");
    DictationErrorCode::StoreFailed
}

/// Downloads every file of `spec` into `dir` as `.part` files, checking each against its size and SHA-256.
/// `progress(done, total)` is called as bytes arrive, at most [`PROGRESS_PER_SECOND`] times a second and once at
/// the end. The caller finishes ([`commit_download`]) or cleans up ([`clean_partials`]).
///
/// # Errors
///
/// The [`DictationErrorCode`]: `cancelled`, `downloadFailed`, `hashMismatch` or `storeFailed`.
pub async fn download_model(
    policy: &DownloadPolicy,
    dir: &Path,
    spec: &ModelSpec,
    cancel: &mut watch::Receiver<bool>,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<(), DictationErrorCode> {
    std::fs::create_dir_all(dir).map_err(|error| store_error(&error))?;
    let client = build_client(policy)?;
    let total = spec.total_bytes();
    let mut done = 0_u64;
    let mut throttle = Throttle::new(policy.progress_interval);
    let mut reported = None;
    for file in spec.files {
        fetch_file(&client, policy, dir, file, cancel, &mut |bytes| {
            done += bytes;
            if throttle.allow(Instant::now()) {
                reported = Some(done);
                progress(done, total);
            }
        })
        .await?;
    }
    if reported != Some(total) {
        progress(total, total);
    }
    Ok(())
}

fn is_cancelled(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

/// Resolves when a cancel is asked for (at once when it already was).
async fn cancel_requested(cancel: &mut watch::Receiver<bool>) {
    // An error means the sender is gone, which is not a cancel: wait for ever then.
    if cancel.wait_for(|cancelled| *cancelled).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Downloads one file to its `.part` and checks it.
async fn fetch_file(
    client: &Client,
    policy: &DownloadPolicy,
    dir: &Path,
    file: &ModelFile,
    cancel: &mut watch::Receiver<bool>,
    arrived: &mut (dyn FnMut(u64) + Send),
) -> Result<(), DictationErrorCode> {
    if is_cancelled(cancel) {
        return Err(DictationErrorCode::Cancelled);
    }
    let request = client.get((policy.url_for)(file)).send();
    let mut response = tokio::select! {
        sent = request => sent.map_err(|_| DictationErrorCode::DownloadFailed)?,
        () = cancel_requested(cancel) => return Err(DictationErrorCode::Cancelled),
    };
    if response.status() != StatusCode::OK {
        return Err(DictationErrorCode::DownloadFailed);
    }
    // A server that announces another size is not serving the pinned file.
    if response
        .content_length()
        .is_some_and(|length| length != file.bytes)
    {
        return Err(DictationErrorCode::HashMismatch);
    }
    let mut out =
        std::fs::File::create(dir.join(part_name(file))).map_err(|error| store_error(&error))?;
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| DictationErrorCode::DownloadFailed)?,
            () = cancel_requested(cancel) => return Err(DictationErrorCode::Cancelled),
        };
        let Some(bytes) = chunk else {
            break;
        };
        received += bytes.len() as u64;
        if received > file.bytes {
            // A longer body than the pinned file: cut and refused.
            return Err(DictationErrorCode::HashMismatch);
        }
        hasher.update(&bytes);
        out.write_all(&bytes).map_err(|error| store_error(&error))?;
        arrived(bytes.len() as u64);
        if is_cancelled(cancel) {
            return Err(DictationErrorCode::Cancelled);
        }
    }
    out.sync_all().map_err(|error| store_error(&error))?;
    drop(out);
    if received != file.bytes {
        return Err(DictationErrorCode::DownloadFailed);
    }
    if hex(&hasher.finalize()) != file.sha256 {
        return Err(DictationErrorCode::HashMismatch);
    }
    Ok(())
}

/// Makes the complete `.part` files of `spec` the model: the old marker goes first (a crash in the middle leaves
/// "not installed"), each file is renamed into place, the marker is written last.
///
/// # Errors
///
/// `storeFailed` when a file cannot be renamed or the marker cannot be written.
pub fn commit_download(dir: &Path, spec: &ModelSpec) -> Result<(), DictationErrorCode> {
    let _ = std::fs::remove_file(dir.join(INSTALLED_FILE));
    for file in spec.files {
        std::fs::rename(dir.join(part_name(file)), dir.join(file.name))
            .map_err(|error| store_error(&error))?;
    }
    write_installed_marker(dir, spec).map_err(|_| DictationErrorCode::StoreFailed)
}

/// Why importing a file failed, before it is an error of the command.
fn import_error(code: DictationErrorCode) -> ApiError {
    let message = match code {
        DictationErrorCode::ImportInvalid => "the chosen folder does not hold every model file",
        DictationErrorCode::HashMismatch => {
            "a model file in the chosen folder is not the expected one"
        }
        _ => "the speech model files could not be stored",
    };
    dictation_error("engine", code, message)
}

/// Copies `from` to `to` and gives the SHA-256 of the bytes copied (and how many).
fn copy_and_hash(from: &Path, to: &Path) -> Result<(u64, String), DictationErrorCode> {
    let mut source = std::fs::File::open(from).map_err(|_| DictationErrorCode::ImportInvalid)?;
    let mut target = std::fs::File::create(to).map_err(|error| store_error(&error))?;
    let mut hasher = Sha256::new();
    let mut block = vec![0_u8; 256 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = source
            .read(&mut block)
            .map_err(|_| DictationErrorCode::ImportInvalid)?;
        if read == 0 {
            break;
        }
        hasher.update(&block[..read]);
        target
            .write_all(&block[..read])
            .map_err(|error| store_error(&error))?;
        bytes += read as u64;
    }
    target.sync_all().map_err(|error| store_error(&error))?;
    Ok((bytes, hex(&hasher.finalize())))
}

/// Imports the model `spec` from `source` into `dir` (see the module documentation). Blocking.
///
/// # Errors
///
/// `InvalidInput` for `engine`, reason `importInvalid` (a file is missing, or is not a regular file),
/// `hashMismatch` (a file has another size or SHA-256) or `storeFailed`. Nothing partial is kept.
pub fn import_folder(dir: &Path, spec: &ModelSpec, source: &Path) -> Result<(), ApiError> {
    import_files(dir, spec, source).map_err(import_error)
}

fn import_files(dir: &Path, spec: &ModelSpec, source: &Path) -> Result<(), DictationErrorCode> {
    // Everything is looked at before anything is copied.
    for file in spec.files {
        let meta = std::fs::symlink_metadata(source.join(file.name))
            .map_err(|_| DictationErrorCode::ImportInvalid)?;
        if !meta.is_file() {
            return Err(DictationErrorCode::ImportInvalid);
        }
        if meta.len() != file.bytes {
            return Err(DictationErrorCode::HashMismatch);
        }
    }
    std::fs::create_dir_all(dir).map_err(|error| store_error(&error))?;
    let copied = (|| {
        for file in spec.files {
            let (bytes, hash) = copy_and_hash(&source.join(file.name), &dir.join(part_name(file)))?;
            if bytes != file.bytes || hash != file.sha256 {
                return Err(DictationErrorCode::HashMismatch);
            }
        }
        commit_download(dir, spec)
    })();
    if copied.is_err() {
        clean_partials(dir, spec);
    }
    copied
}

/// Deletes the model `spec` from the store: its folder, which holds its files and its marker.
///
/// # Errors
///
/// `InvalidInput` for `engine`, reason `storeFailed`, when the folder cannot be deleted.
pub fn remove_model(dir: &Path) -> Result<(), ApiError> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                if Instant::now() >= deadline {
                    tracing::info!(kind = ?error.kind(), "a speech model could not be removed");
                    return Err(import_error(DictationErrorCode::StoreFailed));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn a_redirect_may_only_go_to_hugging_face_over_https() {
        for allowed in [
            "https://huggingface.co/x/resolve/abc/f.onnx",
            "https://cdn-lfs.huggingface.co/repos/ab/cd",
            "https://cdn-lfs-us-1.hf.co/repos/ab/cd?token=1",
            "https://hf.co/x",
            "https://cas-bridge.xethub.hf.co/xet-bridge-us/abc",
            "https://HuggingFace.co/x",
        ] {
            assert!(allowed_redirect(&url(allowed)), "{allowed}");
        }
        for refused in [
            "http://huggingface.co/x",
            "https://evil.example/huggingface.co",
            "https://huggingface.co.evil.example/x",
            "https://nothuggingface.co/x",
            "https://xhf.co/x",
            "https://user:pw@huggingface.co/x",
            "https://huggingface.co@evil.example/x",
            "https://127.0.0.1/x",
            "ftp://huggingface.co/x",
            "https://[::1]/x",
        ] {
            assert!(!allowed_redirect(&url(refused)), "{refused}");
        }
    }

    #[test]
    fn the_throttle_lets_one_event_through_each_interval() {
        let mut throttle = Throttle::new(Duration::from_millis(100));
        let start = Instant::now();
        assert!(throttle.allow(start));
        assert!(!throttle.allow(start + Duration::from_millis(1)));
        assert!(!throttle.allow(start + Duration::from_millis(99)));
        assert!(throttle.allow(start + Duration::from_millis(100)));
        assert!(!throttle.allow(start + Duration::from_millis(150)));
        assert!(throttle.allow(start + Duration::from_millis(210)));
        // A burst of 1,000 events in one second lets at most 10 through.
        let mut throttle = Throttle::new(Duration::from_millis(100));
        let allowed = (0..1_000)
            .filter(|n| throttle.allow(start + Duration::from_millis(*n)))
            .count();
        assert_eq!(allowed, 10);
    }

    #[test]
    fn the_production_policy_follows_the_contract() {
        let policy = DownloadPolicy::production();
        assert!(policy.use_system_proxy);
        assert_eq!(policy.connect_timeout, Duration::from_secs(10));
        assert_eq!(policy.read_timeout, Duration::from_secs(60));
        assert_eq!(policy.progress_interval, Duration::from_millis(100));
        let file = &kb_speech::catalog::MOONSHINE_TINY.files[0];
        assert_eq!((policy.url_for)(file), file.url);
        assert!(file.url.starts_with("https://huggingface.co/"));
    }
}
