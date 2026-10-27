//! The loopback listener of the Google sign-in (ADR-0059, `docs/architecture/drive-backup.md` section 3.2).
//!
//! The browser comes back to `http://127.0.0.1:<port>/?code=...&state=...`: a listener bound to the loopback
//! interface only (port chosen by the system) reads that one `GET` with a tiny HTTP/1.1 reader and answers it
//! with a short page. What it accepts, in order:
//!
//! 1. A request whose `Host` is not `127.0.0.1:<port>` is refused (421), whatever else it says: a page in the
//!    browser cannot make the listener answer through another name (DNS rebinding).
//! 2. Anything but a `GET` of `/` with a query that carries the `state` we made is a 404 and the listener
//!    keeps waiting: a stray request (a favicon, a scan, a forged redirect without our state) neither ends the
//!    sign-in nor tells it anything.
//! 3. A query with our `state` and a `code` ends the wait with the code; `error=access_denied` ends it as
//!    refused. Either way the browser gets the page that says the tab can be closed, in the words of
//!    `dialog_drive_done`, and the listener is dropped (closed) at once.
//!
//! Every connection is served by a task of its own, and gets 5 s to send its head (at most 8 KiB) and 5 s to take
//! the answer: a browser's idle pre-connection, or any connection that sits silent, delays nothing (browsers open
//! one beside the real request). The first request that ends the wait wins. The wait as a whole ends after the
//! browser timeout (`browserTimeout`) or when the person cancels (`cancelled`). The query is never logged.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

use super::Reason;

/// The most bytes of a request's head that are read.
const MAX_HEAD: usize = 8 * 1024;

/// How long a connection has to send its head, and to take the answer.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

/// The listener, bound to `127.0.0.1` on a port the system chose.
pub(super) struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    /// Binds `127.0.0.1:0`.
    pub(super) async fn bind() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// `http://127.0.0.1:<port>`: what Google is told to redirect to.
    pub(super) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Waits for the redirect that carries `state`, and answers it with `done` as the page's text. Consumes the
    /// listener: it is closed when this returns.
    ///
    /// # Errors
    ///
    /// `Cancelled` when `cancel` turns true, `BrowserTimeout` after `timeout`, `Denied` when the person
    /// refused in the browser, `Server` for a redirect with our state that is neither a code nor a refusal.
    pub(super) async fn wait_for_code(
        self,
        state: &str,
        done: &str,
        cancel: &mut watch::Receiver<bool>,
        timeout: Duration,
    ) -> Result<String, Reason> {
        let (sender, mut outcomes) = mpsc::unbounded_channel();
        let wait = async {
            loop {
                tokio::select! {
                    accepted = self.listener.accept() => {
                        // A failed accept (a reset connection, say) is looked past.
                        if let Ok((stream, _)) = accepted {
                            let (port, state, done) = (self.port, state.to_owned(), done.to_owned());
                            let sender = sender.clone();
                            tokio::spawn(async move {
                                if let Some(outcome) = Self::serve(stream, port, &state, &done).await {
                                    // The answer is on its way to the browser by now: the wait may end.
                                    let _ = sender.send(outcome);
                                }
                            });
                        }
                    }
                    Some(outcome) = outcomes.recv() => return outcome,
                }
            }
        };
        tokio::select! {
            outcome = wait => outcome,
            () = super::cancelled_signal(cancel) => Err(Reason::Cancelled),
            () = tokio::time::sleep(timeout) => Err(Reason::BrowserTimeout),
        }
    }

    /// Reads one connection's request and answers it. `Some` ends the wait.
    async fn serve(
        mut stream: TcpStream,
        port: u16,
        state: &str,
        done: &str,
    ) -> Option<Result<String, Reason>> {
        let head = match tokio::time::timeout(CONNECTION_TIMEOUT, read_head(&mut stream)).await {
            Ok(Some(head)) => head,
            _ => {
                respond(&mut stream, 400, "Bad Request", "text/plain", "Bad Request").await;
                return None;
            }
        };
        let Some(request) = parse_head(&head) else {
            respond(&mut stream, 400, "Bad Request", "text/plain", "Bad Request").await;
            return None;
        };
        if !host_is(request.host, port) {
            respond(
                &mut stream,
                421,
                "Misdirected Request",
                "text/plain",
                "Misdirected Request",
            )
            .await;
            return None;
        }
        let Some(query) = redirect_query(&request) else {
            respond(&mut stream, 404, "Not Found", "text/plain", "Not Found").await;
            return None;
        };
        match kb_drive::parse_redirect(query, state) {
            Ok(code) => {
                respond(
                    &mut stream,
                    200,
                    "OK",
                    "text/html; charset=utf-8",
                    &page(done),
                )
                .await;
                Some(Ok(code))
            }
            Err(kb_drive::DriveError::Denied) => {
                respond(
                    &mut stream,
                    200,
                    "OK",
                    "text/html; charset=utf-8",
                    &page(done),
                )
                .await;
                Some(Err(Reason::Denied))
            }
            Err(kb_drive::DriveError::State) => {
                respond(&mut stream, 404, "Not Found", "text/plain", "Not Found").await;
                None
            }
            Err(_) => {
                respond(
                    &mut stream,
                    200,
                    "OK",
                    "text/html; charset=utf-8",
                    &page(done),
                )
                .await;
                Some(Err(Reason::Server))
            }
        }
    }
}

