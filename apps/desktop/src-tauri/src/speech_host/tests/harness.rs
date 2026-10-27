//! What the dictation tests share: a fake sidecar (a thread behind in-process pipes that speaks the real frame
//! protocol), a launcher that hands those out, a world (shell, ready core, store, models) and an event collector.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use kb_speech::api::DictationState;
use kb_speech::catalog::{MOONSHINE_TINY, PARAKEET};
use kb_speech::protocol::{FrameReader, FromSidecar, ToSidecar};

use super::super::*;
use super::{field, install_fake, reason};
use crate::ai_test_support::{SharedScript, eventually, ready_host};

/// Runs a future on a runtime of its own (the session and install tasks run on tauri's).
pub(super) fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

/// What a fake sidecar does after a message.
pub(super) enum Next {
    Go,
    /// Leave without being asked (a crash).
    Exit,
}

/// The fake's stdout.
pub(super) struct Out {
    pipe: std::io::PipeWriter,
}

impl Out {
    pub(super) fn send(&mut self, message: FromSidecar) {
        let _ = self.pipe.write_all(&message.encode().unwrap());
        let _ = self.pipe.flush();
    }

    pub(super) fn raw(&mut self, bytes: &[u8]) {
        let _ = self.pipe.write_all(bytes);
        let _ = self.pipe.flush();
    }
}

pub(super) type Script = Box<dyn FnMut(&ToSidecar, &mut Out) -> Next + Send>;

/// What the test can see of a fake sidecar.
#[derive(Clone, Default)]
pub(super) struct Wire {
    pub(super) received: Arc<Mutex<Vec<ToSidecar>>>,
    pub(super) killed: Arc<AtomicBool>,
    pub(super) exited: Arc<AtomicBool>,
}

impl Wire {
    pub(super) fn received(&self) -> Vec<ToSidecar> {
        self.received.lock().unwrap().clone()
    }

    pub(super) fn audio_frames(&self) -> Vec<Vec<i16>> {
        self.received()
            .into_iter()
            .filter_map(|message| match message {
                ToSidecar::Audio(pcm) => Some(pcm),
                _ => None,
            })
            .collect()
    }

    pub(super) fn got_quit(&self) -> bool {
        self.received().contains(&ToSidecar::Quit)
    }

    pub(super) fn was_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    pub(super) async fn left(&self) -> bool {
        let exited = Arc::clone(&self.exited);
        eventually(Duration::from_secs(5), move || {
            exited.load(Ordering::SeqCst)
        })
        .await
    }

    pub(super) async fn killed(&self) -> bool {
        let killed = Arc::clone(&self.killed);
        eventually(Duration::from_secs(5), move || {
            killed.load(Ordering::SeqCst)
        })
        .await
    }
}

struct FakeProcess {
    wire: Wire,
}

impl SidecarProcess for FakeProcess {
    fn kill(&mut self) {
        self.wire.killed.store(true, Ordering::SeqCst);
    }

    fn has_exited(&mut self) -> bool {
        self.wire.exited.load(Ordering::SeqCst)
    }
}

/// A sidecar that is a thread behind in-process pipes. A `deaf` one never reads its input.
fn spawn_fake(mut script: Script, wire: Wire, deaf: bool) -> Sidecar {
    let (stdin_reader, stdin_writer) = std::io::pipe().unwrap();
    let (stdout_reader, stdout_writer) = std::io::pipe().unwrap();
    let (stderr_reader, mut stderr_writer) = std::io::pipe().unwrap();
    let thread_wire = wire.clone();
    std::thread::spawn(move || {
        let mut out = Out {
            pipe: stdout_writer,
        };
        let mut stdin = stdin_reader;
        let mut reader = FrameReader::new();
        let mut block = vec![0_u8; 64 * 1024];
        'life: loop {
            if deaf {
                while !thread_wire.killed.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                break;
            }
            let read = match stdin.read(&mut block) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            reader.push(&block[..read]);
            while let Some(frame) = reader.next_frame().unwrap() {
                let message = ToSidecar::decode(&frame).unwrap();
                thread_wire.received.lock().unwrap().push(message.clone());
                if message == ToSidecar::Quit {
                    break 'life;
                }
                if matches!(script(&message, &mut out), Next::Exit) {
                    break 'life;
                }
            }
        }
        let _ = stderr_writer.write_all(b"kb-speech: ended (Quit)\n");
        thread_wire.exited.store(true, Ordering::SeqCst);
    });
    Sidecar {
        stdin: Box::new(stdin_writer),
        stdout: Box::new(stdout_reader),
        stderr: Box::new(stderr_reader),
        process: Box::new(FakeProcess { wire }),
    }
}

