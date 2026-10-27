//! The account document's maps for shared spaces (M13, `sharing.rs`): `sharedSpaces`, `profile`,
//! `spaceSettings`, `favorites/<space>` and `verifiedPeople`: reads, writes, idempotence, the
//! document shape, reloads, damaged entries, and merges between two replicas.

use kb_doc::{Doc, DocKind};
use kb_types::{AccountId, FeedId, PageId};
use loro::ToJson;
use serde_json::json;

use super::{add, pid, sid, structure};
use crate::{
    AccountDoc, FAVORITES_MAP, ModelError, PROFILE_MAP, PROFILE_NAME_KEY, SHARED_SPACES_MAP,
    SPACE_AI_KEY, SPACE_SETTINGS_MAP, StructureDoc, VERIFIED_PEOPLE_MAP,
};

fn account(peer: u64) -> AccountDoc {
    AccountDoc::new(Doc::new(DocKind::Account, peer))
}

/// A replica of `doc` (loaded from its snapshot) with peer `peer`.
fn reload(doc: &AccountDoc, peer: u64) -> AccountDoc {
    let snapshot = doc.doc().export_snapshot().unwrap();
    AccountDoc::from_doc(Doc::load(DocKind::Account, peer, Some(&snapshot), &[]).unwrap()).unwrap()
}

/// Sends what `to` lacks from `from`.
fn send(from: &AccountDoc, to: &AccountDoc) {
    if let Some(blob) = from
        .doc()
        .export_updates_since(Some(&to.doc().version()))
        .unwrap()
    {
        to.doc().import(&blob).unwrap();
    }
}

/// Exchanges updates both ways.
fn sync(a: &AccountDoc, b: &AccountDoc) {
    send(a, b);
    send(b, a);
}

fn fid(n: u8) -> FeedId {
    FeedId::from_random_bytes([n; 16])
}

fn aid(n: u8) -> AccountId {
    AccountId::from_random_bytes([n; 16])
}

/// The page ids of these tests are `pid(n)` = sixteen bytes of `n`; recover the numbers.
fn numbers(ids: &[PageId]) -> Vec<u8> {
    ids.iter()
        .map(|id| u8::from_str_radix(&id.to_string()[..2], 16).unwrap())
        .collect()
}

fn favorite_numbers(doc: &AccountDoc, space: u8) -> Vec<u8> {
    numbers(&doc.favorites(sid(space)))
}

/// Whether the document wrote anything at all so far.
fn written(doc: &AccountDoc) -> bool {
    doc.doc().export_updates_since(None).unwrap().is_some()
}

/// The deep value of the whole document as JSON.
fn shape(doc: &AccountDoc) -> serde_json::Value {
    doc.doc().inner().get_deep_value().to_json_value()
}

// ----- the shape ----------------------------------------------------------------------------

#[test]
fn a_new_document_has_no_sharing_state_and_writes_nothing() {
    let doc = account(1);
    assert!(doc.shared_spaces().is_empty());
    assert_eq!(doc.shared_space(sid(1)), None);
    assert_eq!(doc.profile_name(), None);
    assert_eq!(doc.space_ai(sid(1)), None);
    assert!(doc.favorites(sid(1)).is_empty());
    assert!(doc.verified_people().is_empty());
    assert_eq!(doc.verified_at(aid(1)), None);
    assert!(!written(&doc), "reading never writes");
    // Nothing to forget either: removals of what is absent write nothing.
    doc.remove_shared_space(sid(1)).unwrap();
    doc.remove_space_settings(sid(1)).unwrap();
    doc.remove_favorites(sid(1)).unwrap();
    doc.set_verified(aid(1), None).unwrap();
    assert!(!written(&doc));
}

