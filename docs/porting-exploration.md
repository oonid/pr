# Porting proot-distro to Standalone Android — Exploration Report

> Historical exploration report written before the port; the code and the current docs (docs/README.md) are authoritative.

## 1. Overview

This document captures the findings from exploring the three vendor projects
(`vendor/proot`, `vendor/termux-proot`, `vendor/termux-proot-distro`) and
defines a strategy for porting `proot-distro` into a **standalone Android
application package** that does **not depend on Termux**.

### 1.1 Goals

- Bundle a working `proot` binary + `proot-distro` manager inside an Android
  APK.
- Provide runtime utilities (bash, curl, tar, etc.) via a **static busybox**
  binary bundled in the app.
- Use the **vanilla proot** source from `vendor/proot/` as the base, with
  **cherry-picked patches** from `vendor/termux-proot/` applied on top.
- Use `vendor/termux-proot-distro/` as **reference only** — its 3172-line
  `proot-distro.sh` will be forked and stripped of all Termux-specific code.

### 1.2 Decisions Made

| Decision | Choice | Rationale |
|---|---|---|
| Deployment target | Android APK | Self-contained, no root, distributable via app stores or sideload |
| proot source base | Vanilla + cherry-picks | Clean upstream tracking; only apply patches we actually need |
| Runtime utilities | Static busybox | Single binary provides all needed tools; small footprint |
| Termux dependency | None | All Termux paths, packages, and assumptions removed |

---

## 2. Vendor Project Analysis

### 2.1 `vendor/proot/` — Upstream PRoot

**What it is:** A user-space implementation of `chroot`, `mount --bind`, and
`binfmt_misc` for Linux. It uses `ptrace()` to intercept system calls and
translate filesystem paths between a virtual "guest" namespace and the real
"host" namespace.

- **Language:** C (C99/C11 with GNU extensions)
- **License:** GPL-2.0-or-later
- **Build system:** GNU Make (`src/GNUmakefile`), no CMake/Autotools/Meson
- **Key dependency:** `libtalloc` (mandatory — hierarchical memory allocator
  from Samba, linked via `pkg-config --libs talloc`)

**Build steps:**

```bash
make -C src loader.elf loader-m32.elf build.h
make -C src proot
```

**Supported architectures:** x86_64, x86 (i386), ARM (EABI), ARM64 (AArch64),
SH4.

**Source layout:**

| Directory | Purpose |
|---|---|
| `src/cli/` | Command-line interface, option parsing, main entry point |
| `src/tracee/` | Tracee lifecycle, event loop, memory/register access |
| `src/syscall/` | System call translation (enter/exit), seccomp-BPF filter |
| `src/execve/` | ELF loading, shebang handling, execve emulation |
| `src/path/` | Path translation, bind mount emulation, canonicalization |
| `src/ptrace/` | Nested ptrace emulation (for debuggers under proot) |
| `src/loader/` | Freestanding ELF loader (no libc), embedded into proot binary |
| `src/extension/` | Extension framework: fake_id0, kompat, link2symlink, portmap |

**What vanilla proot is missing for Android:**

1. No handling of SIGSYS signals from Android's seccomp filter (blocks 40+
   legacy syscalls)
2. No ARM64 POKEDATA workaround for broken `PTRACE_POKEDATA` on some kernels
3. No `link2symlink` extension (SELinux blocks hard links on Android)
4. No `ashmem_memfd` extension (older Android kernels lack `memfd_create`)
5. No `sysvipc` extension (System V IPC userspace emulation)
6. No `statx()` syscall support (Android uses it extensively)
7. No f2fs filesystem bug workaround
8. No `--kill-on-exit` option
9. No `--link2symlink` CLI option
10. No `fix_symlink_size` extension

### 2.2 `vendor/termux-proot/` — Termux's Fork of PRoot

**What it is:** A copy of the upstream PRoot project with extensive patches
applied to work under Android/Termux. Git origin:
`github.com:termux/proot.git` (branch `master`).

**Build system:** Same GNU Make structure as vanilla, but with additional
source files compiled in:

```makefile
# termux-proot/src/GNUmakefile — additional objects not in vanilla:
path/f2fs-bug.o
tracee/seccomp.o
tracee/statx.o
extension/ashmem_memfd/ashmem_memfd.o
extension/hidden_files/hidden_files.o
extension/mountinfo/mountinfo.o
extension/port_switch/port_switch.o
extension/sysvipc/sysvipc.o
extension/sysvipc/sysvipc_msg.o
extension/sysvipc/sysvipc_sem.o
extension/sysvipc/sysvipc_shm.o
extension/link2symlink/link2symlink.o
extension/fix_symlink_size/fix_symlink_size.o
```

