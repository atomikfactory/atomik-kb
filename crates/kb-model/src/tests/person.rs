//! The person mention (`personMention { accountId, label }`, ADR-0049), the `reminderFor` attribute
//! of the date mention (ADR-0047) and the Person property kind (ADR-0049): what the model reads,
//! derives, renders and refuses. The editor schema parity and the shared golden are in
//! `tests/page_golden.rs`; the file flavour of the Markdown has its case in `markdown_file.rs`.

use std::collections::BTreeSet;

use kb_doc::{Doc, DocKind};
use kb_types::properties::MAX_PEOPLE;
use kb_types::{
    AccountId, BlockId, PropertyFilter, PropertyId, PropertyKind, PropertyValue, RollupFunction,
};
use serde_json::{Value, json};

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{
    BlockText, DateMention, Inline, ModelError, PageDoc, PersonMention, normalise_value,
    person_filter_matches, person_text,
};

fn bid(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

fn s(id: BlockId) -> String {
    id.to_string()
}

fn account(n: u8) -> AccountId {
    AccountId::from_bytes([0xac, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn from_json(json: &Value) -> PageDoc {
    PageDoc::from_portable(Doc::new(DocKind::Page, 3), json).unwrap()
}

fn doc(content: Vec<Value>) -> Value {
    json!({ "type": "doc", "content": content })
}

fn paragraph(id: u8, content: Vec<Value>) -> Value {
    json!({ "type": "paragraph", "attrs": { "id": s(bid(id)) }, "content": content })
}

fn text(value: &str) -> Value {
    json!({ "type": "text", "text": value })
}

/// A `personMention` of `account(n)` labelled `label`.
fn mention(n: u8, label: &str) -> Value {
    json!({
        "type": "personMention",
        "attrs": { "accountId": account(n).to_string(), "label": label }
    })
}

fn node(attrs: &Value) -> Value {
    json!({ "type": "personMention", "attrs": attrs })
}

fn inlines_of(page: &PageDoc, block: u8) -> Vec<Inline> {
    page.blocks()
        .into_iter()
        .find(|b| b.id == bid(block))
        .unwrap()
        .inlines
}

// ----- the personMention node ------------------------------------------------------------------

#[test]
fn a_person_mention_reads_as_its_label_in_the_run_around_it() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![text("Ask "), mention(7, "Ada Lovelace"), text(" about it")],
    )]));
    assert_eq!(page.check_invariants(), Vec::<String>::new());
    // One run, and the mention is no block of its own: it has no id.
    assert_eq!(
        page.blocks(),
        vec![BlockText {
            id: bid(1),
            kind: "paragraph".into(),
            inlines: vec![Inline::Text("Ask Ada Lovelace about it".into())],
        }]
    );
}

#[test]
fn a_mention_is_read_leniently() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            text("a "),
            // No label: nothing to read.
            node(&json!({ "accountId": account(1).to_string() })),
            text(" b "),
            // A label that is not text reads as nothing.
            node(&json!({ "accountId": account(2).to_string(), "label": 42 })),
            text(" c "),
            // No valid id: the label still reads (and is the chip's fallback).
            node(&json!({ "accountId": "not-an-id", "label": "Bob" })),
        ],
    )]));
    assert_eq!(
        inlines_of(&page, 1),
        vec![Inline::Text("a  b  c Bob".into())]
    );
}

#[test]
fn a_person_mention_is_content_a_page_with_only_one_is_not_empty() {
    let empty = from_json(&doc(vec![paragraph(1, vec![])]));
    assert!(empty.is_empty());
    let mentioned = from_json(&doc(vec![paragraph(1, vec![mention(1, "Ada")])]));
    assert!(!mentioned.is_empty());
}

#[test]
fn a_mention_does_not_split_the_run_and_is_no_page_reference() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![text("one "), mention(1, "Ada"), text(" two")],
    )]));
    assert_eq!(
        inlines_of(&page, 1),
        vec![Inline::Text("one Ada two".into())]
    );
    assert!(page.links().is_empty(), "no backlink");
    assert!(page.date_mentions().is_empty());
}

