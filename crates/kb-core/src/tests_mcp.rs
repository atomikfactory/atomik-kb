//! The MCP server through `Request` (ADR-0037, `docs/architecture/ai.md` section 7): clients and
//! their tokens, grants, the audit log, and `McpCall` with the six tools against a real core.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::McpQueries;
use kb_types::ai::{
    McpAuditEntry, McpClientCreated, McpClientInfo, McpGrant, McpOutcome, McpReply,
};
use kb_types::{
    ApiError, ChangeSource, DateValue, ErrorCode, Event, OptionColor, PageId, PropertyKind,
    PropertyValue, Request, Response, SpaceId,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Core;
use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, outline, page, page_content, spaces,
    tree, write_text,
};

const T0: i64 = 10_000;

struct Env {
    fixture: Fixture,
    core: Core,
    work: SpaceId,
    personal: SpaceId,
}

/// A core with the spaces "Work" and "Personal" (in that order).
fn setup() -> Env {
    let fixture = Fixture::new();
    let mut core = fixture.open(1, T0);
    create_workspace(&mut core, "Home", Some("Work"));
    let work = spaces(&mut core)[0].id;
    let personal = create_space(&mut core, "Personal").id;
    core.drain_events();
    Env {
        fixture,
        core,
        work,
        personal,
    }
}

// ----- helpers: clients ---------------------------------------------------------------------------

fn new_client(core: &mut Core, name: &str) -> McpClientCreated {
    match core
        .dispatch(Request::McpClientCreate { name: name.into() })
        .unwrap()
    {
        Response::McpClientCreated(created) => created,
        other => panic!("unexpected {other:?}"),
    }
}

fn grant(space_id: SpaceId, read: bool, write: bool) -> McpGrant {
    McpGrant {
        space_id,
        read,
        write,
    }
}

fn set_grants(core: &mut Core, client_id: &str, grants: Vec<McpGrant>) -> McpClientInfo {
    match core
        .dispatch(Request::McpClientGrantsSet {
            client_id: client_id.into(),
            grants,
        })
        .unwrap()
    {
        Response::McpClient(info) => info,
        other => panic!("unexpected {other:?}"),
    }
}

/// A client with the given grants; answers it and its token.
fn client_with(core: &mut Core, name: &str, grants: Vec<McpGrant>) -> (McpClientInfo, String) {
    let created = new_client(core, name);
    let info = set_grants(core, &created.client.id, grants);
    (info, created.token)
}

fn clients(core: &mut Core) -> Vec<McpClientInfo> {
    match core.dispatch(Request::McpClientList).unwrap() {
        Response::McpClients(list) => list,
        other => panic!("unexpected {other:?}"),
    }
}

