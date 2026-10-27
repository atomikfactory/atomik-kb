//! Unit tests of the reminder attribute of the date mention (ADR-0016, amendment of
//! 2026-10-07 for reminders; ADR-0024): the invariant, the derived rows and the Markdown that
//! does not show it.

use kb_doc::{Doc, DocKind};
use kb_types::BlockId;
use serde_json::{Value, json};

use crate::{DateMention, Inline, PageDoc};

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn s(id: BlockId) -> String {
    id.to_string()
}

fn from_json(json: &Value) -> PageDoc {
    PageDoc::from_portable(Doc::new(DocKind::Page, 3), json).unwrap()
}

/// A `dateMention` with an id and the given extra attributes.
fn mention(id: u8, attrs: &Value) -> Value {
    let mut all = json!({ "id": s(bid(id)) });
    if let (Some(all), Some(extra)) = (all.as_object_mut(), attrs.as_object()) {
        all.extend(extra.clone());
    }
    json!({ "type": "dateMention", "attrs": all })
}

fn paragraph(id: u8, content: Vec<Value>) -> Value {
    json!({ "type": "paragraph", "attrs": { "id": s(bid(id)) }, "content": content })
}

fn doc(content: Vec<Value>) -> Value {
    json!({ "type": "doc", "content": content })
}

fn text(value: &str) -> Value {
    json!({ "type": "text", "text": value })
}

/// 2026-10-07 14:30 on the wall clock, in seconds since 1970-01-01T00:00:00.
const OCT_7_1430: i64 = 1_791_383_400;

fn minutes_of(page: &PageDoc) -> Vec<Option<i64>> {
    page.date_mentions()
        .iter()
        .map(|mention| mention.reminder_minutes)
        .collect()
}

#[test]
fn a_reminder_is_a_non_negative_integer_number_of_minutes() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            mention(
                2,
                &json!({ "date": "2026-10-07", "time": "14:30", "reminder": 0 }),
            ),
            mention(3, &json!({ "date": "2026-10-07", "reminder": 15 })),
            mention(4, &json!({ "date": "2026-10-07", "reminder": 2880 })),
            mention(5, &json!({ "date": "2026-10-07", "reminder": 90_000_000 })),
            mention(6, &json!({ "date": "2026-10-07" })),
        ],
    )]));
    assert_eq!(page.check_invariants(), Vec::<String>::new());
    assert_eq!(
        minutes_of(&page),
        [Some(0), Some(15), Some(2880), Some(90_000_000), None]
    );
}

#[test]
fn check_invariants_rejects_a_negative_or_non_integer_reminder() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            mention(2, &json!({ "date": "2026-10-07", "reminder": -1 })),
            mention(3, &json!({ "date": "2026-10-07", "reminder": 1.5 })),
            mention(4, &json!({ "date": "2026-10-07", "reminder": "15" })),
            mention(5, &json!({ "date": "2026-10-07", "reminder": true })),
            mention(6, &json!({ "date": "2026-10-07", "reminder": 15 })),
            // A whole number written as a float is the same number (JSON has one number type).
            mention(7, &json!({ "date": "2026-10-07", "reminder": 30.0 })),
        ],
    )]));
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /0/0: dateMention with a malformed reminder".to_owned(),
            "node /0/1: dateMention with a malformed reminder".to_owned(),
            "node /0/2: dateMention with a malformed reminder".to_owned(),
            "node /0/3: dateMention with a malformed reminder".to_owned(),
        ]
    );
    // Readers are lenient: a damaged reminder reads as none, the others as they are.
    assert_eq!(
        minutes_of(&page),
        [None, None, None, None, Some(15), Some(30)]
    );
}

