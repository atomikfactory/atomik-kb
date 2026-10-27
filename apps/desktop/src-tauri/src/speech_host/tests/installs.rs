//! Installing, importing and removing models: the consent, the download against a stub server, the checks, the
//! progress, the cancel and what is left on disk.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use kb_speech::api::InstallEvent;
use kb_speech::catalog::{ModelFile, RecognizerKind};
use sha2::{Digest, Sha256};

use super::super::*;
use super::harness::{FakeLauncher, is_reason, quiet, run, world_with};
use super::{field, install_fake, reason};
use crate::ai_test_support::{FakeConfirm, eventually};
use crate::shell_labels::Labels;

// ----- the stub download server -----

/// What the stub does for a file.
#[derive(Clone)]
enum How {
    /// The body with its length.
    Whole,
    /// The body streamed (no length) in pieces with a pause before each.
    Slow { piece: usize, pause: Duration },
    /// Only the first `n` bytes, streamed (no length).
    Short(usize),
    /// The body and `n` more bytes, streamed (no length).
    Long(usize),
    /// An error status.
    Status(u16),
    /// A redirect to this place.
    Redirect(String),
}

#[derive(Clone)]
struct Served {
    body: Vec<u8>,
    how: How,
}

struct Hit {
    path: String,
    host: String,
    agent: String,
    cookies: bool,
}

struct StubInner {
    files: Mutex<HashMap<String, Served>>,
    hits: Mutex<Vec<Hit>>,
}

struct DownloadStub {
    addr: SocketAddr,
    inner: Arc<StubInner>,
}

impl DownloadStub {
    fn serve(&self, name: &str, body: Vec<u8>, how: How) {
        self.inner
            .files
            .lock()
            .unwrap()
            .insert(name.to_owned(), Served { body, how });
    }

    fn hits(&self) -> usize {
        self.inner.hits.lock().unwrap().len()
    }

    fn hosts(&self) -> Vec<String> {
        self.inner
            .hits
            .lock()
            .unwrap()
            .iter()
            .map(|hit| hit.host.clone())
            .collect()
    }
}

fn pieces(body: Vec<u8>, size: usize, pause: Duration) -> Body {
    let pieces: Vec<Bytes> = body.chunks(size).map(Bytes::copy_from_slice).collect();
    Body::from_stream(stream::unfold(
        (pieces.into_iter(), true),
        move |(mut pieces, first)| async move {
            let next = pieces.next()?;
            if !first && !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
            Some((Ok::<Bytes, std::convert::Infallible>(next), (pieces, false)))
        },
    ))
}

async fn handler(State(stub): State<Arc<StubInner>>, uri: Uri, headers: HeaderMap) -> Response {
    let text = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    stub.hits.lock().unwrap().push(Hit {
        path: uri.path().to_owned(),
        host: text(header::HOST),
        agent: text(header::USER_AGENT),
        cookies: headers.contains_key(header::COOKIE),
    });
    let name = uri
        .path()
        .strip_prefix("/f/")
        .unwrap_or_default()
        .to_owned();
    let served = stub.files.lock().unwrap().get(&name).cloned();
    let Some(Served { body, how }) = served else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match how {
        How::Whole => (StatusCode::OK, body).into_response(),
        How::Slow { piece, pause } => (StatusCode::OK, pieces(body, piece, pause)).into_response(),
        How::Short(n) => {
            let mut body = body;
            body.truncate(n);
            (StatusCode::OK, pieces(body, 4_096, Duration::ZERO)).into_response()
        }
        How::Long(extra) => {
            let mut body = body;
            body.extend(std::iter::repeat_n(7_u8, extra));
            (StatusCode::OK, pieces(body, 4_096, Duration::ZERO)).into_response()
        }
        How::Status(code) => StatusCode::from_u16(code).unwrap().into_response(),
        How::Redirect(to) => (StatusCode::FOUND, [(header::LOCATION, to)]).into_response(),
    }
}

