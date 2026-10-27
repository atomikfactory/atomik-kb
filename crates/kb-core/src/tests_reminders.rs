//! Reminders on date mentions (ADR-0024): the derived `date_mentions` table kept in step with
//! the documents, the `RemindersDue` query with the `last_checked` mark and its seven-day
//! window, and the scheduler tick that emits each reminder once.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

use kb_doc::{Doc, DocKind};
use kb_model::PageDoc;
use kb_storage::{DateMentionQueries, DateMentionRow};
use kb_types::{
    BlockId, Clock, Event, Json, PageId, ReminderDue, Request, Response, SpaceId, testing,
};
use serde_json::json;

use crate::test_support::{Fixture, create_page, create_space, create_workspace};
use crate::{CATCH_UP_SECONDS, Core, FIRES_PER_TICK, LAST_CHECKED_SETTING, MAX_REMINDERS_DUE};

/// A clock the test moves while the core runs, with a settable local offset.
#[derive(Clone)]
struct TestClock {
    now_ms: Arc<AtomicI64>,
    offset_seconds: Arc<AtomicI32>,
}

impl TestClock {
    /// Starts at `local_seconds` on the wall clock, at the given offset from UTC.
    fn at(local_seconds: i64, offset_seconds: i32) -> Self {
        Self {
            now_ms: Arc::new(AtomicI64::new(
                (local_seconds - i64::from(offset_seconds)) * 1000,
            )),
            offset_seconds: Arc::new(AtomicI32::new(offset_seconds)),
        }
    }

    /// Sets the wall clock reading (the offset stays).
    fn set_local(&self, local_seconds: i64) {
        let offset = i64::from(self.offset_seconds.load(Ordering::SeqCst));
        self.now_ms
            .store((local_seconds - offset) * 1000, Ordering::SeqCst);
    }

    fn advance(&self, seconds: i64) {
        self.now_ms.fetch_add(seconds * 1000, Ordering::SeqCst);
    }

    fn set_offset(&self, offset_seconds: i32) {
        self.offset_seconds.store(offset_seconds, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }

    fn local_offset_seconds(&self) -> i32 {
        self.offset_seconds.load(Ordering::SeqCst)
    }
}

/// 2026-10-07 14:30 on the wall clock, in seconds since 1970-01-01T00:00:00.
const OCT_7_1430: i64 = 1_791_383_400;
const MINUTE: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn block(n: u8) -> BlockId {
    BlockId::from_random_bytes([n; 16])
}

/// A core over `fx` with the test clock.
fn open(fx: &Fixture, seed: u64, clock: &TestClock) -> Core {
    let mut deps = fx.deps(seed, 0);
    deps.clock = Box::new(clock.clone());
    Core::open(deps).unwrap()
}

/// A workspace with one space and one page; returns the core, the space and the page.
fn setup(fx: &Fixture, clock: &TestClock) -> (Core, SpaceId, PageId) {
    let mut core = open(fx, 1, clock);
    create_workspace(&mut core, "Home", Some("Personal"));
    let space = first_space(&mut core);
    let page = create_page(&mut core, space, None, "Plan").id;
    core.page_doc_open(page).unwrap();
    core.drain_events();
    (core, space, page)
}

fn first_space(core: &mut Core) -> SpaceId {
    match core.dispatch(Request::SpaceList).unwrap() {
        Response::Spaces(spaces) => spaces[0].id,
        other => panic!("unexpected {other:?}"),
    }
}

/// A mention: (node, date, time, reminder minutes).
type Mention = (u8, &'static str, Option<&'static str>, Option<i64>);

/// A page of paragraphs: (paragraph number, text before the mentions, mentions).
fn page_json(paragraphs: &[(u8, &str, &[Mention])]) -> Json {
    let content: Vec<Json> = paragraphs
        .iter()
        .map(|(id, text, mentions)| {
            let mut inline = vec![json!({ "type": "text", "text": text })];
            for (node, date, time, reminder) in *mentions {
                let mut attrs = json!({ "id": block(*node).to_string(), "date": date });
                if let Some(time) = time {
                    attrs["time"] = json!(time);
                }
                if let Some(reminder) = reminder {
                    attrs["reminder"] = json!(reminder);
                }
                inline.push(json!({ "type": "dateMention", "attrs": attrs }));
                inline.push(json!({ "type": "text", "text": " " }));
            }
            json!({ "type": "paragraph", "attrs": { "id": block(*id).to_string() }, "content": inline })
        })
        .collect();
    json!({ "type": "doc", "content": content })
}

/// Sends `content` to the core as an editor's update (a replica built from it merges next to
/// the page's own first paragraph, as in the other page tests).
fn import_page(core: &mut Core, page: PageId, content: &Json, peer: u64) {
    let replica = PageDoc::from_portable(Doc::new(DocKind::Page, peer), content).unwrap();
    let blob = replica.doc().export_updates_since(None).unwrap().unwrap();
    let (_, payload) = kb_doc::decode_blob(&blob).unwrap();
    core.page_doc_import(page, payload).unwrap();
}

/// Replaces the text of a block through an editor replica of the core's document (which drops
/// every mention inside it).
fn clear_block(core: &mut Core, page: PageId, block_id: BlockId, peer: u64) {
    let snapshot = core.page_doc_open(page).unwrap();
    let blob = kb_doc::encode_blob(kb_doc::BlobKind::Snapshot, &snapshot);
    let replica =
        PageDoc::from_doc(Doc::load(DocKind::Page, peer, Some(&blob), &[]).unwrap()).unwrap();
    let before = replica.doc().version();
    replica.set_block_text(block_id, "cleared").unwrap();
    let update = replica
        .doc()
        .export_updates_since(Some(&before))
        .unwrap()
        .unwrap();
    let (_, payload) = kb_doc::decode_blob(&update).unwrap();
    core.page_doc_import(page, payload).unwrap();
}

fn rows(core: &Core, page: PageId) -> Vec<DateMentionRow> {
    core.db.date_mentions_of(page).unwrap()
}

fn due_events(events: &[Event]) -> Vec<ReminderDue> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ReminderDue(due) => Some(due.clone()),
            _ => None,
        })
        .collect()
}