fn sha(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn has_event(events: &[Event], wanted: &Event) -> bool {
    events.iter().any(|event| event == wanted)
}

// ----- helpers: calls -----------------------------------------------------------------------------

fn try_mcp(core: &mut Core, token: &str, message: &str) -> Result<McpReply, ApiError> {
    match core.dispatch(Request::McpCall {
        token: token.into(),
        message: message.into(),
    })? {
        Response::McpReply(reply) => Ok(reply),
        other => panic!("unexpected {other:?}"),
    }
}

/// Sends a JSON-RPC message and parses the reply (`None` for a notification).
fn mcp(core: &mut Core, token: &str, message: &Value) -> Option<Value> {
    let reply = try_mcp(core, token, &message.to_string()).unwrap();
    reply.body.map(|body| {
        assert!(!body.contains('\n'), "a reply is one line");
        serde_json::from_str(&body).unwrap()
    })
}

fn tool_message(name: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

/// Calls a tool; answers its text and `isError`.
fn run(core: &mut Core, token: &str, name: &str, arguments: &Value) -> (String, bool) {
    let reply = mcp(core, token, &tool_message(name, arguments)).unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    (
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned(),
        reply["result"]["isError"].as_bool().unwrap(),
    )
}

/// A tool call that works.
fn ok(core: &mut Core, token: &str, name: &str, arguments: &Value) -> String {
    let (text, is_error) = run(core, token, name, arguments);
    assert!(!is_error, "{name} failed: {text}");
    text
}

/// A tool call that fails (as a result with `isError`).
fn fail(core: &mut Core, token: &str, name: &str, arguments: &Value) -> String {
    let (text, is_error) = run(core, token, name, arguments);
    assert!(is_error, "{name} should have failed, said: {text}");
    text
}

fn audit_rows(core: &mut Core, client_id: Option<&str>) -> Vec<McpAuditEntry> {
    match core
        .dispatch(Request::McpAuditList {
            client_id: client_id.map(str::to_owned),
            before: None,
            limit: Some(500),
        })
        .unwrap()
    {
        Response::McpAudit(rows) => rows,
        other => panic!("unexpected {other:?}"),
    }
}

/// The 36 characters after `key` in a tool's text.
fn id_after(text: &str, key: &str) -> String {
    let start = text.find(key).unwrap() + key.len();
    text[start..start + 36].to_owned()
}

fn page_id_after(text: &str) -> PageId {
    id_after(text, "pageId: ").parse().unwrap()
}

fn markdown(core: &mut Core, page_id: PageId) -> String {
    match core
        .dispatch(Request::PageExportMarkdown { page_id })
        .unwrap()
    {
        Response::Markdown(text) => text,
        other => panic!("unexpected {other:?}"),
    }
}

fn trash(core: &mut Core, page_id: PageId) {
    core.dispatch(Request::PageTrash { page_id }).unwrap();
}

/// A page with a text paragraph.
fn text_page(
    core: &mut Core,
    space: SpaceId,
    parent: Option<PageId>,
    title: &str,
    text: &str,
) -> PageId {
    let created = create_page(core, space, parent, title);
    if !text.is_empty() {
        write_text(core, created.id, text, 77);
    }
    created.id
}

// ----- clients ------------------------------------------------------------------------------------

#[test]
fn a_new_client_gets_a_token_that_is_shown_once_and_only_its_hash_is_stored() {
    let mut env = setup();
    let created = new_client(&mut env.core, "Claude Desktop");
    let token = &created.token;
    assert_eq!(token.len(), 47);
    assert!(token.starts_with("kbm_"));
    assert!(
        token[4..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "base64url without padding: {token}"
    );
    let client = &created.client;
    assert_eq!(client.id.len(), 32);
    assert!(
        client
            .id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(client.name, "Claude Desktop");
    assert!(client.grants.is_empty(), "a new client has no grants");
    assert_eq!(client.created_at, T0);
    assert_eq!(client.last_used_at, None);
    assert!(has_event(&env.core.drain_events(), &Event::McpChanged));

    // The listing never holds the token.
    let listed = clients(&mut env.core);
    assert_eq!(listed, std::slice::from_ref(client));
    assert!(
        !serde_json::to_string(&listed)
            .unwrap()
            .contains(token.as_str())
    );

    // Only the SHA-256 is stored, and it finds the client.
    let row = env
        .core
        .db
        .mcp_client_by_token_hash(&sha(token))
        .unwrap()
        .unwrap();
    assert_eq!(row.id, client.id);
    assert_eq!(row.token_hash, sha(token));
    assert!(!format!("{row:?}").contains(token.as_str()));
    // Nor is the token in any file of the data directory.
    for entry in std::fs::read_dir(env.fixture.dir.path()).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert!(
            !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "the token is written nowhere"
        );
    }
}

#[test]
fn tokens_and_ids_differ_between_clients() {
    let mut env = setup();
    let a = new_client(&mut env.core, "A");
    let b = new_client(&mut env.core, "B");
    assert_ne!(a.token, b.token);
    assert_ne!(a.client.id, b.client.id);
    let names: Vec<String> = clients(&mut env.core).into_iter().map(|c| c.name).collect();
    assert_eq!(names, ["A", "B"], "oldest first");
}

#[test]
fn client_names_are_trimmed_and_limited_to_60_characters() {
    let mut env = setup();
    assert_eq!(
        new_client(&mut env.core, "  Cursor \n").client.name,
        "Cursor"
    );
    let exact = "é".repeat(60);
    assert_eq!(new_client(&mut env.core, &exact).client.name, exact);
    for bad in ["", "   ", &"x".repeat(61), "a\nb", "tab\there"] {
        let err = env
            .core
            .dispatch(Request::McpClientCreate { name: bad.into() })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
    }
    assert_eq!(
        clients(&mut env.core).len(),
        2,
        "refused names create nothing"
    );
}

#[test]
fn grants_write_implies_read_unknown_spaces_are_dropped_and_duplicates_merge() {
    let mut env = setup();
    let created = new_client(&mut env.core, "Agent");
    env.core.drain_events();
    let unknown = SpaceId::from_bytes([9; 16]);
    let info = set_grants(
        &mut env.core,
        &created.client.id,
        vec![
            grant(env.work, false, true),
            grant(unknown, true, true),
            grant(env.personal, true, false),
            // A duplicate of Personal asking for write: the two merge.
            grant(env.personal, false, true),
            // Nothing at all is no grant.
            grant(SpaceId::from_bytes([8; 16]), false, false),
        ],
    );
    assert_eq!(
        info.grants,
        [grant(env.work, true, true), grant(env.personal, true, true)]
    );
    assert!(has_event(&env.core.drain_events(), &Event::McpChanged));
    assert_eq!(clients(&mut env.core)[0].grants, info.grants);

    // A grant of nothing in a space that exists is dropped too; replacing is total.
    let info = set_grants(
        &mut env.core,
        &created.client.id,
        vec![
            grant(env.work, true, false),
            grant(env.personal, false, false),
        ],
    );
    assert_eq!(info.grants, [grant(env.work, true, false)]);
    assert!(
        set_grants(&mut env.core, &created.client.id, Vec::new())
            .grants
            .is_empty()
    );
}

#[test]
fn the_grants_of_a_deleted_space_are_not_listed() {
    let mut env = setup();
    let temp = create_space(&mut env.core, "Temp").id;
    let (client, _) = client_with(
        &mut env.core,
        "Agent",
        vec![grant(env.work, true, false), grant(temp, true, true)],
    );
    assert_eq!(client.grants.len(), 2);
    env.core
        .dispatch(Request::SpaceDelete { space_id: temp })
        .unwrap();
    assert_eq!(
        clients(&mut env.core)[0].grants,
        [grant(env.work, true, false)]
    );
}

#[test]
fn unknown_clients_are_not_found() {
    let mut env = setup();
    let unknown = "ab".repeat(16);
    let err = env
        .core
        .dispatch(Request::McpClientGrantsSet {
            client_id: unknown.clone(),
            grants: vec![grant(env.work, true, false)],
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = env
        .core
        .dispatch(Request::McpClientDelete { client_id: unknown })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn deleting_a_client_stops_its_token_and_keeps_its_audit_rows() {
    let mut env = setup();
    let (client, token) = client_with(&mut env.core, "Agent", vec![grant(env.work, true, false)]);
    ok(&mut env.core, &token, "list_spaces", &json!({}));
    env.core.drain_events();
    env.core
        .dispatch(Request::McpClientDelete {
            client_id: client.id.clone(),
        })
        .unwrap();
    assert!(has_event(&env.core.drain_events(), &Event::McpChanged));
    assert!(clients(&mut env.core).is_empty());
    let err = try_mcp(
        &mut env.core,
        &token,
        &tool_message("list_spaces", &json!({})).to_string(),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let rows = audit_rows(&mut env.core, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].client_name, "Agent");
}

// ----- tokens ---------------------------------------------------------------------------------------

#[test]
fn an_unknown_or_malformed_token_is_invalid_input_for_token_and_leaves_no_trace() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Agent", vec![grant(env.work, true, false)]);
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
    let well_formed_but_unknown = format!("kbm_{}", "A".repeat(43));
    let mut other_case = token.clone();
    other_case.replace_range(5..6, if &token[5..6] == "a" { "b" } else { "a" });
    for bad in [
        "",
        "kbm_",
        "x",
        well_formed_but_unknown.as_str(),
        other_case.as_str(),
        &token[..46],
        &format!("{token}A"),
        &format!("Bearer {token}"),
        &format!(" {token}"),
        &token.to_uppercase(),
        &"é".repeat(40),
    ] {
        let err = try_mcp(&mut env.core, bad, &ping).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad:?}");
        assert_eq!(err.details.as_ref().unwrap()["field"], "token");
        assert!(
            !err.message.contains(&token),
            "the message never echoes a token"
        );
    }
    assert!(audit_rows(&mut env.core, None).is_empty());
    assert_eq!(clients(&mut env.core)[0].last_used_at, None);
    // The right token works.
    assert!(
        try_mcp(&mut env.core, &token, &ping)
            .unwrap()
            .body
            .is_some()
    );
}

#[test]
fn an_authenticated_call_sets_the_last_use() {
    let mut env = setup();
    let (client, token) = client_with(&mut env.core, "Agent", Vec::new());
    assert_eq!(client.last_used_at, None);
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    mcp(&mut env.core, &token, &ping).unwrap();
    assert_eq!(clients(&mut env.core)[0].last_used_at, Some(T0));
    // A notification counts as use too.
    let created = new_client(&mut env.core, "Second");
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert!(mcp(&mut env.core, &created.token, &note).is_none());
    assert_eq!(clients(&mut env.core)[1].last_used_at, Some(T0));
}

#[test]
fn a_message_over_one_mebibyte_is_refused() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Agent", Vec::new());
    let big = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":{{\"x\":\"{}\"}}}}",
        "a".repeat(crate::mcp::MAX_MESSAGE_BYTES)
    );
    let err = try_mcp(&mut env.core, &token, &big).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert_eq!(err.details.as_ref().unwrap()["field"], "message");
}

// ----- the protocol ---------------------------------------------------------------------------------

#[test]
fn the_lifecycle_works_through_mcp_call() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Agent", Vec::new());
    let init = mcp(
        &mut env.core,
        &token,
        &json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }
        }),
    )
    .unwrap();
    assert_eq!(init["id"], "init-1");
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "atomik-kb");
    assert_eq!(
        init["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Markdown")
    );

    let note = try_mcp(
        &mut env.core,
        &token,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .unwrap();
    assert_eq!(note.body, None, "a notification has no reply (HTTP 202)");

    let ping = mcp(
        &mut env.core,
        &token,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }),
    )
    .unwrap();
    assert_eq!(ping["result"], json!({}));
    let list = mcp(
        &mut env.core,
        &token,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "list_spaces",
            "search",
            "list_pages",
            "read_page",
            "create_page",
            "append_to_page"
        ],
        "every tool is listed whatever the grants"
    );
    let unknown = mcp(
        &mut env.core,
        &token,
        &json!({ "jsonrpc": "2.0", "id": 4, "method": "nope" }),
    )
    .unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    let batch = try_mcp(
        &mut env.core,
        &token,
        "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]",
    )
    .unwrap();
    let batch: Value = serde_json::from_str(&batch.body.unwrap()).unwrap();
    assert_eq!(batch["error"]["code"], -32600);
    let broken = try_mcp(&mut env.core, &token, "{oops").unwrap();
    let broken: Value = serde_json::from_str(&broken.body.unwrap()).unwrap();
    assert_eq!(broken["error"]["code"], -32700);
    // None of this is a tool call: nothing is audited.
    assert!(audit_rows(&mut env.core, None).is_empty());
}