Also notable: `CC`, `STRIP`, `OBJCOPY`, `OBJDUMP` are all overridden with `?=`
(conditional assignment) to support cross-compilation, and `pkg-config` is not
used — `libtalloc` is linked directly via `-ltalloc`.

#### 2.2.1 SIGSYS/Seccomp Handler (`src/tracee/seccomp.c`, 568 lines)

This is the single most critical patch. Android's bionic libc and seccomp
policy block many legacy syscalls. When a blocked syscall fires, the kernel
delivers SIGSYS. This handler catches those signals and translates the old
syscalls into modern `*at()` equivalents:

| Blocked syscall | Translated to |
|---|---|
| `open` | `openat` |
| `stat` / `lstat` | `newfstatat` |
| `access` | `faccessat` |
| `chmod` | `fchmodat` |
| `chown` / `lchown` | `fchownat` |
| `unlink` / `rmdir` | `unlinkat` |
| `symlink` | `symlinkat` |
| `link` | `linkat` |
| `rename` | `renameat` |
| `mkdir` | `mkdirat` |
| `dup2` | `dup3` |
| `pipe` | `pipe2` |
| `accept` | `accept4` |
| `send` / `recv` | `sendto` / `recvfrom` |
| `waitpid` | `wait4` |
| `select` | `pselect6` |
| `poll` | `ppoll` |
| `epoll_wait` | `epoll_pwait` |
| `utime` / `utimes` | `utimensat` |
| `time` | userspace `time()` |
| `ftruncate` | `ftruncate64` |
| `statfs` | userspace `statfs64` + compat conversion |
| `statx` | userspace statx handler |
| `setgroups` | fake success (return 0) |
| `getpgrp` | fake via `getpgid()` |
| `setresuid` / `setresgid` | validate & fake |
| `set_robust_list` | fake ENOSYS |

Without this handler, any dynamically-linked binary inside proot that uses
legacy syscalls will crash immediately on Android.

#### 2.2.2 ARM64 Architecture Support (`src/arch.h`)

- Full AArch64 architecture support with `ARCH_ARM64` definition
- `HAS_POKEDATA_WORKAROUND = true` for ARM64 — works around broken
  `PTRACE_POKEDATA` on some Android kernels
- `SYSCALL_AVOIDER` changed from `-2` to `-1` for ARM64
- Loader addresses for ARM64: `0x2000000000` range
- ARM64-specific assembly stub in `mem.c` for the pokedata workaround
- 32-bit ARM on 64-bit kernel (`is_aarch32`) support
- ARM64-specific loader assembly: `src/loader/assembly-arm64.h`

#### 2.2.3 `link2symlink` Extension (`src/extension/link2symlink/`, 816 lines)

Emulates hard links with symbolic links because **Android's SELinux policies
block hard links**. Uses a naming scheme where the original file is renamed to
`.l2s.<name>.NNNN` (4-digit link count). Intercepts:

- `link()` / `linkat()` — creates symlink chains instead
- `stat()` / `lstat()` / `fstat()` — fakes `st_nlink` and `st_size`
- `unlink()` / `rename()` — maintains the link count

Has a `USERLAND` compile flag and integrates with the f2fs bug workaround.

#### 2.2.4 `ashmem_memfd` Extension (`src/extension/ashmem_memfd/`, 238 lines)

Emulates `memfd_create()` through Android's `/dev/ashmem` when the kernel
doesn't support memfd. Detects availability at startup by forking a test
process. If not available, translates `memfd_create` syscalls into
`openat("/dev/ashmem", ...)` and converts `ftruncate` on ashmem FDs to
`ioctl(ASHMEM_SET_SIZE)`.

#### 2.2.5 `sysvipc` Extension (`src/extension/sysvipc/`, ~1000+ lines)

Provides userspace emulation of the full System V IPC API: messages,
semaphores, and shared memory. Two proot instances get separate IPC namespaces.

#### 2.2.6 Other Additions

| File | Purpose |
|---|---|
| `src/tracee/statx.c` / `statx.h` | Handles the `statx()` syscall, translates paths, converts results to statx format |
| `src/path/f2fs-bug.c` / `f2fs-bug.h` | Workaround for f2fs case-sensitivity bug on Android |
| `src/extension/fix_symlink_size/` | Corrects `st_size` from `lstat()` for symbolic links |
| `src/extension/hidden_files/` | `-H` option to hide `.proot.*` files |
| `src/extension/mountinfo/` | Mount information extension |
| `src/extension/port_switch/` | Port switching for protected ports (`-p` option) |