fn reminders_due(core: &mut Core, until: i64) -> Vec<ReminderDue> {
    match core.dispatch(Request::RemindersDue { until }).unwrap() {
        Response::Reminders(list) => list,
        other => panic!("unexpected {other:?}"),
    }
}

fn setting(core: &mut Core, key: &str) -> Option<Json> {
    match core
        .dispatch(Request::SettingsGet { key: key.into() })
        .unwrap()
    {
        Response::Setting(value) => value,
        other => panic!("unexpected {other:?}"),
    }
}

fn nodes(list: &[ReminderDue]) -> Vec<BlockId> {
    list.iter().map(|due| due.node_id).collect()
}

/// A mention of 2026-10-07 at 14:30 with a reminder `minutes` before it fires at
/// `OCT_7_1430 - minutes * 60`.
fn at_1430(node: u8, minutes: i64) -> Mention {
    (node, "2026-10-07", Some("14:30"), Some(minutes))
}

// ----- the derived table ----------------------------------------------------------------

#[test]
fn the_table_follows_the_document_on_every_save() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);

    // Add: two mentions, one with a reminder, with their block and their fire time.
    let added = page_json(&[(7, "Call ", &[at_1430(8, 15), (9, "2026-10-08", None, None)])]);
    import_page(&mut core, page, &added, 0xA1);
    assert_eq!(
        rows(&core, page),
        [
            DateMentionRow {
                block_id: block(7),
                node_id: block(8),
                date: "2026-10-07".into(),
                time: Some("14:30".into()),
                reminder_minutes: Some(15),
                fire_at_local: Some(OCT_7_1430 - 15 * MINUTE),
                reminder_for: None,
            },
            DateMentionRow {
                block_id: block(7),
                node_id: block(9),
                date: "2026-10-08".into(),
                time: None,
                reminder_minutes: None,
                fire_at_local: None,
                reminder_for: None,
            },
        ]
    );

    // Change: another paragraph with a different reminder; the cleared block's rows go.
    clear_block(&mut core, page, block(7), 0xA2);
    assert!(
        rows(&core, page).is_empty(),
        "the mentions went with the text"
    );
    let changed = page_json(&[(10, "Call ", &[at_1430(11, 60)])]);
    import_page(&mut core, page, &changed, 0xA3);
    assert_eq!(
        rows(&core, page),
        [DateMentionRow {
            block_id: block(10),
            node_id: block(11),
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
            reminder_minutes: Some(60),
            fire_at_local: Some(OCT_7_1430 - HOUR),
            reminder_for: None,
        }]
    );

    // Remove.
    clear_block(&mut core, page, block(10), 0xA4);
    assert!(rows(&core, page).is_empty());
}

