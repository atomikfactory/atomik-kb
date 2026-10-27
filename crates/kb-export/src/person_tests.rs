//! People in the exports (ADR-0049, M13 slice 2): a `personMention` is `@label` and a Person
//! property is the names of its people, in the Markdown file (front matter and body), the CSV of a
//! view and the sidecar's schema. The account ids never leave the workspace, and comments are not
//! exported at all (ADR-0048). The HTML flavour is tested with the others, in `html_tests`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use kb_types::{AccountId, PropertyDef, PropertyId, PropertyKind, PropertySchema};
use serde_json::{Value, json};
use yaml_rust2::{Yaml, YamlLoader};

use crate::csv::csv;
use crate::markdown::page_markdown;
use crate::sidecar::sidecar;
use crate::test_support::TestLinks;
use crate::value::{ExportProperty, ExportValue, PageMeta};

fn account(n: u8) -> String {
    AccountId::from_random_bytes([n; 16]).to_string()
}

fn mention(n: u8, label: &str) -> Value {
    json!({ "type": "personMention", "attrs": { "accountId": account(n), "label": label } })
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn paragraph(nodes: Value) -> Value {
    json!({ "type": "paragraph", "content": nodes })
}

fn meta(properties: Vec<ExportProperty>) -> PageMeta {
    PageMeta {
        title: "Standup".to_owned(),
        // 2025-10-09T08:53:20Z and 2025-10-12T09:30:00Z.
        created_at: 1_760_000_000_000,
        updated_at: 1_760_261_400_999,
        properties,
        ..PageMeta::default()
    }
}

/// A Person property's value as the core hands it over: the names, in order.
fn people(name: &str, names: &[&str]) -> ExportProperty {
    ExportProperty {
        name: name.to_owned(),
        tags: false,
        value: ExportValue::Options(names.iter().map(|n| (*n).to_owned()).collect()),
    }
}

const FRONT: &str = "---\n\
                     title: \"Standup\"\n\
                     created: \"2025-10-09T08:53:20Z\"\n\
                     updated: \"2025-10-12T09:30:00Z\"\n";

/// The front matter parsed back by a YAML parser.
fn parsed(markdown: &str) -> Yaml {
    let inner = markdown
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("---\n"))
        .unwrap()
        .0;
    YamlLoader::load_from_str(inner).unwrap().remove(0)
}

// ----- the Markdown file ----------------------------------------------------------------------

#[test]
fn person_values_are_names_in_the_front_matter() {
    let page = meta(vec![
        people("Owner", &["Ann Lee", "Bo"]),
        people("Reviewer", &["Cy"]),
        people("Backup", &["Former member"]),
        // No people, no key.
        people("Nobody", &[]),
    ]);
    let out = page_markdown(&page, &json!({ "type": "doc" }), &TestLinks::default());
    assert_eq!(
        out,
        format!(
            "{FRONT}\"Owner\":\n  - \"Ann Lee\"\n  - \"Bo\"\n\"Reviewer\": \"Cy\"\n\"Backup\": \"Former member\"\n---\n"
        )
    );
    let yaml = parsed(&out);
    assert_eq!(yaml["Owner"][0].as_str(), Some("Ann Lee"));
    assert_eq!(yaml["Owner"][1].as_str(), Some("Bo"));
    assert_eq!(yaml["Reviewer"].as_str(), Some("Cy"));
    assert!(yaml["Nobody"].is_badvalue());
}

#[test]
fn names_that_read_as_yaml_stay_names() {
    // A display name is whatever a member typed: it must not become structure, a number, a
    // boolean, a comment or a second key.
    let names = [
        "Ann: the \"Great\"",
        "# not a comment",
        "- dash",
        "yes",
        "null",
        "0x1F",
        "1e3",
        "[x]",
        "{a: b}",
        "key: value\nother: value",
        "tab\there",
        "line\u{2028}separator",
        "emoji 👩‍💻 日本語 café",
        "@Ann",
        "[[Page]]",
        "---",
    ];
    let page = meta(vec![people("Owner", &names), people("Other", &["x"])]);
    let out = page_markdown(&page, &json!({ "type": "doc" }), &TestLinks::default());
    let yaml = parsed(&out);
    let read: Vec<&str> = yaml["Owner"]
        .as_vec()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect();
    assert_eq!(read, names);
    assert_eq!(yaml["Other"].as_str(), Some("x"));
    // The front matter still ends where it should.
    assert!(out.ends_with("\n---\n"));
    assert_eq!(out.matches("\n---\n").count(), 1, "{out}");
}

