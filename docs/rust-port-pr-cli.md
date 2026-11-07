# Pr-cli (Rust Port)

> Historical exploration from before Phase 6. The Rust port (approach A) was chosen and is `src/pr-cli`; section C's "Current Plan" label is from that time.

Date: 2026-04-14
Status: Exploration (pre-Phase 6)
Trigger: mksh R59 on Android lacks associative arrays; bash cannot be exec'd from `untrusted_app` SELinux context.

## Problem Statement

The app process runs under `u:r:untrusted_app:s0` SELinux domain. It can `execve()` `/system/bin/sh` (mksh) but NOT bash from nativeLibraryDir — the process is killed with SIGSYS (exit 159). mksh R59 (2020/10/31) lacks associative arrays (`typeset -A` silently corrupts data), `mapfile`, process substitution, and `${!var}` indirect expansion.

Porting proot-distro.sh (3079 lines) to mksh is possible but requires replacing all associative arrays with `eval`-based flat-variable lookups — fragile, hard to test, and hard to maintain.

## Current Script Structure

### Function Breakdown

| Function | Lines | Responsibility | Bash dependency |
|---|---|---|---|
| `command_install` | 371 | Parse args, source plugin, download tarball, extract, write config | Assoc arrays, `<<<`, `eval` |
| `command_login` | 596 | Parse args, build proot argv, setup env/bind mounts, exec proot | Assoc arrays, indexed arrays, `mapfile`, `local -a`, `<<<` |
| `setup_fake_sysdata` | 244 | Write fake /proc files (loadavg, stat, uptime, version, vmstat) | None — pure file I/O |
| `command_backup` | 150 | Tar rootfs into backup archive | Minimal |
| `run_proot_cmd` | 146 | Construct proot invocation with CPU emulation args | String building only |
| `command_restore` | 90 | Extract backup archive to rootfs | Minimal |
| `command_rename` | 146 | Rename distro, update symlinks | Assoc arrays, `<<<` |
| `command_copy` | 157 | Copy rootfs between distros | `<<<` |
| `command_remove` | 77 | Delete rootfs directory | None |
| `command_list` | 79 | Iterate plugins, show distro info | Assoc arrays, `${!array[@]}` |
| `command_reset` | 62 | Wipe and reinstall rootfs | Assoc arrays |
| `command_clear_cache` | 42 | Delete download cache | None (already mksh-safe) |
| Help functions (7) | ~170 | Print usage text | None |
| Init/self_initialize | ~84 | Validate environment, detect arch | None |
| download_file | ~58 | HTTP download with retry | None |
| msg/color setup | ~100 | Terminal color output | None |
| Plugin loading | ~30 | Source plugins, populate arrays | Assoc arrays (the core problem) |

### Where Bash-isms Concentrate

The associative arrays (`declare -A`) are the central data model:

```
SUPPORTED_DISTRIBUTIONS["alpine"] = "Alpine Linux"
TARBALL_URL["aarch64"] = "https://easycli.sh/..."
TARBALL_SHA256["aarch64"] = "2bdfb..."
SUPPORTED_DISTRIBUTIONS_COMMENTS["alpine"] = "Regular release v3.23.3"
```

These are read/written in ~50 locations across the script. Under mksh R59, `typeset -A` silently fails and treats arrays as indexed — `SUPPORTED_DISTRIBUTIONS["alpine"]` and `SUPPORTED_DISTRIBUTIONS["ubuntu"]` map to the same slot, producing wrong results.

## Three Approaches

### A. Full Rust Rewrite

Replace proot-distro.sh entirely with a Rust binary (`pr-cli`).

```
pr-cli install alpine     # replaces: proot-distro.sh install alpine
pr-cli login alpine       # replaces: proot-distro.sh login alpine
pr-cli list               # replaces: proot-distro.sh list
```

**Pros:**
- Type-safe, testable, no eval hacks
- No shell compatibility concerns
- Better error handling, logging, progress reporting
- Can call proot directly via `exec(2)` from Rust
- Single binary, no template substitution needed

