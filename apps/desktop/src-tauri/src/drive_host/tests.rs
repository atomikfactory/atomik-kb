//! Tests of the Drive host against a fake Google on `127.0.0.1` (`tests/fake_google.rs`): the sign-in with its
//! loopback redirect (driven here: a wrong state, a wrong `Host`, a stray request, a refusal, a timeout, a
//! cancel), the keystore (an in-memory one), the setting, disconnecting and choosing a client, the delivery
//! (chunks, a 308 in the middle, a lost answer resumed through the status query, giving up, pruning, a refusal
//! of the token), the listing and the download. Nothing reaches Google; the transport's base URLs are rewritten
//! for these tests only, production is HTTPS only (`net`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod fake_google;

use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kb_drive::CHUNK_SIZE;
use kb_keys::{Keystore, MemoryKeystore};
use kb_types::ErrorCode;
use kb_types::interop::{DriveClientInput, DriveClientSource, DriveConnection, DriveProgress};
use reqwest::Url;
use serde_json::json;

use super::*;
use crate::ai_test_support::{
    AiFake, FakeConfirm, SharedScript, closed_port, put_setting, ready_host, script,
};
use crate::state::AppState;
use crate::test_support::captured_logs_through;
use fake_google::{
    ACCOUNT, DownloadFault, Fake, GOOD_CODE, REFRESH_TOKEN, RefreshMode, UploadFault,
};

const TOKEN_ENTRY: &str = "gdrive:test-install";
const SECRET_ENTRY: &str = "gdrive-client:test-install";
const NEW_BACKUP: &str = "atomik-kb-auto-2026-10-13-0930.atomikkb";

/// Runs a future on a runtime of its own.
fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

// --- the world of a test: a fake Google, a scripted core, an in-memory keystore ---

struct World {
    dir: tempfile::TempDir,
    fake: Fake,
    shell: Arc<DriveShell>,
    keystore: Arc<MemoryKeystore>,
    host: Arc<dyn CoreHost>,
    core: SharedScript,
    _state: Arc<AppState<AiFake>>,
}

impl World {
    fn new() -> Self {
        Self::with(Some(("built-in-id", Some("built-in-secret"))), |_| {})
    }

    fn with(builtin: Option<(&str, Option<&str>)>, tweak: impl FnOnce(&mut Config)) -> Self {
        let fake = Fake::start();
        let dir = tempfile::tempdir().unwrap();
        let keystore = Arc::new(MemoryKeystore::new());
        let mut config = Config::local(&fake.base);
        tweak(&mut config);
        let shell = Arc::new(DriveShell::for_tests(
            dir.path(),
            Arc::clone(&keystore) as Arc<dyn Keystore>,
            builtin,
            config,
        ));
        let (state, host, core) = ready_host();
        Self {
            dir,
            fake,
            shell,
            keystore,
            host,
            core,
            _state: state,
        }
    }

    /// The setting `app.backup.drive` as the core holds it.
    fn setting(&self) -> DriveConnection {
        script(&self.core)
            .settings
            .get(BACKUP_DRIVE_SETTING)
            .cloned()
            .map(|value| serde_json::from_value(value).unwrap())
            .unwrap_or_default()
    }

    fn put_connection(&self, connection: &DriveConnection) {
        put_setting(
            &self.core,
            BACKUP_DRIVE_SETTING,
            serde_json::to_value(connection).unwrap(),
        );
    }

