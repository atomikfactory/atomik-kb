//! The HTTP transport of `kb-drive`'s requests (ADR-0059, `docs/architecture/drive-backup.md` section 3.4): a
//! `kb_drive::HttpRequest` becomes a `reqwest` request and the answer a `kb_drive::HttpResponse`.
//!
//! - **Production**: `https` only, the system's proxy settings (as `url_import`'s production policy), the user
//!   agent `atomik-kb/<version>`, 10 s to connect, 60 s for a request (an upload chunk gets that plus the time its
//!   bytes take at [`MIN_UPLOAD_RATE`]), and no total time on a download's body: it is read piece by piece with a
//!   60 s read timeout. No cookies, no `Referer`. A request goes only to a `*.googleapis.com` host, so a
//!   `Location` that Google answered cannot send the token elsewhere, and an API request is never redirected; a
//!   download follows at most [`MAX_REDIRECTS`] redirects, each `https` to a Google host (`reqwest` drops the
//!   `Authorization` header when a redirect changes the host).
//! - **Tests**: [`Config::local`] allows plain `http` to `127.0.0.1`, no proxy, and rewrites the two origins of
//!   Google (`https://oauth2.googleapis.com`, `https://www.googleapis.com`) to a fake server on `127.0.0.1`,
//!   with short timeouts and no waiting between retries. Nothing else changes.
//! - **Logs**: the kind of failure, never an address, a header or a body.

use std::time::Duration;

use kb_drive::{DriveError, HttpRequest, HttpResponse};
use kb_types::ApiError;
use reqwest::redirect::Policy;
use reqwest::{Client, Method, Response, Url};

use super::{Fail, RETRIES};

/// What the shell calls itself to Google: `atomik-kb/<version>`.
const USER_AGENT: &str = concat!("atomik-kb/", env!("CARGO_PKG_VERSION"));

/// How many redirects a download follows.
const MAX_REDIRECTS: usize = 5;

/// The slowest upload the time allowed for a chunk assumes, in bytes per second (64 KiB/s: an 8 MiB chunk
/// may take about two minutes on top of the 60 s of a plain request).
pub(super) const MIN_UPLOAD_RATE: u64 = 64 * 1024;

/// The longest answer read into memory (the API's answers are small; a download is streamed instead).
const MAX_BODY: usize = 8 * 1024 * 1024;

/// How much of an error's body is read to tell what it means.
const MAX_ERROR_BODY: usize = 64 * 1024;

/// The Google origins `kb-drive` builds its URLs from, which the tests replace.
const OAUTH_ORIGIN: &str = "https://oauth2.googleapis.com";
const API_ORIGIN: &str = "https://www.googleapis.com";

/// The domains a request may go to (a host that is one of them or a sub-domain of one).
const API_DOMAINS: [&str; 1] = ["googleapis.com"];

/// The domains a download may be redirected to.
const DOWNLOAD_DOMAINS: [&str; 3] = ["googleapis.com", "googleusercontent.com", "google.com"];

/// How the Drive host talks to Google and how long it waits. [`Config::production`] is what the app uses.
#[derive(Debug, Clone)]
pub(super) struct Config {
    /// Plain `http` to `127.0.0.1` is allowed (tests only).
    pub(super) allow_http: bool,
    /// The system's proxy settings apply (they do for the app).
    pub(super) system_proxy: bool,
    /// Replaces `https://oauth2.googleapis.com` (tests only).
    pub(super) oauth_base: Option<String>,
    /// Replaces `https://www.googleapis.com` (tests only).
    pub(super) api_base: Option<String>,
    /// How long the connection (DNS, TCP, TLS) may take.
    pub(super) connect_timeout: Duration,
    /// How long a request and its answer may take.
    pub(super) request_timeout: Duration,
    /// How long a read of a body (a download's piece) may wait for the next bytes.
    pub(super) read_timeout: Duration,
    /// The slowest upload speed, bytes per second, that the time for a chunk's body allows for.
    pub(super) min_upload_rate: u64,
    /// The waits before each resumption of an upload: 1, 2, 4, 8 and 16 seconds.
    pub(super) backoff: [Duration; RETRIES],
    /// How long the browser has to bring the sign-in back to the loopback address.
    pub(super) browser_timeout: Duration,
}

impl Config {
    /// What the app uses.
    pub(super) const fn production() -> Self {
        Self {
            allow_http: false,
            system_proxy: true,
            oauth_base: None,
            api_base: None,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            read_timeout: Duration::from_secs(60),
            min_upload_rate: MIN_UPLOAD_RATE,
            backoff: [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
            ],
            browser_timeout: Duration::from_secs(5 * 60),
        }
    }

