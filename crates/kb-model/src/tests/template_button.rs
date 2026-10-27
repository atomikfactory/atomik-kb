//! The template button (`templateButton`, ADR-0056, `docs/architecture/templates.md` section 9) and the `relative`
//! attribute of the date and person mentions: what the readers of a page document do with the button's content
//! (template material: no text, no links, no date or person mentions, no first image), what still counts (ids,
//! attachment references, "not empty"), what `check_invariants` reports, and the Markdown of the button in both flavours.

use std::collections::HashSet;

use kb_doc::{Doc, DocKind};
use kb_types::{AccountId, AttachmentId, BlockId, PageId};
use serde_json::{Value, json};

use crate::{Inline, PageDoc, schema, shape};

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn s(id: BlockId) -> String {
    id.to_string()
}

fn page_id(n: u8) -> PageId {
    PageId::from_random_bytes([0x70, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn aid(n: u8) -> AttachmentId {
    AttachmentId::from_random_bytes([0xa0, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn account(n: u8) -> AccountId {
    AccountId::from_random_bytes([0xcc, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn page(content: Value) -> PageDoc {
    let json = json!({ "type": "doc", "content": content });
    PageDoc::from_portable(Doc::new(DocKind::Page, 3), &json).unwrap()
}

fn text(words: &str) -> Value {
    json!({ "type": "text", "text": words })
}

fn para(n: u8, content: Vec<Value>) -> Value {
    json!({ "type": "paragraph", "attrs": { "id": s(bid(n)) }, "content": content })
}

fn reference(n: u8, target: u8) -> Value {
    json!({ "type": "pageRef", "attrs": { "id": s(bid(n)), "pageId": page_id(target).to_string() } })
}

fn date(n: u8, day: &str, extra: Value) -> Value {
    let mut attrs = json!({ "id": s(bid(n)), "date": day });
    if let (Some(attrs), Some(extra)) = (attrs.as_object_mut(), extra.as_object()) {
        attrs.extend(extra.clone());
    }
    json!({ "type": "dateMention", "attrs": attrs })
}

fn person(n: u8, label: &str, extra: Value) -> Value {
    let mut attrs = json!({ "accountId": account(n).to_string(), "label": label });
    if let (Some(attrs), Some(extra)) = (attrs.as_object_mut(), extra.as_object()) {
        attrs.extend(extra.clone());
    }
    json!({ "type": "personMention", "attrs": attrs })
}

fn button(n: u8, attrs: Value, content: Vec<Value>) -> Value {
    let mut all = json!({ "id": s(bid(n)) });
    if let (Some(all), Some(attrs)) = (all.as_object_mut(), attrs.as_object()) {
        all.extend(attrs.clone());
    }
    json!({ "type": "templateButton", "attrs": all, "content": content })
}

fn image(n: u8, attachment: u8, alt: &str, caption: &str) -> Value {
    json!({ "type": "image", "attrs": {
        "id": s(bid(n)), "attachmentId": aid(attachment).to_string(), "alt": alt, "caption": caption } })
}

fn page_link(n: u8, target: u8) -> Value {
    json!({ "type": "pageLink", "attrs": { "id": s(bid(n)), "pageId": page_id(target).to_string() } })
}

fn assert_sound(page: &PageDoc) {
    let problems = page.check_invariants();
    assert!(problems.is_empty(), "invariant violations: {problems:#?}");
}

/// `Before`, a button holding blocks of every kind that has text, a reference, a date, a person or a picture (and
/// a button inside it), then `After`, with a link, a reminder and a picture outside the button as well.
fn sample(first_picture_outside: bool) -> PageDoc {
    let inside = vec![
        para(
            11,
            vec![
                text("Inside "),
                reference(12, 2),
                text(" due "),
                date(
                    13,
                    "2026-10-08",
                    json!({ "reminder": 15, "relative": "today" }),
                ),
                text(" for "),
                person(1, "Ada", json!({ "relative": "me" })),
            ],
        ),
        page_link(14, 3),
        image(15, 1, "Inside alt", "Inside caption"),
        button(16, json!({}), vec![para(17, vec![text("Nested")])]),
    ];
    let mut content = vec![
        para(
            1,
            vec![
                text("Before "),
                reference(2, 1),
                text(" "),
                date(3, "2026-10-07", json!({ "reminder": 60 })),
            ],
        ),
        button(
            10,
            json!({ "label": "Add task", "position": "above" }),
            inside,
        ),
        para(20, vec![text("After")]),
    ];
    if first_picture_outside {
        content.push(image(21, 2, "", "Outside"));
    }
    page(json!(content))
}

#[test]
fn the_schema_names_the_button() {
    assert!(schema::NODE_NAMES.contains(&"templateButton"));
    assert!(schema::ID_BEARING.contains(&"templateButton"));
    assert_eq!(shape::TEMPLATE_BUTTON, "templateButton");
    assert_eq!(shape::POSITION, "position");
    assert_eq!(shape::RELATIVE, "relative");
    assert!(
        sample(false)
            .unknown_node_names(schema::NODE_NAMES)
            .is_empty()
    );
}

#[test]
fn the_attributes_of_a_button_and_a_relative_mention_round_trip() {
    let input = json!({ "type": "doc", "content": [
        button(10, json!({ "label": "Add task", "position": "above" }), vec![para(
            11,
            vec![
                date(12, "2026-10-18", json!({ "time": "09:30", "relative": "now" })),
                person(1, "Ada", json!({ "relative": "me" })),
            ],
        )]),
    ] });
    let page = PageDoc::from_portable(Doc::new(DocKind::Page, 3), &input).unwrap();
    assert_eq!(page.to_portable(), input);
    assert_sound(&page);
}

#[test]
fn the_content_of_a_button_is_not_the_pages_text_links_or_dates() {
    let page = sample(false);
    assert_sound(&page);

    // The blocks are listed (they have ids), the button's content with no text of any kind.
    let blocks = page.blocks();
    assert_eq!(
        blocks.iter().map(|b| b.kind.as_str()).collect::<Vec<_>>(),
        [
            "paragraph",
            "pageRef",
            "dateMention",
            "templateButton",
            "paragraph",
            "pageRef",
            "dateMention",
            "pageLink",
            "image",
            "templateButton",
            "paragraph",
            "paragraph"
        ]
    );
    let inside: Vec<BlockId> = [11, 12, 13, 14, 15, 16, 17].map(bid).to_vec();
    for block in blocks.iter().filter(|b| inside.contains(&b.id)) {
        assert!(
            block.inlines.is_empty(),
            "{} inside a button has no text",
            block.kind
        );
    }
    let outside = |n: u8| blocks.iter().find(|b| b.id == bid(n)).unwrap();
    assert_eq!(
        outside(1).inlines,
        vec![
            Inline::Text("Before ".into()),
            Inline::PageRef {
                page_id: page_id(1),
                block_id: None
            },
            Inline::Text(" 2026-10-07".into())
        ]
    );
    assert_eq!(outside(20).inlines, vec![Inline::Text("After".into())]);
    assert!(outside(10).inlines.is_empty());
    // The ids of what is inside a button, at any depth (not the button itself), for the one text that is not an inline.
    assert_eq!(
        page.template_blocks(),
        inside.iter().copied().collect::<HashSet<_>>()
    );
    // `block_ids` is still the ids of `blocks`: the content is in the document, only its text is not.
    assert_eq!(
        page.block_ids(),
        blocks.iter().map(|b| b.id).collect::<Vec<_>>()
    );

    // No backlink from inside: only the reference of the first paragraph is a link.
    let links = page.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].from_block, bid(1));
    assert_eq!(links[0].to_page, page_id(1));

    // No reminder from inside, and nobody is mentioned.
    let dates = page.date_mentions();
    assert_eq!(dates.len(), 1);
    assert_eq!(dates[0].date, "2026-10-07");
    assert_eq!(dates[0].reminder_minutes, Some(60));
    assert!(page.person_mentions().is_empty());

    // The picture inside is no first image, but the document still references its attachment.
    assert_eq!(page.first_image(), None);
    assert_eq!(page.attachment_refs(), vec![(bid(15), aid(1))]);
}

#[test]
fn the_first_image_is_the_first_one_outside_the_button() {
    let page = sample(true);
    assert_eq!(page.first_image(), Some(aid(2)));
    assert_eq!(
        page.attachment_refs(),
        vec![(bid(15), aid(1)), (bid(21), aid(2))]
    );
}

#[test]
fn a_person_mention_outside_the_button_is_listed() {
    let page = page(json!([
        para(1, vec![person(1, "Ada", json!({}))]),
        button(
            10,
            json!({}),
            vec![para(
                11,
                vec![person(2, "Grace", json!({ "relative": "me" }))]
            )]
        ),
    ]));
    let mentions = page.person_mentions();
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].account_id, account(1));
    assert_eq!(mentions[0].block_id, bid(1));
}

#[test]
fn a_page_with_a_button_is_not_empty() {
    let lone = page(json!([button(10, json!({}), vec![para(11, vec![])])]));
    assert!(!lone.is_empty());
    assert_sound(&lone);
    let blank = page(json!([para(1, vec![])]));
    assert!(blank.is_empty());
}

#[test]
fn check_invariants_reports_a_malformed_relative_and_position() {
    let page = page(json!([
        button(
            10,
            json!({ "position": "middle" }),
            vec![para(
                11,
                vec![
                    date(12, "2026-10-18", json!({ "relative": "yesterday" })),
                    date(13, "2026-10-18", json!({ "relative": "today" })),
                    date(14, "2026-10-18", json!({ "relative": "now" })),
                    person(1, "Ada", json!({ "relative": "you" })),
                    person(2, "Grace", json!({ "relative": "me" })),
                ],
            )]
        ),
        button(20, json!({ "position": "below" }), vec![para(21, vec![])]),
    ]));
    let problems = page.check_invariants();
    assert_eq!(problems.len(), 3, "{problems:#?}");
    assert!(
        problems
            .iter()
            .any(|p| p.contains("templateButton with a malformed position"))
    );
    assert_eq!(
        problems
            .iter()
            .filter(|p| p.contains("dateMention with a malformed relative"))
            .count(),
        1
    );
    assert_eq!(
        problems
            .iter()
            .filter(|p| p.contains("personMention with a malformed relative"))
            .count(),
        1
    );
    // Messages hold positions and names, never text.
    assert!(problems.iter().all(|p| !p.contains("Ada")));
}

// ----- Markdown (clipboard flavour) -----------------------------------------------------------------

#[test]
fn the_clipboard_markdown_writes_nothing_for_a_button() {
    let page = sample(false);
    let markdown = page.to_markdown(&|_| None);
    assert!(!markdown.contains("Inside"), "{markdown}");
    assert!(!markdown.contains("Nested"), "{markdown}");
    assert!(!markdown.contains("Add task"), "{markdown}");
    assert!(!markdown.contains("Ada"), "{markdown}");
    assert!(markdown.starts_with("Before "), "{markdown}");
    // The button is an empty block like the derived ones: its place in the join, and nothing else.
    assert!(markdown.ends_with("\n\nAfter\n"), "{markdown}");
    assert_eq!(markdown.matches("After").count(), 1);
}

#[test]
fn a_relative_date_mention_reads_as_its_stored_date_in_the_clipboard_markdown() {
    let page = page(json!([para(
        1,
        vec![
            text("Due "),
            date(
                2,
                "2026-10-18",
                json!({ "time": "09:30", "relative": "now" })
            ),
            text(" by "),
            person(1, "Ada", json!({ "relative": "me" })),
        ]
    )]));
    assert_eq!(
        page.to_markdown(&|_| None),
        "Due 2026-10-18 09:30 by @Ada\n"
    );
}
