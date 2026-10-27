//! The loop of the sidecar: frames in, a [`Pipeline`] in the middle, frames out.
//!
//! - A reader thread owns the input. It cuts the byte stream into frames with
//!   [`kb_speech::protocol::FrameReader`], decodes them and sends them to the loop, so the loop notices a
//!   `Quit`, the end of the input or the idle timeout even while a recognition is not running, and a pipe
//!   never fills while the loop decodes (the shell bounds what it sends).
//! - The first message must be `Start`. It is validated ([`StartConfig::validate`]) and handed to the
//!   starter, which loads the models and returns the [`Pipeline`]; the loop answers `Ready { load_ms }`.
//!   Anything else first, a second `Start`, or a frame that cannot be read is an `Error` and the end.
//! - `Audio` goes to the pipeline, `Flush` drains it and is answered with `Flushed`, `Quit` ends the loop.
//! - No frame for `idle` (60 s in the binary) ends the loop: the sidecar holds a model in memory and the
//!   shell is supposed to push audio all the time.
//!
//! Nothing here holds or logs what was said: text only passes through the sink.

use std::io::{ErrorKind, Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kb_speech::protocol::{FrameReader, FromSidecar, SidecarErrorCode, StartConfig, ToSidecar};

/// The output cannot be written any more (the shell closed the pipe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkClosed;

/// Where a pipeline sends its messages.
pub trait Sink {
    /// Sends a message to the shell.
    ///
    /// # Errors
    ///
    /// [`SinkClosed`] when the output is gone.
    fn send(&mut self, message: FromSidecar) -> Result<(), SinkClosed>;
}

/// The voice activity detector and the recognizer of one session.
pub trait Pipeline {
    /// Takes more audio (16 kHz mono) in order. A speech segment that is complete is recognised (or, for a
    /// `VadOnly` pipeline, sent back) before this returns, through `sink`, with `SpeechStarted` and
    /// `SpeechEnded` as the detector flips. A recognition that fails is reported as an
    /// `Error { RecognizerFailed }` and the session goes on.
    ///
    /// # Errors
    ///
    /// [`SinkClosed`] when `sink` is.
    fn audio(&mut self, pcm: &[i16], sink: &mut dyn Sink) -> Result<(), SinkClosed>;

    /// Ends the speech in progress and recognises (or sends) everything that is left, through `sink`. The
    /// loop sends `Flushed` after it returns.
    ///
    /// # Errors
    ///
    /// [`SinkClosed`] when `sink` is.
    fn flush(&mut self, sink: &mut dyn Sink) -> Result<(), SinkClosed>;
}

/// Why a [`Pipeline`] could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartFailure {
    /// `ModelLoadFailed` or `VadLoadFailed`.
    pub code: SidecarErrorCode,
    /// A fixed sentence: never a path, never what the person said.
    pub message: &'static str,
}

/// Why the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The shell sent `Quit`.
    Quit,
    /// The shell closed the input (it exited, or ended the session by closing the pipe).
    InputClosed,
    /// No frame arrived for the idle time.
    Idle,
    /// A frame could not be read: the stream is lost.
    BadFrame,
    /// The models could not be loaded.
    StartFailed,
    /// A message came out of order, or the start configuration was refused.
    BadConfig,
    /// The output was closed.
    OutputClosed,
}

impl Exit {
    /// Whether the exit is an ordinary end of a session (exit code 0).
    #[must_use]
    pub const fn is_clean(self) -> bool {
        matches!(self, Self::Quit | Self::InputClosed | Self::Idle)
    }
}

/// Writes frames to a byte output.
struct FrameSink<W: Write> {
    output: W,
}

impl<W: Write> Sink for FrameSink<W> {
    fn send(&mut self, message: FromSidecar) -> Result<(), SinkClosed> {
        // A message that cannot be encoded (a segment over the frame limit, which the hard cap prevents)
        // is dropped like any failure to deliver it; the session carries on.
        let Ok(bytes) = message.encode() else {
            return Ok(());
        };
        self.output
            .write_all(&bytes)
            .and_then(|()| self.output.flush())
            .map_err(|_| SinkClosed)
    }
}

/// What the reader thread tells the loop.
enum Input {
    Message(ToSidecar),
    /// A frame could not be read.
    BadFrame,
    /// The input ended or failed.
    Closed,
}

