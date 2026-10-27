//! Golden fixtures of document format v1 (`fixtures/doc/`, written once by
//! `cargo run -p kb-model --example make_golden`).
//!
//! These tests freeze the blob envelope and the document shapes: blobs written by
//! format v1 must keep loading into the same portable JSON forever.
//!
//! **If one of these tests fails, the format changed.** Do not regenerate the
//! fixtures to make it pass: bump `kb_doc::FORMAT_VERSION` (and
//! `MIN_READER_VERSION` if old readers cannot read the new shape), keep a reader for
//! v1, and add v2 fixtures next to these (`docs/development/milestone-02.md`,
//! "Security and migration notes").
//!
//! Additive containers get fixtures of their own instead: the attachment manifests of
//! Milestone 5 (a new root map, absent from older documents) are frozen by
//! `structure-v1-attachments.*`, and the property schema and values of ADR-0026 (a new root
//! map and a `props` map in the metadata of page nodes) by `structure-v1-properties.*`, so
//! `structure-v1.*` keeps the bytes written before either existed. The view links of Milestone 13
//! slice 4 (a new root map) are frozen by `structure-v1-links.*`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/golden_script.rs"]
mod golden_script;

use kb_doc::{BlobHeader, BlobKind, Doc, DocKind, ENGINE_LORO, decode_blob};
use kb_model::{AccountDoc, StructureDoc};
use kb_types::properties::TAGS_PROPERTY_ID;
use kb_types::{PropertyKind, PropertyValue};

const STRUCTURE_SNAPSHOT: &[u8] = include_bytes!("../../../fixtures/doc/structure-v1.snapshot.bin");
const STRUCTURE_JSON: &str = include_str!("../../../fixtures/doc/structure-v1.portable.json");
const ACCOUNT_SNAPSHOT: &[u8] = include_bytes!("../../../fixtures/doc/account-v1.snapshot.bin");
const ACCOUNT_JSON: &str = include_str!("../../../fixtures/doc/account-v1.portable.json");

const V1_SNAPSHOT_HEADER: BlobHeader = BlobHeader {
    engine: ENGINE_LORO,
    format_version: 1,
    min_reader_version: 1,
    kind: BlobKind::Snapshot,
};

fn golden(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("fixture JSON parses")
}

#[test]
fn structure_v1_blob_has_the_v1_envelope() {
    let (header, payload) = decode_blob(STRUCTURE_SNAPSHOT).unwrap();
    assert_eq!(header, V1_SNAPSHOT_HEADER);
    assert_eq!(&STRUCTURE_SNAPSHOT[..4], b"KBD1");
    assert!(!payload.is_empty());
}

#[test]
fn structure_v1_snapshot_loads_into_the_golden_json() {
    let doc = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, 0xfeed, Some(STRUCTURE_SNAPSHOT), &[]).unwrap(),
    )
    .unwrap();
    assert_eq!(doc.to_portable(), golden(STRUCTURE_JSON));
    assert!(
        doc.check_invariants().is_empty(),
        "{:?}",
        doc.check_invariants()
    );
    // Same text, not only the same value: the export is byte-stable.
    let mut text = serde_json::to_string_pretty(&doc.to_portable()).unwrap();
    text.push('\n');
    assert_eq!(
        text.replace("\r\n", "\n"),
        STRUCTURE_JSON.replace("\r\n", "\n")
    );
}

#[test]
fn structure_v1_snapshot_stays_editable() {
    let mut doc = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, 0xfeed, Some(STRUCTURE_SNAPSHOT), &[]).unwrap(),
    )
    .unwrap();
    let inbox = golden_script::page_id(1);
    doc.create_page(golden_script::page_id(200), Some(inbox), Some(0), "New", 1)
        .unwrap();
    doc.move_page(golden_script::page_id(8), Some(inbox), 0, 2)
        .unwrap();
    doc.restore_page(golden_script::page_id(6), 3).unwrap();
    doc.undo().unwrap().unwrap();
    assert!(doc.check_invariants().is_empty());
    // Updates written now apply on top of the old snapshot.
    let delta = doc
        .doc()
        .export_updates_since(Some(
            &Doc::load(DocKind::Structure, 1, Some(STRUCTURE_SNAPSHOT), &[])
                .unwrap()
                .version(),
        ))
        .unwrap()
        .unwrap();
    let replay = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, 2, Some(STRUCTURE_SNAPSHOT), &[delta]).unwrap(),
    )
    .unwrap();
    assert_eq!(replay.to_portable(), doc.to_portable());
}

