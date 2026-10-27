//! The view-link step of the driver (M13 slice 4), against the scripted host and remote of the parent
//! module: it runs after the feeds, each answer of the server becomes the outcome the core is told, a
//! failure that is not the link's ends the cycle without a report, the budget and a forked personal feed.
//! The reference remote with a real core is in `tests/links.rs`.

use kb_sync::SyncError;
use kb_sync::v2::{LinkOutcome, LinkPut, LinkWork};
use kb_types::LinkId;
use kb_types::sync::TRANSFER_BUDGET;

use super::*;

fn link(n: u8) -> LinkId {
    LinkId::from_bytes([n; 16])
}

fn put(n: u8) -> LinkWork {
    LinkWork::Put {
        link_id: link(n),
        put: LinkPut {
            feed: personal(),
            expires_at: None,
            chunks: Vec::new(),
            publication: vec![n],
        },
    }
}

fn delete(n: u8) -> LinkWork {
    LinkWork::Delete { link_id: link(n) }
}

fn told(rig: &Rig) -> Vec<(LinkId, LinkOutcome)> {
    rig.host.with(|data| data.link_done.clone())
}

#[tokio::test(start_paused = true)]
async fn the_link_step_follows_the_feeds_and_each_answer_becomes_an_outcome() {
    let mut rig = Rig::new(vec![personal_work(), shared_work(shared_a())]);
    rig.host
        .with(|data| data.link_work = vec![put(1), put(2), delete(3), delete(4)]);
    // The second upload is refused, the first deletion finds the link gone, the second is refused.
    rig.remote.fail_nth("link_put", 1, SyncError::Forbidden);
    rig.remote.fail("link_delete", SyncError::NotFound);
    rig.remote.fail("link_delete", SyncError::Forbidden);
    rig.one_cycle().await;

    assert_eq!(
        told(&rig),
        vec![
            (link(1), LinkOutcome::Published { pub_seq: 7 }),
            (link(2), LinkOutcome::Refused),
            (link(3), LinkOutcome::Deleted),
            (link(4), LinkOutcome::Refused),
        ]
    );
    // After the last feed's step (its head), before the cycle ends; the core is asked once.
    let calls = rig.calls(&["put_head", "sync_link_work", "link_put", "link_delete"]);
    let start = calls
        .iter()
        .position(|call| call == "h:sync_link_work")
        .unwrap();
    assert!(
        calls[..start]
            .iter()
            .all(|call| call.starts_with("r:put_head"))
    );
    assert_eq!(
        calls.len() - start,
        5,
        "the work, two uploads and two deletions: {calls:?}"
    );
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn nothing_due_means_no_link_call_and_a_quota_refusal_of_an_upload_is_a_refusal() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_link_work"), 1);
    assert!(
        rig.calls(&["link_put", "link_delete", "sync_link_done"])
            .is_empty()
    );

    // A publication over the server's size limit does not shrink by being sent again.
    rig.host.with(|data| data.link_work = vec![put(1)]);
    rig.remote.fail("link_put", SyncError::Quota);
    rig.host.emit(&Event::SyncRequested);
    rig.until_cycles(2).await;
    assert_eq!(told(&rig), vec![(link(1), LinkOutcome::Refused)]);
    assert_eq!(
        rig.host.last_state().0,
        SyncState::Idle,
        "not the account's quota"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failure_that_is_not_the_links_is_not_reported_and_the_work_comes_back() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.link_work = vec![put(1)]);
    rig.remote
        .fail("link_put", SyncError::Unavailable("down".to_owned()));
    rig.start();
    rig.until_cycles(1).await;
    assert_eq!(rig.host.last_state().0, SyncState::Offline);
    assert!(told(&rig).is_empty(), "nothing was reported");
    // The backoff retry brings the same work, and now it goes through.
    rig.until_cycles(2).await;
    assert_eq!(
        told(&rig),
        vec![(link(1), LinkOutcome::Published { pub_seq: 7 })]
    );
    assert_eq!(rig.host.last_state().0, SyncState::Idle);
}

#[tokio::test(start_paused = true)]
async fn a_protocol_failure_goes_to_the_cycles_problems_and_the_pieces_after_it_still_run() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.link_work = vec![put(1), put(2)]);
    rig.remote
        .fail("link_put", SyncError::Protocol("odd".to_owned()));
    rig.one_cycle().await;
    assert_eq!(
        told(&rig),
        vec![(link(2), LinkOutcome::Published { pub_seq: 7 })],
        "the first is left for the next cycle"
    );
    let (state, message) = rig.host.last_state();
    assert_eq!(state, SyncState::Error);
    assert!(message.is_some_and(|message| message.contains("odd")));
}

#[tokio::test(start_paused = true)]
async fn a_refused_session_authenticates_again_for_a_link_call() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| data.link_work = vec![put(1)]);
    rig.remote.fail("link_put", SyncError::Unauthorized);
    rig.one_cycle().await;
    assert_eq!(
        told(&rig),
        vec![(link(1), LinkOutcome::Published { pub_seq: 7 })]
    );
    assert_eq!(rig.count("r:link_put"), 2, "the retry is the second call");
}

#[tokio::test(start_paused = true)]
async fn the_link_step_has_a_budget_and_asks_for_another_cycle() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.link_work = (1..=TRANSFER_BUDGET as u8 + 2).map(put).collect();
    });
    rig.start();
    // More is waiting after the first cycle: the next follows without the pull interval.
    rig.until_cycles(2).await;
    assert_eq!(told(&rig).len(), TRANSFER_BUDGET + 2);
    let calls = rig.calls(&["sync_link_work", "link_put"]);
    let second = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| *call == "h:sync_link_work")
        .map(|(at, _)| at)
        .nth(1)
        .unwrap();
    assert_eq!(
        second - 1,
        TRANSFER_BUDGET,
        "the first cycle sent a budget's worth: {calls:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_forked_personal_feed_pauses_the_link_step() {
    let mut rig = Rig::new(vec![personal_work()]);
    rig.host.with(|data| {
        data.fork_detected = true;
        data.link_work = vec![put(1)];
    });
    rig.one_cycle().await;
    assert_eq!(rig.count("h:sync_link_work"), 0);
    assert!(rig.calls(&["link_put", "link_delete"]).is_empty());
}
