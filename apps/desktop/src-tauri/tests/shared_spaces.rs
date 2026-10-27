//! Shared spaces through the shell, end to end (Milestone 13, slice 1): people are `AppState<kb_core::Core>`
//! (the real core over a plain SQLite file, the shell's `driver_starter` as the background work: the v2 sync
//! driver, the device tasks and the invite tasks), one in-process `kb-server` on a loopback port is between
//! them, and the requests are the ones the UI sends through `dispatch`. What the OS does is played by the
//! shell's own functions: an invitation link goes through `deep_link::offer` and the `ShellEvents` channel,
//! and the test, as the UI does, hands it to `InviteLinkOpen`. What the driver and the tasks do
//! with scripted hosts is in `kb-net`'s tests, and with a bare core and the reference remote in
//! `kb-net/tests/fleet.rs`; these tests are the ones that need the shell's host methods to be real.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atomik_kb_desktop_lib::deep_link;
use atomik_kb_desktop_lib::{AppState, CoreHost, ShellEvent, ShellEvents, driver_starter};
use kb_core::{Core, CoreDeps};
use kb_keys::{DbKey, MemoryKeystore};
use kb_storage::{DbOpener, Encryption, MemoryBlobStore, OpenedDb, StorageError};
use kb_storage_native::SystemClock;
use kb_types::testing::SeededRng;
use kb_types::{
    ApiError, ErrorCode, Event, InstallId, InviteSessionInfo, InviteSessionState, MemberRole,
    MembersInfo, PageId, PresenceMessage, PresenceState, Request, Response, SpaceId, SpaceInfo,
    SpaceShareState, SyncInfo,
};

/// A plain SQLite file: the tests need no SQLCipher key.
struct PlainFileOpener(std::path::PathBuf);

impl DbOpener for PlainFileOpener {
    fn open(&self, _key: &DbKey) -> Result<OpenedDb, StorageError> {
        let conn = kb_storage::rusqlite::Connection::open(&self.0)?;
        Ok(OpenedDb {
            conn,
            encryption: Encryption::None,
        })
    }
}

/// An in-process `kb-server` on Tauri's runtime.
struct Server {
    base: String,
    _dir: tempfile::TempDir,
    _server: kb_server::Server,
}

fn start_server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let config = kb_server::Config {
        listen: ([127, 0, 0, 1], 0).into(),
        data_dir: dir.path().to_path_buf(),
        invite_tokens: vec!["dev".to_owned()],
        account_quota_bytes: 10 * 1024 * 1024 * 1024,
        prune_grace_days: 7,
        idle_device_days: None,
        trusted_proxies: Vec::new(),
        share_signups: 20,
        share_signups_per_account: 5,
        viewer_dir: None,
    };
    let server = kb_server::Server::open(&config).unwrap();
    let app = server
        .router()
        .into_make_service_with_connect_info::<SocketAddr>();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    tauri::async_runtime::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        base: format!("http://{addr}"),
        _dir: dir,
        _server: server,
    }
}

/// Waits (in real time: the driver and the tasks run on Tauri's runtime) for `condition`.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !condition() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One person's device: the shell's state over a real core, its background work running.
struct Person {
    state: Arc<AppState<Core>>,
    _dir: tempfile::TempDir,
}

impl Person {
    fn open(seed: u64) -> Self {
        Self::start(seed, true)
    }

    /// A device without the background work (no driver, no tasks).
    fn quiet(seed: u64) -> Self {
        Self::start(seed, false)
    }

