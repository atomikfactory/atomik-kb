//! Milestone 13, slice 3: realtime and presence in the core, on the people of the simulator in `super`.
//! Presence frames are sealed by one account's core and opened by another's (a spoofed account, a
//! replayed or reordered counter, a removed member, an epoch that is not the current one, the 30-second
//! timeout, the debounce hint); inline entries are offered to the apply path the way `kb-net`'s driver
//! offers what the socket relayed (equal to a pull, a gap, a duplicate, a Viewer's entry, a chain that
//! does not line up, a tampered signature, a membership entry, a feed that has not been read yet).

use kb_crypto::{Secret32, SigningKey};
use kb_keys::space_keys::presence_key;
use kb_storage::SyncQueries;
use kb_sync::v2::{
    InlineEntry, InlineOutcome, PresenceBody, PresenceHeader, SealedPresence, chain_next,
    peek_presence, seal_presence,
};
use kb_types::sharing::{PRESENCE_UPDATE_MAX_BYTES, PUSH_DEBOUNCE_PRESENCE_MS};
use kb_types::sync::{KEYRING_DOC_ID, PUSH_DEBOUNCE_MS};
use kb_types::{DeviceId, PresenceMessage, PresenceState, Request, Response};

use super::requests::*;
use super::*;

// ----- helpers ----------------------------------------------------------------------------------

fn seal(
    dev: &mut Dev,
    space: SpaceId,
    page: Option<PageId>,
    state: PresenceState,
    update: &[u8],
) -> SealedPresence {
    dev.core
        .presence_seal(space, page, state, update)
        .unwrap()
        .unwrap_or_else(|| panic!("{}: a frame to send", dev.name))
}

fn open(dev: &mut Dev, frame: &SealedPresence) -> Option<PresenceMessage> {
    dev.core.presence_open(frame.feed, &frame.envelope).unwrap()
}

fn open_bytes(dev: &mut Dev, feed: FeedId, envelope: &[u8]) -> Option<PresenceMessage> {
    dev.core.presence_open(feed, envelope).unwrap()
}

/// The frame is dropped, for this reason and no other.
fn dropped(dev: &mut Dev, frame: &SealedPresence, reason: &str) {
    dropped_bytes(dev, frame.feed, &frame.envelope, reason);
}

fn dropped_bytes(dev: &mut Dev, feed: FeedId, envelope: &[u8], reason: &str) {
    dev.core.shared.presence.last_drop = None;
    assert!(
        open_bytes(dev, feed, envelope).is_none(),
        "{}: the frame is dropped",
        dev.name
    );
    assert_eq!(
        dev.core.shared.presence.last_drop,
        Some(reason),
        "{}: why",
        dev.name
    );
}

fn counter_of(frame: &SealedPresence) -> u64 {
    peek_presence(&frame.envelope).unwrap().counter
}

fn list(dev: &mut Dev, space: SpaceId) -> Vec<kb_types::PresenceInfo> {
    match call(dev, Request::PresenceList { space_id: space }).unwrap() {
        Response::Presence(people) => people,
        other => panic!("{}: unexpected {other:?}", dev.name),
    }
}

fn presence_changed(events: &[Event], space: SpaceId) -> usize {
    events
        .iter()
        .filter(|event| **event == Event::PresenceChanged { space_id: space })
        .count()
}

fn ids(dev: &Dev) -> (kb_types::AccountId, DeviceId) {
    dev.core.sync_identity().unwrap().unwrap()
}

/// Alice's project with Bob (`role`) and Carol (an Editor), everybody settled.
fn trio(role: MemberRole) -> (Pair, Dev) {
    let mut pair = pair(role);
    let server = pair.alice.remote.clone();
    let (mut carol, _) = Dev::first("carol", 3, &server);
    carol.configure();
    add_member(
        &mut pair.alice,
        &mut carol,
        pair.feed,
        pair.space,
        MemberRole::Editor,
        "Carol",
    );
    Dev::settle(&mut [&mut pair.alice, &mut pair.bob, &mut carol], 3);
    pair.alice.events();
    pair.bob.events();
    carol.events();
    (pair, carol)
}

/// A frame as a hostile member would build it: the header and body of its choosing under the key
/// `holder` holds for epoch 1 of `feed`, signed by `signer`.
fn forged(
    holder: &mut Dev,
    feed: FeedId,
    header: &PresenceHeader,
    page: Option<PageId>,
    signer: &SigningKey,
) -> Vec<u8> {
    let sk = holder
        .core
        .feed_sk(feed, 1)
        .unwrap()
        .expect("the space key of epoch 1");
    seal_presence(
        header,
        &PresenceBody {
            doc_id: page.map_or(KEYRING_DOC_ID, DocId::for_page),
            state: PresenceState::Editing,
            update: vec![9, 9, 9],
        },
        &presence_key(&sk, feed),
        signer,
        &SeededRng::new(31),
    )
}

fn stranger_key(seed: u64) -> SigningKey {
    SigningKey::from_seed(&Secret32::generate(&SeededRng::new(seed)))
}

// ----- sealing and opening ----------------------------------------------------------------------