#[test]
fn a_page_without_mentions_has_no_rows_and_other_pages_keep_theirs() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, space, first) = setup(&fx, &clock);
    let second = create_page(&mut core, space, None, "Other").id;
    core.page_doc_open(second).unwrap();
    import_page(
        &mut core,
        first,
        &page_json(&[(7, "a ", &[at_1430(8, 0)])]),
        0xB1,
    );
    import_page(
        &mut core,
        second,
        &page_json(&[(7, "b ", &[at_1430(9, 5)])]),
        0xB2,
    );
    assert_eq!(rows(&core, first).len(), 1);
    assert_eq!(rows(&core, second).len(), 1);
    clear_block(&mut core, first, block(7), 0xB3);
    assert!(rows(&core, first).is_empty());
    assert_eq!(rows(&core, second).len(), 1, "the other page is untouched");
}

#[test]
fn a_damaged_reminder_is_stored_as_no_reminder() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    let content = json!({ "type": "doc", "content": [{
        "type": "paragraph", "attrs": { "id": block(7).to_string() },
        "content": [
            { "type": "dateMention", "attrs": { "id": block(8).to_string(), "date": "2026-10-07", "reminder": -5 } },
            { "type": "dateMention", "attrs": { "id": block(9).to_string(), "date": "2026-10-07", "reminder": 1.5 } },
            // More minutes than the table keeps: no reminder rather than a failed save.
            { "type": "dateMention", "attrs": { "id": block(10).to_string(), "date": "2026-10-07", "reminder": 90_000_000_000_i64 } }
        ]
    }]});
    import_page(&mut core, page, &content, 0xC1);
    let stored = rows(&core, page);
    assert_eq!(stored.len(), 3);
    assert!(
        stored
            .iter()
            .all(|row| row.reminder_minutes.is_none() && row.fire_at_local.is_none())
    );
}

#[test]
fn trashing_hides_the_reminders_restoring_brings_them_back_and_purging_drops_the_rows() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "Call ", &[at_1430(8, 0)])]),
        0xD1,
    );
    let until = OCT_7_1430 + HOUR;
    assert_eq!(nodes(&reminders_due(&mut core, until)), [block(8)]);

    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    assert!(reminders_due(&mut core, until).is_empty(), "trashed");
    assert_eq!(rows(&core, page).len(), 1, "the rows wait for a restore");

    core.dispatch(Request::PageRestore { page_id: page })
        .unwrap();
    assert_eq!(nodes(&reminders_due(&mut core, until)), [block(8)]);

    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    core.dispatch(Request::PagePurge { page_id: page }).unwrap();
    assert!(rows(&core, page).is_empty(), "purged");
    assert!(reminders_due(&mut core, until).is_empty());
}

#[test]
fn index_rebuild_recreates_the_table_from_the_documents() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "Call ", &[at_1430(8, 30)])]),
        0xE1,
    );
    let before = rows(&core, page);
    assert_eq!(before.len(), 1);

    core.db.date_mentions_clear().unwrap();
    assert!(rows(&core, page).is_empty());
    core.dispatch(Request::IndexRebuild { space_id: None })
        .unwrap();
    assert_eq!(rows(&core, page), before);
}

