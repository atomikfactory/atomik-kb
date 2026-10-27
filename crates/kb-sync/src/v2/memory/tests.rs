//! `MemoryRemote` v2: the conformance suite, then what the suite cannot see (connections, the
//! knobs, the adversarial modes, the limits that need a clock or a small quota).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use kb_types::sharing::SpaceSettings;
use kb_types::testing::SeededRng;
use kb_types::{FeedId, MemberRole};

use super::*;
use crate::v2::conformance::{self, FIXTURE_TIME_MS, Fixture, Person, Space};

/// Polls a future that never waits (every `MemoryRemote` future is ready at once).
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

/// A remote whose clock agrees with the fixture's, and a fixture that can move it.
fn remote_and_fixture(seed: u64) -> (MemoryRemote, Fixture) {
    let remote = MemoryRemote::new(&SeededRng::new(seed));
    remote.set_now_ms(FIXTURE_TIME_MS);
    let for_clock = remote.clone();
    let for_prune = remote.clone();
    let fixture = Fixture::generate(seed).with_control(
        move |ms| for_clock.advance_ms(ms),
        move || {
            for_prune.prune(for_prune.now_ms());
        },
    );
    (remote, fixture)
}

#[test]
fn the_reference_remote_passes_the_conformance_suite() {
    let (remote, fixture) = remote_and_fixture(1);
    block_on(conformance::run_all_v2(&remote, &fixture));
}

#[test]
fn the_suite_runs_without_a_way_to_move_the_clock_and_skips_what_needs_it() {
    let remote = MemoryRemote::new(&SeededRng::new(2));
    remote.set_now_ms(FIXTURE_TIME_MS);
    block_on(conformance::run_all_v2(&remote, &Fixture::generate(2)));
}

#[test]
fn a_small_quota_remote_passes_the_quota_check() {
    let (remote, fixture) = remote_and_fixture(3);
    remote.set_account_quota(Some(2 * 1024 * 1024));
    block_on(conformance::check_quota_passes_to_owner(&remote, &fixture));
    // Without a small quota the check skips itself (the default remote has none).
    let (unlimited, other) = remote_and_fixture(4);
    block_on(conformance::check_quota_passes_to_owner(&unlimited, &other));
}

#[test]
fn a_device_idle_beyond_the_limit_is_refused_until_it_signs_in_again() {
    let (remote, fixture) = remote_and_fixture(10);
    remote.set_idle_device_days(Some(30));
    block_on(conformance::check_idle_device(&remote, &fixture));
}

#[test]
fn the_feed_limit_is_enforced() {
    let (remote, fixture) = remote_and_fixture(5);
    block_on(conformance::check_feed_limit(&remote, &fixture));
}

#[test]
fn the_total_signup_limit_is_enforced() {
    let (remote, fixture) = remote_and_fixture(6);
    remote.set_share_signups(2, 5);
    block_on(conformance::check_share_signup_total_limit(
        &remote, &fixture,
    ));
}

#[test]
fn the_per_inviter_signup_limit_is_enforced() {
    let (remote, fixture) = remote_and_fixture(7);
    remote.set_share_signups(20, 2);
    block_on(conformance::check_share_signup(&remote, &fixture));
}

#[test]
fn share_signups_can_be_switched_off() {
    let (remote, fixture) = remote_and_fixture(8);
    remote.set_share_signups(0, 0);
    assert_eq!(remote.server_info_sync().unwrap().share_signups, 0);
    block_on(conformance::check_share_signup(&remote, &fixture));
}

#[test]
fn server_info_reports_the_settings() {
    let remote = MemoryRemote::new(&SeededRng::new(9));
    remote.set_account_quota(Some(777));
    remote.set_share_signups(3, 2);
    remote.set_idle_device_days(Some(12));
    let info = remote.server_info_sync().unwrap();
    assert_eq!(info.protocol_version, 2);
    assert_eq!(info.server_epoch, remote.server_epoch());
    assert_eq!(info.account_quota_bytes, Some(777));
    assert_eq!((info.share_signups, info.share_signups_per_account), (3, 2));
    assert_eq!(info.idle_device_days, Some(12));
    assert_eq!(info.max_wire_bytes, kb_types::sync::MAX_WIRE_BYTES);
}

/// A trio: Alice (Owner, creator), Bob (Editor), Carol (Viewer) in one space.
async fn trio(remote: &MemoryRemote, fixture: &Fixture) -> ([Person; 3], Space) {
    let [alice, bob, carol] = conformance::enroll_n::<MemoryRemote, 3>(remote, fixture).await;
    let mut space = fixture.space("Trio");
    conformance::found(remote, &mut space, &alice).await;
    conformance::invite_and_join(
        remote,
        fixture,
        &mut space,
        &alice,
        &bob,
        MemberRole::Editor,
    )
    .await;
    conformance::invite_and_join(
        remote,
        fixture,
        &mut space,
        &alice,
        &carol,
        MemberRole::Viewer,
    )
    .await;
    ([alice, bob, carol], space)
}

fn membership_seqs(remote: &MemoryRemote, who: &Person, feed: FeedId) -> Vec<u64> {
    login_sync(remote, who);
    remote
        .membership_get_sync(feed, None)
        .unwrap()
        .entries
        .iter()
        .map(|item| MembershipEntry::decode(&item.bytes).unwrap().header.seq)
        .collect()
}

fn login_sync(remote: &MemoryRemote, who: &Person) {
    let dev = who.active_device();
    remote
        .authenticate_sync(who.account, who.device_id(dev), &who.signer(dev))
        .unwrap();
}

/// The bytes of the feed's items as `who` reads them.
fn read_bytes(remote: &MemoryRemote, who: &Person, feed: FeedId) -> Vec<Vec<u8>> {
    login_sync(remote, who);
    remote
        .changes_sync(feed, 0, 200)
        .unwrap()
        .changes
        .into_iter()
        .map(|(_, bytes)| bytes)
        .collect()
}

#[test]
fn cloning_opens_a_new_connection_that_shares_the_server() {
    let (remote, fixture) = remote_and_fixture(10);
    let ([alice, ..], space) = block_on(trio(&remote, &fixture));
    let other = remote.clone();
    // The clone sees the same server but has no session.
    assert_eq!(other.server_epoch(), remote.server_epoch());
    assert_eq!(other.feeds_list_sync(), Err(SyncError::Unauthorized));
    login_sync(&other, &alice);
    assert!(
        other
            .feeds_list_sync()
            .unwrap()
            .iter()
            .any(|listing| listing.feed == space.feed)
    );
    // The original's session is untouched by the clone's login.
    login_sync(&remote, &alice);
    assert!(remote.feeds_list_sync().is_ok());
}

