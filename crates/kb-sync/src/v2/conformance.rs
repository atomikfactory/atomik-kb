//! The conformance suite of protocol v2 (M13, `docs/development/milestone-13.md` section 1.3): one
//! suite that [`super::MemoryRemote`] and the real `kb-server` (through `kb-net`'s `HttpRemote`)
//! must both pass. Behind the `testing` feature, like v1's.
//!
//! Each `check_*` function drives one behaviour and panics with a message naming the rule on the
//! first deviation, like a test assertion. [`run_all_v2`] runs them against a **fresh** remote. The
//! checks are independent: each enrols its own people (accounts drawn from the [`Fixture`], which
//! hands out a new one every time) and its own feeds, so they run in any order and the suite can
//! share one server. They use **one handle**: the remote signs in as whoever acts next
//! (`authenticate` replaces the session), which both `MemoryRemote` and `HttpRemote` allow.
//!
//! The checks, and what each needs:
//!
//! - [`check_feed_creation`], [`check_access_list_per_role`], [`check_non_member_and_low_role`],
//!   [`check_removed_reads_up_to_remove`], [`check_membership_compare_and_swap`],
//!   [`check_membership_batch_atomic`], [`check_rekey_needed`], [`check_stale_epoch`],
//!   [`check_per_feed_chain_cursors_and_heads`], [`check_chunk_isolation_and_copy`],
//!   [`check_accounts_and_logs`], [`check_rekey_rate_limit`]: nothing special;
//! - [`check_invite_lifecycle`], [`check_closed_feed`]: their expiry and deletion parts need the
//!   server's clock (see [`Fixture::with_control`]) and are skipped without it;
//! - [`check_pruning_waits`]: needs the clock and a pruning pass ([`ServerControl`]);
//! - [`check_view_links`] (M13 slice 4): its expiry part needs the clock (skipped without it); it makes
//!   about 30 link reads;
//! - [`check_share_signup`]: reads the limits from `server_info` (the per-inviter limit is
//!   exercised when it is at most 12);
//! - [`check_quota_passes_to_owner`]: needs a remote announcing a quota of at most 64 MiB, so it
//!   skips itself otherwise: run it on a small-quota remote as well;
//! - [`check_pairing_mailbox`] (opens one mailbox, tries one oversized offer and makes 13 mailbox
//!   requests, all with `wait_seconds = 0`), [`check_recovery_blob`] (makes 4 `recovery_get` calls)
//!   and [`check_epoch_constant`]: nothing special (the device pairing and the kit recovery of
//!   ADR-0022 run on the same remote as the feeds);
//! - not in [`run_all_v2`]: [`check_feed_limit`] (creates `MAX_FEEDS_PER_ACCOUNT + 1` feeds),
//!   [`check_share_signup_total_limit`] (needs a remote whose total signup limit is small),
//!   [`check_idle_device`] (needs the clock and a remote with idle-device expiry on);
//! - last of all in [`run_all_v2`], because they exhaust a per-client limit:
//!   [`check_invite_open_rate_limit`], then [`check_link_read_rate_limit`].
//!
//! - the realtime checks of M13 slice 3 (`check_ws_*`, [`run_all_ws`], in [`ws`]) need a [`WsHarness`] that
//!   opens sockets and lets time pass besides the remote itself; `MemoryRemote` is its own harness.
//!
//! The trait is not object-safe (`async fn`), so the checks are generic over `R: RemoteStore`.

// A test suite: failing loudly with the rule's name is its whole job.
#![allow(clippy::panic)]

pub mod kit;
pub mod ws;

use std::cell::Cell;
use std::sync::Arc;

use kb_crypto::Secret32;
use kb_keys::invite::{InviteSecret, invite_id};
use kb_keys::link::LinkKey;
use kb_keys::membership::{EntryBody, MembershipEntry, Private};
use kb_keys::space_keys::SpaceKey;
use kb_types::devices::{PAIRING_MAX_MESSAGE_BYTES, RECOVERY_BLOB_MAX_BYTES};
use kb_types::sharing::{
    CLOSE_READ_DAYS, INVITE_OPEN_PER_IP_PER_HOUR, LINK_GETS_PER_MINUTE_PER_IP, LINK_MAX_CHUNKS,
    MAX_FEEDS_PER_ACCOUNT, PRUNE_ACK_DAYS, REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY, SpaceSettings,
};
use kb_types::sync::FeedKind;
use kb_types::sync::MAX_WIRE_BYTES;
use kb_types::testing::SeededRng;
use kb_types::{AttachmentId, ChunkId, DocId, FeedId, MemberRole, PageId};

pub use ws::{
    WsConnection, WsHarness, check_ws_frames_follow_the_feeds_order, check_ws_inline_entries,
    check_ws_large_entries_are_announced, check_ws_presence_rate_limit, check_ws_presence_refusals,
    check_ws_presence_relay, check_ws_removed_member_stops_at_its_remove,
    check_ws_revoked_device_is_closed_and_cannot_reconnect, run_all_ws,
};

pub use kit::{
    DAY_MS, Dev, FIXTURE_TIME_MS, HOUR_MS, Person, Space, World, random_chunk, random_doc,
    sealed_entry, sealed_snapshot, signed_head,
};

use super::{
    ItemKind, LinkFetch, LinkPut, MembershipHead, PAIR_ROLE_OPENER, PAIR_ROLE_TYPIST, Publication,
    PublishedAttachment, RemoteStore, open_publication, peek_feed_item, seal_publication,
};
use crate::{PairingMessage, SyncError};

/// What a remote under test lets the suite do besides the protocol: move its clock and run its
/// pruning pass. `MemoryRemote` has both (`advance_ms`, `prune`); the server tests wire them to their
/// `FixedClock` and `Server::prune_now`.
pub struct ServerControl {
    advance_ms: Box<dyn Fn(i64) + Send + Sync>,
    prune: Box<dyn Fn() + Send + Sync>,
}

impl core::fmt::Debug for ServerControl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ServerControl")
    }
}

/// The inputs of the suite: people (accounts with devices and logs), feeds and invites generated
/// from a seed. Deterministic: two fixtures of one seed build the same people and spaces. Use one
/// seed per remote.
#[derive(Debug, Clone)]
pub struct Fixture {
    seed: u64,
    /// The suite's own "now" (Unix ms): the `at` of the log entries it builds and the base of the
    /// expiry times. It starts at [`FIXTURE_TIME_MS`]; the remote under test must agree.
    now: Cell<i64>,
    /// People handed out so far.
    people: Cell<u64>,
    /// Spaces handed out so far.
    spaces: Cell<u64>,
    control: Option<Arc<ServerControl>>,
    /// The operator token `create_account` uses (default `dev`).
    pub invite_token: String,
}

impl Fixture {
    /// A fixture whose keys, ids and payloads all derive from `seed` (reproducible, like v1's).
    #[must_use]
    pub fn generate(seed: u64) -> Self {
        Self {
            seed,
            now: Cell::new(FIXTURE_TIME_MS),
            people: Cell::new(0),
            spaces: Cell::new(0),
            control: None,
            invite_token: super::memory::DEFAULT_INVITE_TOKEN.to_owned(),
        }
    }

    /// The seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The same fixture with another operator invite token.
    #[must_use]
    pub fn with_invite_token(mut self, token: &str) -> Self {
        token.clone_into(&mut self.invite_token);
        self
    }

    /// The same fixture with a way to move the remote's clock and to run its pruning pass, which
    /// the expiry, pruning and deletion parts of the suite need. `advance_ms` must move the clock
    /// the remote's invite expiry, idle devices and pruning read; `prune` runs one pass of it.
    #[must_use]
    pub fn with_control(
        mut self,
        advance_ms: impl Fn(i64) + Send + Sync + 'static,
        prune: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.control = Some(Arc::new(ServerControl {
            advance_ms: Box::new(advance_ms),
            prune: Box::new(prune),
        }));
        self
    }

    /// The same fixture whose "now" starts at `now_ms` (a remote with another clock).
    #[must_use]
    pub fn with_now_ms(self, now_ms: i64) -> Self {
        self.now.set(now_ms);
        self
    }

    /// The suite's "now", Unix ms.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.now.get()
    }

    /// Whether the clock and the pruning pass can be controlled.
    #[must_use]
    pub fn has_control(&self) -> bool {
        self.control.is_some()
    }

    /// Moves the remote's clock (and the suite's) forward; does nothing without control.
    pub fn advance(&self, delta_ms: i64) {
        if let Some(control) = &self.control {
            (control.advance_ms)(delta_ms);
            self.now.set(self.now.get().saturating_add(delta_ms));
        }
    }

    /// Runs the remote's pruning pass; does nothing without control.
    pub fn prune(&self) {
        if let Some(control) = &self.control {
            (control.prune)();
        }
    }

    /// `n` people nobody was handed before, not yet known to any server.
    #[must_use]
    pub fn people(&self, n: usize) -> Vec<Person> {
        (0..n)
            .map(|_| {
                let index = self.people.get();
                self.people.set(index + 1);
                let seed = self.seed.wrapping_mul(1_000_003).wrapping_add(index);
                Person::generate(seed, &format!("Person {index}"), self.now())
            })
            .collect()
    }

    /// A new space (a random feed id, no key yet) named `name`.
    #[must_use]
    pub fn space(&self, name: &str) -> Space {
        let index = self.spaces.get();
        self.spaces.set(index + 1);
        let seed = self
            .seed
            .wrapping_mul(7_919)
            .wrapping_add(0x5eed_0000 + index);
        Space::new(seed, name, self.now())
    }

    /// A random document id.
    #[must_use]
    pub fn doc(&self) -> DocId {
        let index = self.spaces.get();
        self.spaces.set(index + 1);
        random_doc(&SeededRng::new(self.seed ^ 0xd0c0_0000 ^ index))
    }

    /// A random chunk (`len` bytes) and its id.
    #[must_use]
    pub fn chunk(&self, len: usize) -> (kb_types::ChunkId, Vec<u8>) {
        let index = self.spaces.get();
        self.spaces.set(index + 1);
        random_chunk(&SeededRng::new(self.seed ^ 0xc0c0_0000 ^ index), len)
    }
}

// ----- helpers ------------------------------------------------------------------------------

/// A result that must be `Ok` (panics naming `what` otherwise).
pub fn ok<T>(result: Result<T, SyncError>, what: &str) -> T {
    result.unwrap_or_else(|e| panic!("conformance: {what} failed: {e:?}"))
}

/// A result that must be an error the matcher accepts.
pub fn expect_err<T: core::fmt::Debug>(
    result: Result<T, SyncError>,
    matcher: fn(&SyncError) -> bool,
    what: &str,
) {
    match result {
        Err(e) if matcher(&e) => {}
        other => panic!("conformance: {what}: unexpected answer {other:?}"),
    }
}