#[test]
fn a_database_from_before_schema_v9_is_indexed_once_at_open() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "Call ", &[at_1430(8, 30)])]),
        0xF1,
    );
    let before = rows(&core, page);
    // What the migration of a v8 file leaves: the table empty, the marker not there.
    core.db.date_mentions_clear().unwrap();
    core.db
        .connection()
        .execute("DELETE FROM meta WHERE key = 'date_mentions_indexed'", [])
        .unwrap();
    drop(core);

    let core = open(&fx, 2, &clock);
    assert_eq!(rows(&core, page), before, "filled from the documents");
    assert_eq!(
        core.db
            .meta_get("date_mentions_indexed")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    drop(core);

    // The marker holds: a table that was emptied afterwards is not refilled at open.
    let core = open(&fx, 3, &clock);
    core.db.date_mentions_clear().unwrap();
    drop(core);
    let core = open(&fx, 4, &clock);
    assert!(rows(&core, page).is_empty());
}

// ----- RemindersDue and the mark ----------------------------------------------------------

#[test]
fn reminders_due_lists_what_fell_due_with_the_page_and_the_block_text() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, space, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[
            (7, "Call the dentist ", &[at_1430(8, 15)]),
            (9, "No reminder ", &[(10, "2026-10-07", None, None)]),
            (11, "A day ", &[(12, "2026-10-08", None, Some(0))]),
        ]),
        0x11,
    );

    core.drain_events();

    // 14:15 is the first fire time: nothing before it, one at it, the whole day's one later.
    assert!(reminders_due(&mut core, OCT_7_1430 - 16 * MINUTE).is_empty());
    let at_1415 = reminders_due(&mut core, OCT_7_1430 - 15 * MINUTE);
    assert_eq!(
        at_1415,
        [ReminderDue {
            page_id: page,
            space_id: space,
            block_id: block(7),
            node_id: block(8),
            page_title: "Plan".into(),
            text: "Call the dentist 2026-10-07 14:30 ".into(),
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
            reminder_minutes: 15,
            fire_at: OCT_7_1430 - 15 * MINUTE,
        }]
    );
    // 2026-10-08 at 09:00 (a day without a time), 18 hours 30 minutes after 14:30.
    let both = reminders_due(&mut core, OCT_7_1430 + 18 * HOUR + 30 * MINUTE);
    assert_eq!(nodes(&both), [block(8), block(12)]);
    assert_eq!(both[1].time, None);
    assert_eq!(both[1].reminder_minutes, 0);
    assert_eq!(both[1].fire_at, OCT_7_1430 + 18 * HOUR + 30 * MINUTE);
    // A pure query: asking twice gives the same answer, and no mark was written.
    assert_eq!(reminders_due(&mut core, OCT_7_1430 + DAY), both);
    assert_eq!(setting(&mut core, LAST_CHECKED_SETTING), None);
    assert!(core.drain_events().is_empty(), "no event either");
}

#[test]
fn reminders_due_starts_after_the_mark_and_is_bounded_to_seven_days() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(
            7,
            "x ",
            &[
                at_1430(8, 0),                              // fires 2026-10-07 14:30
                (9, "2026-10-08", Some("14:30"), Some(0)),  // 2026-10-08 14:30
                (10, "2026-10-09", Some("14:30"), Some(0)), // 2026-10-09 14:30
            ],
        )]),
        0x21,
    );
    let until = OCT_7_1430 + 3 * DAY;
    assert_eq!(
        nodes(&reminders_due(&mut core, until)),
        [block(8), block(9), block(10)],
        "no mark: the last seven days"
    );

    // With a mark: only what is after it.
    core.db
        .setting_set(LAST_CHECKED_SETTING, &Json::from(OCT_7_1430 + DAY), 1)
        .unwrap();
    assert_eq!(
        nodes(&reminders_due(&mut core, until)),
        [block(10)],
        "a reminder at the mark itself has been fired"
    );

    // A mark older than seven days counts as seven days: only the last seven days before
    // `until` are looked at, the window open below.
    assert_eq!(CATCH_UP_SECONDS, 7 * DAY);
    core.db
        .setting_set(LAST_CHECKED_SETTING, &Json::from(0), 1)
        .unwrap();
    assert_eq!(
        nodes(&reminders_due(&mut core, OCT_7_1430 + 7 * DAY - 1)),
        [block(8), block(9), block(10)],
        "one second short of seven days back"
    );
    assert_eq!(
        nodes(&reminders_due(&mut core, OCT_7_1430 + 7 * DAY)),
        [block(9), block(10)],
        "exactly seven days back is outside"
    );
    assert_eq!(
        nodes(&reminders_due(&mut core, OCT_7_1430 + 8 * DAY)),
        [block(10)]
    );
}