#[test]
fn the_root_maps_and_their_keys_are_the_documented_ones() {
    assert_eq!(SHARED_SPACES_MAP, "sharedSpaces");
    assert_eq!(PROFILE_MAP, "profile");
    assert_eq!(PROFILE_NAME_KEY, "name");
    assert_eq!(SPACE_SETTINGS_MAP, "spaceSettings");
    assert_eq!(SPACE_AI_KEY, "ai");
    assert_eq!(FAVORITES_MAP, "favorites");
    assert_eq!(VERIFIED_PEOPLE_MAP, "verifiedPeople");

    let doc = account(1);
    doc.add_space(sid(1)).unwrap();
    doc.set_device_name(kb_types::DeviceId::from_random_bytes([9; 16]), "Laptop")
        .unwrap();
    doc.set_shared_space(sid(1), fid(2)).unwrap();
    doc.set_profile_name("Alice").unwrap();
    doc.set_space_ai(sid(1), true).unwrap();
    doc.set_favorite(sid(1), pid(7), true).unwrap();
    doc.set_verified(aid(3), Some(1_700_000_000_000)).unwrap();

    let value = shape(&doc);
    assert_eq!(
        value["sharedSpaces"],
        json!({ sid(1).to_string(): fid(2).to_string() })
    );
    assert_eq!(value["profile"], json!({ "name": "Alice" }));
    assert_eq!(
        value["spaceSettings"],
        json!({ sid(1).to_string(): { "ai": true } })
    );
    // `favorites/<space id>`: the root map `favorites`, one map per space (Loro root names cannot
    // hold a `/`), page id -> a fractional position.
    let favorites = value["favorites"][sid(1).to_string()].as_object().unwrap();
    assert_eq!(favorites.len(), 1);
    assert!(favorites[&pid(7).to_string()].is_string());
    assert_eq!(
        value["verifiedPeople"],
        json!({ aid(3).to_string(): 1_700_000_000_000_i64 })
    );
    // What was there before stays where it was.
    assert_eq!(value["spaces"], json!([sid(1).to_string()]));
    assert!(value["devices"].is_object());
}

#[test]
fn the_portable_json_does_not_change() {
    let doc = account(1);
    doc.add_space(sid(1)).unwrap();
    let before = doc.to_portable();
    doc.set_shared_space(sid(1), fid(2)).unwrap();
    doc.set_profile_name("Alice").unwrap();
    doc.set_space_ai(sid(1), false).unwrap();
    doc.set_favorite(sid(1), pid(1), true).unwrap();
    doc.set_verified(aid(1), Some(5)).unwrap();
    assert_eq!(
        doc.to_portable(),
        before,
        "the golden of `account-v1.portable.json` stays valid"
    );
}

// ----- sharedSpaces -------------------------------------------------------------------------

#[test]
fn shared_spaces_are_recorded_listed_by_space_id_and_forgotten() {
    let doc = account(1);
    for (space, feed) in [(3, 30), (1, 10), (2, 20)] {
        doc.set_shared_space(sid(space), fid(feed)).unwrap();
    }
    assert_eq!(doc.shared_space(sid(2)), Some(fid(20)));
    assert_eq!(doc.shared_space(sid(9)), None);
    assert_eq!(
        doc.shared_spaces(),
        [(sid(1), fid(10)), (sid(2), fid(20)), (sid(3), fid(30))],
        "ordered by space id"
    );

    doc.remove_shared_space(sid(2)).unwrap();
    doc.remove_shared_space(sid(2)).unwrap();
    assert_eq!(doc.shared_space(sid(2)), None);
    assert_eq!(doc.shared_spaces().len(), 2);

    // Re-recording after a removal works (a space that was left and joined again).
    doc.set_shared_space(sid(2), fid(21)).unwrap();
    assert_eq!(doc.shared_space(sid(2)), Some(fid(21)));
}

#[test]
fn recording_the_same_pair_again_writes_nothing() {
    let doc = account(1);
    doc.set_shared_space(sid(1), fid(1)).unwrap();
    let version = doc.doc().version();
    doc.set_shared_space(sid(1), fid(1)).unwrap();
    assert_eq!(doc.doc().version(), version);
    // A different feed for the space replaces the pair (last writer wins).
    doc.set_shared_space(sid(1), fid(2)).unwrap();
    assert_ne!(doc.doc().version(), version);
    assert_eq!(doc.shared_spaces(), [(sid(1), fid(2))]);
}

#[test]
fn shared_spaces_survive_a_reload_and_damaged_entries_are_ignored() {
    let doc = account(1);
    doc.set_shared_space(sid(1), fid(1)).unwrap();
    let map = doc.doc().inner().get_map(SHARED_SPACES_MAP);
    map.insert("not a uuid", fid(2).to_string().as_str())
        .unwrap();
    map.insert(&sid(2).to_string(), "not a uuid").unwrap();
    map.insert(&sid(3).to_string(), 42).unwrap();
    doc.doc().inner().commit();
    assert_eq!(doc.shared_spaces(), [(sid(1), fid(1))]);
    assert_eq!(doc.shared_space(sid(2)), None);
    assert_eq!(doc.shared_space(sid(3)), None);
    assert_eq!(reload(&doc, 2).shared_spaces(), [(sid(1), fid(1))]);
}

