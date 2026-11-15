#!/usr/bin/env bash
# Builds the Rust engine (crates/kb-mobile) for Android and generates its Kotlin bindings.
#
#   scripts/android/build-engine.sh [--release] [--abi arm64-v8a --abi x86_64]
#
# Output (neither is committed):
#   apps/android/engine/src/main/jniLibs/<abi>/libkb_mobile.so
#   apps/android/engine/build/generated/uniffi/   (package io.atomik.kb.engine.ffi)
#
# Run it from Git Bash. On the development machine (Windows) the environment below is what makes the
# vendored OpenSSL (SQLCipher) build: MSYS perl with the modules in PERL5LIB, and a target directory
# without spaces. Elsewhere set KB_ANDROID_ENV=skip to keep your own environment.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
profile=debug
profile_flag=()
abis=()
while [ $# -gt 0 ]; do
  case "$1" in
    --release) profile=release; profile_flag=(--release) ;;
    --abi) shift; abis+=("$1") ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ ${#abis[@]} -eq 0 ]; then abis=(arm64-v8a x86_64); fi

orig_path="$PATH"
if [ "${KB_ANDROID_ENV:-}" != "skip" ]; then
  export MSYS2_ENV_CONV_EXCL=PERL5LIB
  export PERL5LIB=/c/Users/Ben/.cache/kb-android-perl5lib
  export OPENSSL_SRC_PERL='C:\Users\Ben\.cache\kb-android-perl.cmd'
  export PATH="/usr/bin:$HOME/.cargo/bin:$PATH"
  export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$LOCALAPPDATA/Android/Sdk/ndk/27.1.12297006}"
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/Users/Ben/.cache/kb-android-target}"
  export CARGO_HTTP_PROXY="${CARGO_HTTP_PROXY:-http://127.0.0.1:3128}"
fi

# The workspace release profile aborts on panic; the engine catches panics at the FFI boundary, which needs unwinding.
export CARGO_PROFILE_RELEASE_PANIC=unwind

jni="$root/apps/android/engine/src/main/jniLibs"
out="$root/apps/android/engine/build/generated/uniffi"
mkdir -p "$jni" "$out"

cd "$root"
targets=()
for abi in "${abis[@]}"; do targets+=(-t "$abi"); done
# Release keeps symbols here (the bindings are read from the library's metadata, which strip removes); the copies
# in jniLibs are stripped afterwards with the NDK's llvm-strip.
export CARGO_PROFILE_RELEASE_STRIP=none
cargo ndk "${targets[@]}" --platform 28 -o "$jni" build -p kb-mobile ${profile_flag[@]+"${profile_flag[@]}"}

# Kotlin bindings, read from the library's own metadata (an Android .so parses on any host).
case "${abis[0]}" in
  arm64-v8a) triple=aarch64-linux-android ;;
  x86_64) triple=x86_64-linux-android ;;
  armeabi-v7a) triple=armv7-linux-androideabi ;;
  x86) triple=i686-linux-android ;;
  *) echo "unknown ABI ${abis[0]}" >&2; exit 2 ;;
esac
target_dir="${CARGO_TARGET_DIR:-$root/target}"
lib="$target_dir/$triple/$profile/libkb_mobile.so"

# The generator is a host program that links the crate, so it needs a host build of SQLCipher: that takes the
# host toolchain (MSVC, Strawberry Perl, NASM on Windows), not the Android perl above, and its own target dir.
host_env=()
host_target="${KB_HOST_TARGET_DIR:-$target_dir-host}"
if [ "${KB_ANDROID_ENV:-}" != "skip" ]; then
  host_target="${KB_HOST_TARGET_DIR:-C:/Users/Ben/.cache/kb-host-target}"
  progs="$LOCALAPPDATA/Programs"
  host_env=(env -u PERL5LIB -u OPENSSL_SRC_PERL -u MSYS2_ENV_CONV_EXCL -u ANDROID_NDK_HOME
    "PATH=$HOME/.cargo/bin:$progs/StrawberryPerl/perl/bin:$progs/StrawberryPerl/c/bin:$progs/NASM:$orig_path")
fi
CARGO_TARGET_DIR="$host_target" ${host_env[@]+"${host_env[@]}"}   cargo run -p kb-mobile --features bindgen --bin uniffi-bindgen -- generate   --library "$lib" --language kotlin --out-dir "$out"

if [ "$profile" = release ]; then
  ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
  strip_bin="$(ls "$ndk"/toolchains/llvm/prebuilt/*/bin/llvm-strip* 2>/dev/null | head -n 1)"
  if [ -n "$strip_bin" ]; then
    for abi in "${abis[@]}"; do "$strip_bin" --strip-unneeded "$jni/$abi/libkb_mobile.so"; done
  else
    echo "llvm-strip not found: jniLibs keep their symbols" >&2
  fi
fi

echo "engine built: $jni"
echo "bindings:     $out"
