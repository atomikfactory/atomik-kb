//! The tool catalog: names, descriptions and JSON schemas of `tools/list`, and the parsing of
//! `tools/call` arguments into a [`ToolCall`](crate::ToolCall).
//!
//! Arguments are read leniently where a model is likely to be sloppy and strictly where it
//! matters: unknown keys are ignored, `null` and an empty string stand for an absent optional
//! argument, whitespace (newlines included) in a query or a title becomes single spaces, and
//! limits are checked, never clamped. Every reason names the tool and the argument and never
//! repeats what the client sent, apart from the tool name cut to 64 characters.

use serde_json::{Map, Value, json};

use crate::ToolCall;

/// Tool name: the spaces the client may use.
pub const LIST_SPACES: &str = "list_spaces";
/// Tool name: full-text search.
pub const SEARCH: &str = "search";
/// Tool name: the children of a page or the top level of a space.
pub const LIST_PAGES: &str = "list_pages";
/// Tool name: one page as Markdown.
pub const READ_PAGE: &str = "read_page";
/// Tool name: a new page.
pub const CREATE_PAGE: &str = "create_page";
/// Tool name: more content at the end of a page.
pub const APPEND_TO_PAGE: &str = "append_to_page";

/// Most characters of a search query.
pub const MAX_QUERY_CHARS: usize = 200;
/// Most characters of a title.
pub const MAX_TITLE_CHARS: usize = 200;
/// Most bytes of Markdown in a write.
pub const MAX_MARKDOWN_BYTES: usize = 256 * 1024;
/// Most results of a search.
pub const MAX_SEARCH_LIMIT: u32 = 20;
/// Results of a search when `limit` is left out.
pub const DEFAULT_SEARCH_LIMIT: u32 = 10;

/// Longest id argument, in characters (ids are 36-character UUIDs).
const MAX_ID_CHARS: usize = 64;
/// Longest tool name echoed in a reason, in characters.
const MAX_NAME_ECHO_CHARS: usize = 64;

/// The `tools` array of `tools/list`: every tool, whatever the client's grants.
#[must_use]
pub fn catalog() -> Value {
    let id = |what: &str| json!({ "type": "string", "description": what });
    Value::Array(vec![
        tool(
            LIST_SPACES,
            "List spaces",
            "List the spaces this client can use, with their ids and whether it can only read \
             them or also write. Call this first.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            true,
        ),
        tool(
            SEARCH,
            "Search pages",
            "Search the text of the pages (titles, content and properties) in the spaces this \
             client can read. Returns matching pages with their ids and a snippet. Words are \
             matched by prefix and must all appear.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_QUERY_CHARS,
                        "description": "Words to look for.",
                    },
                    "spaceId": id("Only search this space (an id from list_spaces)."),
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_SEARCH_LIMIT,
                        "default": DEFAULT_SEARCH_LIMIT,
                        "description": "Most pages to return.",
                    },
                },
                "required": ["query"],
                "additionalProperties": false,
            }),
            true,
        ),
        tool(
            LIST_PAGES,
            "List pages",
            "List the pages directly under a page, or at the top level of a space when no \
             parentId is given, in the order the person arranged them.",
            json!({
                "type": "object",
                "properties": {
                    "spaceId": id("The space (an id from list_spaces)."),
                    "parentId": id("List the children of this page. Leave out for the top level."),
                },
                "required": ["spaceId"],
                "additionalProperties": false,
            }),
            true,
        ),
        tool(
            READ_PAGE,
            "Read a page",
            "Read one page: its title, its properties and its content as Markdown.",
            json!({
                "type": "object",
                "properties": { "pageId": id("The page (an id from search or list_pages).") },
                "required": ["pageId"],
                "additionalProperties": false,
            }),
            true,
        ),
        tool(
            CREATE_PAGE,
            "Create a page",
            "Create a new page in a space the client can write to, optionally under a parent \
             page and with content. Returns the new page's id. The Markdown is converted to \
             blocks; images and links to other pages are kept as text.",
            json!({
                "type": "object",
                "properties": {
                    "spaceId": id("The space to create the page in."),
                    "parentId": id("Create it under this page. Leave out for the top level."),
                    "title": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_TITLE_CHARS,
                        "description": "The page title, on one line.",
                    },
                    "markdown": {
                        "type": "string",
                        "description": "The content as Markdown, at most 256 KiB. Leave out for \
                                        an empty page.",
                    },
                },
                "required": ["spaceId", "title"],
                "additionalProperties": false,
            }),
            false,
        ),
        tool(
            APPEND_TO_PAGE,
            "Append to a page",
            "Add Markdown content at the end of an existing page in a space the client can \
             write to. Existing content is never changed or removed.",
            json!({
                "type": "object",
                "properties": {
                    "pageId": id("The page (an id from search or list_pages)."),
                    "markdown": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The content to add as Markdown, at most 256 KiB.",
                    },
                },
                "required": ["pageId", "markdown"],
                "additionalProperties": false,
            }),
            false,
        ),
    ])
}