#### 2.2.7 New CLI Options

| Option | Description |
|---|---|
| `--link2symlink` / `-l` | Emulate hard links with symlinks |
| `--ashmem-memfd` | Emulate memfd_create via ashmem |
| `--sysvipc` | Handle System V IPC syscalls |
| `--kill-on-exit` | Kill all tracees on main process exit |
| `-L` | Fix symlink size in lstat |
| `-H` | Hide `.proot.*` files |
| `-p` | Port switching for protected ports |

#### 2.2.8 Tracee Struct Additions

The `Tracee` struct in `src/tracee/tracee.h` has additional fields for:
- `killall_on_exit` — kill all tracees when main process exits
- `skip_next_seccomp_signal` — suppress spurious SIGSYS after voiding syscalls
- `seccomp_already_handled_enter` — prevent double-handling under seccomp
- `restore_original_regs_after_seccomp_event` — restore regs after SIGSYS rewrite
- `pokedata_workaround_stub_addr` and related fields for ARM64 workaround state
- `is_aarch32` — 32-bit ARM mode on 64-bit kernel
- `skip_proot_loader` — skip the ELF loader
- Enhanced syscall chaining fields

### 2.3 `vendor/termux-proot-distro/` — PRoot Distro Manager

**What it is:** A 3172-line Bash script (v4.38.0) that wraps the `proot`
utility for managing chroot-based Linux distribution installations on Android.
Licensed under GPL-3.0.

#### 2.3.1 File Layout

```
termux-proot-distro/
├── proot-distro.sh          # Main 3172-line Bash script (entire application)
├── install.sh               # Installer: sed-replaces @TERMUX_@ templates
├── distro-plugins/          # One .sh file per distro (metadata + setup hook)
│   ├── debian.sh            # URL + SHA256 for 4 architectures + locale setup
│   ├── alpine.sh            # Pure metadata (5 architectures)
│   ├── archlinux.sh         # PAM fix + locale setup
│   ├── ubuntu.sh            # Locale + Mozilla PPA setup
│   ├── fedora.sh            # authselect + PAM fix
│   ├── termux.sh            # SPECIAL: DISTRO_TYPE="termux", ZIP bootstrap
│   ├── distro.sh.sample     # Template for creating new plugins
│   └── ... (19 total)
├── distro-build/            # CI scripts to BUILD rootfs tarballs (not on device)
├── bootstrap-rootfs.sh      # CI orchestrator
├── completions/             # Bash and Fish shell completions
└── README.md
```

#### 2.3.2 Installation Flow (`command_install`, lines 234–663)

The install flow is a 7-step pipeline:

1. **Parse arguments** — accept `--override-alias` for custom naming
2. **Validate distro** — check it exists in plugin directory, not already
   installed
3. **Source the distro plugin** — each plugin defines:
   - `DISTRO_NAME`, `DISTRO_TYPE` (normal|termux), `DISTRO_COMMENT`
   - `TARBALL_URL[arch]` — associative array of download URLs per architecture
   - `TARBALL_SHA256[arch]` — integrity checksums
   - `TARBALL_STRIP_OPT` — path components to strip (default: 1)
   - Optional `distro_setup()` function for post-install hooks
4. **Download** rootfs tarball with `curl`, cached in
   `$RUNTIME_DIR/dlcache/`
5. **Verify SHA-256** checksum
6. **Extract rootfs** — two paths:
   - **Normal distros**: `proot --link2symlink tar ...` (handles hardlink
     emulation)
   - **Termux type**: `unzip` for bootstrap ZIP, restore symlinks from
     `SYMLINKS.txt`
7. **Post-install setup** for normal distros:
   - Write `/etc/environment` with Android environment variables
   - Fix PATH in `/etc/bash.bashrc`, `/etc/profile`, `/etc/login.defs`
   - Create `/etc/resolv.conf` (Google DNS: 8.8.8.8, 8.8.4.4)
   - Create `/etc/hosts`
   - Register Android UIDs/GIDs in `/etc/passwd`, `/etc/group`,
     `/etc/shadow`, `/etc/gshadow` (prefixed with `aid_`)
   - Create fake `/proc` and `/sys` data files via `setup_fake_sysdata()`
   - Run optional `distro_setup()` hook from plugin

#### 2.3.3 Login Flow (`command_login`, lines 1544–2201)

Constructs a massive `proot` command line:

1. **Detect CPU architecture** of installed rootfs (reads ELF headers with
   `file` command)
2. **Determine if CPU emulation needed** (guest arch != host arch):
   - Uses QEMU user-mode or Blink for x86_64
   - aarch64 host can run arm natively (if 32-bit support present)
   - x86_64 host can run i686 natively
