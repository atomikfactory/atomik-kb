#!/usr/bin/env bash
# Runs inside the build image (see scripts/package/linux.ps1): unpacks the source tarball
# the host streamed in, installs the JS dependencies from the lockfile, and builds the .deb.
set -euo pipefail

SRC=/build/src
rm -rf "$SRC"
mkdir -p "$SRC"
# -m: stamp every file with the extraction time. Keeping the host mtimes would let cargo
# treat a file edited on the host as older than the cached build output (stale artifacts).
tar -xmf /build/source.tar -C "$SRC"
cd "$SRC"

# Named volumes: the pnpm store, the cargo registry and the container's own target dir.
export CARGO_TARGET_DIR=/cache/target
pnpm config set store-dir /cache/pnpm-store >/dev/null
pnpm install --frozen-lockfile

# The stdio bridge of the MCP server (ADR-0037) ships next to the app as a sidecar (/usr/bin/kb-mcp-bridge).
# It is built here and copied where tauri expects it (binaries/kb-mcp-bridge-<target triple>); the merge
# file tauri.bridge-linux.conf.json adds bundle.externalBin (and the dictation resources) and is NOT part of
# tauri.conf.json, because tauri-build checks the sidecar's path at compile time, which would break every dev
# build and test run.
cargo build -p kb-mcp --bin kb-mcp-bridge --release
TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
BIN=apps/desktop/src-tauri/binaries
mkdir -p "$BIN"
cp "$CARGO_TARGET_DIR/release/kb-mcp-bridge" "$BIN/kb-mcp-bridge-$TRIPLE"

# The dictation sidecar (ADR-0053) ships as /usr/bin/kb-speech. Its sherpa-onnx libraries and the Silero VAD model
# are bundle resources, installed to /usr/lib/atomik-kb/ (where the shell looks for the model: the resource
# directory). The sidecar finds the libraries through an rpath relative to its own location.
eval "$(scripts/fetch-speech-deps.sh --env)"
cargo build -p kb-speech-native --release
cp "$CARGO_TARGET_DIR/release/kb-speech" "$BIN/kb-speech-$TRIPLE"
# The build script already adds $ORIGIN; add the installed library directory (relative, so it survives a relocation).
patchelf --set-rpath '$ORIGIN:$ORIGIN/../lib/atomik-kb' "$BIN/kb-speech-$TRIPLE"
cp "$SHERPA_ONNX_LIB_DIR/libsherpa-onnx-c-api.so" "$SHERPA_ONNX_LIB_DIR/libonnxruntime.so" "$BIN/"
cp "$KB_SPEECH_VAD" "$BIN/silero_vad.onnx"
# The sidecar and the libraries, alone in a folder laid out like the install (bin/ and lib/atomik-kb/), must run.
PROBE=$(mktemp -d)
mkdir -p "$PROBE/bin" "$PROBE/lib/atomik-kb"
cp "$BIN/kb-speech-$TRIPLE" "$PROBE/bin/kb-speech"
cp "$BIN/libsherpa-onnx-c-api.so" "$BIN/libonnxruntime.so" "$PROBE/lib/atomik-kb/"
env -u LD_LIBRARY_PATH "$PROBE/bin/kb-speech" --version
rm -rf "$PROBE"

cd apps/desktop
# `tauri build` runs `pnpm build` (beforeBuildCommand), then cargo build --release, then
# bundles. tauri.linux.conf.json is merged on top of tauri.conf.json automatically.
pnpm tauri build --bundles deb --config src-tauri/tauri.bridge-linux.conf.json

rm -rf /build/out
mkdir -p /build/out
cp "$CARGO_TARGET_DIR"/release/bundle/deb/*.deb /build/out/
ls -l /build/out
