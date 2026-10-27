//! A dictation session: the sidecar process with its three threads, and the task that turns what the sidecar says
//! into `DictationEvent`s (`docs/architecture/dictation.md` section 4.4).
//!
//! ```text
//!  commands (audio, stop, cancel) ──▶ SessionHandle ──▶ writer thread ──▶ sidecar stdin
//!                                          │
//!                                          └─▶ control channel ─┐
//!  sidecar stdout ──▶ reader thread (FrameReader) ──────────────┤
//!                                                               ▼
//!                       Driver (async task): state machine, Ollama requests ──▶ emit(DictationEvent)
//!  sidecar stderr ──▶ stderr thread (counts; logs only its own fixed lines)
//! ```
//!
//! - **The writer thread** owns stdin and writes the frames in order. It is a thread of its own because a write
//!   can wait for a sidecar that is busy; the commands never wait. The audio queued for it is counted
//!   ([`MAX_QUEUED_SAMPLES`], 60 s): over that, the session ends with `engineFailed` and the process is killed.
//!   (The sidecar's reader thread drains its pipe at once, so what this bound catches is a process that does not
//!   read at all.)
//! - **The reader thread** owns stdout, cuts it into frames and sends every message to the driver; its end is
//!   the end of the process.
//! - **The driver** is the only place that decides what the webview hears: the states (only changes are sent),
//!   the phrases, the one `Error` and the last `Ended`. It also runs the Ollama requests, one at a time and in
//!   order, and the two timeouts (the model's loading, [`LOAD_TIMEOUT`]; a stop's flush, `FLUSH_TIMEOUT`).
//! - **Ending**: the session leaves the shell's slot first (so a new one may start the moment the webview sees
//!   `ended`), then `Ended` is sent, then the process is told to quit (a stop) and killed 2 s later when it is
//!   still there, or killed at once (a cancel, a failure).
//!
//! Nothing here logs what was said: the log gets kinds, codes, engine names and sizes.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use kb_speech::api::{DictationErrorCode, DictationEvent, DictationState};
use kb_speech::catalog::SpeechEngine;
use kb_speech::policy::{FLUSH_TIMEOUT, MAX_AUDIO_FRAME_SAMPLES};
use kb_speech::protocol::{
    FrameReader, FromSidecar, SAMPLE_RATE_HZ, SidecarErrorCode, StartConfig, ToSidecar,
};
use kb_speech::wav::encode_wav;
use kb_types::ApiError;
use kb_types::ai::AiTranscribed;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::SpeechShell;
use super::process::{Sidecar, SidecarProcess};
use crate::ai_host::{self, AiShell};
use crate::state::CoreHost;

/// The audio the writer queue may hold, in samples: 60 s.
pub const MAX_QUEUED_SAMPLES: usize = 60 * SAMPLE_RATE_HZ as usize;

/// How long the sidecar may take to load its models and say `Ready`.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a sidecar that was told to quit may take to leave before it is killed.
pub const QUIT_GRACE: Duration = Duration::from_secs(2);

/// The three waits of a session. The shell uses [`Timeouts::PRODUCTION`]; the tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// How long the sidecar may take to say `Ready`.
    pub load: Duration,
    /// How long a stop waits for the last phrases.
    pub flush: Duration,
    /// How long a sidecar that was told to quit may take to leave.
    pub quit_grace: Duration,
}

impl Timeouts {
    /// 60 s to load, 20 s to flush, 2 s to quit.
    pub const PRODUCTION: Self = Self {
        load: LOAD_TIMEOUT,
        flush: FLUSH_TIMEOUT,
        quit_grace: QUIT_GRACE,
    };
}

/// How many speech segments may wait for the Ollama engine's transcription (each is at most 15 s of audio and
/// a little more). A person who talks faster than Ollama answers ends the session with `ollamaUnavailable`.
pub const MAX_TRANSCRIBE_QUEUE: usize = 8;

/// How many failed Ollama requests in a row end the session (`ollamaUnavailable`).
pub const MAX_TRANSCRIBE_FAILURES: u32 = 3;

