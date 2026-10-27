//! The structure document's `links` map (ADR-0051, `links.rs`): the entry, the order, merges, undo
//! and the findings of `check_invariants`.

use kb_types::{AccountId, LinkId};

use super::{add, assert_healthy, pid, replica, structure, sync};
use crate::{LINKS_MAP, ViewLinkRecord};

fn link(n: u8) -> LinkId {
    LinkId::from_bytes([n; 16])
}

fn record(n: u8, page: u8, created_at: i64, expires_at: Option<i64>) -> ViewLinkRecord {
    ViewLinkRecord {
        link_id: link(n),
        page_id: pid(page),
        created_by: AccountId::from_bytes([0xa0 + n; 16]),
        created_at,
        expires_at,
    }
}

#[test]
fn a_new_space_has_no_links() {
    let doc = structure(1);
    assert!(doc.links().is_empty());
    assert!(doc.link(link(1)).is_none());
    assert_healthy(&doc);
}

#[test]
fn an_entry_round_trips_with_and_without_an_expiry_and_survives_a_reload() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    let forever = record(1, 1, 100, None);
    let expiring = record(2, 1, 200, Some(9_000));
    doc.link_put(&forever).unwrap();
    doc.link_put(&expiring).unwrap();
    assert_eq!(doc.link(link(1)), Some(forever.clone()));
    assert_eq!(doc.link(link(2)), Some(expiring.clone()));
    assert_healthy(&doc);
    let again = replica(&doc, 2);
    assert_eq!(again.links(), vec![expiring, forever]);
}

#[test]
fn links_list_newest_first_then_by_id_and_a_put_replaces() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    doc.link_put(&record(3, 1, 50, None)).unwrap();
    doc.link_put(&record(1, 1, 50, None)).unwrap();
    doc.link_put(&record(2, 1, 70, None)).unwrap();
    let ids: Vec<LinkId> = doc.links().into_iter().map(|r| r.link_id).collect();
    assert_eq!(ids, [link(2), link(1), link(3)]);
    doc.link_put(&record(1, 1, 90, Some(5))).unwrap();
    assert_eq!(doc.links().len(), 3);
    assert_eq!(doc.links()[0].link_id, link(1));
    assert_eq!(doc.link(link(1)).unwrap().expires_at, Some(5));
}

#[test]
fn removing_is_a_no_op_when_absent_and_never_touches_other_entries() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    doc.link_put(&record(1, 1, 1, None)).unwrap();
    doc.link_put(&record(2, 1, 2, None)).unwrap();
    doc.link_remove(link(9)).unwrap();
    doc.link_remove(link(1)).unwrap();
    assert!(doc.link(link(1)).is_none());
    assert!(doc.link(link(2)).is_some());
}

#[test]
fn link_writes_are_never_undone_and_never_undo_a_page_edit() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    doc.link_put(&record(1, 1, 1, None)).unwrap();
    add(&mut doc, 2, None, "Other");
    doc.link_remove(link(1)).unwrap();
    doc.link_put(&record(2, 1, 2, None)).unwrap();
    // Undo takes the page back and leaves the links as they are.
    assert!(doc.undo().unwrap().is_some());
    assert!(doc.page(pid(2)).is_none());
    assert!(doc.link(link(1)).is_none());
    assert!(doc.link(link(2)).is_some());
    assert!(doc.redo().unwrap().is_some());
    assert!(doc.link(link(2)).is_some());
}

#[test]
fn entries_merge_per_key_across_replicas() {
    let mut a = structure(1);
    add(&mut a, 1, None, "Page");
    let mut b = replica(&a, 2);
    a.link_put(&record(1, 1, 10, None)).unwrap();
    b.link_put(&record(2, 1, 20, Some(99))).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.links(), b.links());
    assert_eq!(a.links().len(), 2);
    // A removal on one side wins over nothing on the other.
    b.link_remove(link(1)).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.links().len(), 1);
    assert_healthy(&a);
}

#[test]
fn an_entry_that_does_not_parse_is_ignored_and_a_finding() {
    use loro::LoroValue;
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    doc.link_put(&record(1, 1, 1, None)).unwrap();
    // Another version, a mistyped field and a malformed key: none is listed, each is reported.
    let map = doc.doc().inner().get_map(LINKS_MAP);
    let other_version =
        LoroValue::from(std::collections::HashMap::from([("v", LoroValue::I64(2))]));
    map.insert(&link(2).to_string(), other_version).unwrap();
    let mistyped = LoroValue::from(std::collections::HashMap::from([
        ("v", LoroValue::I64(1)),
        ("page", LoroValue::I64(5)),
    ]));
    map.insert(&link(3).to_string(), mistyped).unwrap();
    map.insert("not-an-id", LoroValue::I64(1)).unwrap();
    doc.doc().inner().commit();
    assert_eq!(doc.links().len(), 1);
    assert!(doc.link(link(2)).is_none());
    let problems = doc.check_invariants();
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("malformed key")));
}

#[test]
fn a_link_of_a_purged_page_is_a_finding_and_nothing_else() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    add(&mut doc, 2, None, "Gone");
    doc.link_put(&record(1, 1, 1, None)).unwrap();
    doc.link_put(&record(2, 2, 2, None)).unwrap();
    doc.trash_page(pid(2), 10).unwrap();
    assert!(doc.check_invariants().is_empty(), "trashed is fine");
    doc.purge_page(pid(2), 11).unwrap();
    let problems = doc.check_invariants();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains(&link(2).short_hex()));
    // The entry stays: an Owner revokes the link, which removes it.
    assert_eq!(doc.links().len(), 2);
    doc.link_remove(link(2)).unwrap();
    assert_healthy(&doc);
}

#[test]
fn the_links_map_does_not_leak_into_the_portable_form() {
    let mut doc = structure(1);
    add(&mut doc, 1, None, "Page");
    let before = doc.to_portable();
    doc.link_put(&record(1, 1, 1, Some(7))).unwrap();
    assert_eq!(doc.to_portable(), before);
}
