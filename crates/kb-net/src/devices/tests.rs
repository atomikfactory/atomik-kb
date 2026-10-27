//! The pairing and recovery tasks against the reference `MemoryRemote` (protocol v2: its mailbox,
//! its recovery store and a real account log from the `kb-sync` conformance kit),
//! through fake hosts that behave like the core's state machines. Tokio's clock is
//! paused, so retries and quiet polls cost nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kb_sync::v2::MemoryRemote;
use kb_sync::v2::conformance::Person;
use kb_types::testing::SeededRng;
use kb_types::{
    AccountId, ApiError, DeviceId, Event, PairingInfo, PairingRole, PairingState, RecoveryInfo,
    RecoveryState,
};

use super::{
    ACCOUNT_NOT_ON_SERVER, CODE_EXPIRED, DeviceTasks, KIT_UNKNOWN, PEER_LEFT, PairingHost,
    PairingTask, RecoveryHost, RecoveryTask, TaskHandle,
};
use crate::driver::RemoteFactory;

const URL: &str = "memory://";
const FIXTURE_AT_MS: i64 = 1_790_000_000_000;

/// The account of a test: a person whose log holds the genesis and one `DeviceAdded` (so a sponsor
/// has an entry to publish), and a valid entry that would follow them (what a recovering device
/// appends).
struct Fixture {
    person: Person,
    account_id: AccountId,
    invite_token: String,
    next: Vec<u8>,
}

impl Fixture {
    fn generate(seed: u64) -> Self {
        let mut person = Person::generate(seed, "Account", FIXTURE_AT_MS);
        person.add_device(FIXTURE_AT_MS + 1);
        let mut later = person.clone();
        later.revoke_device(1, FIXTURE_AT_MS + 2);
        let next = later.encoded_log_after(person.log_seq()).remove(0);
        Self {
            account_id: person.account,
            person,
            invite_token: kb_sync::v2::memory::DEFAULT_INVITE_TOKEN.to_owned(),
            next,
        }
    }

    fn encoded_log(&self) -> Vec<Vec<u8>> {
        self.person.encoded_log()
    }

    fn revocation(&self) -> Vec<u8> {
        self.next.clone()
    }
}

type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Default)]
struct Listeners {
    next: AtomicU64,
    entries: Mutex<Vec<(u64, Listener)>>,
}

impl Listeners {
    fn add(
        self: &Arc<Self>,
        on_event: Box<dyn Fn(&Event) + Send + Sync>,
    ) -> Box<dyn FnOnce() + Send> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.entries.lock().unwrap().push((id, Arc::from(on_event)));
        let listeners = Arc::clone(self);
        Box::new(move || {
            listeners
                .entries
                .lock()
                .unwrap()
                .retain(|(other, _)| *other != id)
        })
    }

    fn emit(&self, event: &Event) {
        let listeners: Vec<Listener> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(event);
        }
    }

    fn count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// Lets the paused clock run until `condition` holds.
async fn until(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..200_000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("never happened: {what}");
}

fn factory(remote: &MemoryRemote) -> RemoteFactory<MemoryRemote> {
    // Each task gets its own connection to the shared server.
    let remote = remote.clone();
    Arc::new(move |url: &str| {
        assert_eq!(url, URL);
        Ok(Arc::new(remote.clone()))
    })
}

fn server() -> MemoryRemote {
    MemoryRemote::new(&SeededRng::new(5))
}

// ----- a core-like pairing host ---------------------------------------------------------------------

/// What the fake core holds of a pairing session.
struct PairingData {
    role: PairingRole,
    state: PairingState,
    server_url: Option<String>,
    mailbox: Option<[u8; 6]>,
    expires_at: Option<i64>,
    outbox: Vec<(u32, Vec<u8>)>,
    next_out: u32,
    received: Vec<(u32, PairingRole, Vec<u8>)>,
    posted: Vec<u32>,
    error: Option<String>,
    peer_name: Option<String>,
    /// The sponsor's device log, what of it is published, and the entry it appends for
    /// the joiner when the user confirms.
    log: Vec<Vec<u8>>,
    published: u64,
    log_published: Vec<u64>,
    device_added: Vec<u8>,
    account: Option<AccountId>,
    bundle_seq: Option<u32>,
    /// When the bundle was posted, was the whole log published?
    log_before_bundle: Option<bool>,
    /// A method that fails once, and how; and whether the core then ends the session.
    fail: Option<(&'static str, ApiError, bool)>,
    calls: Vec<&'static str>,
}

impl PairingData {
    fn joiner() -> Self {
        Self {
            role: PairingRole::Joiner,
            state: PairingState::Starting,
            outbox: vec![(0, b"offer".to_vec())],
            next_out: 1,
            ..Self::sponsor(None)
        }
    }

