//! atomik-kb desktop entry point. Everything lives in the library crate.

// Release builds on Windows are GUI applications: no console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    match atomik_kb_desktop_lib::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "the desktop shell stopped with an error");
            ExitCode::FAILURE
        }
    }
}
