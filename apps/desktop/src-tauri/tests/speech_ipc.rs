//! The dictation commands of ADR-0053 through Tauri's real IPC layer, on the mock runtime: they are registered
//! under the documented names, take their arguments the way the webview's `TauriDictation` sends them, answer
//! what a core that is opening or locked answers (before any dialog would open: no test here may open a native
//! dialog), check the header and the body of `dictation_audio`, and run one session at a time (here against a
//! fake sidecar that speaks the real frame protocol over in-process pipes).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atomik_kb_desktop_lib::speech_host::{
    Launcher, Located, Sidecar, SidecarProcess, SpeechShell, model_dir, write_installed_marker,
};
use atomik_kb_desktop_lib::{AppState, MAIN_WINDOW, SharedState, configure, context};
use kb_speech::catalog::MOONSHINE_TINY;
use kb_speech::protocol::{FrameReader, FromSidecar, ToSidecar};
use serde_json::{Value, json};
use tauri::http::{HeaderMap, HeaderName, HeaderValue};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use common::Fake;

/// The header of `dictation_audio`, spelled out here so a rename in the shell breaks this test (and the
/// TypeScript contract test) instead of passing silently.
const SESSION_HEADER: &str = "x-kb-dictation";

/// What a channel argument the webview sends carries.
const CHANNEL: &str = "__CHANNEL__:7";

const COMMANDS: [&str; 10] = [
    "dictation_status",
    "dictation_select",
    "dictation_install",
    "dictation_install_cancel",
    "dictation_import",
    "dictation_remove",
    "dictation_start",
    "dictation_audio",
    "dictation_stop",
    "dictation_cancel",
];

fn app_over(state: SharedState) -> App<MockRuntime> {
    configure(mock_builder(), state).build(context()).unwrap()
}

fn main_webview(app: &App<MockRuntime>) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
        .build()
        .unwrap()
}

fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    command: &str,
    body: InvokeBody,
    headers: &[(&str, &str)],
) -> Result<Value, Value> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: (if cfg!(windows) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            })
            .parse()
            .unwrap(),
            body,
            headers: map,
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|body: InvokeResponseBody| body.deserialize::<Value>().unwrap())
}

fn call(webview: &WebviewWindow<MockRuntime>, command: &str, args: Value) -> Result<Value, Value> {
    invoke(webview, command, InvokeBody::Json(args), &[])
}

fn audio(
    webview: &WebviewWindow<MockRuntime>,
    session: Option<&str>,
    bytes: Vec<u8>,
) -> Result<Value, Value> {
    let headers: Vec<(&str, &str)> = session
        .map(|id| vec![(SESSION_HEADER, id)])
        .unwrap_or_default();
    invoke(webview, "dictation_audio", InvokeBody::Raw(bytes), &headers)
}

fn starting() -> Value {
    json!({ "code": "Internal", "message": "core is starting" })
}

fn locked() -> Value {
    json!({ "code": "Locked", "message": "the workspace is locked" })
}

fn field_of(error: &Value) -> Option<&str> {
    error["details"]["field"].as_str()
}

fn reason_of(error: &Value) -> Option<&str> {
    error["details"]["reason"].as_str()
}

/// Every command that needs the core, with the arguments the transport sends.
fn core_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("dictation_status", json!({})),
        ("dictation_select", json!({ "engine": "light" })),
        (
            "dictation_install",
            json!({ "engine": "light", "onEvent": CHANNEL }),
        ),
        ("dictation_import", json!({ "engine": "light" })),
        ("dictation_remove", json!({ "engine": "light" })),
        (
            "dictation_start",
            json!({ "sessionId": "s1", "engine": "light", "onEvent": CHANNEL }),
        ),
    ]
}

/// The commands that never need the core.
fn core_free_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("dictation_install_cancel", json!({})),
        ("dictation_stop", json!({ "sessionId": "nobody" })),
        ("dictation_cancel", json!({ "sessionId": "nobody" })),
    ]
}

#[test]
fn every_dictation_command_is_registered() {
    let app = app_over(Arc::new(AppState::<kb_core::Core>::new()));
    let webview = main_webview(&app);
    for command in COMMANDS {
        let message = match invoke(&webview, command, InvokeBody::default(), &[]) {
            Ok(_) => String::new(),
            Err(error) => error.to_string(),
        };
        assert!(
            !message.contains("not found"),
            "{command} is not registered: {message}"
        );
    }
}