#[test]
fn a_frame_is_sealed_by_one_account_and_opened_by_another() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let (bob_account, bob_device) = ids(&bob);
    let secret = b"the caret of bob is at block 7, offset 12";
    let frame = seal(&mut bob, space, Some(plan), PresenceState::Editing, secret);
    // The server sees a sealed envelope: the cursor, the page and the name are not in it.
    assert!(
        !frame
            .envelope
            .windows(secret.len())
            .any(|window| window == secret),
        "the update is sealed"
    );
    assert!(
        !frame
            .envelope
            .windows(DocId::for_page(plan).to_bytes().len())
            .any(|window| window == DocId::for_page(plan).to_bytes()),
        "the page is sealed"
    );
    let header = peek_presence(&frame.envelope).unwrap();
    assert_eq!(header.feed, frame.feed);
    assert_eq!(
        (header.account_id, header.device_id),
        (bob_account, bob_device)
    );
    assert_eq!(header.epoch, 1);
    // The counter is seeded from the clock at the first frame and grows by one.
    assert_eq!(header.counter, bob.clock.now_ms() as u64);

    alice.events();
    let message = open(&mut alice, &frame).expect("an accepted frame");
    assert_eq!(message.update, secret.to_vec());
    let info = &message.info;
    assert_eq!(info.account_id, bob_account);
    assert_eq!(info.device_id, bob_device);
    assert_eq!(info.name, "Bob");
    assert_eq!(info.color, 1, "Alice created the space, Bob joined second");
    assert_eq!(info.page_id, Some(plan));
    assert_eq!(info.state, PresenceState::Editing);
    assert_eq!(
        info.updated_at,
        alice.clock.now_ms(),
        "the receiver's clock, never the sender's"
    );
    assert_eq!(presence_changed(&alice.events(), space), 1);

    // The request answers who is where; this device never shows itself.
    assert_eq!(list(&mut alice, space), vec![message.info.clone()]);
    assert!(list(&mut bob, space).is_empty());
    assert_eq!(
        fails(
            &mut alice,
            Request::PresenceList {
                space_id: SpaceId::from_bytes([7; 16])
            }
        )
        .code,
        ErrorCode::NotFound
    );

    // Alice answers with her own frame: another colour, another name.
    let back = seal(&mut alice, space, None, PresenceState::Idle, &[]);
    let reply = open(&mut bob, &back).expect("Bob accepts Alice's frame");
    assert_eq!(
        (
            reply.info.name.as_str(),
            reply.info.color,
            reply.info.page_id
        ),
        ("Alice", 0, None)
    );
    assert_eq!(reply.info.state, PresenceState::Idle);
    assert!(reply.update.is_empty());
}

#[test]
fn a_viewer_may_be_present_and_a_cursor_move_is_not_a_change() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        notes,
        ..
    } = pair(MemberRole::Viewer);
    alice.events();
    let first = seal(&mut bob, space, Some(plan), PresenceState::Viewing, &[1]);
    assert!(open(&mut alice, &first).is_some(), "a Viewer is a member");
    assert_eq!(presence_changed(&alice.events(), space), 1);

    // The cursor moves: same page, same state, a new frame, no event.
    let moved = seal(&mut bob, space, Some(plan), PresenceState::Viewing, &[2]);
    assert_eq!(counter_of(&moved), counter_of(&first) + 1);
    let message = open(&mut alice, &moved).expect("accepted");
    assert_eq!(message.update, vec![2]);
    assert_eq!(presence_changed(&alice.events(), space), 0);

    // Another page, then another state, then no page at all: an event each.
    let page = seal(&mut bob, space, Some(notes), PresenceState::Viewing, &[3]);
    assert_eq!(open(&mut alice, &page).unwrap().info.page_id, Some(notes));
    assert_eq!(presence_changed(&alice.events(), space), 1);
    let state = seal(&mut bob, space, Some(notes), PresenceState::Editing, &[4]);
    assert!(open(&mut alice, &state).is_some());
    assert_eq!(presence_changed(&alice.events(), space), 1);
    let gone = seal(&mut bob, space, None, PresenceState::Idle, &[]);
    let info = open(&mut alice, &gone).unwrap().info;
    assert_eq!((info.page_id, info.state), (None, PresenceState::Idle));
    assert_eq!(presence_changed(&alice.events(), space), 1);
    // One entry per device, whatever the number of frames.
    assert_eq!(list(&mut alice, space).len(), 1);

    // A document that is not a page of the space is "in no page".
    let (_, device) = ids(&bob);
    let header = PresenceHeader {
        feed: first.feed,
        account_id: bob.account(),
        device_id: device,
        epoch: 1,
        counter: counter_of(&gone) + 1,
    };
    let sk = bob.core.feed_sk(first.feed, 1).unwrap().unwrap();
    let odd = seal_presence(
        &header,
        &PresenceBody {
            doc_id: DocId::from_bytes([0x5a; 16]),
            state: PresenceState::Viewing,
            update: vec![],
        },
        &presence_key(&sk, first.feed),
        bob.core.signer(),
        &SeededRng::new(3),
    );
    let info = open_bytes(&mut alice, first.feed, &odd).unwrap().info;
    assert_eq!(info.page_id, None);
}