/// Matches `SyncError::*` for `expect_err`.
pub fn is_unauthorized(e: &SyncError) -> bool {
    matches!(e, SyncError::Unauthorized)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_not_found(e: &SyncError) -> bool {
    matches!(e, SyncError::NotFound)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_forbidden(e: &SyncError) -> bool {
    matches!(e, SyncError::Forbidden)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_authentication(e: &SyncError) -> bool {
    matches!(e, SyncError::Authentication)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_protocol(e: &SyncError) -> bool {
    matches!(e, SyncError::Protocol(_))
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_conflict(e: &SyncError) -> bool {
    matches!(e, SyncError::Conflict(_))
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_rekey_needed(e: &SyncError) -> bool {
    matches!(e, SyncError::RekeyNeeded)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_stale_epoch(e: &SyncError) -> bool {
    matches!(e, SyncError::StaleEpoch)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_closed(e: &SyncError) -> bool {
    matches!(e, SyncError::Closed)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_quota(e: &SyncError) -> bool {
    matches!(e, SyncError::Quota)
}
/// Matches `SyncError::*` for `expect_err`.
pub fn is_unavailable(e: &SyncError) -> bool {
    matches!(e, SyncError::Unavailable(_))
}

/// Signs in as `person`'s first active device.
pub async fn login<R: RemoteStore>(remote: &R, person: &Person) {
    let dev = person.active_device();
    ok(
        remote
            .authenticate(person.account, person.device_id(dev), &person.signer(dev))
            .await,
        "authenticate",
    );
}

/// Creates the accounts of `n` fresh people with the operator token.
pub async fn enroll<R: RemoteStore>(remote: &R, fx: &Fixture, n: usize) -> Vec<Person> {
    let people = fx.people(n);
    for person in &people {
        ok(
            remote
                .create_account(&fx.invite_token, &person.encoded_log())
                .await,
            "create_account",
        );
    }
    people
}

/// Two, three or four enrolled people as an array.
pub async fn enroll_n<R: RemoteStore, const N: usize>(remote: &R, fx: &Fixture) -> [Person; N] {
    let people = enroll(remote, fx, N).await;
    people
        .try_into()
        .unwrap_or_else(|_| panic!("conformance: enrolled the wrong number of people"))
}

/// Posts one membership entry as `who` (signing in first) and records it when accepted.
pub async fn post<R: RemoteStore>(
    remote: &R,
    space: &mut Space,
    who: &Person,
    entry: &MembershipEntry,
) -> Result<MembershipHead, SyncError> {
    login(remote, who).await;
    let result = remote
        .membership_append(space.feed, &[entry.encode()])
        .await;
    if result.is_ok() {
        space.accept(entry);
    }
    result
}

/// Posts one membership entry that must be accepted.
pub async fn post_ok<R: RemoteStore>(
    remote: &R,
    space: &mut Space,
    who: &Person,
    entry: &MembershipEntry,
    what: &str,
) -> MembershipHead {
    ok(post(remote, space, who, entry).await, what)
}

/// Founds the space: `creator` posts its `Create` (epoch 1).
pub async fn found<R: RemoteStore>(
    remote: &R,
    space: &mut Space,
    creator: &Person,
) -> super::FeedCreated {
    login(remote, creator).await;
    let create = space.create(creator, 1);
    let created = ok(remote.feeds_create(&create.encode()).await, "feeds_create");
    space.accept(&create);
    created
}

/// `owner` invites `joiner` with a link invite of `role` (a week) and `joiner` joins.
pub async fn invite_and_join<R: RemoteStore>(
    remote: &R,
    fx: &Fixture,
    space: &mut Space,
    owner: &Person,
    joiner: &Person,
    role: MemberRole,
) -> InviteSecret {
    space.now = fx.now();
    let expires_at = fx.now() + 7 * DAY_MS;
    let (invite, secret) = space.invite(owner, role, expires_at, None);
    post_ok(remote, space, owner, &invite, "the Invite").await;
    let join = space.join(joiner, &secret);
    post_ok(remote, space, joiner, &join, "the Join").await;
    secret
}

/// The server's epoch.
pub async fn server_epoch<R: RemoteStore>(remote: &R) -> [u8; 16] {
    ok(remote.server_info().await, "server_info").server_epoch
}

/// Every item of the feed as the signed-in session sees it, with its cursor.
pub async fn read_feed<R: RemoteStore>(remote: &R, feed: FeedId) -> Vec<(u64, Vec<u8>)> {
    let mut out = Vec::new();
    let mut after = 0;
    loop {
        let page = ok(remote.changes(feed, after, 50).await, "changes");
        let more = page.more;
        for (cursor, bytes) in page.changes {
            after = cursor;
            out.push((cursor, bytes));
        }
        if !more {
            return out;
        }
    }
}

/// The newest cursor of the feed and the chain hash after it, as the signed-in member reads them.
pub async fn feed_tip<R: RemoteStore>(remote: &R, feed: FeedId) -> (u64, [u8; 32]) {
    let items = read_feed(remote, feed).await;
    let cursor = items.last().map_or(0, |(cursor, _)| *cursor);
    let page = ok(remote.changes(feed, cursor, 1).await, "changes at the tip");
    (cursor, page.chain_hash_after)
}

/// Whether the feed's items hold exactly these bytes.
/// Whether the feed items hold exactly these bytes.
pub fn holds(items: &[(u64, Vec<u8>)], bytes: &[u8]) -> bool {
    items.iter().any(|(_, item)| item == bytes)
}

// ----- the checks ---------------------------------------------------------------------------

/// Runs every check against `remote`, which must be a fresh server (see the module documentation
/// for what each needs; `check_invite_open_rate_limit` goes last because it exhausts the
/// per-client limit of `invite_open`).
pub async fn run_all_v2<R: RemoteStore>(remote: &R, fixture: &Fixture) {
    check_accounts_and_logs(remote, fixture).await;
    check_feed_creation(remote, fixture).await;
    check_access_list_per_role(remote, fixture).await;
    check_non_member_and_low_role(remote, fixture).await;
    check_removed_reads_up_to_remove(remote, fixture).await;
    check_membership_compare_and_swap(remote, fixture).await;
    check_membership_batch_atomic(remote, fixture).await;
    check_rekey_needed(remote, fixture).await;
    check_stale_epoch(remote, fixture).await;
    check_rekey_rate_limit(remote, fixture).await;
    check_invite_lifecycle(remote, fixture).await;
    check_share_signup(remote, fixture).await;
    check_quota_passes_to_owner(remote, fixture).await;
    check_per_feed_chain_cursors_and_heads(remote, fixture).await;
    check_chunk_isolation_and_copy(remote, fixture).await;
    check_pruning_waits(remote, fixture).await;
    check_closed_feed(remote, fixture).await;
    check_pairing_mailbox(remote, fixture).await;
    check_recovery_blob(remote, fixture).await;
    check_epoch_constant(remote, fixture).await;
    check_view_links(remote, fixture).await;
    check_invite_open_rate_limit(remote, fixture).await;
    check_link_read_rate_limit(remote, fixture).await;
}

/// Accounts and device logs: the operator token, an account that exists, device ids unique across
/// accounts, appending to and fetching logs (one and in a batch), calls without a session, a revoked
/// device's session.
pub async fn check_accounts_and_logs<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let info = ok(remote.server_info().await, "server_info");
    assert_eq!(
        info.protocol_version,
        kb_types::sync::PROTOCOL_VERSION,
        "conformance: server_info speaks protocol v2"
    );
    let [mut a] =
        <[Person; 1]>::try_from(fx.people(1)).unwrap_or_else(|_| panic!("conformance: one person"));
    expect_err(
        remote
            .create_account("not-the-token", &a.encoded_log())
            .await,
        is_unauthorized,
        "create_account with a wrong operator token",
    );
    ok(
        remote
            .create_account(&fx.invite_token, &a.encoded_log())
            .await,
        "create_account",
    );
    expect_err(
        remote
            .create_account(&fx.invite_token, &a.encoded_log())
            .await,
        is_conflict,
        "create_account of an account that exists",
    );
    // A device id is unique across accounts.
    let twin = Person::generate_with_device_id(
        fx.seed().wrapping_add(0x7a17),
        "Twin",
        fx.now(),
        Some(a.device_id(0)),
    );
    expect_err(
        remote
            .create_account(&fx.invite_token, &twin.encoded_log())
            .await,
        is_conflict,
        "create_account with a device id another account uses",
    );
    // Logs: whole, after a sequence number, in a batch; unknown accounts.
    let whole = ok(remote.fetch_log(a.account, None).await, "fetch_log");
    assert_eq!(whole, a.encoded_log(), "conformance: fetch_log is the log");
    assert!(
        ok(
            remote.fetch_log(a.account, Some(0)).await,
            "fetch_log after 0"
        )
        .is_empty(),
        "conformance: nothing follows the genesis yet"
    );
    expect_err(
        remote.fetch_log(twin.account, None).await,
        is_not_found,
        "fetch_log of an unknown account",
    );
    a.add_device(fx.now());
    let tail_seq = a.log_seq();
    ok(
        remote.append_log(a.account, &a.encoded_log_after(0)).await,
        "append_log",
    );
    // Publishing again is fine; a different entry at a taken sequence number is not.
    ok(
        remote.append_log(a.account, &a.encoded_log_after(0)).await,
        "append_log again",
    );
    let mut fork = Person::clone(&a);
    fork.log = kb_keys::DeviceLog::from_entries(a.log.entries()[..1].to_vec())
        .unwrap_or_else(|e| panic!("conformance: log prefix: {e}"));
    fork.devices.truncate(1);
    fork.add_device(fx.now() + 5);
    expect_err(
        remote
            .append_log(a.account, &fork.encoded_log_after(0))
            .await,
        is_conflict,
        "append_log of a forked log",
    );
    let tail = ok(remote.fetch_log(a.account, Some(0)).await, "fetch_log tail");
    assert_eq!(
        tail,
        a.encoded_log_after(0),
        "conformance: the tail after 0"
    );
    let batch = ok(
        remote
            .logs_batch(&[
                super::LogRequest {
                    account: a.account,
                    after: None,
                },
                super::LogRequest {
                    account: twin.account,
                    after: None,
                },
                super::LogRequest {
                    account: a.account,
                    after: Some(1),
                },
            ])
            .await,
        "logs_batch",
    );
    assert_eq!(
        batch.len(),
        2,
        "conformance: logs_batch leaves unknown accounts out"
    );
    assert_eq!(batch[0].entries, a.encoded_log());
    assert!(batch[1].entries.is_empty());
    // A wrong signature is refused (`Authentication` from the reference remote, `Unauthorized` from
    // the server's session endpoint, which v1 and v2 share) and leaves no session behind.
    let wrong = wrong_signer();
    expect_err(
        remote
            .authenticate(a.account, a.device_id(0), &|m: &[u8]| wrong(m))
            .await,
        |e| matches!(e, SyncError::Authentication | SyncError::Unauthorized),
        "authenticate with a wrong signature",
    );
    expect_err(
        remote.feeds_list().await,
        is_unauthorized,
        "feeds_list without a session",
    );
    expect_err(
        remote.changes(FeedId::from_account(a.account), 0, 10).await,
        is_unauthorized,
        "changes without a session",
    );
    // A revoked device loses its session at once.
    login(remote, &a).await;
    ok(remote.feeds_list().await, "feeds_list");
    a.revoke_device(0, fx.now() + 10);
    ok(
        remote
            .append_log(a.account, &a.encoded_log_after(tail_seq))
            .await,
        "append_log of a revocation",
    );
    expect_err(
        remote.feeds_list().await,
        is_unauthorized,
        "feeds_list of a revoked device",
    );
    expect_err(
        remote
            .authenticate(a.account, twin.device_id(0), &twin.signer(0))
            .await,
        |_| true,
        "authenticate as a device of another account",
    );
}

/// A signer whose signatures no server accepts.
fn wrong_signer() -> impl Fn(&[u8]) -> kb_crypto::Signature {
    let key = kb_crypto::SigningKey::from_seed(&kb_crypto::Secret32::from_bytes([0x66; 32]));
    move |message: &[u8]| kb_crypto::sign(&key, "auth", message)
}

/// A feed is created from a `Create`; a feed id collision (also with an account id) is `Conflict`; a
/// bad signature is `Authentication`, a `Create` that breaks a rule `Protocol`; the creator is listed
/// as Owner and reads its own log.
pub async fn check_feed_creation<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Feed creation");
    let create = space.create(&alice, 1);
    login(remote, &alice).await;
    let created = ok(remote.feeds_create(&create.encode()).await, "feeds_create");
    space.accept(&create);
    assert_eq!(created.feed, space.feed, "conformance: the created feed");
    assert_eq!(
        created.head,
        create.head(),
        "conformance: the head after the Create"
    );
    assert_eq!(created.head.seq, 0);
    assert!(created.cursor > 0, "conformance: the Create has a cursor");

    let listing = ok(remote.feeds_list().await, "feeds_list");
    let personal = listing
        .iter()
        .find(|l| l.feed == FeedId::from_account(alice.account))
        .unwrap_or_else(|| panic!("conformance: feeds_list names the personal feed"));
    assert_eq!(personal.kind, FeedKind::Personal);
    assert_eq!(personal.role, None);
    let shared = listing
        .iter()
        .find(|l| l.feed == space.feed)
        .unwrap_or_else(|| panic!("conformance: feeds_list names the shared feed"));
    assert_eq!(shared.kind, FeedKind::Shared);
    assert_eq!(shared.role, Some(MemberRole::Owner));
    assert!(!shared.removed && !shared.closed);

    let page = ok(
        remote.membership_get(space.feed, None).await,
        "membership_get",
    );
    assert_eq!(
        page.entries.len(),
        1,
        "conformance: the log holds the Create"
    );
    assert_eq!(page.entries[0].bytes, create.encode());
    assert_eq!(page.entries[0].cursor, created.cursor);
    assert_eq!(page.head, create.head());
    assert!(!page.more);
    let feed_items = read_feed(remote, space.feed).await;
    assert_eq!(
        feed_items.len(),
        1,
        "conformance: the feed holds the Create"
    );
    assert_eq!(
        peek_feed_item(&feed_items[0].1).map(|i| i.kind),
        Ok(ItemKind::Membership)
    );

    // Collisions.
    expect_err(
        remote.feeds_create(&create.encode()).await,
        is_conflict,
        "feeds_create of a feed that exists",
    );
    login(remote, &bob).await;
    let mut twin = fx.space("Same feed id");
    twin.feed = space.feed;
    let dup = twin.create(&bob, 1);
    expect_err(
        remote.feeds_create(&dup.encode()).await,
        is_conflict,
        "feeds_create with the id of another feed",
    );
    let mut squat = fx.space("An account id");
    squat.feed = FeedId::from_account(alice.account);
    let squatted = squat.create(&bob, 1);
    expect_err(
        remote.feeds_create(&squatted.encode()).await,
        is_conflict,
        "feeds_create with an account id",
    );
    // A bad signature, a rule broken, a log that does not start, an entry that is no Create.
    let mut fresh = fx.space("Bad signature");
    let mut bytes = fresh.create(&bob, 1).encode();
    if let Some(last) = bytes.last_mut() {
        *last ^= 1;
    }
    expect_err(
        remote.feeds_create(&bytes).await,
        is_authentication,
        "feeds_create with a bad signature",
    );
    let bad = fx.space("No boxes");
    let sk = SpaceKey::generate(&SeededRng::new(fx.seed() ^ 0xbad));
    let (_, member_heads) = bad.boxes_for(&sk, 1, &[&bob]);
    let broken = bad.entry_at(
        &bob,
        0,
        0,
        [0u8; 32],
        1,
        EntryBody::Create {
            epoch: 1,
            sk_commitment: sk.commitment(bad.feed, 1),
            boxes: Vec::new(),
            member_heads,
        },
        &Private::Create {
            space_name: "No boxes".to_owned(),
            settings: SpaceSettings::default(),
            profile_name: bob.name.clone(),
        },
    );
    expect_err(
        remote.feeds_create(&broken.encode()).await,
        is_protocol,
        "feeds_create of a Create that boxes nobody",
    );
    let late = fx.space("Seq 3");
    let sk = SpaceKey::generate(&SeededRng::new(fx.seed() ^ 0x1a7e));
    let (boxes, member_heads) = late.boxes_for(&sk, 1, &[&bob]);
    let off = late.entry_at(
        &bob,
        0,
        3,
        [9u8; 32],
        1,
        EntryBody::Create {
            epoch: 1,
            sk_commitment: sk.commitment(late.feed, 1),
            boxes,
            member_heads,
        },
        &Private::Create {
            space_name: "Seq 3".to_owned(),
            settings: SpaceSettings::default(),
            profile_name: bob.name.clone(),
        },
    );
    expect_err(
        remote.feeds_create(&off.encode()).await,
        is_protocol,
        "feeds_create of a Create that is not at seq 0",
    );
    let stray = fx.space("Not a Create");
    let not_create = stray.profile(&bob, "Bob");
    expect_err(
        remote.feeds_create(&not_create.encode()).await,
        is_protocol,
        "feeds_create of an entry that is no Create",
    );
    expect_err(
        remote.feeds_create(b"not an envelope").await,
        |e| matches!(e, SyncError::Encoding(_) | SyncError::Protocol(_)),
        "feeds_create of garbage",
    );
    // Bob has no part in Alice's feed.
    let bobs = ok(remote.feeds_list().await, "feeds_list of Bob");
    assert!(
        bobs.iter().all(|l| l.feed != space.feed),
        "conformance: a stranger's feeds_list does not name the feed"
    );
}

/// What each role may do (`milestone-13.md` 1.3): reads and heads for every member, appends and
/// snapshot puts from Commenter, chunk puts and copies, and snapshot disputes from Editor, chunk
/// deletes for Owners; a role change takes effect at once.
pub async fn check_access_list_per_role<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob, carol, dave] = enroll_n::<R, 4>(remote, fx).await;
    let mut space = fx.space("Roles");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    invite_and_join(
        remote,
        fx,
        &mut space,
        &alice,
        &carol,
        MemberRole::Commenter,
    )
    .await;
    invite_and_join(remote, fx, &mut space, &alice, &dave, MemberRole::Viewer).await;
    let feed = space.feed;
    let epoch = space.epoch;
    let server_epoch = server_epoch(remote).await;
    // Every role: (person, may append / snapshot, may put chunks, may dispute).
    let cast = [
        (&alice, MemberRole::Owner),
        (&bob, MemberRole::Editor),
        (&carol, MemberRole::Commenter),
        (&dave, MemberRole::Viewer),
    ];
    // Appends and snapshots.
    let doc = fx.doc();
    let mut docs = Vec::new();
    for (index, (person, role)) in cast.iter().enumerate() {
        login(remote, person).await;
        let entry = space.doc_entry(person, 0, doc, 1 + index as u64, epoch, b"a change");
        let appended = remote.append(feed, &entry).await;
        let keyring = space.keyring_entry(person, 0, 1, epoch);
        let keyed = remote.append(feed, &keyring).await;
        let own_doc = fx.doc();
        let snapshot = space.doc_snapshot(person, 0, own_doc, 1, epoch, b"a snapshot");
        let snapped = remote.put_snapshot(feed, &snapshot).await;
        if role.can_comment() {
            ok(appended, "append by a member who may comment");
            ok(keyed, "a keyring entry by a member who may comment");
            ok(snapped, "put_snapshot by a member who may comment");
            docs.push(own_doc);
        } else {
            expect_err(appended, is_forbidden, "append by a Viewer");
            expect_err(keyed, is_forbidden, "a keyring entry by a Viewer");
            expect_err(snapped, is_forbidden, "put_snapshot by a Viewer");
        }
    }
    // Heads: every member, Viewers included.
    let (tip, chain) = {
        login(remote, &alice).await;
        feed_tip(remote, feed).await
    };
    for (person, _) in &cast {
        login(remote, person).await;
        let head = space.head_of(person, 0, tip, chain, server_epoch);
        ok(remote.put_head(feed, &head).await, "put_head by a member");
    }
    // Chunks.
    let (chunk, chunk_bytes) = fx.chunk(300);
    for (person, role) in &cast {
        login(remote, person).await;
        let (id, bytes) = fx.chunk(200);
        let put = remote.put_chunk(feed, id, &bytes).await;
        if role.can_edit() {
            ok(put, "put_chunk by an Editor");
        } else {
            expect_err(put, is_forbidden, "put_chunk below Editor");
        }
    }
    login(remote, &bob).await;
    ok(
        remote.put_chunk(feed, chunk, &chunk_bytes).await,
        "put_chunk",
    );
    for (person, role) in &cast {
        login(remote, person).await;
        assert_eq!(
            ok(remote.get_chunk(feed, chunk).await, "get_chunk by a member"),
            Some(chunk_bytes.clone()),
            "conformance: every member reads chunks ({role:?})"
        );
    }
    // Copy: an Editor copies from its personal namespace, a Commenter may not.
    let (personal_chunk, personal_bytes) = fx.chunk(120);
    login(remote, &bob).await;
    ok(
        remote
            .put_chunk(
                FeedId::from_account(bob.account),
                personal_chunk,
                &personal_bytes,
            )
            .await,
        "put_chunk into the personal feed",
    );
    ok(
        remote.copy_chunk(feed, personal_chunk).await,
        "copy_chunk by an Editor",
    );
    assert_eq!(
        ok(remote.get_chunk(feed, personal_chunk).await, "get_chunk"),
        Some(personal_bytes),
        "conformance: the copy holds the bytes"
    );
    login(remote, &carol).await;
    expect_err(
        remote.copy_chunk(feed, personal_chunk).await,
        is_forbidden,
        "copy_chunk by a Commenter",
    );
    // Disputes: Editor and up (the snapshot of Alice's document exists).
    let disputed = docs[0];
    login(remote, &carol).await;
    expect_err(
        remote.snapshot_dispute(feed, disputed, 1).await,
        is_forbidden,
        "snapshot_dispute by a Commenter",
    );
    login(remote, &bob).await;
    ok(
        remote.snapshot_dispute(feed, disputed, 1).await,
        "snapshot_dispute by an Editor",
    );
    expect_err(
        remote.snapshot_dispute(feed, fx.doc(), 1).await,
        is_not_found,
        "snapshot_dispute of a document without a snapshot",
    );
    // Chunk deletes: Owners only.
    login(remote, &bob).await;
    expect_err(
        remote.delete_chunk(feed, chunk).await,
        is_forbidden,
        "delete_chunk by an Editor",
    );
    login(remote, &carol).await;
    expect_err(
        remote.delete_chunk(feed, chunk).await,
        is_forbidden,
        "delete_chunk by a Commenter",
    );
    login(remote, &alice).await;
    ok(
        remote.delete_chunk(feed, chunk).await,
        "delete_chunk by an Owner",
    );
    assert_eq!(
        ok(
            remote.get_chunk(feed, chunk).await,
            "get_chunk after the delete"
        ),
        None
    );
    ok(
        remote.delete_chunk(feed, chunk).await,
        "delete_chunk of a chunk the feed does not hold",
    );
    // Reads: every role.
    for (person, role) in &cast {
        login(remote, person).await;
        ok(remote.changes(feed, 0, 10).await, "changes by a member");
        ok(remote.snapshots(feed).await, "snapshots by a member");
        ok(remote.heads(feed).await, "heads by a member");
        ok(
            remote.snapshot_marks(feed, 0).await,
            "snapshot_marks by a member",
        );
        ok(
            remote.membership_get(feed, None).await,
            "membership_get by a member",
        );
        ok(
            remote.get_snapshot(feed, docs[0]).await,
            "get_snapshot by a member",
        );
        let listing = ok(remote.feeds_list().await, "feeds_list");
        let mine = listing
            .iter()
            .find(|l| l.feed == feed)
            .unwrap_or_else(|| panic!("conformance: a member's feeds_list names the feed"));
        assert_eq!(mine.role, Some(*role), "conformance: the listed role");
    }
    let marks = ok(remote.snapshot_marks(feed, 0).await, "snapshot_marks");
    assert!(
        marks.marks.len() >= docs.len(),
        "conformance: every snapshot has a mark"
    );
    assert!(
        marks.marks.windows(2).all(|w| w[0].mark < w[1].mark),
        "conformance: marks increase"
    );
    // A role change takes effect at once.
    space.now = fx.now();
    let demote = space.role_set(&alice, bob.account, MemberRole::Viewer);
    post_ok(remote, &mut space, &alice, &demote, "RoleSet to Viewer").await;
    login(remote, &bob).await;
    let entry = space.doc_entry(&bob, 0, doc, 50, epoch, b"after the demotion");
    expect_err(
        remote.append(feed, &entry).await,
        is_forbidden,
        "append by a demoted Editor",
    );
    ok(
        remote.changes(feed, 0, 10).await,
        "changes by a demoted Editor",
    );
    let promote = space.role_set(&alice, bob.account, MemberRole::Editor);
    post_ok(remote, &mut space, &alice, &promote, "RoleSet to Editor").await;
    login(remote, &bob).await;
    ok(
        remote.append(feed, &entry).await,
        "append by a promoted Editor",
    );
}

/// A non-member gets `NotFound` from every feed call (as for an unknown feed), a member whose role
/// is too low `Forbidden`.
pub async fn check_non_member_and_low_role<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, viewer, erin] = enroll_n::<R, 3>(remote, fx).await;
    let mut space = fx.space("Strangers");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &viewer, MemberRole::Viewer).await;
    let feed = space.feed;
    let epoch = space.epoch;
    let server_epoch = server_epoch(remote).await;
    login(remote, &alice).await;
    let doc = fx.doc();
    ok(
        remote
            .append(feed, &space.doc_entry(&alice, 0, doc, 1, epoch, b"x"))
            .await,
        "append",
    );
    ok(
        remote
            .put_snapshot(feed, &space.doc_snapshot(&alice, 0, doc, 1, epoch, b"s"))
            .await,
        "put_snapshot",
    );
    let (chunk, bytes) = fx.chunk(64);
    ok(remote.put_chunk(feed, chunk, &bytes).await, "put_chunk");

    // The Viewer's refusals are `Forbidden`.
    login(remote, &viewer).await;
    expect_err(
        remote
            .append(feed, &space.doc_entry(&viewer, 0, doc, 9, epoch, b"x"))
            .await,
        is_forbidden,
        "append by a Viewer",
    );
    expect_err(
        remote.put_chunk(feed, chunk, &bytes).await,
        is_forbidden,
        "put_chunk by a Viewer",
    );
    let profile = space.profile(&viewer, "Viewer");
    ok(
        remote.membership_append(feed, &[profile.encode()]).await,
        "a Viewer may post its own Profile",
    );
    space.accept(&profile);
    let settings = space.settings(&viewer, SpaceSettings::default());
    expect_err(
        remote.membership_append(feed, &[settings.encode()]).await,
        is_forbidden,
        "a Viewer's Settings entry",
    );

    // The stranger's every call is `NotFound`, whatever the feed is.
    login(remote, &erin).await;
    let alice_personal = FeedId::from_account(alice.account);
    let unknown = space.random_feed();
    for (name, target) in [
        ("an existing feed", feed),
        ("an unknown feed", unknown),
        ("another account's personal feed", alice_personal),
    ] {
        let what = |call: &str| format!("{call} on {name} by a stranger");
        let entry = sealed_entry(
            target,
            &erin,
            0,
            doc,
            1,
            epoch,
            b"x",
            &SeededRng::new(fx.seed() ^ 0x57a9),
        );
        let snapshot = sealed_snapshot(
            target,
            &erin,
            0,
            doc,
            1,
            epoch,
            b"s",
            &SeededRng::new(fx.seed() ^ 0x57aa),
        );
        let head = signed_head(target, &erin, 0, 1, [0u8; 32], server_epoch, fx.now());
        expect_err(
            remote.append(target, &entry).await,
            is_not_found,
            &what("append"),
        );
        expect_err(
            remote.put_snapshot(target, &snapshot).await,
            is_not_found,
            &what("put_snapshot"),
        );
        expect_err(
            remote.put_head(target, &head).await,
            is_not_found,
            &what("put_head"),
        );
        expect_err(
            remote.put_chunk(target, chunk, &bytes).await,
            is_not_found,
            &what("put_chunk"),
        );
        expect_err(
            remote.get_chunk(target, chunk).await,
            is_not_found,
            &what("get_chunk"),
        );
        expect_err(
            remote.delete_chunk(target, chunk).await,
            is_not_found,
            &what("delete_chunk"),
        );
        expect_err(
            remote.copy_chunk(target, chunk).await,
            is_not_found,
            &what("copy_chunk"),
        );
        expect_err(
            remote.changes(target, 0, 10).await,
            is_not_found,
            &what("changes"),
        );
        expect_err(
            remote.snapshots(target).await,
            is_not_found,
            &what("snapshots"),
        );
        expect_err(
            remote.get_snapshot(target, doc).await,
            is_not_found,
            &what("get_snapshot"),
        );
        expect_err(
            remote.snapshot_marks(target, 0).await,
            is_not_found,
            &what("snapshot_marks"),
        );
        expect_err(
            remote.snapshot_dispute(target, doc, 1).await,
            is_not_found,
            &what("snapshot_dispute"),
        );
        expect_err(remote.heads(target).await, is_not_found, &what("heads"));
        expect_err(
            remote.membership_get(target, None).await,
            is_not_found,
            &what("membership_get"),
        );
        let junk = space.profile(&erin, "Erin");
        expect_err(
            remote.membership_append(target, &[junk.encode()]).await,
            is_not_found,
            &what("membership_append"),
        );
    }
    let listing = ok(remote.feeds_list().await, "feeds_list of a stranger");
    assert!(
        listing.iter().all(|l| l.feed != feed),
        "conformance: a stranger's feeds_list does not name the feed"
    );
}

/// A removed account reads the membership log up to its own `Remove` and the feed up to that cursor
/// and nothing else: `changes` past it is `Forbidden`, the other calls `NotFound`; a departed account
/// is read the same way; an account that is invited again is a member again.
pub async fn check_removed_reads_up_to_remove<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob, carol, dave] = enroll_n::<R, 4>(remote, fx).await;
    let mut space = fx.space("Removal");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    invite_and_join(remote, fx, &mut space, &alice, &carol, MemberRole::Editor).await;
    invite_and_join(remote, fx, &mut space, &alice, &dave, MemberRole::Editor).await;
    let feed = space.feed;
    let doc = fx.doc();
    login(remote, &alice).await;
    let before = space.doc_entry(&alice, 0, doc, 1, space.epoch, b"before the removal");
    ok(
        remote.append(feed, &before).await,
        "append before the Remove",
    );

    // Alice removes Bob (with the Rekey for who stays).
    space.now = fx.now();
    let remove = space.remove(&alice, bob.account, &[&alice, &carol, &dave]);
    post_ok(remote, &mut space, &alice, &remove, "the Remove").await;
    let remove_cursor = {
        let page = ok(
            remote
                .membership_get(feed, Some(remove.header.seq - 1))
                .await,
            "membership_get",
        );
        page.entries[0].cursor
    };
    let after = space.doc_entry(&alice, 0, doc, 2, space.epoch, b"after the removal");
    ok(remote.append(feed, &after).await, "append after the Remove");

    login(remote, &bob).await;
    let log = ok(
        remote.membership_get(feed, None).await,
        "membership_get by a removed account",
    );
    let last = log
        .entries
        .last()
        .unwrap_or_else(|| panic!("conformance: the removed account reads the log"));
    assert_eq!(
        last.bytes,
        remove.encode(),
        "conformance: the log ends at the Remove"
    );
    assert_eq!(
        log.head,
        remove.head(),
        "conformance: the head is the Remove's"
    );
    assert_eq!(last.cursor, remove_cursor);
    let items = read_feed(remote, feed).await;
    assert!(
        holds(&items, &before),
        "conformance: what came before is readable"
    );
    assert!(
        holds(&items, &remove.encode()),
        "conformance: the Remove is readable"
    );
    assert!(
        !holds(&items, &after),
        "conformance: nothing after the Remove is readable"
    );
    assert!(
        items.iter().all(|(cursor, _)| *cursor <= remove_cursor),
        "conformance: no cursor past the Remove"
    );
    expect_err(
        remote.changes(feed, remove_cursor, 10).await,
        is_forbidden,
        "changes at the Remove by a removed account",
    );
    let entry = space.doc_entry(&bob, 0, doc, 5, 1, b"x");
    expect_err(
        remote.append(feed, &entry).await,
        is_not_found,
        "append by a removed account",
    );
    expect_err(
        remote.heads(feed).await,
        is_not_found,
        "heads by a removed account",
    );
    expect_err(
        remote.snapshots(feed).await,
        is_not_found,
        "snapshots by a removed account",
    );
    expect_err(
        remote.get_chunk(feed, fx.chunk(8).0).await,
        is_not_found,
        "get_chunk by a removed account",
    );
    let listing = ok(remote.feeds_list().await, "feeds_list of a removed account");
    let mine = listing
        .iter()
        .find(|l| l.feed == feed)
        .unwrap_or_else(|| panic!("conformance: a removed account still lists the feed"));
    assert!(
        mine.removed && mine.role.is_none(),
        "conformance: removed, without a role"
    );

    // A departed account is read the same way.
    let leave = space.leave(&dave);
    post_ok(remote, &mut space, &dave, &leave, "the Leave").await;
    login(remote, &dave).await;
    let log = ok(
        remote.membership_get(feed, None).await,
        "membership_get after a Leave",
    );
    assert_eq!(
        log.head,
        leave.head(),
        "conformance: the log ends at the Leave"
    );
    let cursor = log.entries.last().map_or(0, |e| e.cursor);
    expect_err(
        remote.changes(feed, cursor, 10).await,
        is_forbidden,
        "changes at the Leave by a departed account",
    );
    expect_err(
        remote
            .append(feed, &space.doc_entry(&dave, 0, doc, 9, 1, b"x"))
            .await,
        is_not_found,
        "append by a departed account",
    );

    // Carol sees everything.
    login(remote, &carol).await;
    let items = read_feed(remote, feed).await;
    assert!(
        holds(&items, &after),
        "conformance: a member reads what follows the Remove"
    );
    space.now = fx.now();
    // The Leave left a Rekey pending: Alice makes it, then invites Bob again.
    let rekey = space.rekey(&alice, &[&alice, &carol]);
    post_ok(
        remote,
        &mut space,
        &alice,
        &rekey,
        "the Rekey after the Leave",
    )
    .await;
    let (invite, secret) = space.invite(&alice, MemberRole::Viewer, fx.now() + DAY_MS, None);
    post_ok(remote, &mut space, &alice, &invite, "the second Invite").await;
    let rejoin = space.join(&bob, &secret);
    post_ok(
        remote,
        &mut space,
        &bob,
        &rejoin,
        "the Join of a removed account",
    )
    .await;
    login(remote, &bob).await;
    let listing = ok(remote.feeds_list().await, "feeds_list");
    let mine = listing
        .iter()
        .find(|l| l.feed == feed)
        .unwrap_or_else(|| panic!("conformance: the feed is listed"));
    assert_eq!(mine.role, Some(MemberRole::Viewer));
    assert!(!mine.removed);
    ok(
        remote.changes(feed, 0, 10).await,
        "changes by a re-invited account",
    );
}