    fn start(seed: u64, background: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut install = [0u8; 16];
        install[..8].copy_from_slice(&seed.to_le_bytes());
        let deps = CoreDeps {
            install_id: InstallId::from_random_bytes(install),
            platform: "windows".into(),
            data_dir: dir.path().display().to_string(),
            keystore: Box::new(MemoryKeystore::new()),
            opener: Box::new(PlainFileOpener(dir.path().join("workspace.db"))),
            clock: Box::new(SystemClock),
            rng: Box::new(SeededRng::new(seed)),
            lock_params: kb_crypto::Argon2Params {
                memory_kib: 8 * 1024,
                iterations: 1,
                parallelism: 1,
            },
            blob_store: Box::new(MemoryBlobStore::new()),
        };
        let core = Core::open(deps).unwrap();
        let state = Arc::new(AppState::<Core>::new());
        if background {
            state.set_background(driver_starter(Arc::downgrade(&state)));
        }
        state.set_ready(core);
        let person = Self { state, _dir: dir };
        person.try_send(Request::WorkspaceCreate {
            name: "Workspace".to_owned(),
            first_space_name: Some("Personal".to_owned()),
        });
        person.try_send(Request::RecoveryKitCreate);
        person
    }

    fn try_send(&self, request: Request) -> Response {
        self.state.dispatch(request).unwrap()
    }

    fn send(&self, request: Request) -> Result<Response, ApiError> {
        self.state.dispatch(request)
    }

    fn profile(&self, name: &str) {
        self.try_send(Request::ProfileSet {
            display_name: name.to_owned(),
        });
    }

    fn configure(&self, url: &str, token: Option<&str>) {
        self.try_send(Request::SyncConfigure {
            server_url: Some(url.to_owned()),
            invite_token: token.map(str::to_owned),
        });
    }