async fn start_downloads() -> DownloadStub {
    let inner = Arc::new(StubInner {
        files: Mutex::new(HashMap::new()),
        hits: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .fallback(handler)
        .with_state(Arc::clone(&inner));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    DownloadStub { addr, inner }
}

fn policy_for(stub: &DownloadStub) -> DownloadPolicy {
    let base = format!("http://{}", stub.addr);
    DownloadPolicy {
        url_for: Arc::new(move |file: &ModelFile| format!("{base}/f/{}", file.name)),
        allow_redirect: Arc::new(|url| url.host_str() == Some("127.0.0.1")),
        use_system_proxy: false,
        connect_timeout: Duration::from_secs(2),
        read_timeout: Duration::from_secs(5),
        progress_interval: Duration::from_millis(100),
    }
}

// ----- test models -----

fn hash_of(bytes: &[u8]) -> String {
    super::super::process::hex(&Sha256::digest(bytes))
}

/// Deterministic content of `length` bytes.
fn content(length: usize, seed: u8) -> Vec<u8> {
    (0..length)
        .map(|n| u8::try_from(n % 251).unwrap().wrapping_add(seed))
        .collect()
}

/// A model of these files (name and content), pinned by what they are, living for the rest of the test run.
fn test_spec(id: &str, files: &[(&str, &[u8])]) -> &'static ModelSpec {
    let files: Vec<ModelFile> = files
        .iter()
        .map(|(name, bytes)| ModelFile {
            name: Box::leak((*name).to_owned().into_boxed_str()),
            url: "",
            bytes: bytes.len() as u64,
            sha256: Box::leak(hash_of(bytes).into_boxed_str()),
        })
        .collect();
    Box::leak(Box::new(ModelSpec {
        id: Box::leak(id.to_owned().into_boxed_str()),
        engine: SpeechEngine::Light,
        name: "Test model",
        kind: RecognizerKind::Moonshine,
        repo: "test/model",
        commit: "0123456789abcdef0123456789abcdef01234567",
        files: Box::leak(files.into_boxed_slice()),
        languages: &["en"],
        attribution: "",
        licence: "MIT",
        threads: 1,
        hard_cap_ms: 25_000,
    }))
}

/// The two files most tests use: 60,000 and 1,000 bytes.
fn pair() -> (Vec<u8>, Vec<u8>) {
    (content(60_000, 1), content(1_000, 2))
}

/// An installation that runs in the background, and the events it sends.
type Running = (
    tokio::task::JoinHandle<Result<(), ApiError>>,
    Arc<Mutex<Vec<InstallEvent>>>,
);

struct InstallWorld {
    shell: Arc<SpeechShell>,
    /// Keeps the store's folder until the test ends.
    _dir: tempfile::TempDir,
    stub: DownloadStub,
    spec: &'static ModelSpec,
    big: Vec<u8>,
    small: Vec<u8>,
}

impl InstallWorld {
    async fn new() -> Self {
        let (big, small) = pair();
        let spec = test_spec("test-model", &[("big.bin", &big), ("small.txt", &small)]);
        let stub = start_downloads().await;
        let dir = tempfile::tempdir().unwrap();
        let shell = Arc::new(SpeechShell::new());
        shell.set_data_dir(dir.path());
        shell.set_download_policy(policy_for(&stub));
        let world = Self {
            shell,
            _dir: dir,
            stub,
            spec,
            big,
            small,
        };
        world.serve_right_files();
        world
    }

    fn serve_right_files(&self) {
        self.stub.serve("big.bin", self.big.clone(), How::Whole);
        self.stub.serve("small.txt", self.small.clone(), How::Whole);
    }

    fn model_dir(&self) -> PathBuf {
        model_dir(&self.shell.store_root().unwrap(), self.spec)
    }

    fn folder_is_gone(&self) -> bool {
        !self.model_dir().exists()
    }

    fn no_part_files(&self) -> bool {
        match std::fs::read_dir(self.model_dir()) {
            Ok(entries) => entries
                .flatten()
                .all(|entry| entry.path().extension().is_none_or(|ext| ext != "part")),
            Err(_) => true,
        }
    }

    /// Installs with a person who says `answers`, collecting the events.
    async fn install(
        &self,
        answers: &[bool],
    ) -> (Result<(), ApiError>, Vec<InstallEvent>, Arc<FakeConfirm>) {
        let confirm = FakeConfirm::saying(answers);
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let emit = move |event| {
            sink.lock().unwrap().push(event);
            true
        };
        let result = install_model(
            &self.shell,
            Arc::clone(&confirm) as Arc<dyn Confirm>,
            self.spec,
            &emit,
        )
        .await;
        let events = events.lock().unwrap().clone();
        (result, events, confirm)
    }

    /// Starts an installation that the person agreed to, in the background, with the events collected.
    fn install_in_background(&self) -> Running {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let shell = Arc::clone(&self.shell);
        let spec = self.spec;
        let task = tokio::spawn(async move {
            let emit = move |event| {
                sink.lock().unwrap().push(event);
                true
            };
            install_model(
                &shell,
                FakeConfirm::saying(&[true]) as Arc<dyn Confirm>,
                spec,
                &emit,
            )
            .await
        });
        (task, events)
    }
}

fn code_of(events: &[InstallEvent]) -> Option<DictationErrorCode> {
    match events.last() {
        Some(InstallEvent::Error { code }) => Some(*code),
        _ => None,
    }
}

async fn first_progress(events: &Arc<Mutex<Vec<InstallEvent>>>) {
    let events = Arc::clone(events);
    assert!(
        eventually(Duration::from_secs(10), move || events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, InstallEvent::Progress { .. })))
        .await
    );
}

// ===== installing =====