#[test]
fn sealing_has_nothing_to_send_outside_a_live_shared_space_and_checks_its_input() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    // A personal space: nothing to send, whatever the page.
    let personal = crate::test_support::spaces(&mut alice.core)
        .into_iter()
        .find(|info| info.id != space)
        .expect("Alice's personal space")
        .id;
    assert_eq!(
        alice
            .core
            .presence_seal(personal, None, PresenceState::Idle, &[])
            .unwrap(),
        None
    );
    assert!(list(&mut alice, personal).is_empty());
    // Input errors name their field; an unknown space is NotFound.
    let error = bob
        .core
        .presence_seal(
            space,
            Some(plan),
            PresenceState::Editing,
            &vec![0; PRESENCE_UPDATE_MAX_BYTES + 1],
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(
        error.details,
        Some(serde_json::json!({ "field": "update" }))
    );
    assert!(
        bob.core
            .presence_seal(
                space,
                Some(plan),
                PresenceState::Editing,
                &vec![0; PRESENCE_UPDATE_MAX_BYTES],
            )
            .unwrap()
            .is_some(),
        "the largest update fits"
    );
    let other_space_page = create_page(&mut alice.core, personal, None, "Mine").id;
    let error = alice
        .core
        .presence_seal(space, Some(other_space_page), PresenceState::Editing, &[])
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(
        error.details,
        Some(serde_json::json!({ "field": "pageId" }))
    );
    assert_eq!(
        bob.core
            .presence_seal(SpaceId::from_bytes([8; 16]), None, PresenceState::Idle, &[])
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    // An archive: Bob is removed, learns it, and has nothing to send any more.
    let bob_account = bob.account();
    remove_member(&mut alice, feed, bob_account);
    Dev::settle(&mut [&mut alice, &mut bob], 2);
    assert_eq!(
        bob.core
            .presence_seal(space, Some(plan), PresenceState::Viewing, &[])
            .unwrap(),
        None
    );
    assert!(list(&mut bob, space).is_empty());
}

// ----- forged, replayed, removed, outdated ------------------------------------------------------

#[test]
fn a_spoofed_account_a_stranger_s_device_and_a_wrong_feed_are_dropped() {
    let (
        Pair {
            mut alice,
            mut bob,
            space,
            feed,
            plan,
            ..
        },
        _carol,
    ) = trio(MemberRole::Editor);
    let (bob_account, bob_device) = ids(&bob);
    let header = |account, device, counter| PresenceHeader {
        feed,
        account_id: account,
        device_id: device,
        epoch: 1,
        counter,
    };
    alice.events();

    // Bob's account and device claimed by a key that is not Bob's device's: the signature fails.
    let spoof = forged(
        &mut bob,
        feed,
        &header(bob_account, bob_device, 10_000_000),
        Some(plan),
        &stranger_key(1),
    );
    dropped_bytes(&mut alice, feed, &spoof, "signature or sealing");

    // Bob's account with a device Bob's log does not list.
    let unknown_device = forged(
        &mut bob,
        feed,
        &header(bob_account, DeviceId::from_bytes([0x77; 16]), 10_000_000),
        Some(plan),
        &stranger_key(2),
    );
    dropped_bytes(
        &mut alice,
        feed,
        &unknown_device,
        "not a device of the member",
    );

    // An account that is no member at all, whatever it signs with.
    let outsider = forged(
        &mut bob,
        feed,
        &header(
            kb_types::AccountId::from_bytes([0x66; 16]),
            DeviceId::from_bytes([0x67; 16]),
            10_000_000,
        ),
        Some(plan),
        &stranger_key(3),
    );
    dropped_bytes(&mut alice, feed, &outsider, "not a current member");

    // A frame for another feed than the one it is handed to.
    let genuine = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[1]);
    dropped_bytes(
        &mut alice,
        FeedId::from_bytes([0x11; 16]),
        &genuine.envelope,
        "unknown feed",
    );
    let mut wrong_header = peek_presence(&genuine.envelope).unwrap();
    wrong_header.feed = FeedId::from_bytes([0x12; 16]);
    let bobs_signer = SigningKey::from_seed(&bob.core.signer().to_seed());
    let wrong = forged(&mut bob, feed, &wrong_header, Some(plan), &bobs_signer);
    dropped_bytes(&mut alice, feed, &wrong, "another feed");

    // Garbage and oversize frames.
    dropped_bytes(&mut alice, feed, &[], "malformed");
    dropped_bytes(&mut alice, feed, &[2; 40], "malformed");
    dropped_bytes(&mut alice, feed, &vec![0; 5000], "oversize");

    // A frame Bob's own device made is accepted; nothing above left a trace.
    assert!(list(&mut alice, space).is_empty());
    assert_eq!(presence_changed(&alice.events(), space), 0);
    assert!(open(&mut alice, &genuine).is_some());
    // A frame this device made is not a remote presence: it is dropped.
    let own = seal(&mut alice, space, Some(plan), PresenceState::Viewing, &[]);
    dropped(&mut alice, &own, "own frame");
}

#[test]
fn a_replayed_or_reordered_counter_is_dropped() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let one = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[1]);
    let two = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[2]);
    let three = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[3]);
    assert_eq!(
        (counter_of(&two), counter_of(&three)),
        (counter_of(&one) + 1, counter_of(&one) + 2)
    );
    assert!(open(&mut alice, &one).is_some());
    // The same frame again.
    dropped(&mut alice, &one, "counter not above the last accepted");
    // A later one arrives first: the earlier one is behind it.
    assert!(open(&mut alice, &three).is_some());
    dropped(&mut alice, &two, "counter not above the last accepted");
    dropped(&mut alice, &three, "counter not above the last accepted");
    // The newest keeps working.
    let four = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[4]);
    assert_eq!(open(&mut alice, &four).unwrap().update, vec![4]);

    // A restarted device seeds its counter from the clock again, above what it sent before.
    let before = counter_of(&four);
    bob.reopen(22, bob.clock.now_ms() + 60_000);
    let after = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[5]);
    assert!(
        counter_of(&after) > before,
        "{} > {before}",
        counter_of(&after)
    );
    assert_eq!(open(&mut alice, &after).unwrap().update, vec![5]);
}