    fn sponsor(mailbox: Option<[u8; 6]>) -> Self {
        Self {
            role: PairingRole::Sponsor,
            state: PairingState::Starting,
            server_url: Some(URL.to_owned()),
            mailbox,
            expires_at: None,
            outbox: Vec::new(),
            next_out: 0,
            received: Vec::new(),
            posted: Vec::new(),
            error: None,
            peer_name: None,
            log: Vec::new(),
            published: 0,
            log_published: Vec::new(),
            device_added: Vec::new(),
            account: None,
            bundle_seq: None,
            log_before_bundle: None,
            fail: None,
            calls: Vec::new(),
        }
    }

    fn push_out(&mut self, bytes: &[u8]) -> u32 {
        let seq = self.next_out;
        self.next_out += 1;
        self.outbox.push((seq, bytes.to_vec()));
        seq
    }
}

#[derive(Default)]
struct FakePairing {
    data: Mutex<Option<PairingData>>,
    listeners: Arc<Listeners>,
}

impl FakePairing {
    fn new(data: PairingData) -> Arc<Self> {
        Arc::new(Self {
            data: Mutex::new(Some(data)),
            listeners: Arc::default(),
        })
    }

    fn with<T>(&self, look: impl FnOnce(&mut PairingData) -> T) -> T {
        look(self.data.lock().unwrap().as_mut().unwrap())
    }

    /// One core call: recorded, maybe failing, then `answer`; `PairingChanged` follows a
    /// change, as the core emits it.
    fn call<T>(
        &self,
        name: &'static str,
        changes: bool,
        answer: impl FnOnce(&mut PairingData) -> T,
    ) -> Result<T, ApiError> {
        let result = self.with(|data| {
            data.calls.push(name);
            if let Some((failing, error, ends)) = data.fail.clone()
                && failing == name
            {
                data.fail = None;
                if ends {
                    data.state = PairingState::Failed;
                    data.error = Some(error.message.clone());
                }
                return Err(error);
            }
            Ok(answer(data))
        });
        if changes {
            self.listeners.emit(&Event::PairingChanged);
        }
        result
    }

    fn state(&self) -> PairingState {
        self.with(|data| data.state)
    }

    /// The user compared the digits and confirmed.
    fn confirm(&self) {
        self.with(|data| match data.role {
            PairingRole::Joiner => {
                data.state = PairingState::PeerConfirming;
                data.push_out(b"identity");
            }
            PairingRole::Sponsor => {
                let added = data.device_added.clone();
                data.log.push(added);
                let seq = data.push_out(b"bundle");
                data.bundle_seq = Some(seq);
                data.state = PairingState::Transferring;
            }
        });
        self.listeners.emit(&Event::PairingChanged);
    }

    fn cancel(&self) {
        self.with(|data| data.state = PairingState::Cancelled);
        self.listeners.emit(&Event::PairingChanged);
    }
}

impl PairingHost for FakePairing {
    fn pairing_info(&self) -> Result<PairingInfo, ApiError> {
        self.call("pairing_info", false, |data| PairingInfo {
            role: data.role,
            state: data.state,
            code: None,
            sas: None,
            peer_name: data.peer_name.clone(),
            server_url: data.server_url.clone(),
            error: data.error.clone(),
            expires_at: data.expires_at,
        })
    }

    fn pairing_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError> {
        self.call("pairing_mailbox_id", false, |data| data.mailbox)
    }

    fn pairing_mailbox_opened(&self, mailbox_id: [u8; 6], expires_at: i64) -> Result<(), ApiError> {
        self.call("pairing_mailbox_opened", true, |data| {
            data.mailbox = Some(mailbox_id);
            data.expires_at = Some(expires_at);
            if data.role == PairingRole::Joiner {
                data.state = PairingState::Waiting;
            }
        })
    }