#[test]
fn a_model_is_downloaded_verified_and_installed() {
    run(async {
        let world = InstallWorld::new().await;
        world.stub.serve(
            "big.bin",
            world.big.clone(),
            How::Slow {
                piece: 6_000,
                pause: Duration::from_millis(30),
            },
        );
        let (result, events, confirm) = world.install(&[true]).await;
        result.unwrap();
        // The events: progress (growing, of the whole model), then verifying, then done.
        let total = world.spec.total_bytes();
        let progress: Vec<(u64, u64)> = events
            .iter()
            .filter_map(|event| match event {
                InstallEvent::Progress { done, total } => Some((*done, *total)),
                _ => None,
            })
            .collect();
        assert!(progress.len() >= 2, "{events:?}");
        assert!(progress.iter().all(|(_, of)| *of == total));
        assert!(progress.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(*progress.last().unwrap(), (total, total));
        assert_eq!(
            events[events.len() - 2..],
            [InstallEvent::Verifying, InstallEvent::Done]
        );
        // The files are there, whole, with the marker, and nothing else.
        let dir = world.model_dir();
        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), world.big);
        assert_eq!(std::fs::read(dir.join("small.txt")).unwrap(), world.small);
        assert!(is_installed(&dir, world.spec));
        assert!(world.no_part_files());
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["big.bin", "installed.json", "small.txt"]);
        // The person was asked once, in the words of the dialog, naming the model, its size and the host.
        let asked = confirm.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].title, "Download a speech model?");
        assert_eq!(asked[0].accept, "Download");
        assert_eq!(asked[0].cancel, "Cancel");
        assert!(
            asked[0]
                .message
                .contains("Test model (1 MB) from huggingface.co."),
            "{}",
            asked[0].message
        );
        assert!(asked[0].message.contains("huggingface.co will see"));
        // The server saw the app's name and no cookies.
        let hits = world.stub.inner.hits.lock().unwrap();
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|hit| hit.agent.starts_with("atomik-kb/") && !hit.cookies)
        );
        drop(hits);
        assert!(!world.shell.install_running());
    });
}

#[test]
fn the_dialog_is_in_the_words_the_ui_sent() {
    run(async {
        let world = InstallWorld::new().await;
        let french = Labels {
            dialog_dictation_title: "Telecharger un modele vocal ?".to_owned(),
            dialog_dictation_message: "Telecharger {model} ({size}) depuis {host} ?".to_owned(),
            dialog_download: "Telecharger".to_owned(),
            dialog_cancel: "Annuler".to_owned(),
            ..Labels::default()
        };
        let confirm = FakeConfirm::saying_in(&[false], french);
        let error = install_model(
            &world.shell,
            Arc::clone(&confirm) as Arc<dyn Confirm>,
            world.spec,
            &|_event| true,
        )
        .await
        .unwrap_err();
        assert!(is_reason(&error, "engine", "consentDeclined"));
        let asked = confirm.asked.lock().unwrap();
        assert_eq!(asked[0].title, "Telecharger un modele vocal ?");
        assert_eq!(
            asked[0].message,
            "Telecharger Test model (1 MB) depuis huggingface.co ?"
        );
        assert_eq!(asked[0].accept, "Telecharger");
        assert_eq!(asked[0].cancel, "Annuler");
    });
}

#[test]
fn the_consent_question_names_the_real_models_and_their_sizes() {
    assert_eq!(size_text(670_478_772), "671 MB");
    assert_eq!(size_text(123_967_539), "124 MB");
    assert_eq!(size_text(1_000_000), "1 MB");
    assert_eq!(size_text(1_000_001), "2 MB");
    assert_eq!(size_text(0), "0 MB");
    let question = consent_question(&Labels::default(), &kb_speech::catalog::PARAKEET);
    assert!(
        question
            .message
            .contains("NVIDIA Parakeet TDT 0.6B v3 (int8) (671 MB) from huggingface.co.")
    );
    assert!(question.message.contains("never stored or sent anywhere"));
    assert!(!question.message.contains('{'));
}

#[test]
fn a_declined_dialog_starts_nothing_and_changes_nothing() {
    run(async {
        let world = InstallWorld::new().await;
        let (result, events, confirm) = world.install(&[false]).await;
        let error = result.unwrap_err();
        assert!(is_reason(&error, "engine", "consentDeclined"), "{error:?}");
        assert!(events.is_empty());
        assert_eq!(confirm.count(), 1);
        assert_eq!(
            world.stub.hits(),
            0,
            "nothing is contacted before the person agrees"
        );
        assert!(world.folder_is_gone());
        assert!(!world.shell.install_running());
        // And the person may try again.
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(events.last(), Some(&InstallEvent::Done));
    });
}

#[test]
fn an_installed_model_is_done_at_once_with_no_dialog() {
    run(async {
        let world = InstallWorld::new().await;
        install_fake(&world.model_dir(), world.spec);
        let (result, events, confirm) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(events, [InstallEvent::Done]);
        assert_eq!(confirm.count(), 0);
        assert_eq!(world.stub.hits(), 0);
    });
}