#[test]
fn two_devices_that_share_different_spaces_both_keep_theirs() {
    let a = account(1);
    a.set_shared_space(sid(1), fid(1)).unwrap();
    let b = reload(&a, 2);
    a.set_shared_space(sid(2), fid(2)).unwrap();
    b.set_shared_space(sid(3), fid(3)).unwrap();
    sync(&a, &b);
    let expected = [(sid(1), fid(1)), (sid(2), fid(2)), (sid(3), fid(3))];
    assert_eq!(a.shared_spaces(), expected);
    assert_eq!(b.shared_spaces(), expected);
}

#[test]
fn a_concurrent_removal_and_a_new_space_merge_per_key() {
    let a = account(1);
    a.set_shared_space(sid(1), fid(1)).unwrap();
    let b = reload(&a, 2);
    a.remove_shared_space(sid(1)).unwrap();
    b.set_shared_space(sid(2), fid(2)).unwrap();
    sync(&a, &b);
    assert_eq!(a.shared_spaces(), [(sid(2), fid(2))]);
    assert_eq!(b.shared_spaces(), a.shared_spaces());
}

#[test]
fn two_devices_that_record_one_space_with_different_feeds_converge() {
    let a = account(1);
    let b = account(2);
    a.set_shared_space(sid(1), fid(1)).unwrap();
    b.set_shared_space(sid(1), fid(2)).unwrap();
    sync(&a, &b);
    assert_eq!(a.shared_spaces(), b.shared_spaces());
    assert_eq!(a.shared_spaces().len(), 1);
    assert!([fid(1), fid(2)].contains(&a.shared_space(sid(1)).unwrap()));
}

// ----- profile ------------------------------------------------------------------------------

#[test]
fn the_profile_name_is_set_replaced_and_survives_a_reload() {
    let doc = account(1);
    doc.set_profile_name("Alice").unwrap();
    assert_eq!(doc.profile_name().as_deref(), Some("Alice"));
    let version = doc.doc().version();
    doc.set_profile_name("Alice").unwrap();
    assert_eq!(doc.doc().version(), version, "the same name writes nothing");
    doc.set_profile_name("Alice B. \u{1F389}").unwrap();
    assert_eq!(doc.profile_name().as_deref(), Some("Alice B. \u{1F389}"));
    assert_eq!(
        reload(&doc, 2).profile_name().as_deref(),
        Some("Alice B. \u{1F389}")
    );
}

#[test]
fn a_profile_name_that_is_not_text_reads_as_none() {
    let doc = account(1);
    doc.doc()
        .inner()
        .get_map(PROFILE_MAP)
        .insert(PROFILE_NAME_KEY, 7)
        .unwrap();
    doc.doc().inner().commit();
    assert_eq!(doc.profile_name(), None);
    doc.set_profile_name("Bob").unwrap();
    assert_eq!(doc.profile_name().as_deref(), Some("Bob"));
}

#[test]
fn two_devices_that_rename_the_profile_converge() {
    let a = account(1);
    a.set_profile_name("Alice").unwrap();
    let b = reload(&a, 2);
    a.set_profile_name("Alice A").unwrap();
    b.set_profile_name("Alice B").unwrap();
    sync(&a, &b);
    assert_eq!(a.profile_name(), b.profile_name());
    assert!(["Alice A", "Alice B"].contains(&a.profile_name().unwrap().as_str()));
}

// ----- spaceSettings ------------------------------------------------------------------------

#[test]
fn the_ai_switch_is_kept_per_space_and_none_until_chosen() {
    let doc = account(1);
    assert_eq!(doc.space_ai(sid(1)), None);
    doc.set_space_ai(sid(1), true).unwrap();
    doc.set_space_ai(sid(2), false).unwrap();
    assert_eq!(doc.space_ai(sid(1)), Some(true));
    assert_eq!(doc.space_ai(sid(2)), Some(false));
    assert_eq!(doc.space_ai(sid(3)), None);

    doc.set_space_ai(sid(1), false).unwrap();
    assert_eq!(doc.space_ai(sid(1)), Some(false));
    let version = doc.doc().version();
    doc.set_space_ai(sid(1), false).unwrap();
    assert_eq!(
        doc.doc().version(),
        version,
        "the same value writes nothing"
    );

    let loaded = reload(&doc, 2);
    assert_eq!(loaded.space_ai(sid(1)), Some(false));
    assert_eq!(loaded.space_ai(sid(2)), Some(false));
}

