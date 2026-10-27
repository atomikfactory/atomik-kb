//! Generates the golden files of sync protocol v2 under `fixtures/sync/`: `entry-v2.bin`,
//! `keyring-v2.bin`, `snapshot-v2.bin`, `head-v2.bin`, `changes-page-v2.bin` (ADR-0045) and, for the
//! WebSocket of M13 slice 3 (ADR-0050), `presence-v2.bin`, `ws-entry-frame-v2.bin`,
//! `ws-presence-frame-v2.bin`, `ws-subscribe-v2.json` and `ws-changed-v2.json`, and for the view link
//! of slice 4 (ADR-0051) `publication-v2.bin`.
//!
//! Every input is fixed in `tests/support/vectors_v2.rs` and every random byte comes from
//! `kb_types::testing::SeededRng`, so the output is reproducible; `tests/goldens_v2.rs`
//! regenerates the bytes and compares them with the frozen files, and opens each one. Run it only
//! when a wire format changes on purpose (with a protocol version bump), and review the diff like a
//! protocol change:
//!
//! ```text
//! cargo run -p kb-sync --example make_sync_vectors_v2
//! ```
//!
//! **Existing files are never rewritten**: a file whose bytes differ is reported and kept. To
//! regenerate one on purpose, delete it first.

// A developer tool, not library code: printing and unwrapping are fine here.
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/support/vectors_v2.rs"]
mod vectors;

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sync");
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