/// The longest line of the sidecar's stderr that is kept to be looked at; the rest of a longer line is dropped.
const MAX_STDERR_LINE: usize = 128;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the writer thread is asked to do, in order.
enum WriterMsg {
    Start(Box<StartConfig>),
    Audio(Vec<i16>),
    Flush,
    Quit,
    /// Stop writing and close stdin now (a cancel, a failure).
    Abort,
}

/// What the driver is told, by the reader thread, the writer thread and the commands.
pub enum Msg {
    /// A message of the sidecar.
    Sidecar(FromSidecar),
    /// The sidecar's stdout ended: the process has gone.
    Eof,
    /// A frame of the sidecar could not be read.
    BadFrame,
    /// The writer could not write to the sidecar any more.
    WriterFailed,
    /// More than 60 s of audio are waiting for the sidecar.
    Overflow,
    /// The person stopped: the flush was queued, wait for it.
    Stop,
    /// Cancel now (the command, a lock).
    Cancel,
}

/// `InvalidInput` for `sessionId` with the reason `ended`: the session is not running (any more).
#[must_use]
pub fn ended_error() -> ApiError {
    ApiError::invalid("sessionId", "that dictation session is not running")
        .with_details(serde_json::json!({ "field": "sessionId", "reason": "ended" }))
}

/// What the commands and the shell hold of a running session.
#[derive(Clone)]
pub struct SessionHandle {
    /// The id the webview gave it.
    pub id: String,
    /// The engine it runs.
    pub engine: SpeechEngine,
    /// Tells this session from a later one with the same id.
    pub token: u64,
    /// `true` while the session takes audio; closed by a stop, a cancel or its end. The lock is held while a frame
    /// is queued, so a flush queued by a stop is never overtaken by audio.
    gate: Arc<Mutex<bool>>,
    writer: mpsc::Sender<WriterMsg>,
    /// The samples queued for the writer and not written yet.
    queued: Arc<AtomicUsize>,
    control: UnboundedSender<Msg>,
    process: Arc<Mutex<Box<dyn SidecarProcess>>>,
    quit_grace: Duration,
}

impl SessionHandle {
    /// Queues audio for the sidecar, cut into frames of at most one second. Never waits for the sidecar.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `sessionId` (reason `ended`) when the session no longer takes audio.
    pub fn audio(&self, pcm: Vec<i16>) -> Result<(), ApiError> {
        let mut open = lock(&self.gate);
        if !*open {
            return Err(ended_error());
        }
        let queued = self.queued.fetch_add(pcm.len(), Ordering::SeqCst) + pcm.len();
        if queued > MAX_QUEUED_SAMPLES {
            // The sidecar does not take what it is sent: end the session, now.
            *open = false;
            drop(open);
            self.queued.fetch_sub(pcm.len(), Ordering::SeqCst);
            let _ = self.control.send(Msg::Overflow);
            self.kill();
            return Err(ended_error());
        }
        for frame in pcm.chunks(MAX_AUDIO_FRAME_SAMPLES) {
            if self.writer.send(WriterMsg::Audio(frame.to_vec())).is_err() {
                *open = false;
                return Err(ended_error());
            }
        }
        Ok(())
    }

    /// The person stopped: no more audio is taken, the flush is queued behind what was sent, and the driver waits
    /// for it. Once only.
    pub fn stop(&self) {
        {
            let mut open = lock(&self.gate);
            if !*open {
                return;
            }
            *open = false;
            let _ = self.writer.send(WriterMsg::Flush);
        }
        let _ = self.control.send(Msg::Stop);
    }

    /// Ends the session now: drops what is queued, kills the process and tells the driver.
    pub fn cancel(&self) {
        *lock(&self.gate) = false;
        self.kill();
        let _ = self.control.send(Msg::Cancel);
    }

    /// Closes stdin and kills the process.
    fn kill(&self) {
        let _ = self.writer.send(WriterMsg::Abort);
        lock(&self.process).kill();
    }