#[test]
fn a_mark_that_is_not_a_number_is_no_mark() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[at_1430(8, 0)])]),
        0x31,
    );
    core.db
        .setting_set(LAST_CHECKED_SETTING, &Json::from("garbage"), 1)
        .unwrap();
    assert_eq!(nodes(&reminders_due(&mut core, OCT_7_1430)), [block(8)]);
}

// ----- the tick ---------------------------------------------------------------------------

#[test]
fn the_tick_fires_each_reminder_once_and_advances_the_mark() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - 2 * HOUR, 0);
    let (mut core, space, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "Call ", &[at_1430(8, 15), at_1430(9, 0)])]),
        0x41,
    );
    core.drain_events();

    // Before anything is due: nothing fires, and the mark is now.
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert!(core.drain_events().is_empty());
    assert_eq!(
        setting(&mut core, LAST_CHECKED_SETTING),
        Some(Json::from(OCT_7_1430 - 2 * HOUR))
    );

    // 14:16: the 15 minutes before one is due, the one at the time is not.
    clock.set_local(OCT_7_1430 - 14 * MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 1);
    let events = core.drain_events();
    assert_eq!(
        events,
        [Event::ReminderDue(ReminderDue {
            page_id: page,
            space_id: space,
            block_id: block(7),
            node_id: block(8),
            page_title: "Plan".into(),
            text: "Call 2026-10-07 14:30 2026-10-07 14:30 ".into(),
            date: "2026-10-07".into(),
            time: Some("14:30".into()),
            reminder_minutes: 15,
            fire_at: OCT_7_1430 - 15 * MINUTE,
        })]
    );

    // Ticking again at the same time, or a minute later, fires nothing more.
    assert_eq!(core.reminders_tick().unwrap(), 0);
    clock.advance(MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert!(core.drain_events().is_empty());

    // 14:31: the other one.
    clock.set_local(OCT_7_1430 + MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 1);
    assert_eq!(nodes(&due_events(&core.drain_events())), [block(9)]);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert_eq!(
        setting(&mut core, LAST_CHECKED_SETTING),
        Some(Json::from(OCT_7_1430 + MINUTE))
    );
}

#[test]
fn a_reminder_exactly_at_the_tick_fires_and_one_a_second_later_does_not() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - HOUR, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[at_1430(8, 0)])]),
        0x51,
    );
    assert_eq!(core.reminders_tick().unwrap(), 0);
    clock.set_local(OCT_7_1430 - 1);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    clock.set_local(OCT_7_1430);
    assert_eq!(
        core.reminders_tick().unwrap(),
        1,
        "the clock reached the moment"
    );
}

#[test]
fn the_mark_survives_a_restart_so_nothing_fires_twice() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - HOUR, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[at_1430(8, 0)])]),
        0x61,
    );
    core.reminders_tick().unwrap();
    clock.set_local(OCT_7_1430 + MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 1);
    drop(core);

    let mut core = open(&fx, 2, &clock);
    clock.set_local(OCT_7_1430 + 5 * MINUTE);
    assert_eq!(
        core.reminders_tick().unwrap(),
        0,
        "it was fired before the restart"
    );
}

#[test]
fn what_fell_due_while_the_app_was_closed_fires_at_the_first_tick() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - HOUR, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[at_1430(8, 0), at_1430(9, 30)])]),
        0x71,
    );
    core.reminders_tick().unwrap();
    drop(core);

    // The app is closed from 13:30 to 18:00: the reminders of 14:00 and 14:30 fell due.
    clock.set_local(OCT_7_1430 + 3 * HOUR + 30 * MINUTE);
    let mut core = open(&fx, 2, &clock);
    assert_eq!(core.reminders_tick().unwrap(), 2);
    assert_eq!(
        nodes(&due_events(&core.drain_events())),
        [block(9), block(8)],
        "oldest first"
    );
    assert_eq!(core.reminders_tick().unwrap(), 0);
}