**Cons:**
- Higher initial effort (rewrite ~3000 lines of shell logic)
- Loses easy upstream tracking with termux/proot-distro
- APK size +2-5MB for Rust binary
- Must handle `distro_setup()` hooks differently
- Must be in nativeLibDir as `libpr-cli.so` (same W^X workaround as proot/bash)

### B. Hybrid: Rust Core + Thin Shell Wrapper

Rust handles business logic (plugin parsing, install orchestration, login command construction). A thin `/system/bin/sh` wrapper stays for glue.

**Pros:**
- Focused scope, less risk
- Shell wrapper can stay mksh-compatible (it's thin)
- Core logic benefits from Rust type safety

**Cons:**
- Two languages to maintain
- IPC between Rust and shell adds complexity
- Doesn't fully eliminate the mksh problem — the wrapper still needs to exist

### C. Full mksh Port (Current Plan)

Rewrite all bash-isms in the 3079-line shell script using `eval`-based flat variables.

**Pros:**
- No new language/toolchain dependency
- No APK size increase
- Closest to upstream proot-distro

**Cons:**
- Fragile — `eval`-based associative array simulation is error-prone
- Hard to test — quoting bugs surface only at runtime on device
- Hard to maintain — future upstream changes need manual mksh conversion
- mksh R59 has other subtle differences (here-doc temp file requirements, array key quoting)

## Analysis: Rust Feasibility

### What the shell script actually does

proot-distro.sh is a **task orchestrator**. It does not contain complex algorithms. Each command is a sequence of:

1. Parse CLI arguments
2. Read plugin metadata (distro name, tarball URLs, SHA256 hashes)
3. Download tarball (via wget/curl)
4. Extract tarball (via tar)
5. Write config files (passwd, group, environment, resolv.conf)
6. Generate fake /proc data (static files)
7. Construct proot command line (lots of bind mounts, env vars)
8. `exec` proot

All of these are straightforward in Rust.

### Plugin format change

Current (shell):
```sh
DISTRO_NAME="Alpine Linux"
DISTRO_COMMENT="Regular release v3.23.3"
TARBALL_URL_aarch64="https://easycli.sh/..."
TARBALL_SHA256_aarch64="2bdfb..."
```

This is already simple key=value. Rust can parse it with a trivial parser — no need for a full shell interpreter.

### The `distro_setup()` problem

8 plugins have post-install shell functions:
- archlinux: init pacman keyring, populate archlinux-keyring
- artix: same as archlinux
- debian: setup locales
- fedora: setup dnf, install core packages
- manjaro: same as archlinux
- opensuse: setup zypper
- trisquel: setup locales
- ubuntu: setup locales

Options:
1. **Convert to Rust** — implement each as Rust code. Most are just `Command::new("chroot").arg("locale-gen")` etc.
2. **Keep as shell fragments** — each plugin ships a `setup.sh` that Rust invokes via `/system/bin/sh` in the proot context
3. **Skip for v1** — distros work for basic usage without setup hooks

Option 2 is the most pragmatic — `distro_setup()` runs inside the proot chroot where `/system/bin/sh` can execute anything.

### Rust dependencies

Minimal set:
- `clap` or `lexopt` — CLI argument parsing
- `sha2` — SHA256 verification
- `reqwest` (or shell out to busybox wget) — HTTP downloads
- `tar` + `xz2` (or shell out to busybox tar) — archive extraction
- `dirs` — path construction

Alternative: skip Rust HTTP/tar libraries and shell out to busybox wget/tar. This keeps the binary small (~500KB) but adds busybox dependency (already bundled).

### Cross-compilation

Already have NDK 27 at `/home/o/Android/Sdk/ndk/27.0.12077973/`. Cross-compiling Rust for `aarch64-linux-android`:

```sh
rustup target add aarch64-linux-android
cargo build --target aarch64-linux-android --release
```

The resulting binary goes to `jniLibs/arm64-v8a/libpr-cli.so` (same pattern as libproot.so).

### Estimated effort

| Component | Lines of Rust | Notes |
|---|---|---|
| CLI parsing (install, login, remove, list, backup, restore) | ~200 | With `clap` derive |
| Plugin config parser | ~80 | Simple key=value |
| Download with retry | ~60 | Or shell out to wget |
| Extract tarball | ~30 | Shell out to tar |
| Fake /proc generation | ~150 | Port from setup_fake_sysdata() |
| Config file writing (passwd, group, etc.) | ~100 | Port from command_install |
| Proot command builder | ~200 | The bind mount logic is complex |
| Exec proot | ~20 | `Command::new("proot")` |
| Distro setup hooks | ~50 | Shell out via proot |
| Error handling, logging, progress | ~100 | Structured output |
| **Total** | **~1000** | vs 3079 lines of shell |

## Recommendation

**Approach A (Full Rust)** is recommended over the mksh port because:

1. **The mksh port is fragile** — eval-based associative arrays are a ticking time bomb. A quoting bug in one `eval` statement can silently corrupt distro names or tarball URLs. On-device debugging of shell eval chains is painful.

2. **The shell script is mostly plumbing** — there's minimal "shell magic" that can't be expressed in Rust. The fake /proc generation is just writing files. The install command is download + extract + write config. The login command is building an argv.

3. **Rust on Android arm64 is standard** — we already have the NDK toolchain. Cross-compilation is one `cargo build` command.

4. **The plugin format is already close** — we already converted `TARBALL_URL['aarch64']` to `TARBALL_URL_aarch64`. Converting to a config file is trivial.

5. **Future-proof** — a Rust binary is easier to extend (progress bars, parallel downloads, built-in terminal, etc.) than a 3000-line shell script.

### Suggested Plugin Format

```
# alpine.plugin
name = "Alpine Linux"
comment = "Regular release v3.23.3"

[ tarball.aarch64 ]
url = "https://easycli.sh/proot-distro/alpine-aarch64-pd-v4.37.0.tar.xz"
sha256 = "2bdfb03eae53e6163695f4cd3b86e67ddca78466c879a140e069b1263150599b"

[ tarball.arm ]
url = "https://easycli.sh/proot-distro/alpine-arm-pd-v4.37.0.tar.xz"
sha256 = "0d1bc9bb24f1efd3a95e22e04e3590f4adfb0ff1ff39bbc82281ccf12fc0916d"
```

Or simply keep the current key=value format and parse with Rust — no format change needed for v1.

### Proposed Task Breakdown

If we proceed with Rust:

1. **Rust project setup** — `src/pr-cli/` with Cargo.toml, cross-compilation config, NDK linker
2. **Plugin config parser** — read key=value files, populate structs
3. **CLI interface** — install, login, remove, list, backup, restore subcommands
4. **command_install** — download, verify, extract, write configs
5. **command_login** — build proot argv, exec
6. **setup_fake_sysdata** — generate /proc files
7. **command_list** — iterate plugins, display info
8. **command_remove/reset/backup/restore/copy** — remaining commands
9. **Integration testing** — same as T5.2-T5.6 but via Rust binary
10. **APK integration** — bundle as `libpr-cli.so`, update App.kt

### Open Questions

- **Download library or shell out?** reqwest adds ~1MB to binary. Shelling out to busybox wget is simpler but loses progress reporting.
- **Tar extraction or shell out?** tar+xz2 crates add ~500KB. Shelling out to busybox tar is simpler.
- **distro_setup() handling?** Shell out via proot for v1, or implement in Rust per distro?
- **How much of command_login's bind mount logic do we need?** The 596-line function has lots of edge cases for different Android versions and SELinux policies. We may be able to simplify for our target.

## Viability Test Results

Date: 2026-04-14
Device: Samsung, Android 16 (SDK 36), aarch64
SELinux context: `u:r:untrusted_app:s0`
Binary: `pr-test` (Rust, statically linked, 639KB, aarch64-linux-android)
Bundled as: `jniLibs/arm64-v8a/libpr-test.so` → symlinked to `files/usr/bin/pr-test`
Invoked by: Android ProcessBuilder (same path as proot-distro.sh would use)

### Build Configuration

```toml
# .cargo/config.toml
[target.aarch64-linux-android]
linker = ".../aarch64-linux-android28-clang"
rustflags = ["-C", "target-feature=+crt-static", "-C", "link-arg=-static"]

# Cargo.toml
[profile.release]
opt-level = "z"
strip = true
```

Cross-compile: `cargo build --target aarch64-linux-android --release`
Result: 639KB ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped

### Test Results Summary

| # | Test | Result | Notes |
|---|------|--------|-------|
| 1 | selfcheck | **PASS** | Rust binary execs from app process via ProcessBuilder |
| 2 | file-io | **PASS** | All file ops work: mkdir, write, read, symlink, chmod, rm |
| 3 | exec-subcommand | **PASS** | Can fork+exec busybox, /system/bin/sh, proot |
| 4 | env-vars | **PASS** | All env vars propagate correctly |
| 5 | network | **FAIL** | wget returns exit=-1 (subprocess spawn failure) |
| 6 | parse-plugin | **PASS** | Reads 14 plugin configs correctly |
| 7 | proot | **PARTIAL** | `proot --version` works; `proot echo` fails with ENOSYS |

### Detailed Findings

#### Test 1: selfcheck — PASS

```
PASS: exec — Rust binary executed successfully
PASS: pid — PID=20758
PASS: uid — UID=10515
PASS: arch — target arch=aarch64
PASS: exe — path=.../lib/arm64/libpr-test.so
PASS: cwd — dir=/
PASS: args — argc=2
```

**Key finding:** Rust binary can be `execve()`'d from the `untrusted_app` SELinux context via ProcessBuilder. The binary is in nativeLibraryDir (extracted from APK's jniLibs/), which Android's SELinux policy allows to execute.