3. **Construct proot invocation** with many `--bind` mounts:

   **Core binds (always present):**
   - `/dev`, `/proc`, `/sys`
   - `/dev/urandom → /dev/random`
   - `/proc/self/fd → /dev/fd`
   - `/proc/self/fd/{0,1,2} → /dev/{stdin,stdout,stderr}`

   **Fake /proc entries (bind-mounted from pre-created files):**
   - `/proc/loadavg`, `/proc/stat`, `/proc/uptime`, `/proc/version`,
     `/proc/vmstat`
   - `/proc/sys/kernel/cap_last_cap`
   - `/proc/sys/fs/inotify/max_user_watches`

   **Fake /sys entries:**
   - Empty directory bound to `/sys/fs/selinux` (hide SELinux)

   **Other binds:**
   - `${rootfs}/tmp → /dev/shm`

   **Non-isolated mode adds:**
   - `/apex`, `/data/dalvik-cache`, `/data/data/com.termux`, `/sdcard`,
     `/storage`, `/system`, `/system_ext`, `/vendor`, `/odm`, `/product`
   - `/linkerconfig/ld.config.txt`,
     `/linkerconfig/com.android.art/ld.config.txt`
   - `/plat_property_contexts`, `/property_contexts`
   - Termux prefix (`@TERMUX_PREFIX@`), Termux home (`@TERMUX_HOME@`)

4. **Set proot flags:** `--link2symlink`, `--sysvipc`, `--kill-on-exit`,
   `--root-id`, fake kernel version (`6.17.0-PRoot-Distro`), `-L` (fix symlink
   size)
5. **Execute:** `exec proot "$@"` — replaces the current shell

#### 2.3.4 Fake System Data (`setup_fake_sysdata`, lines 877–1100)

Creates static files that are bind-mounted over restricted `/proc` paths:

| File | Content |
|---|---|
| `proc/.loadavg` | `"0.12 0.07 0.02 2/165 765"` |
| `proc/.stat` | CPU statistics with 8 CPUs |
| `proc/.uptime` | `"124.08 932.80"` |
| `proc/.version` | `"Linux version 6.17.0-PRoot-Distro ..."` |
| `proc/.vmstat` | Detailed VM statistics |
| `proc/.sysctl_entry_cap_last_cap` | Capability info |
| `proc/.sysctl_inotify_max_user_watches` | Inotify limit |
| `sys/.empty/` | Empty directory for SELinux masking |

#### 2.3.5 Other Commands

| Command | Function | Lines |
|---|---|---|
| `command_remove` | `chmod u+rwx -R` + `rm -rf` rootfs | 1163–1272 |
| `command_rename` | Move rootfs dir, update symlinks, copy plugin | 1273–1449 |
| `command_reset` | remove + install (shortcut) | 1450–1543 |
| `command_list` | Enumerate plugins, show install status | 2311–2415 |
| `command_backup` | Tar rootfs + plugin | 2416–2608 |
| `command_restore` | Extract backup tar | 2609–2804 |
| `command_copy` | cp between host and guest | 2805–end |
| `run_proot_cmd` | Execute command inside rootfs (used by distro_setup) | 665–676 |

#### 2.3.6 Distro Plugin Format

Each plugin is a Bash fragment that gets `source`d. Example (Debian):

```bash
DISTRO_NAME="Debian (trixie)"
DISTRO_COMMENT="Stable release."

TARBALL_URL['aarch64']="https://easycli.sh/proot-distro/debian-trixie-aarch64-pd-v4.37.0.tar.xz"
TARBALL_SHA256['aarch64']="9bd3b19ff7cd300c7c7bf33124b726eb199f4bab9a3b1472f34749c6d12c9195"
TARBALL_URL['arm']="https://easycli.sh/proot-distro/debian-trixie-arm-pd-v4.37.0.tar.xz"
TARBALL_SHA256['arm']="af9b22fc1b82ccc665e484342af71c35a86f9f3dd525b0f423649976dded239f"
# ... more architectures

distro_setup() {
    sed -i -E 's/#[[:space:]]?(en_US.UTF-8[[:space:]]+UTF-8)/\1/g' ./etc/locale.gen
    run_proot_cmd DEBIAN_FRONTEND=noninteractive dpkg-reconfigure locales
}
```

**Plugin variables reference:**

