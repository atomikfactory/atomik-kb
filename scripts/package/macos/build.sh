#!/usr/bin/env bash
# Builds the unsigned macOS (Apple Silicon) .app and .dmg. Run from the repository root on an arm64 Mac with the
# pinned Rust toolchain, Node and pnpm; `pnpm install --frozen-lockfile` has been run. CI calls it (_build.yml).
#
# The two sidecars (the MCP stdio bridge, ADR-0037, and the dictation sidecar kb-speech, ADR-0053) are built and
# copied where tauri expects them (binaries/<name>-<target triple>). The sherpa-onnx dylibs go to
# Contents/Frameworks (bundle.macOS.frameworks) and the sidecar finds them through an rpath relative to its own
# location (Contents/MacOS). The app is signed ad hoc ("-"): no Apple developer account is involved.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../.."
TRIPLE=aarch64-apple-darwin
BIN=apps/desktop/src-tauri/binaries
mkdir -p "$BIN"

cargo build -p kb-mcp --bin kb-mcp-bridge --release --target "$TRIPLE"
cp "target/$TRIPLE/release/kb-mcp-bridge" "$BIN/kb-mcp-bridge-$TRIPLE"

eval "$(scripts/fetch-speech-deps.sh --env)"
cargo build -p kb-speech-native --release --target "$TRIPLE"
SIDECAR="$BIN/kb-speech-$TRIPLE"
cp "target/$TRIPLE/release/kb-speech" "$SIDECAR"
cp "$SHERPA_ONNX_LIB_DIR/libsherpa-onnx-c-api.dylib" "$SHERPA_ONNX_LIB_DIR/libonnxruntime.dylib" "$BIN/"
cp "$KB_SPEECH_VAD" "$BIN/silero_vad.onnx"
chmod u+w "$BIN"/*.dylib

DYLIBS=(libsherpa-onnx-c-api.dylib libonnxruntime.dylib)
echo "Install names before:"
for f in "$SIDECAR" "$BIN/${DYLIBS[0]}" "$BIN/${DYLIBS[1]}"; do otool -L "$f"; done

# Every reference to the two libraries becomes @rpath/<name>, whatever it was linked as.
retarget() { # file
  local dep
  while read -r dep; do
    for lib in "${DYLIBS[@]}"; do
      if [ "$(basename "$dep")" = "$lib" ] && [ "$dep" != "@rpath/$lib" ]; then
        install_name_tool -change "$dep" "@rpath/$lib" "$1"
      fi
    done
  done < <(otool -L "$1" | tail -n +2 | awk '{print $1}')
}
for lib in "${DYLIBS[@]}"; do
  install_name_tool -id "@rpath/$lib" "$BIN/$lib"
  retarget "$BIN/$lib"
done
retarget "$SIDECAR"
install_name_tool -add_rpath "@loader_path/../Frameworks" "$SIDECAR"
# Editing a Mach-O file invalidates its signature; Apple Silicon refuses unsigned code.
codesign --force --sign - "$BIN"/*.dylib "$SIDECAR"

echo "Install names after:"
for f in "$SIDECAR" "$BIN/${DYLIBS[0]}" "$BIN/${DYLIBS[1]}"; do otool -L "$f"; done

# The sidecar and the libraries, alone in a folder laid out like the .app, must run.
PROBE=$(mktemp -d)
mkdir -p "$PROBE/Contents/MacOS" "$PROBE/Contents/Frameworks"
cp "$SIDECAR" "$PROBE/Contents/MacOS/kb-speech"
cp "$BIN"/*.dylib "$PROBE/Contents/Frameworks/"
env -u DYLD_LIBRARY_PATH -u DYLD_FALLBACK_LIBRARY_PATH "$PROBE/Contents/MacOS/kb-speech" --version
rm -rf "$PROBE"

cd apps/desktop
# tauri.macos.conf.json is merged on top of tauri.conf.json automatically; the bridge file adds the sidecars.
pnpm tauri build --target "$TRIPLE" --bundles dmg --config src-tauri/tauri.bridge-macos.conf.json

cd ../..
rm -rf dist/macos
mkdir -p dist/macos
cp "target/$TRIPLE"/release/bundle/dmg/*.dmg dist/macos/
ls -l dist/macos