    fn token(&self) -> Option<String> {
        self.keystore
            .get(TOKEN_ENTRY)
            .unwrap()
            .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// A made connection: the folder in the fake, the refresh token in the keystore, the setting.
    fn seed(&self) -> String {
        let folder = self.fake.add_folder("atomik-kb backups");
        self.keystore
            .set(TOKEN_ENTRY, REFRESH_TOKEN.as_bytes())
            .unwrap();
        self.put_connection(&DriveConnection {
            account: Some(ACCOUNT.to_owned()),
            folder_id: Some(folder.clone()),
            connected_at: Some(1_000),
            ..DriveConnection::default()
        });
        folder
    }

    fn connect(
        &self,
        confirm: &FakeConfirm,
        browser: &Redirecting,
    ) -> Result<DriveStatus, ApiError> {
        run(connect_flow(&*self.host, &self.shell, confirm, browser))
    }

    fn deliver(&self, file: &Path, name: &str, keep: u32) -> Result<DriveDelivery, ApiError> {
        deliver(&*self.host, &self.shell, file, name, keep)
    }

    fn list(&self) -> Result<Vec<BackupFileInfo>, ApiError> {
        run(list_flow(&*self.host, &self.shell))
    }

    fn download(
        &self,
        file_id: &str,
        emit: &(dyn Fn(DriveProgress) -> bool + Send + Sync),
    ) -> Result<String, ApiError> {
        run(download_flow(&*self.host, &self.shell, file_id, emit))
    }

    fn staging(&self) -> PathBuf {
        self.dir.path().join(RESTORE_STAGING_DIR)
    }

    fn staged_files(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.staging())
            .map(|read| read.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default()
    }

    /// What the fake received, as `METHOD /path` (an upload session's id left out).
    fn sequence(&self) -> Vec<String> {
        self.fake
            .requests()
            .iter()
            .map(|request| {
                let path = if request.path.starts_with("/upload/session/") {
                    "/upload/session"
                } else if request.path.starts_with("/drive/v3/files/") {
                    "/drive/v3/files/<id>"
                } else {
                    request.path.as_str()
                };
                format!("{} {path}", request.method)
            })
            .collect()
    }

    /// The `Content-Range` of every upload request, in order.
    fn ranges(&self) -> Vec<String> {
        self.fake
            .requests()
            .iter()
            .filter(|request| request.method == "PUT")
            .map(|request| {
                request
                    .header("content-range")
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }
}

/// A backup file of `size` bytes of a pattern, in the world's directory.
fn backup_file(world: &World, name: &str, size: usize) -> (PathBuf, Vec<u8>) {
    let bytes: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
    let path = world.dir.path().join(name);
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

fn reason(error: &ApiError) -> &str {
    reason_of(error).unwrap_or("none")
}

fn is_reason(result: &Result<impl std::fmt::Debug, ApiError>, expected: &str) {
    let error = result.as_ref().unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidInput, "{error:?}");
    assert_eq!(reason(error), expected, "{error:?}");
    assert_eq!(error.details.as_ref().unwrap()["field"], "drive");
}

// --- the browser, as the test plays it ---

/// What the person's browser does with the sign-in's address.
#[derive(Clone, Copy)]
enum Behaviour {
    /// Comes back with a code and the right state.
    Good,
    /// Sends a wrong-Host request, a favicon request, a wrong state and a missing state first, then comes back right.
    Messy,
    /// Comes back with `error=access_denied`.
    Denied,
    /// Never comes back.
    Nothing,
    /// Opens a connection and says nothing on it (as a browser does beside its request), then comes back right.
    IdleFirst,
}

#[derive(Default)]
struct Seen {
    /// The addresses opened.
    urls: Vec<String>,
    /// The status of every answer the loopback gave, in order.
    statuses: Vec<u16>,
    /// The port of the loopback listener.
    port: Option<u16>,
    /// The state of the sign-in.
    state: Option<String>,
    /// The body of every answer, in order.
    bodies: Vec<String>,
}

struct Redirecting {
    behaviour: Behaviour,
    seen: Arc<Mutex<Seen>>,
}

impl Redirecting {
    fn new(behaviour: Behaviour) -> Self {
        Self {
            behaviour,
            seen: Arc::default(),
        }
    }

    fn seen<T>(&self, read: impl FnOnce(&Seen) -> T) -> T {
        read(&self.seen.lock().unwrap())
    }
}

/// One request to the loopback; answers its status code and body.
fn ask(port: u16, host: &str, request_line: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{request_line}\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = Vec::new();
    let _ = stream.read_to_end(&mut text);
    let text = String::from_utf8_lossy(&text);
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

impl Browser for Redirecting {
    fn open(&self, url: &str) -> Result<(), ApiError> {
        let parsed = Url::parse(url).unwrap();
        let query = |name: &str| {
            parsed
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .unwrap()
        };
        let redirect = Url::parse(&query("redirect_uri")).unwrap();
        let port = redirect.port().unwrap();
        let state = query("state");
        {
            let mut seen = self.seen.lock().unwrap();
            seen.urls.push(url.to_owned());
            seen.port = Some(port);
            seen.state = Some(state.clone());
        }
        let behaviour = self.behaviour;
        let seen = Arc::clone(&self.seen);
        std::thread::spawn(move || {
            let host = format!("127.0.0.1:{port}");
            let answer = |host: &str, line: &str| {
                let (status, body) = ask(port, host, line);
                let mut seen = seen.lock().unwrap();
                seen.bodies.push(body);
                seen.statuses.push(status);
            };
            let good = format!("GET /?code={GOOD_CODE}&state={state} HTTP/1.1");
            match behaviour {
                Behaviour::Nothing => {}
                Behaviour::Good => answer(&host, &good),
                Behaviour::IdleFirst => {
                    let _idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
                    answer(&host, &good);
                }
                Behaviour::Messy => {
                    answer("evil.example:80", &good);
                    answer(&host, "GET /favicon.ico HTTP/1.1");
                    answer(
                        &host,
                        &format!("GET /?code={GOOD_CODE}&state=forged HTTP/1.1"),
                    );
                    answer(&host, &format!("GET /?code={GOOD_CODE} HTTP/1.1"));
                    answer(
                        &host,
                        &format!("POST /?code={GOOD_CODE}&state={state} HTTP/1.1"),
                    );
                    answer(&host, &good);
                }
                Behaviour::Denied => answer(
                    &host,
                    &format!("GET /?error=access_denied&state={state} HTTP/1.1"),
                ),
            }
        });
        Ok(())
    }
}

fn yes() -> Arc<FakeConfirm> {
    FakeConfirm::saying(&[true])
}

// --- status ---

#[test]
fn the_status_comes_from_the_setting_and_the_build_with_no_network() {
    let world = World::new();
    let status = status(&*world.host, &world.shell).unwrap();
    assert_eq!(status.client, DriveClientSource::BuiltIn);
    assert!(!status.connected);
    assert!(!status.needs_reconnect);
    assert_eq!(status.account, None);
    assert_eq!(status.folder_url, None);

    world.put_connection(&DriveConnection {
        account: Some(ACCOUNT.to_owned()),
        folder_id: Some("abc".to_owned()),
        connected_at: Some(5),
        own_client_id: Some("mine".to_owned()),
        needs_reconnect: true,
        last: None,
    });
    let status = status_after(&world);
    assert_eq!(status.client, DriveClientSource::Own);
    assert!(status.connected);
    assert!(status.needs_reconnect);
    assert_eq!(status.account.as_deref(), Some(ACCOUNT));
    assert_eq!(status.own_client_id.as_deref(), Some("mine"));
    assert_eq!(
        status.folder_url.as_deref(),
        Some("https://drive.google.com/drive/folders/abc")
    );
    // No network, no keystore: nothing was asked of Google.
    assert!(world.fake.requests().is_empty());
}

fn status_after(world: &World) -> DriveStatus {
    status(&*world.host, &world.shell).unwrap()
}

#[test]
fn a_build_without_a_client_says_none_until_an_own_client_is_set() {
    let world = World::with(None, |_| {});
    assert_eq!(status_after(&world).client, DriveClientSource::None);
    // An own client makes it usable.
    world.put_connection(&DriveConnection {
        own_client_id: Some("mine".to_owned()),
        ..DriveConnection::default()
    });
    assert_eq!(status_after(&world).client, DriveClientSource::Own);
}

#[test]
fn the_built_in_client_is_what_the_build_had_and_a_blank_one_is_none() {
    assert_eq!(builtin_of(None, None), None);
    assert_eq!(builtin_of(Some("  "), Some("secret")), None);
    assert_eq!(
        builtin_of(Some(" id "), Some("  ")),
        Some(("id".to_owned(), None))
    );
    assert_eq!(
        builtin_of(Some("id"), Some(" s ")),
        Some(("id".to_owned(), Some("s".to_owned())))
    );
    // The shell of the app is built from the same two constants.
    let built = DriveShell::new().builtin_client();
    assert_eq!(
        built.map(|client| (client.client_id, client.client_secret)),
        builtin_of(BUILT_IN_CLIENT_ID, BUILT_IN_CLIENT_SECRET)
    );
}

#[test]
fn the_status_while_the_core_is_not_ready_is_that_of_a_drive_not_connected() {
    let world = World::new();
    world.seed();
    let starting: Arc<dyn CoreHost> = Arc::new(AppState::<AiFake>::new());
    let status = status(&*starting, &world.shell).unwrap();
    assert!(!status.connected);
    assert_eq!(status.client, DriveClientSource::BuiltIn);
}

// --- connecting ---

#[test]
fn connecting_signs_in_stores_the_token_and_makes_the_folder() {
    let world = World::new();
    let confirm = yes();
    let browser = Redirecting::new(Behaviour::Good);

    let status = world.connect(&confirm, &browser).unwrap();

    assert!(status.connected);
    assert!(!status.needs_reconnect);
    assert_eq!(status.client, DriveClientSource::BuiltIn);
    assert_eq!(status.account.as_deref(), Some(ACCOUNT));
    // The refresh token is in the keystore, never in the setting.
    assert_eq!(world.token().as_deref(), Some(REFRESH_TOKEN));
    let setting = world.setting();
    assert_eq!(setting.account.as_deref(), Some(ACCOUNT));
    assert!(setting.connected_at.is_some());
    assert!(
        !serde_json::to_string(&setting)
            .unwrap()
            .contains(REFRESH_TOKEN)
    );
    let folder = setting.folder_id.clone().unwrap();
    assert_eq!(
        world.fake.file_named("atomik-kb backups").unwrap().id,
        folder
    );
    assert_eq!(
        status.folder_url.unwrap(),
        format!("https://drive.google.com/drive/folders/{folder}")
    );

    // The native notice was asked once, in the words of the labels.
    let asked = confirm.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    let labels = crate::shell_labels::Labels::default();
    assert_eq!(asked[0].title, labels.dialog_drive_title);
    assert_eq!(asked[0].message, labels.dialog_drive_message);
    assert_eq!(asked[0].accept, labels.dialog_drive_accept);
    assert_eq!(asked[0].cancel, labels.dialog_drive_cancel);

    // The address opened in the browser: S256 PKCE, offline access, the one scope, the loopback redirect.
    let url = browser.seen(|seen| seen.urls[0].clone());
    let parsed = Url::parse(&url).unwrap();
    assert_eq!(
        format!(
            "{}://{}{}",
            parsed.scheme(),
            parsed.host_str().unwrap(),
            parsed.path()
        ),
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    let pairs: std::collections::HashMap<String, String> = parsed
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(pairs["client_id"], "built-in-id");
    assert_eq!(pairs["response_type"], "code");
    assert_eq!(pairs["scope"], "https://www.googleapis.com/auth/drive.file");
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(pairs["code_challenge"].len(), 43);
    assert_eq!(pairs["access_type"], "offline");
    assert_eq!(pairs["prompt"], "consent");
    assert_eq!(pairs["state"].len(), 64);
    assert!(pairs["redirect_uri"].starts_with("http://127.0.0.1:"));

    // The code exchange carried the verifier, the redirect address and the client.
    let exchange = &world.fake.requests_to("POST", "/token")[0];
    assert_eq!(
        exchange.form_value("grant_type").as_deref(),
        Some("authorization_code")
    );
    assert_eq!(exchange.form_value("code").as_deref(), Some(GOOD_CODE));
    assert_eq!(
        exchange.form_value("client_id").as_deref(),
        Some("built-in-id")
    );
    assert_eq!(
        exchange.form_value("client_secret").as_deref(),
        Some("built-in-secret")
    );
    assert_eq!(
        exchange.form_value("redirect_uri").as_deref(),
        Some(pairs["redirect_uri"].as_str())
    );
    assert_eq!(exchange.form_value("code_verifier").unwrap().len(), 43);
    assert_ne!(
        exchange.form_value("code_verifier").unwrap(),
        pairs["code_challenge"]
    );
    // The shell says who it is, and sends the bearer token on the API calls.
    let about = &world.fake.requests_to("GET", "/drive/v3/about")[0];
    assert_eq!(
        about.header("user-agent"),
        Some(concat!("atomik-kb/", env!("CARGO_PKG_VERSION")))
    );
    assert!(
        about
            .header("authorization")
            .unwrap()
            .starts_with("Bearer access-")
    );
    assert_eq!(world.browser_answers(&browser), vec![200]);
    // The listener is closed once the sign-in is done.
    let port = browser.seen(|seen| seen.port.unwrap());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

impl World {
    fn browser_answers(&self, browser: &Redirecting) -> Vec<u16> {
        // The browser's thread has answered by the time the flow has the code.
        for _ in 0..100 {
            let statuses = browser.seen(|seen| seen.statuses.clone());
            if !statuses.is_empty() {
                return statuses;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Vec::new()
    }
}

#[test]
fn an_existing_folder_is_used_not_made_again() {
    let world = World::new();
    let folder = world.fake.add_folder("atomik-kb backups");
    world
        .connect(&yes(), &Redirecting::new(Behaviour::Good))
        .unwrap();
    assert_eq!(world.setting().folder_id.as_deref(), Some(folder.as_str()));
    assert_eq!(world.fake.count("POST", "/drive/v3/files"), 0);
    // A folder in the trash is not found: only live ones count (the query says so).
    let query = world.fake.requests_to("GET", "/drive/v3/files")[0]
        .query_value("q")
        .unwrap()
        .to_owned();
    assert!(query.contains("trashed = false"), "{query}");
}

#[test]
fn a_build_without_a_client_refuses_before_any_dialog() {
    let world = World::with(None, |_| {});
    let confirm = yes();
    let browser = Redirecting::new(Behaviour::Good);
    let result = world.connect(&confirm, &browser);
    is_reason(&result, "noClient");
    assert_eq!(confirm.count(), 0);
    assert!(browser.seen(|seen| seen.urls.is_empty()));
    assert!(world.fake.requests().is_empty());
}

#[test]
fn declining_the_notice_connects_nothing() {
    let world = World::new();
    let confirm = FakeConfirm::saying(&[false]);
    let browser = Redirecting::new(Behaviour::Good);
    let result = world.connect(&confirm, &browser);
    is_reason(&result, "consentDeclined");
    assert!(browser.seen(|seen| seen.urls.is_empty()));
    assert!(world.fake.requests().is_empty());
    assert_eq!(world.token(), None);
    assert!(!world.setting().connected_at.is_some());
}

#[test]
fn a_locked_or_starting_core_is_answered_before_any_dialog() {
    let world = World::new();
    let confirm = yes();
    let browser = Redirecting::new(Behaviour::Good);
    let starting: Arc<dyn CoreHost> = Arc::new(AppState::<AiFake>::new());
    CoreHost::lock(&*world._state).unwrap();
    for (host, expected) in [
        (&starting, ApiError::internal("core is starting")),
        (&world.host, ApiError::locked()),
    ] {
        let shell = &world.shell;
        assert_eq!(
            run(connect_flow(&**host, shell, &*confirm, &browser)).unwrap_err(),
            expected
        );
        assert_eq!(run(list_flow(&**host, shell)).unwrap_err(), expected);
        assert_eq!(
            run(download_flow(&**host, shell, "abc", &|_| true)).unwrap_err(),
            expected
        );
        assert_eq!(
            run(disconnect(Arc::clone(host), Arc::clone(shell))).unwrap_err(),
            expected
        );
        assert_eq!(
            run(client_set(Arc::clone(host), Arc::clone(shell), None)).unwrap_err(),
            expected
        );
        assert_eq!(
            run(backups_list(Arc::clone(host), Arc::clone(shell))).unwrap_err(),
            expected
        );
    }
    assert_eq!(confirm.count(), 0);
    assert!(browser.seen(|seen| seen.urls.is_empty()));
    assert!(world.fake.requests().is_empty());
}

#[test]
fn the_loopback_refuses_what_is_not_the_redirect_and_keeps_waiting_for_it() {
    let world = World::new();
    let browser = Redirecting::new(Behaviour::Messy);
    let status = world.connect(&yes(), &browser).unwrap();
    assert!(status.connected);
    // A wrong Host is 421; a favicon, a forged state, no state and a POST are 404; the real one is 200.
    let statuses = world.browser_answers(&browser);
    for _ in 0..100 {
        if browser.seen(|seen| seen.statuses.len()) == 6 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        browser.seen(|seen| seen.statuses.clone()),
        vec![421, 404, 404, 404, 404, 200],
        "{statuses:?}"
    );
    // The forged redirects never made a code exchange.
    assert_eq!(world.fake.count("POST", "/token"), 1);
}

#[test]
fn a_connection_that_says_nothing_does_not_hold_up_the_sign_in() {
    let world = World::new();
    let browser = Redirecting::new(Behaviour::IdleFirst);
    let started = Instant::now();
    let status = world.connect(&yes(), &browser).unwrap();
    assert!(status.connected);
    // The silent connection gets 5 s before the listener gives up on it; the real request did not wait for that.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(world.browser_answers(&browser), vec![200]);
}

#[test]
fn a_refusal_in_the_browser_is_denied_and_nothing_is_kept() {
    let world = World::new();
    let browser = Redirecting::new(Behaviour::Denied);
    let result = world.connect(&yes(), &browser);
    is_reason(&result, "denied");
    assert_eq!(world.token(), None);
    assert!(world.fake.requests().is_empty());
    assert!(!world.setting().connected_at.is_some());
}

#[test]
fn a_browser_that_never_comes_back_times_out() {
    let world = World::with(Some(("id", None)), |config| {
        config.browser_timeout = Duration::from_millis(400);
    });
    let browser = Redirecting::new(Behaviour::Nothing);
    let started = Instant::now();
    let result = world.connect(&yes(), &browser);
    is_reason(&result, "browserTimeout");
    assert!(started.elapsed() >= Duration::from_millis(350));
    let port = browser.seen(|seen| seen.port.unwrap());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    assert!(world.fake.requests().is_empty());
}

#[test]
fn cancelling_ends_the_wait_and_closes_the_listener() {
    let world = World::new();
    let browser = Redirecting::new(Behaviour::Nothing);
    let shell = Arc::clone(&world.shell);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel(&shell);
    });
    let started = Instant::now();
    let result = world.connect(&yes(), &browser);
    canceller.join().unwrap();
    is_reason(&result, "cancelled");
    assert!(started.elapsed() < Duration::from_secs(4));
    let port = browser.seen(|seen| seen.port.unwrap());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    assert_eq!(world.token(), None);
    // Nothing runs any more: cancelling again is fine.
    cancel(&world.shell);
}

#[test]
fn a_code_google_refuses_is_reconnect_and_keeps_nothing() {
    let world = World::new();
    world.fake.script().exchange_invalid_grant = true;
    let result = world.connect(&yes(), &Redirecting::new(Behaviour::Good));
    is_reason(&result, "reconnect");
    assert_eq!(world.token(), None);
    assert!(!world.setting().connected_at.is_some());
}

#[test]
fn a_failure_after_the_token_was_stored_forgets_it_again() {
    let world = World::new();
    world.fake.script().about_status = 500;
    let result = world.connect(&yes(), &Redirecting::new(Behaviour::Good));
    is_reason(&result, "server");
    assert_eq!(
        world.token(),
        None,
        "the token must not outlive a connection that was not made"
    );
    assert!(!world.setting().connected_at.is_some());
    // And it was revoked at Google.
    assert_eq!(world.fake.script().revoked, vec![REFRESH_TOKEN.to_owned()]);
}

#[test]
fn an_answer_without_a_refresh_token_is_not_a_connection() {
    let world = World::new();
    world.fake.script().issue_refresh_token = false;
    let result = world.connect(&yes(), &Redirecting::new(Behaviour::Good));
    is_reason(&result, "server");
    assert_eq!(world.token(), None);
}

#[test]
fn a_connection_that_cannot_reach_google_is_a_network_failure() {
    let world = World::with(Some(("id", None)), |config| {
        config.oauth_base = Some(format!("http://127.0.0.1:{}", closed_port()));
        config.api_base = config.oauth_base.clone();
    });
    let result = world.connect(&yes(), &Redirecting::new(Behaviour::Good));
    is_reason(&result, "network");
    assert_eq!(world.token(), None);
}

#[test]
fn connecting_again_clears_the_need_to_reconnect_and_keeps_the_own_client() {
    let world = World::new();
    world.put_connection(&DriveConnection {
        account: Some("old@example.com".to_owned()),
        folder_id: Some("old".to_owned()),
        connected_at: Some(1),
        own_client_id: None,
        needs_reconnect: true,
        last: None,
    });
    let status = world
        .connect(&yes(), &Redirecting::new(Behaviour::Good))
        .unwrap();
    assert!(!status.needs_reconnect);
    assert_eq!(status.account.as_deref(), Some(ACCOUNT));
    assert!(!world.setting().needs_reconnect);
}

#[test]
fn the_entry_point_runs_the_flow_on_a_blocking_thread() {
    let world = World::new();
    let confirm: Arc<dyn Confirm> = yes();
    let browser: Arc<dyn Browser> = Arc::new(Redirecting::new(Behaviour::Good));
    let status = run(connect(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
        confirm,
        browser,
    ))
    .unwrap();
    assert!(status.connected);
    let disconnected = run(disconnect(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
    ))
    .unwrap();
    assert!(!disconnected.connected);
}

// --- disconnecting and the client ---

#[test]
fn disconnecting_revokes_forgets_and_keeps_the_own_client() {
    let world = World::new();
    world.seed();
    world.put_connection(&DriveConnection {
        account: Some(ACCOUNT.to_owned()),
        folder_id: Some("f".to_owned()),
        connected_at: Some(1),
        own_client_id: Some("mine".to_owned()),
        needs_reconnect: true,
        last: None,
    });
    let status = run(disconnect(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
    ))
    .unwrap();
    assert!(!status.connected);
    assert!(!status.needs_reconnect);
    assert_eq!(status.account, None);
    assert_eq!(status.own_client_id.as_deref(), Some("mine"));
    assert_eq!(status.client, DriveClientSource::Own);
    assert_eq!(world.token(), None);
    assert_eq!(world.fake.script().revoked, vec![REFRESH_TOKEN.to_owned()]);
    let setting = world.setting();
    assert_eq!(setting.own_client_id.as_deref(), Some("mine"));
    assert!(setting.connected_at.is_none());
    // The files stay in Drive: nothing was deleted.
    assert!(world.fake.script().deleted.is_empty());
}

#[test]
fn disconnecting_works_without_a_network_and_ignores_what_google_says() {
    let world = World::with(Some(("id", None)), |config| {
        config.oauth_base = Some(format!("http://127.0.0.1:{}", closed_port()));
    });
    world.seed();
    let status = run(disconnect(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
    ))
    .unwrap();
    assert!(!status.connected);
    assert_eq!(world.token(), None);

    let world = World::new();
    world.seed();
    world.fake.script().revoke_status = 500;
    let status = run(disconnect(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
    ))
    .unwrap();
    assert!(!status.connected);
    assert_eq!(world.token(), None);
}

#[test]
fn an_own_client_replaces_the_built_in_one_and_disconnects() {
    let world = World::new();
    world.seed();
    let input = DriveClientInput {
        client_id: "  my-client.apps.example  ".to_owned(),
        client_secret: Some(" my-secret ".to_owned()),
    };
    let status = run(client_set(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
        Some(input),
    ))
    .unwrap();
    assert_eq!(status.client, DriveClientSource::Own);
    assert_eq!(
        status.own_client_id.as_deref(),
        Some("my-client.apps.example")
    );
    assert!(!status.connected, "a token is bound to its client");
    assert_eq!(world.token(), None);
    assert_eq!(world.fake.script().revoked.len(), 1);
    // The secret is in the keystore, not in the setting.
    let secret = world.keystore.get(SECRET_ENTRY).unwrap().unwrap();
    assert_eq!(&secret[..], b"my-secret");
    assert!(
        !serde_json::to_string(&world.setting())
            .unwrap()
            .contains("my-secret")
    );

    // Signing in uses it.
    let browser = Redirecting::new(Behaviour::Good);
    world.connect(&yes(), &browser).unwrap();
    let url = Url::parse(&browser.seen(|seen| seen.urls[0].clone())).unwrap();
    let id = url
        .query_pairs()
        .find(|(key, _)| key == "client_id")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(id, "my-client.apps.example");
    let exchange = &world.fake.requests_to("POST", "/token")[0];
    assert_eq!(
        exchange.form_value("client_id").as_deref(),
        Some("my-client.apps.example")
    );
    assert_eq!(
        exchange.form_value("client_secret").as_deref(),
        Some("my-secret")
    );

    // `None` goes back to the built-in client, disconnects again and deletes the secret.
    let status = run(client_set(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
        None,
    ))
    .unwrap();
    assert_eq!(status.client, DriveClientSource::BuiltIn);
    assert_eq!(status.own_client_id, None);
    assert!(!status.connected);
    assert!(world.keystore.get(SECRET_ENTRY).unwrap().is_none());
    assert_eq!(world.setting().own_client_id, None);
}

#[test]
fn an_own_client_without_a_secret_has_none_and_the_old_secret_goes() {
    let world = World::new();
    world.keystore.set(SECRET_ENTRY, b"old-secret").unwrap();
    let input = DriveClientInput {
        client_id: "other".to_owned(),
        client_secret: Some("   ".to_owned()),
    };
    run(client_set(
        Arc::clone(&world.host),
        Arc::clone(&world.shell),
        Some(input),
    ))
    .unwrap();
    assert!(world.keystore.get(SECRET_ENTRY).unwrap().is_none());
    let browser = Redirecting::new(Behaviour::Good);
    world.connect(&yes(), &browser).unwrap();
    let exchange = &world.fake.requests_to("POST", "/token")[0];
    assert_eq!(exchange.form_value("client_id").as_deref(), Some("other"));
    assert_eq!(exchange.form_value("client_secret"), None);
}

#[test]
fn a_client_that_is_not_one_is_refused_and_changes_nothing() {
    let world = World::new();
    world.seed();
    let too_long = "x".repeat(MAX_CLIENT_CHARS + 1);
    for (id, secret, field) in [
        ("", None, "clientId"),
        ("   ", None, "clientId"),
        ("a\nb", None, "clientId"),
        (too_long.as_str(), None, "clientId"),
        ("fine", Some("tab\there"), "clientSecret"),
        ("fine", Some(too_long.as_str()), "clientSecret"),
    ] {
        let input = DriveClientInput {
            client_id: id.to_owned(),
            client_secret: secret.map(str::to_owned),
        };
        let error = run(client_set(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            Some(input),
        ))
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "{id:?}");
        assert_eq!(error.details.unwrap()["field"], field, "{id:?}");
    }
    // Still connected: a refused client changed nothing.
    assert_eq!(world.token().as_deref(), Some(REFRESH_TOKEN));
    assert!(world.setting().connected_at.is_some());
    assert!(world.fake.requests().is_empty());
}

// --- delivering ---

#[test]
fn a_backup_goes_up_in_chunks_and_the_old_scheduled_ones_are_pruned() {
    let world = World::new();
    let folder = world.seed();
    for (name, at) in [
        ("atomik-kb-auto-2026-10-10-0930.atomikkb", 10),
        ("atomik-kb-auto-2026-10-11-0930.atomikkb", 11),
        ("atomik-kb-auto-2026-10-12-0930.atomikkb", 12),
        // Never deleted: not of the scheduled form.
        ("manual.atomikkb", 5),
        ("weird.atomikkb.bak", 6),
        ("notes.txt", 7),
    ] {
        world.fake.add_file(&folder, name, b"old", at * 1_000_000);
    }
    let size = CHUNK_SIZE + 1_000;
    let (path, bytes) = backup_file(&world, NEW_BACKUP, size);

    let delivered = world.deliver(&path, NEW_BACKUP, 2).unwrap();

    assert_eq!(delivered, DriveDelivery::Uploaded { pruned: 2 });
    let stored = world.fake.file_named(NEW_BACKUP).unwrap();
    assert!(stored.bytes == bytes, "the uploaded bytes differ");
    assert_eq!(stored.parent.as_deref(), Some(folder.as_str()));
    assert_eq!(
        world.fake.names_in(&folder),
        vec![
            "atomik-kb-auto-2026-10-12-0930.atomikkb".to_owned(),
            NEW_BACKUP.to_owned(),
            "manual.atomikkb".to_owned(),
            "notes.txt".to_owned(),
            "weird.atomikkb.bak".to_owned(),
        ]
    );
    // The order: refresh, find the folder, start, two chunks (a 308 in the middle), list, two deletions.
    assert_eq!(
        world.sequence(),
        [
            "POST /token",
            "GET /drive/v3/files",
            "POST /upload/drive/v3/files",
            "PUT /upload/session",
            "PUT /upload/session",
            "GET /drive/v3/files",
            "DELETE /drive/v3/files/<id>",
            "DELETE /drive/v3/files/<id>",
        ]
    );
    assert_eq!(
        world.ranges(),
        vec![
            format!("bytes 0-{}/{size}", CHUNK_SIZE - 1),
            format!("bytes {CHUNK_SIZE}-{}/{size}", size - 1),
        ]
    );
    // What was announced.
    let start = &world.fake.requests_to("POST", "/upload/drive/v3/files")[0];
    assert_eq!(start.query_value("uploadType"), Some("resumable"));
    assert_eq!(
        start.header("x-upload-content-length"),
        Some(size.to_string().as_str())
    );
    assert_eq!(start.json()["name"], NEW_BACKUP);
    assert_eq!(start.json()["parents"][0], folder.as_str());
    // The outcome is recorded.
    let last = world.setting().last.unwrap();
    assert!(last.ok);
    assert_eq!(last.file.as_deref(), Some(NEW_BACKUP));
    assert_eq!(last.last_success_at, Some(last.at));
    assert_eq!(last.error, None);
}

#[test]
fn the_access_token_is_kept_between_deliveries() {
    let world = World::new();
    world.seed();
    let (path, _) = backup_file(&world, NEW_BACKUP, 2_000);
    world.deliver(&path, NEW_BACKUP, 7).unwrap();
    world
        .deliver(&path, "atomik-kb-auto-2026-10-14-0930.atomikkb", 7)
        .unwrap();
    assert_eq!(world.fake.count("POST", "/token"), 1);
}

#[test]
fn an_access_token_that_is_about_to_end_is_not_reused() {
    // Valid for 30 s: inside the 60 s margin, so every call that needs one refreshes it first.
    let world = World::new();
    world.seed();
    world.fake.script().expires_in = 30;
    let (path, _) = backup_file(&world, NEW_BACKUP, 2_000);
    world.deliver(&path, NEW_BACKUP, 7).unwrap();
    assert!(
        world.fake.count("POST", "/token") >= 3,
        "{:?}",
        world.sequence()
    );
}

#[test]
fn a_401_with_a_remembered_token_is_refreshed_once() {
    let world = World::new();
    world.seed();
    let (path, _) = backup_file(&world, NEW_BACKUP, 2_000);
    world.deliver(&path, NEW_BACKUP, 7).unwrap();
    world.fake.invalidate_access();
    world
        .deliver(&path, "atomik-kb-auto-2026-10-14-0930.atomikkb", 7)
        .unwrap();
    assert_eq!(world.fake.count("POST", "/token"), 2);
    assert!(!world.setting().needs_reconnect);
}

#[test]
fn a_lost_answer_is_resumed_from_what_drive_stored() {
    let world = World::new();
    let folder = world.seed();
    let size = CHUNK_SIZE + 1_000;
    let (path, bytes) = backup_file(&world, NEW_BACKUP, size);
    // The first chunk reaches Drive and its answer is lost.
    world
        .fake
        .script()
        .upload_faults
        .push_back(UploadFault::StoreThenFail(503));

    let delivered = world.deliver(&path, NEW_BACKUP, 7).unwrap();

    assert_eq!(delivered, DriveDelivery::Uploaded { pruned: 0 });
    assert!(world.fake.file_named(NEW_BACKUP).unwrap().bytes == bytes);
    // The chunk, the status query, and the rest from where Drive says it stands: nothing is sent twice.
    assert_eq!(
        world.ranges(),
        vec![
            format!("bytes 0-{}/{size}", CHUNK_SIZE - 1),
            format!("bytes */{size}"),
            format!("bytes {CHUNK_SIZE}-{}/{size}", size - 1),
        ]
    );
    assert_eq!(world.fake.names_in(&folder), vec![NEW_BACKUP.to_owned()]);
}

#[test]
fn a_chunk_that_never_arrived_is_sent_again() {
    let world = World::new();
    world.seed();
    let size = CHUNK_SIZE + 1_000;
    let (path, bytes) = backup_file(&world, NEW_BACKUP, size);
    world
        .fake
        .script()
        .upload_faults
        .push_back(UploadFault::Fail(500));

    world.deliver(&path, NEW_BACKUP, 7).unwrap();

    assert!(world.fake.file_named(NEW_BACKUP).unwrap().bytes == bytes);
    assert_eq!(
        world.ranges(),
        vec![
            format!("bytes 0-{}/{size}", CHUNK_SIZE - 1),
            format!("bytes */{size}"),
            format!("bytes 0-{}/{size}", CHUNK_SIZE - 1),
            format!("bytes {CHUNK_SIZE}-{}/{size}", size - 1),
        ]
    );
}

#[test]
fn a_chunk_that_times_out_is_resumed_through_the_status_query() {
    let world = World::with(Some(("id", None)), |config| {
        config.request_timeout = Duration::from_millis(300);
    });
    world.seed();
    let (path, bytes) = backup_file(&world, NEW_BACKUP, 1_000);
    world
        .fake
        .script()
        .upload_faults
        .push_back(UploadFault::Silent(Duration::from_millis(1_500)));

    world.deliver(&path, NEW_BACKUP, 7).unwrap();

    assert!(world.fake.file_named(NEW_BACKUP).unwrap().bytes == bytes);
    assert_eq!(
        world.ranges(),
        vec![
            "bytes 0-999/1000".to_owned(),
            "bytes */1000".to_owned(),
            "bytes 0-999/1000".to_owned(),
        ]
    );
}

#[test]
fn a_delivery_gives_up_after_five_resumptions() {
    let world = World::new();
    world.seed();
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);
    world.fake.script().fail_every_upload = Some(503);

    let result = world.deliver(&path, NEW_BACKUP, 7);

    is_reason(&result, "server");
    // The chunk, then five status queries.
    assert_eq!(world.ranges().len(), 6, "{:?}", world.sequence());
    assert!(world.fake.file_named(NEW_BACKUP).is_none());
    let last = world.setting().last.unwrap();
    assert!(!last.ok);
    assert!(last.error.unwrap().contains("did not answer as expected"));
    assert_eq!(last.last_success_at, None);
    assert!(!world.setting().needs_reconnect);
}

#[test]
fn a_failed_delivery_keeps_the_last_success() {
    let world = World::new();
    world.seed();
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);
    world.deliver(&path, NEW_BACKUP, 7).unwrap();
    let before = world.setting().last.unwrap();
    world.fake.script().upload_start_status = Some((403, "storageQuotaExceeded".to_owned()));

    let result = world.deliver(&path, "atomik-kb-auto-2026-10-14-0930.atomikkb", 7);

    is_reason(&result, "quota");
    let last = world.setting().last.unwrap();
    assert!(!last.ok);
    assert_eq!(last.last_success_at, before.last_success_at);
    assert_eq!(last.file, before.file);
    assert!(last.error.unwrap().contains("Google Drive is full"));
}

#[test]
fn google_refusing_the_token_marks_the_connection_and_stops_further_tries() {
    let world = World::new();
    world.seed();
    world.fake.script().refresh = RefreshMode::InvalidGrant;
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);

    let result = world.deliver(&path, NEW_BACKUP, 7);

    is_reason(&result, "reconnect");
    let setting = world.setting();
    assert!(setting.needs_reconnect);
    assert!(setting.connected_at.is_some());
    assert!(!setting.last.unwrap().ok);
    let status = status_after(&world);
    assert!(status.needs_reconnect);
    assert!(status.connected);
    // The next try does not even ask Google.
    let requests = world.fake.requests().len();
    let again = world.deliver(&path, NEW_BACKUP, 7);
    is_reason(&again, "reconnect");
    assert_eq!(world.fake.requests().len(), requests);
}

#[test]
fn a_connection_whose_token_is_gone_from_the_keystore_must_be_made_again() {
    let world = World::new();
    world.seed();
    world.keystore.delete(TOKEN_ENTRY).unwrap();
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);
    let result = world.deliver(&path, NEW_BACKUP, 7);
    is_reason(&result, "reconnect");
    assert!(world.setting().needs_reconnect);
    assert!(world.fake.requests().is_empty());
}

#[test]
fn delivering_without_a_connection_is_not_an_error_and_sends_nothing() {
    let world = World::new();
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);
    assert_eq!(
        world.deliver(&path, NEW_BACKUP, 7).unwrap(),
        DriveDelivery::NotConnected
    );
    assert!(world.fake.requests().is_empty());
    assert!(world.setting().last.is_none());
}

