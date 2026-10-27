//! Milestone 6 tests, on protocol v2: the fleet simulator. Several cores share one account (the same
//! ARK, one device log) and one `MemoryRemote`; a scripted scheduler with a seeded RNG runs edits,
//! pushes, pulls, snapshots, heads and chunk transfers the way the `kb-net` driver does (the cycle
//! of `tests_sharing.rs`, over the personal feed), and every scenario ends with a convergence
//! check: identical trees, titles, page contents (portable JSON), keyring counts and search hits on
//! every device. Pairing, kit recovery, revocation, attachment sync, fork detection and the server
//! reset run here too; the shared spaces have their own file.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};
use std::time::Instant;

use kb_storage::SyncQueries;
use kb_sync::SyncError;
use kb_sync::v2::MemoryRemote;
use kb_sync::v2::memory::Adversary;
use kb_types::attachments::{CHUNK_SIZE, DOWNLOAD_POLICY_ON_DEMAND, DOWNLOAD_POLICY_SETTING};
use kb_types::sharing::PRUNE_ACK_DAYS;
use kb_types::sync::SNAPSHOT_AFTER_ENTRIES;
use kb_types::testing::SeededRng;
use kb_types::{
    ApiError, AttachmentAvailability, AttachmentId, AttachmentInfo, ErrorCode, Event, FeedId,
    PageId, PairingRole, PairingState, Request, Response, RngExt, SearchHit, SpaceId, SyncState,
};

use crate::test_support::{
    Fixture, create_page, create_space, create_workspace, page_content, spaces, tree, write_text,
};
use crate::tests_sharing::{Dev, SERVER_URL, Sim, SimError, open_core};
use crate::{Core, Opened};

const DAY_MS: i64 = 86_400_000;

/// One simulated device: a [`Dev`] of the v2 simulator (its own installation, keystore, clock and
/// connection) with the helpers of these scenarios.
struct Device {
    dev: Dev,
}

impl Deref for Device {
    type Target = Dev;
    fn deref(&self) -> &Dev {
        &self.dev
    }
}

impl DerefMut for Device {
    fn deref_mut(&mut self) -> &mut Dev {
        &mut self.dev
    }
}

impl Device {
    /// The first device of the account: workspace, first space and the recovery kit.
    fn first(name: &'static str, seed: u64, server: &MemoryRemote) -> (Self, String) {
        let (dev, kit) = Dev::first(name, seed, server);
        (Self { dev }, kit)
    }

    /// A device that joins the way Milestone 4 restores: a backup of `source` plus the kit on a
    /// fresh installation (its `DeviceAdded` entry is signed by the kit).
    fn from_backup(
        name: &'static str,
        seed: u64,
        server: &MemoryRemote,
        source: &mut Device,
        kit: &str,
    ) -> Self {
        Self {
            dev: Dev::from_backup(name, seed, server, &mut source.dev, kit),
        }
    }

    /// A device that joins with nothing but the ARK and a `DeviceAdded` entry that `sponsor` signs
    /// and publishes (the shape pairing takes): no documents, it bootstraps everything from the
    /// server.
    fn fresh(name: &'static str, seed: u64, server: &MemoryRemote, sponsor: &mut Device) -> Self {
        Self {
            dev: Dev::fresh(name, seed, server, &mut sponsor.dev),
        }
    }

    /// A fresh installation with its own root key and no workspace.
    fn unjoined(name: &'static str, seed: u64, server: &MemoryRemote) -> Self {
        let fx = Fixture::new();
        let (mut core, clock) = open_core(&fx, seed, 1_000_000 + i64::try_from(seed).unwrap());
        core.drain_events();
        Self {
            dev: Dev {
                name,
                _fx: fx,
                core,
                remote: server.clone(),
                authed: false,
                edits: seed * 1000,
                last_report: None,
                log: Vec::new(),
                clock,
            },
        }
    }

    /// The account's personal feed.
    fn personal(&self) -> FeedId {
        FeedId::from_account(self.dev.account())
    }

    /// One page of the personal feed from the core's cursor, handed to the core by hand (the
    /// cycles drain the events a test may want to see).
    fn pull_once(&mut self) -> kb_sync::v2::SyncApplied {
        let feed = self.personal();
        let cursor = self.core.sync_info().unwrap().remote_cursor;
        let page = self.remote.changes_sync(feed, cursor, 200).unwrap();
        self.core.sync_feed_apply(feed, &page).unwrap()
    }

    /// Publishes a snapshot of one document by hand (the thresholds are far away); answers the
    /// cursor it covers.
    fn publish_snapshot(&mut self, doc: kb_types::DocId) -> u64 {
        let feed = self.personal();
        let (bytes, covers) = self.core.sync_feed_make_snapshot(feed, doc).unwrap();
        self.remote.put_snapshot_sync(feed, &bytes).unwrap();
        self.core
            .sync_feed_snapshot_published(feed, doc, covers)
            .unwrap();
        covers
    }
}

