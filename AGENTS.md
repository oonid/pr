# AGENTS.md

Guide for AI coding agents working on this project.

## Project Overview

**pr** — Android app that runs Linux distributions via proot (ptrace-based root).
- Android package: `id.or.oo.pr`
- proot version: `5.4.0-pr`, fake kernel: `6.17.0-pr`
- Languages: C (proot), Rust (pr-cli, test binary), Kotlin (Android app), POSIX sh (bootstrap)

## Privacy Policy

**NEVER include personal device information in code, comments, commits, or docs.**

Specifically:
- **NO device model numbers** — mask partial: `SM-XXXXX` is OK, full model is not
- **NO marketing names** — mask partial: `Galaxy X` is OK, full product name is not
- **NO device serial numbers** — mask like `ABCD12XXXX` is OK, real serial is not
- **NO adb serials** in commands, scripts, or documentation — use bare `adb`

**OK to include:**
- Manufacturer: Samsung
- Architecture: aarch64, arm64-v8a
- OS version: Android 16 (SDK 36)
- SELinux context: untrusted_app, runas_app

When writing adb commands in docs/scripts, use bare `adb` without `-s <serial>`.

## Directory Structure

```
src/proot/                  # Patched proot C source (working copy, NOT vendor/)
src/pr-cli/                 # Rust CLI (replaces proot-distro.sh)
src/proot-integration-test/ # Guest-side test binary (runs inside proot, TAP output)
src/scripts/                # Shell scripts: bootstrap.sh (POSIX)
android/                    # Android APK (Kotlin + Compose + JNI)
scripts/                    # Host-side build scripts (build.sh, download-*.sh)
vendor/                     # Git submodules — READ ONLY, never modify
docs/                       # Architecture and feature documentation
build/                      # Build artifacts (gitignored)
```

## Build Process

### 1. Build proot (C, NDK cross-compilation)

```bash
scripts/build.sh --arch=arm64
# Output: build/out/arm64/proot, build/out/arm64/loader
# Copy to android/proot-engine/src/main/jniLibs/arm64-v8a/libproot.so
# Copy to android/proot-engine/src/main/jniLibs/arm64-v8a/libproot-loader.so
```

### 2. Build pr-cli (Rust, NDK cross-compilation)

```bash
# MUST run from src/pr-cli/ (required for .cargo/config.toml resolution)
cd src/pr-cli && cargo build --target aarch64-linux-android --release
# Output: target/aarch64-linux-android/release/pr-cli (~900KB)
# Copy to android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so
```

### 3. Build test binary (guest-side, cross-compiled)

```bash
# MUST run from src/proot-integration-test/
cd src/proot-integration-test && cargo build --target aarch64-linux-android --release
# Embedded into pr-cli via include_bytes! at build time
# Must rebuild pr-cli after rebuilding test binary
```

### 4. Build APK

```bash
cd android && ./gradlew assembleDebug
# Output: android/app/build/outputs/apk/debug/app-debug.apk
```

### 5. Install on device

```bash
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
# User must force-close and reopen app after install
```

### Full rebuild sequence (after changing test binary)

```bash
# 1. Build test binary
cd src/proot-integration-test && cargo build --target aarch64-linux-android --release
# 2. Build pr-cli (embeds test binary)
cd src/pr-cli && cargo build --target aarch64-linux-android --release
# 3. Copy to jniLibs (ALWAYS verify with md5sum)
cp -f src/pr-cli/target/aarch64-linux-android/release/pr-cli \
      android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so
md5sum src/pr-cli/target/aarch64-linux-android/release/pr-cli \
       android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so
# 4. Build APK
cd android && rm -rf app/build && ./gradlew assembleDebug
# 5. Install
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

**IMPORTANT:** Gradle caches jniLibs — always verify the copy succeeded with `md5sum` and clean `app/build` before building the APK.

## Testing

### Host-side (pr-cli unit/integration)

```bash
cd src/pr-cli && cargo test
# 169 tests: 144 library unit + 2 binary + 23 integration (14 plugin parsers)
```

### On-device integration tests

Run from app UI via BugReport button on each distro row, or:

```bash
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test alpine
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test debian
```

**Test suites:** distro (8), clone (5), readlink (7), gcc (3), rust (4), git (3), pipe (3), general (9), ssh (4)

**Results:** 46/46 pass on Alpine and Ubuntu 26.04 (Android 16, 2026-10), including rust (rustc compile + cargo build).

### Known test failures (Alpine-specific)

- **rust suite on Alpine**: Previously 1/4 pass — rustc compile and cargo build failed with "linker `cc` not found". **Root cause:** hardcoded package name in bind mounts (see `docs/pr-improvements.md` #1). The bind mount pointed at `/data/data/id.or.oo.pr` regardless of the actual app package, which broke `.l2s` symlink resolution. Fixed by deriving `app_dir` dynamically from `APP_PREFIX` in `shared.rs`.

## Code Conventions

### Working directory requirements

- `src/pr-cli/` — must be cwd for cargo build (`.cargo/config.toml`)
- `src/proot-integration-test/` — must be cwd for cargo build
- `android/` — must be cwd for gradlew

### Vendor directories are READ ONLY

All modifications go in `src/proot/` (working copy), never in `vendor/`.

### Native library disguise

All native binaries (proot, loader, busybox, bash, pr-cli) are named `lib*.so` in jniLibs/ so Android extracts them to nativeLibraryDir where SELinux allows execve. They are standalone ELF executables, not shared libraries.

### Environment variable contract

- `APP_PREFIX` — app files directory (e.g. `/data/data/id.or.oo.pr/files/usr`)
- `APP_HOME` — app home directory
- `APP_PACKAGE` — `id.or.oo.pr`
- `PROOT_NO_SECCOMP=1` — disables proot's own seccomp filter (not the zygote's)
- `PROOT_LOADER` — path to proot loader in nativeLibraryDir

### Shell commands inside proot

The test binary is bionic (Android). It cannot directly exec dynamic distro (musl/glibc) binaries inside proot — gets ENOSYS. **ALL commands that exec distro binaries must go through `/bin/sh -c`:**

```rust
// BROKEN — ENOSYS:
Command::new("apk").args(["update"]).output()

