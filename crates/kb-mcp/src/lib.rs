//! The MCP server protocol (ADR-0037, `docs/architecture/ai.md`, section 7).
//!
//! [`handle`] takes one JSON-RPC 2.0 message from a client and answers it: the MCP lifecycle
//! (`initialize`, notifications, `ping`), `tools/list` and `tools/call`. Tool calls are parsed
//! into a [`ToolCall`] and executed by an [`McpBackend`] (the core, which checks the client's
//! grants and audits the call). Pure: no I/O, no state; compiles for `wasm32-unknown-unknown`.
//! The transport (loopback HTTP) is the desktop shell's; `kb-mcp-bridge` (in `src/bin`) connects
//! stdio clients to it.
//!
//! Errors follow JSON-RPC: unparsable JSON is -32700, a batch or a message that is not a request
//! is -32600, an unknown method -32601, bad parameters (including an unknown tool and a missing or
//! invalid argument) -32602. A tool that ran and failed (no grant, unknown page) is not an error
//! of the protocol: it is a result with `isError: true`. Messages without an `id` are
//! notifications and get no answer.

#![forbid(unsafe_code)]

use serde_json::{Map, Value, json};

pub mod tools;

/// `serverInfo.name`.
pub const SERVER_NAME: &str = "atomik-kb";

/// `serverInfo.version`: the version of the workspace.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Protocol versions spoken, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC error: the client's token was refused (used by the bridge).
pub const ERROR_TOKEN_REFUSED: i64 = -32001;
/// JSON-RPC error: the app is not reachable (used by the bridge).
pub const ERROR_UNREACHABLE: i64 = -32002;

/// JSON-RPC error: the text is not JSON.
pub const ERROR_PARSE: i64 = -32700;
/// JSON-RPC error: not a request object (or a batch).
pub const ERROR_INVALID_REQUEST: i64 = -32600;
/// JSON-RPC error: no such method.
pub const ERROR_METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC error: bad parameters.
pub const ERROR_INVALID_PARAMS: i64 = -32602;

/// What the server tells a client about itself and the tools (`initialize` result).
pub const INSTRUCTIONS: &str = "atomik-kb is a private notes app on the person's own computer. \
These tools read and write their pages. Pages are Markdown. Call list_spaces first: it shows the \
spaces you may use and whether you may only read them or also write. Then use search, list_pages \
and read_page to read, and create_page and append_to_page to write (only in spaces with write \
access). The ids in the results (spaceId, pageId) are what the other tools take. Everything you \
do is recorded in the person's audit log.";

/// Longest tool name passed to [`McpBackend::invalid_call`], in characters.
const MAX_TOOL_NAME_CHARS: usize = 64;

/// A parsed tool call. Ids are the strings the client sent (the backend parses them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCall {
    /// `list_spaces`.
    ListSpaces,
    /// `search`.
    Search {
        /// 1 to 200 characters.
        query: String,
        /// Restrict to a space.
        space_id: Option<String>,
        /// 1 to 20 (default 10).
        limit: u32,
    },
    /// `list_pages`.
    ListPages {
        /// Space.
        space_id: String,
        /// Parent page; `None` for the top level.
        parent_id: Option<String>,
    },
    /// `read_page`.
    ReadPage {
        /// Page.
        page_id: String,
    },
    /// `create_page`.
    CreatePage {
        /// Space.
        space_id: String,
        /// Parent page; `None` for the top level.
        parent_id: Option<String>,
        /// 1 to 200 characters.
        title: String,
        /// Content (at most 256 KiB).
        markdown: Option<String>,
    },
    /// `append_to_page`.
    AppendToPage {
        /// Page.
        page_id: String,
        /// Content (1 byte to 256 KiB).
        markdown: String,
    },
}

impl ToolCall {
    /// The name of the tool, as in `tools/list`.
    #[must_use]
    pub const fn tool_name(&self) -> &'static str {
        match self {
            ToolCall::ListSpaces => tools::LIST_SPACES,
            ToolCall::Search { .. } => tools::SEARCH,
            ToolCall::ListPages { .. } => tools::LIST_PAGES,
            ToolCall::ReadPage { .. } => tools::READ_PAGE,
            ToolCall::CreatePage { .. } => tools::CREATE_PAGE,
            ToolCall::AppendToPage { .. } => tools::APPEND_TO_PAGE,
        }
    }
}

/// The outcome of a tool call, sent as `{ content: [{ type: "text", text }], isError }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// Text for the model.
    pub text: String,
    /// A tool-level failure (no grant, unknown page, ...).
    pub is_error: bool,
}