/// Two Owners act at once: one compare-and-swap wins and the other gets `Conflict` and retries; a
/// wrong `prev`, a gap in `seq` and a replay are `Conflict`.
pub async fn check_membership_compare_and_swap<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Compare and swap");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    space.now = fx.now();
    let promote = space.role_set(&alice, bob.account, MemberRole::Owner);
    post_ok(remote, &mut space, &alice, &promote, "RoleSet to Owner").await;
    let seven = SpaceSettings {
        trash_retention_days: 7,
        ..SpaceSettings::default()
    };
    let fourteen = SpaceSettings {
        trash_retention_days: 14,
        ..SpaceSettings::default()
    };

    // Both build their next entry at the same head; Alice posts first.
    let theirs = space.settings(&alice, seven);
    let mine = space.settings(&bob, fourteen);
    post_ok(remote, &mut space, &alice, &theirs, "the first writer").await;
    expect_err(
        post(remote, &mut space, &bob, &mine).await,
        is_conflict,
        "the second writer at the same head",
    );
    // Bob reads the log again and writes at the new head.
    let page = ok(
        remote.membership_get(space.feed, None).await,
        "membership_get",
    );
    assert_eq!(
        page.head,
        theirs.head(),
        "conformance: the head is the winner's"
    );
    let retry = space.settings(&bob, fourteen);
    post_ok(
        remote,
        &mut space,
        &bob,
        &retry,
        "the retry at the new head",
    )
    .await;

    // A wrong `prev` with the right `seq`, a gap, a replay.
    let (seq, prev) = space.next_position();
    let stale = space.entry_at(
        &alice,
        0,
        seq,
        [7u8; 32],
        space.epoch,
        EntryBody::Settings,
        &Private::Settings { settings: seven },
    );
    expect_err(
        post(remote, &mut space, &alice, &stale).await,
        is_conflict,
        "an entry with the wrong prev",
    );
    let gap = space.entry_at(
        &alice,
        0,
        seq + 2,
        prev,
        space.epoch,
        EntryBody::Settings,
        &Private::Settings { settings: seven },
    );
    expect_err(
        post(remote, &mut space, &alice, &gap).await,
        is_conflict,
        "an entry that skips a seq",
    );
    expect_err(
        post(remote, &mut space, &bob, &retry).await,
        is_conflict,
        "a replay of an accepted entry",
    );
    let page = ok(
        remote.membership_get(space.feed, None).await,
        "membership_get",
    );
    assert_eq!(
        page.head,
        retry.head(),
        "conformance: nothing changed the head"
    );
}