#[test]
fn a_file_with_the_right_size_and_the_wrong_hash_is_a_hash_mismatch_and_nothing_is_kept() {
    run(async {
        let world = InstallWorld::new().await;
        let mut wrong = world.small.clone();
        wrong[0] ^= 0xff;
        world.stub.serve("small.txt", wrong, How::Whole);
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::HashMismatch),
            "{events:?}"
        );
        assert!(!events.contains(&InstallEvent::Verifying));
        // The first file was complete, but nothing of it stays: no file, no `.part`, no folder.
        assert!(world.folder_is_gone());
        assert!(!world.shell.install_running());
    });
}

#[test]
fn a_body_longer_than_the_pinned_size_is_cut_and_refused() {
    run(async {
        let world = InstallWorld::new().await;
        // Streamed with no length: caught as the bytes arrive.
        world
            .stub
            .serve("big.bin", world.big.clone(), How::Long(500));
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::HashMismatch),
            "{events:?}"
        );
        assert!(world.folder_is_gone());
        // Announced with a length that is not the pinned one: caught before the body is read.
        let mut longer = world.big.clone();
        longer.extend_from_slice(&[1; 500]);
        world.stub.serve("big.bin", longer, How::Whole);
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::HashMismatch),
            "{events:?}"
        );
        assert!(world.folder_is_gone());
    });
}

#[test]
fn a_body_shorter_than_the_pinned_size_is_a_failed_download() {
    run(async {
        let world = InstallWorld::new().await;
        world
            .stub
            .serve("small.txt", world.small.clone(), How::Short(400));
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::DownloadFailed),
            "{events:?}"
        );
        assert!(world.folder_is_gone());
    });
}

#[test]
fn a_status_that_is_not_200_is_a_failed_download() {
    run(async {
        let world = InstallWorld::new().await;
        for status in [404, 500, 403] {
            world
                .stub
                .serve("big.bin", world.big.clone(), How::Status(status));
            let (result, events, _) = world.install(&[true]).await;
            result.unwrap();
            assert_eq!(
                code_of(&events),
                Some(DictationErrorCode::DownloadFailed),
                "{status}"
            );
            assert!(world.folder_is_gone());
        }
    });
}

#[test]
fn a_redirect_to_an_allowed_host_is_followed_and_to_another_host_is_refused() {
    run(async {
        let world = InstallWorld::new().await;
        // Followed: the file is served from another path of the same host.
        world.stub.serve(
            "big.bin",
            Vec::new(),
            How::Redirect("/f/real-big.bin".to_owned()),
        );
        world
            .stub
            .serve("real-big.bin", world.big.clone(), How::Whole);
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(events.last(), Some(&InstallEvent::Done), "{events:?}");
        let paths: Vec<String> = world
            .stub
            .inner
            .hits
            .lock()
            .unwrap()
            .iter()
            .map(|hit| hit.path.clone())
            .collect();
        assert!(paths.contains(&"/f/real-big.bin".to_owned()), "{paths:?}");
        // Refused: another host. No request is made to it.
        std::fs::remove_dir_all(world.model_dir()).unwrap();
        let before = world.stub.hits();
        let port = world.stub.addr.port();
        world.stub.serve(
            "big.bin",
            Vec::new(),
            How::Redirect(format!("http://localhost:{port}/f/real-big.bin")),
        );
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::DownloadFailed),
            "{events:?}"
        );
        assert_eq!(
            world.stub.hits(),
            before + 1,
            "only the first request was made"
        );
        assert!(
            world
                .stub
                .hosts()
                .iter()
                .all(|host| !host.starts_with("localhost"))
        );
        assert!(world.folder_is_gone());
    });
}

#[test]
fn no_connection_is_a_failed_download() {
    run(async {
        let world = InstallWorld::new().await;
        let port = crate::ai_test_support::closed_port();
        let mut policy = policy_for(&world.stub);
        policy.url_for =
            Arc::new(move |file: &ModelFile| format!("http://127.0.0.1:{port}/f/{}", file.name));
        world.shell.set_download_policy(policy);
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::DownloadFailed),
            "{events:?}"
        );
        assert!(world.folder_is_gone());
    });
}