#[test]
fn a_folder_that_is_gone_is_made_again_and_remembered() {
    let world = World::new();
    world.seed();
    // The person trashed it.
    world.fake.script().files.clear();
    let (path, bytes) = backup_file(&world, NEW_BACKUP, 1_000);

    world.deliver(&path, NEW_BACKUP, 7).unwrap();

    let new_folder = world.fake.file_named("atomik-kb backups").unwrap();
    assert_eq!(
        world.setting().folder_id.as_deref(),
        Some(new_folder.id.as_str())
    );
    assert!(world.fake.file_named(NEW_BACKUP).unwrap().bytes == bytes);
    assert_eq!(world.fake.count("POST", "/drive/v3/files"), 1);
}

#[test]
fn a_deletion_that_fails_is_not_a_failed_delivery() {
    let world = World::new();
    let folder = world.seed();
    for day in 10..13 {
        world.fake.add_file(
            &folder,
            &format!("atomik-kb-auto-2026-10-{day}-0930.atomikkb"),
            b"old",
            i64::from(day) * 1_000_000,
        );
    }
    world.fake.script().delete_status = 500;
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);

    let delivered = world.deliver(&path, NEW_BACKUP, 1).unwrap();

    assert_eq!(delivered, DriveDelivery::Uploaded { pruned: 0 });
    assert!(world.setting().last.unwrap().ok);
    assert!(world.fake.file_named(NEW_BACKUP).is_some());
}