| Variable | Purpose |
|---|---|
| `DISTRO_NAME` | Human-readable distribution name |
| `DISTRO_TYPE` | `normal` (default) or `termux` |
| `DISTRO_COMMENT` | Comment shown in `list` output |
| `DISTRO_ARCH` | Override CPU architecture (set by user env var) |
| `TARBALL_URL[arch]` | Download URL per architecture |
| `TARBALL_SHA256[arch]` | SHA-256 per tarball |
| `TARBALL_STRIP_OPT` | Tar path strip count (default: 1) |
| `distro_setup()` | Optional post-install function |

---

## 3. Termux-Specific Dependencies to Replace

### 3.1 Hard-Coded Template Variables

The `proot-distro.sh` script uses `@TERMUX_@` placeholders that get replaced
by `install.sh` at install time:

| Template | Termux Value | Used For | Standalone Replacement |
|---|---|---|---|
| `@TERMUX_PREFIX@` | `/data/data/com.termux/files/usr` | Binary location, config dirs, runtime data, PATH override | `${APP_DATA_DIR}/usr` |
| `@TERMUX_HOME@` | `/data/data/com.termux/files/home` | User home, bind-mounted into guest | `${APP_DATA_DIR}/home` |
| `@TERMUX_APP_PACKAGE@` | `com.termux` | Android app package name, data paths | Your app's package name |

### 3.2 Required Runtime Utilities

The dependency check at line 123–131 of `proot-distro.sh`:

```bash
for i in awk basename bzip2 cat chmod cp curl cut du file find grep gzip \
    head id lscpu mkdir proot rm sed tar unzip xargs xz; do
    if [ -z "$(command -v "$i")" ]; then
        msg "${BRED}Utility '${i}' is not installed. Cannot continue.${RST}"
        exit 1
    fi
done
```

All of these will be provided by a **static busybox binary** bundled in the
APK, except:

| Utility | Solution |
|---|---|
| `proot` | Built from patched source, bundled as native binary |
| `curl` | Busybox `wget` as fallback, or bundle a static curl |
| `file` | Busybox `file` has limited ELF support; may need custom ELF header reader |
| `lscpu` | Used only for 32-bit support detection; replace with `/proc/cpuinfo` parsing |
| `unzip` | Only needed for `DISTRO_TYPE="termux"` which will be removed |

### 3.3 Termux-Specific Paths and Behaviors to Remove

These are embedded throughout `proot-distro.sh` and must be stripped or
replaced:

| Item | Location(s) | Action |
|---|---|---|
| `DISTRO_TYPE="termux"` code path | `command_install`, `command_login` | Remove entirely |
| `LD_PRELOAD` save/restore | Line 34, line 2198 | Remove |
| `@TERMUX_PREFIX@/bin` PATH override | Line 37 | Replace with busybox path |
| Termux data directory bind mounts | Lines 2074–2085 | Remove or replace with app's data dir |
| Termux prefix bind mount | Line 2151 | Remove |
| Termux home bind mount (`--termux-home`) | Lines 1578, 2157–2178 | Remove option and code |
| Termux shared tmp (`--shared-tmp`) | Lines 1581, 2182–2184 | Remove or adapt |
| `dpkg` architecture check | Optional check | Remove |
| Termux-specific `distro_setup()` hooks | Various plugins | Audit and adapt |
| Termux home in `detect_cpu_arch` | Line 197 | Remove path from check list |
| Android `aid_*` user/group registration | `command_install` post-install | Keep (Android-generic) |
| Termux app package references in help text | Lines 2268, 2274–2279 | Update wording |

### 3.4 Android-Specific Behaviors to KEEP

These are Android-generic (not Termux-specific) and should be preserved:

| Item | Why Keep |
|---|---|
| Fake `/proc/loadavg`, `/proc/stat`, `/proc/uptime`, `/proc/version`, `/proc/vmstat` | Android restricts read access to these |
| Fake `/proc/sys/kernel/cap_last_cap`, `/proc/sys/fs/inotify/max_user_watches` | Not readable on Android |
| Fake `/sys/fs/selinux` (empty directory) | Hide SELinux from guest programs |
| `/dev/urandom → /dev/random` bind | Android doesn't allow reading `/dev/random` |
| Android UID/GID registration in guest `/etc/passwd`, `/etc/group` | Needed for file ownership mapping |
| `/apex`, `/system`, `/vendor`, `/odm`, `/product` bind mounts (non-isolated mode) | Needed for QEMU and host interaction |
| `/linkerconfig/ld.config.txt` bind | Needed for QEMU |
| Fake kernel release string (`6.17.0-PRoot-Distro`) | Old Android kernels have outdated glibc compatibility issues |
| Android environment variables (`ANDROID_ART_ROOT`, `ANDROID_DATA`, etc.) | Needed by some guest programs |
| `/storage`, `/sdcard` bind mounts | Access to shared storage |
| `/data/dalvik-cache` bind | Needed for QEMU |