#[test]
fn invite_open_is_limited_per_connection() {
    let remote = MemoryRemote::new(&SeededRng::new(11));
    remote.set_now_ms(FIXTURE_TIME_MS);
    let secret = kb_keys::invite::InviteSecret::generate(&SeededRng::new(5));
    let id = kb_keys::invite::invite_id(&secret);
    let sig = Space::open_sig(&secret, remote.server_epoch());
    let other = remote.clone();
    let limit = usize::try_from(kb_types::sharing::INVITE_OPEN_PER_IP_PER_HOUR).unwrap();
    for _ in 0..limit {
        assert_eq!(remote.invite_open_sync(id, &sig), Err(SyncError::NotFound));
    }
    assert!(matches!(
        remote.invite_open_sync(id, &sig),
        Err(SyncError::Unavailable(_))
    ));
    // Another connection has its own budget; the window slides with the clock.
    assert_eq!(other.invite_open_sync(id, &sig), Err(SyncError::NotFound));
    remote.advance_ms(HOUR_MS + 1);
    assert_eq!(remote.invite_open_sync(id, &sig), Err(SyncError::NotFound));
}

#[test]
fn a_lost_compare_and_swap_can_be_injected() {
    let (remote, fixture) = remote_and_fixture(12);
    let ([alice, ..], mut space) = block_on(trio(&remote, &fixture));
    remote.inject_lost_compare_and_swap(space.feed, 2);
    let profile = space.profile(&alice, "Alice");
    for _ in 0..2 {
        assert!(matches!(
            block_on(conformance::post(&remote, &mut space, &alice, &profile)),
            Err(SyncError::Conflict(_))
        ));
    }
    // Nothing was appended meanwhile, so the same entry is accepted the third time.
    conformance::ok(
        block_on(conformance::post(&remote, &mut space, &alice, &profile)),
        "the third attempt",
    );
}

#[test]
fn reset_data_starts_a_new_epoch_and_forgets_everything() {
    let (remote, fixture) = remote_and_fixture(13);
    let ([alice, ..], space) = block_on(trio(&remote, &fixture));
    let epoch = remote.server_epoch();
    assert!(!remote.inspect_feed(space.feed).is_empty());
    remote.reset_data();
    assert_ne!(remote.server_epoch(), epoch);
    assert!(remote.inspect_feed(space.feed).is_empty());
    assert!(remote.feed_members(space.feed).is_empty());
    assert_eq!(remote.feeds_list_sync(), Err(SyncError::Unauthorized));
    assert_eq!(
        remote.authenticate_sync(alice.account, alice.device_id(0), &alice.signer(0)),
        Err(SyncError::NotFound)
    );
    // The configuration and the clock stay.
    assert_eq!(remote.now_ms(), FIXTURE_TIME_MS);
}

#[test]
fn offline_answers_unavailable() {
    let (remote, fixture) = remote_and_fixture(14);
    let ([alice, ..], space) = block_on(trio(&remote, &fixture));
    login_sync(&remote, &alice);
    remote.set_offline(true);
    assert!(matches!(
        remote.changes_sync(space.feed, 0, 10),
        Err(SyncError::Unavailable(_))
    ));
    assert!(matches!(
        remote.server_info_sync(),
        Err(SyncError::Unavailable(_))
    ));
    remote.set_offline(false);
    assert!(remote.changes_sync(space.feed, 0, 10).is_ok());
}

#[test]
fn the_billing_account_follows_the_earliest_owner() {
    let (remote, fixture) = remote_and_fixture(15);
    let ([alice, bob, _], mut space) = block_on(trio(&remote, &fixture));
    assert_eq!(remote.billing_account(space.feed), Some(alice.account));
    let promote = space.role_set(&alice, bob.account, MemberRole::Owner);
    block_on(conformance::post_ok(
        &remote, &mut space, &alice, &promote, "promote",
    ));
    assert_eq!(remote.billing_account(space.feed), Some(alice.account));
    let leave = space.leave(&alice);
    block_on(conformance::post_ok(
        &remote, &mut space, &alice, &leave, "leave",
    ));
    assert_eq!(remote.billing_account(space.feed), Some(bob.account));
    assert!(remote.pending_rekey(space.feed));
    assert_eq!(
        remote.feed_members(space.feed).len(),
        2,
        "Bob and Carol remain"
    );
}

#[test]
fn pruning_frees_the_bytes_and_leaves_membership_alone() {
    let (remote, fixture) = remote_and_fixture(16);
    let ([alice, bob, carol], space) = block_on(trio(&remote, &fixture));
    let feed = space.feed;
    let doc = fixture.doc();
    login_sync(&remote, &alice);
    for seq in 1..=3 {
        remote
            .append_sync(
                feed,
                &space.doc_entry(&alice, 0, doc, seq, space.epoch, &[7u8; 400]),
            )
            .unwrap();
    }
    let tip = remote.last_cursor(feed);
    let snapshot = space.doc_snapshot(&alice, 0, doc, tip, space.epoch, b"state");
    remote.put_snapshot_sync(feed, &snapshot).unwrap();
    let chain = remote.changes_sync(feed, tip, 1).unwrap().chain_hash_after;
    let used = remote.feed_bytes_used(feed);
    for who in [&alice, &bob, &carol] {
        login_sync(&remote, who);
        remote
            .put_head_sync(
                feed,
                &space.head_of(who, 0, tip, chain, remote.server_epoch()),
            )
            .unwrap();
    }
    // Too early: the snapshot is less than seven days old.
    assert_eq!(remote.prune(remote.now_ms() + DAY_MS), 0);
    let removed = remote.prune(remote.now_ms() + 8 * DAY_MS);
    assert_eq!(removed, 3);
    assert!(remote.feed_bytes_used(feed) < used - 3 * 400);
    login_sync(&remote, &carol);
    let page = remote.changes_sync(feed, 0, 200).unwrap();
    assert!(
        page.changes
            .iter()
            .all(|(_, bytes)| MembershipEntry::decode(bytes).is_ok()),
        "only membership entries are left"
    );
    // The chain hash at the old tip is unchanged: pruned entries keep their place in the chain.
    assert_eq!(
        remote.changes_sync(feed, tip, 1).unwrap().chain_hash_after,
        chain
    );
}

#[test]
fn a_device_marked_idle_does_not_hold_pruning_back() {
    let (remote, fixture) = remote_and_fixture(17);
    let ([alice, bob, carol], space) = block_on(trio(&remote, &fixture));
    let feed = space.feed;
    let doc = fixture.doc();
    login_sync(&remote, &alice);
    remote
        .append_sync(
            feed,
            &space.doc_entry(&alice, 0, doc, 1, space.epoch, b"one"),
        )
        .unwrap();
    let tip = remote.last_cursor(feed);
    remote
        .put_snapshot_sync(
            feed,
            &space.doc_snapshot(&alice, 0, doc, tip, space.epoch, b"s"),
        )
        .unwrap();
    let chain = remote.changes_sync(feed, tip, 1).unwrap().chain_hash_after;
    for who in [&alice, &bob] {
        login_sync(&remote, who);
        remote
            .put_head_sync(
                feed,
                &space.head_of(who, 0, tip, chain, remote.server_epoch()),
            )
            .unwrap();
    }
    // Carol has not acknowledged and her device is not idle: nothing is pruned.
    assert_eq!(remote.prune(remote.now_ms() + 8 * DAY_MS), 0);
    remote.mark_device_idle(carol.account, carol.device_id(0));
    assert_eq!(remote.prune(remote.now_ms() + 8 * DAY_MS), 1);
}

