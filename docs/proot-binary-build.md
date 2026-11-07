# Proot Binary Build

> Historical record of Phase 1. The build script is `scripts/build.sh` (see docs/scripts.md), and its output is copied to `android/proot-engine/src/main/jniLibs` by hand.

Phase 1 produces a statically-linked proot binary for Android arm64, built from the termux-proot fork with additional patches for NDK cross-compilation and Android Bionic compatibility.

Status: **Complete** (commits `91537c8..a8b8c85`)

---

## Architecture

```
vendor/proot/              # upstream proot v5.0.0-291-g5f780cb (pristine, read-only)
vendor/termux-proot/       # termux fork with Android patches (pristine, read-only)
vendor/samba/              # samba source containing lib/talloc (pristine, read-only)
src/proot/                 # working copy: termux-proot + our patches (build target)
src/proot/lib/talloc/      # our stub header for cross-compiling talloc
build.sh                   # NDK cross-compilation build script
build/out/arm64/proot      # output binary (2.5MB, static, arm64)
```

All patching happens in `src/proot/`. Vendor directories are never modified.

---

## What Was Done

### T1.1 — New files from termux-proot

Copied ~20 new files that exist in termux-proot but not in upstream proot:

- Extensions: `ashmem_memfd/`, `link2symlink/`, `fix_symlink_size/`, `sysvipc/`, `hidden_files/`, `mountinfo/`, `port_switch/`
- Tracee: `seccomp.c/h`, `statx.c/h`
- Path: `f2fs-bug.c/h`
- Loader: `loader-info.awk` (rewritten for POSIX awk — see patches below)

### T1.2 — Merged modified files

Copied 46 files where termux-proot diverged from upstream. Initially 8 files were planned, but a full diff revealed **47 files differ**. The 38 remaining files were synced during T1.4 build verification. Key areas:

| Area | Files | What changed |
|---|---|---|
| CLI | `cli.c`, `proot.c`, `proot.h` | New options (link2symlink, ashmem-memfd, sysvipc, -L, -H, -p) |
| Execve | `enter.c`, `exit.c`, `ldso.c` | Android linker paths, improved ELF parsing |
| Syscall | `enter.c`, `exit.c`, `chain.c`, `syscall.c` | execveat, extended syscall translation |
| Syscall tables | `sysnums-*.h` (6 files), `sysnums.list` | Newer syscalls (bpf, execveat, getrandom, memfd_create, statx, copy_file_range) |
| Tracee | `event.c`, `mem.c`, `reg.c`, `tracee.c` | Seccomp-aware event loop, kill-on-exit, POKEDATA workaround |
| Path | `canon.c`, `path.c`, `proc.c`, `temp.c` | Path translation fixes |
| Ptrace | `ptrace.c`, `wait.c`, `wait.h` | Improved waitpid, seccomp support |
| Extension | `kompat.c`, `extension.c` | Simplified compat, new events |

### T1.3 — GNUmakefile and fake_id0

- Copied termux GNUmakefile with `?=` for CC/STRIP/OBJCOPY/OBJDUMP (cross-compilation support)
- 18 split fake_id0 `.c` files + 17 `.h` files from termux
- 86 object files total in build

### T1.4 — Build system

**build.sh** (427 lines): NDK cross-compilation script that:

1. Downloads NDK r27c if not present
2. Sets up sysroot per architecture from NDK headers/libs
3. Compiles `vendor/samba/lib/talloc/talloc.c` with stub `replace.h` into `libtalloc.a`
4. Builds proot via GNUmakefile with NDK clang
5. Patches TLS alignment (see T1.5)
6. Verifies output binary

**talloc integration**: `vendor/samba/lib/talloc/` is compiled directly. Samba's `waf` build system is incompatible with cross-compilation, so `src/proot/lib/talloc/replace.h` provides minimal stubs (`uint8_t`, `bool`, `talloc_get_type_abort` fallback, dummy `_PUBLIC_` macro).