    /// Asks the sidecar to quit and kills it when it is still there [`QUIT_GRACE`] later.
    fn quit_and_reap(&self) {
        let _ = self.writer.send(WriterMsg::Quit);
        let process = Arc::clone(&self.process);
        let grace = self.quit_grace;
        let spawned = std::thread::Builder::new()
            .name("kb-speech-reaper".into())
            .spawn(move || {
                let deadline = Instant::now() + grace;
                loop {
                    if lock(&process).has_exited() {
                        return;
                    }
                    if Instant::now() >= deadline {
                        lock(&process).kill();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            });
        if spawned.is_err() {
            self.kill();
        }
    }
}

/// Writes the frames queued by the handle to the sidecar's stdin, in order, until a `Quit`, an `Abort`, an error
/// or the end of the queue.
fn writer_loop(
    queue: &mpsc::Receiver<WriterMsg>,
    mut stdin: Box<dyn Write + Send>,
    queued: &AtomicUsize,
    control: &UnboundedSender<Msg>,
) {
    for message in queue {
        let (bytes, samples, last) = match message {
            WriterMsg::Abort => return,
            WriterMsg::Start(config) => (ToSidecar::Start(*config).encode(), 0, false),
            WriterMsg::Audio(pcm) => {
                let samples = pcm.len();
                (ToSidecar::Audio(pcm).encode(), samples, false)
            }
            WriterMsg::Flush => (ToSidecar::Flush.encode(), 0, false),
            WriterMsg::Quit => (ToSidecar::Quit.encode(), 0, true),
        };
        let written = bytes
            .ok()
            .is_some_and(|bytes| stdin.write_all(&bytes).and_then(|()| stdin.flush()).is_ok());
        if samples > 0 {
            queued.fetch_sub(samples, Ordering::SeqCst);
        }
        if !written {
            let _ = control.send(Msg::WriterFailed);
            return;
        }
        if last {
            return;
        }
    }
}

/// Reads the sidecar's stdout into messages for the driver until it ends.
fn reader_loop(mut stdout: Box<dyn Read + Send>, control: &UnboundedSender<Msg>) {
    let mut reader = FrameReader::new();
    let mut block = vec![0_u8; 64 * 1024];
    loop {
        let read = match stdout.read(&mut block) {
            Ok(0) | Err(_) => {
                let _ = control.send(Msg::Eof);
                return;
            }
            Ok(read) => read,
        };
        reader.push(&block[..read]);
        loop {
            match reader.next_frame() {
                Ok(None) => break,
                Ok(Some(frame)) => match FromSidecar::decode(&frame) {
                    Ok(message) => {
                        if control.send(Msg::Sidecar(message)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = control.send(Msg::BadFrame);
                        return;
                    }
                },
                Err(_) => {
                    let _ = control.send(Msg::BadFrame);
                    return;
                }
            }
        }
    }
}

/// How the sidecar's own last line ends: the fixed words of `kb-speech: ended (<Exit>)`.
const EXIT_NAMES: [&str; 7] = [
    "Quit",
    "InputClosed",
    "Idle",
    "BadFrame",
    "StartFailed",
    "BadConfig",
    "OutputClosed",
];

/// The reason in a line of the sidecar's own (`kb-speech: ended (Quit)`), when that is what the line is.
fn own_diagnostic(line: &[u8]) -> Option<&'static str> {
    let text = std::str::from_utf8(line).ok()?;
    let name = text
        .trim_end()
        .strip_prefix("kb-speech: ended (")?
        .strip_suffix(')')?;
    EXIT_NAMES.into_iter().find(|known| *known == name)
}

/// Reads the sidecar's stderr to its end. Only the sidecar's own fixed line is logged (its reason); everything
/// else (the native libraries write what they like, paths included) is counted and dropped.
pub(super) fn stderr_loop(mut stderr: Box<dyn Read + Send>) {
    let mut block = [0_u8; 4096];
    let mut line: Vec<u8> = Vec::new();
    let (mut lines, mut dropped_bytes) = (0_u64, 0_u64);
    loop {
        let read = match stderr.read(&mut block) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for byte in &block[..read] {
            if *byte == b'\n' {
                match own_diagnostic(&line) {
                    Some(reason) => tracing::debug!(reason, "the dictation sidecar ended"),
                    None => {
                        lines += 1;
                        dropped_bytes += line.len() as u64;
                    }
                }
                line.clear();
            } else if line.len() < MAX_STDERR_LINE {
                line.push(*byte);
            }
        }
    }
    if !line.is_empty() && own_diagnostic(&line).is_none() {
        lines += 1;
        dropped_bytes += line.len() as u64;
    }
    if lines > 0 {
        tracing::debug!(
            lines,
            bytes = dropped_bytes,
            "the dictation sidecar wrote diagnostics (not logged)"
        );
    }
}

/// What the Ollama engine needs to transcribe.
pub struct Ollama {
    /// The core, for the AI settings.
    pub host: Arc<dyn CoreHost>,
    /// The AI client.
    pub ai: Arc<AiShell>,
}

/// The event sink of a session: `false` means the webview's channel is gone.
pub type SessionEmit = Arc<dyn Fn(DictationEvent) -> bool + Send + Sync>;

/// Starts the threads of a launched sidecar and queues its first frame. The session is not registered anywhere;
/// the caller does that and then runs the [`Driver`].
///
/// # Errors
///
/// The process is killed and `Err(())` is returned when a thread cannot be started.
pub fn open(
    sidecar: Sidecar,
    config: StartConfig,
    id: String,
    engine: SpeechEngine,
    token: u64,
    quit_grace: Duration,
) -> Result<(SessionHandle, UnboundedReceiver<Msg>), ()> {
    let Sidecar {
        stdin,
        stdout,
        stderr,
        process,
    } = sidecar;
    let process = Arc::new(Mutex::new(process));
    let (control, driver_rx) = unbounded_channel();
    let (writer, writer_rx) = mpsc::channel();
    let queued = Arc::new(AtomicUsize::new(0));
    let handle = SessionHandle {
        id,
        engine,
        token,
        gate: Arc::new(Mutex::new(true)),
        writer,
        queued: Arc::clone(&queued),
        control: control.clone(),
        process,
        quit_grace,
    };
    // The first frame is queued before any thread runs.
    if handle
        .writer
        .send(WriterMsg::Start(Box::new(config)))
        .is_err()
    {
        handle.kill();
        return Err(());
    }
    let writer_control = control.clone();
    let spawned = std::thread::Builder::new()
        .name("kb-speech-writer".into())
        .spawn(move || writer_loop(&writer_rx, stdin, &queued, &writer_control))
        .and_then(|_| {
            let reader_control = control;
            std::thread::Builder::new()
                .name("kb-speech-reader".into())
                .spawn(move || reader_loop(stdout, &reader_control))
        })
        .and_then(|_| {
            std::thread::Builder::new()
                .name("kb-speech-stderr".into())
                .spawn(move || stderr_loop(stderr))
        });
    if spawned.is_err() {
        handle.kill();
        return Err(());
    }
    Ok((handle, driver_rx))
}

/// What an Ollama request is, boxed.
type InFlight = Pin<Box<dyn Future<Output = Result<AiTranscribed, ApiError>> + Send>>;

/// Waits for the request in `slot` and empties the slot; pends forever while there is none.
async fn finished(slot: &mut Option<InFlight>) -> Result<AiTranscribed, ApiError> {
    match slot.as_mut() {
        Some(request) => {
            let outcome = request.as_mut().await;
            *slot = None;
            outcome
        }
        None => std::future::pending().await,
    }
}

/// Sleeps until `deadline`; pends forever without one.
async fn wake_at(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
        None => std::future::pending().await,
    }
}

/// What the driver does after a message.
enum Flow {
    Go,
    /// The session is over: with this failure, or none.
    Finish(Option<DictationErrorCode>),
}

/// The state machine of one session (see the module documentation).
pub struct Driver {
    handle: SessionHandle,
    rx: UnboundedReceiver<Msg>,
    shell: Arc<SpeechShell>,
    emit: SessionEmit,
    ollama: Option<Ollama>,
    timeouts: Timeouts,
    started: Instant,
    /// The last state sent.
    state: Option<DictationState>,
    ready: bool,
    /// Segments the sidecar is recognising (`SpeechEnded` seen, `Text` or `Empty` not yet).
    recognising: usize,
    /// The Ollama engine: a segment ended and its audio has not arrived yet.
    awaiting_audio: bool,
    /// The Ollama engine: segments waiting to be transcribed, oldest first.
    queue: VecDeque<Vec<i16>>,
    in_flight: Option<InFlight>,
    failures: u32,
    /// The flush deadline, once the person stopped.
    stopping: Option<Instant>,
    flushed: bool,
    /// The sidecar was asked to end (or the session is ending): its end is expected.
    quitting: bool,
    receiver_gone: bool,
}

impl Driver {
    /// A driver for `handle`, whose events go to `emit`. `State(Loading)` was already sent by the caller.
    #[must_use]
    pub fn new(
        handle: SessionHandle,
        rx: UnboundedReceiver<Msg>,
        shell: Arc<SpeechShell>,
        emit: SessionEmit,
        ollama: Option<Ollama>,
        timeouts: Timeouts,
    ) -> Self {
        Self {
            handle,
            rx,
            shell,
            emit,
            ollama,
            timeouts,
            started: Instant::now(),
            state: Some(DictationState::Loading),
            ready: false,
            recognising: 0,
            awaiting_audio: false,
            queue: VecDeque::new(),
            in_flight: None,
            failures: 0,
            stopping: None,
            flushed: false,
            quitting: false,
            receiver_gone: false,
        }
    }