// ----- list_spaces ----------------------------------------------------------------------------------

#[test]
fn list_spaces_names_only_the_spaces_the_client_can_read() {
    let mut env = setup();
    text_page(&mut env.core, env.work, None, "One", "");
    text_page(&mut env.core, env.work, None, "Two", "");
    let (_, reader) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    let (_, writer) = client_with(
        &mut env.core,
        "Writer",
        vec![
            grant(env.personal, true, true),
            grant(env.work, true, false),
        ],
    );
    let (_, nobody) = client_with(&mut env.core, "Nobody", Vec::new());

    assert_eq!(
        ok(&mut env.core, &reader, "list_spaces", &json!({})),
        format!(
            "Spaces you can access (1):\n- Work (spaceId: {}, 2 pages): read only",
            env.work
        )
    );
    assert_eq!(
        ok(&mut env.core, &writer, "list_spaces", &json!({})),
        format!(
            "Spaces you can access (2):\n- Work (spaceId: {}, 2 pages): read only\n- Personal (spaceId: {}, 0 pages): read and write",
            env.work, env.personal
        ),
        "in the person's order"
    );
    let none = ok(&mut env.core, &nobody, "list_spaces", &json!({}));
    assert!(
        none.starts_with("No spaces are shared with this client yet."),
        "{none}"
    );
    assert!(!none.contains(&env.work.to_string()));
}

// ----- search ---------------------------------------------------------------------------------------

#[test]
fn search_finds_pages_only_in_readable_spaces() {
    let mut env = setup();
    let budget = text_page(
        &mut env.core,
        env.work,
        None,
        "Budget plan",
        "quarterly numbers",
    );
    text_page(
        &mut env.core,
        env.personal,
        None,
        "Secret budget",
        "budget secret",
    );
    let (_, token) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);

    let text = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "budget" }),
    );
    assert_eq!(
        text,
        format!(
            "Found 1 page for \"budget\":\n\n1. Budget plan\n   pageId: {budget} | space: Work (spaceId: {})",
            env.work
        )
    );
    assert!(!text.contains("Secret"));
    assert!(!text.contains(&env.personal.to_string()));

    // Asking for the other space by id is refused, and says nothing about its pages.
    let denied = fail(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "budget", "spaceId": env.personal.to_string() }),
    );
    assert_eq!(denied, "This client has no read access to that space");
    // A space that does not exist is the same answer.
    let unknown = fail(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "budget", "spaceId": SpaceId::from_bytes([5; 16]).to_string() }),
    );
    assert_eq!(unknown, denied);
    // No match is not an error.
    let none = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "zebra" }),
    );
    assert_eq!(none, "No pages found for \"zebra\".");
}

#[test]
fn search_finds_a_page_by_words_in_its_title_and_text_but_has_no_fuzzy_titles() {
    // ADR-0040: a page matches when every word is somewhere in it; the fuzzy title pass of the
    // search dialog is not run for MCP, so a typo finds nothing.
    let mut env = setup();
    let budget = text_page(
        &mut env.core,
        env.work,
        None,
        "Budget plan",
        "quarterly numbers",
    );
    let (_, token) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    let text = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "budget quarterly" }),
    );
    assert_eq!(
        text,
        format!(
            "Found 1 page for \"budget quarterly\":

1. Budget plan
   pageId: {budget} | space: Work (spaceId: {})",
            env.work
        )
    );
    for typo in ["budgte", "bdgt"] {
        let none = ok(&mut env.core, &token, "search", &json!({ "query": typo }));
        assert_eq!(none, format!("No pages found for \"{typo}\"."));
    }
}

#[test]
fn search_with_no_access_at_all_says_so() {
    let mut env = setup();
    text_page(&mut env.core, env.work, None, "Budget plan", "x");
    let (_, token) = client_with(&mut env.core, "Nobody", Vec::new());
    let text = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "budget" }),
    );
    assert!(
        text.starts_with("No spaces are shared with this client yet."),
        "{text}"
    );
}

#[test]
fn search_lists_each_page_once_respects_the_limit_and_the_space_filter() {
    let mut env = setup();
    for n in 1..=5 {
        text_page(
            &mut env.core,
            env.work,
            None,
            &format!("Alpha {n}"),
            "alpha alpha alpha",
        );
    }
    text_page(&mut env.core, env.personal, None, "Alpha personal", "alpha");
    let (_, token) = client_with(
        &mut env.core,
        "Reader",
        vec![
            grant(env.work, true, false),
            grant(env.personal, true, false),
        ],
    );

    // Title and body both match; every page is listed once.
    let all = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "alpha" }),
    );
    assert!(all.starts_with("Found 6 pages for \"alpha\":"), "{all}");
    for n in 1..=5 {
        assert_eq!(all.matches(&format!("Alpha {n}\n")).count(), 1, "{all}");
    }
    assert!(all.contains("space: Work") && all.contains("space: Personal"));

    // The spaces take turns, so a small limit sees both.
    let two = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "alpha", "limit": 2 }),
    );
    assert!(two.starts_with("Found 2 pages"), "{two}");
    assert!(
        two.contains("space: Work") && two.contains("space: Personal"),
        "{two}"
    );
    assert!(two.contains("\n\n2. "));
    assert!(!two.contains("\n\n3. "));

    let only_personal = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "alpha", "spaceId": env.personal.to_string() }),
    );
    assert!(
        only_personal.starts_with("Found 1 page for"),
        "{only_personal}"
    );
    assert!(only_personal.contains("Alpha personal"));
    // A body match shows its snippet.
    assert!(only_personal.contains("alpha"), "{only_personal}");
}

#[test]
fn search_leaves_out_trashed_pages_and_shows_snippets_of_the_text() {
    let mut env = setup();
    let gone = text_page(
        &mut env.core,
        env.work,
        None,
        "Old notes",
        "pineapple pizza",
    );
    text_page(
        &mut env.core,
        env.work,
        None,
        "Menu",
        "we like pineapple on pizza",
    );
    let (_, token) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    trash(&mut env.core, gone);
    let text = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "pineapple" }),
    );
    assert!(text.starts_with("Found 1 page"), "{text}");
    assert!(text.contains("Menu") && !text.contains("Old notes"));
    assert!(
        text.contains("we like pineapple on pizza"),
        "the snippet shows: {text}"
    );
}

// ----- list_pages -----------------------------------------------------------------------------------