#[test]
fn person_mentions_lists_each_mention_with_its_block_in_document_order() {
    let page = from_json(&doc(vec![
        paragraph(
            1,
            vec![
                text("a "),
                mention(1, "Ada"),
                text(" b "),
                mention(2, "Bob"),
                mention(1, "Ada"),
            ],
        ),
        json!({
            "type": "bulletList",
            "attrs": { "id": s(bid(10)) },
            "content": [{
                "type": "listItem",
                "attrs": { "id": s(bid(11)) },
                "content": [paragraph(12, vec![mention(3, "Cy")])]
            }]
        }),
        paragraph(
            2,
            vec![
                // No valid id: left out.
                node(&json!({ "accountId": "not-an-id", "label": "Bad" })),
                node(&json!({ "label": "Missing" })),
                node(&json!({ "accountId": 5, "label": "Number" })),
                // No label: listed, with the empty label.
                node(&json!({ "accountId": account(4).to_string() })),
            ],
        ),
    ]));
    let mention_of = |block: u8, who: u8, label: &str| PersonMention {
        block_id: bid(block),
        account_id: account(who),
        label: label.to_owned(),
    };
    assert_eq!(
        page.person_mentions(),
        vec![
            mention_of(1, 1, "Ada"),
            mention_of(1, 2, "Bob"),
            // The same account twice in one block is listed twice.
            mention_of(1, 1, "Ada"),
            mention_of(12, 3, "Cy"),
            mention_of(2, 4, ""),
        ]
    );
}

#[test]
fn a_mention_outside_any_block_is_left_out_of_the_derived_rows() {
    let page = from_json(&doc(vec![mention(1, "Ada")]));
    assert!(page.person_mentions().is_empty());
}

#[test]
fn check_invariants_reports_a_mention_without_a_valid_account_id() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            mention(1, "Ada"),
            node(&json!({ "label": "No id" })),
            node(&json!({ "accountId": "nope", "label": "Malformed" })),
            node(&json!({ "accountId": 7, "label": "Not a string" })),
            // A mention has no block id of its own, and a missing label is tolerated.
            node(&json!({ "accountId": account(2).to_string() })),
        ],
    )]));
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /0/1: personMention without a valid accountId".to_owned(),
            "node /0/2: personMention without a valid accountId".to_owned(),
            "node /0/3: personMention without a valid accountId".to_owned(),
        ]
    );
}

#[test]
fn a_mention_survives_the_portable_round_trip_without_an_id() {
    let json = doc(vec![paragraph(
        1,
        vec![
            text("Ping "),
            mention(5, "Grace Hopper"),
            text(" and "),
            mention(6, "Alan"),
        ],
    )]);
    let page = from_json(&json);
    assert_eq!(page.to_portable(), json);
    let portable = page.to_portable();
    let mention_node = &portable["content"][0]["content"][1];
    assert_eq!(mention_node["type"], "personMention");
    assert!(mention_node["attrs"].get("id").is_none());
    assert_eq!(
        mention_node["attrs"]["accountId"],
        account(5).to_string().as_str()
    );
    assert!(!crate::schema::ID_BEARING.contains(&"personMention"));
}

// ----- Markdown --------------------------------------------------------------------------------

fn markdown(page: &PageDoc) -> String {
    page.to_markdown(&|_| None)
}

#[test]
fn markdown_writes_a_mention_as_at_and_its_label() {
    let page = from_json(&doc(vec![
        paragraph(
            1,
            vec![text("Ping "), mention(1, "Ada Lovelace"), text(", please.")],
        ),
        json!({
            "type": "heading",
            "attrs": { "id": s(bid(2)), "level": 2 },
            "content": [mention(2, "Bob")]
        }),
        json!({
            "type": "bulletList",
            "attrs": { "id": s(bid(3)) },
            "content": [{
                "type": "listItem",
                "attrs": { "id": s(bid(4)) },
                "content": [paragraph(5, vec![text("owner: "), mention(3, "Cy")])]
            }]
        }),
    ]));
    assert_eq!(
        markdown(&page),
        "Ping @Ada Lovelace, please.\n\n## @Bob\n\n- owner: @Cy\n"
    );
}