#[test]
fn a_removed_members_frames_are_refused_and_their_presence_goes_at_once() {
    let (
        Pair {
            mut alice,
            mut bob,
            space,
            feed,
            plan,
            ..
        },
        mut carol,
    ) = trio(MemberRole::Editor);
    alice.events();
    let from_bob = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[1]);
    let from_carol = seal(&mut carol, space, Some(plan), PresenceState::Viewing, &[2]);
    assert!(open(&mut alice, &from_bob).is_some());
    assert!(open(&mut alice, &from_carol).is_some());
    let people = list(&mut alice, space);
    assert_eq!(
        people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["Bob", "Carol"],
        "in join order"
    );
    assert_eq!(
        people.iter().map(|p| p.color).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        alice.core.sync_push_debounce_ms().unwrap(),
        PUSH_DEBOUNCE_PRESENCE_MS
    );
    alice.events();

    // Alice removes Bob. When she folds the entry his presence goes, before any timeout.
    let bob_account = bob.account();
    remove_member(&mut alice, feed, bob_account);
    alice.cycle().unwrap();
    let events = alice.events();
    assert_eq!(presence_changed(&events, space), 1, "{events:?}");
    let people = list(&mut alice, space);
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].name, "Carol");
    assert_eq!(
        people[0].color, 1,
        "the colour is the index among the members now"
    );
    // Bob is offline and has not heard: his next frame, and the one he sent before, are refused.
    let late = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[3]);
    dropped(&mut alice, &late, "not a current member");
    dropped(&mut alice, &from_bob, "not a current member");
    // Carol, who is still a member but still at the old epoch, is refused until she has the new key.
    let stale = seal(&mut carol, space, Some(plan), PresenceState::Viewing, &[4]);
    dropped(&mut alice, &stale, "not the current epoch");
    Dev::settle(&mut [&mut carol, &mut alice], 2);
    let fresh = seal(&mut carol, space, Some(plan), PresenceState::Viewing, &[5]);
    assert_eq!(peek_presence(&fresh.envelope).unwrap().epoch, 2);
    assert!(open(&mut alice, &fresh).is_some());
    // Bob's frames stay refused after Carol's epoch moved on, and Bob reads no later frame either.
    dropped(&mut alice, &late, "not a current member");
    bob.cycle().unwrap();
    assert!(
        bob.core
            .presence_seal(space, Some(plan), PresenceState::Editing, &[])
            .unwrap()
            .is_none(),
        "Bob's space is an archive now"
    );
    assert!(open(&mut bob, &fresh).is_none(), "an archive hears nobody");

    // Nobody but Carol is present, and when she goes quiet the debounce returns to its default.
    alice.clock.advance(30_000);
    alice.core.presence_expire().unwrap();
    assert_eq!(
        alice.core.sync_push_debounce_ms().unwrap(),
        PUSH_DEBOUNCE_MS
    );
}

#[test]
fn a_member_who_leaves_is_not_present_any_more() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    let frame = seal(&mut bob, space, Some(plan), PresenceState::Viewing, &[]);
    assert!(open(&mut alice, &frame).is_some());
    alice.events();
    leave(&mut bob, feed);
    Dev::settle(&mut [&mut bob, &mut alice], 3);
    assert!(list(&mut alice, space).is_empty());
    assert_eq!(presence_changed(&alice.events(), space), 1);
}

#[test]
fn an_epoch_that_is_not_the_current_one_is_dropped() {
    let (
        Pair {
            mut alice,
            mut bob,
            space,
            feed,
            plan,
            ..
        },
        carol,
    ) = trio(MemberRole::Editor);
    // A frame Bob sealed under epoch 1, before a Rekey.
    let old = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[1]);
    assert_eq!(peek_presence(&old.envelope).unwrap().epoch, 1);
    // Alice removes Carol: the Rekey makes epoch 2, and Bob and Alice move to it.
    let carol_account = carol.account();
    remove_member(&mut alice, feed, carol_account);
    Dev::settle(&mut [&mut alice, &mut bob], 3);
    assert_eq!(alice.core.members_info(space).unwrap().epoch, 2);
    dropped(&mut alice, &old, "not the current epoch");
    let new = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[2]);
    assert_eq!(peek_presence(&new.envelope).unwrap().epoch, 2);
    assert!(open(&mut alice, &new).is_some());
    // The old frame stays refused after a newer one was accepted.
    dropped(&mut alice, &old, "not the current epoch");

    // An epoch nobody holds yet: a frame from the future.
    let (account, device) = ids(&bob);
    let future = PresenceHeader {
        feed,
        account_id: account,
        device_id: device,
        epoch: 5,
        counter: counter_of(&new) + 10,
    };
    let sk = bob.core.feed_sk(feed, 2).unwrap().unwrap();
    let bytes = seal_presence(
        &future,
        &PresenceBody {
            doc_id: KEYRING_DOC_ID,
            state: PresenceState::Idle,
            update: vec![],
        },
        &presence_key(&sk, feed),
        bob.core.signer(),
        &SeededRng::new(5),
    );
    dropped_bytes(&mut alice, feed, &bytes, "not the current epoch");
}

#[test]
fn a_frame_sealed_under_a_key_the_receiver_does_not_have_is_dropped() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    // Bob seals under another key than the space's, with his own device key and the right epoch.
    let (account, device) = ids(&bob);
    let header = PresenceHeader {
        feed,
        account_id: account,
        device_id: device,
        epoch: 1,
        counter: 50_000_000,
    };
    let wrong = seal_presence(
        &header,
        &PresenceBody {
            doc_id: DocId::for_page(plan),
            state: PresenceState::Editing,
            update: vec![1],
        },
        &Secret32::generate(&SeededRng::new(12)),
        bob.core.signer(),
        &SeededRng::new(13),
    );
    dropped_bytes(&mut alice, feed, &wrong, "signature or sealing");
    // The genuine one is accepted: a refused frame did not move the counter.
    let genuine = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[2]);
    assert!(open(&mut alice, &genuine).is_some());
}