#[test]
fn forgetting_a_spaces_settings_removes_them_for_good() {
    let doc = account(1);
    doc.set_space_ai(sid(1), true).unwrap();
    doc.set_space_ai(sid(2), true).unwrap();
    doc.remove_space_settings(sid(1)).unwrap();
    assert_eq!(doc.space_ai(sid(1)), None);
    assert_eq!(doc.space_ai(sid(2)), Some(true), "the other space is kept");
    doc.remove_space_settings(sid(1)).unwrap();
    // A later write starts from nothing: the old value does not come back.
    doc.set_space_ai(sid(1), false).unwrap();
    assert_eq!(doc.space_ai(sid(1)), Some(false));
    doc.remove_space_settings(sid(1)).unwrap();
    assert_eq!(reload(&doc, 2).space_ai(sid(1)), None);
}

#[test]
fn an_ai_value_that_is_not_a_boolean_reads_as_none() {
    let doc = account(1);
    doc.set_space_ai(sid(1), true).unwrap();
    let child = doc
        .doc()
        .inner()
        .get_map(SPACE_SETTINGS_MAP)
        .ensure_mergeable_map(&sid(1).to_string())
        .unwrap();
    child.insert(SPACE_AI_KEY, "yes").unwrap();
    doc.doc().inner().commit();
    assert_eq!(doc.space_ai(sid(1)), None);
    doc.set_space_ai(sid(1), false).unwrap();
    assert_eq!(doc.space_ai(sid(1)), Some(false));
    // A value that is not a map at all is replaced by a write and ignored by a read.
    doc.doc()
        .inner()
        .get_map(SPACE_SETTINGS_MAP)
        .insert(&sid(2).to_string(), 5)
        .unwrap();
    doc.doc().inner().commit();
    assert_eq!(doc.space_ai(sid(2)), None);
    doc.set_space_ai(sid(2), true).unwrap();
    assert_eq!(doc.space_ai(sid(2)), Some(true));
}

#[test]
fn settings_of_different_spaces_written_on_two_devices_both_survive() {
    let a = account(1);
    let b = account(2);
    a.set_space_ai(sid(1), true).unwrap();
    b.set_space_ai(sid(2), false).unwrap();
    sync(&a, &b);
    for doc in [&a, &b] {
        assert_eq!(doc.space_ai(sid(1)), Some(true));
        assert_eq!(doc.space_ai(sid(2)), Some(false));
    }
}

#[test]
fn two_devices_that_set_one_spaces_switch_first_converge_on_one_value() {
    // Both create the space's settings map at the same moment: it is one map on both, so
    // nothing written into it is lost, and the switch itself is last-writer-wins.
    let a = account(1);
    let b = account(2);
    a.set_space_ai(sid(1), true).unwrap();
    b.set_space_ai(sid(1), false).unwrap();
    sync(&a, &b);
    assert_eq!(a.space_ai(sid(1)), b.space_ai(sid(1)));
    assert!(a.space_ai(sid(1)).is_some());
}

#[test]
fn a_setting_a_newer_version_adds_to_the_same_space_is_not_lost_in_a_concurrent_first_write() {
    // The reason the per-space map is a mergeable container: another key written into the
    // space's settings concurrently with the first `ai` write is kept on both replicas.
    let a = account(1);
    let b = account(2);
    let key = sid(1).to_string();
    a.set_space_ai(sid(1), true).unwrap();
    let other = b
        .doc()
        .inner()
        .get_map(SPACE_SETTINGS_MAP)
        .ensure_mergeable_map(&key)
        .unwrap();
    other.insert("futureSetting", 7).unwrap();
    b.doc().inner().commit();
    sync(&a, &b);
    for doc in [&a, &b] {
        assert_eq!(doc.space_ai(sid(1)), Some(true));
        assert_eq!(
            shape(doc)["spaceSettings"][&key]["futureSetting"],
            json!(7),
            "the other key survived"
        );
    }
}

// ----- favorites/<space> --------------------------------------------------------------------

#[test]
fn favourites_are_listed_in_the_order_they_were_added() {
    let doc = account(1);
    for n in [3, 1, 4] {
        assert!(doc.set_favorite(sid(1), pid(n), true).unwrap());
    }
    assert_eq!(favorite_numbers(&doc, 1), [3, 1, 4]);
    assert!(favorite_numbers(&doc, 2).is_empty(), "per space");
    assert_eq!(favorite_numbers(&reload(&doc, 2), 1), [3, 1, 4]);
}