#[test]
fn markdown_writes_nothing_for_a_mention_without_a_label_and_never_an_account_id() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            text("a"),
            node(&json!({ "accountId": account(1).to_string() })),
            node(&json!({ "accountId": account(2).to_string(), "label": "" })),
            node(&json!({ "accountId": account(3).to_string(), "label": 9 })),
            text("b"),
        ],
    )]));
    assert_eq!(markdown(&page), "ab\n");
    let with_label = from_json(&doc(vec![paragraph(1, vec![mention(1, "Ada")])]));
    assert!(!markdown(&with_label).contains(&account(1).to_string()));
}

#[test]
fn a_label_cannot_break_the_line_it_sits_in() {
    // A name is self-asserted: a line break or a control character in it would end the paragraph
    // and could start a block of the exported file.
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![mention(1, "Ada\n# Heading\r\n- item\t!")],
    )]));
    let written = markdown(&page);
    assert_eq!(written.lines().count(), 1, "{written:?}");
    assert_eq!(written, "@Ada # Heading  - item !\n");
}

#[test]
fn a_mention_in_a_toggle_summary_and_among_blocks_is_still_inline() {
    let page = from_json(&doc(vec![json!({
        "type": "details",
        "attrs": { "id": s(bid(1)) },
        "content": [
            {
                "type": "detailsSummary",
                "attrs": { "id": s(bid(2)) },
                "content": [text("Owner "), mention(1, "Ada")]
            },
            {
                "type": "detailsContent",
                "attrs": { "id": s(bid(3)) },
                "content": [paragraph(4, vec![mention(2, "Bob")])]
            }
        ]
    })]));
    assert_eq!(
        markdown(&page),
        "<details>\n<summary>Owner @Ada</summary>\n\n@Bob\n\n</details>\n"
    );
}

#[test]
fn the_search_text_of_a_block_holds_the_label_and_never_the_account() {
    let page = from_json(&doc(vec![paragraph(1, vec![mention(1, "Ada")])]));
    for block in page.blocks() {
        for inline in block.inlines {
            if let Inline::Text(text) = inline {
                assert_eq!(text, "Ada");
            }
        }
    }
}

// ----- reminderFor (ADR-0047) -------------------------------------------------------------------

fn date_mention(id: u8, attrs: &Value) -> Value {
    let mut all = json!({ "id": s(bid(id)), "date": "2026-10-07" });
    if let (Some(all), Some(extra)) = (all.as_object_mut(), attrs.as_object()) {
        all.extend(extra.clone());
    }
    json!({ "type": "dateMention", "attrs": all })
}

fn reminders(page: &PageDoc) -> Vec<Option<AccountId>> {
    page.date_mentions()
        .iter()
        .map(|mention| mention.reminder_for)
        .collect()
}

#[test]
fn reminder_for_is_the_account_whose_devices_fire_the_reminder() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            date_mention(
                2,
                &json!({ "reminder": 15, "reminderFor": account(7).to_string() }),
            ),
            date_mention(3, &json!({ "reminder": 15 })),
            date_mention(4, &json!({ "reminderFor": account(8).to_string() })),
        ],
    )]));
    assert_eq!(page.check_invariants(), Vec::<String>::new());
    assert_eq!(
        reminders(&page),
        [Some(account(7)), None, Some(account(8))],
        "absent means the Owners' devices in a shared space; a value without a reminder is kept"
    );
    // The rest of the row is as it was.
    let first = &page.date_mentions()[0];
    assert_eq!(first.reminder_minutes, Some(15));
    assert!(first.fire_at_local.is_some());
}

#[test]
fn a_malformed_reminder_for_reads_as_none_and_is_reported() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![
            date_mention(2, &json!({ "reminder": 15, "reminderFor": "nope" })),
            date_mention(3, &json!({ "reminder": 15, "reminderFor": 5 })),
            date_mention(4, &json!({ "reminder": 15, "reminderFor": "" })),
            date_mention(
                5,
                &json!({ "reminder": 15, "reminderFor": account(1).to_string() }),
            ),
        ],
    )]));
    assert_eq!(reminders(&page), [None, None, None, Some(account(1))]);
    assert_eq!(
        page.check_invariants(),
        vec![
            "node /0/0: dateMention with a malformed reminderFor".to_owned(),
            "node /0/1: dateMention with a malformed reminderFor".to_owned(),
            "node /0/2: dateMention with a malformed reminderFor".to_owned(),
        ]
    );
}