#[test]
fn a_device_the_log_revoked_may_not_be_present_even_before_the_rekey() {
    let server = server();
    let (mut a1, kit) = Dev::first("a1", 1, &server);
    let (mut bob, _) = Dev::first("bob", 2, &server);
    a1.configure();
    bob.configure();
    let space = create_space(&mut a1.core, "Project").id;
    let plan = create_page(&mut a1.core, space, None, "Plan").id;
    a1.write(plan, "first");
    let feed = share(&mut a1, space, "Project", "Alice");
    add_member(&mut a1, &mut bob, feed, space, MemberRole::Editor, "Bob");
    a1.cycle().unwrap();
    // Alice's second device holds the space key through the personal keyring.
    let mut a2 = Dev::from_backup("a2", 4, &server, &mut a1, &kit);
    a2.configure();
    Dev::settle(&mut [&mut a2, &mut a1, &mut bob], 3);
    let before = seal(&mut a2, space, Some(plan), PresenceState::Editing, &[1]);
    let message =
        open(&mut bob, &before).expect("another device of a member is a device of a member");
    assert_eq!(message.info.account_id, a1.account());
    assert_eq!(message.info.name, "Alice");
    assert_eq!(message.info.device_id, a2.core.device.device_id);

    // The account revokes a2. Bob learns it from the account's log (the Rekey that follows has not
    // reached him: his feed is not pulled), and a2, which has not heard, goes on sending.
    let a2_device = a2.core.device.device_id;
    assert!(
        list(&mut bob, space)
            .iter()
            .any(|d| d.device_id == a2_device),
        "Bob lists a2 as present before the revocation"
    );
    bob.events();
    a1.core
        .dispatch(Request::DeviceRevoke {
            device_id: a2_device,
            passphrase: None,
        })
        .unwrap();
    a1.cycle().unwrap();
    bob.fetch_logs().unwrap();
    // The revocation takes a2 off Bob's list at once (not when its entry times out), and Bob's screens are told.
    assert!(
        !list(&mut bob, space)
            .iter()
            .any(|d| d.device_id == a2_device),
        "the revoked device is no longer listed once Bob has read the log"
    );
    assert_eq!(
        presence_changed(&bob.events(), space),
        1,
        "Bob is told that who is present changed"
    );
    let folded_epoch = |dev: &Dev| dev.core.shared.timelines.get(&feed).unwrap().state().epoch;
    assert_eq!(folded_epoch(&bob), 1, "the pull has not brought the Rekey");
    let after = seal(&mut a2, space, Some(plan), PresenceState::Editing, &[2]);
    dropped(&mut bob, &after, "not a device of the member");
    // Once Bob has the Rekey too, Alice's remaining device is heard and the revoked one is still not.
    Dev::settle(&mut [&mut a1, &mut bob], 3);
    assert_eq!(folded_epoch(&bob), 2);
    let from_a1 = seal(&mut a1, space, Some(plan), PresenceState::Viewing, &[3]);
    assert!(open(&mut bob, &from_a1).is_some());
    dropped(&mut bob, &after, "not the current epoch");
}

// ----- the timeout and the debounce hint ------------------------------------------------------------

#[test]
fn presence_times_out_after_thirty_seconds_and_the_debounce_follows_it() {
    let Pair {
        mut alice,
        mut bob,
        space,
        plan,
        ..
    } = pair(MemberRole::Editor);
    assert_eq!(
        alice.core.sync_push_debounce_ms().unwrap(),
        PUSH_DEBOUNCE_MS
    );
    assert_eq!(PUSH_DEBOUNCE_MS, 500);
    alice.events();

    let frame = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[1]);
    assert!(open(&mut alice, &frame).is_some());
    assert_eq!(list(&mut alice, space).len(), 1);
    assert_eq!(
        alice.core.sync_push_debounce_ms().unwrap(),
        PUSH_DEBOUNCE_PRESENCE_MS,
        "150 ms while somebody else is here"
    );
    assert_eq!(PUSH_DEBOUNCE_PRESENCE_MS, 150);
    alice.events();

    // One millisecond short of the timeout: still here, and expiring changes nothing.
    alice.clock.advance(29_999);
    alice.core.presence_expire().unwrap();
    assert_eq!(list(&mut alice, space).len(), 1);
    assert_eq!(alice.core.sync_push_debounce_ms().unwrap(), 150);
    assert_eq!(presence_changed(&alice.events(), space), 0);

    // At thirty seconds the list and the hint stop showing it, before and after the expiry ran.
    alice.clock.advance(1);
    assert!(list(&mut alice, space).is_empty());
    assert_eq!(alice.core.sync_push_debounce_ms().unwrap(), 500);
    alice.core.presence_expire().unwrap();
    assert_eq!(
        presence_changed(&alice.events(), space),
        1,
        "avatars refresh once"
    );
    alice.core.presence_expire().unwrap();
    assert_eq!(presence_changed(&alice.events(), space), 0, "and only once");
    assert_eq!(alice.core.shared.presence.held(), 0);

    // A newer frame brings the device back (and a replay of the old one does not).
    dropped(&mut alice, &frame, "counter not above the last accepted");
    let again = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[2]);
    assert!(open(&mut alice, &again).is_some());
    assert_eq!(list(&mut alice, space).len(), 1);
    assert_eq!(presence_changed(&alice.events(), space), 1);

    // A frame that arrives after the entry timed out, before any expiry ran, is a new appearance.
    alice.clock.advance(30_000);
    let late = seal(&mut bob, space, Some(plan), PresenceState::Editing, &[3]);
    assert!(open(&mut alice, &late).is_some());
    assert_eq!(presence_changed(&alice.events(), space), 1);
}

// ----- inline entries ---------------------------------------------------------------------------------

/// The item after `cursor` as the server relays it inline to a device that is at `cursor`.
fn next_inline(dev: &Dev, feed: FeedId, cursor: u64) -> Option<InlineEntry> {
    let page = dev.remote.changes_sync(feed, cursor, 1).unwrap();
    let (item, bytes) = page.changes.first()?.clone();
    Some(InlineEntry {
        feed,
        prev_cursor: cursor,
        cursor: item,
        chain_after: page.chain_hash_after,
        entry: bytes,
    })
}

fn cursor_of(dev: &Dev, feed: FeedId) -> u64 {
    dev.core.feed_row(feed).unwrap().cursor
}

/// Offers every item the feed holds beyond the device's cursor, one at a time, the way the socket
/// would relay them; the sum of what the applies did.
fn relay_all(dev: &mut Dev, feed: FeedId) -> kb_sync::v2::SyncApplied {
    let mut total = kb_sync::v2::SyncApplied::default();
    for _ in 0..1000 {
        let cursor = cursor_of(dev, feed);
        let Some(inline) = next_inline(dev, feed, cursor) else {
            break;
        };
        match dev.core.sync_feed_apply_inline(feed, &inline).unwrap() {
            InlineOutcome::Applied(report) => {
                total.applied += report.applied;
                total.skipped_own += report.skipped_own;
                total.pending += report.pending;
                total.rejected += report.rejected;
                total.new_cursor = report.new_cursor;
                assert_eq!(report.gaps, 0);
                assert_eq!(report.new_cursor, inline.cursor);
            }
            other => panic!("{}: {other:?} for the item at {}", dev.name, inline.cursor),
        }
    }
    total
}