#[test]
fn list_pages_lists_the_top_level_and_the_children_in_order() {
    let mut env = setup();
    let a = text_page(&mut env.core, env.work, None, "A", "");
    let b = text_page(&mut env.core, env.work, None, "B", "");
    let a1 = text_page(&mut env.core, env.work, Some(a), "A1", "");
    let a2 = text_page(&mut env.core, env.work, Some(a), "A2", "");
    let (_, token) = client_with(
        &mut env.core,
        "Reader",
        vec![
            grant(env.work, true, false),
            grant(env.personal, true, false),
        ],
    );
    let space = env.work.to_string();

    assert_eq!(
        ok(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": space })
        ),
        format!(
            "Top-level pages in \"Work\" (2):\n- A (pageId: {a}, 2 subpages)\n- B (pageId: {b})"
        )
    );
    assert_eq!(
        ok(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": space, "parentId": a.to_string() })
        ),
        format!("Subpages of \"A\" (2):\n- A1 (pageId: {a1})\n- A2 (pageId: {a2})")
    );
    assert_eq!(
        ok(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": space, "parentId": b.to_string() })
        ),
        "Subpages of \"B\": none."
    );
    assert_eq!(
        ok(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": env.personal.to_string() })
        ),
        "Top-level pages in \"Personal\": none."
    );
}

#[test]
fn list_pages_checks_the_space_and_the_parent() {
    let mut env = setup();
    let in_personal = text_page(&mut env.core, env.personal, None, "Mine", "");
    let trashed = text_page(&mut env.core, env.work, None, "Trashed", "");
    trash(&mut env.core, trashed);
    let (_, token) = client_with(
        &mut env.core,
        "Reader",
        vec![
            grant(env.work, true, false),
            grant(env.personal, true, false),
        ],
    );
    let (_, narrow) = client_with(&mut env.core, "Narrow", vec![grant(env.work, true, false)]);
    let work = env.work.to_string();

    assert_eq!(
        fail(
            &mut env.core,
            &narrow,
            "list_pages",
            &json!({ "spaceId": env.personal.to_string() })
        ),
        "This client has no read access to that space"
    );
    // A parent of another space.
    let text = fail(
        &mut env.core,
        &token,
        "list_pages",
        &json!({ "spaceId": work, "parentId": in_personal.to_string() }),
    );
    assert!(text.contains("not a page of that space"), "{text}");
    assert_eq!(
        fail(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": work, "parentId": trashed.to_string() })
        ),
        "That page is in the trash"
    );
    assert_eq!(
        fail(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": work, "parentId": PageId::from_bytes([3; 16]).to_string() })
        ),
        "Page not found"
    );
    for bad in ["nope", "123"] {
        let text = fail(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": bad }),
        );
        assert!(text.contains("spaceId is not a valid id"), "{text}");
        let text = fail(
            &mut env.core,
            &token,
            "list_pages",
            &json!({ "spaceId": work, "parentId": bad }),
        );
        assert!(text.contains("pageId is not a valid id"), "{text}");
    }
}

// ----- read_page ------------------------------------------------------------------------------------

#[test]
fn read_page_returns_the_title_the_ids_the_properties_and_the_markdown() {
    let mut env = setup();
    let folder = text_page(&mut env.core, env.work, None, "Folder", "");
    let notes = text_page(
        &mut env.core,
        env.work,
        Some(folder),
        "Notes",
        "first paragraph",
    );
    write_text(&mut env.core, notes, "second paragraph", 78);
    // Properties: a select, a date, a checkbox and a number.
    let status = match env
        .core
        .dispatch(Request::PropertyCreate {
            space_id: env.work,
            name: "Status".into(),
            kind: PropertyKind::Select,
            config: None,
        })
        .unwrap()
    {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    };
    let done = match env
        .core
        .dispatch(Request::PropertyOptionAdd {
            space_id: env.work,
            property_id: status.id,
            name: "Done".into(),
            color: OptionColor::Green,
            group: None,
        })
        .unwrap()
    {
        Response::PropertyOption(option) => option,
        other => panic!("unexpected {other:?}"),
    };
    let set = |env: &mut Env, name: &str, kind: PropertyKind, value: PropertyValue| {
        let id = if name == "Status" {
            status.id
        } else {
            match env
                .core
                .dispatch(Request::PropertyCreate {
                    space_id: env.work,
                    name: name.into(),
                    kind,
                    config: None,
                })
                .unwrap()
            {
                Response::Property(def) => def.id,
                other => panic!("unexpected {other:?}"),
            }
        };
        env.core
            .dispatch(Request::PagePropertySet {
                page_id: notes,
                property_id: id,
                value: Some(value),
            })
            .unwrap();
    };
    set(
        &mut env,
        "Status",
        PropertyKind::Select,
        PropertyValue::Select(done.id),
    );
    set(
        &mut env,
        "Due",
        PropertyKind::Date,
        PropertyValue::Date(DateValue {
            date: "2026-10-20".into(),
            time: Some("14:30".into()),
        }),
    );
    set(
        &mut env,
        "Reviewed",
        PropertyKind::Checkbox,
        PropertyValue::Checkbox(true),
    );
    set(
        &mut env,
        "Points",
        PropertyKind::Number,
        PropertyValue::Number(5.0),
    );

    let (_, token) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    let text = ok(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": notes.to_string() }),
    );
    let body = markdown(&mut env.core, notes);
    assert!(
        body.contains("first paragraph\n\nsecond paragraph"),
        "{body}"
    );
    // The Markdown is shown without the blank lines the page's empty first paragraph leaves.
    let shown = body.trim_start_matches('\n');
    assert_eq!(
        text,
        format!(
            "# Notes\npageId: {notes} | space: Work (spaceId: {}) | parentId: {folder}\n\
             properties: Status: Done; Due: 2026-10-20 14:30; Reviewed: yes; Points: 5\n\n{shown}",
            env.work
        )
    );

    // A page without properties has no properties line, and a top-level page no parent.
    let plain = text_page(&mut env.core, env.work, None, "Plain", "text");
    let text = ok(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": plain.to_string() }),
    );
    let body = markdown(&mut env.core, plain);
    assert!(body.contains("text"), "{body}");
    assert_eq!(
        text,
        format!(
            "# Plain\npageId: {plain} | space: Work (spaceId: {})\n\n{}",
            env.work,
            body.trim_start_matches('\n')
        )
    );
}

#[test]
fn read_page_shows_the_formulas_of_the_page_like_the_property_they_look_like() {
    use kb_types::PropertyConfig;

    let mut env = setup();
    let notes = text_page(&mut env.core, env.work, None, "Notes", "body");
    let create =
        |env: &mut Env, name: &str, kind: PropertyKind, config: Option<PropertyConfig>| match env
            .core
            .dispatch(Request::PropertyCreate {
                space_id: env.work,
                name: name.into(),
                kind,
                config,
            })
            .unwrap()
        {
            Response::Property(def) => def.id,
            other => panic!("unexpected {other:?}"),
        };
    let formula = |expression: String| {
        Some(PropertyConfig::Formula {
            expression,
            result_type: None,
        })
    };
    let points = create(&mut env, "Points", PropertyKind::Number, None);
    env.core
        .dispatch(Request::PagePropertySet {
            page_id: notes,
            property_id: points,
            value: Some(PropertyValue::Number(5.0)),
        })
        .unwrap();
    create(
        &mut env,
        "Double",
        PropertyKind::Formula,
        formula(format!("prop(\"{points}\") * 2")),
    );
    create(
        &mut env,
        "Label",
        PropertyKind::Formula,
        formula("prop(\"title\") + \"!\"".into()),
    );
    create(
        &mut env,
        "Cheap",
        PropertyKind::Formula,
        formula(format!("prop(\"{points}\") < 10")),
    );
    // A formula with no value (a text that is empty) and a computed time are left out.
    create(
        &mut env,
        "Nothing",
        PropertyKind::Formula,
        formula("\"\"".into()),
    );
    create(&mut env, "Created", PropertyKind::CreatedTime, None);

    let (_, token) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    let text = ok(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": notes.to_string() }),
    );
    assert!(
        text.contains("\nproperties: Points: 5; Double: 10; Label: Notes!; Cheap: yes\n"),
        "{text}"
    );
}