#[test]
fn reminder_for_is_not_text_and_not_markdown_and_survives_the_round_trip() {
    let json = doc(vec![paragraph(
        1,
        vec![
            text("Call on "),
            date_mention(
                2,
                &json!({ "reminder": 60, "reminderFor": account(3).to_string() }),
            ),
            text("."),
        ],
    )]);
    let with = from_json(&json);
    assert_eq!(with.to_portable(), json);
    let without = from_json(&doc(vec![paragraph(
        1,
        vec![
            text("Call on "),
            date_mention(2, &json!({ "reminder": 60 })),
            text("."),
        ],
    )]));
    assert_eq!(with.blocks(), without.blocks());
    assert_eq!(markdown(&with), markdown(&without));
    assert!(!markdown(&with).contains(&account(3).to_string()));
}

#[test]
fn a_date_mention_row_names_its_block_node_and_account() {
    let page = from_json(&doc(vec![paragraph(
        1,
        vec![date_mention(
            2,
            &json!({ "reminder": 0, "reminderFor": account(9).to_string() }),
        )],
    )]));
    let rows = page.date_mentions();
    assert_eq!(rows.len(), 1);
    let DateMention {
        block_id,
        node_id,
        reminder_for,
        reminder_minutes,
        ..
    } = rows[0].clone();
    assert_eq!(block_id, bid(1));
    assert_eq!(node_id, bid(2));
    assert_eq!(reminder_for, Some(account(9)));
    assert_eq!(reminder_minutes, Some(0));
}

// ----- the Person property ------------------------------------------------------------------------

fn prop(n: u8) -> PropertyId {
    PropertyId::from_random_bytes([0xb9, n, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, n])
}

fn people(ns: &[u8]) -> PropertyValue {
    PropertyValue::Person(ns.iter().map(|n| account(*n)).collect())
}

fn create_person(doc: &mut crate::StructureDoc, n: u8) {
    doc.create_property(
        prop(n),
        "Assignee",
        PropertyKind::Person,
        &mut || kb_types::OptionId::from_random_bytes([1; 16]),
        10,
    )
    .unwrap();
}

fn stored(doc: &crate::StructureDoc, page: u8, property: u8) -> Option<PropertyValue> {
    doc.page_properties(pid(page))
        .unwrap()
        .into_iter()
        .find(|(id, _)| *id == prop(property))
        .map(|(_, value)| value)
}

fn field_of(error: &ModelError) -> Option<&'static str> {
    match error {
        ModelError::InvalidProperty { field, .. } => Some(field),
        _ => None,
    }
}

#[test]
fn a_person_value_is_an_ordered_list_of_accounts() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "Task");
    create_person(&mut doc, 1);
    assert_eq!(doc.property(prop(1)).unwrap().kind, PropertyKind::Person);
    assert!(doc.property(prop(1)).unwrap().config.is_none());

    doc.set_page_property(page, prop(1), Some(people(&[3, 1, 2])), 100)
        .unwrap();
    assert_eq!(stored(&doc, 1, 1), Some(people(&[3, 1, 2])), "order kept");
    assert_healthy(&doc);

    // The stored form is a list of account id strings, like a relation's pages.
    let portable = doc.to_portable();
    let props = &portable["pages"][0]["props"][prop(1).to_string()];
    assert_eq!(
        props,
        &json!([
            account(3).to_string(),
            account(1).to_string(),
            account(2).to_string()
        ])
    );

    // Clearing: `None`, or an empty list, is no value.
    doc.set_page_property(page, prop(1), Some(people(&[])), 110)
        .unwrap();
    assert_eq!(stored(&doc, 1, 1), None);
    doc.set_page_property(page, prop(1), Some(people(&[4])), 120)
        .unwrap();
    doc.set_page_property(page, prop(1), None, 130).unwrap();
    assert_eq!(stored(&doc, 1, 1), None);
}

#[test]
fn a_person_value_holds_at_most_twenty_distinct_accounts() {
    assert_eq!(MAX_PEOPLE, 20);
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "Task");
    create_person(&mut doc, 1);
    let twenty: Vec<u8> = (1..=20).collect();
    doc.set_page_property(page, prop(1), Some(people(&twenty)), 100)
        .unwrap();
    assert_eq!(stored(&doc, 1, 1), Some(people(&twenty)));

    let twenty_one: Vec<u8> = (1..=21).collect();
    let error = doc
        .set_page_property(page, prop(1), Some(people(&twenty_one)), 110)
        .unwrap_err();
    assert_eq!(field_of(&error), Some("value"));
    let error = doc
        .set_page_property(page, prop(1), Some(people(&[1, 2, 1])), 110)
        .unwrap_err();
    assert_eq!(field_of(&error), Some("value"));
    assert_eq!(
        stored(&doc, 1, 1),
        Some(people(&twenty)),
        "a refused write changes nothing"
    );
}