#[test]
fn a_core_that_is_still_opening_answers_before_anything_else() {
    let app = app_over(Arc::new(AppState::<kb_core::Core>::new()));
    let webview = main_webview(&app);
    for (command, args) in core_calls() {
        assert_eq!(
            call(&webview, command, args).unwrap_err(),
            starting(),
            "{command}"
        );
    }
    // Stopping and cancelling need no core, and an unknown session is fine.
    for (command, args) in core_free_calls() {
        assert_eq!(
            call(&webview, command, args).unwrap(),
            Value::Null,
            "{command}"
        );
    }
}

#[test]
fn a_locked_core_answers_locked_and_opens_no_dialog() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    call(&webview, "lock", json!({})).unwrap();
    // An install or an import on a ready core with a model that is not there would open a native dialog.
    for (command, args) in core_calls() {
        assert_eq!(
            call(&webview, command, args).unwrap_err(),
            locked(),
            "{command}"
        );
    }
    for (command, args) in core_free_calls() {
        assert_eq!(
            call(&webview, command, args).unwrap(),
            Value::Null,
            "{command}"
        );
    }
}

#[test]
fn dictation_audio_checks_its_header_and_its_body_before_anything_else() {
    let app = app_over(Arc::new(AppState::<kb_core::Core>::new()));
    let webview = main_webview(&app);
    // No header, a header that is not an id.
    for session in [None, Some("bad id"), Some("../x")] {
        let error = audio(&webview, session, vec![0, 0]).unwrap_err();
        assert_eq!(error["code"], "InvalidInput");
        assert_eq!(field_of(&error), Some("sessionId"), "{session:?}");
    }
    // A body that is nothing, odd, or more than two seconds.
    for body in [vec![], vec![1, 2, 3], vec![0; 64_002]] {
        let error = audio(&webview, Some("s1"), body.clone()).unwrap_err();
        assert_eq!(field_of(&error), Some("audio"), "{} bytes", body.len());
    }
    // A good push for a session that is not running: refused with the reason `ended`.
    let error = audio(&webview, Some("s1"), vec![0; 3_200]).unwrap_err();
    assert_eq!(field_of(&error), Some("sessionId"));
    assert_eq!(reason_of(&error), Some("ended"));
    let error = audio(&webview, Some("s1"), vec![0; 64_000]).unwrap_err();
    assert_eq!(reason_of(&error), Some("ended"));
}

#[test]
fn the_status_and_the_choice_have_the_shape_the_transport_validates() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    app.state::<Arc<SpeechShell>>().set_data_dir(dir.path());
    let status = call(&webview, "dictation_status", json!({})).unwrap();
    assert_eq!(status["selected"], Value::Null);
    let engines = status["engines"].as_array().unwrap();
    let names: Vec<&str> = engines
        .iter()
        .map(|engine| engine["engine"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["accurate", "light", "ollama"]);
    for engine in engines {
        // The sidecar and the voice model are not next to a test executable.
        assert_eq!(engine["available"], false);
        assert_eq!(engine["unavailableReason"], "engineMissing");
    }
    assert_eq!(engines[0]["installed"], false);
    assert_eq!(engines[0]["downloadBytes"], 670_478_772_u64);
    assert_eq!(engines[1]["downloadBytes"], 123_967_539_u64);
    assert_eq!(engines[2]["installed"], true);
    assert_eq!(engines[2]["downloadBytes"], 0);
    assert!(
        engines[0]["attribution"]
            .as_str()
            .unwrap()
            .contains("CC-BY-4.0")
    );
    // Any engine may be chosen, installed or not; the choice is kept in the store.
    let status = call(&webview, "dictation_select", json!({ "engine": "ollama" })).unwrap();
    assert_eq!(status["selected"], "ollama");
    let status = call(&webview, "dictation_status", json!({})).unwrap();
    assert_eq!(status["selected"], "ollama");
    // An engine name that is not one is refused by the argument check.
    let error = call(&webview, "dictation_select", json!({ "engine": "cloud" })).unwrap_err();
    assert!(error.to_string().contains("engine") || error["code"] == "InvalidInput");
}