#[test]
fn an_inline_entry_goes_through_the_apply_path_and_equals_a_pulled_one() {
    let (
        Pair {
            mut alice,
            mut bob,
            space,
            feed,
            plan,
            ..
        },
        mut carol,
    ) = trio(MemberRole::Editor);
    let doc = DocId::for_page(plan);
    let (_, bob_device) = ids(&bob);
    bob.write(plan, "typed on bob");
    bob.cycle().unwrap();
    assert_eq!(cursor_of(&alice, feed), cursor_of(&carol, feed));
    alice.events();

    // Alice gets it inline, Carol pulls it.
    let total = relay_all(&mut alice, feed);
    assert!(total.applied >= 1, "{total:?}");
    assert_eq!((total.rejected, total.pending), (0, 0), "{total:?}");
    carol.pull_step(feed, true).unwrap();

    assert!(alice.texts(plan).contains(&"typed on bob".to_owned()));
    assert_eq!(alice.texts(plan), carol.texts(plan));
    let (a, c) = (
        alice.core.feed_row(feed).unwrap(),
        carol.core.feed_row(feed).unwrap(),
    );
    assert_eq!(a.cursor, c.cursor);
    assert_eq!(a.chain_hash, c.chain_hash, "the same chain");
    assert!(a.chain_hash.is_some());
    assert_eq!(
        alice
            .core
            .db
            .sync_seen_get_in(feed, doc, bob_device)
            .unwrap(),
        carol
            .core
            .db
            .sync_seen_get_in(feed, doc, bob_device)
            .unwrap(),
        "the same bookkeeping"
    );
    // The page was stored, not just shown: it survives a restart of Alice's core.
    alice.reopen(31, alice.clock.now_ms());
    assert!(alice.texts(plan).contains(&"typed on bob".to_owned()));
    // The events a pull raises, an inline apply raises too (the UI refreshes the page).
    let events = {
        // A second round, so that the events of this one are read.
        bob.write(plan, "and a second line");
        bob.cycle().unwrap();
        relay_all(&mut alice, feed);
        alice.events()
    };
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::PageContentChanged { .. })),
        "{events:?}"
    );
    // The next ordinary cycle finds nothing left to pull and the feeds agree.
    alice.cycle().unwrap();
    assert_eq!(alice.last_report.map(|r| r.applied), Some(0));
    assert!(alice.texts(plan).contains(&"and a second line".to_owned()));
    let _ = space;
}

#[test]
fn a_duplicate_or_late_inline_entry_is_known_and_a_gap_answers_pull() {
    let Pair {
        mut alice,
        mut bob,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    bob.write(plan, "first from bob");
    bob.cycle().unwrap();
    let start = cursor_of(&alice, feed);
    let first = next_inline(&alice, feed, start).expect("an item");
    bob.write(plan, "second from bob");
    bob.cycle().unwrap();
    let second = next_inline(&alice, feed, first.cursor).expect("a later item");
    assert!(second.cursor > first.cursor);

    // The later one first: Alice is not at its `prev_cursor`. Nothing is stored, nothing moves.
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &second).unwrap(),
        InlineOutcome::Pull
    );
    assert_eq!(cursor_of(&alice, feed), start);
    assert!(!alice.texts(plan).contains(&"second from bob".to_owned()));
    assert!(alice.core.db.rejected_list(feed, 10).unwrap().is_empty());

    // The earlier one applies; both again are `Known` (a duplicate and a late frame).
    assert!(matches!(
        alice.core.sync_feed_apply_inline(feed, &first).unwrap(),
        InlineOutcome::Applied(_)
    ));
    assert_eq!(cursor_of(&alice, feed), first.cursor);
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &first).unwrap(),
        InlineOutcome::Known
    );
    // Now the second one is the next item and applies too.
    assert!(matches!(
        alice.core.sync_feed_apply_inline(feed, &second).unwrap(),
        InlineOutcome::Applied(_)
    ));
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &second).unwrap(),
        InlineOutcome::Known
    );
    let early = InlineEntry {
        cursor: 1,
        prev_cursor: 0,
        ..first
    };
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &early).unwrap(),
        InlineOutcome::Known,
        "a cursor at or below the feed's, whatever the bytes"
    );
    assert!(alice.texts(plan).contains(&"second from bob".to_owned()));
    // And the pull that comes after has nothing left.
    alice.cycle().unwrap();
    assert_eq!(alice.last_report.map(|r| r.applied), Some(0));
}

