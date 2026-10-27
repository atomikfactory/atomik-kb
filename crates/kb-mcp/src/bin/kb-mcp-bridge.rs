//! `kb-mcp-bridge`: connects an MCP client that speaks stdio (Claude Desktop) to the atomik-kb
//! app's loopback MCP server (ADR-0037, `docs/architecture/ai.md`, section 7, "Bridge").
//!
//! `kb-mcp-bridge --port <n>`, the token in the environment variable `ATOMIK_KB_MCP_TOKEN` (or
//! `--token <t>`). Reads newline-delimited JSON-RPC from stdin, POSTs each message to
//! `http://127.0.0.1:<port>/mcp` and writes each answer as one line to stdout. Standard library
//! only; nothing but answers is ever written to stdout.
//!
//! - A `200` body is written as one line; a `202` (a notification) writes nothing.
//! - A `401` becomes the JSON-RPC error -32001 "atomik-kb refused the token"; a connection that
//!   cannot be made becomes -32002 "atomik-kb is not running, is locked, or MCP is off". Any other
//!   failure (a timeout, an unexpected status) becomes -32603 with a short sentence. All three
//!   are written only for a message that had an `id` (a notification gets no answer, even a
//!   failed one).
//! - Logs go to stderr and never hold a token or any text of a message.

#![forbid(unsafe_code)]
// A command-line tool: stderr is where it talks to the person.
#![allow(clippy::print_stderr)]

use std::io::{self, BufRead, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

/// Environment variable that holds the token.
const TOKEN_ENV: &str = "ATOMIK_KB_MCP_TOKEN";
/// The port of the app's server when `--port` is left out (`McpSettings` default).
const DEFAULT_PORT: u16 = 47_811;
/// JSON-RPC error: the token was refused.
const ERROR_TOKEN_REFUSED: i64 = -32001;
/// JSON-RPC error: the app is not reachable.
const ERROR_UNREACHABLE: i64 = -32002;
/// JSON-RPC error: another failure of the bridge.
const ERROR_INTERNAL: i64 = -32603;
/// Message of [`ERROR_TOKEN_REFUSED`].
const MESSAGE_TOKEN_REFUSED: &str = "atomik-kb refused the token";
/// Message of [`ERROR_UNREACHABLE`].
const MESSAGE_UNREACHABLE: &str = "atomik-kb is not running, is locked, or MCP is off";
/// How long to wait for the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the app to answer (a search over a large workspace can take a while).
const IO_TIMEOUT: Duration = Duration::from_secs(120);
/// The most bytes of an answer that are read.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

const USAGE: &str = "usage: kb-mcp-bridge [--port <n>] [--token <token>]\n\
    The token is read from the environment variable ATOMIK_KB_MCP_TOKEN unless --token is given.\n\
    The port defaults to 47811. Reads JSON-RPC lines from stdin and writes the answers to stdout.";

// ----- arguments -------------------------------------------------------------------------------

/// What the bridge was told.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    port: u16,
    token: String,
}

/// Why the arguments did not make a [`Config`].
#[derive(Debug, PartialEq, Eq)]
enum ArgsError {
    /// `--help`.
    Help,
    /// Something is wrong; the sentence says what (never the token).
    Invalid(String),
}

/// Reads `--port <n>` (or `--port=<n>`) and `--token <t>` (or `--token=<t>`); the token falls
/// back to `env_token`.
fn parse_args(args: &[String], env_token: Option<String>) -> Result<Config, ArgsError> {
    let mut port: Option<u16> = None;
    let mut token: Option<String> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match name {
            "-h" | "--help" => return Err(ArgsError::Help),
            "--port" | "--token" => {
                let value = match inline {
                    Some(value) => value,
                    None => rest
                        .next()
                        .cloned()
                        .ok_or_else(|| ArgsError::Invalid(format!("{name} needs a value")))?,
                };
                if name == "--port" {
                    port = Some(
                        value
                            .parse::<u16>()
                            .ok()
                            .filter(|port| *port != 0)
                            .ok_or_else(|| {
                                ArgsError::Invalid("the port must be from 1 to 65535".to_owned())
                            })?,
                    );
                } else {
                    token = Some(value);
                }
            }
            _ => return Err(ArgsError::Invalid(format!("unknown argument {name}"))),
        }
    }
    let token = token.or(env_token).unwrap_or_default();
    if token.is_empty() {
        return Err(ArgsError::Invalid(format!(
            "no token: set {TOKEN_ENV} or pass --token"
        )));
    }
    // The token goes into a header: visible ASCII only, so it cannot inject another one.
    if !token.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(ArgsError::Invalid(
            "the token must be visible ASCII without spaces".to_owned(),
        ));
    }
    Ok(Config {
        port: port.unwrap_or(DEFAULT_PORT),
        token,
    })
}