impl Device {
    /// Uploads `bytes` as an attachment of the space, the way a shell streams a file.
    fn upload_attachment(&mut self, space_id: SpaceId, name: &str, bytes: &[u8]) -> AttachmentInfo {
        let id = match self
            .core
            .dispatch(Request::AttachmentBegin {
                space_id,
                name: name.into(),
                mime: "application/octet-stream".into(),
            })
            .unwrap()
        {
            Response::AttachmentStarted(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        for piece in bytes.chunks(CHUNK_SIZE / 3 + 1) {
            self.core.attachment_write(id, piece).unwrap();
        }
        let info = match self
            .core
            .dispatch(Request::AttachmentFinish { attachment_id: id })
            .unwrap()
        {
            Response::Attachment(info) => info,
            other => panic!("unexpected {other:?}"),
        };
        self.core.drain_events();
        info
    }

    fn attachment(&mut self, attachment_id: AttachmentId) -> AttachmentInfo {
        match self
            .core
            .dispatch(Request::AttachmentGet { attachment_id })
            .unwrap()
        {
            Response::Attachment(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn read_attachment(&mut self, attachment_id: AttachmentId) -> Result<Vec<u8>, ApiError> {
        let mut out = Vec::new();
        self.core
            .attachment_read(attachment_id, None, &mut |piece| {
                out.extend_from_slice(piece);
                Ok(())
            })
            .map(|_| out)
    }

    fn set_download_policy(&mut self, value: &str) {
        self.core
            .dispatch(Request::SettingsSet {
                key: DOWNLOAD_POLICY_SETTING.into(),
                value: serde_json::Value::String(value.into()),
            })
            .unwrap();
        self.core.drain_events();
    }

    fn next_edit(&mut self) -> u64 {
        self.edits += 1;
        self.edits
    }

    fn space_ids(&mut self) -> Vec<SpaceId> {
        spaces(&mut self.core).into_iter().map(|s| s.id).collect()
    }

    fn live_pages(&mut self, space_id: SpaceId) -> Vec<PageId> {
        fn walk(nodes: &[kb_types::PageNode], out: &mut Vec<PageId>) {
            for node in nodes {
                out.push(node.page.id);
                walk(&node.children, out);
            }
        }
        let view = tree(&mut self.core, space_id);
        let mut out = Vec::new();
        walk(&view.roots, &mut out);
        out
    }

    fn add_paragraph(&mut self, page_id: PageId) {
        let n = self.next_edit();
        let text = format!("{} wrote line {n}", self.name);
        write_text(&mut self.core, page_id, &text, n);
        self.core.drain_events();
    }

    fn new_page(&mut self, space_id: SpaceId, parent: Option<PageId>) -> PageId {
        let n = self.next_edit();
        let page = create_page(
            &mut self.dev.core,
            space_id,
            parent,
            &format!("{} page {n}", self.dev.name),
        );
        self.dev.core.drain_events();
        page.id
    }

    fn rename(&mut self, page_id: PageId) {
        let n = self.next_edit();
        self.dev
            .core
            .dispatch(Request::PageRename {
                page_id,
                title: format!("{} renamed {n}", self.dev.name),
            })
            .unwrap();
        self.core.drain_events();
    }

    fn move_page(&mut self, page_id: PageId, parent: Option<PageId>, index: u32) {
        let _ = self.core.dispatch(Request::PageMove {
            page_id,
            parent_id: parent,
            index,
        });
        self.core.drain_events();
    }

    fn trash(&mut self, page_id: PageId) {
        self.core.dispatch(Request::PageTrash { page_id }).unwrap();
        self.core.drain_events();
    }

    /// Everything the convergence check compares.
    fn state(&mut self) -> DeviceState {
        let mut spaces_out = BTreeMap::new();
        for space_id in self.space_ids() {
            let structure = self.core.docs.structures[&space_id].model.to_portable();
            let outline = crate::test_support::outline(&tree(&mut self.core, space_id));
            let mut pages = BTreeMap::new();
            for page_id in self.core.docs.structures[&space_id].model.all_page_ids() {
                // Reading a page without a document would create one (and a key):
                // the check must not change what it compares.
                let has_doc = kb_storage::DocQueries::doc_get(
                    &self.core.db,
                    kb_types::DocId::for_page(page_id),
                )
                .unwrap()
                .is_some();
                if has_doc {
                    pages.insert(page_id, page_content(&mut self.core, page_id));
                }
            }
            spaces_out.insert(
                space_id,
                SpaceState {
                    outline,
                    structure,
                    pages,
                },
            );
        }
        let hits = match self
            .core
            .dispatch(Request::Search {
                query: "wrote".into(),
                space_id: None,
                scope: None,
                limit: Some(100),
                edited_after: None,
            })
            .unwrap()
        {
            Response::Search(hits) => hits.into_iter().map(|h: SearchHit| h.page_id).collect(),
            other => panic!("unexpected {other:?}"),
        };
        let counts = self.core.security.keyring.counts();
        DeviceState {
            spaces: spaces_out,
            keyring: (counts.spaces, counts.documents, counts.tombstones),
            hits,
        }
    }
}

#[derive(Debug, PartialEq)]
struct SpaceState {
    outline: String,
    structure: serde_json::Value,
    pages: BTreeMap<PageId, serde_json::Value>,
}

#[derive(Debug, PartialEq)]
struct DeviceState {
    spaces: BTreeMap<SpaceId, SpaceState>,
    keyring: (u32, u32, u32),
    hits: BTreeSet<PageId>,
}

/// Runs full cycles until nothing moves any more, then checks every device agrees.
fn settle(devices: &mut [&mut Device]) {
    for _ in 0..3 {
        for device in devices.iter_mut() {
            device
                .cycle()
                .unwrap_or_else(|e| panic!("{} cycle: {e:?}", device.name));
        }
    }
    assert_converged(devices);
}

fn assert_converged(devices: &mut [&mut Device]) {
    let states: Vec<(&str, DeviceState)> =
        devices.iter_mut().map(|d| (d.name, d.state())).collect();
    for pair in states.windows(2) {
        let (a_name, a) = &pair[0];
        let (b_name, b) = &pair[1];
        assert_eq!(
            a.spaces.keys().collect::<Vec<_>>(),
            b.spaces.keys().collect::<Vec<_>>(),
            "{a_name} and {b_name} list different spaces"
        );
        for (space, sa) in &a.spaces {
            let sb = &b.spaces[space];
            assert_eq!(sa.outline, sb.outline, "{a_name}/{b_name} tree of {space}");
            assert_eq!(
                sa.structure, sb.structure,
                "{a_name}/{b_name} structure of {space}"
            );
            assert_eq!(
                sa.pages.keys().collect::<Vec<_>>(),
                sb.pages.keys().collect::<Vec<_>>(),
                "{a_name}/{b_name} pages with documents in {space}"
            );
            for (page, ca) in &sa.pages {
                assert_eq!(
                    Some(ca),
                    sb.pages.get(page),
                    "{a_name}/{b_name} content of {page}"
                );
            }
        }
        assert_eq!(a.keyring, b.keyring, "{a_name}/{b_name} keyring counts");
        assert_eq!(a.hits, b.hits, "{a_name}/{b_name} search hits");
    }
}

fn server(seed: u64) -> MemoryRemote {
    MemoryRemote::new(&SeededRng::new(seed))
}

fn pick<T: Copy>(rng: &SeededRng, items: &[T]) -> T {
    let r: [u8; 8] = rng.random_array();
    items[(u64::from_le_bytes(r) % items.len() as u64) as usize]
}

/// A random local edit on a device.
fn random_edit(rng: &SeededRng, device: &mut Device) {
    let space = pick(rng, &device.space_ids());
    let pages = device.live_pages(space);
    let roll: [u8; 1] = rng.random_array();
    match roll[0] % 10 {
        0 | 1 if !pages.is_empty() => {
            let page = pick(rng, &pages);
            device.rename(page);
        }
        2 if pages.len() > 1 => {
            let page = pick(rng, &pages);
            let parent = pick(rng, &pages);
            if parent != page {
                device.move_page(page, Some(parent), 0);
            }
        }
        3 => {
            let parent = (!pages.is_empty() && roll[0] > 128).then(|| pick(rng, &pages));
            device.new_page(space, parent);
        }
        _ if !pages.is_empty() => {
            let page = pick(rng, &pages);
            device.add_paragraph(page);
        }
        _ => {
            device.new_page(space, None);
        }
    }
}

// ----- scenarios ------------------------------------------------------------------------

#[test]
fn two_devices_ping_pong_and_converge() {
    let server = server(1);
    let (mut a, kit) = Device::first("a", 11, &server);
    let personal = a.space_ids()[0];
    let plan = a.new_page(personal, None);
    a.add_paragraph(plan);
    a.configure();
    assert_eq!(a.core.sync_info().unwrap().state, SyncState::Idle);
    a.cycle().unwrap();
    let info = a.core.sync_info().unwrap();
    assert!(
        info.remote_cursor > 0,
        "own entries advance the cursor: {info:?}"
    );
    assert_eq!(info.pending_entries, 0, "{info:?}");

    let mut b = Device::from_backup("b", 22, &server, &mut a, &kit);
    b.configure();
    b.cycle().unwrap();
    let devices = a.core.security.log_state.as_ref().unwrap().devices.len();
    a.cycle().unwrap();
    assert_eq!(
        a.core.security.log_state.as_ref().unwrap().devices.len(),
        devices + 1,
        "a learns b's keys from the server's log"
    );

    // Concurrent edits on the same page, then on separate ones, with pulls in between.
    let rng = SeededRng::new(5);
    for round in 0..12 {
        a.add_paragraph(plan);
        b.add_paragraph(plan);
        if round % 3 == 0 {
            random_edit(&rng, &mut a);
            random_edit(&rng, &mut b);
        }
        a.cycle().unwrap();
        b.cycle().unwrap();
    }
    settle(&mut [&mut a, &mut b]);
    let content = page_content(&mut a.core, plan);
    let text = content.to_string();
    assert!(text.contains("a wrote line") && text.contains("b wrote line"));
    let info = b.core.sync_info().unwrap();
    assert_eq!(info.devices_seen, 1);
    assert!(!info.fork_detected);
    assert!(info.last_pull_at.is_some() && info.last_push_at.is_some());
}

#[test]
fn a_remote_change_reaches_an_open_editor_and_the_search_index() {
    let server = server(2);
    let (mut a, kit) = Device::first("a", 31, &server);
    let personal = a.space_ids()[0];
    let page = a.new_page(personal, None);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 32, &server, &mut a, &kit);
    b.configure();
    b.cycle().unwrap();
    a.cycle().unwrap();

    // b keeps the page open in an editor while a's change arrives.
    b.core.page_doc_open(page).unwrap();
    a.add_paragraph(page);
    a.cycle().unwrap();
    b.cycle().unwrap_or_else(|e| panic!("{e:?}"));
    // The cycle drained the events; look at the editor's view directly.
    let text = page_content(&mut b.core, page).to_string();
    assert!(text.contains("a wrote line"), "{text}");
    let hits = match b
        .core
        .dispatch(Request::Search {
            query: "wrote".into(),
            space_id: None,
            scope: None,
            limit: None,
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(hits.len(), 1, "the remote text is indexed on b");
    b.core.page_doc_close(page).unwrap();

    // Events: a remote page change is reported with the Remote source.
    a.add_paragraph(page);
    a.cycle().unwrap();
    b.pull_once();
    let events = b.core.drain_events();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::PageContentChanged { page_id, source: kb_types::ChangeSource::Remote } if *page_id == page
        )),
        "{events:?}"
    );
}

#[test]
fn three_devices_one_offline_for_weeks_catches_up_through_snapshots() {
    let server = server(3);
    let (mut a, kit) = Device::first("a", 41, &server);
    let personal = a.space_ids()[0];
    let work = create_space(&mut a.core, "Work").id;
    let p1 = a.new_page(personal, None);
    let p2 = a.new_page(work, None);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 42, &server, &mut a, &kit);
    b.configure();
    // A device restored from a backup appends its own log entry; the next device
    // must restore from a backup that already carries it (the device logs would
    // diverge otherwise), so b syncs before c joins.
    settle(&mut [&mut a, &mut b]);
    let mut c = Device::from_backup("c", 43, &server, &mut a, &kit);
    c.configure();
    settle(&mut [&mut a, &mut b, &mut c]);

    // c goes offline; a and b make hundreds of edits (enough for a snapshot).
    let rng = SeededRng::new(7);
    let rounds = usize::try_from(SNAPSHOT_AFTER_ENTRIES).unwrap() / 2 + 20;
    for i in 0..rounds {
        a.add_paragraph(p1);
        b.add_paragraph(p2);
        if i % 25 == 0 {
            random_edit(&rng, &mut a);
            random_edit(&rng, &mut b);
        }
        a.cycle().unwrap();
        b.cycle().unwrap();
    }
    let snapshots = a.remote.snapshots_sync(a.personal()).unwrap();
    assert!(
        !snapshots.snapshots.is_empty(),
        "many entries on one document trigger a snapshot"
    );
    let before = Instant::now();
    c.cycle().unwrap();
    eprintln!("c caught up in {:?}", before.elapsed());
    settle(&mut [&mut a, &mut b, &mut c]);
    let text = page_content(&mut c.core, p1).to_string();
    assert!(
        text.matches("a wrote line").count() >= rounds,
        "c has every paragraph"
    );
}

#[test]
fn reordered_and_duplicated_pages_converge() {
    let server = server(4);
    let (mut a, kit) = Device::first("a", 51, &server);
    let personal = a.space_ids()[0];
    let pages: Vec<PageId> = (0..6).map(|_| a.new_page(personal, None)).collect();
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 52, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);

    // Both reorder and nest concurrently, create pages with the same titles and trash
    // one page while the other device edits it.
    a.move_page(pages[0], Some(pages[1]), 0);
    b.move_page(pages[1], Some(pages[0]), 0);
    a.move_page(pages[2], None, 0);
    b.move_page(pages[2], None, 5);
    a.core
        .dispatch(Request::PageCreate {
            space_id: personal,
            parent_id: None,
            index: None,
            title: "Twin".into(),
        })
        .unwrap();
    b.core
        .dispatch(Request::PageCreate {
            space_id: personal,
            parent_id: None,
            index: None,
            title: "Twin".into(),
        })
        .unwrap();
    a.trash(pages[3]);
    b.add_paragraph(pages[3]);
    b.rename(pages[4]);
    a.rename(pages[4]);
    settle(&mut [&mut a, &mut b]);
    let outline = crate::test_support::outline(&tree(&mut a.core, personal));
    assert_eq!(outline.matches("Twin").count(), 2, "{outline}");
    assert!(
        !outline.contains(&crate::test_support::page(&mut a.core, pages[3]).title),
        "the trashed page is out of both trees"
    );
}

#[test]
fn server_data_loss_re_uploads_and_a_fourth_device_bootstraps_from_snapshots() {
    let server = server(5);
    let (mut a, kit) = Device::first("a", 61, &server);
    let personal = a.space_ids()[0];
    let work = create_space(&mut a.core, "Work").id;
    let p1 = a.new_page(personal, None);
    let parent = a.new_page(work, None);
    let p2 = a.new_page(work, Some(parent));
    a.add_paragraph(p1);
    a.add_paragraph(p2);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 62, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    let epoch_before = a.core.sync_info().unwrap().server_epoch.clone().unwrap();

    // A server restart keeps the epoch: sessions are gone, nothing else changes.
    a.authed = false;
    b.authed = false;
    a.add_paragraph(p1);
    settle(&mut [&mut a, &mut b]);

    // The server loses its data.
    server.reset_data();
    a.authed = false;
    b.authed = false;
    b.add_paragraph(p2);
    a.cycle().unwrap();
    let info = a.core.sync_info().unwrap();
    assert_ne!(info.server_epoch.as_deref(), Some(epoch_before.as_str()));
    assert!(
        !a.remote
            .snapshots_sync(a.personal())
            .unwrap()
            .snapshots
            .is_empty(),
        "every document is snapshotted on its first upload after the reset"
    );
    settle(&mut [&mut a, &mut b]);
    assert!(
        page_content(&mut a.core, p2)
            .to_string()
            .contains("b wrote")
    );

    // A fresh device (only the account key and a log entry) bootstraps from snapshots.
    let mut d = Device::fresh("d", 64, &server, &mut a);
    d.configure();
    let before = Instant::now();
    d.cycle().unwrap();
    eprintln!("d bootstrapped in {:?}", before.elapsed());
    settle(&mut [&mut a, &mut b, &mut d]);
    assert_eq!(d.space_ids().len(), 2);
    assert!(
        page_content(&mut d.core, p1)
            .to_string()
            .contains("a wrote")
    );
    // d's edits flow back.
    d.add_paragraph(p1);
    settle(&mut [&mut a, &mut b, &mut d]);
    assert!(
        page_content(&mut a.core, p1)
            .to_string()
            .contains("d wrote")
    );
}

#[test]
fn a_slow_device_behind_a_pruned_range_recovers_from_the_snapshot() {
    let server = server(6);
    let (mut a, kit) = Device::first("a", 71, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 72, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);

    // a writes enough for a snapshot while b is offline; the server prunes the covered
    // entries after the grace period.
    let rounds = usize::try_from(SNAPSHOT_AFTER_ENTRIES).unwrap() + 10;
    for _ in 0..rounds {
        a.add_paragraph(p1);
        a.cycle().unwrap();
    }
    let now = server.now_ms();
    server.set_now_ms(now + i64::from(PRUNE_ACK_DAYS + 1) * DAY_MS);
    let pruned = server.prune(server.now_ms());
    assert!(pruned > 0, "entries under the snapshot were pruned");

    b.cycle().unwrap_or_else(|e| panic!("{e:?}"));
    let info = b.core.sync_info().unwrap();
    assert!(!info.fork_detected, "pruning is not a fork: {info:?}");
    settle(&mut [&mut a, &mut b]);
    let text = page_content(&mut b.core, p1).to_string();
    assert_eq!(text.matches("a wrote line").count(), rounds);
}

/// Two devices in sync, with one page carrying text, for the adversarial scenarios.
fn pair(seed: u64) -> (MemoryRemote, Device, Device, PageId) {
    let server = server(seed);
    let (mut a, kit) = Device::first("a", seed * 10 + 1, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    a.add_paragraph(p1);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", seed * 10 + 2, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    (server, a, b, p1)
}

#[test]
fn a_replayed_entry_is_skipped() {
    let (server, _a, mut b, _p1) = pair(7);
    let before = b.state();
    server.set_adversary(Some(Adversary::Replay));
    b.cycle().unwrap();
    assert_eq!(b.state(), before, "a repeated entry changes nothing");
    assert!(!b.core.sync_info().unwrap().fork_detected);
}

#[test]
fn a_forged_entry_is_refused() {
    let (server, mut a, mut b, p1) = pair(12);
    // a's genuine entry is on the server first; the forging server then adds a copy of it whose
    // header claims the next device sequence (its signature no longer matches).
    a.add_paragraph(p1);
    a.cycle().unwrap();
    server.set_adversary(Some(Adversary::Forge));
    let before = page_content(&mut b.core, p1);
    let err = b.cycle().unwrap_err();
    match err {
        SimError::Api(err) => assert_eq!(err.code, ErrorCode::Crypto, "{err}"),
        other => panic!("{other:?}"),
    }
    // The real entry before the forged one was applied; the cursor stopped there.
    let after = page_content(&mut b.core, p1);
    assert_ne!(before, after, "the genuine entry of the page was applied");
    assert!(!b.core.sync_info().unwrap().fork_detected);
}

#[test]
fn a_rollback_is_detected_and_a_reconnect_starts_over() {
    let (server, mut a, mut b, p1) = pair(8);
    server.set_adversary(Some(Adversary::Rollback(2)));
    let err = b.cycle().unwrap_err();
    assert!(
        matches!(err, SimError::Api(ref e) if e.code == ErrorCode::Crypto),
        "{err:?}"
    );
    let info = b.core.sync_info().unwrap();
    assert!(
        info.fork_detected && info.state == SyncState::Forked,
        "{info:?}"
    );
    assert!(info.last_error.is_some());
    server.set_adversary(None);

    // A forked device keeps its data and stops uploading; reconnecting to the server
    // starts over (everything is re-published, idempotently).
    b.add_paragraph(p1);
    b.cycle().unwrap();
    assert!(
        b.core.sync_info().unwrap().pending_entries > 0,
        "uploads paused"
    );
    b.core.sync_configure(None, None).unwrap();
    b.core.sync_configure(Some(SERVER_URL), None).unwrap();
    b.authed = false;
    settle(&mut [&mut a, &mut b]);
    assert!(
        page_content(&mut a.core, p1)
            .to_string()
            .contains("b wrote")
    );
}

#[test]
fn a_withheld_document_is_detected_as_a_fork() {
    let (server, mut a, mut b, _p1) = pair(13);
    let personal = a.space_ids()[0];
    let p2 = a.new_page(personal, None);
    settle(&mut [&mut a, &mut b]);
    server.set_adversary(Some(Adversary::Withhold {
        doc: kb_types::DocId::for_page(p2),
    }));
    // The server hides every entry of the document, including ones both devices
    // already folded into their chain: its history no longer matches theirs.
    a.add_paragraph(p2);
    for device in [&mut a, &mut b] {
        let err = device.cycle().unwrap_err();
        assert!(
            matches!(err, SimError::Api(ref e) if e.code == ErrorCode::Crypto),
            "{err:?}"
        );
        let info = device.core.sync_info().unwrap();
        assert!(
            info.fork_detected && info.state == SyncState::Forked,
            "{info:?}"
        );
    }
}

#[test]
fn sync_requests_and_status_through_dispatch() {
    let server = server(9);
    let (mut a, _kit) = Device::first("a", 101, &server);
    let personal = a.space_ids()[0];
    // Not configured: disabled, no SyncRequested on edits.
    let page = a.new_page(personal, None);
    let info = match a.core.dispatch(Request::SyncStatus).unwrap() {
        Response::Sync(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(info.state, SyncState::Disabled);
    assert!(!info.configured && info.server_url.is_none());

    // Bad addresses and tokens.
    for bad in [
        "ftp://x",
        "http://user@host",
        "http://h/?q",
        "",
        "https://h/#f",
    ] {
        let err = a
            .core
            .dispatch(Request::SyncConfigure {
                server_url: Some(bad.into()),
                invite_token: None,
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput, "{bad}");
    }
    let err = a
        .core
        .dispatch(Request::SyncConfigure {
            server_url: Some(SERVER_URL.into()),
            invite_token: Some("tab\tbad".into()),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);

    // Configure: Idle, SyncChanged then SyncRequested; the token is taken once.
    a.core.drain_events();
    let info = match a
        .core
        .dispatch(Request::SyncConfigure {
            server_url: Some(format!("{SERVER_URL}/")),
            invite_token: Some(" dev ".into()),
        })
        .unwrap()
    {
        Response::Sync(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(info.state, SyncState::Idle);
    assert_eq!(info.server_url.as_deref(), Some(SERVER_URL));
    assert!(
        info.pending_entries > 0,
        "the local documents wait: {info:?}"
    );
    let events = a.core.drain_events();
    assert!(events.contains(&Event::SyncChanged) && events.contains(&Event::SyncRequested));
    assert_eq!(
        a.core.sync_take_invite_token().unwrap().as_deref(),
        Some("dev")
    );
    assert_eq!(a.core.sync_take_invite_token().unwrap(), None);

    // Local edits ask for a cycle, once per batch.
    a.add_paragraph(page);
    a.core.page_doc_open(page).unwrap();
    write_text(&mut a.core, page, "more", 999);
    let events = a.core.drain_events();
    assert_eq!(
        events
            .iter()
            .filter(|e| **e == Event::SyncRequested)
            .count(),
        1,
        "{events:?}"
    );
    a.core.dispatch(Request::SyncNow).unwrap();
    assert!(a.core.drain_events().contains(&Event::SyncRequested));

    // The driver reports states; an unknown account asks for a token.
    a.core.sync_account_unknown().unwrap();
    let info = a.core.sync_info().unwrap();
    assert!(info.needs_invite_token);
    assert_eq!(info.state, SyncState::Error);
    assert_eq!(
        info.last_error.as_deref(),
        Some(crate::ACCOUNT_UNKNOWN_MESSAGE)
    );
    a.core
        .sync_configure(Some(SERVER_URL), Some("dev".into()))
        .unwrap();
    assert!(!a.core.sync_info().unwrap().needs_invite_token);
    a.core
        .sync_set_state(SyncState::Offline, Some("cannot reach the server".into()))
        .unwrap();
    let info = a.core.sync_info().unwrap();
    assert_eq!(info.state, SyncState::Offline);
    assert_eq!(info.last_error.as_deref(), Some("cannot reach the server"));

    // Disabling forgets the remote state; the status is Disabled again.
    a.cycle().unwrap();
    assert!(a.core.sync_info().unwrap().remote_cursor > 0);
    a.core
        .dispatch(Request::SyncConfigure {
            server_url: None,
            invite_token: None,
        })
        .unwrap();
    let info = a.core.sync_info().unwrap();
    assert_eq!(info.state, SyncState::Disabled);
    assert_eq!(info.remote_cursor, 0);
    assert!(info.server_epoch.is_none());
    assert!(a.core.db.sync_seen_clear().is_ok());

    // No kit, no sync.
    let fx = Fixture::new();
    let mut fresh = fx.open(3, 5);
    create_workspace(&mut fresh, "W", Some("S"));
    let err = fresh
        .dispatch(Request::SyncConfigure {
            server_url: Some(SERVER_URL.into()),
            invite_token: None,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn an_unknown_account_report_that_a_token_overtook_asks_for_no_token() {
    // The cycle found the account unknown and had taken no token; before it reported that, the person typed one
    // (a second `SyncConfigure`). The late report must not make the status ask for a token again: the next cycle
    // takes the one that is waiting and makes the account, and nothing else would clear the flag.
    let server = server(90);
    let (mut a, _kit) = Device::first("a", 901, &server);
    a.core
        .sync_configure(Some(SERVER_URL), None)
        .expect("configure without a token");
    assert_eq!(a.core.sync_take_invite_token().unwrap(), None);
    a.core
        .sync_configure(Some(SERVER_URL), Some("dev".into()))
        .expect("configure with the token");
    a.core.sync_account_unknown().unwrap();
    let info = a.core.sync_info().unwrap();
    assert!(!info.needs_invite_token, "{info:?}");
    assert_ne!(info.state, SyncState::Error, "{info:?}");
    assert_eq!(
        a.core.sync_take_invite_token().unwrap().as_deref(),
        Some("dev"),
        "the token is still waiting for the next cycle"
    );
    // With no token waiting, an unknown account asks for one as before.
    a.core.sync_account_unknown().unwrap();
    let info = a.core.sync_info().unwrap();
    assert!(info.needs_invite_token);
    assert_eq!(info.state, SyncState::Error);
}

#[test]
fn the_configuration_survives_a_restart_and_compaction_waits_for_the_push() {
    let server = server(10);
    let (mut a, _kit) = Device::first("a", 111, &server);
    let personal = a.space_ids()[0];
    let page = a.new_page(personal, None);
    a.configure();
    a.cycle().unwrap();
    // A small compaction policy: without sync the log would be folded at once.
    a.core.set_compaction_policy(crate::CompactionPolicy {
        max_updates: 3,
        max_bytes: 1 << 20,
    });
    for _ in 0..6 {
        a.add_paragraph(page);
    }
    let doc = kb_types::DocId::for_page(page);
    let stats = kb_storage::DocQueries::update_stats(&a.core.db, doc).unwrap();
    assert!(
        stats.count >= 6,
        "unpushed updates stay in the log: {stats:?}"
    );
    a.cycle().unwrap();
    a.add_paragraph(page);
    let stats = kb_storage::DocQueries::update_stats(&a.core.db, doc).unwrap();
    assert!(
        stats.count <= 2,
        "after the push the log is compacted: {stats:?}"
    );

    // Restart: the configuration, cursor and state come back.
    let cursor = a.core.sync_info().unwrap().remote_cursor;
    let fx = std::mem::replace(&mut a._fx, Fixture::new());
    let deps = fx.deps(111, 2_000_000);
    let reopened = match Core::open_or_locked(deps).unwrap() {
        Opened::Ready(core) => core,
        Opened::Locked(_) => panic!("no lock"),
    };
    let info = reopened.sync_info().unwrap();
    assert!(info.configured);
    assert_eq!(info.state, SyncState::Idle);
    assert_eq!(info.remote_cursor, cursor);
    assert_eq!(
        info.pending_entries, 1,
        "the paragraph written after the push"
    );
    drop(reopened);
    drop(fx);
}

#[test]
#[ignore = "10,000 pages: half an hour in a debug build (signatures); run with --ignored, KB_SYNC_PAGES overrides the count"]
fn ten_thousand_pages_cold_sync_to_a_new_device_from_snapshots() {
    let pages: u64 = std::env::var("KB_SYNC_PAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let server = server(11);
    let (mut a, _kit) = Device::first("a", 121, &server);
    let mut space_ids = vec![a.space_ids()[0]];
    space_ids.push(create_space(&mut a.core, "Work").id);
    space_ids.push(create_space(&mut a.core, "Archive").id);
    let started = Instant::now();
    let mut with_text = Vec::new();
    for i in 0..pages {
        let space = space_ids[(i % 3) as usize];
        let page = create_page(&mut a.core, space, None, &format!("Page {i}"));
        if i % 50 == 0 {
            write_text(&mut a.core, page.id, &format!("body of page {i}"), i);
            with_text.push(page.id);
        }
        if i % 1000 == 0 {
            a.core.drain_events();
        }
    }
    a.core.drain_events();
    eprintln!("created {pages} pages in {:?}", started.elapsed());
    a.configure();
    let started = Instant::now();
    a.cycle().unwrap();
    eprintln!("a uploaded everything in {:?}", started.elapsed());

    let mut d = Device::fresh("d", 124, &server, &mut a);
    d.configure();
    let started = Instant::now();
    d.cycle().unwrap();
    let elapsed = started.elapsed();
    eprintln!("d cold-synced in {elapsed:?}");
    let mut total = 0;
    for space in d.space_ids() {
        total += d.live_pages(space).len();
    }
    assert_eq!(total as u64, pages);
    for page in with_text.iter().step_by(40) {
        assert!(
            page_content(&mut d.core, *page)
                .to_string()
                .contains("body of page")
        );
    }
    // The number is what matters (recorded in the milestone retrospective); the bound
    // only catches a hang.
    assert!(elapsed.as_secs() < 1_800, "cold sync took {elapsed:?}");
}

// ----- Milestone 7: devices ---------------------------------------------------------------

/// A pairing mailbox as the server keeps it: messages by sequence with the sender's role.
#[derive(Default)]
struct Mailbox {
    id: Option<[u8; 6]>,
    messages: Vec<(u32, PairingRole, Vec<u8>)>,
    /// Next sequence to deliver, per device name.
    next: std::collections::HashMap<&'static str, u32>,
}

impl Device {
    fn pairing(&mut self) -> kb_types::PairingInfo {
        match self.core.dispatch(Request::PairingStatus).unwrap() {
            Response::Pairing(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn security(&mut self) -> kb_types::SecurityInfo {
        match self.core.dispatch(Request::SecurityStatus).unwrap() {
            Response::Security(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn devices(&mut self) -> Vec<kb_types::DeviceEntry> {
        match self.core.dispatch(Request::DeviceList).unwrap() {
            Response::Devices(list) => list,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Publishes the unpublished suffix of the device log, as the driver and the
    /// pairing task do.
    fn publish_log(&mut self) -> Sim {
        let (account, _) = self.core.sync_identity()?.expect("an account");
        let (entries, published) = self.core.sync_account_log()?;
        if (published as usize) < entries.len() {
            self.remote
                .append_log_sync(account, &entries[published as usize..])?;
            self.core.sync_log_published(entries.len() as u64)?;
        }
        Ok(())
    }
}

/// Runs a pairing between a fresh installation and a sponsor through an in-memory
/// mailbox the way the kb-net task does: the joiner opens the mailbox with its offer,
/// the sponsor types the code, both confirm when the digits show, the sponsor publishes
/// its log before the bundle goes.
fn run_pairing(joiner: &mut Device, sponsor: &mut Device) -> Sim {
    let mut mailbox = Mailbox::default();
    joiner.core.dispatch(Request::PairingStart {
        role: PairingRole::Joiner,
        server_url: Some(SERVER_URL.into()),
        code: None,
    })?;
    joiner.core.drain_events();
    let mut sponsor_started = false;
    for _ in 0..60 {
        // The joiner's outbox: the offer opens the mailbox.
        for (seq, bytes) in joiner.core.pairing_outbox()? {
            if seq == 0 && mailbox.id.is_none() {
                let id = [0x42, 0x11, 0x22, 0x33, 0x44, 0x55];
                mailbox.id = Some(id);
                mailbox.messages.push((0, PairingRole::Joiner, bytes));
                joiner.core.pairing_mailbox_opened(id, 2_000_000)?;
            } else {
                mailbox.messages.push((seq, PairingRole::Joiner, bytes));
            }
            joiner.core.pairing_posted(seq)?;
        }
        // The sponsor types the code once it shows.
        if !sponsor_started && let Some(code) = joiner.pairing().code {
            sponsor.core.dispatch(Request::PairingStart {
                role: PairingRole::Sponsor,
                server_url: None,
                code: Some(code),
            })?;
            sponsor.core.drain_events();
            let id = sponsor
                .core
                .pairing_mailbox_id()?
                .expect("the code names the mailbox");
            assert_eq!(Some(id), mailbox.id, "the code carries the mailbox id");
            sponsor.core.pairing_mailbox_opened(id, 2_000_000)?;
            sponsor_started = true;
        }
        if sponsor_started {
            for (seq, bytes) in sponsor.core.pairing_outbox()? {
                if seq == 3 {
                    sponsor.publish_log()?;
                }
                mailbox.messages.push((seq, PairingRole::Sponsor, bytes));
                sponsor.core.pairing_posted(seq)?;
            }
        }
        // Delivery.
        for (device, role) in [
            (&mut *joiner, PairingRole::Joiner),
            (&mut *sponsor, PairingRole::Sponsor),
        ] {
            if role == PairingRole::Sponsor && !sponsor_started {
                continue;
            }
            let next = mailbox.next.entry(device.name).or_insert(0);
            let pending: Vec<(u32, PairingRole, Vec<u8>)> = mailbox
                .messages
                .iter()
                .filter(|(seq, from, _)| *seq >= *next && *from != role)
                .cloned()
                .collect();
            for (seq, from, bytes) in pending {
                device.core.pairing_receive(seq, from, &bytes)?;
                *next = seq + 1;
            }
            // Messages of our own role advance the pointer too.
            if let Some(max) = mailbox.messages.iter().map(|(s, _, _)| *s).max() {
                *next = (*next).max(max + 1).min(max + 1);
                let _ = max;
            }
            device.core.drain_events();
        }
        // Confirmations.
        let j = joiner.pairing();
        if j.state == PairingState::Confirming {
            assert!(j.sas.is_some(), "the joiner shows the digits");
            joiner.core.dispatch(Request::PairingConfirm)?;
            joiner.core.drain_events();
        }
        if sponsor_started {
            let s = sponsor.pairing();
            if s.state == PairingState::Confirming && s.peer_name.is_some() {
                assert_eq!(
                    s.sas,
                    joiner.pairing().sas,
                    "both sides show the same digits"
                );
                sponsor.core.dispatch(Request::PairingConfirm)?;
                sponsor.core.drain_events();
            }
        }
        let j = joiner.pairing();
        let s = sponsor.pairing();
        if j.state == PairingState::Done && s.state == PairingState::Done {
            return Ok(());
        }
        for (name, info) in [("joiner", &j), ("sponsor", &s)] {
            assert!(
                !matches!(info.state, PairingState::Failed | PairingState::Cancelled),
                "{name} pairing {:?}: {:?}",
                info.state,
                info.error
            );
        }
    }
    panic!(
        "pairing did not finish: joiner {:?}, sponsor {:?}",
        joiner.pairing().state,
        sponsor.pairing().state
    );
}

/// Recovers an account on a fresh installation with the kit, through the server, the
/// way the kb-net recovery task does.
fn run_recovery(device: &mut Device, kit: &str) -> Sim {
    device.core.dispatch(Request::RecoveryStart {
        server_url: SERVER_URL.into(),
        kit: kit.into(),
    })?;
    device.core.drain_events();
    let id = device.core.recovery_id()?.expect("a recovery id");
    let Some(blob) = device.remote.recovery_get_sync(id)? else {
        device
            .core
            .recovery_failed("this server does not know that kit")?;
        return Err(SimError::Sync(SyncError::NotFound));
    };
    device.core.recovery_blob_received(&blob)?;
    let account = device
        .core
        .recovery_account_id()?
        .expect("the blob names the account");
    let log = device.remote.fetch_log_sync(account, None)?;
    let entry = device.core.recovery_log_received(&log)?;
    device.remote.append_log_sync(account, &[entry])?;
    device.core.recovery_entry_published()?;
    device.core.drain_events();
    Ok(())
}

fn revoke(
    device: &mut Device,
    target: kb_types::DeviceId,
    passphrase: Option<&str>,
) -> kb_types::RecoveryKit {
    match device
        .core
        .dispatch(Request::DeviceRevoke {
            device_id: target,
            passphrase: passphrase.map(str::to_owned),
        })
        .unwrap()
    {
        Response::RecoveryKit(kit) => kit,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_fresh_device_pairs_by_code_and_syncs() {
    let server = server(21);
    let (mut a, _kit) = Device::first("a", 211, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    a.add_paragraph(p1);
    a.configure();
    a.cycle().unwrap();

    let mut j = Device::unjoined("j", 212, &server);
    run_pairing(&mut j, &mut a).unwrap();
    assert_eq!(j.security().account_id, a.security().account_id);
    let listed = a.devices();
    let joined = listed
        .iter()
        .find(|d| d.device_id == j.core.device.device_id)
        .expect("the sponsor lists the new device");
    assert_eq!(joined.added_by, Some(a.core.device.device_id));
    assert!(!joined.added_via_kit);
    assert_eq!(joined.name, j.core.device.name);
    assert!(
        j.core.sync_info().unwrap().configured,
        "pairing configures sync"
    );

    j.cycle().unwrap();
    settle(&mut [&mut a, &mut j]);
    assert!(
        page_content(&mut j.core, p1)
            .to_string()
            .contains("a wrote")
    );
    j.add_paragraph(p1);
    settle(&mut [&mut a, &mut j]);
    assert!(
        page_content(&mut a.core, p1)
            .to_string()
            .contains("j wrote")
    );
    assert_eq!(j.pairing().state, PairingState::Done);
}

#[test]
fn a_joiner_keeps_every_snapshot_that_waits_for_its_key() {
    // Several snapshots cover the same cursor (a device publishes them in one cycle), and
    // a device that joins afterwards meets them before the keyring entries that open
    // them. Every one of them must wait and apply, not the last one only (the real-app
    // pass of 2026-10-05 lost the page's snapshot that way). After a server reset a
    // device pushes its compacted update logs as they are, so the snapshots are the only
    // copy of the pages' history.
    let server = server(28);
    let (mut a, _kit) = Device::first("a", 281, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    let p2 = a.new_page(personal, None);
    a.add_paragraph(p1);
    a.add_paragraph(p2);
    a.configure();
    a.cycle().unwrap();
    a.core.set_compaction_policy(crate::CompactionPolicy {
        max_updates: 1,
        max_bytes: 1 << 20,
    });
    for _ in 0..3 {
        a.add_paragraph(p1);
        a.add_paragraph(p2);
        a.cycle().unwrap();
    }
    for page in [p1, p2] {
        let doc = kb_types::DocId::for_page(page);
        let stats = kb_storage::DocQueries::update_stats(&a.core.db, doc).unwrap();
        assert!(
            stats.count <= 2,
            "the log of {page} is compacted: {stats:?}"
        );
    }

    server.reset_data();
    a.authed = false;
    a.cycle().unwrap();
    let snapshots = a.remote.snapshots_sync(a.personal()).unwrap().snapshots;
    assert!(snapshots.len() >= 3, "{} snapshots", snapshots.len());
    for bytes in &snapshots {
        let header = kb_sync::v2::peek_snapshot(bytes).unwrap();
        assert_eq!(
            header.covers_cursor, 0,
            "every snapshot covers the same cursor"
        );
    }

    let mut j = Device::unjoined("j", 282, &server);
    run_pairing(&mut j, &mut a).unwrap();
    j.cycle().unwrap();
    settle(&mut [&mut a, &mut j]);
    for page in [p1, p2] {
        let text = page_content(&mut j.core, page).to_string();
        assert_eq!(
            text.matches("a wrote").count(),
            4,
            "page {page} on the joiner: {text:?}"
        );
    }
}

#[test]
fn a_device_never_snapshots_a_document_whose_history_it_lacks() {
    // After a server reset a device pushes its own rows only and a snapshot carries the
    // rest, so a device that meets the rows without the snapshot holds changes whose
    // dependencies it lacks. They wait (parked, nothing persisted), the document is not
    // snapshotted by that device meanwhile (a snapshot of it would claim to cover the
    // feed while missing the history, the server keeps the snapshot that covers most,
    // and every device joining later would get an empty page for good, as the real-app
    // pass of 2026-10-05 showed), and the snapshots are fetched again until one brings
    // the history.
    let server = server(30);
    let (mut a, kit) = Device::first("a", 301, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    a.add_paragraph(p1);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 302, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    b.add_paragraph(p1);
    settle(&mut [&mut a, &mut b]);
    a.add_paragraph(p1);
    settle(&mut [&mut a, &mut b]);

    // The server loses its data; a (b is gone) pushes its own rows, which depend on
    // b's, and a snapshot of the page, which the server then loses too.
    server.reset_data();
    a.authed = false;
    a.cycle().unwrap();
    let doc = kb_types::DocId::for_page(p1);
    assert!(server.forget_snapshot(a.personal(), doc));

    let mut j = Device::unjoined("j", 303, &server);
    run_pairing(&mut j, &mut a).unwrap();
    j.cycle().unwrap();
    j.cycle().unwrap();
    let info = j.core.sync_info().unwrap();
    assert!(info.parked_items > 0, "{info:?}");
    let text = page_content(&mut j.core, p1).to_string();
    assert!(
        !text.contains("wrote"),
        "the rows without their history wait: {text:?}"
    );
    let listed = j.remote.snapshots_sync(j.personal()).unwrap().snapshots;
    assert!(
        listed
            .iter()
            .all(|bytes| kb_sync::v2::peek_snapshot(bytes).unwrap().doc_id != doc),
        "j did not snapshot the page whose history it lacks"
    );

    // a snapshots the page again: j fetches it for its parked rows and completes the
    // page; the parked rows are superseded.
    a.publish_snapshot(doc);
    settle(&mut [&mut a, &mut j]);
    assert_eq!(j.core.sync_info().unwrap().parked_items, 0);
    let text = page_content(&mut j.core, p1).to_string();
    assert_eq!(text.matches("a wrote").count(), 2, "{text:?}");
    assert_eq!(text.matches("b wrote").count(), 1, "{text:?}");
}

/// Pseudo-random bytes (xorshift), `len` of them.
fn attachment_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.max(1);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

// ----- Milestone 8: attachment sync ------------------------------------------------------

#[test]
fn an_attachment_uploaded_on_one_device_is_readable_on_the_other() {
    let server = server(40);
    let (mut a, kit) = Device::first("a", 401, &server);
    let personal = a.space_ids()[0];
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 402, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);

    let bytes = attachment_bytes(CHUNK_SIZE + 12_345, 1);
    let info = a.upload_attachment(personal, "photo.bin", &bytes);
    assert_eq!(info.chunk_count, 2);
    assert_eq!(info.availability, AttachmentAvailability::Complete);
    assert_eq!(a.core.sync_info().unwrap().transfers.uploads, 2);
    a.cycle().unwrap();
    assert_eq!(a.core.sync_info().unwrap().transfers.uploads, 0);
    assert_eq!(a.attachment(info.attachment_id).local_chunks, 2);

    b.cycle().unwrap();
    let on_b = b.attachment(info.attachment_id);
    assert_eq!(
        on_b.availability,
        AttachmentAvailability::Complete,
        "{on_b:?}"
    );
    assert_eq!(on_b.local_chunks, 2);
    assert_eq!(b.read_attachment(info.attachment_id).unwrap(), bytes);
    assert_eq!(b.core.sync_info().unwrap().transfers, Default::default());
    settle(&mut [&mut a, &mut b]);
}

#[test]
fn a_large_attachment_resumes_after_failures_and_a_restart() {
    let server = server(41);
    let (mut a, kit) = Device::first("a", 411, &server);
    let personal = a.space_ids()[0];
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 412, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);

    // Nine chunks: more than one budget on both sides.
    let bytes = attachment_bytes(8 * CHUNK_SIZE + 1, 2);
    let info = a.upload_attachment(personal, "big.bin", &bytes);
    assert_eq!(info.chunk_count, 9);
    // Every third chunk call fails on the way up: the cycle errors, the next one
    // continues where it stopped.
    for _ in 0..3 {
        server.fail_chunk_ops(1);
        let _ = a.cycle();
    }
    while a.core.sync_info().unwrap().transfers.uploads > 0 {
        a.cycle().unwrap();
    }
    assert_eq!(server.chunk_calls().puts, 9, "every chunk went up once");

    // The second device meets the manifest, downloads two batches, is restarted in the
    // middle and completes from the state table.
    b.cycle().unwrap_or_else(|e| panic!("{e:?}"));
    let partial = b.attachment(info.attachment_id);
    assert!(partial.local_chunks < 9 || partial.availability == AttachmentAvailability::Complete);
    b.reopen(412, 2_000_000);
    let after = b.attachment(info.attachment_id);
    assert!(
        after.local_chunks >= partial.local_chunks,
        "nothing is lost by a restart"
    );
    server.fail_chunk_ops(1);
    let _ = b.cycle();
    while b.attachment(info.attachment_id).availability != AttachmentAvailability::Complete {
        b.cycle().unwrap_or_else(|e| panic!("{e:?}"));
    }
    assert_eq!(b.read_attachment(info.attachment_id).unwrap(), bytes);
    assert_eq!(b.core.sync_info().unwrap().transfers, Default::default());
    settle(&mut [&mut a, &mut b]);
}

#[test]
fn an_on_demand_device_fetches_an_attachment_only_when_asked() {
    let server = server(42);
    let (mut a, kit) = Device::first("a", 421, &server);
    let personal = a.space_ids()[0];
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 422, &server, &mut a, &kit);
    b.configure();
    b.set_download_policy(DOWNLOAD_POLICY_ON_DEMAND);
    settle(&mut [&mut a, &mut b]);

    let bytes = attachment_bytes(10_000, 3);
    let first = a.upload_attachment(personal, "first.bin", &bytes);
    let second = a.upload_attachment(personal, "second.bin", &bytes);
    a.cycle().unwrap();
    b.cycle().unwrap();
    b.cycle().unwrap();
    let before = b.attachment(first.attachment_id);
    assert_eq!(
        before.availability,
        AttachmentAvailability::NotDownloaded,
        "{before:?}"
    );
    assert_eq!(before.local_chunks, 0);
    assert_eq!(b.core.sync_info().unwrap().transfers, Default::default());
    assert_eq!(server.chunk_calls().gets, 0, "nothing fetched on demand");

    // Asking for it fetches it; a read of the other one is itself a demand.
    let asked = match b
        .core
        .dispatch(Request::AttachmentDownload {
            attachment_id: first.attachment_id,
        })
        .unwrap()
    {
        Response::Attachment(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(asked.availability, AttachmentAvailability::Downloading);
    assert!(
        b.core
            .drain_events()
            .iter()
            .any(|e| matches!(e, Event::SyncRequested))
    );
    let err = b.read_attachment(second.attachment_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Storage);
    assert!(err.message.contains("not complete"), "{err}");
    assert_eq!(b.core.sync_info().unwrap().transfers.downloads, 2);
    b.cycle().unwrap();
    assert_eq!(b.read_attachment(first.attachment_id).unwrap(), bytes);
    assert_eq!(b.read_attachment(second.attachment_id).unwrap(), bytes);

    // Back to eager: a new attachment comes by itself.
    b.set_download_policy("eager");
    let third = a.upload_attachment(personal, "third.bin", &bytes);
    a.cycle().unwrap();
    b.cycle().unwrap();
    b.cycle().unwrap();
    assert_eq!(
        b.attachment(third.attachment_id).availability,
        AttachmentAvailability::Complete
    );
    settle(&mut [&mut a, &mut b]);
}

#[test]
fn a_device_waits_for_chunks_the_server_does_not_hold_yet() {
    let server = server(43);
    let (mut a, kit) = Device::first("a", 431, &server);
    let personal = a.space_ids()[0];
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 432, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);

    // a publishes the manifest but its chunk uploads fail: b meets a manifest whose
    // chunks the server lacks.
    let bytes = attachment_bytes(5_000, 4);
    let info = a.upload_attachment(personal, "late.bin", &bytes);
    server.fail_chunk_ops(1);
    let _ = a.cycle();
    b.cycle().unwrap();
    let waiting = b.attachment(info.attachment_id);
    assert_eq!(
        waiting.availability,
        AttachmentAvailability::Waiting,
        "{waiting:?}"
    );
    assert_eq!(b.core.sync_info().unwrap().transfers.waiting, 1);
    // Not asked again before the delay.
    let gets = server.chunk_calls().gets;
    b.cycle().unwrap();
    assert_eq!(
        server.chunk_calls().gets,
        gets,
        "the chunk is not asked for again at once"
    );

    // a finishes; after the delay b completes.
    a.cycle().unwrap();
    b.reopen(432, 1_000_000 + 10 * 60_000);
    b.cycle().unwrap();
    let done = b.attachment(info.attachment_id);
    assert_eq!(
        done.availability,
        AttachmentAvailability::Complete,
        "{done:?}"
    );
    assert_eq!(b.read_attachment(info.attachment_id).unwrap(), bytes);
    settle(&mut [&mut a, &mut b]);
}

#[test]
fn a_purge_deletes_the_chunks_on_the_server_and_the_other_device_sweeps_its_copies() {
    let server = server(44);
    let (mut a, kit) = Device::first("a", 441, &server);
    let personal = a.space_ids()[0];
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 442, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);

    let bytes = attachment_bytes(20_000, 5);
    let doomed = a.upload_attachment(personal, "doomed.bin", &bytes);
    let kept = a.upload_attachment(personal, "kept.bin", &bytes);
    settle(&mut [&mut a, &mut b]);
    let doomed_chunks: Vec<_> = a
        .core
        .structure(personal)
        .unwrap()
        .model
        .attachment(doomed.attachment_id)
        .unwrap()
        .chunks;
    let kept_chunks: Vec<_> = a
        .core
        .structure(personal)
        .unwrap()
        .model
        .attachment(kept.attachment_id)
        .unwrap()
        .chunks;
    let feed = a.personal();
    assert!(doomed_chunks.iter().all(|c| server.has_chunk(feed, *c)));
    assert_eq!(b.read_attachment(doomed.attachment_id).unwrap(), bytes);

    a.core
        .dispatch(Request::AttachmentDelete {
            attachment_id: doomed.attachment_id,
        })
        .unwrap();
    a.core.drain_events();
    a.cycle().unwrap();
    assert!(
        doomed_chunks.iter().all(|c| !server.has_chunk(feed, *c)),
        "the server dropped the purged chunks"
    );
    assert!(
        kept_chunks.iter().all(|c| server.has_chunk(feed, *c)),
        "the other attachment's chunks stay"
    );
    assert_eq!(server.chunk_calls().deletes, doomed_chunks.len() as u64);

    b.cycle().unwrap();
    assert_eq!(
        b.core
            .dispatch(Request::AttachmentGet {
                attachment_id: doomed.attachment_id
            })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    b.core.dispatch(Request::AttachmentGc).unwrap();
    assert!(
        doomed_chunks
            .iter()
            .all(|c| !b.core.blob_store.has(c).unwrap()),
        "b swept its copies"
    );
    assert_eq!(b.read_attachment(kept.attachment_id).unwrap(), bytes);
    settle(&mut [&mut a, &mut b]);
}

#[test]
fn a_server_reset_re_uploads_the_chunks() {
    let server = server(45);
    let (mut a, kit) = Device::first("a", 451, &server);
    let personal = a.space_ids()[0];
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 452, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    let bytes = attachment_bytes(30_000, 6);
    let info = a.upload_attachment(personal, "again.bin", &bytes);
    settle(&mut [&mut a, &mut b]);
    let chunks: Vec<_> = a
        .core
        .structure(personal)
        .unwrap()
        .model
        .attachment(info.attachment_id)
        .unwrap()
        .chunks;

    let feed = a.personal();
    server.reset_data();
    a.authed = false;
    b.authed = false;
    assert!(chunks.iter().all(|c| !server.has_chunk(feed, *c)));
    a.cycle().unwrap();
    assert!(
        chunks.iter().all(|c| server.has_chunk(feed, *c)),
        "a uploaded its chunks again"
    );
    b.cycle().unwrap();
    assert_eq!(b.read_attachment(info.attachment_id).unwrap(), bytes);
    settle(&mut [&mut a, &mut b]);
}

#[test]
fn a_mistyped_or_foreign_code_is_refused_and_a_joiner_needs_a_fresh_installation() {
    let server = server(22);
    let (mut a, _kit) = Device::first("a", 221, &server);
    a.configure();
    a.cycle().unwrap();
    let err = a
        .core
        .dispatch(Request::PairingStart {
            role: PairingRole::Sponsor,
            server_url: None,
            code: Some("ABCD EFGH JKMN".into()),
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput, "{err}");
    let err = a
        .core
        .dispatch(Request::PairingStart {
            role: PairingRole::Joiner,
            server_url: Some(SERVER_URL.into()),
            code: None,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::AlreadyExists, "{err}");
    let mut j = Device::unjoined("j", 222, &server);
    let err = j
        .core
        .dispatch(Request::PairingStart {
            role: PairingRole::Joiner,
            server_url: Some("ftp://x".into()),
            code: None,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    j.core
        .dispatch(Request::PairingStart {
            role: PairingRole::Joiner,
            server_url: Some(SERVER_URL.into()),
            code: None,
        })
        .unwrap();
    assert_eq!(j.pairing().state, PairingState::Starting);
    j.core.dispatch(Request::PairingCancel).unwrap();
    assert_eq!(j.pairing().state, PairingState::Cancelled);
}

#[test]
fn revoking_a_device_rotates_the_keys_and_shuts_it_out() {
    let server = server(23);
    let (mut a, kit) = Device::first("a", 231, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    a.add_paragraph(p1);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 232, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    let mut c = Device::from_backup("c", 233, &server, &mut a, &kit);
    c.configure();
    settle(&mut [&mut a, &mut b, &mut c]);
    let b_id = b.core.device.device_id;
    let before = page_content(&mut b.core, p1);

    // a revokes b: a new kit, generation 1, a new epoch for the space.
    let new_kit = revoke(&mut a, b_id, None);
    assert_ne!(new_kit.words.join(" "), kit);
    assert_eq!(a.security().root_generation, 1);
    assert!(
        a.core
            .dispatch(Request::RecoveryKitVerify { kit: kit.clone() })
            .is_err(),
        "the old kit is void"
    );
    a.core
        .dispatch(Request::RecoveryKitVerify {
            kit: new_kit.words.join(" "),
        })
        .unwrap();
    let revoked = a
        .devices()
        .into_iter()
        .find(|d| d.device_id == b_id)
        .unwrap();
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoked.revoked_by, Some(a.core.device.device_id));
    assert_eq!(a.core.space_epoch(personal), 1);
    a.add_paragraph(p1);
    a.cycle().unwrap();

    // c applies the rotation from the log and keeps syncing.
    c.cycle().unwrap();
    let info = c.security();
    assert_eq!(info.root_generation, 1, "{info:?}");
    assert!(!info.rotation_pending);
    settle(&mut [&mut a, &mut c]);
    let text = page_content(&mut c.core, p1).to_string();
    assert_eq!(text.matches("a wrote line").count(), 2);
    c.add_paragraph(p1);
    settle(&mut [&mut a, &mut c]);
    assert!(
        page_content(&mut a.core, p1)
            .to_string()
            .contains("c wrote")
    );

    // b, handed the feed by someone who bypasses the server, cannot read what came
    // after the rotation; the server itself refuses it.
    let cursor = b.core.sync_info().unwrap().remote_cursor;
    let feed = b.personal();
    let page = a.remote.changes_sync(feed, cursor, 200).unwrap();
    let report = b.core.sync_feed_apply(feed, &page).unwrap();
    assert!(report.pending > 0, "{report:?}");
    assert_eq!(report.applied, 0, "{report:?}");
    assert_eq!(page_content(&mut b.core, p1), before);
    let err = b.cycle().unwrap_err();
    assert!(
        matches!(err, SimError::Sync(SyncError::Unauthorized)),
        "{err:?}"
    );
    // The log tells b what happened.
    let (account, _) = b.core.sync_identity().unwrap().unwrap();
    let log = a.remote.fetch_log_sync(account, None).unwrap();
    b.core.sync_log_received(&log).unwrap();
    let info = b.security();
    assert!(info.this_device_revoked, "{info:?}");
    assert!(
        !b.core.sync_info().unwrap().configured,
        "sync is off on the removed device"
    );
    assert_eq!(page_content(&mut b.core, p1), before, "the data stays");
    let events = b.core.drain_events();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::DevicesChanged {
            this_device_revoked: true,
            ..
        }
    )));
}

#[test]
fn a_locked_device_applies_a_rotation_after_the_passphrase() {
    let server = server(24);
    let (mut a, kit) = Device::first("a", 241, &server);
    let personal = a.space_ids()[0];
    let p1 = a.new_page(personal, None);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 242, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    let mut c = Device::from_backup("c", 243, &server, &mut a, &kit);
    c.configure();
    settle(&mut [&mut a, &mut b, &mut c]);
    c.core
        .dispatch(Request::AppLockEnable {
            passphrase: "correct horse battery".into(),
        })
        .unwrap();
    c.core.drain_events();

    revoke(&mut a, b.core.device.device_id, None);
    a.add_paragraph(p1);
    a.cycle().unwrap();
    c.cycle().unwrap();
    let info = c.security();
    assert!(info.rotation_pending, "{info:?}");
    assert_eq!(info.root_generation, 0);
    let err = c
        .core
        .dispatch(Request::RotationApply { passphrase: None })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    let err = c
        .core
        .dispatch(Request::RotationApply {
            passphrase: Some("wrong passphrase".into()),
        })
        .unwrap_err();
    assert_ne!(err.code, ErrorCode::Internal, "{err}");
    match c
        .core
        .dispatch(Request::RotationApply {
            passphrase: Some("correct horse battery".into()),
        })
        .unwrap()
    {
        Response::Security(info) => {
            assert_eq!(info.root_generation, 1);
            assert!(!info.rotation_pending);
        }
        other => panic!("unexpected {other:?}"),
    }
    settle(&mut [&mut a, &mut c]);
    assert!(
        page_content(&mut c.core, p1)
            .to_string()
            .contains("a wrote")
    );
}

#[test]
fn recovery_with_the_kit_from_the_server_after_a_rotation() {
    let server = server(25);
    let (mut a, kit) = Device::first("a", 251, &server);
    let personal = a.space_ids()[0];
    let work = create_space(&mut a.core, "Work").id;
    let p1 = a.new_page(personal, None);
    let p2 = a.new_page(work, None);
    a.add_paragraph(p1);
    a.add_paragraph(p2);
    a.configure();
    a.cycle().unwrap();
    assert!(
        a.security().recovery_published,
        "the cycle uploaded the recovery blob"
    );

    // A fresh device recovers with the first kit.
    let mut d = Device::unjoined("d", 254, &server);
    run_recovery(&mut d, &kit).unwrap();
    let joined = d.devices().into_iter().find(|e| e.is_this_device).unwrap();
    assert!(joined.added_via_kit);
    d.cycle().unwrap();
    settle(&mut [&mut a, &mut d]);
    assert_eq!(d.space_ids().len(), 2);
    assert!(
        page_content(&mut d.core, p2)
            .to_string()
            .contains("a wrote")
    );

    // A revokes d and issues a new kit; the old kit no longer recovers, the new one
    // does, and the new device reads everything written before the rotation.
    let new_kit = revoke(&mut a, d.core.device.device_id, None);
    a.add_paragraph(p1);
    a.cycle().unwrap();
    a.cycle().unwrap();
    assert!(
        a.core.sync_recovery_obsolete().unwrap().is_empty(),
        "the old blob was deleted"
    );
    let old_id = kb_keys::RecoveryKey::from_kit(&kit).unwrap().recovery_id();
    assert!(a.remote.recovery_get_sync(old_id).unwrap().is_none());
    let mut e = Device::unjoined("e", 255, &server);
    assert!(
        run_recovery(&mut e, &kit).is_err(),
        "the old kit finds no blob"
    );
    let mut e = Device::unjoined("e2", 256, &server);
    run_recovery(&mut e, &new_kit.words.join(" ")).unwrap();
    e.cycle().unwrap();
    settle(&mut [&mut a, &mut e]);
    assert_eq!(e.security().root_generation, 1);
    assert_eq!(
        page_content(&mut e.core, p1)
            .to_string()
            .matches("a wrote line")
            .count(),
        2
    );
    assert!(
        page_content(&mut e.core, p2)
            .to_string()
            .contains("a wrote")
    );
}

#[test]
fn an_idle_device_is_refused_by_the_server_until_it_pairs_again() {
    let server = server(26);
    let (mut a, kit) = Device::first("a", 261, &server);
    a.configure();
    a.cycle().unwrap();
    let mut b = Device::from_backup("b", 262, &server, &mut a, &kit);
    b.configure();
    settle(&mut [&mut a, &mut b]);
    server.set_idle_device_days(Some(3));
    // a opens a session two days later; b stays away for four.
    server.set_now_ms(server.now_ms() + 2 * DAY_MS);
    a.authed = false;
    a.cycle().unwrap();
    server.set_now_ms(server.now_ms() + 2 * DAY_MS);
    b.authed = false;
    let err = b.cycle().unwrap_err();
    assert!(matches!(err, SimError::Sync(SyncError::Idle)), "{err:?}");
    // a was seen two days ago: within the limit.
    a.authed = false;
    a.cycle().unwrap();
}

// ----- typed page properties (ADR-0026) ----------------------------------------------------

/// Creates a property on a device and returns its id.
fn make_property(
    device: &mut Device,
    space_id: SpaceId,
    name: &str,
    kind: kb_types::PropertyKind,
) -> kb_types::PropertyId {
    match device
        .core
        .dispatch(Request::PropertyCreate {
            space_id,
            name: name.into(),
            kind,
            config: None,
        })
        .unwrap()
    {
        Response::Property(def) => def.id,
        other => panic!("unexpected {other:?}"),
    }
}

fn make_option(
    device: &mut Device,
    space_id: SpaceId,
    property_id: kb_types::PropertyId,
    name: &str,
) -> kb_types::OptionId {
    match device
        .core
        .dispatch(Request::PropertyOptionAdd {
            space_id,
            property_id,
            name: name.into(),
            color: kb_types::OptionColor::Green,
            group: None,
        })
        .unwrap()
    {
        Response::PropertyOption(option) => option.id,
        other => panic!("unexpected {other:?}"),
    }
}

fn set_property(
    device: &mut Device,
    page_id: PageId,
    property_id: kb_types::PropertyId,
    value: kb_types::PropertyValue,
) {
    device
        .core
        .dispatch(Request::PagePropertySet {
            page_id,
            property_id,
            value: Some(value),
        })
        .unwrap();
}

fn page_values(
    device: &mut Device,
    page_id: PageId,
) -> Vec<(kb_types::PropertyId, kb_types::PropertyValue)> {
    match device
        .core
        .dispatch(Request::PagePropertiesGet { page_id })
        .unwrap()
    {
        Response::PageProperties(values) => values.values,
        other => panic!("unexpected {other:?}"),
    }
}

fn schema_names(device: &mut Device, space_id: SpaceId) -> Vec<String> {
    match device
        .core
        .dispatch(Request::PropertySchemaGet { space_id })
        .unwrap()
    {
        Response::PropertySchema(schema) => schema.properties.into_iter().map(|p| p.name).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

fn pages_with(
    device: &mut Device,
    space_id: SpaceId,
    property_id: kb_types::PropertyId,
    filter: kb_types::PropertyFilter,
) -> Vec<PageId> {
    match device
        .core
        .dispatch(Request::PagesByProperty {
            space_id,
            property_id,
            filter,
        })
        .unwrap()
    {
        Response::PageIds(ids) => ids,
        other => panic!("unexpected {other:?}"),
    }
}

fn tag_hits(device: &mut Device, query: &str) -> Vec<SearchHit> {
    match device
        .core
        .dispatch(Request::Search {
            query: query.into(),
            space_id: None,
            scope: None,
            limit: Some(50),
            edited_after: None,
        })
        .unwrap()
    {
        Response::Search(hits) => hits,
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_schema_and_values_written_on_one_device_reach_the_other_with_their_rows() {
    use kb_types::properties::TAGS_PROPERTY_ID;
    use kb_types::{PropertyFilter, PropertyKind, PropertyValue};
    let (_server, mut a, mut b, page) = pair(41);
    let space = a.space_ids()[0];

    // Device a writes a schema (a property of its own, and Tags with an option) and two values.
    let notes = make_property(&mut a, space, "Notes", PropertyKind::Text);
    let tag = make_option(&mut a, space, TAGS_PROPERTY_ID, "from the laptop");
    set_property(
        &mut a,
        page,
        notes,
        PropertyValue::Text("written on a".into()),
    );
    set_property(
        &mut a,
        page,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![tag]),
    );
    b.core.drain_events();
    settle(&mut [&mut a, &mut b]);

    // Device b has the schema, the values, the derived rows and the search text.
    assert_eq!(schema_names(&mut b, space), ["Tags", "Notes"]);
    assert_eq!(
        page_values(&mut b, page),
        [
            (TAGS_PROPERTY_ID, PropertyValue::MultiSelect(vec![tag])),
            (notes, PropertyValue::Text("written on a".into()))
        ]
    );
    assert_eq!(
        pages_with(&mut b, space, notes, PropertyFilter::HasValue),
        [page]
    );
    assert_eq!(
        pages_with(
            &mut b,
            space,
            TAGS_PROPERTY_ID,
            PropertyFilter::ContainsOption(tag)
        ),
        [page]
    );
    let hits = tag_hits(&mut b, "laptop");
    assert_eq!(
        hits.len(),
        1,
        "a tag written elsewhere is found here: {hits:?}"
    );
    assert_eq!(hits[0].page_id, page);
    assert!(hits[0].block_id.is_none());

    // The change reaches b as a schema change and a structure change.
    let cost = make_property(&mut a, space, "Cost", PropertyKind::Number);
    a.cycle().unwrap();
    b.pull_once();
    let events = b.core.drain_events();
    let schema_at = events
        .iter()
        .position(|e| matches!(e, Event::PropertySchemaChanged { space_id } if *space_id == space))
        .unwrap_or_else(|| panic!("no PropertySchemaChanged in {events:?}"));
    let structure_at = events
        .iter()
        .position(|e| matches!(e, Event::StructureChanged { space_id } if *space_id == space))
        .unwrap();
    assert!(schema_at < structure_at, "{events:?}");

    // A value change alone is no schema change on the other side.
    set_property(&mut a, page, cost, PropertyValue::Number(2.5));
    a.cycle().unwrap();
    b.pull_once();
    let events = b.core.drain_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::PropertySchemaChanged { .. })),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::StructureChanged { .. }))
    );
    settle(&mut [&mut a, &mut b]);
    assert_eq!(
        pages_with(
            &mut b,
            space,
            cost,
            PropertyFilter::Equals(PropertyValue::Number(2.5))
        ),
        [page]
    );
}

#[test]
fn concurrent_property_edits_on_two_devices_merge_and_both_derive_the_same_rows() {
    use kb_types::properties::TAGS_PROPERTY_ID;
    use kb_types::{PropertyFilter, PropertyKind, PropertyValue};
    let (_server, mut a, mut b, page) = pair(42);
    let space = a.space_ids()[0];
    let notes = make_property(&mut a, space, "Notes", PropertyKind::Text);
    let cost = make_property(&mut a, space, "Cost", PropertyKind::Number);
    settle(&mut [&mut a, &mut b]);

    // Offline from each other: different properties of one page, and the first tags of the
    // space (Tags is not written yet on either device), and a property deleted on one side.
    set_property(&mut a, page, notes, PropertyValue::Text("from a".into()));
    set_property(&mut b, page, cost, PropertyValue::Number(7.0));
    let tag_a = make_option(&mut a, space, TAGS_PROPERTY_ID, "alpha");
    let tag_b = make_option(&mut b, space, TAGS_PROPERTY_ID, "beta");
    let extra = make_property(&mut b, space, "Extra", PropertyKind::Checkbox);
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        assert_eq!(
            page_values(device, page),
            [
                (notes, PropertyValue::Text("from a".into())),
                (cost, PropertyValue::Number(7.0))
            ],
            "{}: both values survive",
            device.name
        );
        let tags = match device
            .core
            .dispatch(Request::PropertySchemaGet { space_id: space })
            .unwrap()
        {
            Response::PropertySchema(schema) => schema.properties[0].options.clone(),
            other => panic!("unexpected {other:?}"),
        };
        let mut found: Vec<_> = tags.iter().map(|o| o.id).collect();
        found.sort();
        let mut expected = vec![tag_a, tag_b];
        expected.sort();
        assert_eq!(found, expected, "{}: both tags survive", device.name);
        assert_eq!(schema_names(device, space).len(), 4, "{}", device.name);
        assert_eq!(
            pages_with(device, space, notes, PropertyFilter::HasValue),
            [page],
            "{}: derived rows",
            device.name
        );
        assert_eq!(
            pages_with(device, space, cost, PropertyFilter::HasValue),
            [page]
        );
        assert_eq!(
            tag_hits(device, "alpha").len() + tag_hits(device, "beta").len(),
            0
        );
    }
    let _ = extra;

    // Tag the page on one device and the derived text follows on the other.
    set_property(
        &mut b,
        page,
        TAGS_PROPERTY_ID,
        PropertyValue::MultiSelect(vec![tag_a, tag_b]),
    );
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(tag_hits(device, "alpha").len(), 1, "{}", device.name);
        assert_eq!(
            pages_with(
                device,
                space,
                TAGS_PROPERTY_ID,
                PropertyFilter::ContainsOption(tag_b)
            ),
            [page]
        );
    }

    // Deleting a property and an option on one device clears them on the other: the rows and
    // the search text go, the page keeps reading.
    a.core
        .dispatch(Request::PropertyDelete {
            space_id: space,
            property_id: notes,
        })
        .unwrap();
    a.core
        .dispatch(Request::PropertyOptionDelete {
            space_id: space,
            property_id: TAGS_PROPERTY_ID,
            option_id: tag_a,
        })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(
            page_values(device, page),
            [
                (TAGS_PROPERTY_ID, PropertyValue::MultiSelect(vec![tag_b])),
                (cost, PropertyValue::Number(7.0))
            ],
            "{}",
            device.name
        );
        let err = device
            .core
            .dispatch(Request::PagesByProperty {
                space_id: space,
                property_id: notes,
                filter: PropertyFilter::HasValue,
            })
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::NotFound,
            "{}: the property is gone",
            device.name
        );
        assert!(tag_hits(device, "alpha").is_empty(), "{}", device.name);
        assert_eq!(tag_hits(device, "beta").len(), 1, "{}", device.name);
    }
}

#[test]
fn files_attached_on_one_device_are_references_on_the_other_until_a_value_drops_them() {
    use kb_types::{PropertyFilter, PropertyKind, PropertyValue};
    let (_server, mut a, mut b, page) = pair(43);
    let space = a.space_ids()[0];
    let attached = make_property(&mut a, space, "Attached", PropertyKind::Files);
    let todo = make_property(&mut a, space, "To do", PropertyKind::Checklist);
    let info = a.upload_attachment(space, "plan.pdf", b"the plan");
    let id = info.attachment_id;
    assert_eq!(a.attachment(id).references, 0);
    assert!(a.attachment(id).unreferenced_since.is_some());

    // The files value and a checklist travel with the structure document.
    set_property(&mut a, page, attached, PropertyValue::Files(vec![id]));
    set_property(
        &mut a,
        page,
        todo,
        PropertyValue::Checklist(vec![kb_types::ChecklistItem {
            id: kb_types::ChecklistItemId::from_bytes([3; 16]),
            text: "Read the plan".into(),
            done: false,
        }]),
    );
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(
            (seen.references, seen.unreferenced_since),
            (1, None),
            "{}: the value references the file",
            device.name
        );
        assert_eq!(
            pages_with(device, space, attached, PropertyFilter::HasValue),
            [page],
            "{}",
            device.name
        );
        assert_eq!(
            pages_with(
                device,
                space,
                todo,
                PropertyFilter::ChecklistComplete(false)
            ),
            [page],
            "{}",
            device.name
        );
        assert_eq!(
            tag_hits(device, "pdf").len(),
            1,
            "{}: the file name",
            device.name
        );
        assert_eq!(
            tag_hits(device, "read").len(),
            1,
            "{}: the item",
            device.name
        );
    }

    // The other device removes the value: both devices release the file.
    b.core
        .dispatch(Request::PagePropertySet {
            page_id: page,
            property_id: attached,
            value: None,
        })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(seen.references, 0, "{}", device.name);
        assert!(seen.unreferenced_since.is_some(), "{}", device.name);
        assert!(
            pages_with(device, space, attached, PropertyFilter::HasValue).is_empty(),
            "{}",
            device.name
        );
    }
}

#[test]
fn an_image_icon_set_on_one_device_is_a_reference_on_the_other_until_the_icon_changes() {
    use kb_storage::IconRefQueries;
    let (_server, mut a, mut b, page) = pair(44);
    let space = a.space_ids()[0];
    let upload_image = |device: &mut Device, name: &str| {
        let id = match device
            .core
            .dispatch(Request::AttachmentBegin {
                space_id: space,
                name: name.into(),
                mime: "image/png".into(),
            })
            .unwrap()
        {
            Response::AttachmentStarted(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        device.core.attachment_write(id, name.as_bytes()).unwrap();
        device
            .core
            .dispatch(Request::AttachmentFinish { attachment_id: id })
            .unwrap();
        device.core.drain_events();
        id
    };
    let set_icon = |device: &mut Device, icon: Option<String>| {
        device
            .core
            .dispatch(Request::PageSetIcon {
                page_id: page,
                icon,
            })
            .unwrap();
    };
    let id = upload_image(&mut a, "icon.png");
    assert_eq!(a.attachment(id).references, 0);
    assert!(a.attachment(id).unreferenced_since.is_some());

    // The icon travels with the structure document: both devices count it as a reference.
    set_icon(&mut a, Some(format!("attachment:{id}")));
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(
            (seen.references, seen.unreferenced_since),
            (1, None),
            "{}: the icon references the image",
            device.name
        );
        assert_eq!(
            device.core.db.icon_refs_of_space(space).unwrap(),
            [(page, id)],
            "{}",
            device.name
        );
    }

    // The other device replaces it with an icon of the set: both release the image.
    set_icon(&mut b, Some("icon:book-open:blue".into()));
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(seen.references, 0, "{}", device.name);
        assert!(seen.unreferenced_since.is_some(), "{}", device.name);
        assert!(
            device.core.db.icon_refs_of_space(space).unwrap().is_empty(),
            "{}",
            device.name
        );
    }

    // Setting it again is a reference again, and a purge of the page on one device releases it on both.
    set_icon(&mut a, Some(format!("attachment:{id}")));
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(device.attachment(id).references, 1, "{}", device.name);
    }
    b.core
        .dispatch(Request::PageTrash { page_id: page })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(
            device.attachment(id).references,
            1,
            "{}: a trashed page keeps its icon",
            device.name
        );
    }
    b.core
        .dispatch(Request::PagePurge { page_id: page })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(seen.references, 0, "{}", device.name);
        assert!(seen.unreferenced_since.is_some(), "{}", device.name);
        assert!(
            device.core.db.icon_refs_of_space(space).unwrap().is_empty(),
            "{}",
            device.name
        );
    }
}

#[test]
fn an_image_cover_set_on_one_device_is_a_reference_on_the_other_until_the_cover_changes() {
    use kb_storage::IconRefQueries;
    let (_server, mut a, mut b, page) = pair(45);
    let space = a.space_ids()[0];
    let upload_image = |device: &mut Device, name: &str| {
        let id = match device
            .core
            .dispatch(Request::AttachmentBegin {
                space_id: space,
                name: name.into(),
                mime: "image/png".into(),
            })
            .unwrap()
        {
            Response::AttachmentStarted(id) => id,
            other => panic!("unexpected {other:?}"),
        };
        device.core.attachment_write(id, name.as_bytes()).unwrap();
        device
            .core
            .dispatch(Request::AttachmentFinish { attachment_id: id })
            .unwrap();
        device.core.drain_events();
        id
    };
    let set_cover = |device: &mut Device, cover: Option<String>, cover_y: Option<u8>| {
        device
            .core
            .dispatch(Request::PageSetCover {
                page_id: page,
                cover,
                cover_y,
            })
            .unwrap();
    };
    let id = upload_image(&mut a, "banner.png");
    assert_eq!(a.attachment(id).references, 0);

    // The cover travels with the structure document, with its position: both devices count it as a
    // reference.
    set_cover(&mut a, Some(format!("attachment:{id}")), Some(30));
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(
            (seen.references, seen.unreferenced_since),
            (1, None),
            "{}: the cover references the image",
            device.name
        );
        assert_eq!(
            device.core.db.icon_refs_of_space(space).unwrap(),
            [(page, id)],
            "{}",
            device.name
        );
    }
    let info = match b.core.dispatch(Request::PageGet { page_id: page }).unwrap() {
        Response::Page(info) => info,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        (info.cover.as_deref(), info.cover_y),
        (Some(format!("attachment:{id}").as_str()), Some(30))
    );

    // The other device replaces it with a gradient: both release the image.
    set_cover(&mut b, Some("gradient:sunset".into()), None);
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(seen.references, 0, "{}", device.name);
        assert!(seen.unreferenced_since.is_some(), "{}", device.name);
        assert!(
            device.core.db.icon_refs_of_space(space).unwrap().is_empty(),
            "{}",
            device.name
        );
    }

    // Setting it again is a reference again, and a purge of the page on one device releases it on both.
    set_cover(&mut a, Some(format!("attachment:{id}")), None);
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(device.attachment(id).references, 1, "{}", device.name);
    }
    b.core
        .dispatch(Request::PageTrash { page_id: page })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(
            device.attachment(id).references,
            1,
            "{}: a trashed page keeps its cover",
            device.name
        );
    }
    b.core
        .dispatch(Request::PagePurge { page_id: page })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let seen = device.attachment(id);
        assert_eq!(seen.references, 0, "{}", device.name);
        assert!(seen.unreferenced_since.is_some(), "{}", device.name);
        assert!(
            device.core.db.icon_refs_of_space(space).unwrap().is_empty(),
            "{}",
            device.name
        );
    }
}

#[test]
fn the_font_small_text_and_lock_of_a_page_reach_the_other_device_and_flip_live() {
    use kb_types::PageFont;
    let (_server, mut a, mut b, page) = pair(46);
    let space = a.space_ids()[0];
    let set = |device: &mut Device,
               font: Option<PageFont>,
               small_text: Option<bool>,
               locked: Option<bool>| {
        device
            .core
            .dispatch(Request::PageSetAppearance {
                page_id: page,
                font,
                small_text,
                locked,
            })
            .unwrap();
        device.core.drain_events();
    };
    let look = |device: &mut Device| match device
        .core
        .dispatch(Request::PageGet { page_id: page })
        .unwrap()
    {
        Response::Page(info) => (info.font, info.small_text, info.locked),
        other => panic!("unexpected {other:?}"),
    };

    // One device locks and sets the font: the other reads both, and is told by a structure change
    // (which is what flips an open page read-only).
    set(&mut a, Some(PageFont::Serif), None, Some(true));
    a.cycle().unwrap();
    b.pull_once();
    assert_eq!(look(&mut b), (Some(PageFont::Serif), false, true));
    assert!(
        b.core
            .drain_events()
            .contains(&Event::StructureChanged { space_id: space }),
        "the lock reaches b as a structure change"
    );
    settle(&mut [&mut a, &mut b]);

    // Apart: b unlocks while a turns on small text and changes the font. Each part survives the
    // merge, and the unlock reaches a.
    set(&mut b, None, None, Some(false));
    set(&mut a, Some(PageFont::Mono), Some(true), None);
    settle(&mut [&mut a, &mut b]);
    assert_eq!(look(&mut a), (Some(PageFont::Mono), true, false));
    assert_eq!(look(&mut b), (Some(PageFont::Mono), true, false));

    // Cleared on one device, cleared on the other.
    set(&mut a, Some(PageFont::Default), Some(false), None);
    settle(&mut [&mut a, &mut b]);
    assert_eq!(look(&mut b), (None, false, false));
}

// ----- two-way relations and rollups (ADR-0027) ---------------------------------------------

#[test]
fn two_way_relations_written_on_both_sides_while_apart_converge_with_the_same_symmetric_view() {
    use kb_types::{PropertyConfig, PropertyFilter, PropertyKind, PropertyValue};
    let (_server, mut a, mut b, p1) = pair(43);
    let space = a.space_ids()[0];
    let p2 = a.new_page(space, None);
    let p3 = a.new_page(space, None);
    let forward = match a
        .core
        .dispatch(Request::PropertyCreate {
            space_id: space,
            name: "Related".into(),
            kind: PropertyKind::Relation,
            config: Some(PropertyConfig::Relation {
                two_way: true,
                paired_property_id: None,
            }),
        })
        .unwrap()
    {
        Response::Property(def) => def,
        other => panic!("unexpected {other:?}"),
    };
    let Some(PropertyConfig::Relation {
        paired_property_id: Some(back),
        ..
    }) = forward.config.clone()
    else {
        panic!("not paired: {forward:?}");
    };
    settle(&mut [&mut a, &mut b]);
    assert_eq!(
        schema_names(&mut b, space),
        ["Tags", "Related", "Related (back)"]
    );

    // Apart: a links page 1 to page 2 from page 1's side, b links page 3 to page 1 from page 3's
    // own forward side. Both write the forward list of page 1, so one of the two lists is lost
    // by last-writer-wins, but the paired side of the other page is not.
    set_property(&mut a, p1, forward.id, PropertyValue::Relation(vec![p2]));
    set_property(&mut b, p3, back, PropertyValue::Relation(vec![p1]));
    settle(&mut [&mut a, &mut b]);

    for device in [&mut a, &mut b] {
        let values = page_values(device, p1);
        let Some(PropertyValue::Relation(listed)) = values
            .iter()
            .find(|(id, _)| *id == forward.id)
            .map(|(_, value)| value.clone())
        else {
            panic!("{}: page 1 lists nothing: {values:?}", device.name);
        };
        let mut listed = listed;
        listed.sort();
        let mut expected = vec![p2, p3];
        expected.sort();
        assert_eq!(listed, expected, "{}: both links survive", device.name);
        for other in [p2, p3] {
            let values = page_values(device, other);
            assert!(
                values.contains(&(back, PropertyValue::Relation(vec![p1]))),
                "{}: {other:?} lists page 1 on its back side: {values:?}",
                device.name
            );
        }
        assert_eq!(
            pages_with(device, space, forward.id, PropertyFilter::RelatesTo(p3)),
            [p1],
            "{}",
            device.name
        );
        assert_eq!(
            pages_with(device, space, back, PropertyFilter::RelatesTo(p1)).len(),
            2,
            "{}",
            device.name
        );
    }
}

#[test]
fn favourites_merge_per_page_between_devices_and_follow_a_purge() {
    let (_server, mut a, mut b, p1) = pair(45);
    let space = a.space_ids()[0];
    let p2 = a.new_page(space, None);
    let p3 = a.new_page(space, None);
    settle(&mut [&mut a, &mut b]);
    let favorite = |device: &mut Device, page_id: PageId, favorite: bool| {
        device
            .core
            .dispatch(Request::PageFavoriteSet { page_id, favorite })
            .unwrap();
        device.core.drain_events();
    };
    let listed = |device: &mut Device| tree(&mut device.core, space).favorites;

    // Apart: a favourites two pages, b a third one. Every favourite survives the merge, in the
    // same order on both devices, and a's two keep the order a gave them.
    favorite(&mut a, p1, true);
    favorite(&mut a, p2, true);
    favorite(&mut b, p3, true);
    // The cycles drain the events: b pulls a's change by hand once to see how it is announced.
    a.cycle().unwrap();
    b.pull_once();
    assert!(
        b.core
            .drain_events()
            .contains(&Event::StructureChanged { space_id: space }),
        "the change reaches b as a structure change"
    );
    settle(&mut [&mut a, &mut b]);
    let merged = listed(&mut a);
    assert_eq!(listed(&mut b), merged, "the same list on both devices");
    assert_eq!(merged.len(), 3);
    let at = |page: PageId| merged.iter().position(|p| *p == page).unwrap();
    assert!(at(p1) < at(p2), "a's order is kept: {merged:?}");

    // Apart again: b moves the last favourite to the front, a removes the middle one.
    let [first, middle, last] = [merged[0], merged[1], merged[2]];
    b.core
        .dispatch(Request::PageFavoriteMove {
            page_id: last,
            before: Some(first),
        })
        .unwrap();
    favorite(&mut a, middle, false);
    settle(&mut [&mut a, &mut b]);
    assert_eq!(listed(&mut a), [last, first]);
    assert_eq!(listed(&mut b), [last, first]);

    // A trashed page is hidden on both devices and returns with its restore; a purge removes
    // the entry from both documents.
    a.trash(last);
    settle(&mut [&mut a, &mut b]);
    assert_eq!(listed(&mut b), [first], "hidden while in the trash");
    a.core
        .dispatch(Request::PageRestore { page_id: last })
        .unwrap();
    settle(&mut [&mut a, &mut b]);
    assert_eq!(listed(&mut b).len(), 2, "back with the restore");
    a.trash(first);
    a.core
        .dispatch(Request::PagePurge { page_id: first })
        .unwrap();
    a.core.drain_events();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        assert_eq!(listed(device), [last], "{}", device.name);
        let portable = device.core.structure(space).unwrap().model.to_portable();
        let entries = portable["favorites"].as_object().unwrap();
        assert_eq!(
            entries.len(),
            1,
            "{}: the purged page's entry is gone",
            device.name
        );
    }
}

#[test]
fn trash_expiry_reaches_the_other_device_and_the_shorter_limit_purges_for_all() {
    use kb_types::testing::FixedClock;
    let (_server, mut a, mut b, p1) = pair(46);
    let space = a.space_ids()[0];
    let old = a.new_page(space, None);
    let kid = a.new_page(space, Some(old));
    let recent = a.new_page(space, None);
    settle(&mut [&mut a, &mut b]);
    let t0 = a.core.clock.now_ms();

    // Both devices keep the default 30 days, and `b` is far behind in time: only `a` runs the expiry.
    a.trash(old);
    a.core.clock = Box::new(FixedClock::new(t0 + 20 * DAY_MS));
    a.trash(recent);
    settle(&mut [&mut a, &mut b]);
    a.core.clock = Box::new(FixedClock::new(t0 + 31 * DAY_MS));
    assert_eq!(
        a.core.trash_expire().unwrap(),
        1,
        "the one trashed 31 days ago"
    );
    a.core.drain_events();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let model = &device.core.docs.structures[&space].model;
        assert!(model.page(old).is_none(), "{}", device.name);
        assert!(
            model.page(kid).is_none(),
            "{}: the sub-page goes too",
            device.name
        );
        assert!(
            model.tombstones().iter().any(|(id, _)| *id == old),
            "{}: the purge is a tombstone",
            device.name
        );
        assert!(model.page(recent).is_some_and(|p| p.trashed_at.is_some()));
        assert!(model.page(p1).is_some_and(|p| p.trashed_at.is_none()));
    }

    // A device with a shorter limit purges for every device: `b` keeps nothing for more than a day.
    b.core
        .dispatch(Request::SettingsSet {
            key: crate::TRASH_RETENTION_SETTING.into(),
            value: serde_json::json!(1),
        })
        .unwrap();
    b.core.clock = Box::new(FixedClock::new(t0 + 22 * DAY_MS));
    assert_eq!(b.core.trash_expire().unwrap(), 1);
    b.core.drain_events();
    settle(&mut [&mut a, &mut b]);
    for device in [&mut a, &mut b] {
        let model = &device.core.docs.structures[&space].model;
        assert!(model.page(recent).is_none(), "{}", device.name);
        assert!(model.tombstones().iter().any(|(id, _)| *id == recent));
    }
}
