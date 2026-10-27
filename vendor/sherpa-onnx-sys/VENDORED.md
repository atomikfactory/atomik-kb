# Vendored: sherpa-onnx-sys 1.13.8

This directory is a copy of the crate `sherpa-onnx-sys` 1.13.8 from crates.io ("Raw FFI bindings to the
sherpa-onnx C API", repository https://github.com/k2-fsa/sherpa-onnx, path `sherpa-onnx/rust/sherpa-onnx-sys`),
used through `[patch.crates-io]` in the workspace `Cargo.toml`. It is excluded from the workspace
(`exclude`), so `cargo clippy --workspace` and `cargo test --workspace` do not touch it.

- **Upstream:** `sherpa-onnx-sys` 1.13.8 (the newest release when this was vendored, 2026-10-16;
  upstream master was checked and still says `zip = "2"`).
- **The one change** (to the `zip` build-dependency, in `Cargo.toml`): `[build-dependencies.zip]` `version = "2"`
  became `version = "9"`, and `default-features = false` was added. Nothing else differs: `build.rs`, `src/` and
  `LICENSE` are the published files; `Cargo.lock`, `Cargo.toml.orig` and `.cargo_vcs_info.json` are not copied.
  Without `default-features = false` the build script would pull zip's default compression crates (zstd, bzip2,
  lzma, AES and more) into the lockfile, and `cargo deny` rejects the `bzip2-1.0.6` licence of one of them.
- **Why:** every `zip` 2, 3, 4 and 5 release on crates.io is yanked, so a fresh dependency resolution of the
  published crate fails ("version 2.x.y is yanked"). The build script uses `zip` only to unpack the iOS
  xcframework archive (`ZipArchive::new` and `extract`), a branch that never runs here, so a newer major version
  with no compression features is safe. Building for iOS would need zip's `deflate` feature.
- **Licence:** Apache-2.0 (the `LICENSE` file next to this one is upstream's). `cargo deny` checks it like any
  other dependency.
- **Remove this directory** (and the `[patch.crates-io]` entry and the `exclude`) when upstream publishes a
  release whose `zip` requirement resolves, then run `cargo update -p sherpa-onnx-sys`.
- **Using it:** `SHERPA_ONNX_LIB_DIR=<...>\lib` (see `scripts/fetch-speech-deps.ps1` and `scripts/dev-env.ps1`)
  makes the build script skip its download and copy the DLLs into `target\<profile>\`.
  Design: `docs/architecture/dictation.md`, ADR-0053.