#[test]
fn account_v1_snapshot_loads_into_the_golden_json() {
    let (header, _) = decode_blob(ACCOUNT_SNAPSHOT).unwrap();
    assert_eq!(header, V1_SNAPSHOT_HEADER);
    let doc = AccountDoc::from_doc(
        Doc::load(DocKind::Account, 0xfeed, Some(ACCOUNT_SNAPSHOT), &[]).unwrap(),
    )
    .unwrap();
    assert_eq!(doc.to_portable(), golden(ACCOUNT_JSON));
    assert_eq!(
        doc.spaces(),
        vec![
            golden_script::space_id(3),
            golden_script::space_id(1),
            golden_script::space_id(4)
        ]
    );
}

#[test]
fn the_fixed_script_still_produces_the_golden_json() {
    // Semantics check: replaying the script with today's code gives the frozen
    // result (a failure here without a failure above means behaviour changed, not
    // the format).
    assert_eq!(
        golden_script::structure().to_portable(),
        golden(STRUCTURE_JSON)
    );
    assert_eq!(golden_script::account().to_portable(), golden(ACCOUNT_JSON));
}

// ----- attachments (Milestone 5) ----------------------------------------------------------

const ATTACHMENTS_SNAPSHOT: &[u8] =
    include_bytes!("../../../fixtures/doc/structure-v1-attachments.snapshot.bin");
const ATTACHMENTS_JSON: &str =
    include_str!("../../../fixtures/doc/structure-v1-attachments.portable.json");

fn load_structure(snapshot: &[u8]) -> StructureDoc {
    StructureDoc::from_doc(Doc::load(DocKind::Structure, 0xfeed, Some(snapshot), &[]).unwrap())
        .unwrap()
}

#[test]
fn a_structure_written_before_attachments_has_none() {
    let doc = load_structure(STRUCTURE_SNAPSHOT);
    assert!(doc.attachments().is_empty());
    assert!(doc.to_portable().get("attachments").is_none());
}

#[test]
fn structure_v1_attachments_snapshot_loads_into_the_golden_json() {
    let (header, _) = decode_blob(ATTACHMENTS_SNAPSHOT).unwrap();
    assert_eq!(header, V1_SNAPSHOT_HEADER);
    let doc = load_structure(ATTACHMENTS_SNAPSHOT);
    assert_eq!(doc.to_portable(), golden(ATTACHMENTS_JSON));
    assert!(
        doc.check_invariants().is_empty(),
        "{:?}",
        doc.check_invariants()
    );
    let mut text = serde_json::to_string_pretty(&doc.to_portable()).unwrap();
    text.push('\n');
    assert_eq!(
        text.replace("\r\n", "\n"),
        ATTACHMENTS_JSON.replace("\r\n", "\n")
    );

    // The undone page is gone, the manifests written around it stay, the removed one
    // is gone and the one from the second device is merged in.
    assert!(doc.page(golden_script::page_id(12)).is_none());
    let ids: Vec<_> = doc
        .attachments()
        .into_iter()
        .map(|manifest| manifest.attachment_id)
        .collect();
    assert_eq!(
        ids,
        [1, 2, 3, 4].map(golden_script::attachment_id).to_vec(),
        "by creation time"
    );
    let report = doc.attachment(golden_script::attachment_id(2)).unwrap();
    assert_eq!(
        report,
        golden_script::manifest(
            2,
            "Quarterly report — ünïcödé.pdf",
            "application/pdf",
            9 * 1024 * 1024,
            golden_script::T0 + 32,
            1
        )
    );
    assert_eq!(report.chunks.len(), 3);
    assert!(doc.attachment(golden_script::attachment_id(5)).is_none());
}

#[test]
fn the_fixed_script_still_produces_the_attachments_golden() {
    assert_eq!(
        golden_script::structure_with_attachments().to_portable(),
        golden(ATTACHMENTS_JSON)
    );
}

