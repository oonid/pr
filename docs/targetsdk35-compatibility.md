# targetSdk 35 Compatibility (Google Play Store Compatibility)

> Historical record (2026-04). The PROOT_LOADER mechanism it describes is still how pr runs at targetSdk 35; later changes: `cargo build` works since the T8 fixes (docs/rust-toolchain-support.md), and the native libraries live in `android/proot-engine/src/main/jniLibs`.

Date: 2026-04-16
Status: Complete
Device: Samsung SM-XXXXX (Galaxy X), Android 16 (SDK 36), aarch64
Trigger: Google Play requires targetSdk >= 35 as of August 2025

## Problem Statement

The app was built with targetSdk 28. Google Play enforces annual targetSdk minimums:
- August 2024: targetSdk 34
- August 2025: targetSdk 35 (current enforced minimum)

At targetSdk 29+, Android enforces W^X (Write XOR Execute) via SELinux on the
`untrusted_app` domain. This blocks `execve()` on files labeled `app_data_file`,
which includes the proot rootfs in `/data/data/<pkg>/files/`. Proot's built-in
loader (normally extracted to a temp dir) is also blocked.

Additionally, the zygote's seccomp BPF filter blocks certain syscalls. The filter
varies by Android system image version, not by targetSdk.

## Device Under Test

```
Device:       Samsung SM-XXXXX (Galaxy X)
Android:      16 (SDK 36)
OneUI:        8.0
SoC:          Snapdragon 8 Elite (ARM64)
SELinux:      Enforcing
ptrace_scope: 0 (no Yama restriction)
/data mount:  f2fs, no noexec flag
Knox:         Active
Root:         No
```

## Key SELinux Insight

```
/data/app/~~<random>/<pkg>-<random>/lib/arm64/   → apk_data_file:s0  → execve() ALLOWED
/data/data/<pkg>/files/rootfs/                   → app_data_file:s0  → execve() DENIED
```

Critical distinction discovered via testing:

| Context | SELinux Domain | W^X Enforced | Can execve app_data_file |
|---|---|---|---|
| `run-as` shell | `runas_app` | No | Yes (misleading!) |
| App process (UI) | `untrusted_app_29` | Yes | No |

`run-as` tests can pass while the deployed app fails. Always verify from the app UI.

**Do NOT use `memfd_create` as an alternative.** Samsung patches the kernel to label
memfd regions as non-executable (`ashmem` SELinux context). Android 14+ enforces
`MFD_NOEXEC_SEAL` by default. Dead end on Samsung hardware.

## Proot's Built-in Loader Mechanism

PRoot already has a first-class loader mechanism. No custom ELF loader needed.

Source: `src/proot/src/execve/enter.c:504-586`

PRoot reads the `PROOT_LOADER` env var. If set, it uses that binary as the execution
proxy instead of extracting its internal bundled loader to a temp file. The relevant
code path (line 584, in the `#else` branch — no `PROOT_UNBUNDLE_LOADER` define needed):

```c
loader_path = loader_path ?: getenv("PROOT_LOADER") ?: extract_loader(tracee, false);
```

Solution:
1. Build proot's own loader as a separate binary (`src/proot/src/loader/loader`)
2. Name it `libproot-loader.so`, place in `jniLibs/arm64-v8a/`
3. Set `PROOT_LOADER=<nativeLibDir>/libproot-loader.so` before launching proot

The loader is only 5.6KB. Android installs it to nativeLibraryDir (labeled
`apk_data_file:s0`) automatically from the APK.

## Implementation Details

### Fix 1: fchmodat SIGSYS Noop

File: `src/proot/src/tracee/seccomp.c`

At targetSdk 29+, the zygote's seccomp BPF filter blocks `fchmodat` (syscall 53 on
arm64). Proot's `temp.c` calls `chmod()` on temp directories, which triggers SIGSYS.

Added handler after the existing `PR_chmod` case:

```c
case PR_fchmodat:
    set_result_after_seccomp(tracee, 0);
    break;
```

Returns 0 (noop). The chmod on proot's temp dir is a safety measure, not critical —
safe to suppress since the loader now lives in nativeLibraryDir.

### Fix 2: Loader in nativeLibraryDir

Files changed:
- `src/pr-cli/src/shared.rs` — added `get_native_loader()`
- `src/pr-cli/src/login.rs` — set `PROOT_LOADER` env var before exec'ing proot
- `build.sh` — copy standalone loader to `build/out/<arch>/loader`
- `android/app/src/main/jniLibs/arm64-v8a/libproot-loader.so` — the loader binary
- `src/pr-cli/build-pr-cli.sh` — fixed `PROJECT_ROOT` path (was `src/`, now project root)