#[test]
fn the_progress_events_come_at_most_ten_times_a_second() {
    run(async {
        let world = InstallWorld::new().await;
        // 60 KB in pieces of 1 KB with 5 ms between them: about 300 ms.
        world.stub.serve(
            "big.bin",
            world.big.clone(),
            How::Slow {
                piece: 1_000,
                pause: Duration::from_millis(5),
            },
        );
        let times = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&times);
        let emit = move |event: InstallEvent| {
            if matches!(event, InstallEvent::Progress { .. }) {
                sink.lock().unwrap().push(Instant::now());
            }
            true
        };
        let began = Instant::now();
        install_model(
            &world.shell,
            FakeConfirm::saying(&[true]) as Arc<dyn Confirm>,
            world.spec,
            &emit,
        )
        .await
        .unwrap();
        let elapsed = began.elapsed();
        let times = times.lock().unwrap().clone();
        assert!(
            times.len() >= 2,
            "{} progress events in {elapsed:?}",
            times.len()
        );
        // Between two progress events at least 95 ms, except that the last one is the closing one.
        for pair in times[..times.len() - 1].windows(2) {
            assert!(
                pair[1] - pair[0] >= Duration::from_millis(95),
                "{:?}",
                pair[1] - pair[0]
            );
        }
        let allowed = elapsed.as_millis() / 100 + 2;
        assert!(
            times.len() as u128 <= allowed,
            "{} events in {elapsed:?}",
            times.len()
        );
    });
}

#[test]
fn a_cancel_stops_the_download_and_leaves_no_part_file_and_a_retry_starts_clean() {
    run(async {
        let world = InstallWorld::new().await;
        world.stub.serve(
            "big.bin",
            world.big.clone(),
            How::Slow {
                piece: 2_000,
                pause: Duration::from_millis(100),
            },
        );
        let (task, events) = world.install_in_background();
        first_progress(&events).await;
        assert!(world.shell.install_running());
        install_cancel(&world.shell);
        task.await.unwrap().unwrap();
        let events = events.lock().unwrap().clone();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::Cancelled),
            "{events:?}"
        );
        assert!(world.folder_is_gone());
        assert!(!world.shell.install_running());
        // A cancel with nothing running is fine.
        install_cancel(&world.shell);
        // The retry starts from nothing and succeeds.
        world.serve_right_files();
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(events.last(), Some(&InstallEvent::Done));
        assert!(is_installed(&world.model_dir(), world.spec));
    });
}

#[test]
fn a_cancel_that_comes_while_the_dialog_is_open_stops_before_the_first_byte() {
    run(async {
        let world = InstallWorld::new().await;
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let shell = Arc::clone(&world.shell);
        let spec = world.spec;
        // The person's "Download" comes after a cancel arrived.
        struct CancelThenYes(Arc<SpeechShell>);
        impl Confirm for CancelThenYes {
            fn ask(&self, _question: &Question) -> bool {
                install_cancel(&self.0);
                true
            }
        }
        let emit = move |event| {
            sink.lock().unwrap().push(event);
            true
        };
        install_model(
            &shell,
            Arc::new(CancelThenYes(Arc::clone(&shell))),
            spec,
            &emit,
        )
        .await
        .unwrap();
        let events = events.lock().unwrap().clone();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::Cancelled),
            "{events:?}"
        );
        assert_eq!(world.stub.hits(), 0);
        assert!(world.folder_is_gone());
    });
}

#[test]
fn a_lock_cancels_the_installation() {
    run(async {
        let world = InstallWorld::new().await;
        world.stub.serve(
            "big.bin",
            world.big.clone(),
            How::Slow {
                piece: 2_000,
                pause: Duration::from_millis(100),
            },
        );
        let (task, events) = world.install_in_background();
        first_progress(&events).await;
        drop(speech_starter(Arc::clone(&world.shell))().unwrap());
        task.await.unwrap().unwrap();
        let events = events.lock().unwrap().clone();
        assert_eq!(
            code_of(&events),
            Some(DictationErrorCode::Cancelled),
            "{events:?}"
        );
        assert!(world.folder_is_gone());
    });
}

#[test]
fn a_second_installation_while_one_runs_is_busy_and_so_is_an_import() {
    run(async {
        let world = InstallWorld::new().await;
        world.stub.serve(
            "big.bin",
            world.big.clone(),
            How::Slow {
                piece: 2_000,
                pause: Duration::from_millis(100),
            },
        );
        let (task, events) = world.install_in_background();
        first_progress(&events).await;
        let (result, second_events, confirm) = world.install(&[true]).await;
        let error = result.unwrap_err();
        assert!(is_reason(&error, "engine", "busy"), "{error:?}");
        assert!(second_events.is_empty());
        assert_eq!(confirm.count(), 0, "no dialog for a refused installation");
        // An import of the same engine, and of the other one, wait their turn too.
        let source = tempfile::tempdir().unwrap();
        let (_state, host, _core) = crate::ai_test_support::ready_host();
        for engine in [SpeechEngine::Light, SpeechEngine::Accurate] {
            let error = import_from(
                Arc::clone(&host),
                Arc::clone(&world.shell),
                engine,
                move || Ok(PathBuf::new()),
            )
            .await
            .unwrap_err();
            assert!(is_reason(&error, "engine", "busy"), "{engine:?}: {error:?}");
        }
        drop(source);
        install_cancel(&world.shell);
        task.await.unwrap().unwrap();
    });
}