// ----- the id of a message ---------------------------------------------------------------------

/// The raw text of the top-level `id` member of a JSON-RPC message when it is a string or a number
/// (what the reply to that message must echo), `None` for a notification or text that is not an
/// object. A small scanner: the bridge has no JSON library, and the one thing it needs from a
/// message is this member.
fn request_id(message: &str) -> Option<&str> {
    let bytes = message.as_bytes();
    let mut at = skip_whitespace(bytes, 0);
    if bytes.get(at) != Some(&b'{') {
        return None;
    }
    at += 1;
    loop {
        at = skip_whitespace(bytes, at);
        if bytes.get(at) != Some(&b'"') {
            return None; // `}` (no members left) or garbage
        }
        let key_end = string_end(bytes, at)?;
        let key = message.get(at + 1..key_end - 1)?;
        at = skip_whitespace(bytes, key_end);
        if bytes.get(at) != Some(&b':') {
            return None;
        }
        at = skip_whitespace(bytes, at + 1);
        let value_end = value_end(bytes, at)?;
        if key == "id" {
            let raw = message.get(at..value_end)?;
            let first = *raw.as_bytes().first()?;
            return match first {
                b'"' => Some(raw),
                b'-' | b'0'..=b'9' if is_json_number(raw) => Some(raw),
                _ => None,
            };
        }
        at = skip_whitespace(bytes, value_end);
        match bytes.get(at) {
            Some(b',') => at += 1,
            _ => return None,
        }
    }
}

