//! Trash expiry (ADR-0040 item 3): the setting `app.trash.retention_days`, which entries expire and
//! when, that an expiry is the ordinary purge, and that it runs at open and unlock. (Expiry reaching
//! the other devices is in `tests_sync`, the release of attachments in `tests_attachments`.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kb_storage::TextIndexQueries;
use kb_types::testing::FixedClock;
use kb_types::{ErrorCode, Event, Json, PageId, Request, Response, SearchHit, SpaceId};
use serde_json::json;

use crate::test_support::{Fixture, create_page, create_workspace, page, spaces, tree, write_text};
use crate::{Core, DEFAULT_TRASH_RETENTION_DAYS, Opened, TRASH_RETENTION_SETTING};

const DAY: i64 = 86_400_000;
/// A plausible moment (2001-09-09), far from zero so that no limit reaches back past the epoch.
const T0: i64 = 1_000_000_000_000;

/// Moves the core's clock (tests live inside the crate, so the field is theirs to swap).
fn set_now(core: &mut Core, now_ms: i64) {
    core.clock = Box::new(FixedClock::new(now_ms));
}

/// A workspace with one space, opened at `T0`.
fn setup() -> (Fixture, Core, SpaceId) {
    let fx = Fixture::new();
    let mut core = fx.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    (fx, core, space)
}

fn trash(core: &mut Core, page_id: PageId) {
    core.dispatch(Request::PageTrash { page_id }).unwrap();
}

fn restore(core: &mut Core, page_id: PageId) {
    core.dispatch(Request::PageRestore { page_id }).unwrap();
}

fn set_days(core: &mut Core, value: Json) {
    core.dispatch(Request::SettingsSet {
        key: TRASH_RETENTION_SETTING.into(),
        value,
    })
    .unwrap();
}

/// The top-level entries of a space's trash.
fn trashed(core: &mut Core, space: SpaceId) -> Vec<PageId> {
    tree(core, space)
        .trashed
        .iter()
        .map(|entry| entry.page.id)
        .collect()
}

fn exists(core: &mut Core, page_id: PageId) -> bool {
    core.dispatch(Request::PageGet { page_id }).is_ok()
}

fn tombstoned(core: &Core, space: SpaceId, page_id: PageId) -> Option<i64> {
    core.docs.structures[&space]
        .model
        .tombstones()
        .into_iter()
        .find(|(id, _)| *id == page_id)
        .map(|(_, at)| at)
}

