//! What an atomik-kb export becomes when it is imported again, for the people of ADR-0049 (M13
//! slice 2): a `personMention` is exported as `@label` and comes back as that text; a Person
//! property is exported as names and, with the sidecar that says `person`, comes back as a text
//! property that holds the names. Account ids are not in an export, so there is nobody to point
//! at. What comes back is stable: exporting it again writes the same file.
//!
//! The export side is `kb-export` itself (a dev-dependency), so a change to either side shows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_export::html::page_html;
use kb_export::markdown::page_markdown;
use kb_export::sidecar::sidecar;
use kb_export::{ExportLinks, ExportProperty, ExportValue, PageMeta};
use kb_types::{
    AccountId, AttachmentId, BlockId, ImportWarningCode, OptionId, PageId, PropertyDef, PropertyId,
    PropertyKind, PropertySchema,
};
use serde_json::{Value, json};

use crate::plan_tests::{file, run};
use crate::resolve_tests::{Fake, resolve_and_load};
use crate::{ImportValue, Plan};

/// An export that links to nothing.
struct NoLinks;

impl ExportLinks for NoLinks {
    fn page_href(&self, _: PageId) -> Option<String> {
        None
    }
    fn page_title(&self, _: PageId) -> Option<String> {
        None
    }
    fn attachment_href(&self, _: AttachmentId) -> Option<String> {
        None
    }
    fn attachment_name(&self, _: AttachmentId) -> Option<String> {
        None
    }
    fn whiteboard_href(&self, _: BlockId) -> Option<String> {
        None
    }
    fn property_name(&self, _: PropertyId) -> Option<String> {
        None
    }
    fn option_name(&self, _: PropertyId, _: OptionId) -> Option<String> {
        None
    }
}

fn account(n: u8) -> String {
    AccountId::from_random_bytes([n; 16]).to_string()
}

fn mention(n: u8, label: &str) -> Value {
    json!({ "type": "personMention", "attrs": { "accountId": account(n), "label": label } })
}