#[test]
fn a_receiver_that_is_gone_stops_the_download() {
    run(async {
        let world = InstallWorld::new().await;
        world.stub.serve(
            "big.bin",
            world.big.clone(),
            How::Slow {
                piece: 1_000,
                pause: Duration::from_millis(50),
            },
        );
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        // The webview takes two events and is gone.
        let emit = move |_event| counter.fetch_add(1, Ordering::SeqCst) < 2;
        let began = Instant::now();
        install_model(
            &world.shell,
            FakeConfirm::saying(&[true]) as Arc<dyn Confirm>,
            world.spec,
            &emit,
        )
        .await
        .unwrap();
        // The whole file takes 3 s to arrive (60 pieces, 50 ms apart); the third event, 200 ms in, stops it.
        assert!(
            began.elapsed() < Duration::from_millis(2_500),
            "{:?}",
            began.elapsed()
        );
        assert!(world.folder_is_gone());
        assert!(!world.shell.install_running());
        assert!(!is_installed(&world.model_dir(), world.spec));
    });
}

#[test]
fn a_failed_installation_keeps_an_older_one_and_removes_only_its_own_part_files() {
    run(async {
        let world = InstallWorld::new().await;
        // An older installation (another commit) with a stray file of the person's.
        let dir = world.model_dir();
        install_fake(&dir, world.spec);
        std::fs::write(
            dir.join("installed.json"),
            br#"{"id":"test-model","commit":"old"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
        assert!(!is_installed(&dir, world.spec));
        world
            .stub
            .serve("small.txt", world.small.clone(), How::Status(500));
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(code_of(&events), Some(DictationErrorCode::DownloadFailed));
        // The folder and the stray file stay (it held something else), the `.part` files do not.
        assert!(dir.join("notes.txt").exists());
        assert!(world.no_part_files());
        // A good run replaces the files and the marker; the stray file is untouched.
        world.serve_right_files();
        let (result, events, _) = world.install(&[true]).await;
        result.unwrap();
        assert_eq!(events.last(), Some(&InstallEvent::Done));
        assert!(is_installed(&dir, world.spec));
        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), world.big);
        assert_eq!(std::fs::read(dir.join("notes.txt")).unwrap(), b"mine");
    });
}

#[test]
fn the_command_refuses_before_any_dialog_when_it_cannot_install() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![quiet()]));
        let confirm = FakeConfirm::saying(&[true]);
        // Ollama has nothing to download.
        let error = install(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            Arc::clone(&confirm) as Arc<dyn Confirm>,
            SpeechEngine::Ollama,
            &|_event| true,
        )
        .await
        .unwrap_err();
        assert!(is_reason(&error, "engine", "modelMissing"), "{error:?}");
        // A core that is not ready answers first.
        let starting: Arc<dyn CoreHost> =
            Arc::new(crate::state::AppState::<crate::ai_test_support::AiFake>::new());
        let error = install(
            starting,
            Arc::clone(&world.shell),
            Arc::clone(&confirm) as Arc<dyn Confirm>,
            SpeechEngine::Light,
            &|_event| true,
        )
        .await
        .unwrap_err();
        assert_eq!(error.message, "core is starting");
        assert_eq!(confirm.count(), 0);
        // The Light model is installed in this world: done at once.
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        install(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            confirm as Arc<dyn Confirm>,
            SpeechEngine::Light,
            &move |event| {
                sink.lock().unwrap().push(event);
                true
            },
        )
        .await
        .unwrap();
        assert_eq!(*events.lock().unwrap(), [InstallEvent::Done]);
    });
}

// ===== importing =====

/// A folder with the files of the test model, and a stray one.
fn source_folder(world: &InstallWorld) -> tempfile::TempDir {
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("big.bin"), &world.big).unwrap();
    std::fs::write(source.path().join("small.txt"), &world.small).unwrap();
    std::fs::write(source.path().join("README.md"), b"extra").unwrap();
    source
}

#[test]
fn a_folder_with_every_file_is_imported_and_extra_files_are_ignored() {
    run(async {
        let world = InstallWorld::new().await;
        let source = source_folder(&world);
        let dir = world.model_dir();
        import_folder(&dir, world.spec, source.path()).unwrap();
        assert!(is_installed(&dir, world.spec));
        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), world.big);
        assert_eq!(std::fs::read(dir.join("small.txt")).unwrap(), world.small);
        assert!(!dir.join("README.md").exists());
        assert!(world.no_part_files());
        // The source is untouched.
        assert_eq!(
            std::fs::read(source.path().join("big.bin")).unwrap(),
            world.big
        );
    });
}