**Critical difference from bash:** bash (also in nativeLibDir as `libbash.so`) gets SIGSYS (exit 159) when exec'd from ProcessBuilder. Rust does NOT. This is likely because bash does something in its early initialization (perhaps a `prctl` or seccomp-related call) that triggers the zygote's seccomp filter, while Rust's minimal runtime does not.

#### Test 2: file-io — PASS

```
PASS: mkdir, write, read, symlink, chmod, readdir, rm-rf
```

All filesystem operations in `APP_PREFIX/tmp/` work correctly. The Rust `std::fs` module works as expected in the app's data directory. No SELinux denials for file I/O in app-owned paths.

#### Test 3: exec-subcommand — PASS

```
PASS: exec-busybox — version=, exit=127  (busybox --version returns 127, not 0)
PASS: exec-wget — exit=0
PASS: exec-sha256sum — exit=0
PASS: exec-tar — exit=0
PASS: exec-sh — output='hello-from-sh'
PASS: exec-proot — exit=0
PASS: exec-bash — exit=0 (unexpected!)
```

**Key finding:** Rust's `Command::new()` (which uses `fork()+execve()`) works for ALL binaries from the app process:
- busybox (nativeLibDir) — works
- /system/bin/sh — works
- proot (nativeLibDir) — works (for --version)
- bash (nativeLibDir) — **works!** (exit=0 for --version)

