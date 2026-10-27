//! Generates the golden fixtures of document format v1 in `fixtures/doc/`:
//!
//! - `structure-v1.snapshot.bin` / `structure-v1.portable.json`
//! - `structure-v1-attachments.snapshot.bin` / `structure-v1-attachments.portable.json`
//!   (the same document continued with attachment manifests, Milestone 5)
//! - `structure-v1-properties.snapshot.bin` / `structure-v1-properties.portable.json`
//!   (the same document continued with a property schema and values, ADR-0026)
//! - `structure-v1-links.snapshot.bin` / `structure-v1-links.state.json` (the same document continued
//!   with the view links of Milestone 13 slice 4; the JSON is the document's deep value, because
//!   `to_portable()` does not list the links)
//! - `account-v1.snapshot.bin` / `account-v1.portable.json`
//! - `account-v1-sharing.snapshot.bin` / `account-v1-sharing.state.json` (the same account continued
//!   with the maps of shared spaces, Milestone 13; the JSON is the document's deep value, because
//!   `to_portable()` does not list those maps)
//! - `page-v1.snapshot.bin` / `page-v1.md`, built from the canonical sample
//!   `page-v1.portable.json` (an input, written by hand like
//!   `page-v1-unknown.portable.json`, whose snapshot is `page-v1-unknown.snapshot.bin`)
//!
//! The snapshots are enveloped blobs exactly as they are persisted; the JSON files are
//! the documents' `to_portable()` output. The test `tests/golden.rs` loads the frozen
//! blobs with the current code and requires the same JSON, so old data keeps reading
//! the same way. Run this only when the document format changes on purpose, together
//! with a `FORMAT_VERSION` bump (and a reader for the old version), and review the
//! diff like a protocol change:
//!
//! ```text
//! cargo run -p kb-model --example make_golden
//! ```
//!
//! The scripts (`tests/support/golden_script.rs`, `tests/support/page_golden.rs`) use
//! fixed peer ids and timestamps.
//!
//! **Snapshots that already exist are never rewritten**: only missing `*.snapshot.bin`
//! files are written, so a rerun cannot replace frozen bytes (a rerun of the same script
//! does not reproduce the structure and account snapshots byte for byte). The JSON
//! and Markdown outputs are rewritten every time; the tests then check that the frozen
//! snapshots still load into them. To regenerate a snapshot on purpose, delete it first.

// A developer tool, not library code: printing and unwrapping are fine here.
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/support/golden_script.rs"]
mod golden_script;
#[path = "../tests/support/page_golden.rs"]
mod page_golden;

use std::path::{Path, PathBuf};

/// Writes a fixture; a snapshot (`*.snapshot.bin`) that already exists is kept.
fn write(dir: &Path, name: &str, bytes: &[u8]) {
    let path = dir.join(name);
    if name.ends_with(".snapshot.bin") && path.exists() {
        println!("kept {} (frozen; delete it to regenerate)", path.display());
        return;
    }
    std::fs::write(&path, bytes).unwrap();
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
}

fn json(value: &serde_json::Value) -> Vec<u8> {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text.into_bytes()
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/doc");
    std::fs::create_dir_all(&dir).unwrap();

    let structure = golden_script::structure();
    let problems = structure.check_invariants();
    assert!(
        problems.is_empty(),
        "golden structure is unhealthy: {problems:?}"
    );
    write(
        &dir,
        "structure-v1.snapshot.bin",
        &structure.doc().export_snapshot().unwrap(),
    );
    write(
        &dir,
        "structure-v1.portable.json",
        &json(&structure.to_portable()),
    );

    let with_attachments = golden_script::structure_with_attachments();
    let problems = with_attachments.check_invariants();
    assert!(
        problems.is_empty(),
        "golden structure with attachments is unhealthy: {problems:?}"
    );
    write(
        &dir,
        "structure-v1-attachments.snapshot.bin",
        &with_attachments.doc().export_snapshot().unwrap(),
    );
    write(
        &dir,
        "structure-v1-attachments.portable.json",
        &json(&with_attachments.to_portable()),
    );

    let with_properties = golden_script::structure_with_properties();
    let problems = with_properties.check_invariants();
    assert!(
        problems.is_empty(),
        "golden structure with properties is unhealthy: {problems:?}"
    );
    write(
        &dir,
        "structure-v1-properties.snapshot.bin",
        &with_properties.doc().export_snapshot().unwrap(),
    );
    write(
        &dir,
        "structure-v1-properties.portable.json",
        &json(&with_properties.to_portable()),
    );

    let with_links = golden_script::structure_with_links();
    let problems = with_links.check_invariants();
    assert!(
        problems.is_empty(),
        "golden structure with links is unhealthy: {problems:?}"
    );
    write(
        &dir,
        "structure-v1-links.snapshot.bin",
        &with_links.doc().export_snapshot().unwrap(),
    );
    write(
        &dir,
        "structure-v1-links.state.json",
        &json(&loro::ToJson::to_json_value(
            &with_links.doc().inner().get_deep_value(),
        )),
    );

    let account = golden_script::account();
    write(
        &dir,
        "account-v1.snapshot.bin",
        &account.doc().export_snapshot().unwrap(),
    );
    write(
        &dir,
        "account-v1.portable.json",
        &json(&account.to_portable()),
    );

    let sharing = golden_script::account_with_sharing();
    write(
        &dir,
        "account-v1-sharing.snapshot.bin",
        &sharing.doc().export_snapshot().unwrap(),
    );
    write(
        &dir,
        "account-v1-sharing.state.json",
        &json(&loro::ToJson::to_json_value(
            &sharing.doc().inner().get_deep_value(),
        )),
    );

    let page = page_golden::page();
    let problems = page.check_invariants();
    assert!(
        problems.is_empty(),
        "golden page is unhealthy: {problems:?}"
    );
    assert_eq!(
        page.to_portable(),
        page_golden::json(page_golden::PAGE_JSON),
        "the sample must be in portable form"
    );
    write(
        &dir,
        "page-v1.snapshot.bin",
        &page.doc().export_snapshot().unwrap(),
    );
    write(&dir, "page-v1.md", page_golden::markdown(&page).as_bytes());

    let unknown = page_golden::unknown_page();
    assert_eq!(
        unknown.to_portable(),
        page_golden::json(page_golden::UNKNOWN_JSON),
        "the unknown-content sample must be in portable form"
    );
    write(
        &dir,
        "page-v1-unknown.snapshot.bin",
        &unknown.doc().export_snapshot().unwrap(),
    );
}
