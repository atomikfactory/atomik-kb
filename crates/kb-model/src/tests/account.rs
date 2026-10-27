use kb_doc::{Doc, DocKind};
use serde_json::json;

use super::sid;
use crate::{AccountDoc, ModelError};

fn account(peer: u64) -> AccountDoc {
    AccountDoc::new(Doc::new(DocKind::Account, peer))
}

fn reload(doc: &AccountDoc, peer: u64) -> AccountDoc {
    let snapshot = doc.doc().export_snapshot().unwrap();
    AccountDoc::from_doc(Doc::load(DocKind::Account, peer, Some(&snapshot), &[]).unwrap()).unwrap()
}

#[test]
fn a_new_account_document_is_empty_and_writes_nothing() {
    let doc = account(1);
    assert!(doc.spaces().is_empty());
    assert_eq!(doc.doc().export_updates_since(None).unwrap(), None);
    assert_eq!(doc.to_portable(), json!({"format": 1, "spaces": []}));
}

#[test]
fn add_appends_once_and_survives_reload() {
    let doc = account(1);
    doc.add_space(sid(1)).unwrap();
    doc.add_space(sid(2)).unwrap();
    let version = doc.doc().version();
    doc.add_space(sid(1)).unwrap();
    assert_eq!(
        doc.doc().version(),
        version,
        "adding a present space writes nothing"
    );
    assert_eq!(doc.spaces(), vec![sid(1), sid(2)]);
    assert!(doc.doc().export_updates_since(None).unwrap().is_some());

    let mut loaded = reload(&doc, 2);
    loaded.refresh().unwrap();
    assert_eq!(loaded.spaces(), vec![sid(1), sid(2)]);
    assert_eq!(
        loaded.to_portable(),
        json!({"format": 1, "spaces": [sid(1).to_string(), sid(2).to_string()]})
    );
}

#[test]
fn remove_drops_every_occurrence_and_ignores_unknown_spaces() {
    let doc = account(1);
    for n in 1..=3 {
        doc.add_space(sid(n)).unwrap();
    }
    // A duplicate as a concurrent add would leave it.
    doc.doc()
        .inner()
        .get_movable_list("spaces")
        .push(sid(2).to_string())
        .unwrap();
    doc.remove_space(sid(2)).unwrap();
    assert_eq!(doc.spaces(), vec![sid(1), sid(3)]);
    assert_eq!(doc.doc().inner().get_movable_list("spaces").len(), 2);
    doc.remove_space(sid(9)).unwrap();
    assert_eq!(doc.spaces(), vec![sid(1), sid(3)]);
}

#[test]
fn move_places_the_space_at_the_clamped_index() {
    let doc = account(1);
    for n in 1..=4 {
        doc.add_space(sid(n)).unwrap();
    }
    doc.move_space(sid(3), 0).unwrap();
    assert_eq!(doc.spaces(), vec![sid(3), sid(1), sid(2), sid(4)]);
    doc.move_space(sid(3), 2).unwrap();
    assert_eq!(doc.spaces(), vec![sid(1), sid(2), sid(3), sid(4)]);
    doc.move_space(sid(1), 99).unwrap();
    assert_eq!(doc.spaces(), vec![sid(2), sid(3), sid(4), sid(1)]);
    doc.move_space(sid(4), 1).unwrap();
    assert_eq!(doc.spaces(), vec![sid(2), sid(4), sid(3), sid(1)]);

    let version = doc.doc().version();
    doc.move_space(sid(4), 1).unwrap();
    assert_eq!(
        doc.doc().version(),
        version,
        "a move in place writes nothing"
    );

    assert!(matches!(
        doc.move_space(sid(9), 0),
        Err(ModelError::SpaceNotFound)
    ));
}

#[test]
fn concurrent_adds_of_one_space_are_deduplicated_and_cleaned_by_writers() {
    let a = account(1);
    a.add_space(sid(1)).unwrap();
    let b = reload(&a, 2);
    // Both devices add space 2 and 3 without seeing each other.
    a.add_space(sid(2)).unwrap();
    b.add_space(sid(3)).unwrap();
    b.add_space(sid(2)).unwrap();
    let to_b = a
        .doc()
        .export_updates_since(Some(&b.doc().version()))
        .unwrap()
        .unwrap();
    let to_a = b
        .doc()
        .export_updates_since(Some(&a.doc().version()))
        .unwrap()
        .unwrap();
    a.doc().import(&to_a).unwrap();
    b.doc().import(&to_b).unwrap();
    assert_eq!(a.spaces(), b.spaces(), "replicas agree");
    assert_eq!(a.spaces().len(), 3, "duplicates are hidden");
    assert_eq!(a.doc().inner().get_movable_list("spaces").len(), 4);
    assert_eq!(a.to_portable(), b.to_portable());

    // Moving the duplicated space removes the extra copy.
    let last = a.spaces().len() - 1;
    a.move_space(sid(2), last).unwrap();
    assert_eq!(a.doc().inner().get_movable_list("spaces").len(), 3);
    assert_eq!(*a.spaces().last().unwrap(), sid(2));
}

#[test]
fn entries_that_are_not_space_ids_are_ignored() {
    let doc = account(1);
    doc.add_space(sid(1)).unwrap();
    let list = doc.doc().inner().get_movable_list("spaces");
    list.insert(0, "not a uuid").unwrap();
    list.insert(0, 42).unwrap();
    assert_eq!(doc.spaces(), vec![sid(1)]);
    doc.add_space(sid(2)).unwrap();
    doc.move_space(sid(2), 0).unwrap();
    assert_eq!(doc.spaces(), vec![sid(2), sid(1)]);
}

#[test]
fn from_doc_refuses_other_document_kinds() {
    assert!(matches!(
        AccountDoc::from_doc(Doc::new(DocKind::Structure, 1)),
        Err(ModelError::Shape(_))
    ));
    assert!(AccountDoc::from_doc(Doc::new(DocKind::Account, 1)).is_ok());
}
