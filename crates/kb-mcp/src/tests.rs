//! Protocol tests: JSON-RPC framing, the MCP lifecycle, `tools/list` and `tools/call` over a
//! recording backend.

#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};

use super::*;

/// Records what reached it and answers canned results.
#[derive(Default)]
struct Recorder {
    calls: Vec<ToolCall>,
    invalid: Vec<(String, String)>,
    answer: Option<ToolResult>,
}

impl McpBackend for Recorder {
    fn call_tool(&mut self, call: ToolCall) -> ToolResult {
        self.calls.push(call);
        self.answer
            .clone()
            .unwrap_or_else(|| ToolResult::ok("done"))
    }

    fn invalid_call(&mut self, tool: &str, reason: &str) {
        self.invalid.push((tool.to_owned(), reason.to_owned()));
    }
}

fn ask(message: &Value, backend: &mut Recorder) -> Option<Value> {
    handle(&message.to_string(), backend).map(|reply| {
        assert!(
            !reply.contains('\n') && !reply.contains('\r'),
            "a reply is one line"
        );
        serde_json::from_str(&reply).unwrap()
    })
}

fn request(id: Value, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn error_code(reply: &Value) -> i64 {
    reply["error"]["code"].as_i64().unwrap()
}

#[test]
fn initialize_answers_with_the_server_the_capabilities_and_instructions() {
    let mut backend = Recorder::default();
    let reply = ask(
        &request(
            json!(1),
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["id"], 1);
    let result = &reply["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["name"], "atomik-kb");
    assert_eq!(result["serverInfo"]["version"], SERVER_VERSION);
    assert_eq!(
        result["capabilities"],
        json!({ "tools": { "listChanged": false } })
    );
    let instructions = result["instructions"].as_str().unwrap();
    assert!(instructions.contains("Markdown"));
    assert!(instructions.contains("list_spaces"));
    assert!(backend.calls.is_empty() && backend.invalid.is_empty());
}

#[test]
fn the_protocol_version_is_the_clients_when_supported_else_the_newest() {
    for (asked, answered) in [
        (json!("2025-06-18"), "2025-06-18"),
        (json!("2025-03-26"), "2025-03-26"),
        (json!("2024-11-05"), "2024-11-05"),
        (json!("2099-01-01"), "2025-06-18"),
        (json!("1999"), "2025-06-18"),
        (json!(""), "2025-06-18"),
        (Value::Null, "2025-06-18"),
    ] {
        let mut backend = Recorder::default();
        let reply = ask(
            &request(
                json!("i"),
                "initialize",
                json!({ "protocolVersion": asked }),
            ),
            &mut backend,
        )
        .unwrap();
        assert_eq!(
            reply["result"]["protocolVersion"], answered,
            "asked {asked}"
        );
    }
    // Parameters left out altogether are tolerated; the wrong types are not.
    let mut backend = Recorder::default();
    let bare = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
    assert_eq!(
        ask(&bare, &mut backend).unwrap()["result"]["protocolVersion"],
        "2025-06-18"
    );
    let reply = ask(
        &request(
            json!(1),
            "initialize",
            json!({ "protocolVersion": 20250618 }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    let reply = ask(&request(json!(1), "initialize", json!([1])), &mut backend).unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    assert_eq!(
        PROTOCOL_VERSIONS,
        ["2025-06-18", "2025-03-26", "2024-11-05"]
    );
}

#[test]
fn notifications_get_no_answer() {
    let mut backend = Recorder::default();
    for message in [
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 3 } }),
        // Any message without an id is a notification, even one the server does not know or
        // whose parameters are wrong.
        json!({ "jsonrpc": "2.0", "method": "whatever/else" }),
        json!({ "jsonrpc": "2.0", "method": "tools/call", "params": 5 }),
        json!({ "jsonrpc": "2.0", "method": "ping" }),
    ] {
        assert_eq!(
            handle(&message.to_string(), &mut backend),
            None,
            "{message}"
        );
    }
    assert!(backend.calls.is_empty());
}

#[test]
fn responses_from_the_client_are_ignored() {
    let mut backend = Recorder::default();
    for message in [
        json!({ "jsonrpc": "2.0", "id": 9, "result": {} }),
        json!({ "jsonrpc": "2.0", "id": 9, "error": { "code": -1, "message": "no" } }),
    ] {
        assert_eq!(handle(&message.to_string(), &mut backend), None);
    }
}

#[test]
fn ping_answers_an_empty_object_and_echoes_number_and_string_ids() {
    let mut backend = Recorder::default();
    let reply = ask(&request(json!(42), "ping", Value::Null), &mut backend).unwrap();
    assert_eq!(reply, json!({ "jsonrpc": "2.0", "id": 42, "result": {} }));
    let reply = ask(&request(json!("abc-1"), "ping", json!({})), &mut backend).unwrap();
    assert_eq!(
        reply,
        json!({ "jsonrpc": "2.0", "id": "abc-1", "result": {} })
    );
    let reply = ask(&request(json!(0), "ping", json!({})), &mut backend).unwrap();
    assert_eq!(reply["id"], 0);
    let reply = ask(&request(json!(-7), "ping", json!({})), &mut backend).unwrap();
    assert_eq!(reply["id"], -7);
    let reply = ask(&request(json!(1.5), "ping", json!({})), &mut backend).unwrap();
    assert_eq!(reply["id"], 1.5);
}

#[test]
fn an_unknown_method_is_32601_with_the_id() {
    let mut backend = Recorder::default();
    for method in [
        "resources/list",
        "prompts/list",
        "tools/nope",
        "",
        "Initialize",
    ] {
        let reply = ask(&request(json!(5), method, json!({})), &mut backend).unwrap();
        assert_eq!(error_code(&reply), ERROR_METHOD_NOT_FOUND, "{method}");
        assert_eq!(reply["id"], 5);
        assert!(reply.get("result").is_none());
    }
}

#[test]
fn unparsable_json_is_32700_with_a_null_id() {
    let mut backend = Recorder::default();
    for text in ["", "{", "not json", "{\"jsonrpc\":\"2.0\",", "\u{0}"] {
        let reply: Value = serde_json::from_str(&handle(text, &mut backend).unwrap()).unwrap();
        assert_eq!(error_code(&reply), ERROR_PARSE, "{text:?}");
        assert_eq!(reply["id"], Value::Null);
        assert_eq!(reply["jsonrpc"], "2.0");
    }
}

#[test]
fn batches_and_scalars_are_32600() {
    let mut backend = Recorder::default();
    let batch = json!([
        request(json!(1), "ping", json!({})),
        request(json!(2), "ping", json!({}))
    ]);
    let reply = ask(&batch, &mut backend).unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    assert_eq!(reply["id"], Value::Null);
    let reply = ask(&json!([]), &mut backend).unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    for scalar in [json!(1), json!("x"), json!(true), Value::Null] {
        let reply = ask(&scalar, &mut backend).unwrap();
        assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    }
    assert!(backend.calls.is_empty());
}

#[test]
fn a_malformed_request_object_is_32600() {
    let mut backend = Recorder::default();
    // The id is echoed when it is valid, and null otherwise.
    let reply = ask(
        &json!({ "jsonrpc": "1.0", "id": 3, "method": "ping" }),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    assert_eq!(reply["id"], 3);
    let reply = ask(&json!({ "id": 3, "method": "ping" }), &mut backend).unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    let reply = ask(
        &json!({ "jsonrpc": "2.0", "id": 3, "method": 7 }),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    assert_eq!(reply["id"], 3);
    let reply = ask(&json!({ "jsonrpc": "2.0", "id": 3 }), &mut backend).unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
    for id in [Value::Null, json!(true), json!({}), json!([1])] {
        let reply = ask(
            &json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }),
            &mut backend,
        )
        .unwrap();
        assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST, "{id}");
        assert_eq!(reply["id"], Value::Null);
    }
    let reply = ask(&json!({}), &mut backend).unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_REQUEST);
}

#[test]
fn tools_list_lists_every_tool() {
    let mut backend = Recorder::default();
    let reply = ask(&request(json!(2), "tools/list", json!({})), &mut backend).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);
    assert_eq!(tools[1]["name"], "search");
    assert_eq!(tools[1]["inputSchema"]["required"], json!(["query"]));
    assert_eq!(reply["result"].get("nextCursor"), None);
    // A cursor is accepted and ignored.
    let reply = ask(
        &request(json!(3), "tools/list", json!({ "cursor": "x" })),
        &mut backend,
    )
    .unwrap();
    assert_eq!(reply["result"]["tools"].as_array().unwrap().len(), 6);
}