#[test]
fn read_page_does_not_name_the_pages_of_spaces_the_client_cannot_read() {
    let mut env = setup();
    let visible = text_page(&mut env.core, env.work, None, "Visible title", "");
    let hidden = text_page(&mut env.core, env.personal, None, "Hidden title", "");
    let mixed = text_page(&mut env.core, env.work, None, "Mixed", "");
    let block = |n: u8| kb_types::BlockId::from_random_bytes([n; 16]).to_string();
    env.core
        .replace_page_document_for_test(
            mixed,
            &json!({
                "type": "doc",
                "content": [{
                    "type": "paragraph",
                    "attrs": { "id": block(1) },
                    "content": [
                        { "type": "text", "text": "see " },
                        { "type": "pageRef", "attrs": { "id": block(2), "pageId": visible.to_string() } },
                        { "type": "text", "text": " and " },
                        { "type": "pageRef", "attrs": { "id": block(3), "pageId": hidden.to_string() } }
                    ]
                }]
            }),
        )
        .unwrap();
    let (_, narrow) = client_with(&mut env.core, "Narrow", vec![grant(env.work, true, false)]);
    let (_, wide) = client_with(
        &mut env.core,
        "Wide",
        vec![
            grant(env.work, true, false),
            grant(env.personal, true, false),
        ],
    );
    let args = json!({ "pageId": mixed.to_string() });

    let text = ok(&mut env.core, &narrow, "read_page", &args);
    assert!(text.contains("Visible title"), "{text}");
    assert!(!text.contains("Hidden title"), "{text}");
    assert!(text.contains(&hidden.to_string()), "the id stays: {text}");
    let text = ok(&mut env.core, &wide, "read_page", &args);
    assert!(
        text.contains("Visible title") && text.contains("Hidden title"),
        "{text}"
    );
}

#[test]
fn read_page_is_denied_without_a_grant_and_refuses_unknown_and_trashed_pages() {
    let mut env = setup();
    let private = text_page(&mut env.core, env.personal, None, "Diary", "dear diary");
    let gone = text_page(&mut env.core, env.work, None, "Gone", "bye");
    trash(&mut env.core, gone);
    let (_, token) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);

    let text = fail(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": private.to_string() }),
    );
    assert_eq!(text, "This client has no read access to that space");
    assert!(!text.contains("diary"));
    assert_eq!(
        fail(
            &mut env.core,
            &token,
            "read_page",
            &json!({ "pageId": gone.to_string() })
        ),
        "That page is in the trash"
    );
    assert_eq!(
        fail(
            &mut env.core,
            &token,
            "read_page",
            &json!({ "pageId": PageId::from_bytes([1; 16]).to_string() })
        ),
        "Page not found"
    );
    let text = fail(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": "not-an-id" }),
    );
    assert!(text.contains("pageId is not a valid id"), "{text}");
    // A write grant reads too, even if the read flag was left off by hand.
    let (_, writer) = client_with(
        &mut env.core,
        "Writer",
        vec![grant(env.personal, false, true)],
    );
    assert!(
        ok(
            &mut env.core,
            &writer,
            "read_page",
            &json!({ "pageId": private.to_string() })
        )
        .contains("dear diary")
    );
}

#[test]
fn read_page_cuts_the_markdown_at_256_kib_with_a_note() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    let line = |n: usize| {
        format!("Line {n:05} of the long page with some filler words to make it longer.\n\n")
    };
    let chunk: String = (0..2_400).map(line).collect();
    assert!(chunk.len() > 100_000 && chunk.len() < 256 * 1024);
    let created = ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": env.work.to_string(), "title": "Long", "markdown": chunk }),
    );
    let page_id = page_id_after(&created);
    ok(
        &mut env.core,
        &token,
        "append_to_page",
        &json!({ "pageId": page_id.to_string(), "markdown": chunk.replace("Line", "More") }),
    );
    let full = markdown(&mut env.core, page_id);
    assert!(full.len() > 256 * 1024, "the page is longer than the limit");

    let text = ok(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": page_id.to_string() }),
    );
    let (head, body) = text.split_once("\n\n").unwrap();
    assert!(head.starts_with("# Long\n"));
    let note_at = body.find("\n\n[Truncated: the page is ").unwrap();
    assert_eq!(
        note_at,
        256 * 1024,
        "exactly 256 KiB of Markdown precede the note"
    );
    assert!(body.starts_with("Line 00000"));
    assert!(body[note_at..].contains("the first 256 KiB are shown"));
    // A page under the limit has no note.
    let small = text_page(&mut env.core, env.work, None, "Small", "short");
    assert!(
        !ok(
            &mut env.core,
            &token,
            "read_page",
            &json!({ "pageId": small.to_string() })
        )
        .contains("Truncated")
    );
}

// ----- create_page ----------------------------------------------------------------------------------

#[test]
fn create_page_makes_a_page_with_its_content_in_the_right_place() {
    let mut env = setup();
    let parent = text_page(&mut env.core, env.work, None, "Projects", "");
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    env.core.drain_events();

    let text = ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({
            "spaceId": env.work.to_string(),
            "parentId": parent.to_string(),
            "title": "  Meeting\nnotes ",
            "markdown": "# Agenda\n\n- one\n- two\n\nSome **bold** text"
        }),
    );
    let created = page_id_after(&text);
    assert_eq!(
        text,
        format!(
            "Created page \"Meeting notes\" (pageId: {created}) in space \"Work\" (spaceId: {}).",
            env.work
        )
    );
    let info = page(&mut env.core, created);
    assert_eq!(info.title, "Meeting notes");
    assert_eq!(info.parent_id, Some(parent));
    assert_eq!(info.space_id, env.work);
    let body = markdown(&mut env.core, created);
    assert!(body.contains("# Agenda"), "{body}");
    assert!(body.contains("- one") && body.contains("- two"), "{body}");
    assert!(body.contains("Some **bold** text"), "{body}");
    assert_eq!(
        outline(&tree(&mut env.core, env.work)),
        "Projects\n  Meeting notes\n"
    );

    // The title and the text are searchable, and the person's UI hears about it.
    let found = ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "agenda" }),
    );
    assert!(found.contains("Meeting notes"), "{found}");
    let events = env.core.drain_events();
    assert!(has_event(
        &events,
        &Event::StructureChanged { space_id: env.work }
    ));
    assert!(has_event(&events, &Event::SpacesChanged));
    assert!(has_event(&events, &Event::McpChanged));
    assert!(has_event(
        &events,
        &Event::PageContentChanged {
            page_id: created,
            source: ChangeSource::Core
        }
    ));
}

#[test]
fn create_page_without_content_makes_an_empty_page_and_a_top_level_page_has_no_parent() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    for args in [
        json!({ "spaceId": env.work.to_string(), "title": "Blank" }),
        json!({ "spaceId": env.work.to_string(), "title": "Blank", "markdown": "" }),
    ] {
        let created = page_id_after(&ok(&mut env.core, &token, "create_page", &args));
        assert_eq!(page(&mut env.core, created).parent_id, None);
        let content = page_content(&mut env.core, created);
        assert_eq!(
            content["content"].as_array().unwrap().len(),
            1,
            "one empty paragraph"
        );
        assert!(content["content"][0].get("content").is_none());
    }
}