/// A batch of membership entries is applied atomically: all or none.
pub async fn check_membership_batch_atomic<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice] = enroll_n::<R, 1>(remote, fx).await;
    let mut space = fx.space("Batches");
    found(remote, &mut space, &alice).await;
    space.now = fx.now();
    let head_before = space.head;
    let expires = fx.now() + 7 * DAY_MS;

    // A valid Invite followed by an Invite that grants Owner: the whole batch is refused.
    let (first, _) = space.invite(&alice, MemberRole::Editor, expires, None);
    space.accept(&first);
    let secret = InviteSecret::generate(&SeededRng::new(fx.seed() ^ 0xba7c));
    let second = space.invite_with(&alice, &secret, MemberRole::Owner, expires, None);
    space.head = head_before;
    login(remote, &alice).await;
    expect_err(
        remote
            .membership_append(space.feed, &[first.encode(), second.encode()])
            .await,
        is_protocol,
        "a batch whose second entry breaks a rule",
    );
    let page = ok(
        remote.membership_get(space.feed, None).await,
        "membership_get",
    );
    assert_eq!(
        Some(page.head),
        head_before,
        "conformance: a refused batch changes nothing"
    );
    assert!(
        !page.entries.iter().any(|item| item.bytes == first.encode()),
        "conformance: the first entry of a refused batch did not land"
    );
    // A batch whose second entry does not chain.
    space.accept(&first);
    let unchained = space.entry_at(
        &alice,
        0,
        first.header.seq + 1,
        [3u8; 32],
        space.epoch,
        EntryBody::Settings,
        &Private::Settings {
            settings: SpaceSettings::default(),
        },
    );
    space.head = head_before;
    expect_err(
        remote
            .membership_append(space.feed, &[first.encode(), unchained.encode()])
            .await,
        |e| matches!(e, SyncError::Protocol(_) | SyncError::Conflict(_)),
        "a batch whose second entry does not chain",
    );
    // The first entry alone is accepted: its position was never taken.
    post_ok(remote, &mut space, &alice, &first, "the entry alone").await;
    // Two valid entries land together.
    let profile = space.profile(&alice, "Alice");
    space.accept(&profile);
    let settings = space.settings(
        &alice,
        SpaceSettings {
            trash_retention_days: 3,
            ..SpaceSettings::default()
        },
    );
    space.head = Some(first.head());
    login(remote, &alice).await;
    let head = ok(
        remote
            .membership_append(space.feed, &[profile.encode(), settings.encode()])
            .await,
        "a batch of two valid entries",
    );
    space.accept(&profile);
    space.accept(&settings);
    assert_eq!(
        head,
        settings.head(),
        "conformance: the batch answers the last head"
    );
    let page = ok(
        remote
            .membership_get(space.feed, Some(first.header.seq))
            .await,
        "membership_get after the first entry",
    );
    assert_eq!(
        page.entries.len(),
        2,
        "conformance: both entries of the batch landed"
    );
}

/// A `Leave` (or a device revocation no `Rekey` covers) sets `pending_rekey`: content appends get
/// `RekeyNeeded` until a `Rekey` covers it; reads, heads and snapshots stay open; an entry signed by
/// a revoked device is refused.
pub async fn check_rekey_needed<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob, mut carol] = enroll_n::<R, 3>(remote, fx).await;
    let mut space = fx.space("Rekey");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    invite_and_join(remote, fx, &mut space, &alice, &carol, MemberRole::Editor).await;
    let feed = space.feed;
    let doc = fx.doc();
    let server_epoch = server_epoch(remote).await;
    login(remote, &alice).await;
    ok(
        remote
            .append(
                feed,
                &space.doc_entry(&alice, 0, doc, 1, space.epoch, b"before"),
            )
            .await,
        "append before the Leave",
    );

    // Bob leaves: appends wait for a Rekey.
    space.now = fx.now();
    let leave = space.leave(&bob);
    post_ok(remote, &mut space, &bob, &leave, "the Leave").await;
    let epoch = space.epoch;
    let blocked = space.doc_entry(&alice, 0, doc, 2, epoch, b"after the Leave");
    let blocked_keyring = space.keyring_entry(&alice, 0, 1, epoch);
    login(remote, &alice).await;
    expect_err(
        remote.append(feed, &blocked).await,
        is_rekey_needed,
        "append while a Rekey is pending",
    );
    expect_err(
        remote.append(feed, &blocked_keyring).await,
        is_rekey_needed,
        "a keyring entry while a Rekey is pending",
    );
    login(remote, &carol).await;
    expect_err(
        remote
            .append(feed, &space.doc_entry(&carol, 0, doc, 1, epoch, b"carol"))
            .await,
        is_rekey_needed,
        "append by another member while a Rekey is pending",
    );
    // Reads, heads and snapshots stay open.
    let (tip, chain) = feed_tip(remote, feed).await;
    ok(
        remote
            .put_head(feed, &space.head_of(&carol, 0, tip, chain, server_epoch))
            .await,
        "put_head while a Rekey is pending",
    );
    ok(
        remote
            .put_snapshot(feed, &space.doc_snapshot(&carol, 0, doc, 1, epoch, b"snap"))
            .await,
        "put_snapshot while a Rekey is pending",
    );
    // A member makes the Rekey; appends work again, at the new epoch.
    space.now = fx.now();
    let rekey = space.rekey(&alice, &[&alice, &carol]);
    post_ok(remote, &mut space, &alice, &rekey, "the Rekey").await;
    login(remote, &alice).await;
    ok(
        remote
            .append(
                feed,
                &space.doc_entry(&alice, 0, doc, 2, space.epoch, b"after the Rekey"),
            )
            .await,
        "append after the Rekey",
    );

    // A device revocation in a member's log: Carol adds a device and revokes the first one.
    let before_add = carol.clone();
    carol.add_device(fx.now());
    ok(
        remote
            .append_log(
                carol.account,
                &carol.encoded_log_after(before_add.log_seq()),
            )
            .await,
        "append_log of a new device",
    );
    let with_two_devices = carol.clone();
    carol.revoke_device(0, fx.now() + 1);
    ok(
        remote
            .append_log(
                carol.account,
                &carol.encoded_log_after(with_two_devices.log_seq()),
            )
            .await,
        "append_log of a revocation",
    );
    login(remote, &alice).await;
    let after_revocation = space.doc_entry(&alice, 0, doc, 3, space.epoch, b"after the revocation");
    expect_err(
        remote.append(feed, &after_revocation).await,
        is_rekey_needed,
        "append while a revocation is not covered",
    );
    // A Rekey that names Carol's log before the revocation does not cover it...
    space.now = fx.now();
    let stale = space.rekey(&alice, &[&alice, &with_two_devices]);
    post_ok(
        remote,
        &mut space,
        &alice,
        &stale,
        "a Rekey over the old log of Carol",
    )
    .await;
    login(remote, &alice).await;
    expect_err(
        remote
            .append(
                feed,
                &space.doc_entry(&alice, 0, doc, 3, space.epoch, b"after a stale Rekey"),
            )
            .await,
        is_rekey_needed,
        "append after a Rekey that does not cover the revocation",
    );
    // ...one that names the log after it does.
    let covering = space.rekey(&alice, &[&alice, &carol]);
    post_ok(
        remote,
        &mut space,
        &alice,
        &covering,
        "a Rekey over the new log of Carol",
    )
    .await;
    login(remote, &alice).await;
    ok(
        remote
            .append(
                feed,
                &space.doc_entry(&alice, 0, doc, 3, space.epoch, b"after the covering Rekey"),
            )
            .await,
        "append after the covering Rekey",
    );
    // The revoked device cannot sign membership entries any more.
    let from_revoked = space.entry_at(
        &carol,
        0,
        space.next_position().0,
        space.next_position().1,
        space.epoch,
        EntryBody::Profile,
        &Private::Profile {
            display_name: "Carol".to_owned(),
        },
    );
    login(remote, &carol).await;
    expect_err(
        remote
            .membership_append(feed, &[from_revoked.encode()])
            .await,
        is_forbidden,
        "a membership entry signed by a revoked device",
    );
}

/// An append whose `key_epoch` is below the feed's epoch gets `StaleEpoch` (the client re-seals);
/// a duplicate of an entry stored earlier is still answered; snapshots keep their epoch.
pub async fn check_stale_epoch<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Epochs");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    let doc = fx.doc();
    let first_epoch = space.epoch;
    login(remote, &alice).await;
    let early = space.doc_entry(&alice, 0, doc, 1, first_epoch, b"written at epoch 1");
    let (cursor, chain) = ok(
        remote.append(feed, &early).await,
        "append at the current epoch",
    );
    space.now = fx.now();
    let rekey = space.rekey(&alice, &[&alice, &bob]);
    post_ok(remote, &mut space, &alice, &rekey, "the Rekey").await;
    login(remote, &bob).await;
    expect_err(
        remote
            .append(
                feed,
                &space.doc_entry(&bob, 0, doc, 1, first_epoch, b"stale"),
            )
            .await,
        is_stale_epoch,
        "append sealed under an old epoch",
    );
    expect_err(
        remote
            .append(feed, &space.keyring_entry(&bob, 0, 1, first_epoch))
            .await,
        is_stale_epoch,
        "a keyring entry sealed under an old epoch",
    );
    ok(
        remote
            .append(
                feed,
                &space.doc_entry(&bob, 0, doc, 1, space.epoch, b"fresh"),
            )
            .await,
        "append at the new epoch",
    );
    ok(
        remote
            .append(
                feed,
                &space.doc_entry(&bob, 0, doc, 2, space.epoch + 1, b"ahead"),
            )
            .await,
        "append sealed under a newer epoch than the feed's",
    );
    // The entry stored before the Rekey is answered again (a retry after a lost answer).
    login(remote, &alice).await;
    assert_eq!(
        ok(
            remote.append(feed, &early).await,
            "a duplicate of an older entry"
        ),
        (cursor, chain),
        "conformance: a duplicate answers the stored cursor and chain"
    );
    // A snapshot of an unchanged document keeps its old epoch (document keys rotate lazily).
    ok(
        remote
            .put_snapshot(
                feed,
                &space.doc_snapshot(&alice, 0, fx.doc(), 1, first_epoch, b"unchanged"),
            )
            .await,
        "put_snapshot under an old epoch",
    );
}

/// A member may make `REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY` Rekeys a day; the next is
/// `Unavailable` (429), and works again a day later when the clock can move.
pub async fn check_rekey_rate_limit<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice] = enroll_n::<R, 1>(remote, fx).await;
    let mut space = fx.space("Rekey limit");
    found(remote, &mut space, &alice).await;
    login(remote, &alice).await;
    for round in 0..REKEYS_PER_ACCOUNT_PER_FEED_PER_DAY {
        let rekey = space.rekey(&alice, &[&alice]);
        post_ok(
            remote,
            &mut space,
            &alice,
            &rekey,
            &format!("Rekey {round}"),
        )
        .await;
    }
    let one_more = space.rekey(&alice, &[&alice]);
    expect_err(
        post(remote, &mut space, &alice, &one_more).await,
        is_unavailable,
        "the Rekey beyond the daily limit",
    );
    if fx.has_control() {
        fx.advance(DAY_MS + HOUR_MS);
        space.now = fx.now();
        let later = space.rekey(&alice, &[&alice]);
        post_ok(remote, &mut space, &alice, &later, "a Rekey a day later").await;
    }
}