#[test]
fn a_person_mention_is_its_label_in_the_body() {
    let content = json!({ "type": "doc", "content": [
        { "type": "heading", "attrs": { "level": 2 }, "content": [text("Review by "), mention(1, "Ann Lee")] },
        paragraph(json!([text("Assigned to "), mention(1, "Ann Lee"), text(", cc "), mention(2, "Bo"), text(".")])),
        { "type": "bulletList", "content": [
            { "type": "listItem", "content": [paragraph(json!([mention(3, "Cy"), text(" owns it")]))] },
        ] },
        paragraph(json!([mention(4, "")])),
    ] });
    let out = page_markdown(&meta(vec![]), &content, &TestLinks::sample());
    assert_eq!(
        out,
        format!(
            "{FRONT}---\n\n\
             ## Review by @Ann Lee\n\n\
             Assigned to @Ann Lee, cc @Bo.\n\n\
             - @Cy owns it\n"
        )
    );
    // The account ids stay in the workspace.
    for n in 1..=4 {
        assert!(!out.contains(&account(n)), "{out}");
    }
}

// ----- the CSV of a view ----------------------------------------------------------------------

#[test]
fn a_csv_cell_lists_the_names_of_the_people() {
    let header = vec!["Name".to_owned(), "Owner".to_owned()];
    let rows = vec![
        vec![
            Some(ExportValue::Text("Alpha".into())),
            Some(ExportValue::Options(vec!["Ann Lee".into(), "Bo".into()])),
        ],
        vec![
            Some(ExportValue::Text("Beta".into())),
            Some(ExportValue::Options(vec!["Former member".into()])),
        ],
        vec![
            Some(ExportValue::Text("Gamma".into())),
            Some(ExportValue::Options(vec![])),
        ],
        vec![Some(ExportValue::Text("Delta".into())), None],
    ];
    let out = csv(&header, &rows, 0);
    assert_eq!(
        out.strip_prefix('\u{feff}').unwrap(),
        "Name,Owner\r\nAlpha,\"Ann Lee, Bo\"\r\nBeta,Former member\r\nGamma,\r\nDelta,\r\n"
    );
}

#[test]
fn a_name_cannot_make_a_csv_cell_a_formula() {
    // A display name is self-asserted: the person who typed `=HYPERLINK(...)` as theirs must not
    // get it run in the exporter's spreadsheet.
    let header = vec!["Owner".to_owned()];
    for (names, cell) in [
        (
            vec!["=HYPERLINK(\"http://evil.example\")"],
            "\"'=HYPERLINK(\"\"http://evil.example\"\")\"",
        ),
        (vec!["+1", "Bo"], "\"'+1, Bo\""),
        (vec!["-2"], "'-2"),
        (vec!["@SUM(A1)"], "'@SUM(A1)"),
        (vec!["Ann", "=1+1"], "\"Ann, =1+1\""),
    ] {
        let rows = vec![vec![Some(ExportValue::Options(
            names.iter().map(|n| (*n).to_owned()).collect(),
        ))]];
        let out = csv(&header, &rows, 0);
        assert_eq!(
            out.strip_prefix('\u{feff}').unwrap(),
            format!("Owner\r\n{cell}\r\n"),
            "{names:?}"
        );
    }
}

// ----- the sidecar ----------------------------------------------------------------------------

fn def(n: u8, name: &str, kind: PropertyKind) -> PropertyDef {
    PropertyDef {
        id: PropertyId::from_random_bytes([n; 16]),
        name: name.to_owned(),
        kind,
        options: vec![],
        created_at: 5,
        config: None,
    }
}

#[test]
fn the_sidecar_keeps_a_person_property_by_its_kind() {
    let schema = PropertySchema {
        properties: vec![
            def(1, "Owner", PropertyKind::Person),
            def(2, "Estimate", PropertyKind::Number),
            def(3, "Reviewer", PropertyKind::Person),
        ],
    };
    let text = sidecar(
        "Projects",
        1_760_261_400_000,
        &schema,
        &["Projects.md".to_owned()],
    );
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["properties"],
        json!([
            { "name": "Owner", "kind": "person" },
            { "name": "Estimate", "kind": "number" },
            { "name": "Reviewer", "kind": "person" },
        ])
    );
    // No account id, no member, no option: a person kind has nothing but its name.
    assert!(!text.contains("accountId"));
    assert!(!text.contains("options"));
    assert!(!text.contains("config"));
}