#[test]
fn create_page_needs_a_write_grant_and_a_usable_parent() {
    let mut env = setup();
    let in_personal = text_page(&mut env.core, env.personal, None, "Mine", "");
    let trashed = text_page(&mut env.core, env.work, None, "Trashed", "");
    trash(&mut env.core, trashed);
    let (_, reader) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    let (_, writer) = client_with(
        &mut env.core,
        "Writer",
        vec![
            grant(env.work, true, true),
            grant(env.personal, true, false),
        ],
    );
    let work = env.work.to_string();
    let before = outline(&tree(&mut env.core, env.work));

    assert_eq!(
        fail(
            &mut env.core,
            &reader,
            "create_page",
            &json!({ "spaceId": work, "title": "No" })
        ),
        "This client has no write access to that space"
    );
    // Read access to the other space is not write access.
    assert_eq!(
        fail(
            &mut env.core,
            &writer,
            "create_page",
            &json!({ "spaceId": env.personal.to_string(), "title": "No" })
        ),
        "This client has no write access to that space"
    );
    // A parent of another space, a parent in the trash, an unknown parent, an id that is no id.
    let text = fail(
        &mut env.core,
        &writer,
        "create_page",
        &json!({ "spaceId": work, "title": "No", "parentId": in_personal.to_string() }),
    );
    assert!(text.contains("not a page of that space"), "{text}");
    assert_eq!(
        fail(
            &mut env.core,
            &writer,
            "create_page",
            &json!({ "spaceId": work, "title": "No", "parentId": trashed.to_string() })
        ),
        "The parent page is in the trash"
    );
    assert_eq!(
        fail(
            &mut env.core,
            &writer,
            "create_page",
            &json!({ "spaceId": work, "title": "No", "parentId": PageId::from_bytes([4; 16]).to_string() })
        ),
        "Parent page not found"
    );
    let text = fail(
        &mut env.core,
        &writer,
        "create_page",
        &json!({ "spaceId": work, "title": "No", "parentId": "zzz" }),
    );
    assert!(text.contains("pageId is not a valid id"), "{text}");
    assert_eq!(
        outline(&tree(&mut env.core, env.work)),
        before,
        "nothing was created"
    );
}

#[test]
fn pages_made_through_mcp_are_not_an_undo_step_of_the_person() {
    let mut env = setup();
    // The person's own page is an undo step.
    create_page(&mut env.core, env.work, None, "Mine");
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": env.work.to_string(), "title": "Theirs" }),
    );
    assert_eq!(outline(&tree(&mut env.core, env.work)), "Mine\nTheirs\n");

    // Undo takes back the person's page, not the agent's, which did not push it off the stack.
    let undone = match env
        .core
        .dispatch(Request::StructureUndo { space_id: env.work })
        .unwrap()
    {
        Response::Tree(view) => view,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(outline(&undone), "Theirs\n");
    assert!(!undone.undo.can_undo);
    // Redo brings back only what undo took.
    let redone = match env
        .core
        .dispatch(Request::StructureRedo { space_id: env.work })
        .unwrap()
    {
        Response::Tree(view) => view,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(outline(&redone), "Mine\nTheirs\n");
}

#[test]
fn create_page_drops_a_repeated_title_and_keeps_links_to_pages_as_text() {
    let mut env = setup();
    let other = text_page(&mut env.core, env.work, None, "Other page", "x");
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    let work = env.work.to_string();
    let source = format!(
        "# Plan\n\nSee [[Other page]], [site](https://example.com), \
         [by id](atomik-kb://page/{other}) and ![pic](missing.png).\n\n## Next"
    );

    let created = page_id_after(&ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": work, "title": "plan", "markdown": source }),
    ));
    let content = page_content(&mut env.core, created);
    let blocks = content["content"].as_array().unwrap();
    assert_eq!(
        blocks[0]["type"], "paragraph",
        "the first heading repeated the title: {content}"
    );
    assert_eq!(blocks[1]["type"], "heading");
    let json = content.to_string();
    for node in ["pageRef", "pageLink", "\"image\"", "attachmentId"] {
        assert!(!json.contains(node), "no {node} in {json}");
    }
    let body = markdown(&mut env.core, created);
    assert!(body.contains("Other page"), "{body}");
    assert!(body.contains("site"), "{body}");
    // The page did not become a link target.
    let backlinks = env
        .core
        .dispatch(Request::BacklinksGet { page_id: other })
        .unwrap();
    assert!(matches!(backlinks, Response::Backlinks(list) if list.is_empty()));

    // A first heading that is not the title stays.
    let kept = page_id_after(&ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": work, "title": "Overview", "markdown": "# Plan\n\nbody" }),
    ));
    assert!(markdown(&mut env.core, kept).starts_with("# Plan"));
}

// ----- append_to_page -------------------------------------------------------------------------------

#[test]
fn append_to_page_adds_blocks_at_the_end_and_keeps_the_rest() {
    let mut env = setup();
    let log = text_page(&mut env.core, env.work, None, "Log", "first entry");
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    env.core.drain_events();

    let text = ok(
        &mut env.core,
        &token,
        "append_to_page",
        &json!({ "pageId": log.to_string(), "markdown": "## Update\n\n- item\n- item two" }),
    );
    assert_eq!(
        text,
        format!("Appended 2 blocks to \"Log\" (pageId: {log}).")
    );
    let body = markdown(&mut env.core, log);
    let (first, update) = (
        body.find("first entry").unwrap(),
        body.find("## Update").unwrap(),
    );
    assert!(first < update, "{body}");
    assert!(body.contains("- item two"), "{body}");
    let events = env.core.drain_events();
    assert!(has_event(
        &events,
        &Event::PageContentChanged {
            page_id: log,
            source: ChangeSource::Core
        }
    ));
    assert!(has_event(&events, &Event::McpChanged));

    // A single block is "1 block".
    let text = ok(
        &mut env.core,
        &token,
        "append_to_page",
        &json!({ "pageId": log.to_string(), "markdown": "last" }),
    );
    assert_eq!(
        text,
        format!("Appended 1 block to \"Log\" (pageId: {log}).")
    );
    assert!(markdown(&mut env.core, log).ends_with("last\n"));
    // It is searchable.
    assert!(
        ok(
            &mut env.core,
            &token,
            "search",
            &json!({ "query": "item two" })
        )
        .contains("Log")
    );
}

#[test]
fn append_to_page_writes_to_a_locked_page_because_the_lock_is_a_guard_of_the_ui() {
    use kb_types::PageFont;
    let mut env = setup();
    let log = text_page(&mut env.core, env.work, None, "Log", "first entry");
    env.core
        .dispatch(Request::PageSetAppearance {
            page_id: log,
            font: Some(PageFont::Mono),
            small_text: Some(true),
            locked: Some(true),
        })
        .unwrap();
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);

    ok(
        &mut env.core,
        &token,
        "append_to_page",
        &json!({ "pageId": log.to_string(), "markdown": "added by a client" }),
    );
    assert!(markdown(&mut env.core, log).contains("added by a client"));
    let info = page(&mut env.core, log);
    assert_eq!(
        (info.font, info.small_text, info.locked),
        (Some(PageFont::Mono), true, true),
        "the write leaves the look and the lock as they were"
    );
}