#[test]
fn adding_is_idempotent_and_removing_deletes_the_entry() {
    let doc = account(1);
    doc.set_favorite(sid(1), pid(1), true).unwrap();
    doc.set_favorite(sid(1), pid(2), true).unwrap();
    let version = doc.doc().version();
    assert!(!doc.set_favorite(sid(1), pid(1), true).unwrap());
    assert_eq!(doc.doc().version(), version, "nothing was written");
    assert_eq!(favorite_numbers(&doc, 1), [1, 2]);

    assert!(doc.set_favorite(sid(1), pid(1), false).unwrap());
    assert!(!doc.set_favorite(sid(1), pid(1), false).unwrap());
    assert!(!doc.set_favorite(sid(1), pid(9), false).unwrap());
    assert_eq!(favorite_numbers(&doc, 1), [2]);
    // Adding again puts it last, whatever its old place was.
    doc.set_favorite(sid(1), pid(1), true).unwrap();
    assert_eq!(favorite_numbers(&doc, 1), [2, 1]);
    // Another space is another list.
    doc.set_favorite(sid(2), pid(1), true).unwrap();
    assert_eq!(favorite_numbers(&doc, 2), [1]);
    assert_eq!(favorite_numbers(&doc, 1), [2, 1]);
}

#[test]
fn a_move_puts_the_favourite_in_front_of_another_or_last() {
    let doc = account(1);
    for n in 1..=5 {
        doc.set_favorite(sid(1), pid(n), true).unwrap();
    }
    assert!(doc.move_favorite(sid(1), pid(4), Some(pid(2))).unwrap());
    assert_eq!(favorite_numbers(&doc, 1), [1, 4, 2, 3, 5]);
    assert!(doc.move_favorite(sid(1), pid(5), Some(pid(1))).unwrap());
    assert_eq!(favorite_numbers(&doc, 1), [5, 1, 4, 2, 3]);
    assert!(doc.move_favorite(sid(1), pid(5), None).unwrap());
    assert_eq!(favorite_numbers(&doc, 1), [1, 4, 2, 3, 5]);

    // Moves to the place a favourite already holds write nothing.
    let version = doc.doc().version();
    assert!(!doc.move_favorite(sid(1), pid(5), None).unwrap());
    assert!(!doc.move_favorite(sid(1), pid(4), Some(pid(2))).unwrap());
    assert!(!doc.move_favorite(sid(1), pid(4), Some(pid(4))).unwrap());
    assert_eq!(doc.doc().version(), version);

    // Many moves in the same spot never run out of positions.
    for round in 0..40 {
        let target = if round % 2 == 0 { pid(1) } else { pid(4) };
        doc.move_favorite(sid(1), pid(3), Some(target)).unwrap();
    }
    let mut sorted = favorite_numbers(&doc, 1);
    sorted.sort_unstable();
    assert_eq!(sorted, [1, 2, 3, 4, 5]);
}

fn invalid_field(err: &ModelError) -> Option<&'static str> {
    match err {
        ModelError::InvalidFavorite { field, .. } => Some(field),
        _ => None,
    }
}

#[test]
fn a_move_needs_favourites_of_that_space() {
    let doc = account(1);
    doc.set_favorite(sid(1), pid(1), true).unwrap();
    doc.set_favorite(sid(2), pid(2), true).unwrap();
    let err = doc.move_favorite(sid(1), pid(2), None).unwrap_err();
    assert_eq!(invalid_field(&err), Some("pageId"), "not a favourite here");
    let err = doc.move_favorite(sid(1), pid(1), Some(pid(2))).unwrap_err();
    assert_eq!(invalid_field(&err), Some("before"));
    let err = doc.move_favorite(sid(3), pid(1), None).unwrap_err();
    assert_eq!(invalid_field(&err), Some("pageId"));
    assert_eq!(
        favorite_numbers(&doc, 1),
        [1],
        "refused moves change nothing"
    );
}

#[test]
fn equal_positions_are_ordered_by_page_id_and_unreadable_entries_are_ignored() {
    let doc = account(1);
    let key = sid(1).to_string();
    let map = doc
        .doc()
        .inner()
        .get_map(FAVORITES_MAP)
        .ensure_mergeable_map(&key)
        .unwrap();
    // Two devices appended at the same moment: equal positions, and damaged entries around them.
    map.insert(&pid(5).to_string(), "V").unwrap();
    map.insert(&pid(2).to_string(), "V").unwrap();
    map.insert(&pid(9).to_string(), "A").unwrap();
    map.insert("not a page id", "B").unwrap();
    map.insert(&pid(7).to_string(), 12).unwrap();
    doc.doc().inner().commit();
    assert_eq!(favorite_numbers(&doc, 1), [9, 2, 5]);

    // Adding goes after the last position, moving fixes the tie.
    doc.set_favorite(sid(1), pid(1), true).unwrap();
    assert_eq!(favorite_numbers(&doc, 1), [9, 2, 5, 1]);
    doc.move_favorite(sid(1), pid(5), Some(pid(2))).unwrap();
    assert_eq!(favorite_numbers(&doc, 1), [9, 5, 2, 1]);
}