#[test]
fn an_inline_entry_of_this_device_is_counted_as_its_own() {
    let Pair {
        mut bob,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    bob.write(plan, "bob's own echo");
    // Bob appends and the socket relays his own entries back to him before any pull.
    let at = cursor_of(&bob, feed);
    bob.push_step(feed, true).unwrap();
    let total = relay_all(&mut bob, feed);
    assert!(total.skipped_own >= 1, "{total:?}");
    assert_eq!((total.applied, total.rejected), (0, 0), "{total:?}");
    assert!(cursor_of(&bob, feed) > at, "his cursor stays contiguous");
    bob.cycle().unwrap();
    assert_eq!(bob.last_report.map(|r| r.applied + r.skipped_own), Some(0));
}

#[test]
fn an_inline_entry_from_a_viewer_is_quarantined_like_a_pulled_one() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        plan,
        ..
    } = pair(MemberRole::Viewer);
    // A hostile Viewer writes past the request's refusal, and a server that accepts anything lets it in.
    bob.core.shared.force_push = true;
    bob.write(plan, "a viewer should not type this");
    server_adversary(&bob, Some(Adversary::AcceptAll));
    bob.cycle().unwrap();
    server_adversary(&bob, None);
    bob.core.shared.force_push = false;

    alice.events();
    let total = relay_all(&mut alice, feed);
    assert_eq!(total.rejected, 1, "{total:?}");
    assert_eq!(total.applied, 0, "{total:?}");
    assert!(
        !alice
            .texts(plan)
            .contains(&"a viewer should not type this".to_owned()),
        "the quarantined entry is not applied"
    );
    let rows = alice.core.db.rejected_list(feed, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account_id, bob.account());
    assert_eq!(rows[0].reason, kb_storage::RejectReason::RoleForbids);
    assert_eq!(rows[0].cursor, cursor_of(&alice, feed));
    assert!(
        alice
            .events()
            .contains(&Event::SpaceMembersChanged { space_id: space })
    );
    let info = alice.core.members_info(space).unwrap();
    assert_eq!(info.refused_total, 1);
    assert_eq!(
        info.members
            .iter()
            .find(|m| m.name == "Bob")
            .unwrap()
            .refused_changes,
        1
    );
    // The chain advanced past it, so the pull that follows finds it settled and counts nothing twice.
    alice.cycle().unwrap();
    assert_eq!(alice.core.members_info(space).unwrap().refused_total, 1);
    assert_eq!(alice.core.db.rejected_list(feed, 10).unwrap().len(), 1);
}

#[test]
fn a_chain_that_does_not_line_up_and_a_tampered_signature_answer_pull() {
    let Pair {
        mut alice,
        mut bob,
        feed,
        plan,
        ..
    } = pair(MemberRole::Editor);
    bob.write(plan, "a line to relay");
    bob.cycle().unwrap();
    let start = cursor_of(&alice, feed);
    let before = alice.core.feed_row(feed).unwrap();
    let inline = next_inline(&alice, feed, start).expect("an item");

    // The chain hash after the item is not the one the bytes lead to.
    let mut wrong_chain = inline.clone();
    wrong_chain.chain_after[0] ^= 1;
    assert_eq!(
        alice
            .core
            .sync_feed_apply_inline(feed, &wrong_chain)
            .unwrap(),
        InlineOutcome::Pull
    );
    // The bytes differ from the ones the chain covers.
    let mut wrong_bytes = inline.clone();
    let last = wrong_bytes.entry.len() - 1;
    wrong_bytes.entry[last] ^= 1;
    assert_eq!(
        alice
            .core
            .sync_feed_apply_inline(feed, &wrong_bytes)
            .unwrap(),
        InlineOutcome::Pull
    );
    // A tampered signature whose chain still lines up: the verifier refuses it, and the pull will say so.
    let mut tampered = wrong_bytes.clone();
    tampered.chain_after = chain_next(
        &before.chain_hash.expect("a chain after a pull"),
        &tampered.entry,
    );
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &tampered).unwrap(),
        InlineOutcome::Pull
    );
    // Too large, another feed and nothing known of the feed.
    let mut huge = inline.clone();
    huge.entry
        .resize(kb_types::sharing::INLINE_ENTRY_MAX_BYTES + 1, 0);
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &huge).unwrap(),
        InlineOutcome::Pull
    );
    let elsewhere = InlineEntry {
        feed: FeedId::from_bytes([0x44; 16]),
        ..inline.clone()
    };
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &elsewhere).unwrap(),
        InlineOutcome::Pull
    );
    assert_eq!(
        alice
            .core
            .sync_feed_apply_inline(elsewhere.feed, &elsewhere)
            .unwrap(),
        InlineOutcome::Pull
    );

    // None of it moved or stored anything, or flagged the server.
    let after = alice.core.feed_row(feed).unwrap();
    assert_eq!(
        (after.cursor, after.chain_hash),
        (before.cursor, before.chain_hash)
    );
    assert!(!after.fork);
    assert!(alice.core.db.rejected_list(feed, 10).unwrap().is_empty());
    assert!(!alice.texts(plan).contains(&"a line to relay".to_owned()));
    // The genuine entry still applies, and the ordinary pull has nothing more.
    assert!(matches!(
        alice.core.sync_feed_apply_inline(feed, &inline).unwrap(),
        InlineOutcome::Applied(_)
    ));
    assert!(alice.texts(plan).contains(&"a line to relay".to_owned()));
}

#[test]
fn membership_entries_and_feeds_not_read_yet_are_left_to_the_pull() {
    let Pair {
        mut alice,
        mut bob,
        space,
        feed,
        ..
    } = pair(MemberRole::Editor);
    // A membership entry (Bob becomes a Viewer) is relayed inline: the pull sees it first.
    let bob_account = bob.account();
    set_role(&mut alice, feed, bob_account, MemberRole::Viewer);
    alice.cycle().unwrap();
    let start = cursor_of(&bob, feed);
    let inline = next_inline(&bob, feed, start).expect("the RoleSet");
    assert_eq!(
        bob.core.sync_feed_apply_inline(feed, &inline).unwrap(),
        InlineOutcome::Pull
    );
    assert_eq!(cursor_of(&bob, feed), start);
    assert_eq!(role_of(&mut bob, space), Some(MemberRole::Editor));
    bob.cycle().unwrap();
    assert_eq!(role_of(&mut bob, space), Some(MemberRole::Viewer));

    // A feed that has not finished its first read (a device that has pulled nothing yet).
    let server = bob.remote.clone();
    let (mut fresh, _) = Dev::first("fresh", 9, &server);
    fresh.configure();
    let personal = fresh.core.ensure_personal_feed_row().unwrap().unwrap();
    assert_eq!(cursor_of(&fresh, personal), 0);
    let other = InlineEntry {
        feed: personal,
        prev_cursor: 0,
        cursor: 1,
        chain_after: [0; 32],
        entry: vec![1, 2, 3],
    };
    assert_eq!(
        fresh.core.sync_feed_apply_inline(personal, &other).unwrap(),
        InlineOutcome::Pull
    );
}