#[test]
fn appending_to_an_empty_page_replaces_its_empty_paragraph() {
    let mut env = setup();
    let blank = create_page(&mut env.core, env.work, None, "Blank").id;
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    ok(
        &mut env.core,
        &token,
        "append_to_page",
        &json!({ "pageId": blank.to_string(), "markdown": "# Title\n\ntext" }),
    );
    let content = page_content(&mut env.core, blank);
    let types: Vec<&str> = content["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        ["heading", "paragraph"],
        "no leading empty paragraph: {content}"
    );
}

#[test]
fn append_to_page_needs_a_write_grant_content_and_a_live_page() {
    let mut env = setup();
    let notes = text_page(&mut env.core, env.work, None, "Notes", "keep");
    let private = text_page(&mut env.core, env.personal, None, "Private", "keep");
    let gone = text_page(&mut env.core, env.work, None, "Gone", "keep");
    trash(&mut env.core, gone);
    let (_, reader) = client_with(&mut env.core, "Reader", vec![grant(env.work, true, false)]);
    let (_, writer) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    let page_arg = |id: PageId, md: &str| json!({ "pageId": id.to_string(), "markdown": md });

    assert_eq!(
        fail(
            &mut env.core,
            &reader,
            "append_to_page",
            &page_arg(notes, "more")
        ),
        "This client has no write access to that space"
    );
    assert_eq!(
        fail(
            &mut env.core,
            &writer,
            "append_to_page",
            &page_arg(private, "more")
        ),
        "This client has no write access to that space"
    );
    assert_eq!(
        fail(
            &mut env.core,
            &writer,
            "append_to_page",
            &page_arg(gone, "more")
        ),
        "That page is in the trash"
    );
    assert_eq!(
        fail(
            &mut env.core,
            &writer,
            "append_to_page",
            &page_arg(PageId::from_bytes([6; 16]), "more")
        ),
        "Page not found"
    );
    // Markdown that has no content at all.
    let text = fail(
        &mut env.core,
        &writer,
        "append_to_page",
        &page_arg(notes, "  \n\n "),
    );
    assert!(text.contains("no content"), "{text}");
    assert_eq!(markdown(&mut env.core, notes).trim(), "keep");
    assert_eq!(markdown(&mut env.core, private).trim(), "keep");
}

// ----- the audit log --------------------------------------------------------------------------------

#[test]
fn every_tool_call_is_audited_with_its_outcome_and_a_summary_without_page_text() {
    let mut env = setup();
    let diary = text_page(
        &mut env.core,
        env.personal,
        None,
        "Diary",
        "SECRET BODY of the diary",
    );
    let notes = text_page(
        &mut env.core,
        env.work,
        None,
        "Notes",
        "SECRET BODY of the notes",
    );
    let (client, token) = client_with(
        &mut env.core,
        "Claude Desktop",
        vec![grant(env.work, true, false)],
    );
    env.core.drain_events();

    ok(&mut env.core, &token, "list_spaces", &json!({}));
    ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "  secret \n body " }),
    );
    fail(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": "secret", "spaceId": env.personal.to_string() }),
    );
    ok(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": notes.to_string() }),
    );
    fail(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": diary.to_string() }),
    );
    fail(
        &mut env.core,
        &token,
        "read_page",
        &json!({ "pageId": PageId::from_bytes([2; 16]).to_string() }),
    );
    fail(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": env.work.to_string(), "title": "Sneaky" }),
    );
    fail(
        &mut env.core,
        &token,
        "list_pages",
        &json!({ "spaceId": "bad" }),
    );
    let events = env.core.drain_events();
    assert_eq!(
        events.iter().filter(|e| **e == Event::McpChanged).count(),
        8,
        "one McpChanged per audit write"
    );

    let rows = audit_rows(&mut env.core, None);
    assert_eq!(rows.len(), 8);
    // Newest first, with increasing ids going back in time.
    assert!(rows.windows(2).all(|w| w[0].id > w[1].id));
    let summary: Vec<(&str, &str, McpOutcome)> = rows
        .iter()
        .rev()
        .map(|r| (r.tool.as_str(), r.summary.as_str(), r.outcome))
        .collect();
    assert_eq!(
        summary,
        [
            ("list_spaces", "", McpOutcome::Ok),
            ("search", "secret body", McpOutcome::Ok),
            ("search", "secret", McpOutcome::Denied),
            ("read_page", "Notes", McpOutcome::Ok),
            ("read_page", "", McpOutcome::Denied),
            ("read_page", "", McpOutcome::Error),
            ("create_page", "Sneaky", McpOutcome::Denied),
            ("list_pages", "", McpOutcome::Invalid),
        ]
    );
    for row in &rows {
        assert_eq!(row.client_id, client.id);
        assert_eq!(row.client_name, "Claude Desktop");
        assert_eq!(row.at, T0);
    }
    // The space and page the call was about.
    let by_order: Vec<_> = rows.iter().rev().collect();
    assert_eq!((by_order[0].space_id, by_order[0].page_id), (None, None));
    assert_eq!(
        (by_order[2].space_id, by_order[2].page_id),
        (Some(env.personal), None)
    );
    assert_eq!(
        (by_order[3].space_id, by_order[3].page_id),
        (Some(env.work), Some(notes))
    );
    assert_eq!(
        (by_order[4].space_id, by_order[4].page_id),
        (Some(env.personal), Some(diary))
    );
    assert_eq!(by_order[6].space_id, Some(env.work));
    // No page text, anywhere in the log.
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("SECRET BODY")
    );
}

#[test]
fn a_created_page_is_audited_with_its_id_and_title() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    let created = page_id_after(&ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": env.work.to_string(), "title": "Fresh", "markdown": "private words" }),
    ));
    ok(
        &mut env.core,
        &token,
        "append_to_page",
        &json!({ "pageId": created.to_string(), "markdown": "more private words" }),
    );
    let rows = audit_rows(&mut env.core, None);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[1].tool.as_str(), rows[1].summary.as_str()),
        ("create_page", "Fresh")
    );
    assert_eq!(
        (rows[1].space_id, rows[1].page_id),
        (Some(env.work), Some(created))
    );
    assert_eq!(
        (rows[0].tool.as_str(), rows[0].summary.as_str()),
        ("append_to_page", "Fresh")
    );
    assert_eq!(rows[0].outcome, McpOutcome::Ok);
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("private words")
    );
}

#[test]
fn a_long_query_or_title_is_cut_to_200_characters_in_the_audit_log() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Writer", vec![grant(env.work, true, true)]);
    let query = "word ".repeat(40);
    ok(
        &mut env.core,
        &token,
        "search",
        &json!({ "query": query.trim() }),
    );
    let rows = audit_rows(&mut env.core, None);
    assert_eq!(
        rows[0].summary.chars().count(),
        199,
        "199 characters of the query"
    );
    let title = "t".repeat(200);
    ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": env.work.to_string(), "title": title }),
    );
    let rows = audit_rows(&mut env.core, None);
    assert_eq!(rows[0].summary.chars().count(), 200);
}