/// Whether the whole text is a JSON number: `-?(0|[1-9][0-9]*)(.[0-9]+)?([eE][+-]?[0-9]+)?`.
fn is_json_number(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |from: usize| {
        bytes[from.min(bytes.len())..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut at = usize::from(bytes.first() == Some(&b'-'));
    match digits(at) {
        0 => return false,
        1 => at += 1,
        n => {
            if bytes[at] == b'0' {
                return false;
            }
            at += n;
        }
    }
    if bytes.get(at) == Some(&b'.') {
        let n = digits(at + 1);
        if n == 0 {
            return false;
        }
        at += 1 + n;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let n = digits(at);
        if n == 0 {
            return false;
        }
        at += n;
    }
    at == bytes.len()
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        at += 1;
    }
    at
}

/// The index just after the string that starts at `at` (an opening quote).
fn string_end(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    while let Some(byte) = bytes.get(i) {
        match byte {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// The index just after the value that starts at `at`.
fn value_end(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'"' => string_end(bytes, at),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut i = at;
            while let Some(byte) = bytes.get(i) {
                match byte {
                    b'"' => {
                        i = string_end(bytes, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            None
        }
        b',' | b'}' | b']' | b':' => None,
        _ => {
            let mut i = at;
            while let Some(byte) = bytes.get(i) {
                if matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
                    break;
                }
                i += 1;
            }
            Some(i)
        }
    }
}

// ----- the exchange with the app ---------------------------------------------------------------

/// What came of forwarding one message.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The app answered 200 with this body.
    Reply(String),
    /// The app answered 202, or 200 with nothing: no reply to write.
    NoReply,
    /// 401.
    TokenRefused,
    /// The connection could not be made.
    Unreachable,
    /// Anything else, as a short sentence (no content of any message).
    Failed(String),
}

/// A parsed HTTP answer.
#[derive(Debug, PartialEq, Eq)]
struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

/// The request for one message: HTTP/1.1, `Connection: close`, `Content-Length`.
fn build_request(port: u16, token: &str, body: &str) -> Vec<u8> {
    let mut request = format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body.as_bytes());
    request
}

/// Splits the whole answer into status and body: the status line, `Content-Length` or chunked
/// encoding, else the rest of the stream.
fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let head_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("the answer has no header end")?;
    let head = std::str::from_utf8(&raw[..head_end]).map_err(|_| "the answer is not text")?;
    let rest = &raw[head_end + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split_whitespace();
    if !parts
        .next()
        .is_some_and(|version| version.starts_with("HTTP/1."))
    {
        return Err("the answer is not HTTP/1.x".to_owned());
    }
    let status: u16 = parts
        .next()
        .and_then(|status| status.parse().ok())
        .ok_or("the answer has no status")?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().ok(),
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some(length) = length {
        rest.get(..length)
            .ok_or("the answer is shorter than its Content-Length")?
            .to_vec()
    } else {
        rest.to_vec()
    };
    Ok(HttpResponse { status, body })
}

/// Decodes a `Transfer-Encoding: chunked` body.
fn decode_chunked(mut raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let line_end = raw
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or("a chunk has no size line")?;
        let size_line =
            std::str::from_utf8(&raw[..line_end]).map_err(|_| "a chunk size is not text")?;
        let size_text = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| "a chunk size is invalid")?;
        raw = &raw[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        let chunk = raw.get(..size).ok_or("a chunk is cut short")?;
        body.extend_from_slice(chunk);
        raw = raw
            .get(size..)
            .and_then(|after| after.strip_prefix(b"\r\n"))
            .ok_or("a chunk is not followed by a line end")?;
    }
}

/// Sends one message to the app and reads the answer.
fn forward(config: &Config, message: &str) -> Outcome {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, config.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) else {
        return Outcome::Unreachable;
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let request = build_request(config.port, &config.token, message);
    if let Err(err) = stream.write_all(&request) {
        return Outcome::Failed(io_failure("send", &err));
    }
    let mut raw = Vec::new();
    if let Err(err) = (&mut stream).take(MAX_RESPONSE_BYTES).read_to_end(&mut raw) {
        // A reset after the whole answer arrived still leaves the answer in `raw`.
        if raw.is_empty() {
            return Outcome::Failed(io_failure("read", &err));
        }
    }
    let response = match parse_response(&raw) {
        Ok(response) => response,
        Err(why) => return Outcome::Failed(format!("atomik-kb sent an unreadable answer ({why})")),
    };
    match response.status {
        200 => match String::from_utf8(response.body) {
            Ok(body) if !body.trim().is_empty() => Outcome::Reply(body),
            Ok(_) => Outcome::NoReply,
            Err(_) => Outcome::Failed("atomik-kb sent an answer that is not text".to_owned()),
        },
        202 => Outcome::NoReply,
        401 => Outcome::TokenRefused,
        other => Outcome::Failed(format!("atomik-kb answered HTTP {other}")),
    }
}

/// A short sentence for an I/O failure after the connection was made.
fn io_failure(what: &str, err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            format!("atomik-kb did not answer in time ({what})")
        }
        _ => format!("the connection to atomik-kb failed ({what})"),
    }
}

// ----- the answers -----------------------------------------------------------------------------

/// A JSON-RPC error line for the message whose raw `id` is given.
fn error_line(id: &str, code: i64, message: &str) -> String {
    format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":{code},\"message\":\"{}\"}}}}",
        json_escape(message)
    )
}

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// What to write to stdout for an outcome: one line, or nothing.
fn answer(outcome: &Outcome, id: Option<&str>) -> Option<String> {
    match outcome {
        // Newlines can only be whitespace between tokens in JSON; a space keeps one line.
        Outcome::Reply(body) => Some(body.replace(['\r', '\n'], " ").trim().to_owned()),
        Outcome::NoReply => None,
        Outcome::TokenRefused => {
            id.map(|id| error_line(id, ERROR_TOKEN_REFUSED, MESSAGE_TOKEN_REFUSED))
        }
        Outcome::Unreachable => id.map(|id| error_line(id, ERROR_UNREACHABLE, MESSAGE_UNREACHABLE)),
        Outcome::Failed(why) => id.map(|id| error_line(id, ERROR_INTERNAL, why)),
    }
}