#[test]
fn a_folder_that_lacks_a_file_is_import_invalid_and_nothing_is_copied() {
    run(async {
        let world = InstallWorld::new().await;
        let source = source_folder(&world);
        std::fs::remove_file(source.path().join("small.txt")).unwrap();
        let error = import_folder(&world.model_dir(), world.spec, source.path()).unwrap_err();
        assert!(is_reason(&error, "engine", "importInvalid"), "{error:?}");
        assert!(world.folder_is_gone());
        // A folder that is not there.
        let error =
            import_folder(&world.model_dir(), world.spec, &source.path().join("nope")).unwrap_err();
        assert!(is_reason(&error, "engine", "importInvalid"), "{error:?}");
        // A folder in the place of a file is no file.
        std::fs::write(source.path().join("small.txt"), &world.small).unwrap();
        std::fs::remove_file(source.path().join("big.bin")).unwrap();
        std::fs::create_dir(source.path().join("big.bin")).unwrap();
        let error = import_folder(&world.model_dir(), world.spec, source.path()).unwrap_err();
        assert!(is_reason(&error, "engine", "importInvalid"), "{error:?}");
        assert!(world.folder_is_gone());
    });
}

#[test]
fn a_file_of_another_size_or_hash_is_a_hash_mismatch_and_nothing_is_kept() {
    run(async {
        let world = InstallWorld::new().await;
        let source = source_folder(&world);
        // Another size: caught before anything is copied.
        std::fs::write(source.path().join("small.txt"), b"short").unwrap();
        let error = import_folder(&world.model_dir(), world.spec, source.path()).unwrap_err();
        assert!(is_reason(&error, "engine", "hashMismatch"), "{error:?}");
        assert!(world.folder_is_gone());
        // The same size, other bytes: caught on the copy, and the `.part` files go.
        let mut wrong = world.small.clone();
        wrong[10] ^= 1;
        std::fs::write(source.path().join("small.txt"), wrong).unwrap();
        let error = import_folder(&world.model_dir(), world.spec, source.path()).unwrap_err();
        assert!(is_reason(&error, "engine", "hashMismatch"), "{error:?}");
        assert!(world.folder_is_gone());
        assert!(!world.shell.install_running());
    });
}

#[test]
fn an_import_of_the_catalogs_model_from_an_empty_folder_is_import_invalid_and_frees_the_slot() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![]));
        let source = tempfile::tempdir().unwrap();
        let path = source.path().to_path_buf();
        let error = import_from(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
            move || Ok(path),
        )
        .await;
        // The Light model is installed in this world and nothing is replaced when the files are not there.
        let error = error.unwrap_err();
        assert!(is_reason(&error, "engine", "importInvalid"), "{error:?}");
        assert!(!world.shell.install_running());
        let root = world.shell.store_root().unwrap();
        assert!(is_installed(
            &model_dir(&root, &kb_speech::catalog::MOONSHINE_TINY),
            &kb_speech::catalog::MOONSHINE_TINY
        ));
    });
}

#[test]
fn a_cancelled_folder_dialog_is_invalid_input_for_the_path_and_ollama_has_nothing_to_import() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![]));
        let error = import_from(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
            || Err(ApiError::invalid("path", "no file was chosen")),
        )
        .await
        .unwrap_err();
        assert_eq!(field(&error), Some("path"));
        assert!(!world.shell.install_running());
        let error = import_from(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Ollama,
            || panic!("no dialog for an engine with no model"),
        )
        .await
        .unwrap_err();
        assert!(is_reason(&error, "engine", "modelMissing"), "{error:?}");
        // A core that is not ready answers first, before any dialog.
        let starting: Arc<dyn CoreHost> =
            Arc::new(crate::state::AppState::<crate::ai_test_support::AiFake>::new());
        let error = import_from(
            starting,
            Arc::clone(&world.shell),
            SpeechEngine::Light,
            || panic!("no dialog while the core is not ready"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.message, "core is starting");
    });
}

/// The real Moonshine files, from a folder named by `KB_TEST_MOONSHINE_DIR` (124 MB: run by hand with
/// `--ignored`). Proves the catalog's sizes and SHA-256 are what Hugging Face serves.
#[test]
#[ignore = "needs the real Moonshine tiny files in KB_TEST_MOONSHINE_DIR"]
fn the_real_moonshine_files_import() {
    let Some(source) = std::env::var_os("KB_TEST_MOONSHINE_DIR").map(PathBuf::from) else {
        panic!("set KB_TEST_MOONSHINE_DIR");
    };
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join(kb_speech::catalog::MOONSHINE_TINY.id);
    let began = Instant::now();
    import_folder(&target, &kb_speech::catalog::MOONSHINE_TINY, &source).unwrap();
    assert!(is_installed(&target, &kb_speech::catalog::MOONSHINE_TINY));
    println!("imported 124 MB in {:?}", began.elapsed());
}

// ===== removing and choosing =====