/// One entry of the catalog. Write tools only add content, so they are not destructive.
fn tool(name: &str, title: &str, description: &str, input_schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": input_schema,
        "annotations": {
            "title": title,
            "readOnlyHint": read_only,
            "destructiveHint": false,
            "idempotentHint": read_only,
            "openWorldHint": false,
        },
    })
}

/// Parses `tools/call` params (`name`, `arguments`) into a call; `Err` is the reason (unknown
/// tool, missing or invalid argument) for a -32602 error.
///
/// # Errors
///
/// A short reason naming the tool or the argument.
pub fn parse_call(name: &str, arguments: &Value) -> Result<ToolCall, String> {
    let empty = Map::new();
    let args = match arguments {
        Value::Object(args) => args,
        Value::Null => &empty,
        _ => return Err("arguments must be an object".to_owned()),
    };
    match name {
        LIST_SPACES => Ok(ToolCall::ListSpaces),
        SEARCH => {
            let query = one_line(string(args, SEARCH, "query")?.unwrap_or_default());
            if query.is_empty() || query.chars().count() > MAX_QUERY_CHARS {
                return Err(format!(
                    "{SEARCH}: query must be 1 to {MAX_QUERY_CHARS} characters"
                ));
            }
            Ok(ToolCall::Search {
                query,
                space_id: optional_id(args, SEARCH, "spaceId")?,
                limit: limit(args)?,
            })
        }
        LIST_PAGES => Ok(ToolCall::ListPages {
            space_id: required_id(args, LIST_PAGES, "spaceId")?,
            parent_id: optional_id(args, LIST_PAGES, "parentId")?,
        }),
        READ_PAGE => Ok(ToolCall::ReadPage {
            page_id: required_id(args, READ_PAGE, "pageId")?,
        }),
        CREATE_PAGE => {
            let title = one_line(string(args, CREATE_PAGE, "title")?.unwrap_or_default());
            if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
                return Err(format!(
                    "{CREATE_PAGE}: title must be 1 to {MAX_TITLE_CHARS} characters"
                ));
            }
            let markdown = markdown(args, CREATE_PAGE)?.filter(|text| !text.is_empty());
            Ok(ToolCall::CreatePage {
                space_id: required_id(args, CREATE_PAGE, "spaceId")?,
                parent_id: optional_id(args, CREATE_PAGE, "parentId")?,
                title,
                markdown,
            })
        }
        APPEND_TO_PAGE => {
            let page_id = required_id(args, APPEND_TO_PAGE, "pageId")?;
            let markdown = markdown(args, APPEND_TO_PAGE)?
                .filter(|text| !text.is_empty())
                .ok_or_else(|| format!("{APPEND_TO_PAGE}: markdown must not be empty"))?;
            Ok(ToolCall::AppendToPage { page_id, markdown })
        }
        other => {
            let shown: String = other.chars().take(MAX_NAME_ECHO_CHARS).collect();
            Err(format!("unknown tool {shown}"))
        }
    }
}

/// A string argument: `None` when absent or `null`.
fn string<'a>(
    args: &'a Map<String, Value>,
    tool: &str,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(format!("{tool}: {key} must be a string")),
    }
}

/// An id argument that must be there.
fn required_id(args: &Map<String, Value>, tool: &str, key: &str) -> Result<String, String> {
    optional_id(args, tool, key)?.ok_or_else(|| format!("{tool}: {key} is required"))
}