/// The head of a request, as text, read until its blank line.
async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut piece = [0_u8; 1024];
    loop {
        let read = stream.read(&mut piece).await.ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(piece.get(..read)?);
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(head).ok();
        }
        if head.len() > MAX_HEAD {
            return None;
        }
    }
}

/// The parts of a request's head the listener looks at.
struct Head<'a> {
    method: &'a str,
    target: &'a str,
    host: Option<&'a str>,
}

/// The request line and the `Host` header of a head (`HTTP/1.0` and `HTTP/1.1` only).
fn parse_head(head: &str) -> Option<Head<'_>> {
    let mut lines = head.split("\r\n");
    let mut line = lines.next()?.split(' ');
    let method = line.next()?;
    let target = line.next()?;
    let version = line.next()?;
    if line.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") || method.is_empty() {
        return None;
    }
    let host = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim());
    Some(Head {
        method,
        target,
        host,
    })
}

/// Whether the `Host` header names this listener: `127.0.0.1:<port>`.
fn host_is(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|host| host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}")))
}

/// The query of a `GET /?...`, when that is what the request is.
fn redirect_query<'a>(request: &Head<'a>) -> Option<&'a str> {
    if request.method != "GET" {
        return None;
    }
    let (path, query) = request.target.split_once('?')?;
    (path == "/" && !query.is_empty()).then_some(query)
}

/// The page the browser lands on: a title and one sentence, no script, no link, nothing loaded from anywhere.
fn page(done: &str) -> String {
    let text = escape(done);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>atomik-kb</title>\
         <style>body{{font-family:system-ui,sans-serif;margin:3rem auto;max-width:32rem;padding:0 1rem;\
         text-align:center;color:#222}}h1{{font-size:1.25rem}}</style></head>\
         <body><h1>atomik-kb</h1><p>{text}</p></body></html>"
    )
}

/// `text` safe inside an HTML element.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Writes one response and closes the connection. A write that fails or takes too long is dropped: the browser
/// already has what it needs or the connection is gone.
async fn respond(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\n\
         X-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    );
    let write = async {
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        stream.shutdown().await
    };
    let _ = tokio::time::timeout(CONNECTION_TIMEOUT, write).await;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_request_line_and_the_host_are_read() {
        let head = "GET /?code=a&state=b HTTP/1.1\r\nHost: 127.0.0.1:5000\r\nUser-Agent: x\r\n\r\n";
        let parsed = parse_head(head).unwrap();
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.target, "/?code=a&state=b");
        assert_eq!(parsed.host, Some("127.0.0.1:5000"));
        assert_eq!(redirect_query(&parsed), Some("code=a&state=b"));
    }

    #[test]
    fn the_host_header_is_found_in_any_case_and_a_missing_one_is_none() {
        let parsed = parse_head("GET / HTTP/1.1\r\nhOsT:   127.0.0.1:1  \r\n\r\n").unwrap();
        assert_eq!(parsed.host, Some("127.0.0.1:1"));
        let parsed = parse_head("GET / HTTP/1.1\r\nAccept: */*\r\n\r\n").unwrap();
        assert_eq!(parsed.host, None);
        assert!(!host_is(None, 1));
        assert!(host_is(Some("127.0.0.1:1"), 1));
        assert!(host_is(Some("127.0.0.1:1"), 1));
        for bad in [
            "localhost:1",
            "127.0.0.1:2",
            "127.0.0.1",
            "evil.example:1",
            "127.0.0.1:1.evil.example",
        ] {
            assert!(!host_is(Some(bad), 1), "{bad}");
        }
    }

    #[test]
    fn a_head_that_is_not_http_one_is_refused() {
        for bad in [
            "",
            "GET /",
            "GET / HTTP/2\r\n\r\n",
            "GET / HTTP/1.1 extra\r\n\r\n",
            " / HTTP/1.1\r\n\r\n",
            "BAD\r\n\r\n",
        ] {
            assert!(parse_head(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn only_a_get_of_the_root_with_a_query_is_a_redirect() {
        let query = |head: &str| {
            let owned = head.to_owned();
            parse_head(&owned).and_then(|parsed| redirect_query(&parsed).map(str::to_owned))
        };
        assert_eq!(
            query("GET /?code=1 HTTP/1.1\r\n\r\n").as_deref(),
            Some("code=1")
        );
        assert_eq!(query("GET / HTTP/1.1\r\n\r\n"), None);
        assert_eq!(query("GET /? HTTP/1.1\r\n\r\n"), None);
        assert_eq!(query("GET /favicon.ico HTTP/1.1\r\n\r\n"), None);
        assert_eq!(query("GET /other?code=1 HTTP/1.1\r\n\r\n"), None);
        assert_eq!(query("POST /?code=1 HTTP/1.1\r\n\r\n"), None);
    }

    #[test]
    fn the_page_escapes_the_words_it_is_given() {
        let html = page("a <b> & \"c\" 'd'");
        assert!(html.contains("a &lt;b&gt; &amp; &quot;c&quot; &#39;d&#39;"));
        assert!(!html.contains("<b>"));
        assert!(!html.contains("<script"));
        assert!(html.contains("no-referrer"));
    }
}