---

## 4. Cherry-Pick Plan: termux-proot Patches into Vanilla proot

### 4.1 Patch Priority

| Priority | Patch / Feature | Source File(s) | Justification |
|---|---|---|---|
| **P0** | SIGSYS/seccomp handler | `src/tracee/seccomp.c`, `src/tracee/seccomp.h` | Android blocks 40+ syscalls; nothing works without this |
| **P0** | ARM64 arch support + POKEDATA workaround | `src/arch.h`, `src/tracee/mem.c`, `src/loader/assembly-arm64.h` | Required for all modern Android devices (aarch64) |
| **P0** | `link2symlink` extension | `src/extension/link2symlink/` | SELinux blocks hard links on Android |
| **P1** | `ashmem_memfd` extension | `src/extension/ashmem_memfd/` | Older Android kernels lack `memfd_create` |
| **P1** | `statx()` support | `src/tracee/statx.c`, `src/tracee/statx.h` | Android uses `statx` extensively |
| **P1** | `--kill-on-exit` option | `src/cli/proot.c`, `src/tracee/tracee.h`, `src/tracee/event.c` | Prevents orphan processes on session exit |
| **P2** | `sysvipc` extension | `src/extension/sysvipc/` | Some distros need System V IPC |
| **P2** | f2fs bug workaround | `src/path/f2fs-bug.c`, `src/path/f2fs-bug.h` | Device-specific filesystem bug |
| **P2** | `fix_symlink_size` extension | `src/extension/fix_symlink_size/` | Prevents dpkg symlink size warnings |
| **P2** | `hidden_files` extension | `src/extension/hidden_files/` | Hide `.proot.*` files from guest |
| **P2** | `mountinfo` extension | `src/extension/mountinfo/` | Mount information |
| **P2** | `port_switch` extension | `src/extension/port_switch/` | Port switching for protected ports |
| **P3** | 32-bit ARM on 64-bit kernel support | `src/tracee/tracee.h` (`is_aarch32`) | For running arm distros on aarch64 |
| **P3** | Enhanced syscall chaining | `src/tracee/tracee.h` (chain fields) | Improved stability |
| **P3** | Seccomp filter additions | `src/syscall/seccomp.c` | Android-specific syscalls in filter |

### 4.2 Integration Approach

1. Copy `vendor/proot/` into `src/proot/` as a working copy (vendor submodules remain pristine references).
2. Copy over entire new files from `vendor/termux-proot/` into `src/proot/` (extensions, seccomp
   handler, statx, f2fs-bug, ARM64 loader assembly).
3. Merge modifications to existing files in `src/proot/` (`arch.h`, `tracee.h`, `mem.c`,
   `event.c`, `GNUmakefile`, `proot.c`, `proot.h`).
4. Update `GNUmakefile` to compile the new objects.
5. Test build with Android NDK standalone toolchain.

### 4.3 Build Dependencies for proot

| Dependency | How to Obtain |
|---|---|
| Android NDK | Download from developer.android.com |
| `libtalloc` | Cross-compile from source as static library with NDK |
| GNU Make | Host tool |
| `objcopy`, `objdump`, `strip` | From NDK toolchain |

**Build target:** Single static `proot` binary for `aarch64` (primary), with
optional `arm` support.

---

## 5. Proposed Standalone Architecture

### 5.1 Android App Structure

```
id.or.oo.pr/
├── src/
│   ├── proot/                         # Working copy: vanilla proot + cherry-picked patches
│   │   └── src/                       # Patched proot source (built with NDK)
│   └── scripts/
│       └── proot-distro.sh            # Standalone proot-distro script
├── app/src/main/
│   ├── java/com/example/linuxonandroid/
│   │   ├── MainActivity.java          # App entry point, UI
│   │   ├── TerminalActivity.java      # Terminal emulator
│   │   ├── DistroManager.java         # Install/remove/list distros
│   │   └── ProotLauncher.java         # JNI bridge to launch proot
│   ├── assets/
│   │   ├── bin/
│   │   │   └── busybox-aarch64        # Static busybox binary
│   │   ├── scripts/
│   │   │   └── proot-distro.sh        # Standalone proot-distro script
│   │   └── plugins/
│   │       ├── debian.sh              # Distro plugin definitions
│   │       ├── alpine.sh
│   │       └── ...
│   └── res/
│       └── ...                        # Android resources
├── vendor/                            # Pristine git submodules (reference only)
│   ├── proot/                         # Upstream vanilla proot
│   ├── termux-proot/                  # Termux fork (Android patch source)
│   └── termux-proot-distro/           # Termux distro manager (script source)
├── build.sh                           # Build script: NDK compile + asset packaging
└── README.md
```

