# Busybox & Bash Integration

Phase 3 obtains and integrates static busybox and bash binaries, creates the bootstrap script for first-run initialization, and verifies tool compatibility with our standalone Android environment.

Status: **Complete** (commits `c32a86b..df73f58`)

Superseded: the APK no longer ships bash or busybox, and `bootstrap.sh` is gone. pr-cli does
its file work in Rust and `PrBootstrap` creates the prefix directories. This page records the
original phase.

---

## Architecture

```
download-busybox.sh                    # Download script (host, pins Alpine package)
download-bash.sh                       # Download script (host, pins GitHub release)
build/assets/arm64-v8a/busybox         # Static busybox binary (1.1MB, aarch64)
build/assets/arm64-v8a/bash            # Static bash binary (2.3MB, aarch64)
src/scripts/bootstrap.sh               # First-run setup (POSIX sh, 184 lines)
src/scripts/proot-distro.sh            # Main script (3051 lines, updated in T3.5)
src/scripts/plugins/                   # 17 distro plugins (unchanged)
```

Runtime directory layout after bootstrap:

```
${APP_PREFIX}/
├── .bootstrapped                      # Marker file (idempotency)
├── bin/
│   ├── busybox                        # Static binary (1.1MB)
│   ├── bash                           # Static binary (2.3MB, overwrites busybox symlink)
│   ├── proot                          # Static binary (2.5MB, from Phase 1)
│   ├── proot-distro                   # proot-distro.sh with shebang replaced
│   ├── awk -> busybox                 # 311 applet symlinks total
│   ├── tar -> busybox
│   ├── grep -> busybox
│   └── ... (308 more)
├── etc/proot-distro/
│   ├── alpine.sh                      # 17 distro plugins
│   ├── debian.sh
│   └── ...
├── scripts/
│   └── proot-distro.sh               # Original script (template shebang)
├── plugins/                           # Staging dir for plugin install
├── tmp/                               # TMPDIR for proot loader
├── home/                              # User home
└── var/lib/proot-distro/
    ├── dlcache/                       # Downloaded rootfs tarballs
    └── installed-rootfs/              # Extracted distro rootfs
```

---

## What Was Done

### T3.1 — Static busybox binary

Downloaded Alpine's `busybox-static` package with pinned version and SHA256 verification.

**Binary**: BusyBox v1.37.0-r14 (1.1MB, static aarch64, GPL-2.0-only)

**Source**: `https://dl-cdn.alpinelinux.org/alpine/v3.21/main/aarch64/busybox-static-1.37.0-r14.apk`

| Property | Value |
|---|---|
| SHA256 (APK) | `6fd7ea97062beb51fa785ba858f823e1dfe4daf6bfa91ff4d5359b1061988c69` |
| SHA256 (binary) | `e383c8bc25a1137b8ee88718cc6df1f1e84c54521d6045fc837385995dcdf031` |
| Size | 1,115,944 bytes (1.1MB) |
| TLS segment | None (no alignment fix needed) |
| Applets | 305 total (verified on-device via `busybox --list`) |

**Script**: `download-busybox.sh` at project root:
- Downloads pinned APK from Alpine CDN
- Verifies SHA256 of both APK and extracted binary
- Validates ELF64/aarch64/static-link
- Checks TLS alignment (< 64 bytes warns, >= 64 OK, absent OK)
- Outputs to `build/assets/arm64-v8a/busybox`
- Supports `--force`, `--verify-only`
- Caches in `build/dl/` for repeat runs

**Important finding**: Alpine's busybox-static does **NOT** include the `file` applet. See T3.5 for how this was resolved.

### T3.2 — Static bash binary

Downloaded `robxu9/bash-static` from GitHub with pinned version and SHA256 verification.

**Binary**: GNU bash 5.2.015 (2.3MB, static aarch64, GPL-3.0-or-later)

**Source**: `https://github.com/robxu9/bash-static/releases/download/5.2.015-1.2.3-2/bash-linux-aarch64`

| Property | Value |
|---|---|
| SHA256 (binary) | `8877ad33344af461ed801066322fd9a7808cd73e4e81087da228e32e8fad54ca` |
| Size | 2,380,424 bytes (2.3MB) |
| TLS segment | None (no alignment fix needed) |

**Script**: `download-bash.sh` at project root (same pattern as download-busybox.sh).