    /// Runs the session to its end: `Ended` is the last event sent.
    pub async fn run(mut self) {
        let outcome = self.event_loop().await;
        self.finish(outcome);
    }

    async fn event_loop(&mut self) -> Flow {
        loop {
            let deadline = self.deadline();
            let flow = tokio::select! {
                message = self.rx.recv() => match message {
                    Some(message) => self.on_message(message),
                    // Every sender is gone: nothing can reach the session any more.
                    None => Flow::Finish(None),
                },
                outcome = finished(&mut self.in_flight) => self.on_transcribed(outcome),
                () = wake_at(deadline) => self.on_deadline(),
            };
            if matches!(flow, Flow::Finish(_)) {
                return flow;
            }
            if self.receiver_gone {
                return Flow::Finish(None);
            }
            self.pump();
            if self.stopping.is_some()
                && self.flushed
                && self.queue.is_empty()
                && self.in_flight.is_none()
            {
                return Flow::Finish(None);
            }
        }
    }

    /// The next moment something has to happen without a message: the load timeout, the flush deadline.
    fn deadline(&self) -> Option<Instant> {
        let load = (!self.ready).then(|| self.started + self.timeouts.load);
        match (load, self.stopping) {
            (Some(load), Some(flush)) => Some(load.min(flush)),
            (Some(at), None) | (None, Some(at)) => Some(at),
            (None, None) => None,
        }
    }