#[test]
fn an_engine_without_a_download_has_nothing_to_install_import_or_remove() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    app.state::<Arc<SpeechShell>>().set_data_dir(dir.path());
    for (command, args) in [
        (
            "dictation_install",
            json!({ "engine": "ollama", "onEvent": CHANNEL }),
        ),
        ("dictation_import", json!({ "engine": "ollama" })),
        ("dictation_remove", json!({ "engine": "ollama" })),
    ] {
        let error = call(&webview, command, args).unwrap_err();
        assert_eq!(error["code"], "InvalidInput", "{command}");
        assert_eq!(field_of(&error), Some("engine"), "{command}");
        assert_eq!(reason_of(&error), Some("modelMissing"), "{command}");
    }
}

#[test]
fn an_installed_model_installs_at_once_without_a_dialog() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let shell = app.state::<Arc<SpeechShell>>();
    shell.set_data_dir(dir.path());
    install_fake_light(&shell);
    assert_eq!(
        call(
            &webview,
            "dictation_install",
            json!({ "engine": "light", "onEvent": CHANNEL })
        )
        .unwrap(),
        Value::Null
    );
    // And it can be removed, which the status then shows.
    let status = call(&webview, "dictation_remove", json!({ "engine": "light" })).unwrap();
    assert_eq!(status["engines"][1]["installed"], false);
}

// ----- a whole session over IPC -----

fn install_fake_light(shell: &SpeechShell) {
    let dir = model_dir(&shell.store_root().unwrap(), &MOONSHINE_TINY);
    std::fs::create_dir_all(&dir).unwrap();
    for file in MOONSHINE_TINY.files {
        std::fs::File::create(dir.join(file.name))
            .unwrap()
            .set_len(file.bytes)
            .unwrap();
    }
    write_installed_marker(&dir, &MOONSHINE_TINY).unwrap();
}

struct FakeProcess(Arc<AtomicBool>);