#[test]
fn nothing_but_the_scheduled_form_is_ever_deleted_and_never_the_new_file() {
    let world = World::new();
    let folder = world.seed();
    // Clock skew: an older run's name sorts after the new one.
    world.fake.add_file(
        &folder,
        "atomik-kb-auto-2030-01-01-0000.atomikkb",
        b"future",
        1_000,
    );
    world
        .fake
        .add_file(&folder, "manual-1.atomikkb", b"m", 2_000);
    world
        .fake
        .add_file(&folder, "manual-2.atomikkb", b"m", 3_000);
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);

    let delivered = world.deliver(&path, NEW_BACKUP, 1).unwrap();

    // `keep` is 1: the newest by name is the future one, so the new file is the one the core would prune,
    // and the host must still keep it.
    assert_eq!(delivered, DriveDelivery::Uploaded { pruned: 0 });
    let names = world.fake.names_in(&folder);
    assert!(names.contains(&NEW_BACKUP.to_owned()), "{names:?}");
    assert!(names.contains(&"manual-1.atomikkb".to_owned()));
    assert!(names.contains(&"manual-2.atomikkb".to_owned()));
}

#[test]
fn an_unreachable_google_is_a_network_failure_that_is_recorded() {
    let world = World::with(Some(("id", None)), |config| {
        config.api_base = Some(format!("http://127.0.0.1:{}", closed_port()));
        config.oauth_base = config.api_base.clone();
    });
    world.seed();
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);
    let result = world.deliver(&path, NEW_BACKUP, 7);
    is_reason(&result, "network");
    let last = world.setting().last.unwrap();
    assert!(!last.ok);
    assert!(last.error.unwrap().contains("could not be reached"));
}