#[test]
fn structure_v1_attachments_snapshot_stays_editable() {
    let mut doc = load_structure(ATTACHMENTS_SNAPSHOT);
    doc.create_page(golden_script::page_id(201), None, None, "New", 1)
        .unwrap();
    doc.attachment_put(&golden_script::manifest(
        6,
        "new.png",
        "image/png",
        10,
        2,
        1,
    ))
    .unwrap();
    doc.attachment_remove(golden_script::attachment_id(1))
        .unwrap();
    doc.undo().unwrap().unwrap();
    assert!(doc.page(golden_script::page_id(201)).is_none());
    assert!(doc.attachment(golden_script::attachment_id(6)).is_some());
    assert!(doc.attachment(golden_script::attachment_id(1)).is_none());
    assert!(doc.check_invariants().is_empty());
    let delta = doc
        .doc()
        .export_updates_since(Some(
            &Doc::load(DocKind::Structure, 1, Some(ATTACHMENTS_SNAPSHOT), &[])
                .unwrap()
                .version(),
        ))
        .unwrap()
        .unwrap();
    let replay = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, 2, Some(ATTACHMENTS_SNAPSHOT), &[delta]).unwrap(),
    )
    .unwrap();
    assert_eq!(replay.to_portable(), doc.to_portable());
}

// ----- properties (ADR-0026) --------------------------------------------------------------

const PROPERTIES_SNAPSHOT: &[u8] =
    include_bytes!("../../../fixtures/doc/structure-v1-properties.snapshot.bin");
const PROPERTIES_JSON: &str =
    include_str!("../../../fixtures/doc/structure-v1-properties.portable.json");

#[test]
fn a_structure_written_before_properties_has_none_but_lists_tags() {
    for snapshot in [STRUCTURE_SNAPSHOT, ATTACHMENTS_SNAPSHOT] {
        let doc = load_structure(snapshot);
        let schema = doc.property_schema();
        assert_eq!(schema.len(), 1);
        assert_eq!(schema[0].id, TAGS_PROPERTY_ID);
        assert_eq!(schema[0].name, "Tags");
        assert!(doc.to_portable().get("properties").is_none());
        let page = doc.page(golden_script::page_id(1)).unwrap();
        assert!(page.properties.is_empty());
    }
}

#[test]
fn structure_v1_properties_snapshot_loads_into_the_golden_json() {
    let (header, _) = decode_blob(PROPERTIES_SNAPSHOT).unwrap();
    assert_eq!(header, V1_SNAPSHOT_HEADER);
    let doc = load_structure(PROPERTIES_SNAPSHOT);
    assert_eq!(doc.to_portable(), golden(PROPERTIES_JSON));
    assert!(
        doc.check_invariants().is_empty(),
        "{:?}",
        doc.check_invariants()
    );
    let mut text = serde_json::to_string_pretty(&doc.to_portable()).unwrap();
    text.push('\n');
    assert_eq!(
        text.replace("\r\n", "\n"),
        PROPERTIES_JSON.replace("\r\n", "\n")
    );

    // Tags first, then the properties in order (the phone's after the others, "Phone" gone, a
    // moved one in front), each with the options it ended with.
    let schema = doc.property_schema();
    let names: Vec<&str> = schema.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Tags",
            "Summary line",
            "Cost",
            "Area",
            "Stage",
            "Due",
            "Done",
            "Link",
            "Contact",
            "Skills",
            "Attachments",
            "To do",
            "From the phone"
        ]
    );
    let tags = &schema[0];
    assert_eq!(tags.options.len(), 5, "three, plus one from each device");
    let area = doc.property(golden_script::property_id(3)).unwrap();
    assert_eq!(
        area.options
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        ["Home", "Elsewhere"],
        "Work was deleted, one option was renamed"
    );
    let stage = doc.property(golden_script::property_id(4)).unwrap();
    assert_eq!(stage.options.len(), 4);
    assert_eq!(stage.options[3].name, "Blocked");

    // The first page has a value of every kind but the deleted property's (it was pruned by the
    // next write), and the select it held was cleared with its option.
    let inbox = doc.page(golden_script::page_id(1)).unwrap();
    assert_eq!(inbox.properties.len(), 11);
    // The files and the checklist (amendment of 2026-10-07) read back as written.
    let value_of = |n: u8| {
        inbox
            .properties
            .iter()
            .find(|(id, _)| *id == golden_script::property_id(n))
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        value_of(12),
        Some(PropertyValue::Files(vec![
            golden_script::attachment_id(2),
            golden_script::attachment_id(1)
        ]))
    );
    assert_eq!(
        value_of(13),
        Some(PropertyValue::Checklist(vec![
            golden_script::checklist_item(1, "Draft the plan", true),
            golden_script::checklist_item(2, "Review with the team \u{2713}", false),
            golden_script::checklist_item(3, "", false),
        ]))
    );
    assert!(
        inbox
            .properties
            .iter()
            .all(|(id, _)| *id != golden_script::property_id(3))
    );
    // A value the first device and the phone wrote to different properties of one page.
    let projects = doc.page(golden_script::page_id(2)).unwrap();
    assert_eq!(projects.properties.len(), 5);
    // The undone page is gone; the trashed page kept its value.
    assert!(doc.page(golden_script::page_id(13)).is_none());
    let archive = doc.page(golden_script::page_id(6)).unwrap();
    assert_eq!(archive.properties.len(), 1);
    assert!(archive.trashed_at.is_some());
}