/// Invites: single use, bound to an account, revoked, expired on the server's clock, void when their
/// Owner is removed, the horizon of 30 days on the server's clock, a forged proof; every dead
/// invite gets the same `NotFound`.
pub async fn check_invite_lifecycle<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob, carol, dave, erin, frank] = enroll_n::<R, 6>(remote, fx).await;
    let mut space = fx.space("Invites");
    found(remote, &mut space, &alice).await;
    let feed = space.feed;
    let server_epoch = server_epoch(remote).await;
    space.now = fx.now();
    let week = fx.now() + 7 * DAY_MS;
    let open = |secret: &InviteSecret| Space::open_sig(secret, server_epoch);

    // Open, join, and use it up.
    let (invite, s1) = space.invite(&alice, MemberRole::Editor, week, None);
    post_ok(remote, &mut space, &alice, &invite, "the Invite").await;
    let opened = ok(
        remote.invite_open(invite_id(&s1), &open(&s1)).await,
        "invite_open of a live invite",
    );
    assert_eq!(opened.feed, feed, "conformance: invite_open names the feed");
    assert_eq!(
        opened.log.last().map(|item| item.bytes.clone()),
        Some(invite.encode()),
        "conformance: invite_open answers the log up to the Invite"
    );
    assert_eq!(
        opened.log.len(),
        2,
        "conformance: the Create and the Invite"
    );
    let stranger_secret = InviteSecret::generate(&SeededRng::new(fx.seed() ^ 0x5ec7));
    expect_err(
        remote
            .invite_open(invite_id(&s1), &open(&stranger_secret))
            .await,
        is_not_found,
        "invite_open with a signature of another secret",
    );
    expect_err(
        remote
            .invite_open(invite_id(&stranger_secret), &open(&stranger_secret))
            .await,
        is_not_found,
        "invite_open of an unknown invite",
    );
    let join = space.join(&bob, &s1);
    post_ok(remote, &mut space, &bob, &join, "the Join").await;
    expect_err(
        remote.invite_open(invite_id(&s1), &open(&s1)).await,
        is_not_found,
        "invite_open of a used invite",
    );
    let replay = space.join(&carol, &s1);
    expect_err(
        post(remote, &mut space, &carol, &replay).await,
        is_not_found,
        "a Join with a used invite",
    );
    expect_err(
        post(remote, &mut space, &bob, &join).await,
        is_conflict,
        "a replay of an accepted Join",
    );

    // Revoked.
    let (invite, s2) = space.invite(&alice, MemberRole::Viewer, week, None);
    post_ok(remote, &mut space, &alice, &invite, "the second Invite").await;
    let revoke = space.invite_revoke(&alice, invite_id(&s2));
    post_ok(remote, &mut space, &alice, &revoke, "the InviteRevoke").await;
    expect_err(
        remote.invite_open(invite_id(&s2), &open(&s2)).await,
        is_not_found,
        "invite_open of a revoked invite",
    );
    let late = space.join(&carol, &s2);
    expect_err(
        post(remote, &mut space, &carol, &late).await,
        is_not_found,
        "a Join with a revoked invite",
    );

    // Bound to an account.
    let (invite, s3) = space.invite(&alice, MemberRole::Editor, week, Some(dave.account));
    post_ok(remote, &mut space, &alice, &invite, "the bound Invite").await;
    let wrong_account = space.join(&erin, &s3);
    expect_err(
        post(remote, &mut space, &erin, &wrong_account).await,
        is_not_found,
        "a Join of another account with a bound invite",
    );
    let bound_join = space.join(&dave, &s3);
    post_ok(
        remote,
        &mut space,
        &dave,
        &bound_join,
        "the Join of the bound account",
    )
    .await;

    // A forged proof: the Join names a live invite but is proven with another secret.
    let (invite, s4) = space.invite(&alice, MemberRole::Editor, week, None);
    post_ok(remote, &mut space, &alice, &invite, "the fourth Invite").await;
    let forged = space.join_forged(&erin, invite_id(&s4), &stranger_secret);
    expect_err(
        post(remote, &mut space, &erin, &forged).await,
        is_not_found,
        "a Join with a forged proof",
    );

    // The horizon is 30 days on the server's clock, whatever the entry's own time says.
    let real_now = space.now;
    space.now = fx.now() + 60 * DAY_MS;
    let (far, _) = space.invite(&alice, MemberRole::Editor, space.now + DAY_MS, None);
    space.now = real_now;
    expect_err(
        post(remote, &mut space, &alice, &far).await,
        is_protocol,
        "an Invite that expires beyond the server's 30-day horizon",
    );

    // An Owner who is removed takes their invites with them.
    let promote_invite = space.invite(&alice, MemberRole::Editor, week, None);
    post_ok(
        remote,
        &mut space,
        &alice,
        &promote_invite.0,
        "the Invite of Frank",
    )
    .await;
    let frank_join = space.join(&frank, &promote_invite.1);
    post_ok(remote, &mut space, &frank, &frank_join, "the Join of Frank").await;
    let owner = space.role_set(&alice, frank.account, MemberRole::Owner);
    post_ok(remote, &mut space, &alice, &owner, "Frank becomes an Owner").await;
    let (franks, s5) = space.invite(&frank, MemberRole::Viewer, week, None);
    post_ok(
        remote,
        &mut space,
        &frank,
        &franks,
        "the Invite of an Owner",
    )
    .await;
    ok(
        remote.invite_open(invite_id(&s5), &open(&s5)).await,
        "invite_open of an Owner's invite",
    );
    let remove = space.remove(&alice, frank.account, &[&alice, &bob, &dave]);
    post_ok(remote, &mut space, &alice, &remove, "the Remove of Frank").await;
    expect_err(
        remote.invite_open(invite_id(&s5), &open(&s5)).await,
        is_not_found,
        "invite_open of the invite of a removed Owner",
    );

    // Expiry on the server's clock.
    if fx.has_control() {
        space.now = fx.now();
        let (soon, s6) = space.invite(&alice, MemberRole::Editor, fx.now() + HOUR_MS, None);
        post_ok(remote, &mut space, &alice, &soon, "the short Invite").await;
        ok(
            remote.invite_open(invite_id(&s6), &open(&s6)).await,
            "invite_open before the expiry",
        );
        fx.advance(2 * HOUR_MS);
        space.now = fx.now();
        expect_err(
            remote.invite_open(invite_id(&s6), &open(&s6)).await,
            is_not_found,
            "invite_open of an expired invite",
        );
        let expired = space.join(&erin, &s6);
        expect_err(
            post(remote, &mut space, &erin, &expired).await,
            is_not_found,
            "a Join with an expired invite",
        );
    }
}

/// Share signup: an invite lets a newcomer create an account (one per invite, only the account a
/// bound invite names); a dead invite is `NotFound`, a bad signature `Authentication`, the limits
/// `Forbidden`.
pub async fn check_share_signup<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let info = ok(remote.server_info().await, "server_info");
    let [alice] = enroll_n::<R, 1>(remote, fx).await;
    let mut space = fx.space("Signups");
    found(remote, &mut space, &alice).await;
    let server_epoch = info.server_epoch;
    space.now = fx.now();
    let week = fx.now() + 7 * DAY_MS;
    let mut fresh = fx.people(20).into_iter();
    let mut next = || {
        fresh
            .next()
            .unwrap_or_else(|| panic!("conformance: out of fresh people"))
    };
    if info.share_signups == 0 {
        let (invite, secret) = space.invite(&alice, MemberRole::Editor, week, None);
        post_ok(remote, &mut space, &alice, &invite, "the Invite").await;
        let newcomer = next();
        expect_err(
            remote
                .account_create_share(
                    &newcomer.encoded_log(),
                    invite_id(&secret),
                    &Space::signup_sig(&secret, newcomer.account, server_epoch),
                )
                .await,
            is_forbidden,
            "a share signup on a server that allows none",
        );
        return;
    }

    // One account per invite: the second signup with an invite that is still open is refused.
    let (invite, secret) = space.invite(&alice, MemberRole::Editor, week, None);
    post_ok(remote, &mut space, &alice, &invite, "the Invite").await;
    let id = invite_id(&secret);
    let (first, second) = (next(), next());
    let sig = |person: &Person, secret: &InviteSecret| {
        Space::signup_sig(secret, person.account, server_epoch)
    };
    ok(
        remote
            .account_create_share(&first.encoded_log(), id, &sig(&first, &secret))
            .await,
        "a share signup",
    );
    expect_err(
        remote
            .account_create_share(&second.encoded_log(), id, &sig(&second, &secret))
            .await,
        is_forbidden,
        "a second signup with the same invite",
    );
    expect_err(
        remote
            .account_create_share(&first.encoded_log(), id, &sig(&first, &secret))
            .await,
        |e| matches!(e, SyncError::Forbidden | SyncError::Conflict(_)),
        "a signup of an account that exists",
    );
    // The new account is a normal one: it signs in, joins and lists the feed.
    login(remote, &first).await;
    let join = space.join(&first, &secret);
    post_ok(
        remote,
        &mut space,
        &first,
        &join,
        "the Join of a signed-up account",
    )
    .await;
    let listing = ok(remote.feeds_list().await, "feeds_list");
    assert!(
        listing.iter().any(|l| l.feed == space.feed),
        "conformance: the signed-up account lists the feed"
    );
    // The invite is used up: a dead invite.
    expect_err(
        remote
            .account_create_share(&second.encoded_log(), id, &sig(&second, &secret))
            .await,
        is_not_found,
        "a signup with a used invite",
    );

    // A bad signature, a revoked invite, an unknown invite.
    let (invite, s2) = space.invite(&alice, MemberRole::Editor, week, None);
    post_ok(remote, &mut space, &alice, &invite, "the second Invite").await;
    let third = next();
    let other = InviteSecret::generate(&SeededRng::new(fx.seed() ^ 0x516));
    expect_err(
        remote
            .account_create_share(
                &third.encoded_log(),
                invite_id(&s2),
                &Space::signup_sig(&other, third.account, server_epoch),
            )
            .await,
        is_authentication,
        "a signup signed with another secret",
    );
    let revoke = space.invite_revoke(&alice, invite_id(&s2));
    post_ok(remote, &mut space, &alice, &revoke, "the InviteRevoke").await;
    expect_err(
        remote
            .account_create_share(&third.encoded_log(), invite_id(&s2), &sig(&third, &s2))
            .await,
        is_not_found,
        "a signup with a revoked invite",
    );
    expect_err(
        remote
            .account_create_share(
                &third.encoded_log(),
                invite_id(&other),
                &sig(&third, &other),
            )
            .await,
        is_not_found,
        "a signup with an unknown invite",
    );

    // A bound invite admits its account only.
    let (invite, s3) = space.invite(&alice, MemberRole::Editor, week, Some(third.account));
    post_ok(remote, &mut space, &alice, &invite, "the bound Invite").await;
    let fourth = next();
    expect_err(
        remote
            .account_create_share(&fourth.encoded_log(), invite_id(&s3), &sig(&fourth, &s3))
            .await,
        is_forbidden,
        "a signup of another account with a bound invite",
    );
    ok(
        remote
            .account_create_share(&third.encoded_log(), invite_id(&s3), &sig(&third, &s3))
            .await,
        "the signup of the bound account",
    );

    // The per-inviter limit: Alice's invites create at most that many accounts.
    let per_account = usize::try_from(info.share_signups_per_account).unwrap_or(usize::MAX);
    if per_account <= 12 {
        let mut created = 2usize; // `first` and `third`
        let mut refused = false;
        for _ in 0..=per_account {
            let (invite, secret) = space.invite(&alice, MemberRole::Viewer, week, None);
            post_ok(
                remote,
                &mut space,
                &alice,
                &invite,
                "an Invite for the limit",
            )
            .await;
            let newcomer = next();
            match remote
                .account_create_share(
                    &newcomer.encoded_log(),
                    invite_id(&secret),
                    &sig(&newcomer, &secret),
                )
                .await
            {
                Ok(()) => created += 1,
                Err(SyncError::Forbidden) => {
                    refused = true;
                    break;
                }
                other => panic!("conformance: a signup at the limit answered {other:?}"),
            }
        }
        assert!(
            refused,
            "conformance: the per-inviter signup limit ({per_account}) never refused"
        );
        assert!(
            created <= per_account,
            "conformance: {created} signups by one inviter, the limit is {per_account}"
        );
    }
}

/// The total signup limit: a server that allows `n` share signups refuses the next, whoever invites.
/// Needs a fresh remote with the limit set to a small `n` (at most 10) that is not above the
/// per-inviter limit; not part of [`run_all_v2`].
pub async fn check_share_signup_total_limit<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let info = ok(remote.server_info().await, "server_info");
    let total = usize::try_from(info.share_signups).unwrap_or(usize::MAX);
    assert!(
        (1..=10).contains(&total),
        "conformance: this check needs a small total signup limit"
    );
    let [alice] = enroll_n::<R, 1>(remote, fx).await;
    let mut space = fx.space("Total signups");
    found(remote, &mut space, &alice).await;
    space.now = fx.now();
    let week = fx.now() + 7 * DAY_MS;
    let mut refused = false;
    let mut created = 0usize;
    for newcomer in fx.people(total + 2) {
        let (invite, secret) = space.invite(&alice, MemberRole::Viewer, week, None);
        post_ok(remote, &mut space, &alice, &invite, "an Invite").await;
        let result = remote
            .account_create_share(
                &newcomer.encoded_log(),
                invite_id(&secret),
                &Space::signup_sig(&secret, newcomer.account, info.server_epoch),
            )
            .await;
        match result {
            Ok(()) => created += 1,
            Err(SyncError::Forbidden) => {
                refused = true;
                break;
            }
            other => panic!("conformance: a signup answered {other:?}"),
        }
    }
    assert!(refused, "conformance: the total signup limit never refused");
    assert_eq!(
        created, total,
        "conformance: the total limit counts accounts"
    );
}

/// The quota of a shared feed counts against its billing account (the creator) and passes to the
/// earliest-joined Owner when the creator leaves. Needs a remote announcing a quota of at most 12
/// MiB (it skips itself otherwise): run it on a small-quota remote.
pub async fn check_quota_passes_to_owner<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let info = ok(remote.server_info().await, "server_info");
    let Some(quota) = info.account_quota_bytes.filter(|q| *q <= 12 * 1024 * 1024) else {
        return;
    };
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut space = fx.space("Billing");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    space.now = fx.now();
    let promote = space.role_set(&alice, bob.account, MemberRole::Owner);
    post_ok(remote, &mut space, &alice, &promote, "RoleSet to Owner").await;
    let feed = space.feed;
    let size = usize::try_from(quota * 3 / 5).unwrap_or(1);
    let (shared_chunk, shared_bytes) = fx.chunk(size);
    let (alice_personal, alice_bytes) = fx.chunk(size);
    let (bob_personal, bob_bytes) = fx.chunk(size);

    // Alice is billed for what the feed holds: a second large chunk of her own does not fit.
    login(remote, &alice).await;
    ok(
        remote.put_chunk(feed, shared_chunk, &shared_bytes).await,
        "put_chunk into the feed",
    );
    expect_err(
        remote
            .put_chunk(
                FeedId::from_account(alice.account),
                alice_personal,
                &alice_bytes,
            )
            .await,
        is_quota,
        "put_chunk beyond the quota of the billing account",
    );
    // She leaves; the earliest-joined remaining Owner, Bob, is billed from now on.
    let leave = space.leave(&alice);
    post_ok(remote, &mut space, &alice, &leave, "the creator leaves").await;
    ok(
        remote
            .put_chunk(
                FeedId::from_account(alice.account),
                alice_personal,
                &alice_bytes,
            )
            .await,
        "put_chunk by the former billing account",
    );
    login(remote, &bob).await;
    expect_err(
        remote
            .put_chunk(FeedId::from_account(bob.account), bob_personal, &bob_bytes)
            .await,
        is_quota,
        "put_chunk by the new billing account",
    );
    ok(
        remote.delete_chunk(feed, shared_chunk).await,
        "delete_chunk by the new Owner",
    );
    ok(
        remote
            .put_chunk(FeedId::from_account(bob.account), bob_personal, &bob_bytes)
            .await,
        "put_chunk once the feed's chunk is gone",
    );
}