    fn on_deadline(&mut self) -> Flow {
        let now = Instant::now();
        if self.stopping.is_some_and(|flush| now >= flush) {
            // The flush took too long: end anyway, with what came.
            return Flow::Finish(None);
        }
        if !self.ready && now >= self.started + self.timeouts.load {
            return Flow::Finish(Some(DictationErrorCode::EngineFailed));
        }
        Flow::Go
    }

    fn on_message(&mut self, message: Msg) -> Flow {
        match message {
            Msg::Sidecar(message) => self.on_sidecar(message),
            Msg::Eof | Msg::WriterFailed => {
                if self.quitting {
                    Flow::Go
                } else if self.ready {
                    Flow::Finish(Some(DictationErrorCode::EngineCrashed))
                } else {
                    // The process ended while it loaded its models: a file it could not take.
                    Flow::Finish(Some(DictationErrorCode::EngineFailed))
                }
            }
            Msg::BadFrame | Msg::Overflow => Flow::Finish(Some(DictationErrorCode::EngineFailed)),
            Msg::Stop => {
                if self.stopping.is_none() {
                    self.stopping = Some(Instant::now() + self.timeouts.flush);
                }
                Flow::Go
            }
            Msg::Cancel => Flow::Finish(None),
        }
    }

    fn on_sidecar(&mut self, message: FromSidecar) -> Flow {
        match message {
            FromSidecar::Ready { load_ms } => {
                self.ready = true;
                tracing::info!(
                    engine = self.handle.engine.as_str(),
                    load_ms,
                    "the dictation engine is ready"
                );
                self.send_state(DictationState::Listening);
            }
            // Informational: the bar's level and timer run in the webview.
            FromSidecar::SpeechStarted => {}
            FromSidecar::SpeechEnded => {
                if self.ollama.is_some() {
                    self.awaiting_audio = true;
                } else {
                    self.recognising += 1;
                }
                self.send_state(DictationState::Recognising);
            }
            FromSidecar::Text { text, .. } => {
                self.recognising = self.recognising.saturating_sub(1);
                self.phrase(&text);
                self.relisten();
            }
            FromSidecar::Empty { .. } => {
                self.recognising = self.recognising.saturating_sub(1);
                self.relisten();
            }
            FromSidecar::SegmentAudio { pcm, .. } => {
                self.awaiting_audio = false;
                if self.ollama.is_none() || pcm.is_empty() {
                    return Flow::Go;
                }
                if self.queue.len() >= MAX_TRANSCRIBE_QUEUE {
                    // Ollama does not keep up with the speech.
                    return Flow::Finish(Some(DictationErrorCode::OllamaUnavailable));
                }
                self.queue.push_back(pcm);
            }
            FromSidecar::Flushed => self.flushed = true,
            FromSidecar::Error { code, .. } => {
                if code == SidecarErrorCode::RecognizerFailed {
                    // One segment failed; the session goes on. Only the code is logged.
                    tracing::info!(
                        engine = self.handle.engine.as_str(),
                        "the dictation recognizer failed on a segment"
                    );
                } else {
                    tracing::info!(
                        engine = self.handle.engine.as_str(),
                        "the dictation engine reported an error"
                    );
                    return Flow::Finish(Some(DictationErrorCode::EngineFailed));
                }
            }
        }
        Flow::Go
    }