/// What the log says about a failure (never any content).
fn log_failure(outcome: &Outcome, port: u16) {
    match outcome {
        Outcome::Unreachable => eprintln!("kb-mcp-bridge: cannot connect to 127.0.0.1:{port}"),
        Outcome::TokenRefused => eprintln!("kb-mcp-bridge: the token was refused (HTTP 401)"),
        Outcome::Failed(why) => eprintln!("kb-mcp-bridge: {why}"),
        Outcome::Reply(_) | Outcome::NoReply => {}
    }
}

/// Reads lines from `input`, forwards each message and writes the answers to `output`, until the
/// input ends.
fn run<R: BufRead, W: Write>(config: &Config, mut input: R, output: &mut W) -> io::Result<()> {
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        if input.read_until(b'\n', &mut buffer)? == 0 {
            return Ok(());
        }
        let Ok(line) = std::str::from_utf8(&buffer) else {
            eprintln!("kb-mcp-bridge: skipped a line that is not UTF-8");
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let id = request_id(line);
        let outcome = forward(config, line);
        log_failure(&outcome, config.port);
        if let Some(text) = answer(&outcome, id) {
            output.write_all(text.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = match parse_args(&args, std::env::var(TOKEN_ENV).ok()) {
        Ok(config) => config,
        Err(ArgsError::Help) => {
            eprintln!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(ArgsError::Invalid(why)) => {
            eprintln!("kb-mcp-bridge: {why}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    eprintln!(
        "kb-mcp-bridge: forwarding to 127.0.0.1:{} (version {})",
        config.port,
        env!("CARGO_PKG_VERSION")
    );
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match run(&config, stdin.lock(), &mut output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::io::Cursor;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    use serde_json::{Value, json};

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    // ----- arguments

    #[test]
    fn the_port_and_the_token_come_from_the_arguments_and_the_environment() {
        let config = parse_args(&args(&["--port", "4000"]), Some("kbm_env".into())).unwrap();
        assert_eq!(
            config,
            Config {
                port: 4000,
                token: "kbm_env".into()
            }
        );
        let config = parse_args(
            &args(&["--port=4001", "--token=kbm_arg"]),
            Some("kbm_env".into()),
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                port: 4001,
                token: "kbm_arg".into()
            }
        );
        // --token wins over the environment; the port defaults.
        let config = parse_args(&args(&["--token", "kbm_arg"]), Some("kbm_env".into())).unwrap();
        assert_eq!(config.token, "kbm_arg");
        assert_eq!(config.port, 47_811);
        // A token with an equals sign in it survives.
        let config = parse_args(&args(&["--token=a=b"]), None).unwrap();
        assert_eq!(config.token, "a=b");
    }

    #[test]
    fn bad_arguments_are_refused_without_echoing_the_token() {
        let invalid = |list: &[&str], env: Option<&str>| match parse_args(
            &args(list),
            env.map(str::to_owned),
        )
        .unwrap_err()
        {
            ArgsError::Invalid(why) => why,
            ArgsError::Help => panic!("not help"),
        };
        assert!(invalid(&["--port", "4000"], None).contains("ATOMIK_KB_MCP_TOKEN"));
        assert!(invalid(&["--port", "4000"], Some("")).contains("no token"));
        for port in ["0", "65536", "abc", "-1", ""] {
            assert!(
                invalid(&["--port", port], Some("t")).contains("port"),
                "{port}"
            );
        }
        assert!(invalid(&["--port"], Some("t")).contains("needs a value"));
        assert!(invalid(&["--bogus"], Some("t")).contains("unknown argument"));
        assert!(invalid(&["extra"], Some("t")).contains("unknown argument"));
        for bad in [
            "with space",
            "tab\there",
            "line\nbreak",
            "caf\u{e9}",
            "a\r\nX: y",
        ] {
            let why = invalid(&[], Some(bad));
            assert!(why.contains("visible ASCII"), "{why}");
            assert!(!why.contains(bad), "the token is never echoed");
        }
        assert_eq!(parse_args(&args(&["--help"]), None), Err(ArgsError::Help));
        assert_eq!(parse_args(&args(&["-h"]), None), Err(ArgsError::Help));
    }

    // ----- the id of a message

    #[test]
    fn the_id_is_the_top_level_member_whatever_its_type() {
        for (message, id) in [
            (r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, Some("1")),
            (r#"{"jsonrpc":"2.0","method":"ping","id":42}"#, Some("42")),
            (r#"{"id":"abc-1","method":"x"}"#, Some(r#""abc-1""#)),
            (r#"{ "id" : -7 , "method":"x" }"#, Some("-7")),
            (r#"{"id":1.5e3}"#, Some("1.5e3")),
            ("{\n  \"id\"\t:\n 3\n}", Some("3")),
            // Escapes stay as they are: the reply repeats the token.
            (r#"{"id":"a\"b\\c\u00e9"}"#, Some(r#""a\"b\\c\u00e9""#)),
            // Nested members named id, strings that look like members: only the top level counts.
            (r#"{"params":{"id":5},"id":7}"#, Some("7")),
            (r#"{"params":{"id":5}}"#, None),
            (
                r#"{"a":[{"id":1},{"id":2}],"b":"\"id\":9","id":8}"#,
                Some("8"),
            ),
            (r#"{"text":"{\"id\":4}"}"#, None),
            // Notifications and things that are not requests.
            (
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                None,
            ),
            (r#"{"id":null}"#, None),
            (r#"{"id":true}"#, None),
            (r#"{"id":{}}"#, None),
            (r#"{"id":[1]}"#, None),
            (r#"[{"id":1}]"#, None),
            ("5", None),
            ("", None),
            ("{", None),
            (r#"{"id""#, None),
            (r#"{"id":"#, None),
            (r#"{"id":"unterminated"#, None),
            (r#"{"id":-}"#, None),
            (r#"{"id":1x}"#, None),
            (r#"{"id":01}"#, None),
            (r#"{"id":1.}"#, None),
            (r#"{"id":1e}"#, None),
            (r#"{"id":-0.5E-3}"#, Some("-0.5E-3")),
            (r#"{"id":0}"#, Some("0")),
        ] {
            assert_eq!(request_id(message), id, "{message:?}");
        }
    }

    // ----- the request and the response

    #[test]
    fn the_request_is_http_1_1_with_the_token_and_a_content_length_in_bytes() {
        let body = r#"{"id":1,"text":"é"}"#;
        let request = build_request(4000, "kbm_abc", body);
        let text = String::from_utf8(request).unwrap();
        let (head, sent) = text.split_once("\r\n\r\n").unwrap();
        assert_eq!(sent, body);
        let lines: Vec<&str> = head.split("\r\n").collect();
        assert_eq!(lines[0], "POST /mcp HTTP/1.1");
        assert!(lines.contains(&"Host: 127.0.0.1:4000"));
        assert!(lines.contains(&"Authorization: Bearer kbm_abc"));
        assert!(lines.contains(&"Content-Type: application/json"));
        assert!(lines.contains(&"Connection: close"));
        let length = format!("Content-Length: {}", body.len());
        assert!(lines.contains(&length.as_str()));
        assert!(
            body.len() > body.chars().count(),
            "the test body has a multi-byte character"
        );
    }

    #[test]
    fn a_response_is_split_by_content_length_by_chunks_or_by_the_end_of_the_stream() {
        let ok = parse_response(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 4\r\n\r\n{\"a\"}trailing")
            .unwrap();
        assert_eq!(ok.status, 200);
        assert_eq!(ok.body, b"{\"a\"");
        let accepted =
            parse_response(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!((accepted.status, accepted.body.len()), (202, 0));
        let chunked = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3;ext=1\r\n:1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(chunked.body, b"{\"a\":1}");
        let open_ended = parse_response(b"HTTP/1.0 200 OK\r\n\r\nall of it").unwrap();
        assert_eq!(open_ended.body, b"all of it");
        let unauthorized = parse_response(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(unauthorized.status, 401);
    }

    #[test]
    fn a_damaged_response_is_an_error_not_a_panic() {
        for raw in [
            &b""[..],
            b"HTTP/1.1 200 OK",
            b"HTTP/1.1 200 OK\r\n\r",
            b"garbage\r\n\r\n",
            b"HTTP/2 200\r\n\r\n",
            b"HTTP/1.1 abc OK\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nxx",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nabXX0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            b"\xff\xfe\r\n\r\n",
        ] {
            assert!(parse_response(raw).is_err(), "{raw:?}");
        }
    }

    // ----- the answers

    #[test]
    fn errors_are_valid_json_rpc_with_the_id_repeated_verbatim() {
        for id in ["1", "-7", r#""abc""#, r#""a\"b""#, "1.5e3"] {
            for (outcome, code, message) in [
                (Outcome::TokenRefused, -32001, "atomik-kb refused the token"),
                (
                    Outcome::Unreachable,
                    -32002,
                    "atomik-kb is not running, is locked, or MCP is off",
                ),
                (
                    Outcome::Failed("atomik-kb answered HTTP 500".into()),
                    -32603,
                    "atomik-kb answered HTTP 500",
                ),
            ] {
                let line = answer(&outcome, Some(id)).unwrap();
                assert!(!line.contains('\n'));
                let parsed: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(parsed["jsonrpc"], "2.0");
                assert_eq!(parsed["id"], serde_json::from_str::<Value>(id).unwrap());
                assert_eq!(parsed["error"]["code"], code);
                assert_eq!(parsed["error"]["message"], message);
                // Nothing for a message without an id.
                assert_eq!(answer(&outcome, None), None);
            }
        }
    }

    #[test]
    fn a_reply_is_one_trimmed_line_and_a_missing_reply_is_nothing() {
        assert_eq!(
            answer(&Outcome::Reply("{\"a\":1}\n".into()), None).unwrap(),
            "{\"a\":1}"
        );
        assert_eq!(
            answer(
                &Outcome::Reply("{\r\n  \"a\": 1\r\n}\r\n".into()),
                Some("1")
            )
            .unwrap(),
            "{    \"a\": 1  }"
        );
        assert_eq!(answer(&Outcome::NoReply, Some("1")), None);
    }

    #[test]
    fn the_error_codes_are_the_ones_of_the_library() {
        assert_eq!(ERROR_TOKEN_REFUSED, kb_mcp::ERROR_TOKEN_REFUSED);
        assert_eq!(ERROR_UNREACHABLE, kb_mcp::ERROR_UNREACHABLE);
    }

    // ----- against a stub server

    /// Serves one canned answer per connection and returns the requests it received.
    fn serve(answers: Vec<Vec<u8>>) -> (u16, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut received = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head_end, length) = loop {
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0, "the client closed before the request was complete");
                    received.extend_from_slice(&chunk[..n]);
                    if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while received.len() < head_end + length {
                    let n = stream.read(&mut chunk).unwrap();
                    received.extend_from_slice(&chunk[..n]);
                }
                requests.push(String::from_utf8(received).unwrap());
                stream.write_all(&answer).unwrap();
                // Dropping the stream closes it: `Connection: close`.
            }
            requests
        });
        (port, handle)
    }

    fn http(status: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn config(port: u16) -> Config {
        Config {
            port,
            token: "kbm_secret".into(),
        }
    }

    #[test]
    fn forward_posts_the_message_verbatim_and_reads_each_kind_of_answer() {
        let message = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let (port, server) = serve(vec![
            http("200 OK", r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
            http("202 Accepted", ""),
            http("401 Unauthorized", ""),
            http("403 Forbidden", "no"),
            http("200 OK", ""),
            b"nonsense".to_vec(),
        ]);
        let config = config(port);
        assert_eq!(
            forward(&config, message),
            Outcome::Reply(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.into())
        );
        assert_eq!(forward(&config, message), Outcome::NoReply);
        assert_eq!(forward(&config, message), Outcome::TokenRefused);
        assert_eq!(
            forward(&config, message),
            Outcome::Failed("atomik-kb answered HTTP 403".into())
        );
        assert_eq!(
            forward(&config, message),
            Outcome::NoReply,
            "200 without a body"
        );
        assert!(
            matches!(forward(&config, message), Outcome::Failed(why) if why.contains("unreadable"))
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 6);
        for request in &requests {
            let (head, body) = request.split_once("\r\n\r\n").unwrap();
            assert_eq!(body, message);
            assert!(head.starts_with("POST /mcp HTTP/1.1\r\n"));
            assert!(head.contains(&format!("Host: 127.0.0.1:{port}\r\n")));
            assert!(head.contains("Authorization: Bearer kbm_secret\r\n"));
            assert!(head.contains("Connection: close"));
        }
    }

    #[test]
    fn forward_reports_a_refused_connection_as_unreachable() {
        // Bind a port, then free it: nothing listens there.
        let port = {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        assert_eq!(forward(&config(port), "{}"), Outcome::Unreachable);
    }

    #[test]
    fn forward_reads_a_chunked_answer() {
        let body = r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true}}"#;
        let (a, b) = body.split_at(10);
        let answer = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
            a.len(),
            b.len()
        );
        let (port, server) = serve(vec![answer.into_bytes()]);
        assert_eq!(
            forward(&config(port), r#"{"id":2}"#),
            Outcome::Reply(body.into())
        );
        server.join().unwrap();
    }

    #[test]
    fn run_writes_one_line_per_answered_message_and_nothing_else() {
        let reply = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let (port, server) = serve(vec![
            http("200 OK", reply),
            http("202 Accepted", ""),
            http("200 OK", &format!("{reply}\n")),
        ]);
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "",
            "   ",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":"s","method":"ping"}"#,
        ]
        .join("\r\n");
        let mut output = Vec::new();
        run(&config(port), Cursor::new(input.into_bytes()), &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!("{reply}\n{reply}\n")
        );
        assert_eq!(server.join().unwrap().len(), 3, "blank lines are not sent");
    }

    #[test]
    fn run_answers_failures_only_for_messages_with_an_id() {
        let port = {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/list"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":"x-1","method":"ping"}"#, // no trailing newline
        );
        let mut output = Vec::new();
        run(
            &config(port),
            Cursor::new(input.as_bytes().to_vec()),
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert_eq!(lines[0]["id"], 5);
        assert_eq!(lines[1]["id"], "x-1");
        for line in &lines {
            assert_eq!(line["error"]["code"], -32002);
        }
        assert_eq!(
            lines[0],
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "error": { "code": -32002, "message": "atomik-kb is not running, is locked, or MCP is off" }
            })
        );
    }

    #[test]
    fn run_maps_a_401_to_the_token_error_and_skips_lines_that_are_not_utf8() {
        let (port, server) = serve(vec![http("401 Unauthorized", "")]);
        let mut input = b"\xff\xfe broken\n".to_vec();
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#);
        input.push(b'\n');
        let mut output = Vec::new();
        run(&config(port), Cursor::new(input), &mut output).unwrap();
        let parsed: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(parsed["id"], 9);
        assert_eq!(parsed["error"]["code"], -32001);
        assert_eq!(parsed["error"]["message"], "atomik-kb refused the token");
        assert_eq!(output.iter().filter(|b| **b == b'\n').count(), 1);
        server.join().unwrap();
    }

    #[test]
    fn json_escape_covers_quotes_backslashes_and_controls() {
        assert_eq!(json_escape("a\"b\\c\nd\u{1}"), "a\\\"b\\\\c\\u000ad\\u0001");
        assert_eq!(json_escape("plain é"), "plain é");
    }
}