#[test]
fn tools_call_parses_the_arguments_runs_the_backend_and_wraps_the_text() {
    let mut backend = Recorder {
        answer: Some(ToolResult::ok("Found 1 page\nline two")),
        ..Recorder::default()
    };
    let reply = ask(
        &request(
            json!(7),
            "tools/call",
            json!({ "name": "search", "arguments": { "query": "budget", "limit": 3 } }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(reply["id"], 7);
    assert_eq!(
        reply["result"],
        json!({
            "content": [{ "type": "text", "text": "Found 1 page\nline two" }],
            "isError": false
        })
    );
    assert_eq!(
        backend.calls,
        [ToolCall::Search {
            query: "budget".into(),
            space_id: None,
            limit: 3
        }]
    );
    assert!(backend.invalid.is_empty());
}

#[test]
fn a_tool_that_fails_is_a_result_with_is_error_not_a_protocol_error() {
    let mut backend = Recorder {
        answer: Some(ToolResult::error(
            "This client has no read access to that space",
        )),
        ..Recorder::default()
    };
    let reply = ask(
        &request(
            json!("c"),
            "tools/call",
            json!({ "name": "read_page", "arguments": { "pageId": "p" } }),
        ),
        &mut backend,
    )
    .unwrap();
    assert!(reply.get("error").is_none());
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(
        reply["result"]["content"][0]["text"],
        "This client has no read access to that space"
    );
}

#[test]
fn a_call_without_arguments_still_parses_for_list_spaces() {
    let mut backend = Recorder::default();
    let reply = ask(
        &request(json!(1), "tools/call", json!({ "name": "list_spaces" })),
        &mut backend,
    )
    .unwrap();
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(backend.calls, [ToolCall::ListSpaces]);
}

#[test]
fn a_refused_call_is_32602_and_reaches_the_backend_as_invalid() {
    let mut backend = Recorder::default();
    // An unknown tool.
    let reply = ask(
        &request(
            json!(1),
            "tools/call",
            json!({ "name": "rm_rf", "arguments": {} }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    assert_eq!(reply["id"], 1);
    assert_eq!(reply["error"]["message"], "unknown tool rm_rf");
    // A missing argument.
    let reply = ask(
        &request(
            json!(2),
            "tools/call",
            json!({ "name": "search", "arguments": {} }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("query")
    );
    // Arguments that are not an object.
    let reply = ask(
        &request(
            json!(3),
            "tools/call",
            json!({ "name": "search", "arguments": "x" }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    // No name, a name that is not a string, params that are not an object.
    for params in [json!({}), json!({ "name": 4 }), json!([1]), Value::Null] {
        let reply = ask(&request(json!(4), "tools/call", params), &mut backend).unwrap();
        assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    }
    assert!(backend.calls.is_empty());
    let tools: Vec<&str> = backend.invalid.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(tools, ["rm_rf", "search", "search", "", "", "", ""]);
    assert!(backend.invalid[1].1.contains("query"));
}

#[test]
fn the_tool_name_given_to_the_backend_is_cut_to_64_characters() {
    let mut backend = Recorder::default();
    let long = "é".repeat(300);
    let reply = ask(
        &request(
            json!(1),
            "tools/call",
            json!({ "name": long, "arguments": {} }),
        ),
        &mut backend,
    )
    .unwrap();
    assert_eq!(error_code(&reply), ERROR_INVALID_PARAMS);
    assert_eq!(backend.invalid[0].0.chars().count(), 64);
    // The reply echoes no more of the name than that either.
    assert!(reply["error"]["message"].as_str().unwrap().chars().count() < 100);
}

#[test]
fn a_refused_call_without_an_id_is_not_answered_but_is_still_reported() {
    let mut backend = Recorder::default();
    let message = json!({ "jsonrpc": "2.0", "method": "tools/call", "params": { "name": "nope" } });
    assert_eq!(handle(&message.to_string(), &mut backend), None);
    assert_eq!(backend.invalid.len(), 1);
}

#[test]
fn text_with_special_characters_stays_one_line_and_round_trips() {
    let text = "a \"quoted\" line\nnext\tline \u{2028} \\ é 日本 \u{1F600}";
    let mut backend = Recorder {
        answer: Some(ToolResult::ok(text)),
        ..Recorder::default()
    };
    let reply = handle(
        &request(json!(1), "tools/call", json!({ "name": "list_spaces" })).to_string(),
        &mut backend,
    )
    .unwrap();
    assert!(!reply.contains('\n'));
    let parsed: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(parsed["result"]["content"][0]["text"], text);
}

#[test]
fn error_codes_for_the_bridge_are_the_documented_ones() {
    assert_eq!(ERROR_TOKEN_REFUSED, -32001);
    assert_eq!(ERROR_UNREACHABLE, -32002);
    assert_eq!(ERROR_PARSE, -32700);
    assert_eq!(ERROR_INVALID_REQUEST, -32600);
    assert_eq!(ERROR_METHOD_NOT_FOUND, -32601);
    assert_eq!(ERROR_INVALID_PARAMS, -32602);
}