#[test]
fn the_personal_feed_belongs_to_its_account() {
    let (remote, fixture) = remote_and_fixture(18);
    let [alice, bob] = block_on(conformance::enroll_n::<MemoryRemote, 2>(&remote, &fixture));
    let personal = FeedId::from_account(alice.account);
    let rng = SeededRng::new(77);
    login_sync(&remote, &alice);
    let doc = fixture.doc();
    let entry = conformance::sealed_entry(personal, &alice, 0, doc, 1, 0, b"mine", &rng);
    let (cursor, _) = remote.append_sync(personal, &entry).unwrap();
    assert_eq!(
        remote.changes_sync(personal, 0, 10).unwrap().changes[0].0,
        cursor
    );
    // A personal feed refuses membership entries and entries of another feed.
    let mut space = fixture.space("Not personal");
    let create = space.create(&alice, 1);
    assert!(matches!(
        remote.membership_append_sync(personal, &[create.encode()]),
        Err(SyncError::NotFound)
    ));
    assert!(matches!(
        remote.append_sync(personal, &create.encode()),
        Err(SyncError::Protocol(_))
    ));
    let elsewhere = conformance::sealed_entry(space.feed, &alice, 0, doc, 2, 0, b"x", &rng);
    assert!(matches!(
        remote.append_sync(personal, &elsewhere),
        Err(SyncError::Protocol(_))
    ));
    // A personal feed has no epoch rules.
    let late = conformance::sealed_entry(personal, &alice, 0, doc, 3, 0, b"again", &rng);
    assert!(remote.append_sync(personal, &late).is_ok());
    // Bob cannot read it; his own is his.
    login_sync(&remote, &bob);
    assert_eq!(
        remote.changes_sync(personal, 0, 10),
        Err(SyncError::NotFound)
    );
    let mine = FeedId::from_account(bob.account);
    assert!(remote.changes_sync(mine, 0, 10).unwrap().changes.is_empty());
    // The listing shows the personal feed even before anything was written.
    let listing = remote.feeds_list_sync().unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].kind, kb_types::sync::FeedKind::Personal);
}

#[test]
fn a_member_posting_a_foreign_entry_or_the_wrong_feed_is_refused() {
    let (remote, fixture) = remote_and_fixture(19);
    let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
    let rng = SeededRng::new(78);
    let doc = fixture.doc();
    login_sync(&remote, &bob);
    // An entry that names Alice, posted by Bob's session.
    let foreign = conformance::sealed_entry(space.feed, &alice, 0, doc, 1, space.epoch, b"x", &rng);
    assert_eq!(
        remote.append_sync(space.feed, &foreign),
        Err(SyncError::Forbidden)
    );
    // An entry of another feed.
    let other = conformance::sealed_entry(
        FeedId::from_account(bob.account),
        &bob,
        0,
        doc,
        1,
        space.epoch,
        b"x",
        &rng,
    );
    assert!(matches!(
        remote.append_sync(space.feed, &other),
        Err(SyncError::Protocol(_))
    ));
    // A membership entry through the entries endpoint.
    let profile = space.profile(&bob, "Bob");
    assert!(matches!(
        remote.append_sync(space.feed, &profile.encode()),
        Err(SyncError::Protocol(_))
    ));
    // Garbage.
    assert!(matches!(
        remote.append_sync(space.feed, b"garbage"),
        Err(SyncError::Encoding(_))
    ));
    // Oversized.
    let huge = vec![0u8; usize::try_from(kb_types::sync::MAX_WIRE_BYTES).unwrap() + 1];
    assert!(matches!(
        remote.append_sync(space.feed, &huge),
        Err(SyncError::Protocol(_))
    ));
}

// ----- the adversary -------------------------------------------------------------------------

#[test]
fn the_adversary_drops_the_tail_of_the_membership_log() {
    let (remote, fixture) = remote_and_fixture(20);
    let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
    let honest = membership_seqs(&remote, &bob, space.feed);
    assert_eq!(honest, vec![0, 1, 2, 3, 4]);
    login_sync(&remote, &bob);
    let honest_chain = remote
        .changes_sync(space.feed, 0, 200)
        .unwrap()
        .chain_hash_after;
    remote.set_adversary(Some(Adversary::DropMembership { from_seq: 3 }));
    assert_eq!(membership_seqs(&remote, &bob, space.feed), vec![0, 1, 2]);
    let page = remote.membership_get_sync(space.feed, None).unwrap();
    assert_eq!(
        page.head.seq, 2,
        "the served head is the last served entry's"
    );
    let changes = remote.changes_sync(space.feed, 0, 200).unwrap();
    assert_eq!(changes.changes.len(), 3);
    assert_ne!(changes.chain_hash_after, honest_chain);
    // The store is intact.
    assert_eq!(remote.inspect_feed(space.feed).len(), 5);
    remote.set_adversary(None);
    assert_eq!(membership_seqs(&remote, &alice, space.feed), honest);
}

#[test]
fn the_adversary_swaps_two_membership_entries() {
    let (remote, fixture) = remote_and_fixture(21);
    let ([_, bob, _], space) = block_on(trio(&remote, &fixture));
    login_sync(&remote, &bob);
    let honest = remote.membership_get_sync(space.feed, None).unwrap();
    remote.set_adversary(Some(Adversary::ReorderMembership));
    let lied = remote.membership_get_sync(space.feed, None).unwrap();
    let n = honest.entries.len();
    assert_eq!(lied.entries[n - 1].bytes, honest.entries[n - 2].bytes);
    assert_eq!(lied.entries[n - 2].bytes, honest.entries[n - 1].bytes);
    assert_eq!(
        lied.entries.iter().map(|i| i.cursor).collect::<Vec<_>>(),
        honest.entries.iter().map(|i| i.cursor).collect::<Vec<_>>(),
        "the cursors stay in place"
    );
    assert_eq!(lied.entries[0].bytes, honest.entries[0].bytes);
}