fn text(text: &str) -> Value {
    json!({ "type": "text", "text": text })
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

/// A page that mentions a person in a paragraph, a list item and a heading.
fn content(label: &str) -> Value {
    json!({ "type": "doc", "content": [
        { "type": "heading", "attrs": { "level": 2 }, "content": [text("Review by "), mention(1, label)] },
        { "type": "paragraph", "content": [text("Assigned to "), mention(1, label), text(" today, cc "), mention(2, "Bo")] },
        { "type": "bulletList", "content": [
            { "type": "listItem", "content": [{ "type": "paragraph", "content": [mention(3, label), text(" owns it")] }] },
        ] },
    ] })
}

/// The text of a node and the types of every node under it.
fn read(node: &Value, text: &mut String, types: &mut Vec<String>) {
    let kind = node["type"].as_str().unwrap_or_default();
    types.push(kind.to_owned());
    if kind == "text" {
        text.push_str(node["text"].as_str().unwrap_or_default());
    }
    for child in node["content"].as_array().into_iter().flatten() {
        read(child, text, types);
    }
}

/// The text of the paragraph blocks of a planned page, one string per paragraph.
fn paragraphs(plan: &Plan, page: usize) -> Vec<String> {
    plan.pages[page].content["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "paragraph")
        .map(|block| {
            let (mut text, mut types) = (String::new(), Vec::new());
            read(block, &mut text, &mut types);
            assert!(
                types
                    .iter()
                    .all(|kind| kind == "paragraph" || kind == "text"),
                "{types:?}"
            );
            text
        })
        .collect()
}

/// Names a member can give themselves: whatever they hold is text on the way back.
const LABELS: [&str; 14] = [
    "Ann Lee",
    "Ann *the* Great",
    "a_b_c",
    "`Ann`",
    "[Ann](http://x.example)",
    "<b>Ann</b>",
    "<script>alert(1)</script>",
    "<!-- atomik:{\"node\":\"tableOfContents\"} -->",
    "Ann & Co &amp; &copy;",
    "$Ann$ and $$Bo$$",
    "a|b",
    "ends with a backslash \\",
    "# Ann #",
    "emoji 👩‍💻 日本語 café",
];

// ----- mentions ---------------------------------------------------------------------------------

#[test]
fn a_person_mention_comes_back_from_markdown_as_its_text() {
    for label in LABELS {
        let first = page_markdown(&meta(vec![]), &content(label), &NoLinks);
        assert!(!first.contains("accountId") && !first.contains(&account(1)));
        let plan = run(vec![file("Standup.md", &first)], &[], None);
        assert_eq!(plan.pages.len(), 1);
        assert!(plan.warnings.is_empty(), "{label:?}: {:?}", plan.warnings);
        // Plain text, not a mention: there is no account to point at.
        let json = serde_json::to_string(&plan.pages[0].content).unwrap();
        assert!(!json.contains("personMention"), "{label:?}");
        assert_eq!(
            paragraphs(&plan, 0)[0],
            format!("Assigned to @{label} today, cc @Bo"),
            "{label:?} wrote {first:?}"
        );
    }
}

#[test]
fn a_template_and_its_placeholders_come_back_from_markdown_and_export_the_same_file() {
    use kb_types::TemplatePlaceholder;

    let template = PageMeta {
        template: true,
        template_placeholders: vec![
            ("Due".to_owned(), TemplatePlaceholder::Today),
            ("Owner \"quoted\"".to_owned(), TemplatePlaceholder::Me),
            ("Seen".to_owned(), TemplatePlaceholder::Now),
        ],
        ..meta(vec![])
    };
    let first = page_markdown(&template, &content("Ann"), &NoLinks);
    assert!(
        first.contains("\ntemplate: true\ntemplatePlaceholders:\n"),
        "{first}"
    );
    let plan = run(vec![file("Standup.md", &first)], &[], None);
    assert_eq!(plan.pages.len(), 1);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    assert!(plan.pages[0].template);
    assert_eq!(
        plan.pages[0].template_placeholders,
        template.template_placeholders
    );
    // Neither key is a property of the plan.
    assert!(plan.properties.is_empty(), "{:?}", plan.properties);
    assert!(plan.pages[0].properties.is_empty());
    // Exporting what came back writes the same file.
    let imported = PageMeta {
        template: plan.pages[0].template,
        template_placeholders: plan.pages[0].template_placeholders.clone(),
        ..meta(vec![])
    };
    let (content_back, warnings, _doc) = resolve_and_load(&plan.pages[0], &Fake::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(page_markdown(&imported, &content_back, &NoLinks), first);

    // A sub-page of a template: placeholders without the flag; a page with neither.
    let inside = PageMeta {
        template_placeholders: vec![("Owner".to_owned(), TemplatePlaceholder::Me)],
        ..meta(vec![])
    };
    let text = page_markdown(&inside, &content("Ann"), &NoLinks);
    let plan = run(vec![file("Inside.md", &text)], &[], None);
    assert!(!plan.pages[0].template);
    assert_eq!(
        plan.pages[0].template_placeholders,
        vec![("Owner".to_owned(), TemplatePlaceholder::Me)]
    );
    let plain = page_markdown(&meta(vec![]), &content("Ann"), &NoLinks);
    let plan = run(vec![file("Plain.md", &plain)], &[], None);
    assert!(!plan.pages[0].template && plan.pages[0].template_placeholders.is_empty());
}

#[test]
fn what_comes_back_from_markdown_exports_the_same_file() {
    for label in LABELS {
        let first = page_markdown(&meta(vec![]), &content(label), &NoLinks);
        let plan = run(vec![file("Standup.md", &first)], &[], None);
        let (imported, warnings, _doc) = resolve_and_load(&plan.pages[0], &Fake::default());
        assert!(warnings.is_empty(), "{label:?}: {warnings:?}");
        let second = page_markdown(&meta(vec![]), &imported, &NoLinks);
        assert_eq!(second, first, "{label:?}");
        // And once more: from here on nothing moves.
        let again = run(vec![file("Standup.md", &second)], &[], None);
        let (imported, _, _) = resolve_and_load(&again.pages[0], &Fake::default());
        assert_eq!(page_markdown(&meta(vec![]), &imported, &NoLinks), first);
    }
}

#[test]
fn a_person_mention_comes_back_from_html_as_its_text() {
    for label in LABELS {
        let html = page_html(&meta(vec![]), &content(label), &NoLinks);
        assert!(html.contains("class=\"person-mention\""), "{label:?}");
        assert!(!html.contains(&account(1)));
        let plan = run(vec![file("Standup.html", &html)], &[], None);
        assert_eq!(plan.pages.len(), 1);
        assert!(plan.warnings.is_empty(), "{label:?}: {:?}", plan.warnings);
        let json = serde_json::to_string(&plan.pages[0].content).unwrap();
        assert!(!json.contains("personMention"), "{label:?}");
        assert_eq!(
            paragraphs(&plan, 0)[0],
            format!("Assigned to @{label} today, cc @Bo"),
            "{label:?}"
        );
    }
}

#[test]
fn a_mention_without_a_label_leaves_nothing_behind() {
    let content = json!({ "type": "doc", "content": [
        { "type": "paragraph", "content": [text("cc "), mention(1, ""), text("done")] },
    ] });
    for (name, export) in [
        (
            "Standup.md",
            page_markdown(&meta(vec![]), &content, &NoLinks),
        ),
        ("Standup.html", page_html(&meta(vec![]), &content, &NoLinks)),
    ] {
        let plan = run(vec![file(name, &export)], &[], None);
        assert_eq!(paragraphs(&plan, 0), ["cc done"], "{name}");
    }
}

// ----- the Person property -----------------------------------------------------------------------

fn schema(kind: PropertyKind) -> PropertySchema {
    PropertySchema {
        properties: vec![PropertyDef {
            id: PropertyId::from_random_bytes([7; 16]),
            name: "Owner".to_owned(),
            kind,
            options: vec![],
            created_at: 5,
            config: None,
        }],
    }
}

fn people(names: &[&str]) -> ExportProperty {
    ExportProperty {
        name: "Owner".to_owned(),
        tags: false,
        value: ExportValue::Options(names.iter().map(|n| (*n).to_owned()).collect()),
    }
}

fn owner(plan: &Plan, page: usize) -> Option<&ImportValue> {
    plan.pages[page]
        .properties
        .iter()
        .find(|(name, _)| name == "Owner")
        .map(|(_, value)| value)
}

/// An archive of one page with its sidecar, as an export writes it.
fn archive(kind: PropertyKind, property: ExportProperty) -> Vec<crate::SourceFile> {
    let page = page_markdown(&meta(vec![property]), &json!({ "type": "doc" }), &NoLinks);
    let side = sidecar(
        "Export",
        1_760_261_400_000,
        &schema(kind),
        &["Standup.md".to_owned()],
    );
    vec![file("Standup.md", &page), file("atomik-export.json", &side)]
}

#[test]
fn a_person_property_comes_back_as_text_with_one_warning() {
    for (names, text) in [
        (&["Ann Lee", "Bo"][..], "Ann Lee, Bo"),
        (&["Cy"][..], "Cy"),
        (&["Former member"][..], "Former member"),
    ] {
        let plan = run(archive(PropertyKind::Person, people(names)), &[], None);
        assert_eq!(plan.pages.len(), 1);
        let planned: Vec<_> = plan
            .properties
            .iter()
            .map(|p| (p.name.as_str(), p.kind))
            .collect();
        assert_eq!(planned, [("Owner", PropertyKind::Text)]);
        assert!(plan.properties[0].options.is_empty() && plan.properties[0].config.is_none());
        assert_eq!(
            owner(&plan, 0),
            Some(&ImportValue::Text(text.to_owned())),
            "{names:?}"
        );
        // The person is told once, on the sidecar, which property was read as text.
        assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
        assert_eq!(plan.warnings[0].code, ImportWarningCode::UnsupportedContent);
        assert_eq!(plan.warnings[0].path, "atomik-export.json");
        assert_eq!(
            plan.warnings[0].detail.as_deref(),
            Some("Owner (people as text)")
        );
    }
}

#[test]
fn a_person_property_nobody_filled_still_warns_once_and_is_text() {
    let page = page_markdown(&meta(vec![]), &json!({ "type": "doc" }), &NoLinks);
    let side = sidecar("Export", 0, &schema(PropertyKind::Person), &[]);
    let plan = run(
        vec![file("Standup.md", &page), file("atomik-export.json", &side)],
        &[],
        None,
    );
    assert_eq!(plan.properties.len(), 1);
    assert_eq!(plan.properties[0].kind, PropertyKind::Text);
    assert_eq!(plan.warnings.len(), 1);
    assert_eq!(owner(&plan, 0), None);
}

#[test]
fn what_comes_back_for_a_person_property_exports_the_same_file() {
    // Generation one: the names of a Person property, with the sidecar that says so.
    let plan = run(
        archive(PropertyKind::Person, people(&["Ann Lee", "Bo"])),
        &[],
        None,
    );
    let Some(ImportValue::Text(names)) = owner(&plan, 0).cloned() else {
        panic!("{:?}", plan.pages[0].properties);
    };
    // Generation two: the same page, now a text property, exported again.
    let second = archive(
        PropertyKind::Text,
        ExportProperty {
            name: "Owner".to_owned(),
            tags: false,
            value: ExportValue::Text(names.clone()),
        },
    );
    let again = run(second.clone(), &[], None);
    assert!(again.warnings.is_empty(), "{:?}", again.warnings);
    assert_eq!(again.properties[0].kind, PropertyKind::Text);
    assert_eq!(owner(&again, 0), Some(&ImportValue::Text(names.clone())));
    // A third time: nothing moves.
    let third = archive(
        PropertyKind::Text,
        ExportProperty {
            name: "Owner".to_owned(),
            tags: false,
            value: ExportValue::Text(names),
        },
    );
    assert_eq!(third, second);
}

#[test]
fn a_person_property_without_its_sidecar_is_a_list_of_names() {
    // A page of the export read on its own, or a hand-written one: the front matter says only
    // names, and the importer infers what it infers for any list of names.
    let page = page_markdown(
        &meta(vec![people(&["Ann Lee", "Bo"])]),
        &json!({ "type": "doc" }),
        &NoLinks,
    );
    let plan = run(vec![file("Standup.md", &page)], &[], None);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    assert_eq!(plan.properties[0].kind, PropertyKind::MultiSelect);
    assert_eq!(
        owner(&plan, 0),
        Some(&ImportValue::List(vec!["Ann Lee".into(), "Bo".into()]))
    );
    // One name is a scalar, as for a select.
    let page = page_markdown(
        &meta(vec![people(&["Cy"])]),
        &json!({ "type": "doc" }),
        &NoLinks,
    );
    let plan = run(vec![file("Standup.md", &page)], &[], None);
    assert_eq!(plan.properties[0].kind, PropertyKind::Text);
    assert_eq!(owner(&plan, 0), Some(&ImportValue::Text("Cy".into())));
}

#[test]
fn a_sidecar_kind_this_build_does_not_know_is_still_left_out() {
    // This importer reads `person`; a kind it does not know (a later build's) is dropped from the
    // sidecar and the values are inferred, as before.
    let page = page_markdown(
        &meta(vec![people(&["Ann Lee", "Bo"])]),
        &json!({ "type": "doc" }),
        &NoLinks,
    );
    let side =
        json!({ "properties": [{ "name": "Owner", "kind": "group" }], "pages": [] }).to_string();
    let plan = run(
        vec![file("Standup.md", &page), file("atomik-export.json", &side)],
        &[],
        None,
    );
    assert_eq!(plan.properties[0].kind, PropertyKind::MultiSelect);
    assert!(plan.warnings.is_empty());
}

// ----- template buttons (ADR-0056) --------------------------------------------------------------

/// A page with a template button above and below its content, a date mention filled in with today and one that is not.
fn button_content() -> Value {
    let date = |attrs: Value| json!({ "type": "dateMention", "attrs": attrs });
    json!({ "type": "doc", "content": [
        { "type": "paragraph", "content": [text("Before "), date(json!({ "date": "2026-10-07", "reminder": 60 }))] },
        { "type": "templateButton", "attrs": { "label": "Add *task*", "position": "above" }, "content": [
            { "type": "paragraph", "content": [
                text("Task due "),
                date(json!({ "date": "2026-10-18", "reminder": 30, "relative": "today" })),
            ] },
            { "type": "taskList", "content": [
                { "type": "taskItem", "attrs": { "checked": false }, "content": [{ "type": "paragraph", "content": [text("sub")] }] },
            ] },
        ] },
        { "type": "templateButton", "content": [{ "type": "paragraph", "content": [text("Plain")] }] },
        { "type": "paragraph", "content": [text("After")] },
    ] })
}

#[test]
fn a_template_button_comes_back_from_the_markdown_file_flavour() {
    let first = page_markdown(&meta(vec![]), &button_content(), &NoLinks);
    assert!(first.contains("\"node\":\"templateButton\""), "{first}");
    let plan = run(vec![file("Standup.md", &first)], &[], None);
    assert_eq!(plan.pages.len(), 1);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    let (imported, warnings, _doc) = resolve_and_load(&plan.pages[0], &Fake::default());
    assert!(warnings.is_empty(), "{warnings:?}");

    let blocks = imported["content"].as_array().unwrap();
    let buttons: Vec<&Value> = blocks
        .iter()
        .filter(|b| b["type"] == "templateButton")
        .collect();
    assert_eq!(buttons.len(), 2);
    assert_eq!(buttons[0]["attrs"]["label"], "Add *task*");
    assert_eq!(buttons[0]["attrs"]["position"], "above");
    assert_eq!(buttons[0]["content"].as_array().map(Vec::len), Some(2));
    // The date mention that waits to be filled in is still one, with its reminder.
    let mention = &buttons[0]["content"][0]["content"][1];
    assert_eq!(mention["type"], "dateMention");
    assert_eq!(mention["attrs"]["relative"], "today");
    assert_eq!(mention["attrs"]["reminder"], 30);
    assert_eq!(buttons[1]["content"][0]["content"][0]["text"], "Plain");
    assert!(buttons[1]["attrs"].get("label").is_none() || buttons[1]["attrs"]["label"] == "");

    // And what came back exports the same file.
    let second = page_markdown(&meta(vec![]), &imported, &NoLinks);
    assert_eq!(second, first);
}

#[test]
fn the_plain_exports_leave_a_template_buttons_content_out() {
    let html = page_html(&meta(vec![]), &button_content(), &NoLinks);
    assert!(!html.contains("Task due"), "{html}");
    assert!(!html.contains("Plain"), "{html}");
    assert!(html.contains("After"), "{html}");
}

// ----- the views of a page list (ADR-0054, ADR-0058) ----------------------------------------------

/// An export that names two properties (Status 2, Due 3) and one option of Status (Done 11).
struct NamedLinks;

impl ExportLinks for NamedLinks {
    fn page_href(&self, _: PageId) -> Option<String> {
        None
    }
    fn page_title(&self, _: PageId) -> Option<String> {
        None
    }
    fn attachment_href(&self, _: AttachmentId) -> Option<String> {
        None
    }
    fn attachment_name(&self, _: AttachmentId) -> Option<String> {
        None
    }
    fn whiteboard_href(&self, _: BlockId) -> Option<String> {
        None
    }
    fn property_name(&self, property: PropertyId) -> Option<String> {
        [(2u8, "Status"), (3, "Due")]
            .into_iter()
            .find(|(n, _)| PropertyId::from_random_bytes([*n; 16]) == property)
            .map(|(_, name)| name.to_owned())
    }
    fn option_name(&self, property: PropertyId, option: OptionId) -> Option<String> {
        (property == PropertyId::from_random_bytes([2; 16])
            && option == OptionId::from_random_bytes([11; 16]))
        .then(|| "Done".to_owned())
    }
}

#[test]
fn the_sorts_the_new_operators_and_the_linked_source_of_a_view_survive_an_export_and_an_import() {
    let page = |n: u8| PageId::from_random_bytes([n; 16]).to_string();
    let (status, due) = (
        PropertyId::from_random_bytes([2; 16]).to_string(),
        PropertyId::from_random_bytes([3; 16]).to_string(),
    );
    let done = OptionId::from_random_bytes([11; 16]).to_string();
    let view = json!({
        "id": "k3f9a2bc", "name": "Open", "layout": "table", "properties": [status],
        "sort": { "by": due, "dir": "desc" },
        "sorts": [
            { "by": due, "dir": "desc" },
            { "by": "created", "dir": "asc" },
            { "by": status, "dir": "asc" }
        ],
        "filter": { "op": "and", "items": [
            { "propertyId": status, "op": "lacksOption", "value": done },
            { "propertyId": due, "op": "within", "value": { "relative": "pastDays", "n": 7 } },
            { "propertyId": due, "op": "onOrAfter", "value": { "relative": "today" } },
            { "propertyId": due, "op": "notRelatesTo", "value": page(1) }
        ] }
    });
    let content = json!({ "type": "doc", "content": [{
        "type": "pageList",
        "attrs": {
            "id": BlockId::from_random_bytes([5; 16]).to_string(),
            "source": { "kind": "page", "pageId": page(7) },
            "layout": "table", "properties": [status],
            "sort": { "by": due, "dir": "desc" },
            // The mirror of the first view: its plain filters.
            "filters": view["filter"]["items"],
            "views": [view]
        }
    }] });
    let exported = page_markdown(&meta(vec![]), &content, &NamedLinks);
    assert!(exported.contains("\"sorts\":["), "{exported}");
    assert!(exported.contains("\"op\":\"lacksOption\""), "{exported}");
    assert!(!exported.contains(&status) && !exported.contains(&due) && !exported.contains(&done));

    let plan = run(vec![file("Standup.md", &exported)], &[], None);
    assert_eq!(plan.pages.len(), 1);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    let mut resolver = Fake::default();
    resolver
        .properties
        .insert("status".into(), PropertyId::from_random_bytes([2; 16]));
    resolver
        .properties
        .insert("due".into(), PropertyId::from_random_bytes([3; 16]));
    resolver.options.insert(
        ("status".into(), "Done".into()),
        OptionId::from_random_bytes([11; 16]),
    );
    resolver
        .existing_pages
        .insert(PageId::from_random_bytes([1; 16]));
    let (imported, warnings, _doc) = resolve_and_load(&plan.pages[0], &resolver);
    assert!(warnings.is_empty(), "{warnings:?}");

    let attrs = &imported["content"][0]["attrs"];
    assert_eq!(attrs["views"][0], view, "the view comes back as it was");
    assert_eq!(
        attrs["source"],
        json!({ "kind": "page", "pageId": page(7) }),
        "the source page is kept, existing in the target or not"
    );
    assert_eq!(
        attrs["sort"], view["sort"],
        "the block mirrors the first view"
    );
    // And what came back exports the same file.
    let again = page_markdown(&meta(vec![]), &imported, &NamedLinks);
    assert_eq!(again, exported);
}