#[test]
fn a_file_that_cannot_be_read_is_a_storage_failure() {
    let world = World::new();
    world.seed();
    let missing = world.dir.path().join("missing.atomikkb");
    let error = world.deliver(&missing, NEW_BACKUP, 7).unwrap_err();
    assert_eq!(error.code, ErrorCode::Storage);
}

#[test]
fn a_delivery_with_no_client_is_no_client() {
    let world = World::with(None, |_| {});
    world.seed();
    let (path, _) = backup_file(&world, NEW_BACKUP, 1_000);
    is_reason(&world.deliver(&path, NEW_BACKUP, 7), "noClient");
}

// --- listing ---

#[test]
fn the_backups_of_the_folder_come_newest_first_and_only_backups() {
    let world = World::new();
    let folder = world.seed();
    world
        .fake
        .add_file(&folder, "b-old.atomikkb", &[0; 10], 1_700_000_000_000);
    world
        .fake
        .add_file(&folder, "b-new.atomikkb", &[0; 30], 1_700_000_900_000);
    world
        .fake
        .add_file(&folder, "b-mid.atomikkb", &[0; 20], 1_700_000_500_000);
    // Drive's `contains` is loose: this one comes back from it and is not a backup.
    world
        .fake
        .add_file(&folder, "x.atomikkb.bak", &[0; 5], 1_700_001_000_000);

    let backups = world.list().unwrap();

    let names: Vec<&str> = backups.iter().map(|backup| backup.name.as_str()).collect();
    assert_eq!(
        names,
        ["b-new.atomikkb", "b-mid.atomikkb", "b-old.atomikkb"]
    );
    assert_eq!(backups[0].size, 30);
    assert_eq!(backups[0].modified_at, 1_700_000_900_000);
    assert!(backups[0].id.starts_with("file-"));
}