**Build output** (arm64):
```
build/out/arm64/proot: ELF 64-bit LSB executable, ARM aarch64, version 1 (SYSV),
statically linked, with debug_info, not stripped — 2.5MB
```

### T1.5 — Device testing and TLS fix

**TLS alignment bug**: Android Bionic on ARM64 requires PT_TLS segment alignment >= 64 bytes. The NDK static linker produces only 8-byte alignment. Bionic refuses execution:

```
error: executable's TLS segment is underaligned: alignment is 8 (skew 0),
needs to be at least 64 for ARM64 Bionic
```

Fix: `fix_tls_alignment()` in build.sh uses Python to patch the `p_align` field of the PT_TLS program header from 8 to 64 post-link. This is necessary because the NDK linker doesn't propagate TLS variable alignment to the PT_TLS header in static builds.

`src/proot/src/tls-align.c` contains a dummy `__thread` variable included in the link to ensure a TLS segment exists, but the actual alignment fix is the post-build patch.

---

## Patches Applied on Top of termux-proot

These are changes we made beyond what termux-proot provides:

| File | Change | Reason |
|---|---|---|
| `src/proot/src/cli/proot.h` | `VERSION` set to `"5.4.0-pr"` | Project identity |
| `src/proot/src/extension/ashmem_memfd/ashmem_memfd.c` | Added `#include <string.h>` | NDK clang requires explicit include for memset/memcpy |
| `src/proot/src/loader/loader-info.awk` | Rewritten: replaced GAWK `strtonum()` with portable hex conversion | NDK build env may not have gawk; Termux provides it but we can't assume it |
| `src/proot/src/tls-align.c` | New file: dummy TLS variable with 64-byte alignment | Ensures TLS segment exists for post-build alignment patch |
| `src/proot/lib/talloc/replace.h` | New file: standalone stub header | Replaces samba's waf-generated replace.h for cross-compilation |

---

## Build Warnings (Known, Non-blocking)

The build produces warnings from termux-proot upstream code. No errors:

- `deprecated declaration of 'talloc_autofree_context'` — talloc API deprecation
- `the use of 'mktemp' is dangerous` — termux-proot uses mktemp in temp.c
- Sign comparison warnings in several files
- Unused variable warnings in several files
- Reserved register clobber in loader assembly (ARM specific)

These are cosmetic only and do not affect functionality.

---

## Device Test Results

**Test device**: Unrooted Samsung, Android 16 (SDK 36), aarch64, SELinux context `u:r:shell:s0`

| Test | Command | Result |
|---|---|---|
| Version | `proot --version` | `proot v5.4.0-pr` |
| Fake root | `proot -0 ... /bin/busybox sh -c "id"` | `uid=0(root) gid=0(root)` |
| Alpine rootfs | `proot -0 -r alpine ... /bin/busybox ls /` | Shows Alpine dirs (bin, etc, lib, usr...) |
| OS release | `proot -0 -r alpine ... /bin/busybox cat /etc/os-release` | `Alpine Linux v3.21` |
| link2symlink | `ln orig link; cat link` | Hard link emulated via symlink, content readable |
| --kill-on-exit | `sleep 60 & echo DONE` | Background process killed, exits cleanly |
| Interactive shell | `proot ... /bin/busybox sh -l` | Works; PATH must be set externally |

### Critical finding: PROOT_NO_SECCOMP=1 required

On this device, proot **fails** without `PROOT_NO_SECCOMP=1`:

```
proot error: execve("/system/bin/sh"): Operation not permitted
proot info: It seems your kernel contains this bug...
To workaround it, set the env. variable PROOT_NO_SECCOMP to 1.
```

With `PROOT_NO_SECCOMP=1`, everything works. The APK launcher **must** set this environment variable by default. This disables proot's built-in seccomp filter (which accelerates syscall dispatch via BPF) and falls back to ptrace-only interception. Performance is slightly reduced but correctness is maintained.