#[test]
fn the_adversary_adds_a_member_without_an_entry() {
    let (remote, fixture) = remote_and_fixture(22);
    let (_, space) = block_on(trio(&remote, &fixture));
    let [erin] = block_on(conformance::enroll_n::<MemoryRemote, 1>(&remote, &fixture));
    let rng = SeededRng::new(79);
    login_sync(&remote, &erin);
    assert_eq!(
        remote.changes_sync(space.feed, 0, 10),
        Err(SyncError::NotFound)
    );
    remote.set_adversary(Some(Adversary::AddMemberWithoutEntry {
        account: erin.account,
    }));
    assert!(remote.changes_sync(space.feed, 0, 10).is_ok(), "reads");
    let entry = conformance::sealed_entry(
        space.feed,
        &erin,
        0,
        fixture.doc(),
        1,
        space.epoch,
        b"intruder",
        &rng,
    );
    assert!(
        remote.append_sync(space.feed, &entry).is_ok(),
        "appends as an Editor"
    );
    let listing = remote.feeds_list_sync().unwrap();
    assert!(
        listing
            .iter()
            .any(|l| l.feed == space.feed && l.role == Some(MemberRole::Editor))
    );
    // The honest access list does not name her.
    assert!(
        remote
            .feed_members(space.feed)
            .iter()
            .all(|(account, _)| *account != erin.account)
    );
}

#[test]
fn the_adversary_forks_the_membership_log_between_readers() {
    let (remote, fixture) = remote_and_fixture(23);
    let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
    remote.set_adversary(Some(Adversary::ForkMembership));
    let creator = membership_seqs(&remote, &alice, space.feed);
    let other = membership_seqs(&remote, &bob, space.feed);
    assert_eq!(creator, vec![0, 1, 2, 3, 4]);
    assert_eq!(other, vec![0, 1, 2, 3], "the newest entry is missing");
    login_sync(&remote, &alice);
    let creators_chain = remote
        .changes_sync(space.feed, 0, 200)
        .unwrap()
        .chain_hash_after;
    login_sync(&remote, &bob);
    let others_chain = remote
        .changes_sync(space.feed, 0, 200)
        .unwrap()
        .chain_hash_after;
    assert_ne!(creators_chain, others_chain, "the chains fork");
}

#[test]
fn the_adversary_withholds_the_tail_of_a_device_log() {
    let (remote, fixture) = remote_and_fixture(24);
    let [mut bob] = block_on(conformance::enroll_n::<MemoryRemote, 1>(&remote, &fixture));
    bob.add_device(fixture.now());
    bob.add_device(fixture.now() + 1);
    remote
        .append_log_sync(bob.account, &bob.encoded_log_after(0))
        .unwrap();
    assert_eq!(remote.fetch_log_sync(bob.account, None).unwrap().len(), 3);
    remote.set_adversary(Some(Adversary::WithholdLogTail {
        account: bob.account,
        keep: 1,
    }));
    assert_eq!(remote.fetch_log_sync(bob.account, None).unwrap().len(), 1);
    let batch = remote
        .logs_batch_sync(&[LogRequest {
            account: bob.account,
            after: None,
        }])
        .unwrap();
    assert_eq!(batch[0].entries.len(), 1);
    assert!(
        remote
            .fetch_log_sync(bob.account, Some(0))
            .unwrap()
            .is_empty()
    );
    remote.set_adversary(None);
    assert_eq!(remote.fetch_log_sync(bob.account, None).unwrap().len(), 3);
}

#[test]
fn the_adversary_serves_an_older_snapshot() {
    let (remote, fixture) = remote_and_fixture(25);
    let ([alice, ..], space) = block_on(trio(&remote, &fixture));
    let (once, twice) = (fixture.doc(), fixture.doc());
    login_sync(&remote, &alice);
    let first = space.doc_snapshot(&alice, 0, twice, 1, space.epoch, b"first");
    let second = space.doc_snapshot(&alice, 0, twice, 2, space.epoch, b"second");
    let single = space.doc_snapshot(&alice, 0, once, 2, space.epoch, b"single");
    remote.put_snapshot_sync(space.feed, &first).unwrap();
    remote.put_snapshot_sync(space.feed, &second).unwrap();
    remote.put_snapshot_sync(space.feed, &single).unwrap();
    assert_eq!(
        remote.get_snapshot_sync(space.feed, twice).unwrap(),
        Some(second.clone())
    );
    remote.set_adversary(Some(Adversary::TruncateSnapshot));
    assert_eq!(
        remote.get_snapshot_sync(space.feed, twice).unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        remote.get_snapshot_sync(space.feed, once).unwrap(),
        Some(single)
    );
    assert!(
        remote
            .snapshots_sync(space.feed)
            .unwrap()
            .snapshots
            .contains(&first)
    );
}

#[test]
fn the_adversary_serves_an_expired_invite() {
    let (remote, fixture) = remote_and_fixture(26);
    let ([alice, ..], mut space) = block_on(trio(&remote, &fixture));
    let [erin] = block_on(conformance::enroll_n::<MemoryRemote, 1>(&remote, &fixture));
    space.now = fixture.now();
    let (invite, secret) = space.invite(&alice, MemberRole::Editor, fixture.now() + HOUR_MS, None);
    block_on(conformance::post_ok(
        &remote, &mut space, &alice, &invite, "invite",
    ));
    let id = kb_keys::invite::invite_id(&secret);
    let sig = Space::open_sig(&secret, remote.server_epoch());
    remote.advance_ms(2 * HOUR_MS);
    assert_eq!(remote.invite_open_sync(id, &sig), Err(SyncError::NotFound));
    let join = space.join(&erin, &secret);
    assert_eq!(
        block_on(conformance::post(&remote, &mut space, &erin, &join)),
        Err(SyncError::NotFound)
    );
    remote.set_adversary(Some(Adversary::ServeExpiredInvite));
    assert!(remote.invite_open_sync(id, &sig).is_ok(), "served as open");
    conformance::ok(
        block_on(conformance::post(&remote, &mut space, &erin, &join)),
        "the Join of an expired invite is accepted",
    );
    assert!(
        remote
            .feed_members(space.feed)
            .iter()
            .any(|(account, _)| *account == erin.account)
    );
}