#[test]
fn a_listing_reads_every_page_and_stops_at_two_hundred() {
    let world = World::new();
    let folder = world.seed();
    for index in 0..250 {
        world.fake.add_file(
            &folder,
            &format!("b-{index:03}.atomikkb"),
            b"x",
            1_700_000_000_000 + i64::from(index) * 1_000,
        );
    }
    world.fake.script().page_size = Some(100);

    let backups = world.list().unwrap();

    assert_eq!(backups.len(), 200);
    assert_eq!(backups[0].name, "b-249.atomikkb");
    assert_eq!(backups[199].name, "b-050.atomikkb");
    assert_eq!(world.fake.count("GET", "/drive/v3/files"), 3);
}

#[test]
fn a_folder_that_is_gone_is_looked_up_again_for_a_listing() {
    let world = World::new();
    world.seed();
    // The setting names a folder Drive does not know; the real one is another.
    let real = world.fake.add_folder("atomik-kb backups");
    world
        .fake
        .add_file(&real, "found.atomikkb", b"x", 1_700_000_000_000);
    world.put_connection(&DriveConnection {
        account: Some(ACCOUNT.to_owned()),
        folder_id: Some("gone".to_owned()),
        connected_at: Some(1),
        ..DriveConnection::default()
    });
    world
        .fake
        .script()
        .files
        .retain(|file| file.id != "folder-1");

    let backups = world.list().unwrap();

    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].name, "found.atomikkb");
    assert_eq!(world.setting().folder_id.as_deref(), Some(real.as_str()));
}