#[test]
fn the_fixed_script_still_produces_the_properties_golden() {
    assert_eq!(
        golden_script::structure_with_properties().to_portable(),
        golden(PROPERTIES_JSON)
    );
}

#[test]
fn structure_v1_properties_snapshot_stays_editable() {
    let mut doc = load_structure(PROPERTIES_SNAPSHOT);
    let mut counter = 90;
    let mut new_option = move || {
        counter += 1;
        golden_script::option_id(counter)
    };
    doc.create_property(
        golden_script::property_id(20),
        "New",
        PropertyKind::Status,
        &mut new_option,
        1,
    )
    .unwrap();
    doc.set_page_property(
        golden_script::page_id(1),
        golden_script::property_id(20),
        Some(PropertyValue::Status(golden_script::option_id(91))),
        2,
    )
    .unwrap();
    doc.delete_option(
        golden_script::property_id(4),
        golden_script::option_id(42),
        3,
    )
    .unwrap();
    doc.rename_property(TAGS_PROPERTY_ID, "Labels", 4).unwrap();
    doc.rename_page(golden_script::page_id(2), "Edited", 5)
        .unwrap();
    doc.undo().unwrap().unwrap();
    assert_eq!(
        doc.page(golden_script::page_id(2)).unwrap().title,
        "Projects 2026"
    );
    assert_eq!(
        doc.property_schema()[0].name,
        "Labels",
        "only the rename of the page was undone"
    );
    assert!(doc.check_invariants().is_empty());
    let delta = doc
        .doc()
        .export_updates_since(Some(
            &Doc::load(DocKind::Structure, 1, Some(PROPERTIES_SNAPSHOT), &[])
                .unwrap()
                .version(),
        ))
        .unwrap()
        .unwrap();
    let replay = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, 2, Some(PROPERTIES_SNAPSHOT), &[delta]).unwrap(),
    )
    .unwrap();
    assert_eq!(replay.to_portable(), doc.to_portable());
}

// ----- shared spaces (Milestone 13) -------------------------------------------------------

const SHARING_SNAPSHOT: &[u8] =
    include_bytes!("../../../fixtures/doc/account-v1-sharing.snapshot.bin");
const SHARING_JSON: &str = include_str!("../../../fixtures/doc/account-v1-sharing.state.json");

fn load_account(snapshot: &[u8]) -> AccountDoc {
    AccountDoc::from_doc(Doc::load(DocKind::Account, 0xfeed, Some(snapshot), &[]).unwrap()).unwrap()
}

/// The deep value of the whole account document: `to_portable` does not list the maps of shared
/// spaces, so the golden is what the engine holds.
fn deep_json(doc: &AccountDoc) -> serde_json::Value {
    loro::ToJson::to_json_value(&doc.doc().inner().get_deep_value())
}