    fn space(&self, name: &str) -> SpaceId {
        match self.try_send(Request::SpaceCreate {
            name: name.to_owned(),
            icon: None,
        }) {
            Response::Space(space) => space.id,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn page(&self, space: SpaceId, parent: Option<PageId>, title: &str) -> PageId {
        match self.try_send(Request::PageCreate {
            space_id: space,
            parent_id: parent,
            index: None,
            title: title.to_owned(),
        }) {
            Response::Page(page) => page.id,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn info_of(&self, space: SpaceId) -> Option<SpaceInfo> {
        match self.try_send(Request::SpaceList) {
            Response::Spaces(spaces) => spaces.into_iter().find(|info| info.id == space),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn outline(&self, space: SpaceId) -> String {
        fn walk(nodes: &[kb_types::PageNode], depth: usize, out: &mut String) {
            for node in nodes {
                out.push_str(&"  ".repeat(depth));
                out.push_str(&node.page.title);
                out.push('\n');
                walk(&node.children, depth + 1, out);
            }
        }
        let Response::Tree(tree) = self.try_send(Request::TreeGet { space_id: space }) else {
            panic!("a tree");
        };
        let mut out = String::new();
        walk(&tree.roots, 0, &mut out);
        out
    }

    fn members(&self, space: SpaceId) -> MembersInfo {
        match self.try_send(Request::SpaceMembers { space_id: space }) {
            Response::Members(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn invite(&self) -> InviteSessionInfo {
        match self.try_send(Request::InviteStatus) {
            Response::Invite(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn sync(&self) -> SyncInfo {
        match self.try_send(Request::SyncStatus) {
            Response::Sync(info) => info,
            other => panic!("unexpected {other:?}"),
        }
    }

    /// Alice shares the space and the server takes the `Create`.
    fn share(&self, space: SpaceId) {
        self.try_send(Request::SpaceShare { space_id: space });
        wait_until("the server took the Create", || {
            self.info_of(space)
                .is_some_and(|info| info.share_state == SpaceShareState::Shared)
        });
    }
}

/// The messages a webview subscribed to the shell's channel would be sent.
fn subscribe(events: &ShellEvents) -> Arc<Mutex<Vec<ShellEvent>>> {
    let seen: Arc<Mutex<Vec<ShellEvent>>> = Arc::default();
    let sink = Arc::clone(&seen);
    events.subscribe(Box::new(move |event| {
        sink.lock().unwrap().push(event.clone());
        true
    }));
    seen
}

/// Alice with a profile, sync and a shared space `Project` of two pages.
fn alice_shares_a_project(server: &Server) -> (Person, SpaceId) {
    let alice = Person::open(1);
    alice.profile("Alice");
    alice.configure(&server.base, Some("dev"));
    let space = alice.space("Project");
    let plan = alice.page(space, None, "Plan");
    alice.page(space, Some(plan), "Notes");
    alice.share(space);
    (alice, space)
}

#[test]
fn an_invitation_link_the_os_opens_joins_a_shared_space_through_the_shell() {
    let server = start_server();
    let (alice, space) = alice_shares_a_project(&server);
    let bob = Person::open(2);
    bob.profile("Bob");

    // Alice makes a link; her driver puts the `Invite` on the server (the link is shared by hand).
    let Response::InviteLink(link) = alice.try_send(Request::InviteLinkCreate {
        space_id: space,
        role: MemberRole::Editor,
        expires_in_days: None,
    }) else {
        panic!("a link");
    };
    wait_until("the Invite is on the server", || {
        alice.sync().pending_entries == 0
            && alice
                .members(space)
                .invites
                .iter()
                .any(|invite| invite.status == kb_types::InviteStatus::Open)
    });

    // The OS hands Bob's app the link: the shell accepts it and the UI, once a workspace is open, gives it
    // to the core.
    let events = ShellEvents::new();
    let seen = subscribe(&events);
    assert!(deep_link::offer(&events, &link.link));
    let ShellEvent::InviteLink { link: received } = seen.lock().unwrap()[0].clone() else {
        panic!("an inviteLink message");
    };
    assert_eq!(received.as_str(), link.link);
    bob.try_send(Request::InviteLinkOpen {
        link: received.as_str().to_owned(),
    });

    wait_until("Bob sees the preview", || {
        bob.invite().state == InviteSessionState::Previewing
    });
    let preview = bob.invite();
    assert_eq!(preview.inviter_name.as_deref(), Some("Alice"));
    assert_eq!(preview.space_name.as_deref(), Some("Project"));
    assert_eq!(preview.role, Some(MemberRole::Editor));
    bob.try_send(Request::InviteConfirm);
    wait_until("Bob joined", || {
        bob.invite().state == InviteSessionState::Done
    });
    wait_until("Bob has the pages", || {
        bob.info_of(space)
            .is_some_and(|info| info.role == Some(MemberRole::Editor))
            && bob.outline(space) == "Plan\n  Notes\n"
    });
    // The account came from the invitation: no invite token was needed, nothing asks for one.
    wait_until("Bob's sync is quiet", || {
        let info = bob.sync();
        info.configured && !info.needs_invite_token && info.state == kb_types::SyncState::Idle
    });
    // And what Bob renames reaches Alice through the same hosts.
    let Response::Tree(tree) = bob.try_send(Request::TreeGet { space_id: space }) else {
        panic!("a tree");
    };
    let plan = tree.roots[0].page.id;
    bob.try_send(Request::PageRename {
        page_id: plan,
        title: "Plan v2".to_owned(),
    });
    wait_until("Alice sees Bob's rename", || {
        alice.outline(space).starts_with("Plan v2\n")
    });
}

#[test]
fn a_code_invite_runs_through_the_shell_hosts_with_the_same_digits_on_both_sides() {
    let server = start_server();
    let (alice, space) = alice_shares_a_project(&server);
    let carol = Person::open(3);
    carol.profile("Carol");

    alice.try_send(Request::InviteCodeStart {
        space_id: space,
        role: MemberRole::Commenter,
    });
    wait_until("the inviter shows the code", || {
        alice.invite().code.is_some()
    });
    carol.try_send(Request::InviteCodeJoin {
        server_url: server.base.clone(),
        code: alice.invite().code.unwrap(),
    });
    wait_until("both show the digits", || {
        alice.invite().state == InviteSessionState::Confirming
            && carol.invite().state == InviteSessionState::Confirming
    });
    let digits = alice.invite().sas.unwrap();
    assert_eq!(digits.len(), 6);
    assert_eq!(carol.invite().sas, Some(digits));
    carol.try_send(Request::InviteConfirm);
    wait_until("the inviter sees who is asking", || {
        alice.invite().state == InviteSessionState::PeerConfirming
    });
    assert_eq!(alice.invite().peer_name.as_deref(), Some("Carol"));
    alice.try_send(Request::InviteConfirm);
    wait_until("Carol joined", || {
        carol.invite().state == InviteSessionState::Done
    });
    wait_until("Alice's session saw the Join", || {
        alice.invite().state == InviteSessionState::Done
    });
    wait_until("Carol has the pages", || {
        carol
            .info_of(space)
            .is_some_and(|info| info.role == Some(MemberRole::Commenter))
            && carol.outline(space) == alice.outline(space)
    });
    // The digits verified the other person on both sides.
    let carol_view = carol.members(space);
    let alice_listed = carol_view
        .members
        .iter()
        .find(|member| member.name == "Alice")
        .expect("Alice is listed");
    assert!(alice_listed.verified);
}

#[test]
fn cancelling_an_invitation_through_the_requests_ends_the_session() {
    let server = start_server();
    let (alice, space) = alice_shares_a_project(&server);
    alice.try_send(Request::InviteCodeStart {
        space_id: space,
        role: MemberRole::Viewer,
    });
    wait_until("the inviter shows the code", || {
        alice.invite().code.is_some()
    });
    alice.try_send(Request::InviteCancel);
    assert_eq!(alice.invite().state, InviteSessionState::Cancelled);
    // A cancelled session stays as it ended, and a second cancel is nothing.
    alice.try_send(Request::InviteCancel);
    assert_eq!(alice.invite().state, InviteSessionState::Cancelled);
}

#[test]
fn a_bad_link_is_refused_by_the_core_and_starts_no_task() {
    let bob = Person::open(2);
    let error = bob
        .send(Request::InviteLinkOpen {
            link: "atomik-kb://invite?server=http%3A%2F%2F127.0.0.1%3A9#nope".to_owned(),
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput);
    assert_eq!(bob.invite().state, InviteSessionState::Idle);
}

/// What an invitee's driver must not do while the invitation is about to create its account: ask for an
/// invite token (the guard of `SyncCoreApi::sync_account_unknown`, which the shell's host calls).
#[test]
fn an_invitee_whose_account_the_invitation_creates_is_not_asked_for_an_invite_token() {
    // A person whose sync points at a server that does not know her (nothing listens: no task could
    // run, and the test looks at the core's answers only), without the background work.
    let carol = Person::quiet(4);
    carol.profile("Carol");
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    carol.configure(&dead, None);
    let asks = |person: &Person| {
        kb_net::v2::SyncHost::sync_account_unknown(&*person.state).unwrap();
        person.sync().needs_invite_token
    };
    // Without an invitation the driver's report stands: the status asks for a token.
    assert!(asks(&carol));
    carol.configure(&dead, None);
    assert!(
        !carol.sync().needs_invite_token,
        "configuring clears the request"
    );

    // With an invitation under way (a link whose secret is all zeros, which is well formed) it is not
    // recorded: the share signup will create the account.
    let link = format!(
        "atomik-kb://invite?server={}#{}",
        dead.replace(':', "%3A").replace('/', "%2F"),
        "0".repeat(52)
    );
    carol.try_send(Request::InviteLinkOpen { link });
    assert_eq!(carol.invite().state, InviteSessionState::Starting);
    assert!(!asks(&carol), "an invitee is not asked for a token");
    // The invitation ended: the plain rule is back.
    carol.try_send(Request::InviteCancel);
    assert!(asks(&carol));
}

#[test]
fn presence_crosses_the_server_sealed_and_reaches_the_other_members_channel_only() {
    let server = start_server();
    let (alice, space) = alice_shares_a_project(&server);
    let Response::Tree(tree) = alice.try_send(Request::TreeGet { space_id: space }) else {
        panic!("a tree");
    };
    let plan = tree.roots[0].page.id;
    let bob = Person::open(2);
    bob.profile("Bob");
    let Response::InviteLink(link) = alice.try_send(Request::InviteLinkCreate {
        space_id: space,
        role: MemberRole::Editor,
        expires_in_days: None,
    }) else {
        panic!("a link");
    };
    wait_until("the Invite is on the server", || {
        alice.sync().pending_entries == 0
            && alice
                .members(space)
                .invites
                .iter()
                .any(|invite| invite.status == kb_types::InviteStatus::Open)
    });
    bob.try_send(Request::InviteLinkOpen { link: link.link });
    wait_until("Bob sees the preview", || {
        bob.invite().state == InviteSessionState::Previewing
    });
    bob.try_send(Request::InviteConfirm);
    wait_until("Bob joined and has the pages", || {
        bob.invite().state == InviteSessionState::Done
            && bob
                .info_of(space)
                .is_some_and(|info| info.role == Some(MemberRole::Editor))
            && bob.outline(space) == "Plan\n  Notes\n"
    });

    // Each webview has registered its `presence_events` channel; Bob's core also tells its listeners (the
    // events channel) when somebody appeared.
    let channel = |person: &Person| -> Arc<Mutex<Vec<PresenceMessage>>> {
        let heard: Arc<Mutex<Vec<PresenceMessage>>> = Arc::default();
        let sink = Arc::clone(&heard);
        person.state.presence_subscribe(Box::new(move |message| {
            sink.lock().unwrap().push(message.clone());
            true
        }));
        heard
    };
    let (bob_heard, alice_heard) = (channel(&bob), channel(&alice));
    let changes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&changes);
    let _unsubscribe = bob.state.add_listener(Arc::new(move |event| {
        if matches!(event, Event::PresenceChanged { .. }) {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    }));

    // Alice is in the page. The sockets may still be coming up, so she goes on sending (at most ten a second
    // go through) until Bob's channel has heard one.
    wait_until("Bob's channel hears Alice", || {
        alice
            .state
            .presence_send(space, Some(plan), PresenceState::Editing, b"cursor")
            .unwrap();
        !bob_heard.lock().unwrap().is_empty()
    });
    let message = bob_heard.lock().unwrap()[0].clone();
    assert_eq!(message.info.name, "Alice");
    assert_eq!(message.info.page_id, Some(plan));
    assert_eq!(message.info.state, PresenceState::Editing);
    assert_eq!(message.update, b"cursor");
    assert!(
        alice_heard.lock().unwrap().is_empty(),
        "a frame never goes back to the sender's own device"
    );
    wait_until("Bob's core told its listeners", || {
        changes.load(Ordering::SeqCst) >= 1
    });
    // Who is where, as the UI asks for it.
    let Response::Presence(here) = bob.try_send(Request::PresenceList { space_id: space }) else {
        panic!("a presence list");
    };
    assert_eq!(here.len(), 1);
    assert_eq!(here[0].name, "Alice");

    // Bob's reply goes the other way, in no page.
    wait_until("Alice's channel hears Bob", || {
        bob.state
            .presence_send(space, None, PresenceState::Idle, b"")
            .unwrap();
        !alice_heard.lock().unwrap().is_empty()
    });
    assert_eq!(alice_heard.lock().unwrap()[0].info.name, "Bob");
    assert_eq!(alice_heard.lock().unwrap()[0].info.page_id, None);

    // A personal space has nobody to talk to: nothing is sent, and nothing fails.
    let Response::Spaces(spaces) = alice.try_send(Request::SpaceList) else {
        panic!("spaces");
    };
    let personal = spaces.iter().find(|info| info.id != space).unwrap().id;
    alice
        .state
        .presence_send(personal, None, PresenceState::Viewing, b"x")
        .unwrap();
}