fn search(core: &mut Core, query: &str) -> Vec<SearchHit> {
    match core
        .dispatch(Request::Search {
            query: query.into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_page_stays_for_thirty_days_and_goes_a_millisecond_after() {
    assert_eq!(DEFAULT_TRASH_RETENTION_DAYS, 30);
    let (_fx, mut core, space) = setup();
    let old = create_page(&mut core, space, None, "Old").id;
    trash(&mut core, old);
    assert_eq!(core.trash_expire().unwrap(), 0, "just trashed");

    set_now(&mut core, T0 + 30 * DAY);
    assert_eq!(
        core.trash_expire().unwrap(),
        0,
        "exactly the limit is not older"
    );
    assert_eq!(trashed(&mut core, space), [old]);
    assert!(tombstoned(&core, space, old).is_none());

    set_now(&mut core, T0 + 30 * DAY + 1);
    assert_eq!(core.trash_expire().unwrap(), 1);
    assert!(trashed(&mut core, space).is_empty());
    assert!(!exists(&mut core, old));
    assert_eq!(
        tombstoned(&core, space, old),
        Some(T0 + 30 * DAY + 1),
        "the tombstone is what sync carries"
    );
    assert_eq!(core.trash_expire().unwrap(), 0, "nothing is left to do");
}

#[test]
fn live_pages_are_never_expired() {
    let (_fx, mut core, space) = setup();
    let live = create_page(&mut core, space, None, "Live").id;
    set_now(&mut core, T0 + 400 * DAY);
    assert_eq!(core.trash_expire().unwrap(), 0);
    assert!(exists(&mut core, live));
}

#[test]
fn zero_keeps_trashed_pages_forever() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Kept").id;
    set_days(&mut core, json!(0));
    trash(&mut core, page);
    set_now(&mut core, T0 + 3650 * DAY);
    assert_eq!(core.trash_expire().unwrap(), 0);
    assert_eq!(trashed(&mut core, space), [page]);
}

#[test]
fn the_setting_is_the_number_of_days() {
    // (value, a moment just inside the limit, the first moment past it)
    let cases = [
        (json!(7), 7 * DAY, 7 * DAY + 1),
        (json!(1), DAY, DAY + 1),
        (json!(90), 90 * DAY, 90 * DAY + 1),
        // Fractions are allowed, as for the attachments' retention.
        (json!(0.5), DAY / 2, DAY / 2 + 1),
        // Not a number: the default, not a deletion.
        (json!("seven"), 30 * DAY, 30 * DAY + 1),
        (json!(null), 30 * DAY, 30 * DAY + 1),
        (json!({ "days": 1 }), 30 * DAY, 30 * DAY + 1),
    ];
    for (value, inside, past) in cases {
        let (_fx, mut core, space) = setup();
        let page = create_page(&mut core, space, None, "Page").id;
        set_days(&mut core, value.clone());
        trash(&mut core, page);
        set_now(&mut core, T0 + inside);
        assert_eq!(core.trash_expire().unwrap(), 0, "{value}: at the limit");
        set_now(&mut core, T0 + past);
        assert_eq!(core.trash_expire().unwrap(), 1, "{value}: past the limit");
    }
}

#[test]
fn a_negative_or_unusable_limit_keeps_everything() {
    for value in [json!(-1), json!(-0.5), json!(-30)] {
        let (_fx, mut core, space) = setup();
        let page = create_page(&mut core, space, None, "Page").id;
        set_days(&mut core, value.clone());
        trash(&mut core, page);
        set_now(&mut core, T0 + 3650 * DAY);
        assert_eq!(core.trash_expire().unwrap(), 0, "{value}");
        assert_eq!(trashed(&mut core, space), [page], "{value}");
    }
}

#[test]
fn a_huge_limit_is_never_reached() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Page").id;
    set_days(&mut core, json!(1e300));
    trash(&mut core, page);
    set_now(&mut core, i64::MAX / 2);
    assert_eq!(core.trash_expire().unwrap(), 0);
    assert_eq!(trashed(&mut core, space), [page]);
}

/// The check at open compares the text index's page rows with the pages of the structure. The sub-pages of a
/// trashed page have rows and are pages of the structure, so a trashed page with sub-pages must not read as "out
/// of sync" (which rebuilt the index at every open, found by the real-app pass of ADR-0040).
#[test]
fn a_trashed_page_with_sub_pages_leaves_the_text_index_in_step_with_the_tree() {
    let (_fx, mut core, space) = setup();
    let top = create_page(&mut core, space, None, "Orchard").id;
    let child = create_page(&mut core, space, Some(top), "Apples").id;
    create_page(&mut core, space, Some(child), "Cox");
    create_page(&mut core, space, None, "Other");
    assert!(core.spaces_with_stale_text_index().unwrap().is_empty());
    assert_eq!(core.db.fts_page_count(space).unwrap(), 4);

    // Three of the four pages are in the trash, one of them as the top of its subtree.
    trash(&mut core, top);
    assert_eq!(core.db.fts_page_count(space).unwrap(), 4);
    assert!(
        core.spaces_with_stale_text_index().unwrap().is_empty(),
        "a trashed page and its sub-pages keep their rows: nothing to rebuild"
    );
    restore(&mut core, top);
    assert!(core.spaces_with_stale_text_index().unwrap().is_empty());

    // A control: a row that really went missing is still found.
    core.db.transaction(|tx| tx.fts_delete_page(child)).unwrap();
    assert_eq!(
        core.spaces_with_stale_text_index().unwrap(),
        vec![(space, 3, 4)]
    );
    // And the repair at open puts it right.
    core.verify_text_index().unwrap();
    assert!(core.spaces_with_stale_text_index().unwrap().is_empty());
}

#[test]
fn only_entries_of_the_trash_count_and_a_subtree_goes_whole() {
    let (_fx, mut core, space) = setup();
    let other = create_page(&mut core, space, None, "Other").id;
    let parent = create_page(&mut core, space, None, "Parent").id;
    let kid = create_page(&mut core, space, Some(parent), "Kid").id;
    let grand = create_page(&mut core, space, Some(kid), "Grand").id;
    let early = create_page(&mut core, space, Some(parent), "Early").id;

    // `early` is trashed first, on its own: it is an entry of its own and keeps its time when
    // its parent goes to the trash later, with the rest of the subtree.
    trash(&mut core, early);
    set_now(&mut core, T0 + 5 * DAY);
    trash(&mut core, other);
    set_now(&mut core, T0 + 20 * DAY);
    trash(&mut core, parent);
    let entries = tree(&mut core, space).trashed;
    assert_eq!(entries.len(), 3);
    let parent_entry = entries.iter().find(|e| e.page.id == parent).unwrap();
    assert_eq!(parent_entry.descendant_count, 2);
    // The descendants carry the parent's time, so none of them is a reason to expire it.
    assert_eq!(page(&mut core, grand).trashed_at, Some(T0 + 20 * DAY));

    set_now(&mut core, T0 + 31 * DAY);
    assert_eq!(
        core.trash_expire().unwrap(),
        1,
        "early, 31 days in the trash"
    );
    assert!(!exists(&mut core, early));
    assert!(exists(&mut core, other) && exists(&mut core, parent));
    set_now(&mut core, T0 + 35 * DAY + 1);
    assert_eq!(
        core.trash_expire().unwrap(),
        1,
        "other, 30 days and a millisecond"
    );
    assert!(!exists(&mut core, other));
    assert_eq!(trashed(&mut core, space), [parent]);
    assert!(exists(&mut core, kid) && exists(&mut core, grand));

    set_now(&mut core, T0 + 50 * DAY + 1);
    assert_eq!(core.trash_expire().unwrap(), 1, "one entry, three pages");
    for gone in [parent, kid, grand] {
        assert!(!exists(&mut core, gone));
        assert!(tombstoned(&core, space, gone).is_some());
    }
    assert!(core.docs.structures[&space].model.all_page_ids().is_empty());
}

#[test]
fn a_restored_page_that_is_trashed_again_starts_over() {
    let (_fx, mut core, space) = setup();
    let page = create_page(&mut core, space, None, "Back and forth").id;
    trash(&mut core, page);
    set_now(&mut core, T0 + 20 * DAY);
    restore(&mut core, page);
    set_now(&mut core, T0 + 25 * DAY);
    trash(&mut core, page);

    set_now(&mut core, T0 + 31 * DAY);
    assert_eq!(core.trash_expire().unwrap(), 0, "it was trashed 6 days ago");
    set_now(&mut core, T0 + 55 * DAY);
    assert_eq!(
        core.trash_expire().unwrap(),
        0,
        "30 days to the millisecond"
    );
    set_now(&mut core, T0 + 55 * DAY + 1);
    assert_eq!(core.trash_expire().unwrap(), 1);
    assert!(!exists(&mut core, page));
}

#[test]
fn every_space_is_swept() {
    let (_fx, mut core, first) = setup();
    let second = match core
        .dispatch(Request::SpaceCreate {
            name: "Work".into(),
            icon: None,
        })
        .unwrap()
    {
        Response::Space(space) => space.id,
        other => panic!("unexpected {other:?}"),
    };
    let a = create_page(&mut core, first, None, "A").id;
    let b = create_page(&mut core, second, None, "B").id;
    let c = create_page(&mut core, second, None, "C").id;
    trash(&mut core, a);
    trash(&mut core, b);
    set_now(&mut core, T0 + 10 * DAY);
    trash(&mut core, c);

    set_now(&mut core, T0 + 31 * DAY);
    assert_eq!(core.trash_expire().unwrap(), 2);
    assert!(trashed(&mut core, first).is_empty());
    assert_eq!(trashed(&mut core, second), [c]);
}

#[test]
fn an_expiry_is_a_page_purge_with_the_same_effects() {
    // Two identical workspaces (same seed, same steps): one page purged by hand, one expired.
    fn build() -> (Fixture, Core, SpaceId, PageId, PageId) {
        let (fx, mut core, space) = setup();
        let top = create_page(&mut core, space, None, "Marmalade recipes").id;
        let child = create_page(&mut core, space, Some(top), "Seville oranges").id;
        write_text(&mut core, top, "marmalade needs bitter oranges", 1);
        write_text(&mut core, child, "seville season is January", 2);
        assert!(
            search(&mut core, "marmalade")
                .iter()
                .any(|hit| hit.page_id == top)
        );
        assert!(
            search(&mut core, "seville")
                .iter()
                .any(|hit| hit.page_id == child)
        );
        trash(&mut core, top);
        core.drain_events();
        (fx, core, space, top, child)
    }
    let later = T0 + 31 * DAY;

    let (_fx_a, mut by_hand, space, top, child) = build();
    set_now(&mut by_hand, later);
    by_hand
        .dispatch(Request::PagePurge { page_id: top })
        .unwrap();

    let (_fx_b, mut expired, space_b, top_b, child_b) = build();
    assert_eq!((space, top, child), (space_b, top_b, child_b));
    set_now(&mut expired, later);
    assert_eq!(expired.trash_expire().unwrap(), 1);

    assert_eq!(
        expired.docs.structures[&space].model.to_portable(),
        by_hand.docs.structures[&space].model.to_portable(),
        "the same structure document, tombstones included"
    );
    assert_eq!(
        expired.security.keyring.counts(),
        by_hand.security.keyring.counts(),
        "the keys of both pages are erased the same way"
    );
    assert!(expired.security.keyring.counts().tombstones >= 2);
    for core in [&mut expired, &mut by_hand] {
        assert!(search(core, "seville").is_empty());
        assert!(search(core, "marmalade").is_empty());
    }
}

#[test]
fn the_events_are_those_of_a_purge_and_each_comes_once() {
    let (_fx, mut core, space) = setup();
    let pages: Vec<PageId> = (0..3)
        .map(|n| create_page(&mut core, space, None, &format!("Page {n}")).id)
        .collect();
    for page in &pages {
        trash(&mut core, *page);
    }
    core.drain_events();

    set_now(&mut core, T0 + 31 * DAY);
    assert_eq!(core.trash_expire().unwrap(), 3);
    let events = core.drain_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == Event::StructureChanged { space_id: space })
            .count(),
        1,
        "{events:?}"
    );
    assert!(
        !events.contains(&Event::SpacesChanged),
        "the live pages did not change: {events:?}"
    );

    // Nothing to do, nothing announced.
    assert_eq!(core.trash_expire().unwrap(), 0);
    assert!(core.drain_events().is_empty());
}