### 5.2 Runtime Data Layout (on device)

```
/data/data/id.or.oo.pr/
├── files/
│   ├── usr/
│   │   ├── bin/
│   │   │   ├── proot                  # Native proot binary (extracted from lib/)
│   │   │   ├── busybox                # Static busybox (extracted from assets)
│   │   │   ├── sh -> busybox          # Symlinks for standard utilities
│   │   │   ├── bash -> busybox
│   │   │   ├── tar -> busybox
│   │   │   ├── curl -> busybox        # or separate static curl
│   │   │   └── ...                    # Other busybox applet symlinks
│   │   └── etc/
│   │       └── proot-distro/
│   │           ├── debian.sh          # Distro plugins
│   │           ├── alpine.sh
│   │           └── ...
│   ├── home/                          # User home directory
│   └── tmp/                           # Temporary directory
├── cache/
│   └── proot-distro/
│       └── dlcache/                   # Downloaded tarball cache
└── lib/
    └── libproot.so                    # proot as JNI library (or extracted binary)
```

Installed rootfs layout:

```
/data/data/id.or.oo.pr/files/usr/var/lib/proot-distro/
└── installed-rootfs/
    └── debian/
        ├── .l2s/                      # proot link2symlink data
        ├── bin/, etc/, usr/, ...      # The actual rootfs
        ├── proc/
        │   ├── .loadavg               # Fake /proc data files
        │   ├── .stat
        │   ├── .uptime
        │   ├── .version
        │   └── .vmstat
        └── sys/
            └── .empty/                # Fake SELinux mask
```

### 5.3 Ported proot-distro.sh Changes

The standalone `proot-distro.sh` will be forked from the Termux version with
these modifications:

#### Template Variable Replacements

```
@TERMUX_PREFIX@         → ${APP_PREFIX}     (e.g. /data/data/id.or.oo.pr/files/usr)
@TERMUX_HOME@           → ${APP_HOME}        (e.g. /data/data/id.or.oo.pr/files/home)
@TERMUX_APP_PACKAGE@    → ${APP_PACKAGE}     (e.g. id.or.oo.pr)
```

These are resolved at install time by the Android app's initialization code,
not via `sed` at build time.

#### Removed Code

1. **`DISTRO_TYPE="termux"` entire code path** — the `termux` distro type,
   its ZIP-based bootstrap, `SYMLINKS.txt` handling, Termux-specific login
   flow (lines 1786–1797, 2082–2085)
2. **`LD_PRELOAD` save/restore** (line 34, line 2198)
3. **Termux prefix bind mount** (line 2151)
4. **`--termux-home` option** (lines 1578, 2157–2178)
5. **`--shared-tmp` option** referencing Termux tmp (lines 1581, 2182–2184)
6. **Termux-specific help text** about Termux utilities, PAM, and
   `/etc/environment` (lines 2274–2293)
7. **GNU bash and GNU tar version checks** (lines 134–146) — busybox
   provides compatible-enough versions
8. **`dpkg` architecture check**
9. **Termux-specific `detect_cpu_arch` paths** (line 197:
   `/data/data/com.termux/files/usr/bin/bash`)
10. **Anti-root fuse** warning about Termux specifically (line 159)

#### Adapted Code

1. **Dependency check** (lines 123–131) — remove `unzip`, `lscpu` from
   required list; resolve via busybox symlinks
2. **`detect_cpu_arch`** — remove Termux paths, add busybox-based ELF header
   detection as fallback
3. **32-bit CPU support detection** — replace `lscpu` with `/proc/cpuinfo`
   parsing
4. **QEMU path** — replace `@TERMUX_PREFIX@/bin/qemu-*` with bundled QEMU
   path or downloadable QEMU binary
5. **Non-isolated bind mounts** — replace Termux data paths with app's own
   data paths where appropriate
6. **`run_proot_cmd`** — update proot invocation path to bundled binary
7. **`DEFAULT_PATH_ENV`** — remove `@TERMUX_PREFIX@/bin` and
   `/system/bin:/system/xbin` references; add app's bin directory
8. **`setup_fake_sysdata`** — update kernel version string to not reference
   Termux

#### New Code

1. **Busybox bootstrap** — on first run, extract busybox from assets and
   create applet symlinks
2. **Self-initialization** — create directory structure on first launch
3. **App context integration** — receive paths from Android app via
   environment variables or arguments

---

## 6. Build Strategy

### 6.1 Building proot