// CORRECT:
Command::new("/bin/sh").args(["-c", "apk update 2>&1"]).output()
```

### Proot caveats

**`touch` on older proot builds:** before the utimensat ENOENT fix, `touch /new/file` exited 0 without creating the file. Current builds create it; scripts that must also run on older builds can keep `echo > /path/to/file`. See `docs/important-notes.md` § pr-cli Caveats.

**Double shell wrapping via `pr-cli login`:** `pr-cli login <distro> -- <args>` internally prepends `/bin/sh -l -c "<args.join(' ')>"`. Never pass `/bin/sh -c` as trailing args — it causes double wrapping and broken quoting:
```bash
# WRONG — double-wrapped, quotes get stripped:
pr-cli login alpine -- /bin/sh -c "apk update && apk add rust"

# CORRECT — pass the raw command string directly:
pr-cli login alpine -- "apk update && apk add rust"
```
See `docs/important-notes.md` § pr-cli Caveats.

**OCI container path vs legacy plugin path:** OCI installs land in `var/lib/pr/containers/<alias>/rootfs/`. Legacy plugin installs land in `var/lib/pr/installed-rootfs/<distro>/`. See `docs/android-apk-architecture.md` § OCI Container Paths.

### Commit style

Format: `feat|fix|chore|docs|test: <description>`
Examples: `fix: fake root and kernel-release for Debian`

Commit message policy:
- Do not add `Co-authored-by` trailers.
- Keep subject/body informative, with wrapped body lines (target <= 72 cols).
- Avoid migration-marketing phrasing; describe technical changes directly.

### Coverage gate policy

- Preserve or improve test coverage on touched code.
- Do not close a task unless coverage is at least 80%.
- Prefer targeted tests first, then broader coverage runs when needed.

### Code style

- No comments unless explicitly requested
- Follow existing patterns in each file
- Use `cfg(target_os = "android")` to gate Android-specific code for host testability

## Critical Constraints

1. **targetSdk is 35** — Play Store minimum. Works via PROOT_LOADER mechanism (unbundled loader binary in nativeLibraryDir).
2. **W^X / SELinux** — Only nativeLibraryDir allows execve. Files in app data cannot be exec'd. The PROOT_LOADER mechanism works around this.
3. **Zygote seccomp** blocks 18+ syscalls. Proot has SIGSYS handlers — never remove them. See `docs/important-notes.md` for full list.
4. **bootstrap.sh must be POSIX sh** — runs before bash is available.
5. **ARM64 x0 clobber bug** — register x0 may be clobbered by kernel before proot reads it at SIGSYS time. All handlers use ORIGINAL register version.
6. **TLS alignment** — proot needs 64-byte PT_TLS alignment for Bionic. Build script patches this.
7. **Fake root required** — `--change-id=0:0` (proot `-0`) must always be passed. dpkg and other tools check `getuid() == 0`.
8. **Kernel release format** — `--kernel-release` takes ONLY the release string (e.g. `6.17.0-pr`), NOT the full `uname` output. The libc6 preinst script parses this with shell arithmetic.

## Superpowers Workflow

Use the Superpowers workflow and skills (`subagent-driven-development`, `brainstorming`, `writing-plans`, `executing-plans`, etc.) for managing tasks, implementing changes, and verifying logic.

## Key References

- `docs/important-notes.md` — Critical constraints, seccomp handlers, pr-cli caveats, read first
- `docs/proot-improvement.md` — Our proot fork vs vendor/proot and vendor/termux-proot; §29 covers bind mount bug
- `docs/rust-toolchain-support.md` — vfork/CLONE_VM fix, link2symlink readlink fix
- `docs/integration-tests.md` — Integration test suite (46 tests)
- `docs/targetsdk35-compatibility.md` — targetSdk 35 compatibility (PROOT_LOADER mechanism)