#[test]
fn an_account_written_before_shared_spaces_has_none() {
    let doc = load_account(ACCOUNT_SNAPSHOT);
    assert!(doc.shared_spaces().is_empty());
    assert_eq!(doc.profile_name(), None);
    assert_eq!(doc.space_ai(golden_script::space_id(1)), None);
    assert!(doc.favorites(golden_script::space_id(1)).is_empty());
    assert!(doc.verified_people().is_empty());
    assert_eq!(doc.to_portable(), golden(ACCOUNT_JSON));
}

#[test]
fn account_v1_sharing_snapshot_loads_into_the_golden_state() {
    let (header, _) = decode_blob(SHARING_SNAPSHOT).unwrap();
    assert_eq!(header, V1_SNAPSHOT_HEADER);
    let doc = load_account(SHARING_SNAPSHOT);
    assert_eq!(deep_json(&doc), golden(SHARING_JSON));
    let mut text = serde_json::to_string_pretty(&deep_json(&doc)).unwrap();
    text.push('\n');
    assert_eq!(
        text.replace("\r\n", "\n"),
        SHARING_JSON.replace("\r\n", "\n")
    );
    // The portable JSON is the one of `account-v1`: it does not list the new maps.
    assert_eq!(doc.to_portable(), golden(ACCOUNT_JSON));

    // Through the accessors.
    let space = golden_script::space_id;
    let mut shared = vec![
        (space(1), golden_script::feed_id(1)),
        (space(4), golden_script::feed_id(4)),
    ];
    shared.sort();
    assert_eq!(doc.shared_spaces(), shared);
    assert_eq!(doc.profile_name().as_deref(), Some("Alice \u{2713}"));
    assert_eq!(doc.space_ai(space(1)), Some(false));
    assert_eq!(doc.space_ai(space(3)), Some(true));
    assert_eq!(doc.space_ai(space(2)), None);
    let page = golden_script::page_id;
    let favorites = doc.favorites(space(1));
    assert_eq!(
        &favorites[..3],
        [page(3), page(1), page(2)],
        "3 was moved to the front"
    );
    assert_eq!(
        favorites.len(),
        4,
        "the second device's favourite is merged in, last"
    );
    assert_eq!(favorites[3], page(4));
    // Neither device's first favourite of space 4 was lost.
    let mut of_space_4 = doc.favorites(space(4));
    of_space_4.sort();
    assert_eq!(of_space_4, [page(7), page(8)]);
    // The second mark of account 1 kept the first time, the unmarked account 3 is gone.
    assert_eq!(
        doc.verified_people(),
        [
            (golden_script::account_id(1), golden_script::T0 + 1),
            (golden_script::account_id(2), golden_script::T0 + 2),
        ]
    );
}

#[test]
fn the_fixed_script_still_produces_the_golden_sharing_state() {
    assert_eq!(
        deep_json(&golden_script::account_with_sharing()),
        golden(SHARING_JSON)
    );
}

#[test]
fn account_v1_sharing_snapshot_stays_editable_and_merges_with_a_replica() {
    let doc = load_account(SHARING_SNAPSHOT);
    let replica = AccountDoc::from_doc(
        Doc::load(DocKind::Account, 0xbeef, Some(SHARING_SNAPSHOT), &[]).unwrap(),
    )
    .unwrap();
    let space = golden_script::space_id;
    let page = golden_script::page_id;
    // Both devices write into the maps that already exist and into a new space's.
    doc.set_favorite(space(1), page(9), true).unwrap();
    replica.set_favorite(space(1), page(10), true).unwrap();
    doc.set_favorite(space(5), page(1), true).unwrap();
    replica.set_favorite(space(5), page(2), true).unwrap();
    doc.set_space_ai(space(5), false).unwrap();
    replica.set_space_ai(space(6), true).unwrap();
    for (from, to) in [(&doc, &replica), (&replica, &doc)] {
        let delta = from
            .doc()
            .export_updates_since(Some(&to.doc().version()))
            .unwrap()
            .unwrap();
        to.doc().import(&delta).unwrap();
    }
    assert_eq!(deep_json(&doc), deep_json(&replica));
    assert_eq!(doc.favorites(space(1)).len(), 6);
    assert_eq!(doc.favorites(space(5)).len(), 2);
    assert_eq!(doc.space_ai(space(5)), Some(false));
    assert_eq!(replica.space_ai(space(6)), Some(true));
}