#[test]
fn an_inline_entry_waits_for_a_device_log_the_pull_fetches() {
    let (
        Pair {
            mut alice,
            mut bob,
            feed,
            plan,
            ..
        },
        carol,
    ) = trio(MemberRole::Editor);
    // Carol writes; Alice forgets Carol's device log (a log that is not held yet): the entry cannot be judged.
    let carol_account = carol.account();
    let mut carol = carol;
    carol.write(plan, "carol's line");
    carol.cycle().unwrap();
    alice.core.shared.logs.remove(&carol_account);
    let start = cursor_of(&alice, feed);
    let inline = next_inline(&alice, feed, start).expect("an item");
    assert_eq!(
        alice.core.sync_feed_apply_inline(feed, &inline).unwrap(),
        InlineOutcome::Pull
    );
    assert_eq!(cursor_of(&alice, feed), start, "nothing was stored");
    assert!(alice.core.db.rejected_list(feed, 10).unwrap().is_empty());
    // The pull asks for the log, gets it and carries on.
    alice.cycle().unwrap();
    assert!(alice.texts(plan).contains(&"carol's line".to_owned()));
    let _ = &mut bob;
}

/// What a scrambled schedule did: inline entries applied, answered `Pull`, answered `Known`, pulls.
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    applied: u32,
    pull: u32,
    known: u32,
    pulls: u32,
}

/// Brings a device to the head of the feed through a mix of inline entries (the next one, one that
/// skips ahead, a duplicate, a repeat) and now and then a whole pull, in an order the seed decides.
fn scramble(dev: &mut Dev, feed: FeedId, seed: u64) -> Tally {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut next_random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state >> 33
    };
    let mut tally = Tally::default();
    for _ in 0..500 {
        let cursor = cursor_of(dev, feed);
        let Some(next) = next_inline(dev, feed, cursor) else {
            break;
        };
        let offer = |dev: &mut Dev, inline: &InlineEntry, tally: &mut Tally| match dev
            .core
            .sync_feed_apply_inline(feed, inline)
            .unwrap()
        {
            InlineOutcome::Applied(_) => tally.applied += 1,
            InlineOutcome::Pull => tally.pull += 1,
            InlineOutcome::Known => tally.known += 1,
        };
        match next_random() % 9 {
            0..=3 => offer(dev, &next, &mut tally),
            4 | 5 => match next_inline(dev, feed, next.cursor) {
                // One item is skipped: this device is not at the frame's `prev_cursor`.
                Some(ahead) => {
                    offer(dev, &ahead, &mut tally);
                    assert_eq!(cursor_of(dev, feed), cursor, "a gap stores nothing");
                }
                None => offer(dev, &next, &mut tally),
            },
            6 => {
                // A duplicate of something already applied, or the next one twice.
                offer(dev, &next, &mut tally);
                let again = InlineEntry {
                    prev_cursor: cursor,
                    ..next.clone()
                };
                offer(dev, &again, &mut tally);
            }
            7 => {
                // A late frame: a cursor this device is past.
                let late = InlineEntry {
                    cursor,
                    prev_cursor: 0,
                    ..next.clone()
                };
                offer(dev, &late, &mut tally);
            }
            _ => {
                dev.pull_step(feed, true).unwrap();
                tally.pulls += 1;
            }
        }
    }
    dev.pull_step(feed, true).unwrap();
    tally
}

#[test]
fn any_mix_of_inline_and_pulled_entries_converges_to_the_state_of_pulling_alone() {
    let (
        Pair {
            mut alice,
            mut bob,
            space,
            feed,
            plan,
            notes,
            ..
        },
        mut carol,
    ) = trio(MemberRole::Editor);
    let server = alice.remote.clone();
    let (mut dave, _) = Dev::first("dave", 4, &server);
    dave.configure();
    add_member(
        &mut alice,
        &mut dave,
        feed,
        space,
        MemberRole::Editor,
        "Dave",
    );
    Dev::settle(&mut [&mut alice, &mut bob, &mut carol, &mut dave], 3);

    // Bob types in rounds across two pages, a cycle each: a run of feed items.
    for round in 0..8 {
        let page = if round % 2 == 0 { plan } else { notes };
        bob.write(page, &format!("bob round {round}"));
        bob.cycle().unwrap();
    }
    // Bob's own state is what pulling alone gives: he reads everything back through his pull.
    bob.cycle().unwrap();
    let reference = bob.core.feed_row(feed).unwrap();
    assert!(reference.cursor > cursor_of(&alice, feed) + 6);

    let mut total = Tally::default();
    for (seed, dev) in [(1, &mut alice), (2, &mut carol), (3, &mut dave)] {
        let tally = scramble(dev, feed, seed);
        total.applied += tally.applied;
        total.pull += tally.pull;
        total.known += tally.known;
        total.pulls += tally.pulls;
        let row = dev.core.feed_row(feed).unwrap();
        assert_eq!(row.cursor, reference.cursor, "{}: {tally:?}", dev.name);
        assert_eq!(row.chain_hash, reference.chain_hash, "{}", dev.name);
        for page in [plan, notes] {
            assert_eq!(dev.texts(page), bob.texts(page), "{}", dev.name);
        }
        assert!(dev.core.db.rejected_list(feed, 10).unwrap().is_empty());
        assert_eq!(dev.core.members_info(space).unwrap().refused_total, 0);
    }
    // The schedules really mixed the kinds of answer.
    assert!(
        total.applied > 0 && total.pull > 0 && total.known > 0,
        "{total:?}"
    );
    // Nothing is left for a normal cycle either.
    for dev in [&mut alice, &mut carol, &mut dave] {
        dev.cycle().unwrap();
        assert_eq!(dev.last_report.map(|r| r.applied), Some(0), "{}", dev.name);
    }
}