    fn pairing_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        self.call("pairing_outbox", false, |data| data.outbox.clone())
    }

    fn pairing_posted(&self, seq: u32) -> Result<(), ApiError> {
        self.call("pairing_posted", true, |data| {
            data.outbox.retain(|(other, _)| *other != seq);
            data.posted.push(seq);
            if data.bundle_seq == Some(seq) {
                data.log_before_bundle = Some(data.published == data.log.len() as u64);
                data.state = PairingState::Done;
            }
        })
    }

    fn pairing_receive(&self, seq: u32, role: PairingRole, bytes: &[u8]) -> Result<(), ApiError> {
        self.call("pairing_receive", true, |data| {
            data.received.push((seq, role, bytes.to_vec()));
            match (data.role, bytes) {
                // Commit then reveal: the joiner opens its commitment once it has the
                // sponsor's answer; both show the digits from then on.
                (PairingRole::Joiner, b"answer") => {
                    data.push_out(b"reveal");
                    data.state = PairingState::Confirming;
                }
                (PairingRole::Joiner, b"bundle") => data.state = PairingState::Done,
                (PairingRole::Sponsor, b"offer") => {
                    data.push_out(b"answer");
                }
                (PairingRole::Sponsor, b"reveal") => data.state = PairingState::Confirming,
                (PairingRole::Sponsor, b"identity") => {
                    data.peer_name = Some("New laptop".to_owned());
                }
                _ => {}
            }
        })
    }

    fn pairing_failed(&self, message: &str) -> Result<(), ApiError> {
        self.call("pairing_failed", true, |data| {
            data.state = PairingState::Failed;
            data.error = Some(message.to_owned());
        })
    }

    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        self.call("sync_identity", false, |data| {
            data.account
                .map(|account| (account, DeviceId::from_bytes([3; 16])))
        })
    }

    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.call("sync_account_log", false, |data| {
            (data.log.clone(), data.published)
        })
    }

    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.call("sync_log_published", false, |data| {
            data.published = up_to_seq + 1;
            data.log_published.push(up_to_seq);
        })
    }

    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.listeners.add(on_event)
    }
}

/// A server with the fixture's account (genesis only: the sponsor publishes the
/// `DeviceAdded` of the fixture's second device) and a sponsor host for it.
fn sponsor_account(remote: &MemoryRemote, fixture: &Fixture) -> PairingData {
    let log = fixture.encoded_log();
    remote
        .create_account_sync(&fixture.invite_token, &log[..1])
        .unwrap();
    PairingData {
        log: log[..1].to_vec(),
        published: 1,
        device_added: log[1].clone(),
        account: Some(fixture.account_id),
        ..PairingData::sponsor(None)
    }
}

fn spawn_pairing(host: &Arc<FakePairing>, remote: &MemoryRemote) -> TaskHandle {
    PairingTask::spawn(
        Arc::clone(host) as Arc<dyn PairingHost>,
        factory(remote),
        &tokio::runtime::Handle::current(),
    )
}

