# Proot Integration Test Suite

Automated regression testing for proot behavior inside Linux distros (Alpine,
Debian, etc.). A cross-compiled guest binary runs inside proot via `pr-cli test <distro>`,
exercising syscall interception, filesystem virtualization, and toolchain support.

Result: **46/46 pass** on Alpine and Ubuntu 26.04 (Android 16, 2026-10).

---

## Architecture

- **Guest binary** (`src/proot-integration-test/`): cross-compiled for aarch64-linux-android,
  linked against bionic (Android). Runs inside proot, outputs TAP protocol to stdout,
  diagnostics to stderr. Auto-probes prerequisites and skips unavailable tests.
- **Host runner** (`pr-cli test <distro>` in `src/pr-cli/src/cmd_test.rs`): deploys the
  pre-compiled guest binary (embedded via `include_bytes!`) to the rootfs, invokes proot
  per-suite, captures merged stdout+stderr, parses TAP, presents results.
- **Multi-distro**: works with any installed distro (Alpine, Debian, etc.) — the guest
  binary is statically available in the APK and deployed to `/tmp/pit` inside proot.

### Three-stage pipeline

1. **Install tools** via proot+sh (`apk add` / `apt install`) — only if `rustc` not present
2. **Deploy test binary** from embedded bytes (`include_bytes!`) to `{cache_dir}/pit`
3. **Run binary per-suite** inside proot, capture TAP output, parse and report results

---

## Project Structure

```
src/proot-integration-test/
  Cargo.toml              libc = "0.2" dependency
  .cargo/config.toml      NDK cross-compilation (aarch64-linux-android)
  src/main.rs             TAP runner: proot-integration-test [suite|all]
  src/distro.rs           Package manager detection + tool installation (8 tests)
  src/clone.rs            CLONE_VM stripping tests (5 tests)
  src/readlink.rs         .l2s. hiding, readlink, realpath tests (7 tests)
  src/gcc.rs              GCC prefix resolution, C compilation (3 tests)
  src/rust.rs             rustc, cargo build with timing diagnostics (4 tests)
  src/git.rs              git init, git config, cargo new with vcs git (3 tests)
  src/pipe.rs             pipe/pipe2 syscall availability (3 tests)
  src/general.rs          file I/O, symlinks, pipes, signals, env, touch, auxv, execveat, statx (9 tests)
  src/ssh.rs              openssh-client: version, keygen, fingerprint, scp (4 tests)
  src/chroot.rs           fake-root chroot into a directory and a missing one (2 tests)
```

---

## Test Suites

| Suite | Tests | Probe | Verifies |
|-------|-------|-------|----------|
| distro | 8 | always | Package manager detection, tool installation, os-release |
| clone | 5 | always | T8.1 CLONE_VM/VFORK stripping (ptrace + SIGSYS paths) |
| readlink | 7 | `/bin/sh` exists | T8.2 Part B: .l2s. symlink hiding via readlink skip; fd links of hard-linked files |
| gcc | 3 | `/usr/bin/cc` or `/usr/bin/gcc` | T8.2 Part A: host_exe_before_l2s for /proc/self/exe |
| rust | 4 | `/usr/bin/rustc` exists | T8.3 si_syscall=-1 suppression, T8.4 SIGSYS handlers |
| git | 3 | `/usr/bin/git` exists | T8.3 si_syscall=-1, T8.4 setuid/setgid handlers |
| pipe | 3 | always | pipe2 availability (disproves blocked-by-seccomp theory) |
| general | 9 | always | Basic proot stability: file I/O, symlinks, pipes, signals, env; touch, auxv views, execveat, stat/statx owner |
| ssh | 4 | `/usr/bin/ssh` exists | openssh-client: version, ed25519 keygen, fingerprint, scp binary |
| chroot | 2 | always | fake_id0 chroot emulation (ptrace and SIGSYS paths) |

### Suite Details

#### distro (8 tests)

1. **detect PM** — checks for apk (Alpine) or apt (Debian)
2. **update repos** — `apk update` or `apt-get update`
3. **install tools** — `apk add vim gcc rust cargo git` (idempotent, skips if tools present)
4. **verify vim** — `vim --version | head -1` contains "VIM"
5. **verify gcc** — `cc --version | head -1` non-empty
6. **verify rustc** — `rustc --version` contains "rustc"
7. **verify cargo** — `cargo --version` contains "cargo"
8. **read os-release** — `/etc/os-release` contains NAME= and ID=

#### clone (5 tests)

1. **fork+exec baseline** — `echo ok` via `/bin/sh -c`
2. **stdout piped** — explicit `Stdio::piped()` capture
3. **nested spawn** — 3 levels of `sh -c 'sh -c "sh -c ..."'`
4. **CLONE_THREAD preserved** — `std::thread::spawn()` + exec (validates CLONE_THREAD guard)
5. **concurrent spawn stress** — 10 threads each spawning a process

#### readlink (7 tests)

1. **regular symlink resolves** — Rust `read_link()` on `/tmp` symlink
2. **realpath no .l2s.** — `realpath /usr/bin/cc` has no `.l2s.` path
3. **readlink EINVAL on .l2s.** — readlink on `.l2s.` symlink returns EINVAL
4. **/proc/self/exe no .l2s.** — `std::env::current_exe()` has no `.l2s.`
5. **lstat vs stat consistency** — both return valid results
6. **readlink small buffer** — compiles C program with `cc` that calls `readlink()` with 4-byte buffer
7. **readlink /proc/self/fd of a hard link** — the fd link of a file opened through a second
   hard link names the open file (same inode)

#### gcc (3 tests)