impl ToolResult {
    /// A successful result.
    #[must_use]
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    /// A failed result: the model reads the text and may try again.
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// What executes tool calls (the core: grants, audit, the work itself).
pub trait McpBackend {
    /// Executes a parsed call.
    fn call_tool(&mut self, call: ToolCall) -> ToolResult;
    /// A `tools/call` whose name or arguments were refused before reaching [`Self::call_tool`]
    /// (audited as `invalid`). `tool` is the name the client sent (at most 64 characters kept).
    fn invalid_call(&mut self, tool: &str, reason: &str);
}

/// Answers one JSON-RPC message; `None` for a notification (or a response the client sent).
/// A batch (JSON array) is refused with -32600, unparsable JSON is -32700.
#[must_use]
pub fn handle(message: &str, backend: &mut dyn McpBackend) -> Option<String> {
    let value: Value = match serde_json::from_str(message) {
        Ok(value) => value,
        Err(_) => return Some(error_reply(&Value::Null, ERROR_PARSE, "Parse error")),
    };
    let object = match value {
        Value::Object(object) => object,
        Value::Array(_) => {
            return Some(error_reply(
                &Value::Null,
                ERROR_INVALID_REQUEST,
                "Batch requests are not supported",
            ));
        }
        _ => {
            return Some(error_reply(
                &Value::Null,
                ERROR_INVALID_REQUEST,
                "Invalid Request",
            ));
        }
    };

    // A response from the client (to a request the server never makes) is ignored.
    if !object.contains_key("method")
        && (object.contains_key("result") || object.contains_key("error"))
    {
        return None;
    }

    let id = match object.get("id") {
        None => None,
        Some(id @ (Value::String(_) | Value::Number(_))) => Some(id),
        Some(_) => {
            return Some(error_reply(
                &Value::Null,
                ERROR_INVALID_REQUEST,
                "Invalid Request: id must be a string or a number",
            ));
        }
    };
    let echo = id.cloned().unwrap_or(Value::Null);
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_reply(
            &echo,
            ERROR_INVALID_REQUEST,
            "Invalid Request: jsonrpc must be \"2.0\"",
        ));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Some(error_reply(
            &echo,
            ERROR_INVALID_REQUEST,
            "Invalid Request: method must be a string",
        ));
    };

    let outcome = dispatch(method, object.get("params"), backend);
    // No id: a notification, whatever happened.
    id?;
    Some(match outcome {
        Ok(result) => result_reply(&echo, result),
        Err(error) => error_reply(&echo, error.code, &error.message),
    })
}

/// A failed method call, as JSON-RPC reports it.
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn params(message: impl Into<String>) -> Self {
        Self {
            code: ERROR_INVALID_PARAMS,
            message: message.into(),
        }
    }
}

fn dispatch(
    method: &str,
    params: Option<&Value>,
    backend: &mut dyn McpBackend,
) -> Result<Value, RpcError> {
    match method {
        "initialize" => initialize(params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::catalog() })),
        "tools/call" => tools_call(params, backend),
        _ => Err(RpcError {
            code: ERROR_METHOD_NOT_FOUND,
            message: "Method not found".to_owned(),
        }),
    }
}

/// `initialize`: the client's protocol version when it is one of ours, else the newest.
fn initialize(params: Option<&Value>) -> Result<Value, RpcError> {
    let requested = match params {
        None | Some(Value::Null) => None,
        Some(Value::Object(params)) => match params.get("protocolVersion") {
            None | Some(Value::Null) => None,
            Some(Value::String(version)) => Some(version.as_str()),
            Some(_) => return Err(RpcError::params("protocolVersion must be a string")),
        },
        Some(_) => return Err(RpcError::params("params must be an object")),
    };
    let version = requested
        .and_then(|requested| PROTOCOL_VERSIONS.iter().copied().find(|v| *v == requested))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    Ok(json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "instructions": INSTRUCTIONS,
    }))
}

/// `tools/call`: parses the call, runs it on the backend and wraps the result.
fn tools_call(params: Option<&Value>, backend: &mut dyn McpBackend) -> Result<Value, RpcError> {
    let Some(Value::Object(params)) = params else {
        backend.invalid_call("", "params must be an object");
        return Err(RpcError::params("params must be an object"));
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        backend.invalid_call("", "the tool name is missing");
        return Err(RpcError::params("name must be the name of a tool"));
    };
    let arguments = params.get("arguments").unwrap_or(&Value::Null);
    match tools::parse_call(name, arguments) {
        Ok(call) => {
            let result = backend.call_tool(call);
            Ok(json!({
                "content": [{ "type": "text", "text": result.text }],
                "isError": result.is_error,
            }))
        }
        Err(reason) => {
            let kept: String = name.chars().take(MAX_TOOL_NAME_CHARS).collect();
            backend.invalid_call(&kept, &reason);
            Err(RpcError::params(reason))
        }
    }
}

/// A success reply (one line).
fn result_reply(id: &Value, result: Value) -> String {
    let mut reply = Map::new();
    reply.insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
    reply.insert("id".to_owned(), id.clone());
    reply.insert("result".to_owned(), result);
    Value::Object(reply).to_string()
}

/// An error reply (one line).
fn error_reply(id: &Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

#[cfg(test)]
mod tests;