#[test]
fn a_person_value_belongs_to_a_person_property() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "Task");
    create_person(&mut doc, 1);
    doc.create_property(
        prop(2),
        "Notes",
        PropertyKind::Text,
        &mut || kb_types::OptionId::from_random_bytes([1; 16]),
        10,
    )
    .unwrap();
    let error = doc
        .set_page_property(page, prop(2), Some(people(&[1])), 100)
        .unwrap_err();
    assert_eq!(field_of(&error), Some("value"));
    let error = doc
        .set_page_property(page, prop(1), Some(PropertyValue::Text("Ada".into())), 100)
        .unwrap_err();
    assert_eq!(field_of(&error), Some("value"));
}

#[test]
fn normalise_value_keeps_a_list_and_turns_an_empty_one_into_no_value() {
    assert_eq!(
        normalise_value(&people(&[1, 2])).unwrap(),
        Some(people(&[1, 2]))
    );
    assert_eq!(normalise_value(&people(&[])).unwrap(), None);
    assert!(normalise_value(&people(&[1, 1])).is_err());
    let many: Vec<u8> = (0..=20).collect();
    assert!(normalise_value(&people(&many)).is_err());
}

#[test]
fn derived_rows_hold_one_row_per_account_in_order_and_no_text() {
    let mut doc = structure(1);
    let page = add(&mut doc, 1, None, "Task");
    let other = add(&mut doc, 2, None, "Other");
    create_person(&mut doc, 1);
    doc.set_page_property(page, prop(1), Some(people(&[5, 6, 5 + 10])), 100)
        .unwrap();
    let index = doc.property_index(&[page, other]);
    let rows = &index[0].1.rows;
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row.kind == PropertyKind::Person
        && row.property_id == prop(1)
        && row.value_text.is_none()
        && row.value_number.is_none()
        && row.option_id.is_none()
        && row.related_page_id.is_none()));
    assert_eq!(
        rows.iter().map(|row| row.account_id).collect::<Vec<_>>(),
        [Some(account(5)), Some(account(6)), Some(account(15))]
    );
    assert!(
        index[0].1.texts.is_empty(),
        "names are the core's: the model cannot know them"
    );
    assert!(index[1].1.rows.is_empty());
}

#[test]
fn concurrent_person_writes_converge_last_writer_wins() {
    let mut a = structure(1);
    let page = add(&mut a, 1, None, "Task");
    create_person(&mut a, 1);
    let mut b = replica(&a, 2);
    a.set_page_property(page, prop(1), Some(people(&[1, 2])), 100)
        .unwrap();
    b.set_page_property(page, prop(1), Some(people(&[3])), 101)
        .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(stored(&a, 1, 1), stored(&b, 1, 1));
    assert!(matches!(stored(&a, 1, 1), Some(PropertyValue::Person(_))));
    assert_eq!(a.to_portable(), b.to_portable());
    assert_healthy(&a);
    assert_healthy(&b);
}