### Build Script Bug Fix

The pr-cli build script had `PROJECT_ROOT` resolving to `src/` instead of the project
root, causing the old binary (without PROOT_LOADER support) to be shipped. The fix:

```bash
# Before (wrong):
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"     # → src/

# After (correct):
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"   # → project root
```

## Seccomp Static Analysis (T7.4)

Analyzed the AOSP bionic seccomp policy from `vendor/bionic/` submodule:

- `SECCOMP_BLOCKLIST_APP.TXT` — blocked for apps
- `SECCOMP_ALLOWLIST_APP.TXT` — extra allowed for apps
- `SECCOMP_BLOCKLIST_COMMON.TXT` — blocked for all
- `SECCOMP_ALLOWLIST_COMMON.TXT` — extra allowed for all

Formula: `allowed = SYSCALLS.TXT - BLOCKLIST + ALLOWLIST` (per architecture)

### Key Findings

1. **Proot's own seccomp filter is dead code.** `enable_syscall_filtering()` in
   `src/proot/src/syscall/seccomp.c` is defined but never called. The zygote's BPF
   filter is the only one active. `PROOT_NO_SECCOMP=1` is set but irrelevant.

2. **The bionic seccomp policy does NOT change per targetSdk.** It's compiled into
   the Android system image. Observed behavioral differences between SDK 28 and 29
   were from different proot code paths, not different seccomp filters.

3. **Blocked syscalls for arm64 (lp64):**
   - UID/GID: setuid, setgid, setreuid, setregid, setresgid, setfsgid, setfsuid, setgroups
   - FS namespace: mount, umount2, chroot
   - Time: adjtimex, clock_settime, clock_adjtime, settimeofday
   - System: acct, syslog, init_module, delete_module, reboot, swapon, swapoff, sethostname, setdomainname

4. **All blocked syscalls are handled gracefully.** Proot's default SIGSYS handler
   returns `-ENOSYS` for unknown blocked syscalls. Only specific syscalls need special
   handlers (fchmodat, chdir, fchdir, getcwd, linkat) — all already implemented.

Bionic version: `ndk-r29-321-g731631f30` (AOSP main, 2025-03-26).

## Test Results

### targetSdk 29 (baseline)

- Terminal opens ✅
- `apk --version` ✅
- No SIGSYS events in log

### targetSdk 35 (Play Store minimum)

- Terminal opens ✅
- `apk --version` ✅
- No SELinux denials, no SIGSYS events
- No behavioral difference from targetSdk 29

### targetSdk 36 (device OS, future-proofing)

- Terminal opens ✅
- `apk --version` ✅
- `vim --version` ✅
- No behavioral difference from targetSdk 35

### Full Regression (T7.6) at targetSdk 35

| Test | Result |
|---|---|
| `apk update` | ✅ |
| `apk add openssh` + `ssh -V` | ✅ |
| `apk add gcc` + `gcc --version` | ✅ |
| `gcc` compile + run hello world | ✅ |
| `cargo build` | ❌ pre-existing proot limitation |
| SIGSYS log | empty (no unexpected events) |

`cargo build` fails with ENOSYS when `rustc` tries to execute as a subprocess.
`cargo -V` and `rustc -V` both work (version print only). gcc compilation works fine.
This is a pre-existing proot limitation, not a targetSdk regression. Tracked as T5.7.

## Commits

| Commit | Description |
|---|---|
| `0373929` | Implement targetSdk 29 support: SELinux W^X bypass and fchmodat seccomp fix |
| `474ceff` | Mark T7.4 complete: static seccomp policy analysis |
| `fadaf10` | Mark T7.5/T7.5b complete: targetSdk 35 and 36 verified |
| `66a208b` | Mark T7.6 complete: full regression passes at targetSdk 35 |

## Architecture

```
App.apk
├── jniLibs/arm64-v8a/
│   ├── libproot.so            ← proot binary (static, ~2.6MB)
│   ├── libproot-loader.so     ← proot's loader (separate, ~18KB)
│   └── libpr-cli.so           ← Rust CLI (static, ~3.4MB)
```

Flow at targetSdk 35:
1. App forks PTY, execs `pr-cli login alpine`
2. pr-cli sets `PROOT_LOADER=<nativeLibDir>/libproot-loader.so`
3. pr-cli execs `proot --rootfs=... /bin/sh -l`
4. Proot uses loader from nativeLibDir (`apk_data_file`, execve allowed)
5. Proot traces child via ptrace, handles SIGSYS for blocked syscalls
6. Child runs inside proot namespace as fake root