#[test]
fn the_adversary_accepts_anything_and_clients_must_reject() {
    let (remote, fixture) = remote_and_fixture(27);
    let ([alice, bob, carol], mut space) = block_on(trio(&remote, &fixture));
    let [erin] = block_on(conformance::enroll_n::<MemoryRemote, 1>(&remote, &fixture));
    let rng = SeededRng::new(80);
    let doc = fixture.doc();
    remote.set_adversary(Some(Adversary::AcceptAll));
    // A Viewer's append and a stranger's append are stored.
    login_sync(&remote, &carol);
    let viewer_entry = space.doc_entry(&carol, 0, doc, 1, space.epoch, b"a Viewer writes");
    assert!(remote.append_sync(space.feed, &viewer_entry).is_ok());
    login_sync(&remote, &erin);
    let stranger_entry =
        conformance::sealed_entry(space.feed, &erin, 0, doc, 1, space.epoch, b"intruder", &rng);
    assert!(remote.append_sync(space.feed, &stranger_entry).is_ok());
    // An entry that names another device than the session's, and a stale epoch: stored too.
    let foreign = space.doc_entry(&alice, 0, doc, 7, 0, b"as Alice");
    assert!(remote.append_sync(space.feed, &foreign).is_ok());
    // A forged membership entry (an Editor's Invite) is stored without changing anything.
    let members_before = remote.feed_members(space.feed);
    let forged = space
        .invite(&bob, MemberRole::Owner, fixture.now() + DAY_MS, None)
        .0;
    login_sync(&remote, &bob);
    assert!(
        remote
            .membership_append_sync(space.feed, &[forged.encode()])
            .is_ok()
    );
    assert_eq!(remote.feed_members(space.feed), members_before);
    assert!(
        remote
            .inspect_feed(space.feed)
            .iter()
            .any(|(_, b)| *b == forged.encode())
    );
    remote.set_adversary(None);
    // The honest rules did not learn of the forged Invite: the next honest entry of the head the
    // clients know of is refused as a lost race only because the feed holds the forged entry.
    space.now = fixture.now();
    let next = space.profile(&alice, "Alice");
    assert!(matches!(
        block_on(conformance::post(&remote, &mut space, &alice, &next)),
        Ok(_) | Err(SyncError::Conflict(_))
    ));
}

#[test]
fn the_adversary_forks_the_feed_per_device() {
    let (remote, fixture) = remote_and_fixture(28);
    let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
    let doc = fixture.doc();
    remote.set_adversary(Some(Adversary::ForkFeed));
    login_sync(&remote, &alice);
    let mine = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"alice");
    remote.append_sync(space.feed, &mine).unwrap();
    login_sync(&remote, &bob);
    let theirs = space.doc_entry(&bob, 0, doc, 1, space.epoch, b"bob");
    remote.append_sync(space.feed, &theirs).unwrap();
    let bobs = read_bytes(&remote, &bob, space.feed);
    assert!(bobs.contains(&theirs) && !bobs.contains(&mine));
    let alices = read_bytes(&remote, &alice, space.feed);
    assert!(alices.contains(&mine) && !alices.contains(&theirs));
    remote.set_adversary(None);
    assert_eq!(read_bytes(&remote, &alice, space.feed).len(), 7);
}

#[test]
fn the_adversary_hides_a_remove_from_everyone_but_its_signer() {
    let (remote, fixture) = remote_and_fixture(29);
    let ([alice, bob, carol], mut space) = block_on(trio(&remote, &fixture));
    space.now = fixture.now();
    let remove = space.remove(&alice, bob.account, &[&alice, &carol]);
    block_on(conformance::post_ok(
        &remote, &mut space, &alice, &remove, "remove",
    ));
    remote.set_adversary(Some(Adversary::HideMembership));
    assert_eq!(
        membership_seqs(&remote, &carol, space.feed).last(),
        Some(&4)
    );
    assert_eq!(
        membership_seqs(&remote, &alice, space.feed).last(),
        Some(&5)
    );
    assert!(!read_bytes(&remote, &carol, space.feed).contains(&remove.encode()));
    remote.set_adversary(None);
    assert_eq!(
        membership_seqs(&remote, &carol, space.feed).last(),
        Some(&5)
    );
}

#[test]
fn entries_the_fold_refuses_never_change_the_access_list() {
    let (remote, fixture) = remote_and_fixture(30);
    let ([alice, bob, _], mut space) = block_on(trio(&remote, &fixture));
    let owners_only = space.settings(&bob, SpaceSettings::default());
    assert_eq!(
        block_on(conformance::post(&remote, &mut space, &bob, &owners_only)),
        Err(SyncError::Forbidden)
    );
    let before = remote.feed_members(space.feed);
    let owner_invite = space.invite_with(
        &alice,
        &kb_keys::invite::InviteSecret::generate(&SeededRng::new(3)),
        MemberRole::Owner,
        fixture.now() + DAY_MS,
        None,
    );
    assert!(matches!(
        block_on(conformance::post(
            &remote,
            &mut space,
            &alice,
            &owner_invite
        )),
        Err(SyncError::Protocol(_))
    ));
    assert_eq!(remote.feed_members(space.feed), before);
}

// ----- the realtime socket (M13 slice 3, ADR-0050) ---------------------------------------------

mod sockets {
    use kb_types::sharing::{INLINE_ENTRY_MAX_BYTES, PRESENCE_FRAMES_PER_SECOND};

    use super::*;
    use crate::v2::conformance::ws::WsHarness;
    use crate::v2::ws::{InlineEntry, PresenceFrame, ServerFrame};

    /// A socket of `who` (its own connection and session) subscribed to `feeds`.
    fn socket(remote: &MemoryRemote, who: &Person, feeds: &[FeedId]) -> MemorySocket {
        let socket = block_on(remote.open_socket(who)).unwrap();
        socket.subscribe(feeds).unwrap();
        socket
    }

    /// Appends a document entry of `who` to the feed; answers `(entry, cursor, chain)`.
    fn append(
        remote: &MemoryRemote,
        space: &Space,
        who: &Person,
        doc: kb_types::DocId,
        seq: u64,
        payload: &[u8],
    ) -> (Vec<u8>, u64, [u8; 32]) {
        login_sync(remote, who);
        let bytes = space.doc_entry(who, 0, doc, seq, space.epoch, payload);
        let (cursor, chain) = remote.append_sync(space.feed, &bytes).unwrap();
        (bytes, cursor, chain)
    }

    fn entry_frame(socket: &MemorySocket) -> InlineEntry {
        match socket.recv() {
            Some(ServerFrame::Entry(entry)) => entry,
            other => panic!("expected an entry frame, got {other:?}"),
        }
    }

    #[test]
    fn the_reference_remote_passes_the_realtime_conformance_checks() {
        let (remote, fixture) = remote_and_fixture(60);
        block_on(conformance::run_all_ws(&remote, &remote, &fixture));
    }

    #[test]
    fn a_socket_needs_a_session_and_answers_unauthorized_once_it_is_gone() {
        let (remote, fixture) = remote_and_fixture(61);
        assert_eq!(remote.ws_open().unwrap_err(), SyncError::Unauthorized);
        let ([alice, ..], space) = block_on(trio(&remote, &fixture));
        let mine = socket(&remote, &alice, &[space.feed]);
        assert!(mine.is_open());
        mine.close();
        assert!(!mine.is_open());
        assert_eq!(mine.subscribe(&[space.feed]), Err(SyncError::Unauthorized));
        assert_eq!(
            mine.send_presence(&PresenceFrame {
                feed: space.feed,
                envelope: Vec::new()
            }),
            Err(SyncError::Unauthorized)
        );
        assert_eq!(mine.recv(), None);
    }