/// An id argument that may be left out (also as `null` or an empty string).
fn optional_id(args: &Map<String, Value>, tool: &str, key: &str) -> Result<Option<String>, String> {
    let Some(text) = string(args, tool, key)? else {
        return Ok(None);
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if text.chars().count() > MAX_ID_CHARS || text.chars().any(char::is_control) {
        return Err(format!("{tool}: {key} is not a valid id"));
    }
    Ok(Some(text.to_owned()))
}

/// The Markdown argument, at most [`MAX_MARKDOWN_BYTES`] bytes.
fn markdown(args: &Map<String, Value>, tool: &str) -> Result<Option<String>, String> {
    let Some(text) = string(args, tool, "markdown")? else {
        return Ok(None);
    };
    if text.len() > MAX_MARKDOWN_BYTES {
        return Err(format!(
            "{tool}: markdown must be at most {} KiB",
            MAX_MARKDOWN_BYTES / 1024
        ));
    }
    Ok(Some(text.to_owned()))
}

/// The `limit` of a search: a whole number from 1 to [`MAX_SEARCH_LIMIT`], 10 when absent.
fn limit(args: &Map<String, Value>) -> Result<u32, String> {
    let bad = || format!("{SEARCH}: limit must be a whole number from 1 to {MAX_SEARCH_LIMIT}");
    let wanted = match args.get("limit") {
        None | Some(Value::Null) => return Ok(DEFAULT_SEARCH_LIMIT),
        Some(Value::Number(number)) => number
            .as_u64()
            .or_else(|| {
                // `5.0` is a whole number too.
                number
                    .as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0 && *f <= f64::from(u32::MAX))
                    .map(|f| f as u64)
            })
            .ok_or_else(bad)?,
        Some(_) => return Err(bad()),
    };
    u32::try_from(wanted)
        .ok()
        .filter(|limit| (1..=MAX_SEARCH_LIMIT).contains(limit))
        .ok_or_else(bad)
}