/// Each feed has its own chain hash, cursors and heads.
pub async fn check_per_feed_chain_cursors_and_heads<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let server_epoch = server_epoch(remote).await;
    let mut one = fx.space("Chain one");
    let mut two = fx.space("Chain two");
    found(remote, &mut one, &alice).await;
    found(remote, &mut two, &alice).await;
    invite_and_join(remote, fx, &mut one, &alice, &bob, MemberRole::Editor).await;
    login(remote, &alice).await;
    let doc = fx.doc();
    let mut seq = 0;
    let mut stored = Vec::new();
    for round in 0..3u8 {
        for space in [&one, &two] {
            seq += 1;
            let entry = space.doc_entry(&alice, 0, doc, seq, space.epoch, &[round; 24]);
            let (cursor, chain) = ok(remote.append(space.feed, &entry).await, "append");
            stored.push((space.feed, entry, cursor, chain));
        }
    }
    // Cursors increase inside a feed, the chain is the running hash over the feed's items.
    for space in [&one, &two] {
        let items = read_feed(remote, space.feed).await;
        assert!(
            items.windows(2).all(|w| w[0].0 < w[1].0),
            "conformance: cursors increase inside a feed"
        );
        let mut chain = super::chain_genesis(&server_epoch);
        let mut expected = Vec::new();
        for (_, bytes) in &items {
            chain = super::chain_next(&chain, bytes);
            expected.push(chain);
        }
        let mut after = 0;
        let mut index = 0;
        loop {
            let page = ok(remote.changes(space.feed, after, 2).await, "changes");
            for (cursor, _) in &page.changes {
                after = *cursor;
                index += 1;
            }
            if let Some(last) = page.changes.last() {
                let _ = last;
                assert_eq!(
                    page.chain_hash_after,
                    expected[index - 1],
                    "conformance: the page's chain hash is the running hash"
                );
            }
            if !page.more {
                break;
            }
        }
        assert_eq!(index, items.len());
        for (feed, entry, cursor, chain) in &stored {
            if *feed == space.feed {
                assert!(
                    holds(&items, entry),
                    "conformance: the feed holds its entry"
                );
                assert!(
                    expected.contains(chain),
                    "conformance: an append answers the chain hash after it"
                );
                assert!(items.iter().any(|(c, _)| c == cursor));
            }
        }
    }
    let (_, first_chain) = feed_tip(remote, one.feed).await;
    let (_, second_chain) = feed_tip(remote, two.feed).await;
    assert_ne!(
        first_chain, second_chain,
        "conformance: feeds chain separately"
    );
    // Idempotent append.
    let (feed, entry, cursor, chain) = &stored[0];
    assert_eq!(
        ok(remote.append(*feed, entry).await, "a duplicate append"),
        (*cursor, *chain)
    );

    // Heads are per feed and per device; a head replaces the device's previous one.
    let (tip_one, chain_one) = feed_tip(remote, one.feed).await;
    let (tip_two, chain_two) = feed_tip(remote, two.feed).await;
    ok(
        remote
            .put_head(
                one.feed,
                &one.head_of(&alice, 0, tip_one, chain_one, server_epoch),
            )
            .await,
        "put_head one",
    );
    ok(
        remote
            .put_head(
                two.feed,
                &two.head_of(&alice, 0, tip_two, chain_two, server_epoch),
            )
            .await,
        "put_head two",
    );
    login(remote, &bob).await;
    ok(
        remote
            .put_head(
                one.feed,
                &one.head_of(&bob, 0, tip_one, chain_one, server_epoch),
            )
            .await,
        "put_head by Bob",
    );
    let heads_one = ok(remote.heads(one.feed).await, "heads one");
    assert_eq!(
        heads_one.len(),
        2,
        "conformance: one head per device in feed one"
    );
    for bytes in &heads_one {
        let head = super::peek_head(bytes).unwrap_or_else(|e| panic!("conformance: head: {e:?}"));
        assert_eq!(head.feed, one.feed, "conformance: a head names its feed");
    }
    login(remote, &alice).await;
    let heads_two = ok(remote.heads(two.feed).await, "heads two");
    assert_eq!(heads_two.len(), 1, "conformance: feed two holds one head");
    ok(
        remote
            .put_head(
                one.feed,
                &one.head_of(&alice, 0, tip_one, chain_one, server_epoch),
            )
            .await,
        "put_head again",
    );
    assert_eq!(
        ok(remote.heads(one.feed).await, "heads one").len(),
        2,
        "conformance: a head replaces the device's previous one"
    );
    // Refused heads.
    expect_err(
        remote
            .put_head(
                one.feed,
                &one.head_of(&alice, 0, tip_one + 10_000, chain_one, server_epoch),
            )
            .await,
        is_protocol,
        "a head past the feed's cursor",
    );
    expect_err(
        remote
            .put_head(
                one.feed,
                &two.head_of(&alice, 0, tip_two, chain_two, server_epoch),
            )
            .await,
        is_protocol,
        "a head naming another feed",
    );
    expect_err(
        remote
            .put_head(
                one.feed,
                &signed_head(
                    one.feed,
                    &bob,
                    0,
                    tip_one,
                    chain_one,
                    server_epoch,
                    fx.now(),
                ),
            )
            .await,
        is_forbidden,
        "a head of another account",
    );
    let mut forged = one.head_of(&alice, 0, tip_one, chain_one, server_epoch);
    if let Some(last) = forged.last_mut() {
        *last ^= 1;
    }
    expect_err(
        remote.put_head(one.feed, &forged).await,
        is_authentication,
        "a head with a bad signature",
    );
}

/// Chunks are isolated between feeds (and the personal namespace); a chunk copies from the
/// personal namespace; a chunk that does not hash to its id is refused.
pub async fn check_chunk_isolation_and_copy<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let mut one = fx.space("Chunks one");
    let mut two = fx.space("Chunks two");
    found(remote, &mut one, &alice).await;
    found(remote, &mut two, &alice).await;
    invite_and_join(remote, fx, &mut one, &alice, &bob, MemberRole::Editor).await;
    let personal = FeedId::from_account(alice.account);
    login(remote, &alice).await;
    let (shared, shared_bytes) = fx.chunk(256);
    ok(
        remote.put_chunk(one.feed, shared, &shared_bytes).await,
        "put_chunk",
    );
    ok(
        remote.put_chunk(one.feed, shared, &shared_bytes).await,
        "put_chunk again",
    );
    assert_eq!(
        ok(remote.get_chunk(one.feed, shared).await, "get_chunk"),
        Some(shared_bytes.clone())
    );
    assert_eq!(
        ok(
            remote.get_chunk(two.feed, shared).await,
            "get_chunk of another feed"
        ),
        None,
        "conformance: a chunk of one feed is not in another"
    );
    assert_eq!(
        ok(
            remote.get_chunk(personal, shared).await,
            "get_chunk of the personal feed"
        ),
        None,
        "conformance: a chunk of a feed is not in the personal namespace"
    );
    expect_err(
        remote
            .put_chunk(one.feed, fx.chunk(16).0, b"not those bytes")
            .await,
        is_protocol,
        "put_chunk whose bytes do not hash to the id",
    );

    // Copy from the personal namespace into either feed.
    let (mine, mine_bytes) = fx.chunk(180);
    ok(
        remote.put_chunk(personal, mine, &mine_bytes).await,
        "put_chunk, personal",
    );
    assert_eq!(
        ok(
            remote.get_chunk(one.feed, mine).await,
            "get_chunk before the copy"
        ),
        None
    );
    ok(remote.copy_chunk(one.feed, mine).await, "copy_chunk");
    ok(remote.copy_chunk(one.feed, mine).await, "copy_chunk again");
    ok(
        remote.copy_chunk(two.feed, mine).await,
        "copy_chunk into another feed",
    );
    assert_eq!(
        ok(
            remote.get_chunk(one.feed, mine).await,
            "get_chunk after the copy"
        ),
        Some(mine_bytes.clone())
    );
    expect_err(
        remote.copy_chunk(one.feed, fx.chunk(8).0).await,
        is_not_found,
        "copy_chunk of a chunk the personal namespace lacks",
    );
    // Bob's personal namespace is his own: he cannot copy Alice's chunk.
    login(remote, &bob).await;
    expect_err(
        remote.copy_chunk(one.feed, mine).await,
        is_not_found,
        "copy_chunk of a chunk only another account's personal namespace holds",
    );
    expect_err(
        remote.delete_chunk(one.feed, shared).await,
        is_forbidden,
        "delete_chunk by an Editor",
    );
    // Owners delete from one feed; the copies elsewhere stay.
    login(remote, &alice).await;
    ok(remote.delete_chunk(one.feed, mine).await, "delete_chunk");
    assert_eq!(
        ok(
            remote.get_chunk(one.feed, mine).await,
            "get_chunk after the delete"
        ),
        None
    );
    assert_eq!(
        ok(
            remote.get_chunk(two.feed, mine).await,
            "get_chunk of the copy elsewhere"
        ),
        Some(mine_bytes.clone()),
        "conformance: a delete leaves the other feed's copy"
    );
    assert_eq!(
        ok(
            remote.get_chunk(personal, mine).await,
            "get_chunk of the original"
        ),
        Some(mine_bytes),
        "conformance: a delete leaves the personal original"
    );
    // The personal namespace belongs to the account's devices.
    ok(
        remote.delete_chunk(personal, mine).await,
        "delete_chunk, personal",
    );
    assert_eq!(
        ok(
            remote.get_chunk(personal, mine).await,
            "get_chunk after the delete"
        ),
        None
    );
}

/// Pruning waits for the acknowledgement of every active, non-idle device of every member, for the
/// grace period, and for disputes (ADR-0045). Needs the clock and a pruning pass
/// ([`Fixture::with_control`]); skipped without them.
pub async fn check_pruning_waits<R: RemoteStore>(remote: &R, fx: &Fixture) {
    if !fx.has_control() {
        return;
    }
    let [alice, bob, carol] = enroll_n::<R, 3>(remote, fx).await;
    let server_epoch = server_epoch(remote).await;
    let mut space = fx.space("Pruning");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    invite_and_join(remote, fx, &mut space, &alice, &carol, MemberRole::Editor).await;
    let feed = space.feed;
    let epoch = space.epoch;
    let (plain, disputed) = (fx.doc(), fx.doc());
    login(remote, &alice).await;
    let mut entries = Vec::new();
    for (doc, base) in [(plain, 0u64), (disputed, 10)] {
        for seq in 1..=3u64 {
            let entry = space.doc_entry(&alice, 0, doc, base + seq, epoch, &[seq as u8; 16]);
            ok(remote.append(feed, &entry).await, "append");
            entries.push((doc, entry));
        }
    }
    let (tip, chain) = feed_tip(remote, feed).await;
    for doc in [plain, disputed] {
        ok(
            remote
                .put_snapshot(
                    feed,
                    &space.doc_snapshot(&alice, 0, doc, tip, epoch, b"state"),
                )
                .await,
            "put_snapshot",
        );
    }
    // Bob finds the second snapshot incomplete and disputes it.
    login(remote, &bob).await;
    ok(
        remote.snapshot_dispute(feed, disputed, tip).await,
        "snapshot_dispute",
    );
    let present = |items: &[(u64, Vec<u8>)], doc: DocId| {
        entries
            .iter()
            .filter(|(d, _)| *d == doc)
            .all(|(_, entry)| holds(items, entry))
    };
    let absent = |items: &[(u64, Vec<u8>)], doc: DocId| {
        entries
            .iter()
            .filter(|(d, _)| *d == doc)
            .all(|(_, entry)| !holds(items, entry))
    };
    let read = |person: Person| async move {
        login(remote, &person).await;
        read_feed(remote, feed).await
    };

    // Seven days later nobody has acknowledged: nothing is pruned.
    fx.advance(i64::from(PRUNE_ACK_DAYS) * DAY_MS + HOUR_MS);
    fx.prune();
    let items = read(alice.clone()).await;
    assert!(
        present(&items, plain),
        "conformance: pruning waits for acknowledgements"
    );
    // Alice and Bob acknowledge; Carol, a member with a device that is not idle, has not.
    for person in [&alice, &bob] {
        login(remote, person).await;
        ok(
            remote
                .put_head(feed, &space.head_of(person, 0, tip, chain, server_epoch))
                .await,
            "put_head",
        );
    }
    fx.prune();
    let items = read(alice.clone()).await;
    assert!(
        present(&items, plain),
        "conformance: pruning waits for a member's device that is not idle"
    );
    // A month later Carol's device is idle; the others sign in again (they are not).
    fx.advance(25 * DAY_MS);
    for person in [&alice, &bob] {
        login(remote, person).await;
    }
    fx.prune();
    let items = read(alice.clone()).await;
    assert!(
        absent(&items, plain),
        "conformance: pruning ignores a device that is idle"
    );
    assert!(
        present(&items, disputed),
        "conformance: a disputed snapshot's entries are kept"
    );
    assert!(
        items.iter().any(|(_, bytes)| {
            peek_feed_item(bytes).is_ok_and(|info| info.kind == ItemKind::Membership)
        }),
        "conformance: membership entries are never pruned"
    );
    // A newer snapshot replaces the disputed one; the entries it covers go after the grace period.
    login(remote, &alice).await;
    let (tip, chain) = feed_tip(remote, feed).await;
    ok(
        remote
            .put_snapshot(
                feed,
                &space.doc_snapshot(&alice, 0, disputed, tip, epoch, b"republished"),
            )
            .await,
        "put_snapshot of a full copy",
    );
    for person in [&alice, &bob] {
        login(remote, person).await;
        ok(
            remote
                .put_head(feed, &space.head_of(person, 0, tip, chain, server_epoch))
                .await,
            "put_head",
        );
    }
    fx.advance(i64::from(PRUNE_ACK_DAYS) * DAY_MS + HOUR_MS);
    for person in [&alice, &bob] {
        login(remote, person).await;
    }
    fx.prune();
    let items = read(alice.clone()).await;
    assert!(
        absent(&items, disputed),
        "conformance: a republished snapshot ends the dispute"
    );
}

/// A closed feed refuses writes with `Closed` and keeps reads for `CLOSE_READ_DAYS` (the deletion
/// needs the clock and a pruning pass, [`Fixture::with_control`]).
pub async fn check_closed_feed<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob] = enroll_n::<R, 2>(remote, fx).await;
    let server_epoch = server_epoch(remote).await;
    let mut space = fx.space("Closing");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    let feed = space.feed;
    let epoch = space.epoch;
    let doc = fx.doc();
    space.now = fx.now();
    let (open_invite, open_secret) =
        space.invite(&alice, MemberRole::Viewer, fx.now() + 7 * DAY_MS, None);
    post_ok(
        remote,
        &mut space,
        &alice,
        &open_invite,
        "an Invite before the Close",
    )
    .await;
    login(remote, &alice).await;
    let kept = space.doc_entry(&alice, 0, doc, 1, epoch, b"before the Close");
    ok(remote.append(feed, &kept).await, "append before the Close");
    let (chunk, bytes) = fx.chunk(48);
    ok(
        remote.put_chunk(feed, chunk, &bytes).await,
        "put_chunk before the Close",
    );
    let close = space.close(&alice);
    post_ok(remote, &mut space, &alice, &close, "the Close").await;

    login(remote, &bob).await;
    let (tip, chain) = feed_tip(remote, feed).await;
    expect_err(
        remote
            .append(feed, &space.doc_entry(&bob, 0, doc, 1, epoch, b"after"))
            .await,
        is_closed,
        "append to a closed feed",
    );
    expect_err(
        remote
            .put_snapshot(feed, &space.doc_snapshot(&bob, 0, doc, 1, epoch, b"s"))
            .await,
        is_closed,
        "put_snapshot to a closed feed",
    );
    expect_err(
        remote
            .put_head(feed, &space.head_of(&bob, 0, tip, chain, server_epoch))
            .await,
        is_closed,
        "put_head to a closed feed",
    );
    let (late_chunk, late_bytes) = fx.chunk(8);
    expect_err(
        remote.put_chunk(feed, late_chunk, &late_bytes).await,
        is_closed,
        "put_chunk to a closed feed",
    );
    let profile = space.profile(&bob, "Bob");
    expect_err(
        remote.membership_append(feed, &[profile.encode()]).await,
        is_closed,
        "a membership entry after the Close",
    );
    // Reads stay.
    let items = read_feed(remote, feed).await;
    assert!(
        holds(&items, &kept),
        "conformance: a closed feed is readable"
    );
    assert!(
        holds(&items, &close.encode()),
        "conformance: the Close is readable"
    );
    ok(remote.snapshots(feed).await, "snapshots of a closed feed");
    ok(
        remote.membership_get(feed, None).await,
        "membership_get of a closed feed",
    );
    assert_eq!(
        ok(
            remote.get_chunk(feed, chunk).await,
            "get_chunk of a closed feed"
        ),
        Some(bytes)
    );
    let listing = ok(remote.feeds_list().await, "feeds_list");
    let mine = listing
        .iter()
        .find(|l| l.feed == feed)
        .unwrap_or_else(|| panic!("conformance: a closed feed is still listed"));
    assert!(mine.closed, "conformance: the listing says closed");
    expect_err(
        remote
            .invite_open(
                invite_id(&open_secret),
                &Space::open_sig(&open_secret, server_epoch),
            )
            .await,
        is_not_found,
        "invite_open of an invite of a closed feed",
    );
    // Thirty days later the feed is deleted.
    if fx.has_control() {
        fx.advance(i64::from(CLOSE_READ_DAYS) * DAY_MS + DAY_MS);
        fx.prune();
        login(remote, &bob).await;
        expect_err(
            remote.changes(feed, 0, 10).await,
            is_not_found,
            "changes of a feed deleted after its Close",
        );
        let listing = ok(remote.feeds_list().await, "feeds_list");
        assert!(
            listing.iter().all(|l| l.feed != feed),
            "conformance: a deleted feed is not listed"
        );
    }
}