    #[test]
    fn a_subscription_names_at_most_the_feed_limit() {
        let (remote, fixture) = remote_and_fixture(62);
        let ([alice, ..], space) = block_on(trio(&remote, &fixture));
        let mine = socket(&remote, &alice, &[]);
        let many: Vec<FeedId> = (0..=kb_types::sharing::MAX_FEEDS_PER_ACCOUNT)
            .map(|_| space.random_feed())
            .collect();
        assert!(matches!(mine.subscribe(&many), Err(SyncError::Protocol(_))));
        assert!(mine.subscribe(&many[..100]).is_ok());
    }

    #[test]
    fn a_refused_presence_frame_answers_its_reason_and_nothing_reaches_the_others() {
        let (remote, fixture) = remote_and_fixture(63);
        let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
        let [erin] = block_on(conformance::enroll_n::<MemoryRemote, 1>(&remote, &fixture));
        let feed = space.feed;
        let alice_ws = socket(&remote, &alice, &[feed]);
        let bob_ws = socket(&remote, &bob, &[feed, FeedId::from_account(bob.account)]);
        let erin_ws = socket(&remote, &erin, &[feed]);
        let epoch = space.epoch;

        let mut spoof = space.presence_header(&bob, 0, epoch, 1);
        spoof.account_id = alice.account;
        assert_eq!(
            bob_ws.send_presence(&space.presence_frame_with(&spoof, &bob, 0, b"x")),
            Err(SyncError::Authentication)
        );
        let mut elsewhere = space.presence_frame(&bob, 0, epoch, 2, b"x");
        elsewhere.feed = space.random_feed();
        assert!(matches!(
            bob_ws.send_presence(&elsewhere),
            Err(SyncError::Protocol(_))
        ));
        let oversize = PresenceFrame {
            feed,
            envelope: vec![0; kb_types::sharing::PRESENCE_MAX_FRAME_BYTES],
        };
        assert!(matches!(
            bob_ws.send_presence(&oversize),
            Err(SyncError::Protocol(_))
        ));
        // The biggest update the editor sends fits, and is relayed.
        let fits = space.presence_frame(&bob, 0, epoch, 3, &vec![5; 3072]);
        assert!(bob_ws.send_presence(&fits).is_ok());
        assert_eq!(alice_ws.recv(), Some(ServerFrame::Presence(fits)));
        assert!(matches!(
            bob_ws.send_presence(&PresenceFrame {
                feed,
                envelope: vec![1; 60]
            }),
            Err(SyncError::Encoding(_))
        ));
        // A feed the socket never subscribed to, the personal feed, a stranger's frame.
        let mut foreign = space.presence_header(&bob, 0, epoch, 4);
        foreign.feed = space.random_feed();
        assert_eq!(
            bob_ws.send_presence(&space.presence_frame_with(&foreign, &bob, 0, b"x")),
            Err(SyncError::NotFound)
        );
        let mut home = space.presence_header(&bob, 0, epoch, 5);
        home.feed = FeedId::from_account(bob.account);
        assert_eq!(
            bob_ws.send_presence(&space.presence_frame_with(&home, &bob, 0, b"x")),
            Err(SyncError::NotFound)
        );
        let from_erin = space.presence_frame(&erin, 0, epoch, 1, b"x");
        assert_eq!(erin_ws.send_presence(&from_erin), Err(SyncError::NotFound));
        // Nothing of it was relayed.
        assert_eq!(alice_ws.recv(), None);
        assert_eq!(erin_ws.recv(), None);
    }

    #[test]
    fn the_rate_limit_counts_every_frame_of_a_socket_on_the_remotes_clock() {
        let (remote, fixture) = remote_and_fixture(64);
        let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
        let feed = space.feed;
        let alice_ws = socket(&remote, &alice, &[feed]);
        let bob_ws = socket(&remote, &bob, &[feed]);
        let limit = u64::from(PRESENCE_FRAMES_PER_SECOND);
        assert_eq!(limit, 10);
        let frames: Vec<PresenceFrame> = (1..=2 * limit)
            .map(|counter| space.presence_frame(&bob, 0, space.epoch, counter, b"m"))
            .collect();
        for frame in &frames {
            // Over the limit the frame is dropped and the answer is still Ok.
            assert!(bob_ws.send_presence(frame).is_ok());
        }
        let mut got = Vec::new();
        while let Some(frame) = alice_ws.recv() {
            got.push(frame);
        }
        assert_eq!(
            got,
            frames[..10]
                .iter()
                .cloned()
                .map(ServerFrame::Presence)
                .collect::<Vec<_>>()
        );
        // Another socket has its own allowance.
        let from_alice = space.presence_frame(&alice, 0, space.epoch, 1, b"a");
        assert!(alice_ws.send_presence(&from_alice).is_ok());
        assert_eq!(bob_ws.recv(), Some(ServerFrame::Presence(from_alice)));
        // A refused frame counts too: ten spoofed frames use the allowance up.
        remote.advance_ms(1_000);
        let mut spoof = space.presence_header(&bob, 0, space.epoch, 99);
        spoof.account_id = alice.account;
        for _ in 0..10 {
            assert_eq!(
                bob_ws.send_presence(&space.presence_frame_with(&spoof, &bob, 0, b"x")),
                Err(SyncError::Authentication)
            );
        }
        let eleventh = space.presence_frame(&bob, 0, space.epoch, 100, b"x");
        assert!(bob_ws.send_presence(&eleventh).is_ok());
        assert_eq!(alice_ws.recv(), None, "the eleventh frame is dropped");
        // The window moves with the clock.
        remote.advance_ms(1_000);
        let again = space.presence_frame(&bob, 0, space.epoch, 101, b"x");
        assert!(bob_ws.send_presence(&again).is_ok());
        assert_eq!(alice_ws.recv(), Some(ServerFrame::Presence(again)));
    }

    #[test]
    fn a_dropped_frame_leaves_a_gap_the_next_frame_shows() {
        let (remote, fixture) = remote_and_fixture(65);
        let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
        let bob_ws = socket(&remote, &bob, &[space.feed]);
        let doc = fixture.doc();
        let (_, first, _) = append(&remote, &space, &alice, doc, 1, b"one");
        let (_, kept, _) = append(&remote, &space, &alice, doc, 2, b"two");
        assert_eq!(entry_frame(&bob_ws).cursor, first);
        assert_eq!(entry_frame(&bob_ws).cursor, kept);
        remote.ws_drop_next(1);
        let (_, lost, _) = append(&remote, &space, &alice, doc, 3, b"three");
        let (bytes, last, chain) = append(&remote, &space, &alice, doc, 4, b"four");
        let frame = entry_frame(&bob_ws);
        assert_eq!(
            frame,
            InlineEntry {
                feed: space.feed,
                prev_cursor: lost,
                cursor: last,
                chain_after: chain,
                entry: bytes
            }
        );
        // Bob's cursor is `kept`, not `prev_cursor`: he must pull, which has the lost item.
        assert_ne!(frame.prev_cursor, kept);
        assert_eq!(bob_ws.recv(), None);
        login_sync(&remote, &bob);
        let pulled = remote.changes_sync(space.feed, kept, 10).unwrap();
        assert_eq!(pulled.changes.len(), 2);
        // The knob is spent: the next frame arrives.
        let (_, next, _) = append(&remote, &space, &alice, doc, 5, b"five");
        assert_eq!(entry_frame(&bob_ws).cursor, next);
    }