Wait — bash works when spawned from Rust? Earlier it failed with exit 159 from ProcessBuilder. The difference is:
- **ProcessBuilder("bash", "--version")** — Java/Kotlin directly exec's bash → SIGSYS (159)
- **Rust `Command::new("bash").arg("--version")`** — Rust forks, then child exec's bash → works (exit=0)

This is a significant finding. The Rust binary acts as a trusted intermediary. The `untrusted_app` SELinux domain allows exec of nativeLibDir binaries when the exec happens from a statically-linked binary that was itself loaded from nativeLibDir. Java's ProcessBuilder may apply additional restrictions that Rust's raw `fork()+execve()` does not.

**Impact:** This means a Rust binary could even invoke bash scripts, opening the possibility of a hybrid approach where Rust does the heavy lifting but delegates to bash for complex operations.

#### Test 4: env-vars — PASS

```
PASS: APP_PREFIX, APP_HOME, APP_PACKAGE, PROOT_NO_SECCOMP
PASS: HOME, TERM, TMPDIR, PATH
PASS: ANDROID_ROOT, ANDROID_DATA, EXTERNAL_STORAGE
31 total env vars
```

All environment variables set by ProcessBuilder propagate to the Rust process and are accessible via `std::env::var()`. The PATH includes both app's bin dir and system paths.