/// An account may create `MAX_FEEDS_PER_ACCOUNT` feeds; the next `Create` is `Forbidden`. Not part
/// of [`run_all_v2`]: it creates a hundred feeds.
pub async fn check_feed_limit<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice] = enroll_n::<R, 1>(remote, fx).await;
    login(remote, &alice).await;
    for round in 0..MAX_FEEDS_PER_ACCOUNT {
        let mut space = fx.space("Many");
        let create = space.create(&alice, 1);
        ok(
            remote.feeds_create(&create.encode()).await,
            &format!("feeds_create {round}"),
        );
    }
    let mut space = fx.space("One too many");
    let create = space.create(&alice, 1);
    expect_err(
        remote.feeds_create(&create.encode()).await,
        is_forbidden,
        "feeds_create beyond the per-account limit",
    );
}

/// `invite_open` allows `INVITE_OPEN_PER_IP_PER_HOUR` calls per client per hour, unknown invites
/// included; the next is `Unavailable` (429). It exhausts the limit, so [`run_all_v2`] runs it last.
pub async fn check_invite_open_rate_limit<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let server_epoch = server_epoch(remote).await;
    let secret = InviteSecret::generate(&SeededRng::new(fx.seed() ^ 0x7a7e));
    let id = invite_id(&secret);
    let sig = Space::open_sig(&secret, server_epoch);
    let mut limited = false;
    let attempts = INVITE_OPEN_PER_IP_PER_HOUR + 5;
    for _ in 0..attempts {
        match remote.invite_open(id, &sig).await {
            Err(SyncError::NotFound) => {}
            Err(SyncError::Unavailable(_)) => {
                limited = true;
                break;
            }
            other => panic!("conformance: invite_open of an unknown invite answered {other:?}"),
        }
    }
    assert!(limited, "conformance: invite_open was never rate limited");
}

// ----- view links (M13 slice 4, ADR-0051) -----------------------------------------------------

/// A link key of the checks (one per `tag`).
fn view_link_key(fx: &Fixture, tag: u64) -> LinkKey {
    LinkKey::generate(&SeededRng::new(fx.seed() ^ 0x11_4b00_0000 ^ tag))
}

/// A sealed publication of `key` (title "version `version`") whose attachments each have one of
/// `chunks`, the manifest a viewer would fetch them by.
fn view_publication(fx: &Fixture, key: &LinkKey, version: u64, chunks: &[ChunkId]) -> Vec<u8> {
    let attachments = chunks
        .iter()
        .zip(1u8..)
        .map(|(chunk, number)| PublishedAttachment {
            attachment_id: AttachmentId::from_bytes([number; 16]),
            key: Secret32::from_bytes([0x33; 32]),
            name: format!("photo{number}.png"),
            mime: "image/png".to_owned(),
            size: 10,
            hash: [number; 32],
            chunk_size: 4 * 1024 * 1024,
            chunks: vec![*chunk],
        })
        .collect();
    let publication = Publication {
        page_id: PageId::from_bytes([7; 16]),
        title: format!("version {version}"),
        icon: None,
        cover: None,
        cover_y: 50,
        snapshot: b"a snapshot without history".to_vec(),
        attachments,
        mentions: Vec::new(),
    };
    seal_publication(
        key,
        &publication,
        &SeededRng::new(fx.seed() ^ 0x9b_0000 ^ version),
    )
    .unwrap_or_else(|e| panic!("conformance: seal_publication failed: {e:?}"))
}

/// An upload of `publication` for `feed`.
fn link_upload(
    feed: FeedId,
    expires_at: Option<i64>,
    chunks: &[ChunkId],
    publication: Vec<u8>,
) -> LinkPut {
    LinkPut {
        feed,
        expires_at,
        chunks: chunks.to_vec(),
        publication,
    }
}

/// What a link read must answer: the publication with this `pub_seq` and these bytes.
fn expect_publication(
    result: Result<LinkFetch, SyncError>,
    pub_seq: u64,
    bytes: &[u8],
    what: &str,
) {
    match result {
        Ok(LinkFetch::Publication {
            pub_seq: got,
            bytes: served,
        }) if got == pub_seq && served == bytes => {}
        other => panic!(
            "conformance: {what}: expected the publication with pub_seq {pub_seq}, got {:?}",
            other.map(|fetch| match fetch {
                LinkFetch::Publication { pub_seq, bytes } => (Some(pub_seq), bytes.len()),
                LinkFetch::NotModified => (None, 0),
            })
        ),
    }
}

/// View links (ADR-0051): who may upload (an Editor or Owner of the feed, the account for its
/// personal feed; a Viewer or Commenter is `Forbidden`, a stranger `NotFound`), what an upload must
/// be (a publication of that link id, at most `LINK_MAX_CHUNKS` declared chunks, one feed per link),
/// the public reads (the publication, `If-None-Match`, only declared chunks the feed holds, one
/// `NotFound` for everything else), the first upload fixing the expiry (enforced on the server's
/// clock, which needs [`Fixture::with_control`]), revocation (the creator while a member, or an
/// Owner; the link outlives its creator's membership), and a closed feed's links gone. Makes about
/// 30 link reads, well under the per-client limit.
pub async fn check_view_links<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [alice, bob, carol, dave, erin] = enroll_n::<R, 5>(remote, fx).await;
    let mut space = fx.space("Links");
    found(remote, &mut space, &alice).await;
    invite_and_join(remote, fx, &mut space, &alice, &bob, MemberRole::Editor).await;
    invite_and_join(
        remote,
        fx,
        &mut space,
        &alice,
        &carol,
        MemberRole::Commenter,
    )
    .await;
    invite_and_join(remote, fx, &mut space, &alice, &dave, MemberRole::Viewer).await;
    let feed = space.feed;

    // The feed holds a chunk the link declares and one it does not; a third is declared but was
    // never uploaded.
    let (held, held_bytes) = fx.chunk(48);
    let (private, private_bytes) = fx.chunk(40);
    let (missing, _) = fx.chunk(32);
    login(remote, &alice).await;
    ok(
        remote.put_chunk(feed, held, &held_bytes).await,
        "put_chunk of a declared chunk",
    );
    ok(
        remote.put_chunk(feed, private, &private_bytes).await,
        "put_chunk of an undeclared chunk",
    );
    let declared = [held, missing];

    let key = view_link_key(fx, 1);
    let id = key.link_id();
    let v1 = view_publication(fx, &key, 1, &declared);
    let upload = |bytes: &[u8]| link_upload(feed, None, &declared, bytes.to_vec());

    // Who may upload: a Viewer and a Commenter are refused, a stranger and an unknown feed are one
    // NotFound; nothing was stored by any of them.
    for (who, what) in [(&dave, "a Viewer"), (&carol, "a Commenter")] {
        login(remote, who).await;
        expect_err(
            remote.link_put(id, &upload(&v1)).await,
            is_forbidden,
            &format!("link_put by {what}"),
        );
    }
    login(remote, &erin).await;
    expect_err(
        remote.link_put(id, &upload(&v1)).await,
        is_not_found,
        "link_put by a stranger",
    );
    login(remote, &bob).await;
    expect_err(
        remote
            .link_put(id, &link_upload(space.random_feed(), None, &[], v1.clone()))
            .await,
        is_not_found,
        "link_put to an unknown feed",
    );
    expect_err(
        remote.link_get(id, None).await,
        is_not_found,
        "link_get after refused uploads",
    );

    // What an upload must be: a publication of this link, at most LINK_MAX_CHUNKS declared chunks.
    let other_key = view_link_key(fx, 2);
    expect_err(
        remote
            .link_put(id, &upload(&view_publication(fx, &other_key, 1, &declared)))
            .await,
        is_protocol,
        "link_put of another link's publication",
    );
    expect_err(
        remote
            .link_put(
                id,
                &link_upload(feed, None, &vec![held; LINK_MAX_CHUNKS + 1], v1.clone()),
            )
            .await,
        is_protocol,
        "link_put declaring too many chunks",
    );
    expect_err(
        remote.link_get(id, None).await,
        is_not_found,
        "link_get after refused uploads",
    );

    // An Editor creates the link: the first upload is pub_seq 1.
    assert_eq!(
        ok(
            remote.link_put(id, &upload(&v1)).await,
            "the first link_put"
        ),
        1,
        "conformance: the first upload of a link is pub_seq 1"
    );

    // Public reads, as someone who is no member.
    login(remote, &erin).await;
    expect_publication(remote.link_get(id, None).await, 1, &v1, "link_get");
    assert_eq!(
        ok(remote.link_get(id, Some(1)).await, "link_get with a match"),
        LinkFetch::NotModified,
        "conformance: If-None-Match equal to pub_seq is NotModified"
    );
    expect_publication(
        remote.link_get(id, Some(7)).await,
        1,
        &v1,
        "link_get with another pub_seq",
    );
    assert_eq!(
        ok(
            remote.link_chunk_get(id, held).await,
            "link_chunk_get of a declared chunk the feed holds"
        ),
        held_bytes,
        "conformance: a declared chunk is served as the feed holds it"
    );
    expect_err(
        remote.link_chunk_get(id, private).await,
        is_not_found,
        "link_chunk_get of a chunk the link does not declare",
    );
    expect_err(
        remote.link_chunk_get(id, missing).await,
        is_not_found,
        "link_chunk_get of a declared chunk the feed does not hold",
    );
    let unknown = view_link_key(fx, 3).link_id();
    expect_err(
        remote.link_get(unknown, None).await,
        is_not_found,
        "link_get of an unknown link",
    );
    expect_err(
        remote.link_chunk_get(unknown, held).await,
        is_not_found,
        "link_chunk_get of an unknown link",
    );

    // Refreshes: an Editor and an Owner may upload again, each adds one to pub_seq; the link stays
    // in its feed.
    let v2 = view_publication(fx, &key, 2, &declared);
    let v3 = view_publication(fx, &key, 3, &declared);
    login(remote, &bob).await;
    assert_eq!(
        ok(
            remote.link_put(id, &upload(&v2)).await,
            "a refresh by the creator"
        ),
        2
    );
    login(remote, &alice).await;
    assert_eq!(
        ok(
            remote.link_put(id, &upload(&v3)).await,
            "a refresh by an Owner"
        ),
        3
    );
    expect_err(
        remote
            .link_put(
                id,
                &link_upload(
                    FeedId::from_account(alice.account),
                    None,
                    &declared,
                    v3.clone(),
                ),
            )
            .await,
        is_conflict,
        "link_put of a link into another feed",
    );
    expect_publication(
        remote.link_get(id, Some(2)).await,
        3,
        &v3,
        "link_get after refreshes",
    );
    assert_eq!(
        ok(remote.link_get(id, Some(3)).await, "link_get of the newest"),
        LinkFetch::NotModified
    );
    let opened = open_publication(&key, &v3)
        .unwrap_or_else(|e| panic!("conformance: the publication does not open: {e:?}"));
    assert_eq!(opened.title, "version 3");

    // A personal feed takes links from its account only.
    let personal = FeedId::from_account(alice.account);
    let personal_key = view_link_key(fx, 4);
    let personal_pub = view_publication(fx, &personal_key, 1, &[]);
    assert_eq!(
        ok(
            remote
                .link_put(
                    personal_key.link_id(),
                    &link_upload(personal, None, &[], personal_pub.clone())
                )
                .await,
            "link_put into the account's personal feed"
        ),
        1
    );
    login(remote, &bob).await;
    expect_err(
        remote
            .link_put(
                personal_key.link_id(),
                &link_upload(personal, None, &[], personal_pub),
            )
            .await,
        is_not_found,
        "link_put into another account's personal feed",
    );

    // Revocation: not a Commenter, a Viewer or a stranger; the creator may.
    for (who, check, what) in [
        (
            &carol,
            is_forbidden as fn(&SyncError) -> bool,
            "a Commenter",
        ),
        (&dave, is_forbidden, "a Viewer"),
        (&erin, is_not_found, "a stranger"),
    ] {
        login(remote, who).await;
        expect_err(
            remote.link_delete(id).await,
            check,
            &format!("link_delete by {what}"),
        );
    }
    expect_err(
        remote.link_delete(unknown).await,
        is_not_found,
        "link_delete of an unknown link",
    );
    expect_publication(
        remote.link_get(id, None).await,
        3,
        &v3,
        "link_get after refused deletes",
    );
    login(remote, &bob).await;
    ok(remote.link_delete(id).await, "link_delete by the creator");
    expect_err(
        remote.link_get(id, None).await,
        is_not_found,
        "link_get of a revoked link",
    );
    expect_err(
        remote.link_chunk_get(id, held).await,
        is_not_found,
        "link_chunk_get of a revoked link",
    );
    expect_err(
        remote.link_delete(id).await,
        is_not_found,
        "link_delete of a revoked link",
    );

    // An Owner revokes a link someone else made.
    let key2 = view_link_key(fx, 5);
    let pub2 = view_publication(fx, &key2, 1, &declared);
    ok(
        remote.link_put(key2.link_id(), &upload(&pub2)).await,
        "link_put of a second link",
    );
    login(remote, &alice).await;
    ok(
        remote.link_delete(key2.link_id()).await,
        "link_delete by an Owner",
    );
    expect_err(
        remote.link_get(key2.link_id(), None).await,
        is_not_found,
        "link_get of a link an Owner revoked",
    );

    // A creator demoted to Viewer is still a member and may revoke; a removed creator may not, and
    // the link outlives them until an Owner revokes it.
    let key3 = view_link_key(fx, 6);
    let pub3 = view_publication(fx, &key3, 1, &declared);
    login(remote, &bob).await;
    ok(
        remote.link_put(key3.link_id(), &upload(&pub3)).await,
        "link_put of a third link",
    );
    space.now = fx.now();
    let demote = space.role_set(&alice, bob.account, MemberRole::Viewer);
    post_ok(remote, &mut space, &alice, &demote, "the demotion").await;
    login(remote, &bob).await;
    ok(
        remote.link_delete(key3.link_id()).await,
        "link_delete by a creator who is a Viewer now",
    );
    let promote = space.role_set(&alice, bob.account, MemberRole::Editor);
    post_ok(remote, &mut space, &alice, &promote, "the promotion").await;
    let key4 = view_link_key(fx, 7);
    let pub4 = view_publication(fx, &key4, 1, &declared);
    login(remote, &bob).await;
    ok(
        remote.link_put(key4.link_id(), &upload(&pub4)).await,
        "link_put of a fourth link",
    );
    let remove = space.remove(&alice, bob.account, &[&alice, &carol, &dave]);
    post_ok(
        remote,
        &mut space,
        &alice,
        &remove,
        "the Remove of the creator",
    )
    .await;
    login(remote, &erin).await;
    expect_publication(
        remote.link_get(key4.link_id(), None).await,
        1,
        &pub4,
        "link_get of a link whose creator was removed",
    );
    login(remote, &bob).await;
    expect_err(
        remote.link_delete(key4.link_id()).await,
        is_not_found,
        "link_delete by a removed creator",
    );
    expect_err(
        remote.link_put(key4.link_id(), &upload(&pub4)).await,
        is_not_found,
        "link_put by a removed account",
    );
    login(remote, &alice).await;
    ok(
        remote.link_delete(key4.link_id()).await,
        "link_delete by an Owner of a removed creator's link",
    );

    // Expiry is the server's: the first upload fixes it, later uploads cannot move it.
    if fx.has_control() {
        let key5 = view_link_key(fx, 8);
        let pub5 = view_publication(fx, &key5, 1, &declared);
        let expires_at = fx.now() + HOUR_MS;
        login(remote, &alice).await;
        ok(
            remote
                .link_put(
                    key5.link_id(),
                    &link_upload(feed, Some(expires_at), &declared, pub5.clone()),
                )
                .await,
            "link_put of an expiring link",
        );
        let refreshed = view_publication(fx, &key5, 2, &declared);
        ok(
            remote
                .link_put(
                    key5.link_id(),
                    &link_upload(feed, None, &declared, refreshed),
                )
                .await,
            "a refresh that names no expiry",
        );
        fx.advance(HOUR_MS / 2);
        ok(
            remote.link_get(key5.link_id(), None).await,
            "link_get before the expiry",
        );
        ok(
            remote.link_chunk_get(key5.link_id(), held).await,
            "link_chunk_get before the expiry",
        );
        fx.advance(HOUR_MS / 2 + 1);
        expect_err(
            remote.link_get(key5.link_id(), None).await,
            is_not_found,
            "link_get of an expired link",
        );
        expect_err(
            remote.link_chunk_get(key5.link_id(), held).await,
            is_not_found,
            "link_chunk_get of an expired link",
        );
    }

    // Closing a feed deletes its links.
    let mut closing = fx.space("Links closing");
    found(remote, &mut closing, &alice).await;
    let closing_key = view_link_key(fx, 9);
    let closing_pub = view_publication(fx, &closing_key, 1, &[]);
    login(remote, &alice).await;
    ok(
        remote
            .link_put(
                closing_key.link_id(),
                &link_upload(closing.feed, None, &[], closing_pub.clone()),
            )
            .await,
        "link_put into a space that will close",
    );
    expect_publication(
        remote.link_get(closing_key.link_id(), None).await,
        1,
        &closing_pub,
        "link_get before the Close",
    );
    closing.now = fx.now();
    let close = closing.close(&alice);
    post_ok(remote, &mut closing, &alice, &close, "the Close").await;
    expect_err(
        remote.link_get(closing_key.link_id(), None).await,
        is_not_found,
        "link_get after the Close",
    );
    expect_err(
        remote
            .link_put(
                closing_key.link_id(),
                &link_upload(closing.feed, None, &[], closing_pub),
            )
            .await,
        is_closed,
        "link_put into a closed feed",
    );
}