**Why we need bash** (not just busybox ash): proot-distro.sh uses bash-specific features:
- `declare -A` associative arrays (TARBALL_URL, TARBALL_SHA256, etc.)
- `mapfile` for reading /etc/environment
- `[[ ]]` conditional expressions
- `(( ))` arithmetic evaluation
- Process substitution `<(...)`
- `${var:start:length}` substring extraction
- Array operations `${#array[@]}`, `${!array[@]}`, `${array[@]}`

### T3.3 — Bootstrap script

Created `src/scripts/bootstrap.sh` — the first-run setup script called by the APK's BootstrapService.

**Key constraint**: Must be POSIX sh compatible (`#!/system/bin/sh`). It runs BEFORE bash is available, so no bash-isms allowed (no `[[`, no arrays, no `declare`, no process substitution). Verified with `dash -n`.

**Setup sequence**:

```
1. Check .bootstrapped marker (skip if already run)
2. Create directory structure:
   - ${APP_PREFIX}/bin
   - ${APP_PREFIX}/etc/proot-distro
   - ${APP_PREFIX}/var/lib/proot-distro/installed-rootfs
   - ${APP_PREFIX}/var/lib/proot-distro/dlcache
   - ${APP_PREFIX}/home
   - ${APP_PREFIX}/tmp
   - ${APP_PREFIX}/scripts
3. Install busybox → chmod 755
4. Create applet symlinks via busybox --list (305 applets → 311 entries after bash/proot added)
5. Install bash → chmod 755 (overwrites busybox's bash applet symlink with real binary)
6. Install proot → chmod 755
7. Copy proot-distro.sh → bin/proot-distro → chmod 755
8. Replace @APP_PREFIX@ in shebang via sed
9. Copy plugins from plugins/ to etc/proot-distro/
10. Write .bootstrapped marker
```

**Design decisions**:
- **POSIX sh only**: Avoids chicken-and-egg problem (need to install bash before bash exists)
- **Idempotent**: Checks `.bootstrapped` marker; remove it to re-run
- **Shebang fix**: `sed -i "s|@APP_PREFIX@|${APP_PREFIX}|g"` replaces the only remaining template so `proot-distro` can be invoked directly as an executable
- **bash overwrites busybox symlink**: busybox creates `bash -> busybox` applet symlink, but real static bash is installed on top (busybox ash lacks needed features)

**On-device test results** (Samsung Android 16, aarch64, non-rooted):

```
[bootstrap] Starting bootstrap for id.or.oo.pr
[bootstrap] Creating directory structure...
[bootstrap] Installing busybox...
[bootstrap] Creating applet symlinks...
[bootstrap] busybox: 311 entries in /data/local/tmp/pr-test/usr/bin
[bootstrap] Installing bash...
[bootstrap] bash installed (2380424 bytes)
[bootstrap] Installing proot...
[bootstrap] proot installed
[bootstrap] Installing proot-distro.sh...
[bootstrap] Replacing @APP_PREFIX@ template in shebang...
[bootstrap] proot-distro installed
[bootstrap] Installing distro plugins...
[bootstrap] 17 plugins installed
[bootstrap] Bootstrap complete
```

All post-bootstrap tests passed:
- `proot-distro list` — 17 distros listed (shebang works!)
- `proot-distro install alpine` — rootfs extracted correctly
- `proot-distro login alpine` — `root@alpine`, `uname -r` → `6.17.0-pr`

### T3.4 — Tar compatibility verification

Tested busybox tar v1.37.0 extraction of all three compression formats on device.

**Result: All formats work.**

| Format | Files | Symlinks | Subdirs | --strip | proot wrapper |
|---|---|---|---|---|---|
| `.tar.xz` | OK | OK | OK | OK | OK |
| `.tar.gz` | OK | OK | OK | — | — |
| `.tar.bz2` | OK | OK | OK | — | — |

**Finding**: All 17 distro plugins use `.tar.xz` exclusively. No plugin uses `.tar.gz` or `.tar.bz2`. However, busybox supports all three, providing forward compatibility if future plugins use different formats.

**GNU tar options removed** (in T2.10, confirmed safe here):
- `--warning=no-unknown-keyword` — busybox doesn't emit these warnings
- `--delay-directory-restore` — not needed for our rootfs archives
- `--preserve-permissions` — umask behavior is acceptable

