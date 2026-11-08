#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="aarch64-linux-android"
JNI_LIB="${PROJECT_ROOT}/android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so"

NDK="${PROJECT_ROOT}/build/ndk/android-ndk-r27c"
[ -x "${NDK}/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android28-clang" ] \
    || { echo "ERROR: NDK r27c not found at ${NDK}; run scripts/build.sh first, which downloads and checks it" >&2; exit 1; }

cargo_home="${CARGO_HOME:-${HOME}/.cargo}"
rust_sysroot="$(rustc --print sysroot)"
rust_commit="$(rustc -vV | sed -n 's/^commit-hash: //p')"
remap="target.${TARGET}.rustflags=['--remap-path-prefix=${cargo_home}=/cargo', '--remap-path-prefix=${rust_sysroot}=/rust', '--remap-path-prefix=${rust_sysroot}/lib/rustlib/src/rust=/rustc/${rust_commit}']"

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