This is consistent with Termux behavior — Termux's proot-distro also sets `PROOT_NO_SECCOMP=1` on many devices.

---

## Carry-Forward to Phase 2

The following findings from Phase 1 directly affect Phase 2 (proot-distro.sh porting) and beyond:

### 1. Environment variables the launcher must set

```
PROOT_NO_SECCOMP=1    # Required on Android 14+ / SDK 34+ devices
PATH=/bin:/usr/bin:/sbin:/usr/sbin:${APP_PREFIX}/bin  # Alpine minirootfs has no /usr/bin by default
HOME=/root            # Or ${APP_HOME}
```

The proot-distro login function (T2.x) must inject these before launching proot.

### 2. Default proot flags

The working command pattern is:

```
proot --link2symlink --root-id \
    -r <rootfs_path> \
    -b /dev -b /proc -b /sys \
    -w /root \
    /bin/sh -l
```

- `--link2symlink`: Required for SELinux environments that block hard links
- `--root-id` (equivalent to `-0`): Fake root for package managers
- `-b /dev -b /proc -b /sys`: Bind Android's virtual filesystems
- `-w /root`: Start in home directory
- `/bin/sh -l`: Login shell (busybox sh in Alpine; bash in Debian/Ubuntu)

The proot-distro `command_login` function constructs this command line. Phase 2 must replicate this pattern.

### 3. Busybox is required for PATH

Alpine minirootfs ships busybox as `/bin/busybox` with symlinks in `/bin/`. But in the adb shell test, PATH was inherited from Android and commands failed. The proot-distro launcher must:

- Set `PATH=/bin:/usr/bin:/sbin:/usr/sbin` inside proot
- Use `/bin/busybox sh` as the shell (not just `/bin/sh` which may not have proper applet resolution without correct PATH)

### 4. Rootfs location

The test used `/data/local/tmp/alpine`. The APK will use `${APP_DATA}/usr/proot-distro/installed-rootfs/<distro>/`. Phase 2 must ensure this path structure.

### 5. The loader binary

proot bundles its own ELF loader. The verbose output showed:

```
loader: /tmp/prooted-6603-lQuTWg
```

proot extracts its loader to TMPDIR. If TMPDIR points to a noexec mount, this will fail. The launcher should set `TMPDIR=${APP_DATA}/usr/tmp` to a writable, executable location.

### 6. Multi-architecture

Currently only arm64 is tested. The build.sh supports arm as well (`--arch=arm`). Phase 2/4 must handle runtime arch detection and select the correct proot binary.

---

## File Inventory

### New files created (not from termux-proot):

```
build.sh                              # Build script
src/proot/lib/talloc/replace.h        # Talloc stub header
src/proot/src/tls-align.c             # TLS alignment dummy
```

### Modified from termux-proot originals:

```
src/proot/src/cli/proot.h                         # VERSION = "5.4.0-pr"
src/proot/src/extension/ashmem_memfd/ashmem_memfd.c  # Added #include <string.h>
src/proot/src/loader/loader-info.awk              # Rewritten for POSIX awk
```

### Total source tree (src/proot/src/):

- ~120 `.c` files, ~90 `.h` files
- 6 architecture-specific syscall tables (`sysnums-*.h`)
- GNUmakefile (276 lines)
- Output: 87 `.o` files linked into one static `proot` binary

---

## Reproducing the Build

```bash
# Prerequisites: make, python3, readelf, file, curl, unzip, ~2GB disk for NDK

scripts/build.sh --arch=arm64

# Output: build/out/arm64/proot (2.5MB)
# TLS alignment is automatically patched to 64 bytes

# To test on device:
adb push build/out/arm64/proot /data/local/tmp/proot
adb shell chmod 755 /data/local/tmp/proot
adb shell PROOT_NO_SECCOMP=1 /data/local/tmp/proot --version
```