No need to bundle GNU tar as a fallback.

### T3.5 — Replace file(1) with ELF header parsing

**Critical finding**: Alpine's busybox-static v1.37.0 does **NOT** include the `file` applet. The bootstrap created the symlink (`file -> busybox`) but busybox returned `file: applet not found`. This meant `detect_cpu_arch()` was broken.

**Solution**: Parse the ELF header's `e_machine` field directly using `od`:

```bash
machine=$(od -A n -t x1 -j 18 -N 2 "${binary}" 2>/dev/null | tr -d ' ')
```

This reads 2 bytes at offset 18 from the ELF header (the `e_machine` field) and maps the hex value:

| e_machine | Architecture | EM constant |
|---|---|---|
| `0xb7` (183) | aarch64 | EM_AARCH64 |
| `0x28` (40) | arm | EM_ARM |
| `0x3e` (62) | x86_64 | EM_X86_64 |
| `0x03` (3) | i686 | EM_386 |
| `0xf3` (243) | riscv64 | EM_RISCV |
| `0x08` (8) | mips | EM_MIPS |

**Why this is better than file(1)**:
- No dependency on external `file` command (busybox may not have it)
- More reliable: reads binary format directly, not human-readable output
- Simpler: no regex parsing of varying output formats
- `od` is present in all busybox builds

**Updated detect_cpu_arch()** (`src/scripts/proot-distro.sh:232-257`):
- First probes common binaries (`/usr/bin/bash`, `/bin/sh`, `/bin/busybox`, etc.) using `dd` to check for ELF magic
- Then reads `e_machine` with `od` instead of calling `file`
- Added file-existence check before `dd` probe (avoids errors on missing files)

**Updated dependency check** (line 129):
- Removed: `file`
- Added: `dd`, `hexdump`

---

## Busybox Applet Inventory

Alpine's busybox-static v1.37.0 provides 305 applets. Key ones for proot-distro:

**Core (required by proot-distro.sh)**:
`awk`, `basename`, `cat`, `chmod`, `cp`, `cut`, `dd`, `du`, `find`, `grep`,
`gzip`, `head`, `id`, `mkdir`, `rm`, `sed`, `tar`, `wget`, `sha256sum`,
`stat`, `realpath`, `uname`, `xargs`, `hexdump`

**Missing applets** (needed but not available):
- `file` — replaced with ELF header parsing via `od` (T3.5)
- `curl` — optional, wget fallback in download_file() (T2.4)
- `paste` — replaced with bash array loop (T2.10)
- `xz` — not present as standalone, but busybox tar handles .tar.xz internally

**Not present and not needed**:
- `bash` — we bundle a real static bash (T3.2)
- `bzip2` — not used by any plugin (tar handles decompression internally)

---

## Build Asset Inventory

After Phase 3, the following assets are available for APK bundling:

| File | Size | Source | Task |
|---|---|---|---|
| `build/assets/arm64-v8a/busybox` | 1.1MB | Alpine package | T3.1 |
| `build/assets/arm64-v8a/bash` | 2.3MB | robxu9/bash-static | T3.2 |
| `build/out/arm64/proot` | 2.5MB | NDK cross-compile | T1.4/T1.5 |
| `src/scripts/bootstrap.sh` | 5KB | New (POSIX sh) | T3.3 |
| `src/scripts/proot-distro.sh` | 101KB | Ported from Termux | T2.1-T3.5 |
| `src/scripts/plugins/*.sh` | 15KB | 17 plugins | T2.1 |

**Total APK payload**: ~6MB of binaries + scripts (before compression).

---

## Download Scripts

Both download scripts follow the same pattern for reproducibility:

| Script | Source | Pin | SHA256 checks |
|---|---|---|---|
| `download-busybox.sh` | `dl-cdn.alpinelinux.org` | `busybox-static-1.37.0-r14.apk` | APK + extracted binary |
| `download-bash.sh` | `github.com/robxu9/bash-static` | `5.2.015-1.2.3-2` | Downloaded file |

Both scripts:
- Verify SHA256 checksum of all downloaded/extracted files
- Validate ELF64/aarch64/static-link/no-TLS-issues
- Support `--force` (re-download) and `--verify-only` (check existing)
- Cache downloads in `build/dl/` for repeat runs
- Output to `build/assets/arm64-v8a/` for APK bundling