#[test]
fn a_damaged_last_position_is_renumbered_on_add() {
    let doc = account(1);
    let map = doc
        .doc()
        .inner()
        .get_map(FAVORITES_MAP)
        .ensure_mergeable_map(&sid(1).to_string())
        .unwrap();
    // `~` is not a base-62 digit: no position can be made after it.
    map.insert(&pid(1).to_string(), "~~").unwrap();
    doc.doc().inner().commit();
    doc.set_favorite(sid(1), pid(2), true).unwrap();
    assert_eq!(favorite_numbers(&doc, 1).len(), 2);
    doc.set_favorite(sid(1), pid(3), true).unwrap();
    assert_eq!(favorite_numbers(&doc, 1).last(), Some(&3));
}

#[test]
fn forgetting_a_spaces_favourites_removes_them_for_good() {
    let doc = account(1);
    for n in 1..=3 {
        doc.set_favorite(sid(1), pid(n), true).unwrap();
    }
    doc.set_favorite(sid(2), pid(1), true).unwrap();
    doc.remove_favorites(sid(1)).unwrap();
    assert!(doc.favorites(sid(1)).is_empty());
    assert_eq!(favorite_numbers(&doc, 2), [1], "the other space is kept");
    doc.remove_favorites(sid(1)).unwrap();
    // A new favourite later: the forgotten ones do not come back.
    doc.set_favorite(sid(1), pid(7), true).unwrap();
    assert_eq!(favorite_numbers(&doc, 1), [7]);
    assert_eq!(favorite_numbers(&reload(&doc, 2), 1), [7]);
}

#[test]
fn concurrent_favourites_of_different_pages_merge() {
    let a = account(1);
    a.set_favorite(sid(1), pid(1), true).unwrap();
    let b = reload(&a, 2);
    a.set_favorite(sid(1), pid(2), true).unwrap();
    b.set_favorite(sid(1), pid(3), true).unwrap();
    sync(&a, &b);
    assert_eq!(a.favorites(sid(1)), b.favorites(sid(1)), "replicas agree");
    let mut all = favorite_numbers(&a, 1);
    assert_eq!(all[0], 1, "the first favourite stays first");
    all.sort_unstable();
    assert_eq!(all, [1, 2, 3]);
}

#[test]
fn two_devices_that_start_a_spaces_favourites_at_once_keep_both() {
    // Neither had a favourite for the space: both create the space's map. It is one map.
    let a = account(1);
    let b = account(2);
    a.set_favorite(sid(1), pid(1), true).unwrap();
    b.set_favorite(sid(1), pid(2), true).unwrap();
    sync(&a, &b);
    assert_eq!(a.favorites(sid(1)), b.favorites(sid(1)));
    let mut all = favorite_numbers(&a, 1);
    all.sort_unstable();
    assert_eq!(all, [1, 2], "neither device's favourite was lost");
}

#[test]
fn concurrent_moves_of_one_favourite_converge() {
    let a = account(1);
    for n in 1..=4 {
        a.set_favorite(sid(1), pid(n), true).unwrap();
    }
    let b = reload(&a, 2);
    a.move_favorite(sid(1), pid(4), Some(pid(1))).unwrap();
    b.move_favorite(sid(1), pid(4), Some(pid(3))).unwrap();
    sync(&a, &b);
    assert_eq!(a.favorites(sid(1)), b.favorites(sid(1)));
    let mut all = favorite_numbers(&a, 1);
    all.sort_unstable();
    assert_eq!(all, [1, 2, 3, 4]);
}

#[test]
fn a_concurrent_remove_and_add_merge_per_page() {
    let a = account(1);
    a.set_favorite(sid(1), pid(1), true).unwrap();
    a.set_favorite(sid(1), pid(2), true).unwrap();
    let b = reload(&a, 2);
    a.set_favorite(sid(1), pid(1), false).unwrap();
    b.set_favorite(sid(1), pid(3), true).unwrap();
    sync(&a, &b);
    assert_eq!(favorite_numbers(&a, 1), [2, 3]);
    assert_eq!(favorite_numbers(&b, 1), [2, 3]);
}