#[test]
fn no_folder_at_all_is_an_empty_list_and_makes_none() {
    let world = World::new();
    world.seed();
    world.fake.script().files.clear();
    assert!(world.list().unwrap().is_empty());
    assert_eq!(world.fake.count("POST", "/drive/v3/files"), 0);
}

#[test]
fn listing_needs_a_connection_that_works() {
    let world = World::new();
    is_reason(&world.list(), "notConnected");
    world.seed();
    world.put_connection(&DriveConnection {
        connected_at: Some(1),
        needs_reconnect: true,
        ..DriveConnection::default()
    });
    is_reason(&world.list(), "reconnect");
    assert!(world.fake.requests().is_empty());
}

// --- downloading ---

/// The progress events a download sent, with when each came.
type Progress = Arc<Mutex<Vec<(Instant, DriveProgress)>>>;

fn collect_progress() -> (Progress, impl Fn(DriveProgress) -> bool + Send + Sync) {
    let seen: Progress = Arc::default();
    let sink = Arc::clone(&seen);
    (seen, move |progress| {
        sink.lock().unwrap().push((Instant::now(), progress));
        true
    })
}

#[test]
fn a_backup_downloads_into_a_staging_file_and_the_path_is_absolute() {
    let world = World::new();
    let folder = world.seed();
    let content: Vec<u8> = (0..600_000).map(|index| (index % 253) as u8).collect();
    let id = world
        .fake
        .add_file(&folder, "b.atomikkb", &content, 1_700_000_000_000);
    // A previous download is deleted first.
    std::fs::create_dir_all(world.staging()).unwrap();
    std::fs::write(world.staging().join("stale.atomikkb"), b"old").unwrap();
    let (seen, emit) = collect_progress();

    let path = world.download(&id, &emit).unwrap();

    let path = PathBuf::from(path);
    assert!(path.is_absolute());
    assert_eq!(path.parent().unwrap(), world.staging());
    assert_eq!(path.extension().unwrap(), "atomikkb");
    assert!(std::fs::read(&path).unwrap() == content);
    assert_eq!(
        world.staged_files(),
        vec![path.clone()],
        "the stale file is gone"
    );
    // The same name `backup_import` accepts.
    assert!(crate::backup::explicit_path(&path.to_string_lossy()).is_ok());
    let events = seen.lock().unwrap();
    assert_eq!(
        events[0].1,
        DriveProgress {
            received: 0,
            total: Some(600_000)
        }
    );
    assert!(events.len() <= 4, "{} events", events.len());
    let request = &world
        .fake
        .requests_to("GET", &format!("/drive/v3/files/{id}"))[0];
    assert_eq!(request.query_value("alt"), Some("media"));
}

#[test]
fn progress_comes_at_most_every_quarter_of_a_second() {
    let world = World::new();
    let folder = world.seed();
    let id = world.fake.add_file(
        &folder,
        "b.atomikkb",
        &vec![1_u8; 12 * 1024],
        1_700_000_000_000,
    );
    world.fake.script().download_pace = Some((1024, Duration::from_millis(60)));
    let (seen, emit) = collect_progress();

    world.download(&id, &emit).unwrap();

    let events = seen.lock().unwrap();
    // About 0.7 s of download: the first event at once, then one every 250 ms at most.
    assert!(
        events.len() >= 2 && events.len() <= 5,
        "{} events",
        events.len()
    );
    for pair in events.windows(2) {
        assert!(
            pair[1].0.duration_since(pair[0].0) >= Duration::from_millis(240),
            "two events {:?} apart",
            pair[1].0.duration_since(pair[0].0)
        );
    }
    let received: Vec<u64> = events
        .iter()
        .map(|(_, progress)| progress.received)
        .collect();
    assert!(received.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn cancelling_a_download_ends_it_and_leaves_no_file() {
    let world = World::new();
    let folder = world.seed();
    let id = world.fake.add_file(
        &folder,
        "b.atomikkb",
        &vec![1_u8; 100 * 1024],
        1_700_000_000_000,
    );
    world.fake.script().download_pace = Some((1024, Duration::from_millis(100)));
    let shell = Arc::clone(&world.shell);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        cancel(&shell);
    });
    let started = Instant::now();

    let result = world.download(&id, &|_| true);

    canceller.join().unwrap();
    is_reason(&result, "cancelled");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(world.staged_files().is_empty());
}