// ----- view links (Milestone 13, slice 4) -------------------------------------------------

const LINKS_SNAPSHOT: &[u8] =
    include_bytes!("../../../fixtures/doc/structure-v1-links.snapshot.bin");
const LINKS_JSON: &str = include_str!("../../../fixtures/doc/structure-v1-links.state.json");

fn structure_deep_json(doc: &StructureDoc) -> serde_json::Value {
    loro::ToJson::to_json_value(&doc.doc().inner().get_deep_value())
}

#[test]
fn a_structure_written_before_view_links_has_none() {
    for snapshot in [
        STRUCTURE_SNAPSHOT,
        ATTACHMENTS_SNAPSHOT,
        PROPERTIES_SNAPSHOT,
    ] {
        assert!(load_structure(snapshot).links().is_empty());
    }
}

#[test]
fn structure_v1_links_snapshot_loads_into_the_golden_state() {
    let (header, _) = decode_blob(LINKS_SNAPSHOT).unwrap();
    assert_eq!(header, V1_SNAPSHOT_HEADER);
    let doc = load_structure(LINKS_SNAPSHOT);
    assert_eq!(structure_deep_json(&doc), golden(LINKS_JSON));
    let mut text = serde_json::to_string_pretty(&structure_deep_json(&doc)).unwrap();
    text.push('\n');
    assert_eq!(text.replace("\r\n", "\n"), LINKS_JSON.replace("\r\n", "\n"));
    // The links are not exported: the portable JSON is the one of `structure-v1`.
    assert_eq!(doc.to_portable(), golden(STRUCTURE_JSON));
    assert!(
        doc.check_invariants().is_empty(),
        "{:?}",
        doc.check_invariants()
    );

    // Through the accessors: newest first; the revoked link is gone, the replaced one expires and the
    // second device's link is merged in.
    let ids: Vec<_> = doc
        .links()
        .into_iter()
        .map(|record| record.link_id)
        .collect();
    assert_eq!(
        ids,
        [5, 3, 2, 1].map(golden_script::link_id).to_vec(),
        "newest first"
    );
    assert!(doc.link(golden_script::link_id(4)).is_none());
    let replaced = doc.link(golden_script::link_id(3)).unwrap();
    assert_eq!(
        replaced.expires_at,
        Some(golden_script::T0 + 82 + 30 * 86_400_000)
    );
    assert_eq!(replaced.page_id, golden_script::page_id(5));
    assert_eq!(replaced.created_by, golden_script::account_id(1));
    assert_eq!(
        doc.link(golden_script::link_id(1)).unwrap().expires_at,
        None
    );
}

#[test]
fn the_fixed_script_still_produces_the_links_golden() {
    assert_eq!(
        structure_deep_json(&golden_script::structure_with_links()),
        golden(LINKS_JSON)
    );
}

#[test]
fn structure_v1_links_snapshot_stays_editable() {
    let mut doc = load_structure(LINKS_SNAPSHOT);
    doc.create_page(golden_script::page_id(202), None, None, "New", 1)
        .unwrap();
    doc.link_remove(golden_script::link_id(1)).unwrap();
    doc.link_put(&kb_model::ViewLinkRecord {
        link_id: golden_script::link_id(6),
        page_id: golden_script::page_id(202),
        created_by: golden_script::account_id(3),
        created_at: 5,
        expires_at: None,
    })
    .unwrap();
    // Undo takes the page back and leaves both link writes.
    doc.undo().unwrap().unwrap();
    assert!(doc.page(golden_script::page_id(202)).is_none());
    assert!(doc.link(golden_script::link_id(1)).is_none());
    assert!(doc.link(golden_script::link_id(6)).is_some());
    let delta = doc
        .doc()
        .export_updates_since(Some(
            &Doc::load(DocKind::Structure, 1, Some(LINKS_SNAPSHOT), &[])
                .unwrap()
                .version(),
        ))
        .unwrap()
        .unwrap();
    let replay = StructureDoc::from_doc(
        Doc::load(DocKind::Structure, 2, Some(LINKS_SNAPSHOT), &[delta]).unwrap(),
    )
    .unwrap();
    assert_eq!(replay.links(), doc.links());
}