/// Reads `input` to its end, sending every message to the loop. Returns when the input ends, a frame is bad
/// or the loop has gone.
fn read_frames<R: Read>(mut input: R, to_loop: &mpsc::Sender<Input>) {
    let mut reader = FrameReader::new();
    let mut block = vec![0_u8; 64 * 1024];
    loop {
        let read = match input.read(&mut block) {
            Ok(0) => {
                let _ = to_loop.send(Input::Closed);
                return;
            }
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => {
                let _ = to_loop.send(Input::Closed);
                return;
            }
        };
        reader.push(&block[..read]);
        loop {
            match reader.next_frame() {
                Ok(None) => break,
                Ok(Some(frame)) => match ToSidecar::decode(&frame) {
                    Ok(message) => {
                        if to_loop.send(Input::Message(message)).is_err() {
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = to_loop.send(Input::BadFrame);
                        return;
                    }
                },
                Err(_) => {
                    let _ = to_loop.send(Input::BadFrame);
                    return;
                }
            }
        }
    }
}

fn error_message(code: SidecarErrorCode, message: &str) -> FromSidecar {
    FromSidecar::Error {
        code,
        message: message.to_owned(),
    }
}

/// Runs a session: reads frames from `input`, writes frames to `output`.
///
/// `start` is called once, with the first `Start` message's configuration (already validated), and loads the
/// models. `idle` is how long the loop waits for a frame before it ends.
pub fn run<R, W, S>(input: R, output: W, start: S, idle: Duration) -> Exit
where
    R: Read + Send + 'static,
    W: Write,
    S: FnOnce(&StartConfig) -> Result<Box<dyn Pipeline>, StartFailure>,
{
    let (to_loop, from_reader) = mpsc::channel();
    // Detached on purpose: the thread may stay blocked in a read when the loop ends, and the process exits.
    std::thread::spawn(move || read_frames(input, &to_loop));
    let mut sink = FrameSink { output };
    let mut start = Some(start);
    let mut pipeline: Option<Box<dyn Pipeline>> = None;
    loop {
        let input = match from_reader.recv_timeout(idle) {
            Ok(input) => input,
            Err(mpsc::RecvTimeoutError::Timeout) => return Exit::Idle,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Exit::InputClosed,
        };
        let message = match input {
            Input::Closed => return Exit::InputClosed,
            Input::BadFrame => {
                let _ = sink.send(error_message(
                    SidecarErrorCode::BadFrame,
                    "a frame could not be read",
                ));
                return Exit::BadFrame;
            }
            Input::Message(message) => message,
        };
        let outcome = match (message, pipeline.as_mut()) {
            (ToSidecar::Quit, _) => return Exit::Quit,
            (ToSidecar::Start(config), None) => {
                let Some(starter) = start.take() else {
                    return Exit::BadConfig;
                };
                if config.validate().is_err() {
                    let _ = sink.send(error_message(
                        SidecarErrorCode::BadConfig,
                        "the start configuration is not valid",
                    ));
                    return Exit::BadConfig;
                }
                let began = Instant::now();
                match starter(&config) {
                    Ok(started) => {
                        pipeline = Some(started);
                        let load_ms =
                            u32::try_from(began.elapsed().as_millis()).unwrap_or(u32::MAX);
                        sink.send(FromSidecar::Ready { load_ms })
                    }
                    Err(failure) => {
                        let _ = sink.send(error_message(failure.code, failure.message));
                        return Exit::StartFailed;
                    }
                }
            }
            (ToSidecar::Start(_), Some(_)) | (ToSidecar::Audio(_), None) => {
                let _ = sink.send(error_message(
                    SidecarErrorCode::BadConfig,
                    "the messages are out of order",
                ));
                return Exit::BadConfig;
            }
            (ToSidecar::Audio(pcm), Some(active)) => active.audio(&pcm, &mut sink),
            (ToSidecar::Flush, Some(active)) => active
                .flush(&mut sink)
                .and_then(|()| sink.send(FromSidecar::Flushed)),
            // Nothing is loaded and nothing is pending: there is nothing to drain.
            (ToSidecar::Flush, None) => sink.send(FromSidecar::Flushed),
        };
        if outcome.is_err() {
            return Exit::OutputClosed;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use kb_speech::protocol::{ParakeetPaths, Recognizer, VadParams};

    fn config() -> StartConfig {
        StartConfig {
            recognizer: Recognizer::Parakeet {
                paths: ParakeetPaths {
                    encoder: "e".into(),
                    decoder: "d".into(),
                    joiner: "j".into(),
                    tokens: "t".into(),
                },
            },
            vad_path: "v".into(),
            vad: VadParams::default(),
            threads: 2,
            hard_cap_ms: 15_000,
            preroll_ms: 150,
        }
    }

    /// A reader fed through a channel: each message is one read, closing the sender is the end of the input.
    struct ChannelReader {
        chunks: mpsc::Receiver<Vec<u8>>,
        pending: Vec<u8>,
    }

    impl Read for ChannelReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pending.is_empty() {
                match self.chunks.recv() {
                    Ok(chunk) => self.pending = chunk,
                    Err(_) => return Ok(0),
                }
            }
            let n = self.pending.len().min(buf.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            Ok(n)
        }
    }

    /// A writer the test can read back after the loop ended.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Capture {
        fn messages(&self) -> Vec<FromSidecar> {
            let mut reader = FrameReader::new();
            reader.push(&self.0.lock().unwrap());
            let mut messages = Vec::new();
            while let Some(frame) = reader.next_frame().unwrap() {
                messages.push(FromSidecar::decode(&frame).unwrap());
            }
            messages
        }
    }

    /// A pipeline that reports each audio message as a text of its length, and a flush as one more.
    struct Counting {
        heard: Arc<Mutex<Vec<usize>>>,
    }

    impl Pipeline for Counting {
        fn audio(&mut self, pcm: &[i16], sink: &mut dyn Sink) -> Result<(), SinkClosed> {
            self.heard.lock().unwrap().push(pcm.len());
            sink.send(FromSidecar::Text {
                text: format!("{} samples", pcm.len()),
                start_ms: 0,
                end_ms: 1,
            })
        }
        fn flush(&mut self, sink: &mut dyn Sink) -> Result<(), SinkClosed> {
            sink.send(FromSidecar::Empty {
                start_ms: 1,
                end_ms: 2,
            })
        }
    }

    fn counting_starter(
        heard: &Arc<Mutex<Vec<usize>>>,
    ) -> impl FnOnce(&StartConfig) -> Result<Box<dyn Pipeline>, StartFailure> {
        let heard = Arc::clone(heard);
        move |_config| Ok(Box::new(Counting { heard }) as Box<dyn Pipeline>)
    }

    /// Runs the loop with `messages` as the input (then the end of the input unless `keep_open`), and gives back
    /// how it ended and what it wrote.
    fn drive(
        messages: &[ToSidecar],
        keep_open: bool,
        idle: Duration,
    ) -> (Exit, Capture, Vec<usize>) {
        let (sender, chunks) = mpsc::channel();
        for message in messages {
            sender.send(message.encode().unwrap()).unwrap();
        }
        // Closing the sender is the end of the input; a session that ends with Quit keeps it open.
        let held = keep_open.then_some(sender);
        let capture = Capture::default();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            capture.clone(),
            counting_starter(&heard),
            idle,
        );
        drop(held);
        let heard = heard.lock().unwrap().clone();
        (exit, capture, heard)
    }

    const LONG: Duration = Duration::from_secs(30);

    #[test]
    fn a_session_starts_listens_flushes_and_quits() {
        let (exit, capture, heard) = drive(
            &[
                ToSidecar::Start(config()),
                ToSidecar::Audio(vec![1; 100]),
                ToSidecar::Audio(vec![2; 50]),
                ToSidecar::Flush,
                ToSidecar::Quit,
            ],
            true,
            LONG,
        );
        assert_eq!(exit, Exit::Quit);
        assert_eq!(heard, [100, 50]);
        let messages = capture.messages();
        assert!(matches!(messages[0], FromSidecar::Ready { .. }));
        assert_eq!(
            &messages[1..],
            [
                FromSidecar::Text {
                    text: "100 samples".into(),
                    start_ms: 0,
                    end_ms: 1
                },
                FromSidecar::Text {
                    text: "50 samples".into(),
                    start_ms: 0,
                    end_ms: 1
                },
                FromSidecar::Empty {
                    start_ms: 1,
                    end_ms: 2
                },
                FromSidecar::Flushed,
            ]
        );
    }

    #[test]
    fn the_end_of_the_input_ends_the_session() {
        let (exit, capture, _) = drive(&[ToSidecar::Start(config())], false, LONG);
        assert_eq!(exit, Exit::InputClosed);
        assert!(exit.is_clean());
        assert_eq!(capture.messages().len(), 1);
    }

    #[test]
    fn audio_before_start_is_refused() {
        let (exit, capture, heard) = drive(&[ToSidecar::Audio(vec![0; 10])], false, LONG);
        assert_eq!(exit, Exit::BadConfig);
        assert!(heard.is_empty());
        assert!(matches!(
            capture.messages().as_slice(),
            [FromSidecar::Error {
                code: SidecarErrorCode::BadConfig,
                ..
            }]
        ));
    }

    #[test]
    fn a_second_start_is_refused() {
        let (exit, capture, _) = drive(
            &[ToSidecar::Start(config()), ToSidecar::Start(config())],
            false,
            LONG,
        );
        assert_eq!(exit, Exit::BadConfig);
        let messages = capture.messages();
        assert!(matches!(messages[0], FromSidecar::Ready { .. }));
        assert!(matches!(
            messages[1],
            FromSidecar::Error {
                code: SidecarErrorCode::BadConfig,
                ..
            }
        ));
    }

    #[test]
    fn an_invalid_configuration_is_refused_before_loading_anything() {
        let called = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&called);
        let (sender, chunks) = mpsc::channel();
        let mut bad = config();
        bad.hard_cap_ms = 0;
        sender
            .send(ToSidecar::Start(bad).encode().unwrap())
            .unwrap();
        let capture = Capture::default();
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            capture.clone(),
            move |_config: &StartConfig| {
                *flag.lock().unwrap() = true;
                Err(StartFailure {
                    code: SidecarErrorCode::ModelLoadFailed,
                    message: "never",
                })
            },
            LONG,
        );
        drop(sender);
        assert_eq!(exit, Exit::BadConfig);
        assert!(!*called.lock().unwrap());
        assert!(matches!(
            capture.messages().as_slice(),
            [FromSidecar::Error {
                code: SidecarErrorCode::BadConfig,
                ..
            }]
        ));
    }

    #[test]
    fn a_model_that_does_not_load_is_reported_and_ends_the_session() {
        let (sender, chunks) = mpsc::channel();
        sender
            .send(ToSidecar::Start(config()).encode().unwrap())
            .unwrap();
        let capture = Capture::default();
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            capture.clone(),
            |_config: &StartConfig| {
                Err(StartFailure {
                    code: SidecarErrorCode::ModelLoadFailed,
                    message: "a model file could not be loaded",
                })
            },
            LONG,
        );
        drop(sender);
        assert_eq!(exit, Exit::StartFailed);
        assert!(!exit.is_clean());
        assert_eq!(
            capture.messages(),
            [FromSidecar::Error {
                code: SidecarErrorCode::ModelLoadFailed,
                message: "a model file could not be loaded".into()
            }]
        );
    }

    #[test]
    fn a_flush_before_start_is_answered() {
        let (exit, capture, _) = drive(&[ToSidecar::Flush, ToSidecar::Quit], true, LONG);
        assert_eq!(exit, Exit::Quit);
        assert_eq!(capture.messages(), [FromSidecar::Flushed]);
    }

    #[test]
    fn garbage_on_the_input_is_a_bad_frame() {
        let (sender, chunks) = mpsc::channel();
        sender
            .send(ToSidecar::Start(config()).encode().unwrap())
            .unwrap();
        sender.send(b"this is not a frame".to_vec()).unwrap();
        let capture = Capture::default();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            capture.clone(),
            counting_starter(&heard),
            LONG,
        );
        drop(sender);
        assert_eq!(exit, Exit::BadFrame);
        let messages = capture.messages();
        assert!(matches!(messages[0], FromSidecar::Ready { .. }));
        assert!(matches!(
            messages[1],
            FromSidecar::Error {
                code: SidecarErrorCode::BadFrame,
                ..
            }
        ));
    }

    #[test]
    fn a_frame_split_across_reads_is_put_together() {
        let (sender, chunks) = mpsc::channel();
        let mut bytes = ToSidecar::Start(config()).encode().unwrap();
        bytes.extend_from_slice(&ToSidecar::Audio(vec![7; 20]).encode().unwrap());
        bytes.extend_from_slice(&ToSidecar::Quit.encode().unwrap());
        for piece in bytes.chunks(3) {
            sender.send(piece.to_vec()).unwrap();
        }
        let capture = Capture::default();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            capture.clone(),
            counting_starter(&heard),
            LONG,
        );
        drop(sender);
        assert_eq!(exit, Exit::Quit);
        assert_eq!(*heard.lock().unwrap(), [20]);
    }

    #[test]
    fn silence_on_the_input_ends_the_session_after_the_idle_time() {
        let (sender, chunks) = mpsc::channel();
        sender
            .send(ToSidecar::Start(config()).encode().unwrap())
            .unwrap();
        let capture = Capture::default();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let began = Instant::now();
        // The sender stays open: the input neither ends nor delivers anything.
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            capture.clone(),
            counting_starter(&heard),
            Duration::from_millis(150),
        );
        assert_eq!(exit, Exit::Idle);
        assert!(exit.is_clean());
        assert!(began.elapsed() >= Duration::from_millis(150));
        drop(sender);
        assert!(matches!(capture.messages()[0], FromSidecar::Ready { .. }));
    }

    #[test]
    fn a_closed_output_ends_the_session() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (sender, chunks) = mpsc::channel();
        sender
            .send(ToSidecar::Start(config()).encode().unwrap())
            .unwrap();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let exit = run(
            ChannelReader {
                chunks,
                pending: Vec::new(),
            },
            Closed,
            counting_starter(&heard),
            LONG,
        );
        drop(sender);
        assert_eq!(exit, Exit::OutputClosed);
    }
}