// ----- adopt_favorites (the share step) -----------------------------------------------------

/// A structure document with live pages 1 to `n` and the given favourites, in that order.
fn structure_with(n: u8, favorites: &[u8]) -> StructureDoc {
    let mut doc = structure(1);
    for i in 1..=n {
        add(&mut doc, i, None, &format!("P{i}"));
    }
    for fav in favorites {
        doc.set_favorite(pid(*fav), true).unwrap();
    }
    doc
}

#[test]
fn sharing_moves_the_favourites_out_of_the_structure_document() {
    let mut structure = structure_with(4, &[3, 1, 4]);
    let doc = account(1);
    assert_eq!(doc.adopt_favorites(sid(1), &mut structure).unwrap(), 3);
    assert_eq!(
        favorite_numbers(&doc, 1),
        [3, 1, 4],
        "same order, same positions"
    );
    assert!(
        structure.favorites().is_empty(),
        "the structure map is cleared"
    );
    assert!(!structure.is_favorite(pid(3)));
    assert!(structure.to_portable().get("favorites").is_none());
    assert!(structure.check_invariants().is_empty());
    // Clearing is not an undo step: undoing everything that is on the stack never brings them back.
    while structure.can_undo() {
        structure.undo().unwrap();
        assert!(
            structure.favorites().is_empty(),
            "undo never brings them back"
        );
    }

    // Doing it again finds nothing to move.
    assert_eq!(doc.adopt_favorites(sid(1), &mut structure).unwrap(), 0);
    assert_eq!(favorite_numbers(&doc, 1), [3, 1, 4]);
}

#[test]
fn sharing_also_moves_the_hidden_favourite_of_a_trashed_page() {
    let mut structure = structure_with(3, &[1, 2, 3]);
    structure.trash_page(pid(2), 100).unwrap();
    assert_eq!(structure.favorites().len(), 2, "hidden while trashed");
    let doc = account(1);
    // The account document does not know which pages are live: the hidden entry moves with the
    // others, so a restore after sharing brings the favourite back in its place.
    assert_eq!(doc.adopt_favorites(sid(1), &mut structure).unwrap(), 3);
    assert_eq!(favorite_numbers(&doc, 1), [1, 2, 3]);
    assert!(structure.to_portable().get("favorites").is_none());
}

#[test]
fn favourites_the_account_document_has_already_keep_their_positions() {
    // A second device shared first: its positions win, the missing favourites are added.
    let doc = account(1);
    doc.set_favorite(sid(1), pid(3), true).unwrap();
    doc.set_favorite(sid(1), pid(1), true).unwrap();
    let mut structure = structure_with(4, &[1, 2, 3, 4]);
    assert_eq!(doc.adopt_favorites(sid(1), &mut structure).unwrap(), 4);
    let adopted = favorite_numbers(&doc, 1);
    let position = |n: u8| adopted.iter().position(|x| *x == n).unwrap();
    assert!(position(3) < position(1), "3 stays before 1: {adopted:?}");
    let mut sorted = adopted;
    sorted.sort_unstable();
    assert_eq!(sorted, [1, 2, 3, 4]);
    assert!(structure.favorites().is_empty());
}

#[test]
fn favourites_of_other_spaces_are_untouched_by_sharing() {
    let doc = account(1);
    doc.set_favorite(sid(2), pid(9), true).unwrap();
    let mut structure = structure_with(2, &[1, 2]);
    doc.adopt_favorites(sid(1), &mut structure).unwrap();
    assert_eq!(favorite_numbers(&doc, 2), [9]);
    assert_eq!(favorite_numbers(&doc, 1), [1, 2]);
}

#[test]
fn two_devices_that_share_the_same_space_agree_on_the_favourites() {
    let a = account(1);
    let b = reload(&a, 2);
    let mut structure_a = structure_with(4, &[2, 4, 1]);
    let snapshot = structure_a.doc().export_snapshot().unwrap();
    let mut structure_b =
        StructureDoc::from_doc(Doc::load(DocKind::Structure, 2, Some(&snapshot), &[]).unwrap())
            .unwrap();
    a.adopt_favorites(sid(1), &mut structure_a).unwrap();
    b.adopt_favorites(sid(1), &mut structure_b).unwrap();
    sync(&a, &b);
    assert_eq!(a.favorites(sid(1)), b.favorites(sid(1)));
    assert_eq!(
        favorite_numbers(&a, 1),
        [2, 4, 1],
        "the order was kept by both"
    );
}

// ----- verifiedPeople -----------------------------------------------------------------------