---

## Changes to proot-distro.sh in Phase 3

| Line(s) | Change | Task |
|---|---|---|
| 129 | Dependency check: removed `file`, added `dd` and `hexdump` | T3.5 |
| 232-257 | `detect_cpu_arch()`: replaced `file -L | grep -oE` with `od -t x1 -j 18 -N 2` ELF header parsing | T3.5 |
| 238 | Added file-existence check before `dd` ELF magic probe | T3.5 |
| 243-251 | Architecture mapping via e_machine hex values | T3.5 |

---

## Carry-Forward to Phase 4

The following findings from Phase 3 directly affect Phase 4 (Android APK):

### 1. Bootstrap is the APK entry point

`src/scripts/bootstrap.sh` is what the APK's BootstrapService calls on first launch. Phase 4 must:

1. Extract `proot` from `jniLibs/arm64-v8a/libproot.so` to `${APP_PREFIX}/bin/proot`
2. Copy `busybox` and `bash` from `assets/bin/` to `${APP_PREFIX}/bin/`
3. Copy `bootstrap.sh`, `proot-distro.sh`, and plugins from `assets/` to their locations
4. Execute `bootstrap.sh` via `/system/bin/sh`

The APK structure should be:

```
app/src/main/
├── jniLibs/arm64-v8a/libproot.so    # proot binary (Android extracts native libs)
├── assets/
│   ├── bin/
│   │   ├── busybox                  # from build/assets/arm64-v8a/busybox
│   │   └── bash                     # from build/assets/arm64-v8a/bash
│   ├── scripts/
│   │   ├── bootstrap.sh             # src/scripts/bootstrap.sh
│   │   └── proot-distro.sh          # src/scripts/proot-distro.sh
│   └── plugins/
│       ├── alpine.sh                # 17 plugin files
│       └── ...
```

### 2. Environment variables the APK must set

Before calling bootstrap.sh or proot-distro.sh:

```
APP_PREFIX=/data/data/id.or.oo.pr/files/usr
APP_HOME=/data/data/id.or.oo.pr/files/home
APP_PACKAGE=id.or.oo.pr
PROOT_NO_SECCOMP=1       # CRITICAL: required on Android 14+
```

### 3. Only arm64 supported currently

All binaries are aarch64 only. The `build.sh` supports arm (32-bit) but we haven't tested it. Phase 4 should:
- Bundle arm64 binaries only for v1
- Add runtime arch detection if supporting multiple ABIs later
- `proot` is placed in `jniLibs/` with ABI-specific subdirectories

### 4. The `.bootstrapped` marker

Bootstrap writes `${APP_PREFIX}/.bootstrapped` on success. The APK should check this before running bootstrap. If the app is updated (new version), the marker should be removed or versioned to trigger re-bootstrap.

### 5. busybox `file` applet is missing

The `file` command is not available. Any future code that needs file-type detection must use ELF header parsing (see T3.5) or another approach. Do not add `file` to dependency checks.

### 6. proot as `libproot.so`

Android's package manager only extracts native libraries from `jniLibs/` if they have `.so` extension. The proot binary must be renamed to `libproot.so` for bundling. This is a standard Android trick (Termux does the same). After extraction, bootstrap.sh or BootstrapService must rename it to `proot`.

### 7. TLS alignment

Neither busybox nor bash have PT_TLS segments (no alignment issues). Only proot needed the TLS alignment fix (Phase 1). But if additional binaries are added later, check for TLS alignment < 64 bytes on aarch64.

---

## Reproducing the Build

```bash
# Download binaries (from project root)
scripts/download-busybox.sh     # → build/assets/arm64-v8a/busybox (1.1MB)
scripts/download-bash.sh        # → build/assets/arm64-v8a/bash (2.3MB)

# Build proot (from Phase 1)
scripts/build.sh --arch=arm64   # → build/out/arm64/proot (2.5MB)

# Test on device
scripts/test-push.sh setup      # Push binaries + run bootstrap.sh
scripts/test-push.sh test       # Push + bootstrap + proot-distro list
scripts/test-push.sh shell      # Push + bootstrap + interactive bash

# Verify binaries without device
scripts/download-busybox.sh --verify-only
scripts/download-bash.sh --verify-only
```