impl SidecarProcess for FakeProcess {
    fn kill(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn has_exited(&mut self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Sidecars that answer `Ready` to `Start` and `Flushed` to `Flush`, and leave on `Quit`; every frame they
/// receive is kept.
struct Echo {
    received: Arc<Mutex<Vec<ToSidecar>>>,
    starts: Mutex<usize>,
}

impl Launcher for Echo {
    fn locate(&self, _shell: &SpeechShell) -> Option<Located> {
        Some(Located {
            exe: "kb-speech".into(),
            vad: "silero_vad.onnx".into(),
        })
    }

    fn launch(&self, _exe: &Path) -> std::io::Result<Sidecar> {
        *self.starts.lock().unwrap() += 1;
        let (stdin_reader, stdin_writer) = std::io::pipe()?;
        let (stdout_reader, mut stdout_writer) = std::io::pipe()?;
        let (stderr_reader, _stderr_writer) = std::io::pipe()?;
        let received = Arc::clone(&self.received);
        let gone = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&gone);
        std::thread::spawn(move || {
            let mut stdin = stdin_reader;
            let mut reader = FrameReader::new();
            let mut block = vec![0_u8; 64 * 1024];
            'life: loop {
                let read = match stdin.read(&mut block) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                reader.push(&block[..read]);
                while let Some(frame) = reader.next_frame().unwrap() {
                    let message = ToSidecar::decode(&frame).unwrap();
                    received.lock().unwrap().push(message.clone());
                    let answer = match message {
                        ToSidecar::Start(_) => Some(FromSidecar::Ready { load_ms: 1 }),
                        ToSidecar::Flush => Some(FromSidecar::Flushed),
                        ToSidecar::Quit => break 'life,
                        ToSidecar::Audio(_) => None,
                    };
                    if let Some(answer) = answer {
                        stdout_writer.write_all(&answer.encode().unwrap()).unwrap();
                    }
                }
            }
            flag.store(true, Ordering::SeqCst);
        });
        Ok(Sidecar {
            stdin: Box::new(stdin_writer),
            stdout: Box::new(stdout_reader),
            stderr: Box::new(stderr_reader),
            process: Box::new(FakeProcess(gone)),
        })
    }
}

fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(10) {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn one_session_runs_at_a_time_and_takes_audio_until_it_is_stopped() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let shell = Arc::clone(app.state::<Arc<SpeechShell>>().inner());
    shell.set_data_dir(dir.path());
    install_fake_light(&shell);
    let echo = Arc::new(Echo {
        received: Arc::default(),
        starts: Mutex::new(0),
    });
    shell.set_launcher(Arc::clone(&echo) as Arc<dyn Launcher>);

    let start = json!({ "sessionId": "s1", "engine": "light", "onEvent": CHANNEL });
    assert_eq!(
        call(&webview, "dictation_start", start.clone()).unwrap(),
        Value::Null
    );
    assert!(shell.session_running());
    // A second session is busy; nothing is started for it.
    let error = call(
        &webview,
        "dictation_start",
        json!({ "sessionId": "s2", "engine": "light", "onEvent": CHANNEL }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "InvalidInput");
    assert_eq!(field_of(&error), Some("sessionId"));
    assert_eq!(reason_of(&error), Some("busy"));
    assert_eq!(*echo.starts.lock().unwrap(), 1);
    // A model that is not installed, and an id that is not valid.
    let error = call(
        &webview,
        "dictation_start",
        json!({ "sessionId": "bad id", "engine": "light", "onEvent": CHANNEL }),
    )
    .unwrap_err();
    assert_eq!(field_of(&error), Some("sessionId"));

    // Audio of the right session is taken, of another is not.
    assert_eq!(
        audio(&webview, Some("s1"), vec![0; 3_200]).unwrap(),
        Value::Null
    );
    let error = audio(&webview, Some("s2"), vec![0; 3_200]).unwrap_err();
    assert_eq!(reason_of(&error), Some("ended"));

    // Stopping returns at once; the session ends and leaves its slot, and then a new one may start.
    assert_eq!(
        call(&webview, "dictation_stop", json!({ "sessionId": "s1" })).unwrap(),
        Value::Null
    );
    wait_until("the session to end", || !shell.session_running());
    let error = audio(&webview, Some("s1"), vec![0; 3_200]).unwrap_err();
    assert_eq!(reason_of(&error), Some("ended"));
    wait_until("the sidecar to receive Quit", || {
        echo.received.lock().unwrap().contains(&ToSidecar::Quit)
    });
    let kinds: Vec<&str> = echo
        .received
        .lock()
        .unwrap()
        .iter()
        .map(|message| match message {
            ToSidecar::Start(_) => "start",
            ToSidecar::Audio(_) => "audio",
            ToSidecar::Flush => "flush",
            ToSidecar::Quit => "quit",
        })
        .collect();
    assert_eq!(kinds, ["start", "audio", "flush", "quit"]);

    // A second session, ended by a cancel.
    assert_eq!(
        call(
            &webview,
            "dictation_start",
            json!({ "sessionId": "s2", "engine": "light", "onEvent": CHANNEL })
        )
        .unwrap(),
        Value::Null
    );
    assert_eq!(
        call(&webview, "dictation_cancel", json!({ "sessionId": "s2" })).unwrap(),
        Value::Null
    );
    wait_until("the second session to end", || !shell.session_running());
    assert_eq!(*echo.starts.lock().unwrap(), 2);
}

#[test]
fn a_lock_ends_the_running_session_and_installation_slots() {
    let (state, _world) = Fake::ready();
    let app = app_over(Arc::clone(&state) as SharedState);
    let webview = main_webview(&app);
    let dir = tempfile::tempdir().unwrap();
    let shell = Arc::clone(app.state::<Arc<SpeechShell>>().inner());
    shell.set_data_dir(dir.path());
    install_fake_light(&shell);
    shell.set_launcher(Arc::new(Echo {
        received: Arc::default(),
        starts: Mutex::new(0),
    }) as Arc<dyn Launcher>);
    call(
        &webview,
        "dictation_start",
        json!({ "sessionId": "s1", "engine": "light", "onEvent": CHANNEL }),
    )
    .unwrap();
    assert!(shell.session_running());
    // What a lock does to the background work: the shell cancels the session and the installation.
    shell.cancel_all();
    wait_until("the session to end", || !shell.session_running());
}
