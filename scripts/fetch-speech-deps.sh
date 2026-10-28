#!/usr/bin/env bash
# Fetches what the dictation sidecar needs to build and run on Linux x64 (ADR-0053, docs/architecture/dictation.md):
# the sherpa-onnx native libraries (libsherpa-onnx-c-api.so, libonnxruntime.so) and the Silero voice activity
# model. Both land in .build/speech/ (ignored by git), checked against the SHA-256 of the pinned release asset.
# Idempotent. The Linux counterpart of scripts/fetch-speech-deps.ps1.
#
#   scripts/fetch-speech-deps.sh              # prints the two variables
#   eval "$(scripts/fetch-speech-deps.sh --env)"   # exports SHERPA_ONNX_LIB_DIR and KB_SPEECH_VAD
#
# Run it from a bash with curl, tar and bzip2 (the Linux build image has them).
set -euo pipefail

ARCHIVE_NAME="sherpa-onnx-v1.13.8-linux-x64-shared-lib.tar.bz2"
ARCHIVE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/$ARCHIVE_NAME"
ARCHIVE_SHA256="3892d184be41027e18165e67f549cd4e4cdd8dcd73ac5579e97afd55e14e30b6"
EXTRACTED_DIR="sherpa-onnx-v1.13.8-linux-x64-shared-lib"

# Silero VAD (MIT). The release tag is mutable, so the file is pinned by its hash only.
VAD_NAME="silero_vad.onnx"
VAD_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"
VAD_SHA256="9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$REPO/.build/speech"
LIB_DIR="$ROOT/$EXTRACTED_DIR/lib"
VAD_PATH="$ROOT/$VAD_NAME"

sha_ok() { [ -f "$1" ] && [ "$(sha256sum "$1" | cut -d' ' -f1)" = "$2" ]; }
libs_ok() { [ -f "$LIB_DIR/libsherpa-onnx-c-api.so" ] && [ -f "$LIB_DIR/libonnxruntime.so" ]; }

download() { # url path sha256
  echo "Downloading $1" >&2
  rm -f "$2.part"
  curl -fsSL --retry 3 -o "$2.part" "$1"
  if ! sha_ok "$2.part" "$3"; then
    rm -f "$2.part"
    echo "$1 is not the pinned file (SHA-256 differs); nothing was kept" >&2
    exit 1
  fi
  mv -f "$2.part" "$2"
}

mkdir -p "$ROOT"

if ! libs_ok || ! sha_ok "$ROOT/$ARCHIVE_NAME" "$ARCHIVE_SHA256"; then
  sha_ok "$ROOT/$ARCHIVE_NAME" "$ARCHIVE_SHA256" || download "$ARCHIVE_URL" "$ROOT/$ARCHIVE_NAME" "$ARCHIVE_SHA256"
  rm -rf "$ROOT/unpack.tmp" "$ROOT/$EXTRACTED_DIR"
  mkdir -p "$ROOT/unpack.tmp"
  tar -xjf "$ROOT/$ARCHIVE_NAME" -C "$ROOT/unpack.tmp"
  mv "$ROOT/unpack.tmp/$EXTRACTED_DIR" "$ROOT/$EXTRACTED_DIR"
  rm -rf "$ROOT/unpack.tmp"
  libs_ok || { echo "the unpacked archive does not hold the expected libraries in $LIB_DIR" >&2; exit 1; }
  echo "Native libraries ready: $LIB_DIR" >&2
else
  echo "Native libraries already there: $LIB_DIR" >&2
fi

if ! sha_ok "$VAD_PATH" "$VAD_SHA256"; then
  download "$VAD_URL" "$VAD_PATH" "$VAD_SHA256"
  echo "Voice activity model ready: $VAD_PATH" >&2
else
  echo "Voice activity model already there and verified: $VAD_PATH" >&2
fi

if [ "${1:-}" = "--env" ]; then
  echo "export SHERPA_ONNX_LIB_DIR='$LIB_DIR'"
  echo "export KB_SPEECH_VAD='$VAD_PATH'"
else
  echo "SHERPA_ONNX_LIB_DIR = $LIB_DIR"
  echo "KB_SPEECH_VAD       = $VAD_PATH"
fi