// ----- pairing --------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_joiner_and_a_sponsor_pair_through_the_mailbox() {
    let remote = server();
    let fixture = Fixture::generate(41);
    let joiner = FakePairing::new(PairingData::joiner());
    let sponsor = FakePairing::new(sponsor_account(&remote, &fixture));

    let joining = spawn_pairing(&joiner, &remote);
    until("the joiner's mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;
    // The code the joiner shows is typed on the sponsor: the core decodes the mailbox.
    let mailbox = joiner.with(|data| data.mailbox);
    sponsor.with(|data| data.mailbox = mailbox);
    let sponsoring = spawn_pairing(&sponsor, &remote);

    until("both show the digits", || {
        joiner.state() == PairingState::Confirming && sponsor.state() == PairingState::Confirming
    })
    .await;
    joiner.confirm();
    until("the sponsor knows the joiner's name", || {
        sponsor.with(|data| data.peer_name.is_some())
    })
    .await;
    sponsor.confirm();
    until("both are done", || {
        joiner.state() == PairingState::Done && sponsor.state() == PairingState::Done
    })
    .await;
    until("both tasks ended", || {
        !joining.is_running() && !sponsoring.is_running()
    })
    .await;

    // Mailbox order: offer, answer, reveal, identity, bundle.
    joiner.with(|data| {
        assert_eq!(data.posted, [0, 1, 2], "offer, reveal, identity");
        let seen: Vec<(u32, &[u8])> = data
            .received
            .iter()
            .map(|(seq, role, bytes)| {
                assert_eq!(*role, PairingRole::Sponsor);
                (*seq, bytes.as_slice())
            })
            .collect();
        assert_eq!(seen, [(1, &b"answer"[..]), (4, &b"bundle"[..])]);
        assert!(data.expires_at.is_some());
    });
    sponsor.with(|data| {
        let seen: Vec<(u32, &[u8])> = data
            .received
            .iter()
            .map(|(seq, role, bytes)| {
                assert_eq!(*role, PairingRole::Joiner);
                (*seq, bytes.as_slice())
            })
            .collect();
        assert_eq!(
            seen,
            [
                (0, &b"offer"[..]),
                (2, &b"reveal"[..]),
                (3, &b"identity"[..])
            ]
        );
        assert_eq!(
            data.expires_at,
            joiner.with(|j| j.expires_at),
            "the sponsor learns the expiry"
        );
        // The joiner's DeviceAdded reached the server before the bundle left.
        assert_eq!(data.log_published, [1]);
        assert_eq!(data.log_before_bundle, Some(true));
    });
    assert_eq!(
        remote
            .fetch_log_sync(fixture.account_id, None)
            .unwrap()
            .len(),
        2
    );
    // The sponsor closed the mailbox; its messages stay readable until it expires.
    let page = remote.pair_messages_sync(mailbox.unwrap(), 0, 0).unwrap();
    assert!(page.closed);
    assert_eq!(page.messages.len(), 5);
    // Both tasks let go of their subscriptions.
    assert_eq!(joiner.listeners.count(), 0);
    assert_eq!(sponsor.listeners.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn an_expired_mailbox_ends_the_session_with_the_code_expired() {
    let remote = server();
    let joiner = FakePairing::new(PairingData::joiner());
    let task = spawn_pairing(&joiner, &remote);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;

    // Eleven minutes later the server has forgotten it.
    remote.set_now_ms(11 * 60 * 1000);
    until("the session failed", || {
        joiner.state() == PairingState::Failed
    })
    .await;
    assert_eq!(
        joiner.with(|data| data.error.clone()),
        Some(CODE_EXPIRED.to_owned())
    );
    until("the task ended", || !task.is_running()).await;
}

#[tokio::test(start_paused = true)]
async fn a_mailbox_the_other_side_closed_ends_the_session() {
    let remote = server();
    let joiner = FakePairing::new(PairingData::joiner());
    let _task = spawn_pairing(&joiner, &remote);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;

    let mailbox = joiner.with(|data| data.mailbox).unwrap();
    remote.pair_close_sync(mailbox).unwrap();
    until("the session failed", || {
        joiner.state() == PairingState::Failed
    })
    .await;
    assert_eq!(
        joiner.with(|data| data.error.clone()),
        Some(PEER_LEFT.to_owned())
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_closes_the_mailbox_and_ends_the_task() {
    let remote = server();
    let joiner = FakePairing::new(PairingData::joiner());
    let task = spawn_pairing(&joiner, &remote);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;

    joiner.cancel();
    until("the task ended", || !task.is_running()).await;
    let mailbox = joiner.with(|data| data.mailbox).unwrap();
    assert!(remote.pair_messages_sync(mailbox, 0, 0).unwrap().closed);
    // Cancelled stays cancelled: the task does not report a failure on top.
    assert_eq!(joiner.state(), PairingState::Cancelled);
    assert!(!joiner.with(|data| data.calls.contains(&"pairing_failed")));
}

#[tokio::test(start_paused = true)]
async fn a_server_that_stays_down_fails_the_session_after_three_attempts() {
    let remote = server();
    remote.set_offline(true);
    let joiner = FakePairing::new(PairingData::joiner());
    let task = spawn_pairing(&joiner, &remote);
    until("the task ended", || !task.is_running()).await;
    let (state, error) = joiner.with(|data| (data.state, data.error.clone()));
    assert_eq!(state, PairingState::Failed);
    assert!(error.is_some_and(|message| !message.is_empty()));
    assert!(joiner.with(|data| data.mailbox.is_none()));
}

#[tokio::test(start_paused = true)]
async fn a_short_outage_is_retried() {
    let remote = server();
    remote.set_offline(true);
    let joiner = FakePairing::new(PairingData::joiner());
    let _task = spawn_pairing(&joiner, &remote);
    // Back before the third attempt (1 s, then 2 s later).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    remote.set_offline(false);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_message_the_core_refuses_ends_the_session_as_the_core_says() {
    let remote = server();
    let fixture = Fixture::generate(42);
    let joiner = FakePairing::new(PairingData::joiner());
    joiner.with(|data| {
        data.fail = Some((
            "pairing_receive",
            ApiError::crypto("the other device could not be verified"),
            true,
        ));
    });
    let sponsor = FakePairing::new(sponsor_account(&remote, &fixture));
    let joining = spawn_pairing(&joiner, &remote);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;
    let mailbox = joiner.with(|data| data.mailbox);
    sponsor.with(|data| data.mailbox = mailbox);
    let _sponsoring = spawn_pairing(&sponsor, &remote);

    until("the joiner's task ended", || !joining.is_running()).await;
    assert_eq!(
        joiner.with(|data| data.error.clone()),
        Some("the other device could not be verified".to_owned())
    );
    assert!(!joiner.with(|data| data.calls.contains(&"pairing_failed")));
    assert!(
        remote
            .pair_messages_sync(mailbox.unwrap(), 0, 0)
            .unwrap()
            .closed
    );
}

#[tokio::test(start_paused = true)]
async fn a_locked_core_stops_the_task_quietly() {
    let remote = server();
    let joiner = FakePairing::new(PairingData::joiner());
    joiner.with(|data| data.fail = Some(("pairing_mailbox_opened", ApiError::locked(), false)));
    let task = spawn_pairing(&joiner, &remote);
    until("the task ended", || !task.is_running()).await;
    assert!(!joiner.with(|data| data.calls.contains(&"pairing_failed")));
}

#[tokio::test(start_paused = true)]
async fn a_sponsor_whose_account_is_not_on_the_server_cannot_hand_over() {
    let remote = server();
    let fixture = Fixture::generate(43);
    let joiner = FakePairing::new(PairingData::joiner());
    // The sponsor's account was never created on this server.
    let log = fixture.encoded_log();
    let sponsor = FakePairing::new(PairingData {
        log: log[..1].to_vec(),
        published: 1,
        device_added: log[1].clone(),
        account: Some(fixture.account_id),
        ..PairingData::sponsor(None)
    });
    let _joining = spawn_pairing(&joiner, &remote);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;
    let mailbox = joiner.with(|data| data.mailbox);
    sponsor.with(|data| data.mailbox = mailbox);
    let _sponsoring = spawn_pairing(&sponsor, &remote);
    until("both show the digits", || {
        sponsor.state() == PairingState::Confirming
    })
    .await;
    joiner.confirm();
    until("the identity arrived", || {
        sponsor.with(|data| data.peer_name.is_some())
    })
    .await;
    sponsor.confirm();

    until("the sponsor failed", || {
        sponsor.state() == PairingState::Failed
    })
    .await;
    sponsor.with(|data| {
        assert_eq!(data.error.as_deref(), Some(ACCOUNT_NOT_ON_SERVER));
        assert!(
            data.bundle_seq
                .is_some_and(|seq| !data.posted.contains(&seq)),
            "no bundle left"
        );
    });
}

#[tokio::test(start_paused = true)]
async fn a_sponsor_without_a_mailbox_fails() {
    let remote = server();
    let sponsor = FakePairing::new(PairingData::sponsor(None));
    let task = spawn_pairing(&sponsor, &remote);
    until("the task ended", || !task.is_running()).await;
    assert_eq!(sponsor.state(), PairingState::Failed);
}

#[tokio::test(start_paused = true)]
async fn a_task_for_a_session_that_is_over_does_nothing() {
    let remote = server();
    let joiner = FakePairing::new(PairingData {
        state: PairingState::Done,
        ..PairingData::joiner()
    });
    let task = spawn_pairing(&joiner, &remote);
    until("the task ended", || !task.is_running()).await;
    assert_eq!(joiner.with(|data| data.calls.clone()), ["pairing_info"]);
}

#[tokio::test(start_paused = true)]
async fn stopping_the_handle_stops_the_task_and_its_subscription() {
    let remote = server();
    let joiner = FakePairing::new(PairingData::joiner());
    let task = spawn_pairing(&joiner, &remote);
    until("the mailbox is open", || {
        joiner.state() == PairingState::Waiting
    })
    .await;
    assert_eq!(joiner.listeners.count(), 1);
    task.stop();
    until("unsubscribed", || joiner.listeners.count() == 0).await;
    let calls = joiner.with(|data| data.calls.len());
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(
        joiner.with(|data| data.calls.len()),
        calls,
        "no call after the stop"
    );
}

// ----- recovery ---------------------------------------------------------------------------------------

struct RecoveryData {
    state: RecoveryState,
    server_url: Option<String>,
    id: [u8; 32],
    blob: Option<Vec<u8>>,
    account: Option<AccountId>,
    account_from_blob: AccountId,
    log_seen: Option<Vec<Vec<u8>>>,
    entry: Vec<u8>,
    error: Option<String>,
    calls: Vec<&'static str>,
}

struct FakeRecovery {
    data: Mutex<RecoveryData>,
}

impl FakeRecovery {
    fn with<T>(&self, look: impl FnOnce(&mut RecoveryData) -> T) -> T {
        look(&mut self.data.lock().unwrap())
    }

    fn call<T>(
        &self,
        name: &'static str,
        answer: impl FnOnce(&mut RecoveryData) -> T,
    ) -> Result<T, ApiError> {
        Ok(self.with(|data| {
            data.calls.push(name);
            answer(data)
        }))
    }
}

impl RecoveryHost for FakeRecovery {
    fn recovery_info(&self) -> Result<RecoveryInfo, ApiError> {
        self.call("recovery_info", |data| RecoveryInfo {
            state: data.state,
            server_url: data.server_url.clone(),
            error: data.error.clone(),
        })
    }
    fn recovery_id(&self) -> Result<Option<[u8; 32]>, ApiError> {
        self.call("recovery_id", |data| Some(data.id))
    }
    fn recovery_blob_received(&self, bytes: &[u8]) -> Result<(), ApiError> {
        self.call("recovery_blob_received", |data| {
            data.blob = Some(bytes.to_vec());
            data.account = Some(data.account_from_blob);
        })
    }
    fn recovery_log_received(&self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError> {
        self.call("recovery_log_received", |data| {
            data.log_seen = Some(entries.to_vec());
            data.state = RecoveryState::Joining;
            data.entry.clone()
        })
    }
    fn recovery_entry_published(&self) -> Result<(), ApiError> {
        self.call("recovery_entry_published", |data| {
            data.state = RecoveryState::Done
        })
    }
    fn recovery_failed(&self, message: &str) -> Result<(), ApiError> {
        self.call("recovery_failed", |data| {
            data.state = RecoveryState::Failed;
            data.error = Some(message.to_owned());
        })
    }
    fn recovery_account_id(&self) -> Result<Option<AccountId>, ApiError> {
        self.call("recovery_account_id", |data| data.account)
    }
}

/// A server holding the fixture's account (genesis and one device) and a recovery blob
/// the first device uploaded; a fresh device's host that knows the kit's lookup id.
fn recovery_setup(
    remote: &MemoryRemote,
    fixture: &Fixture,
    id: [u8; 32],
    blob: &[u8],
) -> Arc<FakeRecovery> {
    let log = fixture.encoded_log();
    remote
        .create_account_sync(&fixture.invite_token, &log)
        .unwrap();
    let device = remote.clone();
    device
        .authenticate_sync(
            fixture.account_id,
            fixture.person.device_id(0),
            &fixture.person.signer(0),
        )
        .unwrap();
    device.recovery_put_sync(id, blob).unwrap();
    Arc::new(FakeRecovery {
        data: Mutex::new(RecoveryData {
            state: RecoveryState::Fetching,
            server_url: Some(URL.to_owned()),
            id,
            blob: None,
            account: None,
            account_from_blob: fixture.account_id,
            log_seen: None,
            // A valid next entry for the fixture's log (the core would sign its own
            // `DeviceAdded` with the recovery key).
            entry: fixture.revocation(),
            error: None,
            calls: Vec::new(),
        }),
    })
}

fn spawn_recovery(host: &Arc<FakeRecovery>, remote: &MemoryRemote) -> TaskHandle {
    RecoveryTask::spawn(
        Arc::clone(host) as Arc<dyn RecoveryHost>,
        factory(remote),
        &tokio::runtime::Handle::current(),
    )
}

#[tokio::test(start_paused = true)]
async fn recovery_fetches_the_blob_and_the_log_and_publishes_the_new_entry() {
    let remote = server();
    let fixture = Fixture::generate(44);
    let host = recovery_setup(&remote, &fixture, [7; 32], b"wrapped root key");
    let task = spawn_recovery(&host, &remote);
    until("the task ended", || !task.is_running()).await;

    host.with(|data| {
        assert_eq!(data.state, RecoveryState::Done);
        assert_eq!(data.blob.as_deref(), Some(&b"wrapped root key"[..]));
        assert_eq!(data.log_seen.as_ref().unwrap(), &fixture.encoded_log());
        assert_eq!(
            data.calls,
            [
                "recovery_info",
                "recovery_id",
                "recovery_blob_received",
                "recovery_account_id",
                "recovery_log_received",
                "recovery_entry_published",
            ]
        );
    });
    let log = remote.fetch_log_sync(fixture.account_id, None).unwrap();
    assert_eq!(
        log.last(),
        Some(&fixture.revocation()),
        "the new entry is on the server"
    );
}

#[tokio::test(start_paused = true)]
async fn a_kit_the_server_does_not_know_fails_the_recovery() {
    let remote = server();
    let fixture = Fixture::generate(45);
    let host = recovery_setup(&remote, &fixture, [7; 32], b"blob");
    host.with(|data| data.id = [8; 32]);
    let task = spawn_recovery(&host, &remote);
    until("the task ended", || !task.is_running()).await;
    host.with(|data| {
        assert_eq!(data.state, RecoveryState::Failed);
        assert_eq!(data.error.as_deref(), Some(KIT_UNKNOWN));
        assert!(data.blob.is_none());
    });
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_server_fails_the_recovery_after_three_attempts() {
    let remote = server();
    let fixture = Fixture::generate(46);
    let host = recovery_setup(&remote, &fixture, [7; 32], b"blob");
    remote.set_offline(true);
    let started = tokio::time::Instant::now();
    let task = spawn_recovery(&host, &remote);
    until("the task ended", || !task.is_running()).await;
    assert_eq!(host.with(|data| data.state), RecoveryState::Failed);
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "1 s then 2 s between attempts"
    );
}

#[tokio::test(start_paused = true)]
async fn a_recovery_that_is_not_under_way_does_nothing() {
    let remote = server();
    let fixture = Fixture::generate(47);
    let host = recovery_setup(&remote, &fixture, [7; 32], b"blob");
    host.with(|data| data.state = RecoveryState::Idle);
    let task = spawn_recovery(&host, &remote);
    until("the task ended", || !task.is_running()).await;
    assert_eq!(host.with(|data| data.calls.clone()), ["recovery_info"]);
}

// ----- starting them on the core's requests ------------------------------------------------------------

/// Both halves in one host, as the shell's state is.
struct BothHosts {
    pairing: Arc<FakePairing>,
    recovery: Arc<FakeRecovery>,
}

impl PairingHost for BothHosts {
    fn pairing_info(&self) -> Result<PairingInfo, ApiError> {
        self.pairing.pairing_info()
    }
    fn pairing_mailbox_id(&self) -> Result<Option<[u8; 6]>, ApiError> {
        self.pairing.pairing_mailbox_id()
    }
    fn pairing_mailbox_opened(&self, id: [u8; 6], expires_at: i64) -> Result<(), ApiError> {
        self.pairing.pairing_mailbox_opened(id, expires_at)
    }
    fn pairing_outbox(&self) -> Result<Vec<(u32, Vec<u8>)>, ApiError> {
        self.pairing.pairing_outbox()
    }
    fn pairing_posted(&self, seq: u32) -> Result<(), ApiError> {
        self.pairing.pairing_posted(seq)
    }
    fn pairing_receive(&self, seq: u32, role: PairingRole, bytes: &[u8]) -> Result<(), ApiError> {
        self.pairing.pairing_receive(seq, role, bytes)
    }
    fn pairing_failed(&self, message: &str) -> Result<(), ApiError> {
        self.pairing.pairing_failed(message)
    }
    fn sync_identity(&self) -> Result<Option<(AccountId, DeviceId)>, ApiError> {
        PairingHost::sync_identity(self.pairing.as_ref())
    }
    fn sync_account_log(&self) -> Result<(Vec<Vec<u8>>, u64), ApiError> {
        self.pairing.sync_account_log()
    }
    fn sync_log_published(&self, up_to_seq: u64) -> Result<(), ApiError> {
        self.pairing.sync_log_published(up_to_seq)
    }
    fn subscribe(&self, on_event: Box<dyn Fn(&Event) + Send + Sync>) -> Box<dyn FnOnce() + Send> {
        self.pairing.subscribe(on_event)
    }
}

impl RecoveryHost for BothHosts {
    fn recovery_info(&self) -> Result<RecoveryInfo, ApiError> {
        self.recovery.recovery_info()
    }
    fn recovery_id(&self) -> Result<Option<[u8; 32]>, ApiError> {
        self.recovery.recovery_id()
    }
    fn recovery_blob_received(&self, bytes: &[u8]) -> Result<(), ApiError> {
        self.recovery.recovery_blob_received(bytes)
    }
    fn recovery_log_received(&self, entries: &[Vec<u8>]) -> Result<Vec<u8>, ApiError> {
        self.recovery.recovery_log_received(entries)
    }
    fn recovery_entry_published(&self) -> Result<(), ApiError> {
        self.recovery.recovery_entry_published()
    }
    fn recovery_failed(&self, message: &str) -> Result<(), ApiError> {
        self.recovery.recovery_failed(message)
    }
    fn recovery_account_id(&self) -> Result<Option<AccountId>, ApiError> {
        self.recovery.recovery_account_id()
    }
}

#[tokio::test(start_paused = true)]
async fn the_tasks_start_when_the_core_asks_for_them_and_stop_with_the_handle() {
    let remote = server();
    let fixture = Fixture::generate(48);
    let both = Arc::new(BothHosts {
        pairing: FakePairing::new(PairingData::joiner()),
        recovery: recovery_setup(&remote, &fixture, [9; 32], b"blob"),
    });
    let tasks = DeviceTasks::spawn(
        Arc::clone(&both),
        factory(&remote),
        tokio::runtime::Handle::current(),
    );
    assert_eq!(
        both.pairing.listeners.count(),
        1,
        "the tasks listen for requests"
    );
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        both.pairing.with(|data| data.calls.is_empty()),
        "nothing before a request"
    );
    assert!(both.recovery.with(|data| data.calls.is_empty()));

    both.pairing.listeners.emit(&Event::PairingRequested);
    until("the joiner's mailbox is open", || {
        both.pairing.state() == PairingState::Waiting
    })
    .await;
    both.pairing.listeners.emit(&Event::RecoveryRequested);
    until("the recovery is done", || {
        both.recovery.with(|data| data.state) == RecoveryState::Done
    })
    .await;

    // Dropping the handle stops the listener and the pairing task it started.
    drop(tasks);
    until("everything unsubscribed", || {
        both.pairing.listeners.count() == 0
    })
    .await;
    let calls = both.pairing.with(|data| data.calls.len());
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(both.pairing.with(|data| data.calls.len()), calls);
}

#[tokio::test(start_paused = true)]
async fn a_new_pairing_request_replaces_a_running_task() {
    let remote = server();
    let fixture = Fixture::generate(49);
    let both = Arc::new(BothHosts {
        pairing: FakePairing::new(PairingData::joiner()),
        recovery: recovery_setup(&remote, &fixture, [9; 32], b"blob"),
    });
    let _tasks = DeviceTasks::spawn(
        Arc::clone(&both),
        factory(&remote),
        tokio::runtime::Handle::current(),
    );
    both.pairing.listeners.emit(&Event::PairingRequested);
    until("the first mailbox is open", || {
        both.pairing.state() == PairingState::Waiting
    })
    .await;
    // A new session (the user started over): the core has a fresh offer.
    *both.pairing.data.lock().unwrap() = Some(PairingData::joiner());
    both.pairing.listeners.emit(&Event::PairingRequested);
    until("the second mailbox is open", || {
        both.pairing.state() == PairingState::Waiting
    })
    .await;
    // The listener of the replaced task is gone: one for the requests, one for the task.
    until("one task listens", || both.pairing.listeners.count() == 2).await;
}