#[test]
fn the_reminder_is_not_text_and_not_markdown() {
    let with = from_json(&doc(vec![paragraph(
        1,
        vec![
            text("Call on "),
            mention(
                2,
                &json!({ "date": "2026-10-07", "time": "14:30", "reminder": 15 }),
            ),
            text("."),
        ],
    )]));
    let without = from_json(&doc(vec![paragraph(
        1,
        vec![
            text("Call on "),
            mention(2, &json!({ "date": "2026-10-07", "time": "14:30" })),
            text("."),
        ],
    )]));
    assert_eq!(with.blocks(), without.blocks());
    assert_eq!(
        with.blocks()
            .iter()
            .find(|block| block.id == bid(1))
            .map(|block| &block.inlines),
        Some(&vec![Inline::Text("Call on 2026-10-07 14:30.".into())])
    );
    assert_eq!(with.to_markdown(&|_| None), without.to_markdown(&|_| None));
    assert_eq!(with.to_markdown(&|_| None), "Call on 2026-10-07 14:30.\n");
}

#[test]
fn a_reminder_survives_the_portable_round_trip() {
    let json = doc(vec![paragraph(
        1,
        vec![mention(
            2,
            &json!({ "date": "2026-10-07", "time": "14:30", "reminder": 1440 }),
        )],
    )]);
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
}

#[test]
fn date_mentions_list_each_mention_with_its_block_and_its_fire_time() {
    let page = from_json(&doc(vec![
        paragraph(
            1,
            vec![
                text("a "),
                mention(
                    2,
                    &json!({ "date": "2026-10-07", "time": "14:30", "reminder": 15 }),
                ),
                text(" b "),
                mention(3, &json!({ "date": "2026-10-07" })),
            ],
        ),
        json!({
            "type": "bulletList",
            "attrs": { "id": s(bid(10)) },
            "content": [{
                "type": "listItem",
                "attrs": { "id": s(bid(11)) },
                "content": [paragraph(
                    12,
                    vec![mention(13, &json!({ "date": "2026-10-08", "reminder": 60 }))],
                )]
            }]
        }),
    ]));
    assert_eq!(
        page.date_mentions(),
        vec![
            DateMention {
                block_id: bid(1),
                node_id: bid(2),
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
                reminder_minutes: Some(15),
                fire_at_local: Some(OCT_7_1430 - 15 * 60),
                reminder_for: None,
            },
            DateMention {
                block_id: bid(1),
                node_id: bid(3),
                date: "2026-10-07".into(),
                time: None,
                reminder_minutes: None,
                fire_at_local: None,
                reminder_for: None,
            },
            DateMention {
                block_id: bid(12),
                node_id: bid(13),
                date: "2026-10-08".into(),
                time: None,
                reminder_minutes: Some(60),
                // 2026-10-08 at 09:00, minus an hour: 08:00, which is 17 hours and 30 minutes
                // after 2026-10-07 14:30.
                fire_at_local: Some(OCT_7_1430 + (17 * 60 + 30) * 60),
                reminder_for: None,
            },
        ]
    );
}

#[test]
fn a_mention_without_a_valid_date_or_an_id_is_not_listed() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            json!({ "type": "dateMention", "attrs": { "id": s(bid(2)), "date": "2026-02-30", "reminder": 5 } }),
            json!({ "type": "dateMention", "attrs": { "date": "2026-10-07", "reminder": 5 } }),
            json!({ "type": "dateMention", "attrs": { "id": s(bid(4)), "date": "2026-10-07", "time": "25:61", "reminder": 5 } }),
        ],
    )]));
    let listed = page.date_mentions();
    assert_eq!(listed.len(), 1, "only the third has a date and an id");
    // A malformed time counts as no time: the moment is 09:00, and the reminder 5 minutes
    // before it.
    assert_eq!(listed[0].time, None);
    assert_eq!(
        listed[0].fire_at_local,
        Some(OCT_7_1430 - (5 * 60 + 30) * 60 - 5 * 60)
    );
}

#[test]
fn a_mention_that_sits_in_no_block_with_an_id_is_not_listed() {
    // A dateMention directly under the root has no enclosing block.
    let page = from_json(&doc(vec![mention(
        2,
        &json!({ "date": "2026-10-07", "reminder": 5 }),
    )]));
    assert!(page.date_mentions().is_empty());
}

#[test]
fn an_empty_page_has_no_date_mentions() {
    let page = PageDoc::new(Doc::new(DocKind::Page, 1), bid(1)).unwrap();
    assert!(page.date_mentions().is_empty());
}
