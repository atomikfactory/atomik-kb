//! `kb-speech`: the dictation sidecar (ADR-0053, `docs/architecture/dictation.md` section 3).
//!
//! Started by the desktop shell for one session, with its stdin and stdout piped: frames in, frames out
//! (`kb_speech::protocol`). It exits on `Quit`, when stdin closes, or after 60 seconds without a frame, so a
//! model is never held in memory longer than a session. Diagnostics go to stderr and never hold what the
//! person said, a path or a model's name.
//!
//! `kb-speech --version` prints the sherpa-onnx and onnxruntime versions to stdout and exits. It loads the
//! native libraries, so it is also the check that the DLLs sit next to the executable.

#![forbid(unsafe_code)]
// stdout is the frame channel during a session; the one thing printed there otherwise is `--version`. Diagnostics
// are for the developer reading the shell's stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod sherpa_pipeline;

use std::process::ExitCode;

use kb_speech::policy::IDLE_EXIT;
use kb_speech_native::{Exit, run};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--version") => {
            println!("{}", sherpa_pipeline::version_line());
            ExitCode::SUCCESS
        }
        Some(_) => {
            eprintln!("kb-speech: a sidecar of atomik-kb; it takes no arguments but --version");
            ExitCode::from(2)
        }
        None => {
            let exit = run(
                std::io::stdin(),
                std::io::stdout(),
                sherpa_pipeline::start,
                IDLE_EXIT,
            );
            eprintln!("kb-speech: ended ({exit:?})");
            if exit == Exit::OutputClosed || exit.is_clean() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