#[test]
fn the_first_tick_of_a_device_looks_back_seven_days_only() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(
            7,
            "x ",
            &[
                at_1430(8, 0),                              // now
                (9, "2026-09-30", Some("14:30"), Some(0)),  // exactly seven days ago
                (10, "2026-10-01", Some("14:30"), Some(0)), // six days ago
                (11, "2026-08-01", Some("09:00"), Some(0)), // two months ago
            ],
        )]),
        0x81,
    );
    assert_eq!(core.reminders_tick().unwrap(), 2);
    assert_eq!(
        nodes(&due_events(&core.drain_events())),
        [block(10), block(8)]
    );
    assert_eq!(
        setting(&mut core, LAST_CHECKED_SETTING),
        Some(Json::from(OCT_7_1430))
    );
}

#[test]
fn a_reminder_set_on_a_moment_that_has_passed_does_not_fire() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    clock.advance(MINUTE);
    // Yesterday's date with a reminder: its fire time is before the mark.
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[(8, "2026-10-06", Some("09:00"), Some(0))])]),
        0x91,
    );
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert!(
        core.drain_events()
            .iter()
            .all(|e| !matches!(e, Event::ReminderDue(_)))
    );
}

#[test]
fn the_wall_clock_is_utc_plus_the_platform_offset() {
    // 14:30 in Tokyo (UTC+9) is 05:30 UTC: the reminder is due when the local clock says 14:30.
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - HOUR, 9 * HOUR as i32);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[at_1430(8, 0)])]),
        0xA1,
    );
    assert_eq!(core.reminders_tick().unwrap(), 0);
    // 13:59 local is 04:59 UTC.
    clock.set_local(OCT_7_1430 - MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    clock.set_local(OCT_7_1430);
    assert_eq!(
        clock.now_ms() / 1000 % DAY,
        5 * HOUR + 30 * MINUTE,
        "05:30 UTC"
    );
    assert_eq!(core.reminders_tick().unwrap(), 1);
}

#[test]
fn daylight_saving_skips_nothing_and_repeats_nothing() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(
            7,
            "x ",
            &[
                (8, "2026-10-07", Some("14:30"), Some(0)),
                (9, "2026-10-07", Some("15:30"), Some(0)),
            ],
        )]),
        0xB1,
    );
    // The clock was at 14:00; the zone springs forward, and the next reading is 16:00: both
    // reminders fell inside the hour that was skipped, and fire.
    clock.set_local(OCT_7_1430 - 30 * MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    clock.set_offset(3600);
    clock.set_local(OCT_7_1430 + 90 * MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 2);
    core.drain_events();

    // The clock falls back an hour (15:30 again, offset back to 0): nothing fires again, and
    // the mark does not move back, so the repeated hour stays silent.
    clock.set_offset(0);
    clock.set_local(OCT_7_1430 + HOUR);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert_eq!(
        setting(&mut core, LAST_CHECKED_SETTING),
        Some(Json::from(OCT_7_1430 + 90 * MINUTE)),
        "the mark never moves back"
    );
    clock.set_local(OCT_7_1430 + 2 * HOUR);
    assert_eq!(core.reminders_tick().unwrap(), 0);
}

#[test]
fn the_tick_does_not_fire_reminders_of_trashed_pages() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - HOUR, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, "x ", &[at_1430(8, 0)])]),
        0xC1,
    );
    core.reminders_tick().unwrap();
    core.dispatch(Request::PageTrash { page_id: page }).unwrap();
    core.drain_events();
    clock.set_local(OCT_7_1430 + MINUTE);
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert!(due_events(&core.drain_events()).is_empty());
}