#[test]
fn open_expires_what_is_overdue_and_keeps_the_rest() {
    let fx = Fixture::new();
    let (old, recent, live) = {
        let mut core = fx.open(1, T0);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = spaces(&mut core)[0].id;
        let old = create_page(&mut core, space, None, "Old").id;
        let recent = create_page(&mut core, space, None, "Recent").id;
        let live = create_page(&mut core, space, None, "Live").id;
        trash(&mut core, old);
        set_now(&mut core, T0 + 20 * DAY);
        trash(&mut core, recent);
        (old, recent, live)
    };

    // Opened 31 days after the first page was trashed, 11 after the second.
    let mut core = Core::open(fx.deps(2, T0 + 31 * DAY)).unwrap();
    assert!(!exists(&mut core, old));
    assert!(exists(&mut core, recent), "11 days in the trash");
    assert!(exists(&mut core, live));
    let space = spaces(&mut core)[0].id;
    assert_eq!(trashed(&mut core, space), [recent]);
    assert!(tombstoned(&core, space, old).is_some());
    assert!(
        core.drain_events()
            .contains(&Event::StructureChanged { space_id: space }),
        "the open announces the purge like any other"
    );

    // And the shorter limit of this device applies at the next open.
    set_days(&mut core, json!(5));
    drop(core);
    let mut core = Core::open(fx.deps(3, T0 + 31 * DAY)).unwrap();
    assert!(!exists(&mut core, recent));
    assert!(exists(&mut core, live));
}