    #[test]
    fn two_frames_can_arrive_in_the_opposite_order() {
        let (remote, fixture) = remote_and_fixture(66);
        let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
        let bob_ws = socket(&remote, &bob, &[space.feed]);
        let doc = fixture.doc();
        remote.ws_swap_next_two();
        let (_, first, _) = append(&remote, &space, &alice, doc, 1, b"one");
        assert_eq!(bob_ws.recv(), None, "the first frame is held back");
        let (_, second, _) = append(&remote, &space, &alice, doc, 2, b"two");
        let early = entry_frame(&bob_ws);
        let late = entry_frame(&bob_ws);
        assert_eq!((early.cursor, late.cursor), (second, first));
        // The receiver sees it: the early frame's previous cursor is not what it has applied.
        assert_eq!(early.prev_cursor, first);
        assert_eq!(bob_ws.recv(), None);
        // Only once.
        let (_, third, _) = append(&remote, &space, &alice, doc, 3, b"three");
        assert_eq!(entry_frame(&bob_ws).cursor, third);
    }

    #[test]
    fn a_socket_ends_with_its_session_its_device_or_the_connection() {
        let (remote, fixture) = remote_and_fixture(67);
        let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
        let feed = space.feed;
        let doc = fixture.doc();
        // Another authenticate on the connection replaces the session and ends its sockets.
        let connection = remote.clone();
        login_sync(&connection, &bob);
        let replaced = connection.ws_open().unwrap();
        replaced.subscribe(&[feed]).unwrap();
        assert!(replaced.is_open());
        login_sync(&connection, &alice);
        assert!(!replaced.is_open());
        assert_eq!(replaced.recv(), None);
        assert_eq!(replaced.subscribe(&[feed]), Err(SyncError::Unauthorized));
        // A socket of another connection is not affected.
        let kept = socket(&remote, &bob, &[feed]);
        login_sync(&connection, &bob);
        assert!(kept.is_open());
        let (_, cursor, _) = append(&remote, &space, &alice, doc, 1, b"x");
        assert_eq!(entry_frame(&kept).cursor, cursor);
        assert_eq!(remote.ws_open_count(), 1);
        // Offline closes every socket, and they do not come back with the remote.
        remote.set_offline(true);
        assert!(!kept.is_open());
        assert_eq!(remote.ws_open_count(), 0);
        remote.set_offline(false);
        assert!(!kept.is_open());
        // Dropping a socket frees its slot.
        let dropped = socket(&remote, &bob, &[feed]);
        assert_eq!(remote.ws_open_count(), 1);
        drop(dropped);
        assert_eq!(remote.ws_open_count(), 0);
        // A reset forgets every session and so every socket.
        let reset = socket(&remote, &bob, &[feed]);
        remote.reset_data();
        assert!(!reset.is_open());
    }

    #[test]
    fn a_revoked_device_stops_receiving() {
        let (remote, fixture) = remote_and_fixture(68);
        let ([mut alice, ..], space) = block_on(trio(&remote, &fixture));
        // Alice gets a second device and publishes it.
        let second = alice.add_device(fixture.now());
        remote
            .append_log_sync(alice.account, &alice.encoded_log_after(0))
            .unwrap();
        let first_ws = socket(&remote, &alice, &[space.feed]);
        assert!(first_ws.is_open());
        // The second device revokes the first and publishes it.
        alice.revoke_device(0, fixture.now() + 1);
        remote
            .authenticate_sync(
                alice.account,
                alice.device_id(second),
                &alice.signer(second),
            )
            .unwrap();
        remote
            .append_log_sync(alice.account, &alice.encoded_log_after(1))
            .unwrap();
        assert!(!first_ws.is_open(), "the revoked device's socket ended");
        assert_eq!(remote.ws_open_count(), 0);
        // Its session is gone with it, and it cannot sign in again.
        let revoked = remote.clone();
        assert_eq!(
            revoked.authenticate_sync(alice.account, alice.device_id(0), &alice.signer(0)),
            Err(SyncError::Unauthorized)
        );
        assert_eq!(revoked.ws_open().unwrap_err(), SyncError::Unauthorized);
    }

    #[test]
    fn an_entry_over_the_inline_limit_is_announced_not_relayed() {
        let (remote, fixture) = remote_and_fixture(69);
        let ([alice, bob, _], space) = block_on(trio(&remote, &fixture));
        let bob_ws = socket(&remote, &bob, &[space.feed]);
        let doc = fixture.doc();
        let (big, cursor, _) = append(&remote, &space, &alice, doc, 1, &vec![3u8; 40_000]);
        assert!(big.len() > INLINE_ENTRY_MAX_BYTES);
        assert_eq!(
            bob_ws.recv(),
            Some(ServerFrame::Changed {
                feed: space.feed,
                cursor
            })
        );
    }

    #[test]
    fn the_frames_of_membership_entries_chain_and_carry_the_running_hash() {
        let (remote, fixture) = remote_and_fixture(70);
        let ([alice, bob, carol], mut space) = block_on(trio(&remote, &fixture));
        let alice_ws = socket(&remote, &alice, &[space.feed]);
        space.now = fixture.now();
        let one = space.profile(&bob, "Bob");
        block_on(conformance::post_ok(
            &remote,
            &mut space,
            &bob,
            &one,
            "a Profile",
        ));
        let two = space.profile(&carol, "Carol");
        block_on(conformance::post_ok(
            &remote,
            &mut space,
            &carol,
            &two,
            "a Profile",
        ));
        let first = entry_frame(&alice_ws);
        let second = entry_frame(&alice_ws);
        assert_eq!(first.entry, one.encode());
        assert_eq!(second.entry, two.encode());
        assert_eq!(second.prev_cursor, first.cursor);
        // The chain after each is the running hash over the feed's items up to it.
        let mut running = crate::v2::chain_genesis(&remote.server_epoch());
        let mut seen = 0;
        for (cursor, bytes) in remote.inspect_feed(space.feed) {
            running = crate::v2::chain_next(&running, &bytes);
            if cursor == first.cursor {
                assert_eq!(first.chain_after, running);
                seen += 1;
            }
            if cursor == second.cursor {
                assert_eq!(second.chain_after, running);
                seen += 1;
            }
        }
        assert_eq!(seen, 2);
    }
}

/// View links (M13 slice 4): the conformance checks, then what they cannot see from one handle.
mod view_links {
    use kb_keys::link::LinkKey;