#[test]
fn refused_calls_are_32602_and_audited_as_invalid_with_the_reason() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Agent", vec![grant(env.work, true, true)]);
    env.core.drain_events();
    for (params, code) in [
        (json!({ "name": "rm_rf", "arguments": {} }), -32602),
        (json!({ "name": "search", "arguments": {} }), -32602),
        (
            json!({ "name": "search", "arguments": { "query": "x", "limit": 99 } }),
            -32602,
        ),
        (
            json!({ "name": "create_page", "arguments": { "spaceId": "s" } }),
            -32602,
        ),
        (json!({ "arguments": {} }), -32602),
    ] {
        let reply = mcp(
            &mut env.core,
            &token,
            &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": params }),
        )
        .unwrap();
        assert_eq!(reply["error"]["code"], code, "{params}");
        assert_eq!(reply["id"], 9);
    }
    let rows = audit_rows(&mut env.core, None);
    let summary: Vec<(&str, &str, McpOutcome)> = rows
        .iter()
        .rev()
        .map(|r| (r.tool.as_str(), r.summary.as_str(), r.outcome))
        .collect();
    assert_eq!(
        summary,
        [
            ("rm_rf", "unknown tool rm_rf", McpOutcome::Invalid),
            (
                "search",
                "search: query must be 1 to 200 characters",
                McpOutcome::Invalid
            ),
            (
                "search",
                "search: limit must be a whole number from 1 to 20",
                McpOutcome::Invalid
            ),
            (
                "create_page",
                "create_page: title must be 1 to 200 characters",
                McpOutcome::Invalid
            ),
            ("(none)", "the tool name is missing", McpOutcome::Invalid),
        ]
    );
    assert_eq!(
        env.core
            .drain_events()
            .iter()
            .filter(|e| **e == Event::McpChanged)
            .count(),
        5
    );
}

#[test]
fn the_audit_list_filters_by_client_pages_and_validates_its_limit() {
    let mut env = setup();
    let (a, token_a) = client_with(&mut env.core, "A", vec![grant(env.work, true, false)]);
    let (b, token_b) = client_with(&mut env.core, "B", vec![grant(env.work, true, false)]);
    for n in 0..6 {
        let token = if n % 2 == 0 { &token_a } else { &token_b };
        ok(&mut env.core, token, "list_spaces", &json!({}));
    }
    let of_a = audit_rows(&mut env.core, Some(&a.id));
    assert_eq!(of_a.len(), 3);
    assert!(of_a.iter().all(|r| r.client_id == a.id));
    assert_eq!(audit_rows(&mut env.core, Some(&b.id)).len(), 3);
    assert!(audit_rows(&mut env.core, Some(&"ee".repeat(16))).is_empty());

    let list = |core: &mut Core,
                before: Option<i64>,
                limit: Option<u32>|
     -> Result<Vec<McpAuditEntry>, ApiError> {
        match core.dispatch(Request::McpAuditList {
            client_id: None,
            before,
            limit,
        })? {
            Response::McpAudit(rows) => Ok(rows),
            other => panic!("unexpected {other:?}"),
        }
    };
    let first = list(&mut env.core, None, Some(4)).unwrap();
    assert_eq!(first.len(), 4);
    let rest = list(&mut env.core, Some(first[3].id), Some(4)).unwrap();
    assert_eq!(rest.len(), 2);
    assert!(rest[0].id < first[3].id);
    assert_eq!(
        list(&mut env.core, None, None).unwrap().len(),
        6,
        "the default limit is 100"
    );
    for bad in [0, 501] {
        let err = list(&mut env.core, None, Some(bad)).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}");
        assert_eq!(err.details.as_ref().unwrap()["field"], "limit");
    }
    assert_eq!(list(&mut env.core, None, Some(500)).unwrap().len(), 6);
}

#[test]
fn clearing_the_audit_log_deletes_every_row_and_announces_it() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Agent", vec![grant(env.work, true, false)]);
    ok(&mut env.core, &token, "list_spaces", &json!({}));
    ok(&mut env.core, &token, "list_spaces", &json!({}));
    env.core.drain_events();
    assert!(matches!(
        env.core.dispatch(Request::McpAuditClear).unwrap(),
        Response::Ok
    ));
    assert!(has_event(&env.core.drain_events(), &Event::McpChanged));
    assert!(audit_rows(&mut env.core, None).is_empty());
    assert_eq!(clients(&mut env.core).len(), 1, "the clients stay");
    // The next call is audited again.
    ok(&mut env.core, &token, "list_spaces", &json!({}));
    assert_eq!(audit_rows(&mut env.core, None).len(), 1);
}

#[test]
fn a_call_that_cannot_be_audited_fails_instead_of_answering() {
    let mut env = setup();
    let (_, token) = client_with(&mut env.core, "Agent", vec![grant(env.work, true, false)]);
    env.core
        .db
        .connection()
        .execute_batch("DROP TABLE mcp_audit")
        .unwrap();
    let err = try_mcp(
        &mut env.core,
        &token,
        &tool_message("list_spaces", &json!({})).to_string(),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Storage);
    // Messages that are not tool calls need no audit row.
    assert!(
        try_mcp(
            &mut env.core,
            &token,
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#
        )
        .unwrap()
        .body
        .is_some()
    );
}

// ----- clients apart, restarts ------------------------------------------------------------------------

#[test]
fn clients_do_not_see_each_others_grants() {
    let mut env = setup();
    text_page(&mut env.core, env.work, None, "Work page", "shared words");
    text_page(
        &mut env.core,
        env.personal,
        None,
        "Personal page",
        "shared words",
    );
    let (_, work_token) = client_with(
        &mut env.core,
        "WorkOnly",
        vec![grant(env.work, true, false)],
    );
    let (_, personal_token) = client_with(
        &mut env.core,
        "PersonalOnly",
        vec![grant(env.personal, true, false)],
    );
    let a = ok(
        &mut env.core,
        &work_token,
        "search",
        &json!({ "query": "shared" }),
    );
    let b = ok(
        &mut env.core,
        &personal_token,
        "search",
        &json!({ "query": "shared" }),
    );
    assert!(
        a.contains("Work page") && !a.contains("Personal page"),
        "{a}"
    );
    assert!(
        b.contains("Personal page") && !b.contains("Work page"),
        "{b}"
    );
    // Changing one client's grants leaves the other's alone.
    let clients = clients(&mut env.core);
    set_grants(&mut env.core, &clients[0].id, Vec::new());
    assert!(
        ok(
            &mut env.core,
            &personal_token,
            "search",
            &json!({ "query": "shared" })
        )
        .contains("Personal page")
    );
    assert!(
        ok(
            &mut env.core,
            &work_token,
            "search",
            &json!({ "query": "shared" })
        )
        .starts_with("No spaces")
    );
}

#[test]
fn clients_grants_and_the_audit_log_survive_a_restart() {
    let mut env = setup();
    let (client, token) = client_with(&mut env.core, "Agent", vec![grant(env.work, true, true)]);
    let page_id = page_id_after(&ok(
        &mut env.core,
        &token,
        "create_page",
        &json!({ "spaceId": env.work.to_string(), "title": "Persisted", "markdown": "text" }),
    ));
    let Env {
        fixture,
        core,
        work,
        ..
    } = env;
    drop(core);
    let mut core = fixture.open(2, T0 + 5);
    assert_eq!(clients(&mut core)[0].id, client.id);
    assert_eq!(clients(&mut core)[0].grants, [grant(work, true, true)]);
    // The token still works and the page is there.
    let text = ok(
        &mut core,
        &token,
        "read_page",
        &json!({ "pageId": page_id.to_string() }),
    );
    assert!(text.starts_with("# Persisted\n"), "{text}");
    let rows = audit_rows(&mut core, None);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].tool, "create_page");
    assert_eq!(rows[0].tool, "read_page");
    // A client made after the restart gets another token.
    assert_ne!(new_client(&mut core, "Next").token, token);
}