#[test]
fn open_keeps_everything_when_the_limit_is_zero() {
    let fx = Fixture::new();
    let page = {
        let mut core = fx.open(1, T0);
        create_workspace(&mut core, "Home", Some("Personal"));
        let space = spaces(&mut core)[0].id;
        let page = create_page(&mut core, space, None, "Kept").id;
        set_days(&mut core, json!(0));
        trash(&mut core, page);
        page
    };
    let mut core = Core::open(fx.deps(2, T0 + 5000 * DAY)).unwrap();
    assert!(exists(&mut core, page));
}

#[test]
fn nothing_expires_while_locked_and_the_unlock_catches_up() {
    const PASSPHRASE: &str = "correct horse battery";
    let fx = Fixture::new();
    let mut core = fx.open(1, T0);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = spaces(&mut core)[0].id;
    let page = create_page(&mut core, space, None, "Secret").id;
    trash(&mut core, page);
    core.dispatch(Request::AppLockEnable {
        passphrase: PASSPHRASE.into(),
    })
    .unwrap();
    let mut locked = core.lock().map_err(|(_, e)| e).unwrap();

    // A launch long after: the workspace stays closed, and no page is touched.
    let Opened::Locked(still_locked) = Core::open_or_locked(fx.deps(2, T0 + 90 * DAY)).unwrap()
    else {
        panic!("expected a locked core");
    };
    drop(still_locked);
    assert_eq!(
        Core::open(fx.deps(2, T0 + 90 * DAY)).err().unwrap().code,
        ErrorCode::Locked
    );

    // A wrong passphrase opens nothing either.
    locked.deps.clock = Box::new(FixedClock::new(T0 + 90 * DAY));
    let (locked, err) = locked.unlock("not the passphrase").err().unwrap();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    let mut core = locked.unlock(PASSPHRASE).map_err(|(_, e)| e).unwrap();
    assert!(!exists(&mut core, page), "the unlock expired it");
    assert!(tombstoned(&core, space, page).is_some());
}

#[test]
fn a_clock_that_went_backwards_expires_nothing() {
    let (_fx, mut core, space) = setup();
    let page_id = create_page(&mut core, space, None, "Page").id;
    trash(&mut core, page_id);
    // A wrong clock, earlier than the moment the page was trashed (or than the epoch).
    for now in [T0 - 60 * DAY, 0, -DAY] {
        set_now(&mut core, now);
        assert_eq!(core.trash_expire().unwrap(), 0, "{now}");
    }
    assert_eq!(trashed(&mut core, space), [page_id]);
}