    use super::*;
    use crate::v2::{LinkFetch, LinkPut, Publication, open_publication, seal_publication};

    const TITLE: &str = "A page title nobody may read on the server";

    fn publication(key: &LinkKey) -> Vec<u8> {
        let publication = Publication {
            page_id: kb_types::PageId::from_bytes([3; 16]),
            title: TITLE.to_owned(),
            icon: None,
            cover: None,
            cover_y: 0,
            snapshot: b"snapshot".to_vec(),
            attachments: Vec::new(),
            mentions: Vec::new(),
        };
        seal_publication(key, &publication, &SeededRng::new(5)).unwrap()
    }

    /// A remote with one account that owns a space holding one link; the second handle has no session.
    fn with_one_link(
        expires_at: Option<i64>,
    ) -> (
        MemoryRemote,
        MemoryRemote,
        Fixture,
        LinkKey,
        Vec<u8>,
        FeedId,
    ) {
        let (remote, fixture) = remote_and_fixture(30);
        let [alice] = block_on(conformance::enroll_n::<_, 1>(&remote, &fixture));
        let mut space = fixture.space("Links");
        block_on(conformance::found(&remote, &mut space, &alice));
        let key = LinkKey::generate(&SeededRng::new(31));
        let sealed = publication(&key);
        let put = LinkPut {
            feed: space.feed,
            expires_at,
            chunks: Vec::new(),
            publication: sealed.clone(),
        };
        assert_eq!(remote.link_put_sync(key.link_id(), &put).unwrap(), 1);
        let anonymous = remote.clone();
        (remote, anonymous, fixture, key, sealed, space.feed)
    }

    #[test]
    fn the_checks_pass_with_and_without_a_clock() {
        let (remote, fixture) = remote_and_fixture(20);
        block_on(conformance::check_view_links(&remote, &fixture));
        // Without a way to move the clock the expiry part skips itself.
        let plain = MemoryRemote::new(&SeededRng::new(21));
        plain.set_now_ms(FIXTURE_TIME_MS);
        block_on(conformance::check_view_links(
            &plain,
            &Fixture::generate(21),
        ));
        // The read limit has its own check, which exhausts it.
        block_on(conformance::check_link_read_rate_limit(&remote, &fixture));
    }

    #[test]
    fn reads_need_no_session_and_the_server_holds_only_the_sealed_bytes() {
        let (remote, anonymous, _fixture, key, sealed, _feed) = with_one_link(None);
        match anonymous.link_get_sync(key.link_id(), None).unwrap() {
            LinkFetch::Publication { pub_seq, bytes } => {
                assert_eq!(pub_seq, 1);
                assert_eq!(bytes, sealed);
            }
            LinkFetch::NotModified => panic!("a first read is never NotModified"),
        }
        // What the server stores does not contain the title, and opens with the key only.
        let (pub_seq, stored) = remote.inspect_link(key.link_id()).unwrap();
        assert_eq!(pub_seq, 1);
        assert!(
            !stored
                .windows(TITLE.len())
                .any(|window| window == TITLE.as_bytes())
        );
        assert_eq!(open_publication(&key, &stored).unwrap().title, TITLE);
        assert_eq!(remote.inspect_links(), vec![key.link_id()]);
        // Writes and revocation need a session; the handle without one is Unauthorized.
        assert!(matches!(
            anonymous.link_delete_sync(key.link_id()),
            Err(SyncError::Unauthorized)
        ));
        let put = LinkPut {
            feed: FeedId::from_bytes([1; 16]),
            expires_at: None,
            chunks: Vec::new(),
            publication: stored,
        };
        assert!(matches!(
            anonymous.link_put_sync(key.link_id(), &put),
            Err(SyncError::Unauthorized)
        ));
    }

    #[test]
    fn link_reads_are_limited_per_connection_per_minute_for_publications_and_chunks_together() {
        let (remote, anonymous, _fixture, key, _sealed, _feed) = with_one_link(None);
        let id = key.link_id();
        let hash = kb_types::ChunkId::from_bytes([9; 32]);
        for round in 0..LINK_GETS_PER_MINUTE_PER_IP {
            let answer = if round % 2 == 0 {
                anonymous.link_get_sync(id, Some(1)).map(|_| ())
            } else {
                anonymous.link_chunk_get_sync(id, hash).map(|_| ())
            };
            assert!(
                matches!(answer, Ok(()) | Err(SyncError::NotFound)),
                "{round}"
            );
        }
        assert!(matches!(
            anonymous.link_get_sync(id, None),
            Err(SyncError::Unavailable(_))
        ));
        assert!(matches!(
            anonymous.link_chunk_get_sync(id, hash),
            Err(SyncError::Unavailable(_))
        ));
        // Another connection has its own budget; a minute later the first has its budget back.
        assert!(remote.link_get_sync(id, None).is_ok());
        remote.advance_ms(60_001);
        assert!(anonymous.link_get_sync(id, None).is_ok());
    }

    #[test]
    fn a_link_stops_at_its_expiry_on_the_servers_clock_and_a_revoked_one_stays_gone() {
        let expires_at = FIXTURE_TIME_MS + 10_000;
        let (remote, anonymous, _fixture, key, _sealed, feed) = with_one_link(Some(expires_at));
        let id = key.link_id();
        remote.advance_ms(9_999);
        assert!(anonymous.link_get_sync(id, None).is_ok());
        remote.advance_ms(1);
        assert!(matches!(
            anonymous.link_get_sync(id, None),
            Err(SyncError::NotFound)
        ));
        // The row is kept until revoked, and a refresh cannot bring it back.
        assert_eq!(remote.inspect_links(), vec![id]);
        let put = LinkPut {
            feed,
            expires_at: None,
            chunks: Vec::new(),
            publication: publication(&key),
        };
        assert_eq!(remote.link_put_sync(id, &put).unwrap(), 2);
        assert!(matches!(
            anonymous.link_get_sync(id, None),
            Err(SyncError::NotFound)
        ));
        remote.link_delete_sync(id).unwrap();
        assert!(remote.inspect_links().is_empty());
        assert!(remote.inspect_link(id).is_none());
    }

    #[test]
    fn an_offline_server_answers_unavailable_and_a_reset_forgets_the_links() {
        let (remote, anonymous, _fixture, key, _sealed, _feed) = with_one_link(None);
        let id = key.link_id();
        remote.set_offline(true);
        assert!(matches!(
            anonymous.link_get_sync(id, None),
            Err(SyncError::Unavailable(_))
        ));
        assert!(matches!(
            anonymous.link_chunk_get_sync(id, kb_types::ChunkId::from_bytes([1; 32])),
            Err(SyncError::Unavailable(_))
        ));
        remote.set_offline(false);
        assert!(anonymous.link_get_sync(id, None).is_ok());
        remote.reset_data();
        assert!(remote.inspect_links().is_empty());
    }
}