/// Finds and "starts" fake sidecars, one script per start.
pub(super) struct FakeLauncher {
    pub(super) present: AtomicBool,
    launches: AtomicUsize,
    pub(super) scripts: Mutex<VecDeque<Script>>,
    deaf: bool,
    refuse: bool,
    pub(super) wire: Wire,
}

impl FakeLauncher {
    pub(super) fn new(scripts: Vec<Script>) -> Arc<Self> {
        Self::with(scripts, false, false)
    }

    /// A launcher whose sidecars never read (`deaf`), or that cannot start any (`refuse`).
    pub(super) fn with(scripts: Vec<Script>, deaf: bool, refuse: bool) -> Arc<Self> {
        Arc::new(Self {
            present: AtomicBool::new(true),
            launches: AtomicUsize::new(0),
            scripts: Mutex::new(scripts.into()),
            deaf,
            refuse,
            wire: Wire::default(),
        })
    }

    pub(super) fn launches(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }
}

impl Launcher for FakeLauncher {
    fn locate(&self, _shell: &SpeechShell) -> Option<Located> {
        self.present.load(Ordering::SeqCst).then(|| Located {
            exe: PathBuf::from("kb-speech-fake"),
            vad: PathBuf::from("silero_vad.onnx"),
        })
    }

    fn launch(&self, _exe: &Path) -> std::io::Result<Sidecar> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        if self.refuse {
            return Err(std::io::Error::other("access denied"));
        }
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("a script for this launch");
        Ok(spawn_fake(script, self.wire.clone(), self.deaf))
    }
}

/// `Ready` on `Start`, `Flushed` on `Flush`, nothing else.
pub(super) fn quiet() -> Script {
    Box::new(|message, out| {
        match message {
            ToSidecar::Start(_) => out.send(FromSidecar::Ready { load_ms: 1 }),
            ToSidecar::Flush => out.send(FromSidecar::Flushed),
            _ => {}
        }
        Next::Go
    })
}

/// A recogniser: after every `per_phrase` samples it ends a segment and says the next of `texts`; a flush ends
/// the segment in progress (as `Last.`).
pub(super) fn talkative(per_phrase: usize, texts: &'static [&'static str]) -> Script {
    let mut heard = 0_usize;
    let mut said = 0_usize;
    Box::new(move |message, out| {
        match message {
            ToSidecar::Start(_) => out.send(FromSidecar::Ready { load_ms: 1 }),
            ToSidecar::Audio(pcm) => {
                heard += pcm.len();
                while heard >= per_phrase {
                    heard -= per_phrase;
                    out.send(FromSidecar::SpeechEnded);
                    out.send(FromSidecar::Text {
                        text: texts[said % texts.len()].to_owned(),
                        start_ms: 0,
                        end_ms: 1,
                    });
                    said += 1;
                }
            }
            ToSidecar::Flush => {
                if heard > 0 {
                    heard = 0;
                    out.send(FromSidecar::SpeechEnded);
                    out.send(FromSidecar::Text {
                        text: "Last.".to_owned(),
                        start_ms: 0,
                        end_ms: 1,
                    });
                }
                out.send(FromSidecar::Flushed);
            }
            _ => {}
        }
        Next::Go
    })
}

/// What a session sent, collected.
#[derive(Clone, Default, Debug)]
pub(super) struct Events(pub(super) Arc<Mutex<Vec<DictationEvent>>>);

impl Events {
    pub(super) fn emitter(&self) -> Arc<dyn Fn(DictationEvent) -> bool + Send + Sync> {
        let inner = Arc::clone(&self.0);
        Arc::new(move |event| {
            inner.lock().unwrap().push(event);
            true
        })
    }

    pub(super) fn all(&self) -> Vec<DictationEvent> {
        self.0.lock().unwrap().clone()
    }

    pub(super) fn phrases(&self) -> Vec<String> {
        self.all()
            .into_iter()
            .filter_map(|event| match event {
                DictationEvent::Phrase { text } => Some(text),
                _ => None,
            })
            .collect()
    }

    pub(super) async fn wait(&self, what: &str, check: impl Fn(&[DictationEvent]) -> bool) {
        let events = Arc::clone(&self.0);
        assert!(
            eventually(Duration::from_secs(10), move || check(
                &events.lock().unwrap()
            ))
            .await,
            "{what}: {:?}",
            self.all()
        );
    }