1. **cc -print-search-dirs** — install path has no `.l2s.`
2. **compile and run C program** — `printf("ok\n")` compiles and runs
3. **/proc/self/exe after exec** — no `.l2s.` in exe path after exec

#### rust (4 tests)

1. **rustc -vV** — version output (no subprocess needed)
2. **rustc compile .rs** — compile and run a Rust program
3. **cargo build --vcs none** — `cargo new --vcs none` + `cargo build`
4. **cargo build with git** — `cargo new` with default vcs (git) + `cargo build`

Each test uses `run_sh_timed()` which prints timing diagnostics to stderr:
`[rust] rustc compile start: rustc ...`, `[rust] rustc compile done in 0.5s exit=0`

#### git (3 tests)

1. **git init** — creates `.git/` with refs
2. **git config** — `git config --global user.name test`
3. **cargo new with vcs git** — `cargo new` with default git vcs

#### pipe (3 tests)

1. **pipe() baseline** — raw `libc::pipe()` syscall (used by GCC's cc driver)
2. **pipe2(O_CLOEXEC)** — used by rustc for subprocess stdout/stderr capture
3. **pipe2(O_NONBLOCK)** — non-blocking variant

#### general (9 tests)

1. **file I/O roundtrip** — create, write, read, rename, delete in `/tmp`
2. **symlink operations** — create, readlink, remove
3. **pipe between processes** — `echo foo | cat | wc -c` (3-process pipeline)
4. **signal propagation** — SIGINT trap/delivery
5. **environment inheritance** — `$HOME` through proot
6. **touch creates a missing file** — `utimensat()` on a missing file is ENOENT, and `touch`
   creates it
7. **auxv views match the stack** — every `/proc/self/auxv` and `PR_GET_AUXV` record equals
   `getauxval()` (EINVAL from `PR_GET_AUXV` before Linux 6.4 passes)
8. **execveat(AT_FDCWD) runs a program** — a forked child runs `/bin/true` through `execveat`
9. **stat and statx agree on owner** — uid and gid of a new file match between `stat()` and
   `statx()`

#### ssh (4 tests)

Probe: `/usr/bin/ssh` exists (installed by the apt second-pass in `install_tools()`).
Auto-skipped on Alpine (openssh-client not in apk add list) and on any distro where
openssh-client is absent.

1. **ssh -V (OpenSSH version)** — `ssh -V 2>&1` contains "OpenSSH"
2. **ssh-keygen ed25519 key generation** — `ssh-keygen -t ed25519 -N '' -f /tmp/pit-ssh-ed25519` exits 0
3. **ssh-keygen SHA256 fingerprint** — `ssh-keygen -l -f <pub>` outputs a `SHA256:` line
4. **scp available** — `/usr/bin/scp` exists and `scp 2>&1 || true` prints a usage line containing "scp"

Note: no network is required. All tests are local (key generation + binary invocation only).

#### chroot (2 tests)

Each test calls `chroot()` in a forked child, so the runner keeps its root.

1. **chroot into a directory** — `chroot("/tmp/pit-chroot")` succeeds and the marker
   file inside it is visible at `/`
2. **chroot into a missing directory is ENOENT** — `chroot("/tmp/pit-chroot-missing")`
   fails with `ENOENT`

---

## CLI Usage

```
pr-cli test <distro>              # Run all suites
pr-cli test <distro> -s <suite>   # Run single suite
pr-cli test <distro> -v           # Verbose (shows rootfs path)
```

Available suites: distro, clone, readlink, gcc, rust, git, pipe, general, ssh, chroot

---

## Android UI

BugReport icon button on each installed distro row (Play → BugReport → Delete).
Runs `pr-cli test <distro>` via `runDistroCommand`, output shown in the output panel
with ANSI escape code stripping. Sets `PROOT_TMP_DIR` + `TMPDIR` to `app.cacheDir`.

---

## Bugs Found During Evaluation

### Command::status() unreliable inside proot

`std::process::Command::status()` returns incorrect results when exec'ing distro
binaries (musl) from the bionic test binary inside proot. The workaround: use
`Command::output()` (which pipes stdout/stderr) or `Path::exists()` instead.

Affected code (all fixed):
- `distro.rs` `tool_exists()` — changed to `run_sh().is_ok()`
- `gcc.rs` `probe()` — changed to `Path::exists()`
- `rust.rs` `probe()` — changed to `Path::exists()`

### /tmp fallback not writable from run-as

`PROOT_TMP_DIR` / `TMPDIR` not set in `adb shell run-as` context. The fallback was
`/tmp` which isn't writable on Android. Fixed in `shared.rs` and `cmd_test.rs` to use
`{prefix}/tmp` as fallback.

### readlink error reporting

`test_readlink_small_buffer` compiled with `cc ... 2>&1` (stderr→stdout) but reported
`compile.stderr` (always empty). Fixed to `compile.stdout`.

---

## Build & Deploy

```bash
# 1. Build test binary
cd src/proot-integration-test && cargo build --target aarch64-linux-android --release

# 2. Build pr-cli (embeds test binary via include_bytes!)
cd src/pr-cli && cargo build --target aarch64-linux-android --release

# 3. Copy to jniLibs (always verify with md5sum)
cp -f src/pr-cli/target/aarch64-linux-android/release/pr-cli \
      android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so
md5sum src/pr-cli/target/aarch64-linux-android/release/pr-cli \
       android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so

# 4. Build & install APK
cd android && ./gradlew assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

**IMPORTANT:** pr-cli must be rebuilt after any change to the test binary (embedded at
build time). Always verify the copy with `md5sum` — Gradle caches jniLibs.
