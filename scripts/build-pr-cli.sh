#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-linux-android"
JNI_LIB="${PROJECT_ROOT}/android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so"

cargo_home="${CARGO_HOME:-${HOME}/.cargo}"
rust_sysroot="$(rustc --print sysroot)"
remap="target.${TARGET}.rustflags=['--remap-path-prefix=${cargo_home}=/cargo', '--remap-path-prefix=${rust_sysroot}=/rust']"

build() {
    echo "==> Building $1"
    (cd "${PROJECT_ROOT}/src/$1" && cargo build --release --target "$TARGET" --config "$remap")
}

build proot-integration-test
build pr-cli

out="${PROJECT_ROOT}/src/pr-cli/target/${TARGET}/release/pr-cli"
cp -f "$out" "$JNI_LIB"
cmp -s "$out" "$JNI_LIB" || { echo "ERROR: ${JNI_LIB} differs from ${out}" >&2; exit 1; }

leaks=$(strings "$JNI_LIB" | grep -cF -e "$cargo_home" -e "$rust_sysroot" -e "$PROJECT_ROOT" || true)
[ "$leaks" -eq 0 ] || { echo "ERROR: ${leaks} build-machine paths in ${JNI_LIB}" >&2; exit 1; }

echo "==> ${JNI_LIB} ($(du -h "$JNI_LIB" | cut -f1)), no build-machine paths"