#[test]
fn an_event_carries_the_space_of_its_page_and_a_text_of_at_most_200_characters() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430 - HOUR, 0);
    let (mut core, _, _) = setup(&fx, &clock);
    let other = create_space(&mut core, "Work");
    let page = create_page(&mut core, other.id, None, "Standup").id;
    core.page_doc_open(page).unwrap();
    let long = "é".repeat(300);
    import_page(
        &mut core,
        page,
        &page_json(&[(7, long.as_str(), &[at_1430(8, 0)])]),
        0xD1,
    );
    core.reminders_tick().unwrap();
    clock.set_local(OCT_7_1430);
    assert_eq!(core.reminders_tick().unwrap(), 1);
    let events = due_events(&core.drain_events());
    assert_eq!(events[0].space_id, other.id);
    assert_eq!(events[0].page_title, "Standup");
    assert_eq!(events[0].text.chars().count(), 200);
    assert!(events[0].text.chars().all(|c| c == 'é'));
}

#[test]
fn a_large_batch_fires_one_batch_per_tick_and_everything_exactly_once() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, page) = setup(&fx, &clock);
    // 40 reminders for 2026-10-07 14:30, 40, 39, ..., 1 minutes before it: they fire at 13:50,
    // 13:51, ..., 14:29 (nodes 100 to 139), plus three more at 14:14, the minute of the 25th.
    assert_eq!(FIRES_PER_TICK, 25);
    let mut mentions: Vec<Mention> = (0..40)
        .map(|i| {
            (
                100 + i,
                "2026-10-07",
                Some("14:30"),
                Some(40 - i64::from(i)),
            )
        })
        .collect();
    mentions.extend([
        (150, "2026-10-07", Some("14:30"), Some(16)),
        (151, "2026-10-07", Some("14:30"), Some(16)),
        (152, "2026-10-07", Some("14:30"), Some(16)),
    ]);
    import_page(&mut core, page, &page_json(&[(7, "x ", &mentions)]), 0xE1);
    clock.set_local(OCT_7_1430 + HOUR);

    // The first tick of this device (no mark) takes the oldest 25 and the three that fire at
    // the same moment as the 25th: 28. The mark moves to that moment, not to now.
    assert_eq!(core.reminders_tick().unwrap(), 28);
    let first = due_events(&core.drain_events());
    assert_eq!(first.len(), 28);
    assert_eq!(
        setting(&mut core, LAST_CHECKED_SETTING),
        Some(Json::from(OCT_7_1430 - 16 * MINUTE))
    );

    // The next tick fires the other 15, and then nothing is left.
    assert_eq!(core.reminders_tick().unwrap(), 15);
    let second = due_events(&core.drain_events());
    assert_eq!(core.reminders_tick().unwrap(), 0);
    assert_eq!(
        setting(&mut core, LAST_CHECKED_SETTING),
        Some(Json::from(OCT_7_1430 + HOUR))
    );

    let all: Vec<ReminderDue> = first.into_iter().chain(second).collect();
    let mut seen = nodes(&all);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 43, "every reminder fired once");
    let times: Vec<i64> = all.iter().map(|due| due.fire_at).collect();
    assert!(
        times.windows(2).all(|pair| pair[0] <= pair[1]),
        "oldest first"
    );
    assert!(MAX_REMINDERS_DUE as usize >= FIRES_PER_TICK);
}

#[test]
fn the_mark_is_an_ordinary_setting_the_ui_may_read_and_a_reminders_request_has_a_kind() {
    let fx = Fixture::new();
    let clock = TestClock::at(OCT_7_1430, 0);
    let (mut core, _, _) = setup(&fx, &clock);
    core.reminders_tick().unwrap();
    // `app.reminders.last_checked` passes the key grammar of the API.
    assert_eq!(
        setting(&mut core, "app.reminders.last_checked"),
        Some(Json::from(OCT_7_1430))
    );
    assert_eq!(Request::RemindersDue { until: 0 }.kind(), "RemindersDue");
    // The tick does not announce its own mark as a changed setting.
    assert!(
        core.drain_events()
            .iter()
            .all(|e| !matches!(e, Event::SettingChanged { .. }))
    );
}

#[test]
fn the_test_clock_is_the_platform_clock_the_core_reads() {
    // A guard for the helpers above: the default of `local_offset_seconds` is UTC.
    let fixed = testing::FixedClock::new(5_000);
    assert_eq!(fixed.local_offset_seconds(), 0);
    let clock = TestClock::at(1_000, 3600);
    assert_eq!(clock.now_ms(), (1_000 - 3600) * 1000);
}