```bash
# Setup NDK standalone toolchain
NDK=/path/to/android-ndk
$NDK/build/tools/make_standalone_toolchain.py \
    --arch arm64 --api 21 --install-dir /tmp/ndk-arm64

# Build libtalloc as static library
git clone https://gitlab.com/samba-team/talloc.git
cd talloc
./configure --host=aarch64-linux-android \
    --prefix=/tmp/ndk-arm64/sysroot/usr \
    --disable-shared --enable-static
make && make install

# Build proot with cherry-picked patches
cd src/proot/src
make -f GNUmakefile \
    CC=/tmp/ndk-arm64/bin/aarch64-linux-android-gcc \
    STRIP=/tmp/ndk-arm64/bin/aarch64-linux-android-strip \
    OBJCOPY=/tmp/ndk-arm64/bin/aarch64-linux-android-objcopy \
    OBJDUMP=/tmp/ndk-arm64/bin/aarch64-linux-android-objdump \
    LDFLAGS="-ltalloc -static -Wl,-z,noexecstack" \
    proot
```

### 6.2 Bundling busybox

Option A: Use a pre-built static busybox binary for aarch64 (available from
busybox.net or various Android tool projects).

Option B: Build from source with Android NDK, selecting only the needed
applets:

```
awk, basename, bzip2, cat, chmod, cp, cut, du, find, grep, gzip,
head, id, mkdir, rm, sed, tar, xargs, xz, sha256sum, file, wget,
unzip, mknod, chown, env, false, ln, ls, mv, printf, pwd, readlink,
realpath, stat, touch, tr, true, uname, wc, which
```

### 6.3 Rootfs Tarball Hosting

The existing tarballs hosted at `easycli.sh` are generic Linux rootfs images
that do not contain Termux-specific modifications. They can be used directly
by the standalone version. Alternatively:

- Host your own mirror for reliability
- Support `file://` URLs for offline installation
- Bundle minimal rootfs (e.g., Alpine) in APK assets for first-launch demo

---

## 7. Open Questions and Risks

### 7.1 Open Questions

1. **QEMU bundling**: Should QEMU user-mode binaries be bundled in the APK
   (adds ~15MB per architecture), downloaded on-demand, or not supported in
   v1?
2. **Storage access**: How will the app access `/sdcard`? Via Android storage
   access framework, or direct bind mount (requires MANAGE_EXTERNAL_STORAGE
   permission)?
3. **Terminal emulator**: Use a library like `termux-app`'s Terminal Emulator
   (Apache 2.0), Android's built-in `AlertDialog`, or a WebView-based
   terminal?
4. **SELinux policy**: Some Android devices have strict SELinux policies that
   may block `ptrace()` even for app processes. Need testing across OEMs.
5. **Android version support**: Minimum API level? API 21 (Android 5.0) gives
   broad compatibility but API 28 (Android 9) has better security and
   file access patterns.

### 7.2 Known Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Some OEMs block `ptrace()` via SELinux | proot won't work at all | Document affected devices; test on major OEMs |
| busybox `tar` may not handle all formats | Installation failures | Test with each distro's tarball; may need full GNU tar |
| busybox `file` has limited ELF detection | Architecture misdetection | Write custom ELF header reader in shell or bundle real `file` |
| Large APK size (busybox + proot + plugins) | User download reluctance | Keep under 10MB; download rootfs on-demand |
| proot performance overhead on old devices | Poor user experience | Recommend distros with lighter package sets (Alpine) |

---

## 8. Recommended Implementation Order

| Phase | Tasks | Est. Effort |
|---|---|---|
| **Phase 1** — proot binary | Cherry-pick P0 patches into vanilla proot; set up NDK build; produce static aarch64 binary; verify it runs on Android | High |
| **Phase 2** — standalone script | Fork `proot-distro.sh`; remove all Termux-isms; test install/login/remove with Alpine (smallest rootfs) via adb shell | Medium |
| **Phase 3** — busybox integration | Bundle static busybox; create applet symlinks on first run; replace all utility references | Low |
| **Phase 4** — Android app shell | Create APK with JNI launcher; integrate terminal emulator; wire up proot-distro.sh to UI | Medium |
| **Phase 5** — multi-distro support | Add remaining distro plugins; test Debian, Ubuntu, Arch; handle edge cases | Medium |
| **Phase 6** — polish | QEMU integration for foreign architectures; backup/restore UI; settings; error handling | High |

**Suggested first milestone:** Install and login to Alpine Linux inside an
`adb shell` session using only the standalone proot binary, busybox, and the
ported `proot-distro.sh` — no APK, no Java, just native binaries and shell
scripts. This validates the core approach before investing in the Android app.