#[test]
fn a_person_property_can_be_rolled_up_and_listed() {
    use kb_types::{PropertyConfig, RollupResult, RollupTarget, RollupValue};
    let mut doc = structure(1);
    for n in 1..=4 {
        add(&mut doc, n, None, &format!("Page {n}"));
    }
    create_person(&mut doc, 1);
    doc.create_property(
        prop(2),
        "Links",
        PropertyKind::Relation,
        &mut || kb_types::OptionId::from_random_bytes([1; 16]),
        10,
    )
    .unwrap();
    let functions = [
        RollupFunction::CountValues,
        RollupFunction::CountEmpty,
        RollupFunction::ShowOriginal,
    ];
    let slot = |function: RollupFunction| {
        10 + u8::try_from(functions.iter().position(|f| *f == function).unwrap()).unwrap()
    };
    for function in functions {
        let n = slot(function);
        doc.create_property_with(
            prop(n),
            "Rollup",
            PropertyKind::Rollup,
            Some(&PropertyConfig::Rollup {
                relation_property_id: prop(2),
                target: RollupTarget::Property(prop(1)),
                function,
            }),
            &mut || kb_types::OptionId::from_random_bytes([1; 16]),
            10,
        )
        .unwrap();
    }
    doc.set_page_property(
        pid(1),
        prop(2),
        Some(PropertyValue::Relation(vec![pid(2), pid(3), pid(4)])),
        100,
    )
    .unwrap();
    doc.set_page_property(pid(2), prop(1), Some(people(&[1, 2])), 100)
        .unwrap();
    doc.set_page_property(pid(3), prop(1), Some(people(&[2])), 100)
        .unwrap();
    let rolled = |function: RollupFunction| match stored(&doc, 1, slot(function)) {
        Some(PropertyValue::Rollup(RollupValue { result, .. })) => Some(result),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        rolled(RollupFunction::CountValues),
        Some(RollupResult::Number(2.0))
    );
    assert_eq!(
        rolled(RollupFunction::CountEmpty),
        Some(RollupResult::Number(1.0))
    );
    assert_eq!(
        rolled(RollupFunction::ShowOriginal),
        Some(RollupResult::Values(vec![people(&[1, 2]), people(&[2])]))
    );
}

#[test]
fn person_text_is_the_names_the_caller_knows_joined_with_a_comma() {
    let names = |who: AccountId| match who.to_bytes()[1] {
        1 => Some("Ada Lovelace".to_owned()),
        2 => Some("  ".to_owned()),
        3 => Some("Bob".to_owned()),
        _ => None,
    };
    assert_eq!(
        person_text(&[account(1), account(2), account(3), account(9)], &names),
        Some("Ada Lovelace, Bob".to_owned()),
        "blank and unknown names are left out"
    );
    assert_eq!(person_text(&[account(9)], &names), None);
    assert_eq!(person_text(&[], &names), None);
}

#[test]
fn the_person_filters_have_one_meaning() {
    let value = people(&[1, 2]);
    let me = Some(account(2));
    let matches = |filter: &PropertyFilter, value: Option<&PropertyValue>, me| {
        person_filter_matches(filter, value, me)
    };
    // HasValue.
    assert!(matches(&PropertyFilter::HasValue, Some(&value), None));
    assert!(!matches(&PropertyFilter::HasValue, None, me));
    assert!(!matches(&PropertyFilter::HasValue, Some(&people(&[])), me));
    // ContainsPerson.
    assert!(matches(
        &PropertyFilter::ContainsPerson(account(1)),
        Some(&value),
        None
    ));
    assert!(!matches(
        &PropertyFilter::ContainsPerson(account(3)),
        Some(&value),
        me
    ));
    assert!(!matches(
        &PropertyFilter::ContainsPerson(account(1)),
        None,
        me
    ));
    // ContainsMe is ContainsPerson of the viewer; nobody is "me" without an account.
    assert!(matches(&PropertyFilter::ContainsMe, Some(&value), me));
    assert!(!matches(
        &PropertyFilter::ContainsMe,
        Some(&value),
        Some(account(3))
    ));
    assert!(!matches(&PropertyFilter::ContainsMe, Some(&value), None));
    // A value of another kind never matches a person filter; any other filter is not one.
    assert!(!matches(
        &PropertyFilter::ContainsMe,
        Some(&PropertyValue::Text("x".into())),
        me
    ));
    assert!(!matches(
        &PropertyFilter::TextContains("a".into()),
        Some(&value),
        me
    ));
}

#[test]
fn every_account_of_a_value_is_listed_once_in_a_set() {
    // The rows and the filter agree: a page matches ContainsPerson exactly when a row names the account.
    let value = people(&[4, 5, 6]);
    let rows: BTreeSet<AccountId> = match &value {
        PropertyValue::Person(accounts) => accounts.iter().copied().collect(),
        _ => unreachable!(),
    };
    for n in 1..=8 {
        assert_eq!(
            person_filter_matches(
                &PropertyFilter::ContainsPerson(account(n)),
                Some(&value),
                None
            ),
            rows.contains(&account(n)),
            "account {n}"
        );
    }
}
