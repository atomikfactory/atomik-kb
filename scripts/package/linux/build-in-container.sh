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
# file tauri.bridge-linux.conf.json (the Windows one also lists the dictation sidecar, not staged on Linux yet) adds bundle.externalBin and is NOT part of tauri.conf.json, because tauri-build
# checks the sidecar's path at compile time, which would break every dev build and test run.
cargo build -p kb-mcp --bin kb-mcp-bridge --release
TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
mkdir -p apps/desktop/src-tauri/binaries
cp "$CARGO_TARGET_DIR/release/kb-mcp-bridge" "apps/desktop/src-tauri/binaries/kb-mcp-bridge-$TRIPLE"

cd apps/desktop
# `tauri build` runs `pnpm build` (beforeBuildCommand), then cargo build --release, then
# bundles. tauri.linux.conf.json is merged on top of tauri.conf.json automatically.
pnpm tauri build --bundles deb --config src-tauri/tauri.bridge-linux.conf.json

rm -rf /build/out
mkdir -p /build/out
cp "$CARGO_TARGET_DIR"/release/bundle/deb/*.deb /build/out/
ls -l /build/out