    pub(super) async fn listening(&self) {
        self.wait("listening", |events| {
            events.contains(&state(DictationState::Listening))
        })
        .await;
    }

    /// Waits for `Ended`, then a little longer to see that nothing follows it, and gives every event.
    pub(super) async fn ended(&self) -> Vec<DictationEvent> {
        self.wait("ended", |events| {
            events.last() == Some(&DictationEvent::Ended)
        })
        .await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        let all = self.all();
        assert_eq!(all.last(), Some(&DictationEvent::Ended), "{all:?}");
        assert_eq!(
            all.iter()
                .filter(|event| **event == DictationEvent::Ended)
                .count(),
            1,
            "{all:?}"
        );
        all
    }
}

pub(super) fn state(state: DictationState) -> DictationEvent {
    DictationEvent::State { state }
}

pub(super) fn phrase(text: &str) -> DictationEvent {
    DictationEvent::Phrase {
        text: text.to_owned(),
    }
}

pub(super) fn error(code: DictationErrorCode) -> DictationEvent {
    DictationEvent::Error { code }
}

/// Whether `error` is `InvalidInput` for `field_name` with the reason `reason_name`.
pub(super) fn is_reason(error: &ApiError, field_name: &str, reason_name: &str) -> bool {
    field(error) == Some(field_name) && reason(error) == Some(reason_name)
}

/// Timeouts short enough for a test.
pub(super) const QUICK: Timeouts = Timeouts {
    load: Duration::from_secs(10),
    flush: Duration::from_secs(5),
    quit_grace: Duration::from_millis(300),
};

pub(super) fn quick_ai() -> Arc<AiShell> {
    Arc::new(AiShell::with_client(
        crate::ai_client::AiClient::with_timeouts(crate::ai_client::Timeouts {
            connect: Duration::from_secs(1),
            plain: Duration::from_secs(5),
            meta: Duration::from_secs(5),
            first_byte: Duration::from_secs(5),
            between_chunks: Duration::from_secs(5),
            show_cache: Duration::from_secs(60),
        }),
    ))
}

/// The shell, a ready core, and the folders of a test.
pub(super) struct World {
    pub(super) shell: Arc<SpeechShell>,
    pub(super) host: Arc<dyn CoreHost>,
    pub(super) core: SharedScript,
    pub(super) ai: Arc<AiShell>,
    pub(super) launcher: Arc<FakeLauncher>,
    pub(super) dir: tempfile::TempDir,
}

/// A world whose sidecars come from `launcher`, with both catalog models installed (empty files of the right
/// size).
pub(super) fn world_with(launcher: Arc<FakeLauncher>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let shell = Arc::new(SpeechShell::new());
    shell.set_data_dir(dir.path());
    shell.set_launcher(Arc::clone(&launcher) as Arc<dyn Launcher>);
    shell.set_timeouts(QUICK);
    let root = shell.store_root().unwrap();
    install_fake(&model_dir(&root, &MOONSHINE_TINY), &MOONSHINE_TINY);
    install_fake(&model_dir(&root, &PARAKEET), &PARAKEET);
    let (_state, host, core) = ready_host();
    World {
        shell,
        host,
        core,
        ai: quick_ai(),
        launcher,
        dir,
    }
}

/// A world whose sidecars follow `scripts`, one per start.
pub(super) fn world(scripts: Vec<Script>) -> World {
    world_with(FakeLauncher::new(scripts))
}

impl World {
    /// Starts a session of `engine`, answering what `start` answers.
    pub(super) async fn begin_as(
        &self,
        id: &str,
        engine: SpeechEngine,
    ) -> Result<Events, ApiError> {
        let events = Events::default();
        start(
            Arc::clone(&self.host),
            Arc::clone(&self.shell),
            Arc::clone(&self.ai),
            id.to_owned(),
            engine,
            events.emitter(),
        )
        .await?;
        Ok(events)
    }

    /// Starts a Light session that must start.
    pub(super) async fn begin(&self, id: &str) -> Events {
        self.begin_as(id, SpeechEngine::Light).await.unwrap()
    }

    /// Pushes `samples` samples of a constant to the session.
    pub(super) fn push(&self, id: &str, samples: usize) {
        audio(&self.shell, id, vec![100; samples]).unwrap();
    }
}