    /// An Ollama request ended.
    fn on_transcribed(&mut self, outcome: Result<AiTranscribed, ApiError>) -> Flow {
        match outcome {
            Ok(transcribed) => {
                self.failures = 0;
                self.phrase(&transcribed.text);
            }
            Err(error) => {
                self.failures += 1;
                tracing::info!(
                    code = ?error.code,
                    failures = self.failures,
                    "a dictation segment could not be transcribed"
                );
                if self.failures >= MAX_TRANSCRIBE_FAILURES {
                    return Flow::Finish(Some(DictationErrorCode::OllamaUnavailable));
                }
            }
        }
        self.relisten();
        Flow::Go
    }

    /// Starts the next Ollama request when none runs.
    fn pump(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        let Some(ollama) = &self.ollama else {
            return;
        };
        let Some(pcm) = self.queue.pop_front() else {
            return;
        };
        let wav = encode_wav(&pcm);
        let host = Arc::clone(&ollama.host);
        let ai = Arc::clone(&ollama.ai);
        self.in_flight = Some(Box::pin(async move {
            ai_host::transcribe(host, ai, wav, None).await
        }));
    }

    /// Sends a phrase (trimmed; nothing when it is blank).
    fn phrase(&mut self, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.emit(DictationEvent::Phrase {
                text: text.to_owned(),
            });
        }
    }

    /// Back to listening when nothing is being recognised any more.
    fn relisten(&mut self) {
        if self.ready
            && self.recognising == 0
            && !self.awaiting_audio
            && self.queue.is_empty()
            && self.in_flight.is_none()
        {
            self.send_state(DictationState::Listening);
        }
    }

    fn send_state(&mut self, state: DictationState) {
        if self.state != Some(state) {
            self.state = Some(state);
            self.emit(DictationEvent::State { state });
        }
    }

    fn emit(&mut self, event: DictationEvent) {
        if !self.receiver_gone && !(self.emit)(event) {
            self.receiver_gone = true;
        }
    }

    /// The end: leaves the slot, sends the failure (if any) and `Ended`, and gets rid of the process.
    fn finish(&mut self, flow: Flow) {
        let error = match flow {
            Flow::Finish(error) => error,
            Flow::Go => None,
        };
        // A session that ends well and was not cancelled leaves through Quit; every other end kills.
        let polite = error.is_none() && self.stopping.is_some() && self.flushed;
        *lock(&self.handle.gate) = false;
        self.quitting = true;
        self.shell.release_session(self.handle.token);
        if let Some(code) = error {
            tracing::info!(
                engine = self.handle.engine.as_str(),
                code = code.as_str(),
                "the dictation session failed"
            );
            self.emit(DictationEvent::Error { code });
        }
        self.emit(DictationEvent::Ended);
        if polite && !self.receiver_gone {
            self.handle.quit_and_reap();
        } else {
            self.handle.kill();
        }
    }
}