/// `link_get` and `link_chunk_get` allow `LINK_GETS_PER_MINUTE_PER_IP` calls per client per minute
/// together, unknown links included; the next is `Unavailable` (429). It exhausts the limit, so
/// [`run_all_v2`] runs it last, after [`check_invite_open_rate_limit`].
pub async fn check_link_read_rate_limit<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let id = view_link_key(fx, 0x7a7e).link_id();
    let (hash, _) = fx.chunk(8);
    let mut limited = false;
    for round in 0..LINK_GETS_PER_MINUTE_PER_IP + 5 {
        // The two calls share one budget.
        let answer = if round % 2 == 0 {
            remote.link_get(id, None).await.map(|_| ())
        } else {
            remote.link_chunk_get(id, hash).await.map(|_| ())
        };
        match answer {
            Err(SyncError::NotFound) => {}
            Err(SyncError::Unavailable(_)) => {
                limited = true;
                break;
            }
            other => panic!("conformance: a read of an unknown link answered {other:?}"),
        }
    }
    assert!(limited, "conformance: link reads were never rate limited");
}

/// The pairing mailbox (ADR-0022 section 1), which needs no session: message 0 is the opener's
/// offer; posts get the next sequence numbers; `after` counts the messages the caller already has;
/// the size and role limits are `Protocol`; after `pair_close` the messages stay readable with
/// `closed` set and posting is `Conflict`; unknown mailboxes are `NotFound`. Opens one mailbox (and
/// tries one oversized offer) and makes 13 mailbox requests, all with `wait_seconds = 0`. The
/// 64-message limit and expiry are left to each remote's own tests (filling a mailbox exceeds a
/// server's per-mailbox request rate; expiry needs its clock).
pub async fn check_pairing_mailbox<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let message = |seq: u32, role: u8, bytes: &[u8]| PairingMessage {
        seq,
        role,
        bytes: bytes.to_vec(),
    };
    let offer = fx.chunk(64).1;
    let (id, expires_at) = ok(remote.pair_create(&offer).await, "pair_create");
    let page = ok(remote.pair_messages(id, 0, 0).await, "pair_messages");
    assert_eq!(
        page.messages,
        vec![message(0, PAIR_ROLE_OPENER, &offer)],
        "conformance: message 0 must be the offer, role opener"
    );
    assert_eq!(page.expires_at, expires_at);
    assert!(!page.closed);

    let answer = fx.chunk(48).1;
    ok(
        remote.pair_post(id, PAIR_ROLE_TYPIST, &answer).await,
        "pair_post",
    );
    let largest = vec![0x5a; PAIRING_MAX_MESSAGE_BYTES];
    ok(
        remote.pair_post(id, PAIR_ROLE_OPENER, &largest).await,
        "pair_post of the largest message",
    );
    let page = ok(remote.pair_messages(id, 1, 0).await, "pair_messages");
    assert_eq!(
        page.messages,
        vec![
            message(1, PAIR_ROLE_TYPIST, &answer),
            message(2, PAIR_ROLE_OPENER, &largest)
        ],
        "conformance: pair_messages(after = 1) answers the messages from seq 1"
    );
    let rest = ok(remote.pair_messages(id, 3, 0).await, "pair_messages");
    assert!(rest.messages.is_empty() && !rest.closed);
    assert_eq!(
        ok(remote.pair_messages(id, 0, 0).await, "pair_messages")
            .messages
            .len(),
        3
    );
    expect_err(
        remote
            .pair_post(
                id,
                PAIR_ROLE_TYPIST,
                &vec![0u8; PAIRING_MAX_MESSAGE_BYTES + 1],
            )
            .await,
        is_protocol,
        "pair_post beyond the message size",
    );
    expect_err(
        remote.pair_post(id, 2, b"no such role").await,
        is_protocol,
        "pair_post with an unknown role",
    );

    ok(remote.pair_close(id).await, "pair_close");
    let closed = ok(remote.pair_messages(id, 0, 0).await, "pair_messages");
    assert!(closed.closed, "conformance: a closed mailbox must say so");
    assert_eq!(
        closed.messages.len(),
        3,
        "conformance: closing keeps the messages"
    );
    expect_err(
        remote.pair_post(id, PAIR_ROLE_TYPIST, b"late").await,
        is_conflict,
        "pair_post to a closed mailbox",
    );
    ok(remote.pair_close(id).await, "pair_close twice");

    let mut unknown = [0u8; 6];
    unknown.copy_from_slice(&fx.doc().as_bytes()[..6]);
    expect_err(
        remote.pair_messages(unknown, 0, 0).await,
        is_not_found,
        "pair_messages of an unknown mailbox",
    );
    expect_err(
        remote.pair_post(unknown, PAIR_ROLE_TYPIST, b"x").await,
        is_not_found,
        "pair_post to an unknown mailbox",
    );
    expect_err(
        remote.pair_close(unknown).await,
        is_not_found,
        "pair_close of an unknown mailbox",
    );
    expect_err(
        remote
            .pair_create(&vec![0u8; PAIRING_MAX_MESSAGE_BYTES + 1])
            .await,
        is_protocol,
        "pair_create with an oversized offer",
    );
}

/// Recovery blobs (ADR-0022 section 5): `recovery_get` needs no session; `recovery_put` stores and
/// replaces under the session's account (at most `RECOVERY_BLOB_MAX_BYTES`, `Protocol` beyond);
/// another account can neither replace nor delete the blob (`Unauthorized` or `Conflict`); the
/// owner deletes it, and deleting again is fine. Enrols two people and makes 4 `recovery_get`
/// calls.
pub async fn check_recovery_blob<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let [owner, other] = enroll_n::<R, 2>(remote, fx).await;
    let id = fx.chunk(32).0.to_bytes();
    assert_eq!(
        ok(remote.recovery_get(id).await, "recovery_get"),
        None,
        "conformance: an unknown recovery id holds nothing"
    );
    login(remote, &owner).await;
    ok(
        remote.recovery_put(id, &fx.chunk(200).1).await,
        "recovery_put",
    );
    let blob = vec![0x3c; RECOVERY_BLOB_MAX_BYTES];
    ok(
        remote.recovery_put(id, &blob).await,
        "recovery_put of a replacement at the size limit",
    );
    assert_eq!(
        ok(remote.recovery_get(id).await, "recovery_get"),
        Some(blob.clone()),
        "conformance: recovery_put must replace the account's blob"
    );
    expect_err(
        remote
            .recovery_put(
                fx.chunk(32).0.to_bytes(),
                &vec![0u8; RECOVERY_BLOB_MAX_BYTES + 1],
            )
            .await,
        is_protocol,
        "recovery_put beyond the size limit",
    );

    login(remote, &other).await;
    // The reference remote refuses with `Unauthorized`, the server with a 403 (`Forbidden`).
    expect_err(
        remote.recovery_delete(id).await,
        |e| is_unauthorized(e) || is_forbidden(e),
        "recovery_delete by another account",
    );
    expect_err(
        remote.recovery_put(id, b"not yours").await,
        |e| is_unauthorized(e) || is_forbidden(e) || is_conflict(e),
        "recovery_put over another account's blob",
    );
    assert_eq!(
        ok(remote.recovery_get(id).await, "recovery_get"),
        Some(blob),
        "conformance: another account changed the blob"
    );

    login(remote, &owner).await;
    ok(remote.recovery_delete(id).await, "recovery_delete");
    assert_eq!(
        ok(remote.recovery_get(id).await, "recovery_get"),
        None,
        "conformance: recovery_delete must remove the blob"
    );
    match remote.recovery_delete(id).await {
        Ok(()) | Err(SyncError::NotFound) => {}
        Err(e) => panic!("conformance: recovery_delete of a deleted blob: {e:?}"),
    }
}

/// Idle-device expiry (ADR-0022 section 6): a device whose last session is more than
/// `idle_device_days` old (server clock) is refused by `authenticate` with `Idle`; a device never
/// seen counts as seen now; each session resets the count. Not part of [`run_all_v2`]: it needs a
/// remote whose `server_info` announces `idle_device_days` and a fixture [`Fixture::with_control`]
/// whose `advance_ms` moves the **server's** clock:
///
/// - `MemoryRemote`: `remote.set_idle_device_days(Some(30))`, then a fixture whose control calls
///   `remote.set_now_ms(remote.now_ms() + ms)`;
/// - `kb-server`: a server with `KB_SERVER_IDLE_DEVICE_DAYS` set and a test clock; the server's
///   other time limits (sessions, challenges) move with it, which the check tolerates because it
///   authenticates before every step.
///
/// Without the control it does nothing.
pub async fn check_idle_device<R: RemoteStore>(remote: &R, fx: &Fixture) {
    if !fx.has_control() {
        return;
    }
    let days = ok(remote.server_info().await, "server_info")
        .idle_device_days
        .unwrap_or_else(|| panic!("conformance: check_idle_device needs idle_device_days"));
    let limit = i64::from(days).saturating_mul(DAY_MS);
    let mut person = fx
        .people(1)
        .pop()
        .unwrap_or_else(|| panic!("conformance: one person"));
    person.add_device(fx.now());
    ok(
        remote
            .create_account(&fx.invite_token, &person.encoded_log())
            .await,
        "create_account",
    );
    let authenticate = async |device: usize| {
        let signer = person.signer(device);
        remote
            .authenticate(person.account, person.device_id(device), &signer)
            .await
    };
    ok(authenticate(0).await, "authenticate");
    fx.advance(limit + 60_000);
    ok(
        authenticate(1).await,
        "authenticate of a device never seen (it counts as seen now)",
    );
    expect_err(
        authenticate(0).await,
        |e| matches!(e, SyncError::Idle),
        "authenticate of a device idle beyond the limit",
    );
    fx.advance(limit - 60_000);
    ok(
        authenticate(1).await,
        "authenticate of a device within the limit",
    );
    fx.advance(120_000);
    ok(
        authenticate(1).await,
        "authenticate of a device whose last session restarted the count",
    );
    expect_err(
        authenticate(0).await,
        |e| matches!(e, SyncError::Idle),
        "authenticate of a device that stays idle",
    );
}

/// The server epoch is the same in `server_info`, in pages of changes and in the snapshots page,
/// call after call; the server speaks protocol 2 with the 16 MiB limit.
pub async fn check_epoch_constant<R: RemoteStore>(remote: &R, fx: &Fixture) {
    let first = ok(remote.server_info().await, "server_info");
    assert_eq!(first.protocol_version, kb_types::sync::PROTOCOL_VERSION);
    assert_eq!(first.max_wire_bytes, MAX_WIRE_BYTES);
    let [person] = enroll_n::<R, 1>(remote, fx).await;
    login(remote, &person).await;
    let feed = FeedId::from_account(person.account);
    let page = ok(remote.changes(feed, 0, 1).await, "changes");
    let snapshots = ok(remote.snapshots(feed).await, "snapshots");
    let second = ok(remote.server_info().await, "server_info");
    assert_eq!(
        second.server_epoch, first.server_epoch,
        "conformance: server_info epoch changed between calls"
    );
    assert_eq!(
        page.server_epoch, first.server_epoch,
        "conformance: changes answers another epoch"
    );
    assert_eq!(
        snapshots.server_epoch, first.server_epoch,
        "conformance: snapshots answers another epoch"
    );
}
