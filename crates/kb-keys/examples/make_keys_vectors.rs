//! Generates the golden files of the key-management formats under `fixtures/keys/`:
//! `device-log-v1.bin` (every log entry kind, `RootRotated` included),
//! `recovery-blob-v1.bin`, and the pairing messages `pairing-offer-v1.bin`,
//! `pairing-answer-v1.bin`, `pairing-identity-v1.bin` and `pairing-bundle-v1.bin`
//! (ADR-0017, ADR-0022).
//!
//! Every input is fixed in `tests/support/vectors.rs` and every random byte comes from
//! `kb_types::testing::SeededRng`, so the output is reproducible; `tests/goldens.rs`
//! regenerates the bytes and compares them with the frozen files, and opens each one.
//! Run it only when a format changes on purpose (with a version bump), and review the
//! diff like a protocol change:
//!
//! ```text
//! cargo run -p kb-keys --example make_keys_vectors
//! ```
//!
//! **Existing files are never rewritten**: a file whose bytes differ is reported and
//! kept. To regenerate one on purpose, delete it first.

// A developer tool, not library code: printing and unwrapping are fine here.
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/support/vectors.rs"]
mod vectors;

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/keys");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in vectors::generate() {
        let path = dir.join(name);
        match std::fs::read(&path) {
            Ok(existing) if existing == bytes => {
                println!("unchanged {} ({} bytes)", path.display(), bytes.len());
            }
            Ok(_) => {
                println!(
                    "KEPT {}: the frozen bytes differ from today's output (delete it to regenerate)",
                    path.display()
                );
            }
            Err(_) => {
                std::fs::write(&path, &bytes).unwrap();
                println!("wrote {} ({} bytes)", path.display(), bytes.len());
            }
        }
    }
}