#[test]
fn people_are_marked_verified_listed_by_account_id_and_unmarked() {
    let doc = account(1);
    doc.set_verified(aid(3), Some(300)).unwrap();
    doc.set_verified(aid(1), Some(100)).unwrap();
    doc.set_verified(aid(2), Some(200)).unwrap();
    assert_eq!(doc.verified_at(aid(2)), Some(200));
    assert_eq!(doc.verified_at(aid(9)), None);
    assert_eq!(
        doc.verified_people(),
        [(aid(1), 100), (aid(2), 200), (aid(3), 300)],
        "ordered by account id"
    );
    doc.set_verified(aid(2), None).unwrap();
    doc.set_verified(aid(2), None).unwrap();
    assert_eq!(doc.verified_at(aid(2)), None);
    assert_eq!(doc.verified_people().len(), 2);
    assert_eq!(reload(&doc, 2).verified_people(), doc.verified_people());
}

#[test]
fn marking_a_verified_account_again_keeps_the_earlier_time() {
    let doc = account(1);
    doc.set_verified(aid(1), Some(100)).unwrap();
    let version = doc.doc().version();
    doc.set_verified(aid(1), Some(200)).unwrap();
    assert_eq!(doc.verified_at(aid(1)), Some(100));
    assert_eq!(doc.doc().version(), version, "a later mark writes nothing");
    doc.set_verified(aid(1), Some(50)).unwrap();
    assert_eq!(doc.verified_at(aid(1)), Some(50), "the earlier of the two");
    // Unmarking and marking again starts over.
    doc.set_verified(aid(1), None).unwrap();
    doc.set_verified(aid(1), Some(500)).unwrap();
    assert_eq!(doc.verified_at(aid(1)), Some(500));
}

#[test]
fn verified_entries_that_do_not_parse_are_ignored() {
    let doc = account(1);
    doc.set_verified(aid(1), Some(100)).unwrap();
    let map = doc.doc().inner().get_map(VERIFIED_PEOPLE_MAP);
    map.insert("not a uuid", 5).unwrap();
    map.insert(&aid(2).to_string(), "yesterday").unwrap();
    doc.doc().inner().commit();
    assert_eq!(doc.verified_people(), [(aid(1), 100)]);
    assert_eq!(doc.verified_at(aid(2)), None);
    // A write over the damaged value works.
    doc.set_verified(aid(2), Some(7)).unwrap();
    assert_eq!(doc.verified_at(aid(2)), Some(7));
}

#[test]
fn two_devices_that_verify_different_people_both_keep_theirs() {
    let a = account(1);
    let b = account(2);
    a.set_verified(aid(1), Some(10)).unwrap();
    b.set_verified(aid(2), Some(20)).unwrap();
    sync(&a, &b);
    let expected = [(aid(1), 10), (aid(2), 20)];
    assert_eq!(a.verified_people(), expected);
    assert_eq!(b.verified_people(), expected);

    // A concurrent unmark and a new mark merge per person.
    a.set_verified(aid(1), None).unwrap();
    b.set_verified(aid(3), Some(30)).unwrap();
    sync(&a, &b);
    assert_eq!(a.verified_people(), [(aid(2), 20), (aid(3), 30)]);
    assert_eq!(b.verified_people(), a.verified_people());
}

#[test]
fn a_whole_account_document_round_trips_through_updates() {
    // Everything above, written on one device, arrives on a fresh one through updates only.
    let a = account(1);
    a.add_space(sid(1)).unwrap();
    a.set_shared_space(sid(1), fid(1)).unwrap();
    a.set_profile_name("Alice").unwrap();
    a.set_space_ai(sid(1), true).unwrap();
    a.set_favorite(sid(1), pid(1), true).unwrap();
    a.set_favorite(sid(1), pid(2), true).unwrap();
    a.set_verified(aid(1), Some(1)).unwrap();
    let blob = a.doc().export_updates_since(None).unwrap().unwrap();
    let b = AccountDoc::from_doc(Doc::load(DocKind::Account, 2, None, &[blob]).unwrap()).unwrap();
    assert_eq!(b.spaces(), [sid(1)]);
    assert_eq!(b.shared_space(sid(1)), Some(fid(1)));
    assert_eq!(b.profile_name().as_deref(), Some("Alice"));
    assert_eq!(b.space_ai(sid(1)), Some(true));
    assert_eq!(favorite_numbers(&b, 1), [1, 2]);
    assert_eq!(b.verified_at(aid(1)), Some(1));
    assert_eq!(shape(&a), shape(&b));
}