#[test]
fn a_model_is_removed_with_its_folder_and_a_missing_one_is_fine() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![]));
        let root = world.shell.store_root().unwrap();
        let dir = model_dir(&root, &kb_speech::catalog::MOONSHINE_TINY);
        assert!(dir.exists());
        let status = remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
        )
        .await
        .unwrap();
        assert!(!dir.exists());
        let light = status
            .engines
            .iter()
            .find(|engine| engine.engine == SpeechEngine::Light)
            .unwrap();
        assert!(!light.installed);
        let accurate = status
            .engines
            .iter()
            .find(|engine| engine.engine == SpeechEngine::Accurate)
            .unwrap();
        assert!(accurate.installed, "the other model stays");
        // Again: nothing to remove is not an error.
        remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
        )
        .await
        .unwrap();
        // Ollama has no files.
        let error = remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Ollama,
        )
        .await
        .unwrap_err();
        assert!(is_reason(&error, "engine", "modelMissing"), "{error:?}");
    });
}

#[test]
fn removing_the_model_of_the_selected_engine_keeps_the_choice() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![]));
        let status = select(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
        )
        .await
        .unwrap();
        assert_eq!(status.selected, Some(SpeechEngine::Light));
        let status = remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
        )
        .await
        .unwrap();
        assert_eq!(status.selected, Some(SpeechEngine::Light));
        // Any engine may be chosen, installed or not.
        let status = select(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Ollama,
        )
        .await
        .unwrap();
        assert_eq!(status.selected, Some(SpeechEngine::Ollama));
    });
}

#[test]
fn a_model_in_use_is_not_removed() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![quiet()]));
        let events = world.begin("s1").await;
        // The Light model is in use by the session; the Accurate one is not.
        let error = remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
        )
        .await
        .unwrap_err();
        assert!(is_reason(&error, "engine", "busy"), "{error:?}");
        let root = world.shell.store_root().unwrap();
        assert!(model_dir(&root, &kb_speech::catalog::MOONSHINE_TINY).exists());
        remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Accurate,
        )
        .await
        .unwrap();
        cancel(&world.shell, "s1");
        events.ended().await;
        // Once the session is over it may go.
        remove(
            Arc::clone(&world.host),
            Arc::clone(&world.shell),
            SpeechEngine::Light,
        )
        .await
        .unwrap();
        assert!(!model_dir(&root, &kb_speech::catalog::MOONSHINE_TINY).exists());
    });
}

#[test]
fn commands_other_than_stop_and_cancel_answer_what_a_core_that_is_not_ready_answers() {
    run(async {
        let world = world_with(FakeLauncher::new(vec![]));
        let starting: Arc<dyn CoreHost> =
            Arc::new(crate::state::AppState::<crate::ai_test_support::AiFake>::new());
        let is_starting = |error: &ApiError| error.message == "core is starting";
        assert!(is_starting(
            &status(Arc::clone(&starting), Arc::clone(&world.shell))
                .await
                .unwrap_err()
        ));
        assert!(is_starting(
            &select(
                Arc::clone(&starting),
                Arc::clone(&world.shell),
                SpeechEngine::Light
            )
            .await
            .unwrap_err()
        ));
        assert!(is_starting(
            &remove(
                Arc::clone(&starting),
                Arc::clone(&world.shell),
                SpeechEngine::Light
            )
            .await
            .unwrap_err()
        ));
        assert!(is_starting(
            &start(
                starting,
                Arc::clone(&world.shell),
                Arc::clone(&world.ai),
                "s1".into(),
                SpeechEngine::Light,
                Arc::new(|_event| true),
            )
            .await
            .unwrap_err()
        ));
        assert_eq!(world.launcher.launches(), 0);
        // The stop and the cancel need no core.
        stop(&world.shell, "s1");
        cancel(&world.shell, "s1");
        let _ = reason;
    });
}

use std::sync::atomic::{AtomicUsize, Ordering};

/// The real Light model from Hugging Face (124 MB): the catalog's URLs, the real redirect chain and the real
/// hashes. Run by hand with `--ignored`; on a machine where Rust executables cannot connect directly
/// to the internet, set `HTTPS_PROXY=http://127.0.0.1:3128` with `scripts/dev-ipv4-proxy.mjs` running.
#[test]
#[ignore = "downloads 124 MB from huggingface.co"]
fn the_light_model_downloads_from_hugging_face() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let shell = Arc::new(SpeechShell::new());
        shell.set_data_dir(dir.path());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let began = Instant::now();
        install_model(
            &shell,
            FakeConfirm::saying(&[true]) as Arc<dyn Confirm>,
            &kb_speech::catalog::MOONSHINE_TINY,
            &move |event| {
                sink.lock().unwrap().push(event);
                true
            },
        )
        .await
        .unwrap();
        let events = events.lock().unwrap().clone();
        println!(
            "{} events in {:?}; last: {:?}",
            events.len(),
            began.elapsed(),
            events.last()
        );
        assert_eq!(
            events.last(),
            Some(&InstallEvent::Done),
            "{:?}",
            &events[events.len().saturating_sub(3)..]
        );
        let model = &kb_speech::catalog::MOONSHINE_TINY;
        assert!(is_installed(
            &model_dir(&shell.store_root().unwrap(), model),
            model
        ));
    });
}