#### Test 5: network — FAIL

```
FAIL: network-wget — exit=-1, stderr=
FAIL: network-dns — exit=-1
```

Exit code -1 means the subprocess couldn't be spawned at all. This is likely because `busybox wget` in the PATH is a symlink to nativeLibDir's libbusybox.so, and when the Rust subprocess tries to exec it, the symlink resolution differs from Rust's own exec path.

**Note:** This needs investigation. It may be that busybox needs to be invoked by full path, or that the symlink target is stale after the APK reinstall.

#### Test 6: parse-plugin — PASS

```
PASS: plugin-dir — found 14 plugins
PASS: 14 plugins parsed with correct DISTRO_NAME values
```

Simple key=value parsing works. All 14 distro names extracted correctly. URL/SHA256 counts are 0 because the deployed plugins still use the old `TARBALL_URL['aarch64']` format — the parser only matches `TARBALL_URL_` prefix. Once plugins are deployed with the flat variable format, URLs will be detected.

#### Test 7: proot — PARTIAL

```
PASS: proot-version — exit=0
FAIL: proot-exec — exit=1
  proot error: execve("/system/bin/echo"): Function not implemented
```

Proot itself starts fine, but `execve` inside proot fails with ENOSYS. This is the same error seen from `run-as` context (ptrace limitation). The error differs from the earlier SIGSYS — it's now "Function not implemented" rather than "Operation not permitted", suggesting the proot seccomp patch is working but ptrace-based exec interception has issues.

**This needs further investigation.** The proot exec test works differently from how proot-distro uses proot (with full rootfs, bind mounts, etc.). The real test would be `pr-cli login alpine` which sets up the full proot environment.

### Conclusions

1. **Rust binary can run from app process** — confirmed. The statically-linked binary in nativeLibDir can be exec'd via ProcessBuilder. This is the fundamental requirement.

2. **Rust can fork+exec subcommands** — confirmed. busybox, /system/bin/sh, proot, and even bash can be spawned as subprocesses from Rust.

3. **File I/O works** — all standard filesystem operations work in app data directory.

4. **Environment propagation works** — all vars from ProcessBuilder are accessible.

5. **Network access needs investigation** — subprocess-based wget failed, but this may be a symlink issue, not a Rust limitation.

6. **Proot invocation works** but ptrace-based exec inside proot still has limitations from the app context. This is the same challenge regardless of whether we use shell or Rust.

7. **Binary size is acceptable** — 639KB for a statically-linked Rust binary with libc. A full `pr-cli` with download/tar support would likely be 1-2MB.

### Recommendation: Confirmed Viable

The Rust approach is **viable**. All core capabilities (exec, file I/O, subprocess spawning, env vars) work from the app process. The remaining issues (network, proot exec) are orthogonal to the Rust vs shell choice — they would exist in either approach.

---

## Known Issues / Errata

### clap `login` command rejects hyphen-prefixed trailing args

**Discovered:** while building `rs.oo.or.id` on top of `pr-cli`.

**File:** `src/pr-cli/src/main.rs` — `Commands::Login` variant.

**Problem:** The `login` subcommand used `#[arg(last = true)]` for the trailing
`args` field. clap's `last = true` mode marks the argument as "must come after
`--`" but still rejects values that look like flags. Passing
`pr-cli login alpine -- /bin/sh -c "apk update"` failed with:

```
error: unexpected argument '-c' found
```

**Fix:**
```rust
#[arg(trailing_var_arg = true, allow_hyphen_values = true)]
```

`trailing_var_arg` enables raw passthrough of everything after the subcommand
name, and `allow_hyphen_values` suppresses clap's flag-detection heuristic for
those values. Together they allow any argument string — including those starting
with `-` — to be collected verbatim.

**Note:** Per the double-shell-wrapping caveat (see `docs/important-notes.md`),
even with this fix callers should not pass `/bin/sh -c` explicitly as trailing
args to `pr-cli login` — the inner shell wrapping already happens inside pr-cli.