/// One line of single-spaced text: control characters and runs of whitespace become one space,
/// the ends are trimmed.
fn one_line(raw: &str) -> String {
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: Value) -> Result<ToolCall, String> {
        parse_call(name, &arguments)
    }

    #[test]
    fn the_catalog_lists_the_six_tools_with_object_schemas() {
        let tools = catalog();
        let tools = tools.as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "list_spaces",
                "search",
                "list_pages",
                "read_page",
                "create_page",
                "append_to_page"
            ]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert!(tool["description"].as_str().unwrap().len() > 20);
            assert!(tool["title"].is_string());
            assert_eq!(tool["annotations"]["openWorldHint"], false);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
        }
        let read_only: Vec<bool> = tools
            .iter()
            .map(|t| t["annotations"]["readOnlyHint"].as_bool().unwrap())
            .collect();
        assert_eq!(read_only, [true, true, true, true, false, false]);
    }

    #[test]
    fn required_arguments_of_the_schemas_match_the_parser() {
        let tools = catalog();
        for tool in tools.as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let required: Vec<&str> = tool["inputSchema"]["required"]
                .as_array()
                .map(|r| r.iter().map(|v| v.as_str().unwrap()).collect())
                .unwrap_or_default();
            // Every required argument is a property of the schema.
            for key in &required {
                assert!(
                    tool["inputSchema"]["properties"][*key].is_object(),
                    "{name}"
                );
            }
            // With nothing, the tools that need arguments are refused, naming the first one;
            // with exactly the required ones, every tool parses.
            let mut args = Map::new();
            for key in &required {
                let value = if *key == "title" || *key == "query" || *key == "markdown" {
                    json!("x")
                } else {
                    json!("some-id")
                };
                args.insert((*key).to_owned(), value);
            }
            assert!(parse_call(name, &Value::Object(args)).is_ok(), "{name}");
            if !required.is_empty() {
                assert!(parse_call(name, &json!({})).is_err(), "{name}");
            }
        }
    }

    #[test]
    fn list_spaces_takes_no_arguments_and_ignores_extras() {
        assert_eq!(call("list_spaces", Value::Null), Ok(ToolCall::ListSpaces));
        assert_eq!(call("list_spaces", json!({})), Ok(ToolCall::ListSpaces));
        assert_eq!(
            call("list_spaces", json!({ "whatever": 1 })),
            Ok(ToolCall::ListSpaces)
        );
        assert!(call("list_spaces", json!([1])).is_err());
        assert!(call("list_spaces", json!("x")).is_err());
    }

    #[test]
    fn search_reads_query_space_and_limit() {
        assert_eq!(
            call("search", json!({ "query": "  weekly \n report\t" })),
            Ok(ToolCall::Search {
                query: "weekly report".into(),
                space_id: None,
                limit: 10
            })
        );
        assert_eq!(
            call(
                "search",
                json!({ "query": "x", "spaceId": " abc ", "limit": 20 })
            ),
            Ok(ToolCall::Search {
                query: "x".into(),
                space_id: Some("abc".into()),
                limit: 20
            })
        );
        // null and an empty string stand for "absent"; a whole float is a whole number.
        assert_eq!(
            call(
                "search",
                json!({ "query": "x", "spaceId": "", "limit": 5.0 })
            ),
            Ok(ToolCall::Search {
                query: "x".into(),
                space_id: None,
                limit: 5
            })
        );
        assert_eq!(
            call(
                "search",
                json!({ "query": "x", "spaceId": null, "limit": null })
            ),
            Ok(ToolCall::Search {
                query: "x".into(),
                space_id: None,
                limit: 10
            })
        );
    }

    #[test]
    fn search_refuses_bad_arguments_with_a_reason_that_names_the_argument() {
        let query_limit = "x".repeat(201);
        for (args, wanted) in [
            (json!({}), "query"),
            (json!({ "query": "" }), "query"),
            (json!({ "query": " \n\t " }), "query"),
            (json!({ "query": 5 }), "query must be a string"),
            (json!({ "query": query_limit }), "query"),
            (json!({ "query": "x", "limit": 0 }), "limit"),
            (json!({ "query": "x", "limit": 21 }), "limit"),
            (json!({ "query": "x", "limit": -1 }), "limit"),
            (json!({ "query": "x", "limit": 2.5 }), "limit"),
            (json!({ "query": "x", "limit": "5" }), "limit"),
            (json!({ "query": "x", "spaceId": 7 }), "spaceId"),
            (
                json!({ "query": "x", "spaceId": "a".repeat(65) }),
                "spaceId",
            ),
        ] {
            let reason = call("search", args.clone()).unwrap_err();
            assert!(reason.starts_with("search: "), "{reason}");
            assert!(reason.contains(wanted), "{args}: {reason}");
        }
        // A query of exactly 200 characters is fine; characters, not bytes, are counted.
        assert!(call("search", json!({ "query": "é".repeat(200) })).is_ok());
    }

    #[test]
    fn list_pages_and_read_page_need_their_ids() {
        assert_eq!(
            call("list_pages", json!({ "spaceId": "s" })),
            Ok(ToolCall::ListPages {
                space_id: "s".into(),
                parent_id: None
            })
        );
        assert_eq!(
            call("list_pages", json!({ "spaceId": "s", "parentId": "p" })),
            Ok(ToolCall::ListPages {
                space_id: "s".into(),
                parent_id: Some("p".into())
            })
        );
        assert!(
            call("list_pages", json!({}))
                .unwrap_err()
                .contains("spaceId")
        );
        assert!(call("list_pages", json!({ "spaceId": "  " })).is_err());
        assert_eq!(
            call("read_page", json!({ "pageId": "p" })),
            Ok(ToolCall::ReadPage {
                page_id: "p".into()
            })
        );
        assert!(call("read_page", json!({ "pageId": null })).is_err());
        assert!(call("read_page", json!({ "pageId": "a\nb" })).is_err());
    }

    #[test]
    fn create_page_checks_the_title_and_the_markdown() {
        assert_eq!(
            call(
                "create_page",
                json!({ "spaceId": "s", "title": "  My\npage  ", "markdown": "# Hi" })
            ),
            Ok(ToolCall::CreatePage {
                space_id: "s".into(),
                parent_id: None,
                title: "My page".into(),
                markdown: Some("# Hi".into())
            })
        );
        // No content, or empty content: an empty page.
        for args in [
            json!({ "spaceId": "s", "title": "T" }),
            json!({ "spaceId": "s", "title": "T", "markdown": "" }),
            json!({ "spaceId": "s", "title": "T", "markdown": null }),
        ] {
            assert_eq!(
                call("create_page", args),
                Ok(ToolCall::CreatePage {
                    space_id: "s".into(),
                    parent_id: None,
                    title: "T".into(),
                    markdown: None
                })
            );
        }
        assert_eq!(
            call(
                "create_page",
                json!({ "spaceId": "s", "parentId": "p", "title": "T" })
            ),
            Ok(ToolCall::CreatePage {
                space_id: "s".into(),
                parent_id: Some("p".into()),
                title: "T".into(),
                markdown: None
            })
        );
        for (args, wanted) in [
            (json!({ "title": "T" }), "spaceId"),
            (json!({ "spaceId": "s" }), "title"),
            (json!({ "spaceId": "s", "title": "" }), "title"),
            (json!({ "spaceId": "s", "title": "x".repeat(201) }), "title"),
            (json!({ "spaceId": "s", "title": 3 }), "title"),
            (
                json!({ "spaceId": "s", "title": "T", "markdown": 3 }),
                "markdown",
            ),
            (
                json!({ "spaceId": "s", "title": "T", "markdown": "x".repeat(MAX_MARKDOWN_BYTES + 1) }),
                "256 KiB",
            ),
        ] {
            let reason = call("create_page", args.clone()).unwrap_err();
            assert!(reason.starts_with("create_page: "), "{reason}");
            assert!(reason.contains(wanted), "{args}: {reason}");
        }
        // Exactly 256 KiB is accepted, and the limit counts bytes.
        let exact = "x".repeat(MAX_MARKDOWN_BYTES);
        assert!(
            call(
                "create_page",
                json!({ "spaceId": "s", "title": "T", "markdown": exact })
            )
            .is_ok()
        );
        let wide = "é".repeat(MAX_MARKDOWN_BYTES / 2 + 1);
        assert!(
            call(
                "create_page",
                json!({ "spaceId": "s", "title": "T", "markdown": wide })
            )
            .is_err()
        );
    }

    #[test]
    fn append_to_page_needs_content() {
        assert_eq!(
            call(
                "append_to_page",
                json!({ "pageId": "p", "markdown": "more" })
            ),
            Ok(ToolCall::AppendToPage {
                page_id: "p".into(),
                markdown: "more".into()
            })
        );
        for args in [
            json!({ "pageId": "p" }),
            json!({ "pageId": "p", "markdown": "" }),
            json!({ "pageId": "p", "markdown": null }),
            json!({ "markdown": "x" }),
        ] {
            assert!(call("append_to_page", args).is_err());
        }
        // Whitespace is content as far as the parser goes (the core says what it converts to).
        assert!(call("append_to_page", json!({ "pageId": "p", "markdown": " " })).is_ok());
    }

    #[test]
    fn an_unknown_tool_is_refused_and_its_name_is_cut() {
        assert_eq!(
            call("delete_everything", json!({})),
            Err("unknown tool delete_everything".to_owned())
        );
        let long = "n".repeat(500);
        let reason = call(&long, json!({})).unwrap_err();
        assert_eq!(reason.chars().count(), "unknown tool ".len() + 64);
    }

    #[test]
    fn tool_names_round_trip_through_tool_call() {
        for (call, name) in [
            (ToolCall::ListSpaces, "list_spaces"),
            (
                ToolCall::Search {
                    query: "q".into(),
                    space_id: None,
                    limit: 1,
                },
                "search",
            ),
            (
                ToolCall::ListPages {
                    space_id: "s".into(),
                    parent_id: None,
                },
                "list_pages",
            ),
            (
                ToolCall::ReadPage {
                    page_id: "p".into(),
                },
                "read_page",
            ),
            (
                ToolCall::CreatePage {
                    space_id: "s".into(),
                    parent_id: None,
                    title: "t".into(),
                    markdown: None,
                },
                "create_page",
            ),
            (
                ToolCall::AppendToPage {
                    page_id: "p".into(),
                    markdown: "m".into(),
                },
                "append_to_page",
            ),
        ] {
            assert_eq!(call.tool_name(), name);
        }
    }
}