#[test]
fn a_page_that_is_gone_ends_the_download() {
    let world = World::new();
    let folder = world.seed();
    let id = world.fake.add_file(
        &folder,
        "b.atomikkb",
        &vec![1_u8; 100 * 1024],
        1_700_000_000_000,
    );
    world.fake.script().download_pace = Some((1024, Duration::from_millis(50)));

    let result = world.download(&id, &|_| false);

    is_reason(&result, "cancelled");
    assert!(world.staged_files().is_empty());
}

#[test]
fn a_download_that_fails_leaves_no_file_and_says_why() {
    let world = World::new();
    let folder = world.seed();
    let id = world.fake.add_file(
        &folder,
        "b.atomikkb",
        &vec![1_u8; 50_000],
        1_700_000_000_000,
    );

    // Not there.
    is_reason(&world.download("no-such-file", &|_| true), "notFound");
    // A Drive that is full, a refusal of the token, Google failing.
    for (fault, expected) in [
        (
            DownloadFault::Status(403, "storageQuotaExceeded".to_owned()),
            "quota",
        ),
        (
            DownloadFault::Status(500, "backendError".to_owned()),
            "server",
        ),
        // The connection breaks half way.
        (DownloadFault::Break(10_000), "network"),
    ] {
        world.fake.script().download_fault = Some(fault);
        is_reason(&world.download(&id, &|_| true), expected);
        assert!(world.staged_files().is_empty(), "{expected}");
    }
}

#[test]
fn a_file_id_is_checked_before_anything_is_sent() {
    let world = World::new();
    world.seed();
    let long = "x".repeat(129);
    for bad in ["", "a/b", "a b", "../x", "a?b=c", long.as_str()] {
        let error = world.download(bad, &|_| true).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput, "{bad:?}");
        assert_eq!(error.details.unwrap()["field"], "fileId", "{bad:?}");
    }
    assert!(world.fake.requests().is_empty());
    assert!(world.staged_files().is_empty());
}

#[test]
fn downloading_needs_a_connection() {
    let world = World::new();
    is_reason(&world.download("abc", &|_| true), "notConnected");
}

// --- the transport ---

#[test]
fn production_refuses_plain_http_and_foreign_hosts_without_connecting() {
    let net = Net::new(Config::production()).unwrap();
    for url in [
        "http://127.0.0.1:9/x",
        "https://example.com/x",
        "https://www.googleapis.com.evil.example/x",
    ] {
        let request = HttpRequest {
            method: "GET",
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        };
        let started = Instant::now();
        let result = run(net.send(request));
        assert!(
            matches!(result, Err(Fail::Drive(DriveError::Protocol(_)))),
            "{url}: {result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2), "{url}");
    }
}

// --- what is logged ---

#[test]
fn no_log_line_holds_a_token_a_code_a_state_an_id_a_name_or_an_address() {
    let world = World::new();
    let folder = world.fake.add_folder("atomik-kb backups");
    world.fake.add_file(
        &folder,
        "atomik-kb-auto-2026-10-10-0930.atomikkb",
        b"old",
        1_000_000,
    );
    let (path, _) = backup_file(&world, NEW_BACKUP, 5_000);
    let browser = Redirecting::new(Behaviour::Messy);
    let confirm = yes();

    // Through the app's own filter: the HTTP stack's targets stay at `warn`, whatever is asked.
    let filter = crate::log_filter(tracing_subscriber::EnvFilter::new("trace"));
    let logs = captured_logs_through(filter, || {
        world.connect(&confirm, &browser).unwrap();
        world.deliver(&path, NEW_BACKUP, 1).unwrap();
        let backups = world.list().unwrap();
        world.download(&backups[0].id, &|_| true).unwrap();
        // The failures too.
        world.fake.script().refresh = RefreshMode::InvalidGrant;
        world.fake.invalidate_access();
        let _ = world.list();
        let _ = run(disconnect(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
        ));
    });

    let state = browser.seen(|seen| seen.state.clone().unwrap());
    let mut secrets = vec![
        REFRESH_TOKEN.to_owned(),
        "access-".to_owned(),
        GOOD_CODE.to_owned(),
        "built-in-secret".to_owned(),
        "built-in-id".to_owned(),
        state,
        ACCOUNT.to_owned(),
        folder.clone(),
        NEW_BACKUP.to_owned(),
        "atomik-kb-auto".to_owned(),
        "127.0.0.1".to_owned(),
        "googleapis".to_owned(),
    ];
    secrets.push(world.dir.path().to_string_lossy().into_owned());
    assert!(!logs.is_empty(), "the flows log nothing at all");
    for secret in secrets {
        assert!(!logs.contains(&secret), "the log holds {secret:?}:\n{logs}");
    }
}

// --- the labels the flows read ---

#[test]
fn the_notice_and_the_page_are_written_in_the_words_the_ui_sent() {
    let world = World::new();
    let words = crate::shell_labels::Labels {
        dialog_drive_title: "Titre".to_owned(),
        dialog_drive_message: "Message <b>".to_owned(),
        dialog_drive_accept: "Oui".to_owned(),
        dialog_drive_cancel: "Non".to_owned(),
        dialog_drive_done: "Fermez <cet> onglet & revenez".to_owned(),
        ..crate::shell_labels::Labels::default()
    };
    let confirm = FakeConfirm::saying_in(&[true], words);
    let browser = Redirecting::new(Behaviour::Good);
    world.connect(&confirm, &browser).unwrap();
    // The page the browser lands on says it in those words, escaped.
    world.browser_answers(&browser);
    let body = browser.seen(|seen| seen.bodies[0].clone());
    assert!(
        body.contains("Fermez &lt;cet&gt; onglet &amp; revenez"),
        "{body}"
    );
    let asked = confirm.asked.lock().unwrap().clone();
    assert_eq!(
        (
            asked[0].title.as_str(),
            asked[0].message.as_str(),
            asked[0].accept.as_str(),
            asked[0].cancel.as_str()
        ),
        ("Titre", "Message <b>", "Oui", "Non")
    );
}

#[test]
fn the_reasons_are_the_words_the_page_reads() {
    // The TypeScript side lists these words (`DRIVE_ERROR_REASONS`); a test of the desktop app keeps them equal.
    let all = [
        Reason::ConsentDeclined,
        Reason::Cancelled,
        Reason::NoClient,
        Reason::BrowserTimeout,
        Reason::Denied,
        Reason::Reconnect,
        Reason::Quota,
        Reason::NotConnected,
        Reason::NotFound,
        Reason::Network,
        Reason::Server,
    ];
    let words: Vec<&str> = all.iter().map(|reason| reason.as_str()).collect();
    assert_eq!(
        words,
        [
            "consentDeclined",
            "cancelled",
            "noClient",
            "browserTimeout",
            "denied",
            "reconnect",
            "quota",
            "notConnected",
            "notFound",
            "network",
            "server",
        ]
    );
    for reason in all {
        let error = reason.error();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert_eq!(
            error.details,
            Some(json!({ "field": "drive", "reason": reason.as_str() }))
        );
        assert!(!error.message.is_empty());
    }
}