    /// A fake Google at `base` (`http://127.0.0.1:<port>`): plain `http`, no proxy, short timeouts, no waiting.
    #[cfg(test)]
    pub(super) fn local(base: &str) -> Self {
        Self {
            allow_http: true,
            system_proxy: false,
            oauth_base: Some(base.to_owned()),
            api_base: Some(base.to_owned()),
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(5),
            min_upload_rate: MIN_UPLOAD_RATE,
            backoff: [Duration::ZERO; RETRIES],
            browser_timeout: Duration::from_secs(5),
        }
    }

    /// The URL a request is really sent to: the tests' fake server for the two origins of Google.
    fn rewrite(&self, url: &str) -> String {
        for (origin, base) in [
            (OAUTH_ORIGIN, self.oauth_base.as_deref()),
            (API_ORIGIN, self.api_base.as_deref()),
        ] {
            if let (Some(base), Some(rest)) = (base, url.strip_prefix(origin)) {
                return format!("{}{rest}", base.trim_end_matches('/'));
            }
        }
        url.to_owned()
    }
}

/// Whether `host` is one of `domains` or a sub-domain of one (or, for the tests, this machine).
fn host_allowed(allow_http: bool, host: &str, domains: &[&str]) -> bool {
    if allow_http {
        return host == "127.0.0.1";
    }
    domains.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

/// Whether `url` has a scheme and a host that may be used, and no credentials.
fn url_allowed(allow_http: bool, url: &Url, domains: &[&str]) -> bool {
    let scheme_ok = url.scheme() == "https" || (allow_http && url.scheme() == "http");
    scheme_ok
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(|host| host_allowed(allow_http, host, domains))
}

/// What a failed request is called in the log (never its address).
fn kind_of(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_builder() {
        "builder"
    } else if error.is_body() || error.is_decode() {
        "body"
    } else {
        "other"
    }
}

/// A failed request or read as a [`Fail`]: only that it was the network is kept.
fn network_failure(error: &reqwest::Error) -> Fail {
    tracing::warn!(kind = kind_of(error), "a request to Google failed");
    Fail::Network
}

fn protocol(text: &str) -> Fail {
    Fail::Drive(DriveError::Protocol(text.to_owned()))
}

/// The client of the Drive host: one for the API (never redirected) and one for downloads.
pub(super) struct Net {
    config: Config,
    api: Client,
    download: Client,
}

impl Net {
    /// Builds the clients.
    ///
    /// # Errors
    ///
    /// `Internal` when the HTTP stack cannot be set up (its text can name the machine's proxy: not kept).
    pub(super) fn new(config: Config) -> Result<Self, ApiError> {
        let builder = |redirects: Policy| {
            let mut builder = Client::builder()
                .user_agent(USER_AGENT)
                .redirect(redirects)
                .referer(false)
                .https_only(!config.allow_http)
                .connect_timeout(config.connect_timeout);
            if !config.system_proxy {
                builder = builder.no_proxy();
            }
            builder.build()
        };
        let allow_http = config.allow_http;
        let follow = Policy::custom(move |attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if url_allowed(allow_http, attempt.url(), &DOWNLOAD_DOMAINS) {
                attempt.follow()
            } else {
                attempt.error("a redirect that is not allowed")
            }
        });
        let (api, download) = match (builder(Policy::none()), builder(follow)) {
            (Ok(api), Ok(download)) => (api, download),
            _ => {
                return Err(ApiError::internal(
                    "the connection to Google could not be set up",
                ));
            }
        };
        Ok(Self {
            config,
            api,
            download,
        })
    }

    /// How long a read of a body may wait.
    pub(super) fn read_timeout(&self) -> Duration {
        self.config.read_timeout
    }

    /// The URL to send to, or why it may not be used.
    fn target(&self, url: &str, domains: &[&str]) -> Result<Url, Fail> {
        let url = Url::parse(&self.config.rewrite(url)).map_err(|_| protocol("a bad address"))?;
        if url_allowed(self.config.allow_http, &url, domains) {
            Ok(url)
        } else {
            Err(protocol("an address that is not Google's"))
        }
    }

    /// Sends a request and reads the whole answer (at most 8 MiB). Any status is an answer: `kb_drive`'s
    /// parsers say what it means. A transport failure is [`Fail::Network`].
    pub(super) async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Fail> {
        let url = self.target(&request.url, &API_DOMAINS)?;
        let method =
            Method::from_bytes(request.method.as_bytes()).map_err(|_| protocol("a bad method"))?;
        let body_time = Duration::from_secs(
            u64::try_from(request.body.len()).unwrap_or(u64::MAX)
                / self.config.min_upload_rate.max(1),
        );
        let mut builder = self
            .api
            .request(method, url)
            .timeout(self.config.request_timeout.saturating_add(body_time));
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if !request.body.is_empty() || matches!(request.method, "POST" | "PUT") {
            builder = builder.body(request.body);
        }
        let mut response = builder.send().await.map_err(|e| network_failure(&e))?;
        let status = response.status().as_u16();
        let headers = headers_of(&response);
        let body = self.read_body(&mut response, MAX_BODY).await?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Reads a body piece by piece, each within the read timeout, up to `limit` bytes.
    async fn read_body(&self, response: &mut Response, limit: usize) -> Result<Vec<u8>, Fail> {
        let mut body = Vec::new();
        loop {
            let piece = match tokio::time::timeout(self.config.read_timeout, response.chunk()).await
            {
                Ok(Ok(piece)) => piece,
                Ok(Err(error)) => return Err(network_failure(&error)),
                Err(_) => {
                    tracing::warn!(kind = "timeout", "a request to Google failed");
                    return Err(Fail::Network);
                }
            };
            let Some(piece) = piece else {
                return Ok(body);
            };
            if body.len().saturating_add(piece.len()) > limit {
                return Err(protocol("an answer that is too long"));
            }
            body.extend_from_slice(&piece);
        }
    }

    /// Starts a download: the answer's head is checked, and a status that is not a success is turned into the
    /// error `kb_drive` makes of it. A success is the response, whose body the caller reads piece by piece.
    pub(super) async fn download(&self, request: HttpRequest) -> Result<Response, Fail> {
        let url = self.target(&request.url, &DOWNLOAD_DOMAINS)?;
        let mut builder = self.download.get(url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let sent = tokio::time::timeout(self.config.read_timeout, builder.send()).await;
        let mut response = match sent {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(network_failure(&error)),
            Err(_) => {
                tracing::warn!(kind = "timeout", "a request to Google failed");
                return Err(Fail::Network);
            }
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let headers = headers_of(&response);
        // The body says why a 403 is (a full Drive, a limit); a failure to read it is no reason to hide the status.
        let body = self
            .read_body(&mut response, MAX_ERROR_BODY)
            .await
            .unwrap_or_default();
        let answer = HttpResponse {
            status,
            headers,
            body,
        };
        match kb_drive::classify(&answer) {
            Err(error) => Err(Fail::Drive(error)),
            Ok(()) => Err(protocol("a download that did not start")),
        }
    }
}

/// The headers of a response whose values are text.
fn headers_of(response: &Response) -> Vec<(String, String)> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_request_may_only_go_to_google() {
        let allowed = |url: &str| url_allowed(false, &Url::parse(url).unwrap(), &API_DOMAINS);
        assert!(allowed("https://www.googleapis.com/drive/v3/files"));
        assert!(allowed("https://oauth2.googleapis.com/token"));
        assert!(allowed("https://googleapis.com/x"));
        for bad in [
            "http://www.googleapis.com/drive/v3/files",
            "https://www.googleapis.com.evil.example/x",
            "https://evilgoogleapis.com/x",
            "https://example.com/x",
            "https://user:pass@www.googleapis.com/x",
            "https://127.0.0.1/x",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }

    #[test]
    fn a_download_may_follow_google_hosts_only() {
        let allowed = |url: &str| url_allowed(false, &Url::parse(url).unwrap(), &DOWNLOAD_DOMAINS);
        assert!(allowed("https://doc-0g-docs.googleusercontent.com/x"));
        assert!(allowed("https://drive.google.com/x"));
        assert!(!allowed("https://example.com/x"));
        assert!(!allowed("http://drive.google.com/x"));
    }

    #[test]
    fn the_tests_only_rewrite_the_two_google_origins() {
        let config = Config::local("http://127.0.0.1:9");
        assert_eq!(
            config.rewrite("https://oauth2.googleapis.com/token"),
            "http://127.0.0.1:9/token"
        );
        assert_eq!(
            config.rewrite("https://www.googleapis.com/drive/v3/files?a=b"),
            "http://127.0.0.1:9/drive/v3/files?a=b"
        );
        assert_eq!(
            config.rewrite("http://127.0.0.1:5/upload"),
            "http://127.0.0.1:5/upload"
        );
        // Production never rewrites anything.
        let production = Config::production();
        assert_eq!(
            production.rewrite("https://www.googleapis.com/x"),
            "https://www.googleapis.com/x"
        );
        assert!(!production.allow_http);
        assert!(production.system_proxy);
    }

    #[test]
    fn production_waits_one_two_four_eight_and_sixteen_seconds_between_resumptions() {
        let waits: Vec<u64> = Config::production()
            .backoff
            .iter()
            .map(Duration::as_secs)
            .collect();
        assert_eq!(waits, [1, 2, 4, 8, 16]);
    }
}
