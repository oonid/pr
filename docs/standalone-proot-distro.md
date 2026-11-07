# Standalone proot-distro (Legacy Shell Script)

> Historical record of Phase 2. The shell script was replaced by pr-cli (Rust) and is no longer shipped.

Phase 2 ports the Termux proot-distro shell script to work as a standalone program on Android without any Termux dependency. The script is modified to use static busybox for utilities, static bash for execution, and runtime environment variables instead of build-time templates.

Status: **Complete** (commits `56a0264..9b856ab`)

---

## Architecture

```
vendor/termux-proot-distro/     # original Termux script (pristine, read-only)
src/scripts/proot-distro.sh     # our ported version (3049 lines, down from 3176)
src/scripts/plugins/            # 17 distro plugins (unchanged from termux originals)
build/test-setup.sh             # on-device test environment setup script
build/test-push.sh              # convenience script: push + setup + test via adb
```

The script runs on-device as:

```
${APP_PREFIX}/bin/bash proot-distro.sh <command>
```

where `${APP_PREFIX}` defaults to `/data/data/id.or.oo.pr/files/usr`. In the final APK, the BootstrapService will set `APP_PREFIX`, `APP_HOME`, `APP_PACKAGE` environment variables before launching the script.

---

## What Was Done

### T2.1 — Fork proot-distro.sh and plugins

- Copied `proot-distro.sh` (3172 lines) from `vendor/termux-proot-distro/`
- Copied 17 distro plugins to `src/scripts/plugins/`:
  alpine, almalinux, archlinux, artix, adelie, chimera, debian, deepin, fedora, manjaro, opensuse, oracle, pardus, rockylinux, trisquel, ubuntu, void
- Excluded `termux.sh` (Termux-as-a-distro plugin)
- Shebang set to `#!@APP_PREFIX@/bin/bash` (template, replaced at APK install time)

**Decision**: Bundle a static bash binary (~2MB). proot-distro.sh requires bash features (associative arrays via `declare -A`, `[[ ]]` tests, `mapfile`, process substitution `<()`, arithmetic `(( ))`) that busybox ash does not support.

### T2.2 — Replace template variables

Replaced all `@TERMUX_*@` build-time templates with runtime shell variables:

| Template | Replacement | Default |
|---|---|---|
| `@TERMUX_PREFIX@` | `${APP_PREFIX}` | `/data/data/id.or.oo.pr/files/usr` |
| `@TERMUX_HOME@` | `${APP_HOME}` | `/data/data/id.or.oo.pr/files/home` |
| `@TERMUX_APP_PACKAGE@` | `${APP_PACKAGE}` | `id.or.oo.pr` |

21 + 5 + 4 = 30 total replacements. Added defaults at top of script (lines 39-41):

```bash
APP_PREFIX="${APP_PREFIX:-/data/data/id.or.oo.pr/files/usr}"
APP_HOME="${APP_HOME:-/data/data/id.or.oo.pr/files/home}"
APP_PACKAGE="${APP_PACKAGE:-id.or.oo.pr}"
```

Removed `TERMUX_LDPRELOAD` save/restore (not needed without Termux). Verified zero `@TERMUX_` references remain in script and all plugins.

### T2.3 — Remove Termux-specific code

**219 lines removed** (3176 → 2957 lines). All `DISTRO_TYPE="termux"` code paths eliminated:

1. **DISTRO_TYPE="termux" code** — removed ZIP extraction, SYMLINKS.txt processing, Termux bootstrap env setup
2. **LD_PRELOAD management** — removed `TERMUX_LDPRELOAD` save/unset/restore (5 occurrences)
3. **--termux-home / --shared-tmp options** — removed parsing and bind mount logic
4. **GNU version checks** — removed bash version check, tar version check (3 occurrences)
5. **dpkg/lscpu checks** — removed architecture validation, replaced lscpu with `/proc/cpuinfo`
6. **Termux-specific help text** — removed option docs, distro notes, utility references
7. **Minor fixes** — GECOS "Termux" → "proot-distro", `/proc/version` "proot@termux" → "proot@pr"

Remaining Termux references are attribution only (header comments, version display).

### T2.4 — Adapt download mechanism

Created `download_file()` function (lines 268-307) replacing inline curl:

- **curl first, wget fallback** — auto-detects which tool is available at runtime
- **3 retries** with exponential backoff (5s → 10s → 20s, capped at 60s)
- **curl flags**: `--disable --fail --retry 0 --location --connect-timeout 15 --max-time 600`
- **wget flags**: `-T 30 -q` (busybox-compatible)
- **Validation**: checks file exists and is non-empty after download
- **Cleanup**: removes partial file on failure

curl is no longer required. The dependency check (T2.3 already removed it) does not list curl. Devices with only busybox wget will work.

### T2.5 — Adapt dependency check

Updated startup utility check to match actual needs and busybox capabilities:

**Removed**: bzip2 (unused), curl (optional via T2.4 fallback)

**Added**: realpath (8 uses), stat (9 uses), sha256sum (integrity check), wget (download fallback)

**Final list** (23 utilities):
```
awk basename cat chmod cp cut du file find grep gzip head id mkdir
proot realpath rm sed sha256sum stat tar wget xargs
```

All are busybox applets except `proot` (our binary). The proot check looks for `${APP_PREFIX}/bin/proot` specifically.

### T2.6 — Adapt CPU detection

`detect_cpu_arch()` uses `file -L` to identify installed rootfs architecture. The original `cut` pipeline was fragile and broke with busybox `file`:

**Old** (GNU-specific):
```
file -L ... | cut -d':' -f2- | cut -d',' -f2 | cut -d' ' -f2-
```

**New** (busybox-compatible):
```
file -L ... | grep -oE '(ARM aarch64|ARM|UCB RISC-V|Intel 80386|x86-64|MIPS)'
```

Also added MIPS architecture mapping (was missing).

### T2.7 — Add self_initialize()

`self_initialize()` (lines ~139-182) runs once at script startup:

**Validates**:
1. proot binary exists and is executable in PATH
2. `DISTRO_PLUGINS_DIR` directory exists
3. At least one `*.sh` plugin is present

**Creates directories**:
- `${APP_PREFIX}/var/lib/proot-distro`
- `${APP_PREFIX}/var/lib/proot-distro/installed-rootfs`
- `${APP_PREFIX}/var/lib/proot-distro/dlcache`

In the APK context, BootstrapService handles setup. This function is a safety net for development testing.

### T2.8 — Fake kernel version

Changed `DEFAULT_FAKE_KERNEL_RELEASE` from `6.17.0-PRoot-Distro` to `6.17.0-pr`.

This appears in:
- `uname -r` output inside proot sessions
- Fake `/proc/version` file
- proot `--kernel-release` construction

### T2.9 — Default PATH

Removed `/system/bin` and `/system/xbin` from `DEFAULT_PATH_ENV`. Android system binaries should not be in the distro's PATH.

**Final**:
```
/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/games:/usr/games:${APP_PREFIX}/bin
```

This PATH is written into installed distro files: `/etc/environment`, `/etc/profile`, `/etc/bash.bashrc`, `/etc/login.defs`.

### T2.10 — On-device testing and busybox fixes

Tested on Samsung Android 16 (aarch64, non-rooted). Discovered 7 categories of GNU-specific command usage incompatible with static busybox:

| GNU usage | Busybox fix | Count |
|---|---|---|
| `grep -P` / `grep -qP` (PCRE) | `grep -E` / `grep -qE` (ERE) | 7 |
| `stat --format='%a'` | `stat -c '%a'` | 2 |
| `realpath -qe` | `realpath -e` + stderr redirect | 1 |
| `realpath -m` | `realpath` (no `-m` flag) | 4 |
| `paste <(...) <(...)` | Pure bash array loop | 1 |
| GNU tar: `--warning`, `--delay-directory-restore`, `--preserve-permissions` | Removed (busybox tar lacks these) | 1 |
| `\|&` (bash pipe-stderr shorthand) | `2>&1 \|` | 1 |

All PCRE patterns were simple enough to express as POSIX ERE. No functional behavior changed.

**Test results** (device: Samsung, Android 16, SDK 36, aarch64, non-rooted):

| Command | Result |
|---|---|
| `proot-distro list` | Lists 17 distributions |
| `proot-distro install alpine` | Downloads + extracts Alpine 3.23 rootfs (with cached tarball) |
| `proot-distro login alpine` | Logs in as root, `uname -a` shows `6.17.0-pr` |
| `proot-distro remove alpine` | Cleans up rootfs directory |

**Test binaries used**:
- `busybox-static` v1.37.0 from Alpine package — 1.1MB, static aarch64, no PT_TLS segment (no alignment fix needed)
- `bash-static` v5.2.015 from `robxu9/bash-static` — 2.3MB, static aarch64, no PT_TLS segment
- `proot` v5.4.0-pr — built from `src/proot/` with TLS alignment fix

---

## Patches Applied on Top of termux-proot-distro

| Area | Change | Reason |
|---|---|---|
| Template variables | `@TERMUX_*@` → `${APP_*}` with defaults | Runtime configuration instead of build-time |
| Termux code paths | Removed 219 lines of DISTRO_TYPE="termux" code | Not applicable to standalone Android |
| Download | New `download_file()` with retry/fallback | curl not guaranteed on Android; wget fallback needed |
| Dependency check | 23 utilities (was 26, removed curl/bzip2/lscpu, added realpath/stat/sha256sum/wget) | Match busybox capabilities |
| CPU detection | `grep -oE` regex replaces `cut` pipeline | busybox `file` has different output format |
| Self-init | New `self_initialize()` function | Safety net for missing dirs/bins |
| Fake kernel | `6.17.0-pr` (was `6.17.0-PRoot-Distro`) | Project identity |
| Default PATH | Removed `/system/bin:/system/xbin` | Android system paths should not be in distro PATH |
| grep | `-P`/`-qP` → `-E`/`-qE` (7 occurrences) | busybox grep lacks PCRE |
| stat | `--format=` → `-c` (2 occurrences) | busybox stat uses `-c` not `--format=` |
| realpath | `-qe` → `-e`, `-m` removed (5 occurrences) | busybox realpath lacks `-q` and `-m` flags |
| tar | Removed `--warning`, `--delay-directory-restore`, `--preserve-permissions` | busybox tar lacks these GNU options |
| paste | Replaced with bash array loop | paste not in Alpine's busybox-static |
| Pipe stderr | `\|&` → `2>&1 \|` | POSIX-compatible |

---

## Remaining Issues and Follow-ups for Phase 3+

### 1. Shebang template: `#!@APP_PREFIX@/bin/bash`

**Current state**: Line 1 is `#!@APP_PREFIX@/bin/bash` — a literal template string.

**Problem**: The kernel passes this directly to execve. If `@APP_PREFIX@` isn't replaced, the script won't execute as `./proot-distro`. For on-device testing, the workaround is `bash /path/to/proot-distro <command>`.

**Phase 3/4 fix**: The APK's BootstrapService must replace `@APP_PREFIX@` in the shebang with the actual prefix path after copying the script to `${APP_PREFIX}/bin/proot-distro`. This is a simple `sed -i "s|@APP_PREFIX@|${APP_PREFIX}|g"` during first-run setup.

**Important**: The template must be replaced in the shebang line specifically. All other `@APP_PREFIX@` references were already replaced with `${APP_PREFIX}` runtime variables in T2.2. The shebang is the ONLY remaining template because the kernel reads it before bash starts.

### 2. `mapfile` is bash-only

**Current state**: Line 1829 uses `mapfile -t -O` to read environment variables from `/etc/environment`.

**Impact**: This requires bash — it will NOT work with any other shell. This is fine since we're bundling static bash, but it's worth noting for anyone attempting to port to a different shell.

**No action needed** — we've committed to bash.

### 3. Network/DNS resolution on device

**Finding**: During testing, the device could not resolve `easycli.sh` (the mirror URL in distro plugins). The host machine resolved it fine. The rootfs tarball had to be pre-downloaded on the host and pushed to the device's cache directory.

**Possible causes**: Android DNS resolver may block certain domains; or the device's current network had restricted DNS.

**Phase 3/6 follow-up**:
- Test DNS resolution from within the app context (not just adb shell)
- Consider adding a fallback mirror at `pr.oo.or.id/dl/rootfs/` (T6.1)
- Test with `wget` from within the APK's process (different network context than adb shell)
- Consider bundling IP addresses or alternative mirrors

### 4. Android UID/GID registration warnings

**Current state**: During `proot-distro install`, the script tries to register Android-specific groups (via `id -Gn` / `id -G`). On non-rooted devices, many Android group IDs are unknown to the `id` command:

```
id: unknown ID 2000    # shell
id: unknown ID 1004    # input
id: unknown ID 1007    # log
...
```

**Impact**: Non-fatal. The groups that CAN be resolved are registered correctly. Unknown IDs are skipped (the `while` loop / `for` loop continues).

**Phase 3 follow-up**: Consider pre-populating `/etc/group` with known Android AID mappings instead of relying on `id -Gn`/`id -G` which may fail on non-rooted devices. Android defines these in `android_filesystem_config.h`:

```
AID_SDCARD_R(1028), AID_MEDIA_RW(1023), AID_PACKAGE_INFO(1007)...
```

### 5. `/data/data/${APP_PACKAGE}/cache` warning on login

**Current state**: When running via adb shell with `APP_PACKAGE=id.or.oo.pr`, the login function tries to bind `/data/data/id.or.oo.pr/cache` which doesn't exist:

```
proot warning: can't sanitize binding "/data/data/id.or.oo.pr/cache": No such file or directory
```

**Impact**: Non-fatal warning, login still works.

**Phase 4 fix**: The APK's BootstrapService must create the cache directory during initialization. Also, when running via adb shell for testing, create it manually:

```bash
mkdir -p /data/data/id.or.oo.pr/cache  # needs root or app context
```

Or set `APP_PACKAGE` to something that already exists for testing.

### 6. `--isolated` mode, backup/restore not tested

**Current state**: T2.10 tested `list`, `install`, `login`, `remove`. The following were NOT tested:

- `proot-distro login alpine --isolated` — should work, but not verified
- `proot-distro backup alpine` — creates a tarball of the rootfs
- `proot-distro restore alpine` — restores from backup tarball
- `proot-distro rename alpine my-alpine` — renames a distro
- `proot-distro copy alpine my-alpine` — clones a distro

**Phase 5 follow-up**: Integration tests for these commands (T5.5, T5.6).

### 7. tar format compatibility

**Current state**: Removed GNU tar options (`--warning`, `--delay-directory-restore`, `--preserve-permissions`) from rootfs extraction. This works for Alpine's `.tar.xz` rootfs.

**Potential issue**: Some distro rootfs tarballs may rely on these GNU features:
- `--delay-directory-restore`: Sets directory permissions after extraction (prevents "Permission denied" when extracting into a read-only dir). Without it, extraction order matters.
- `--preserve-permissions`: Preserves exact permissions from tarball. Without it, umask may apply.

**Phase 3 follow-up** (T3.4): Test extraction of all distro tarballs (`.tar.xz`, `.tar.gz`, `.tar.bz2`) with busybox tar to verify no issues.

### 8. `file` command output format

**Current state**: `detect_cpu_arch()` uses `grep -oE` regex which handles both GNU `file` and busybox `file` output formats. However, busybox `file` has limited ELF detection capabilities — it may report generic "ELF 64-bit LSB executable, ARM aarch64" without detailed build info.

**Phase 3 follow-up** (T3.5): Verify that busybox `file` on aarch64 correctly identifies all architecture types used by distro plugins (aarch64, arm, x86_64, i686, riscv64).

### 9. curl availability

**Current state**: `download_file()` tries curl first, falls back to wget. In the APK context, only busybox wget will be available (no curl bundled). This is fine, but curl has better HTTPS/TLS handling and retry behavior.

**Phase 3 decision**: We will NOT bundle curl (too large, wget is sufficient). But if users install curl inside a distro and want to use it for downloads, the script already supports it.

### 10. Busybox-static source

**Current state**: Using Alpine's `busybox-static` package (v1.37.0, 1.1MB). It's a standard Alpine package downloaded from `dl-cdn.alpinelinux.org`.

**Phase 3 decision** (T3.1): Two options:
- **Option A**: Download pre-built Alpine busybox-static (tested, works, 1.1MB) — but may change between Alpine versions
- **Option B**: Build busybox from source with NDK (full control over applets and features) — more complex build process

The pre-built binary works well and includes all needed applets. For the APK, we should pin a specific version and host it ourselves or bundle it as an asset.

### 11. Static bash source

**Current state**: Using `robxu9/bash-static` v5.2.015 (2.3MB) from GitHub releases. This is a third-party build.

**Phase 3 decision** (T3.2): Two options:
- **Option A**: Use pre-built bash-static from a trusted source (current approach) — depends on third-party
- **Option B**: Cross-compile bash from source with NDK — guarantees reproducibility

For the APK release, building from source is preferred for security auditability. The bash build is relatively straightforward (configure + make with NDK toolchain).

---

## Environment Variables Reference

The following environment variables control proot-distro.sh behavior:

| Variable | Default | Set by | Purpose |
|---|---|---|---|
| `APP_PREFIX` | `/data/data/id.or.oo.pr/files/usr` | BootstrapService | Base installation directory |
| `APP_HOME` | `/data/data/id.or.oo.pr/files/home` | BootstrapService | User home directory |
| `APP_PACKAGE` | `id.or.oo.pr` | BootstrapService | Android package name |
| `PROOT_NO_SECCOMP` | (unset) | Launcher (MUST set to `1`) | Disables seccomp filter (required on Android 14+) |
| `PATH` | `${APP_PREFIX}/bin` | Script itself (line 43) | Binary search path |
| `TMPDIR` | (system default) | Should be set by launcher | proot extracts loader here; must be writable+executable |

---

## Script Line Reference

Key functions and locations in `src/scripts/proot-distro.sh` (3049 lines):

| Lines | Content |
|---|---|
| 1 | Shebang: `#!@APP_PREFIX@/bin/bash` (template, needs Phase 4 replacement) |
| 28 | `PROGRAM_VERSION="4.38.0"` |
| 39-41 | `APP_PREFIX`, `APP_HOME`, `APP_PACKAGE` defaults |
| 43 | `export PATH="${APP_PREFIX}/bin"` |
| 60-70 | Path constants (`RUNTIME_DIR`, `DISTRO_PLUGINS_DIR`, etc.) |
| 70 | `DEFAULT_FAKE_KERNEL_RELEASE="6.17.0-pr"` |
| 65-66 | `DEFAULT_PATH_ENV` (no `/system/bin`) |
| 100-140 | Dependency check (23 utilities) |
| 139-182 | `self_initialize()` |
| 184-206 | `detect_cpu_arch()` with `grep -oE` regex |
| 268-307 | `download_file()` with retry/fallback |
| 349-750 | `command_install()` |
| 567-571 | Rootfs tar extraction (busybox-compatible) |
| 651-666 | Android UID/GID registration (bash array loop, no paste) |
| 900-1200 | `command_login()` |
| 1827-1835 | `/etc/environment` parsing (mapfile, bash-only) |
| 1967-1969 | `/proc/self/fd` binding (realpath -e) |
| 2017-2018 | `/data/app` permission check (stat -c) |
| 2079-2080 | `/system` mount permission check (stat -c) |
| 2987-2989 | Plugin loading: `declare -A` associative arrays |

---

## Test Infrastructure

### build/test-setup.sh

On-device setup script that creates the test environment at `/data/local/tmp/pr-test/`. Pushes and configures:

- `busybox.static` → `${PREFIX}/bin/busybox` with applet symlinks (35 applets)
- `bash-static` → `${PREFIX}/bin/bash`
- `proot` → `${PREFIX}/bin/proot`
- `proot-distro.sh` → `${PREFIX}/bin/proot-distro`
- 17 plugin `.sh` files → `${PREFIX}/etc/proot-distro/`

Run: `adb push build/test-setup.sh /data/local/tmp/ && adb shell sh /data/local/tmp/test-setup.sh`

### build/test-push.sh

Convenience script for the full push-and-test cycle:

```
./build/test-push.sh           # Push all files only
./build/test-push.sh setup     # Push + run setup
./build/test-push.sh test      # Push + setup + proot-distro list
./build/test-push.sh shell     # Push + setup + interactive bash shell
```

### Manual test session

```bash
adb shell
export PATH=/data/local/tmp/pr-test/usr/bin
export APP_PREFIX=/data/local/tmp/pr-test/usr
export APP_HOME=/data/local/tmp/pr-test/home
export APP_PACKAGE=id.or.oo.pr
export PROOT_NO_SECCOMP=1

# Run commands via bash (shebang template not yet replaced)
bash proot-distro list
bash proot-distro install alpine
bash proot-distro login alpine
bash proot-distro remove alpine
```

---

## Reproducing the Test

```bash
# 1. Build proot
scripts/build.sh --arch=arm64

# 2. Get test binaries
# busybox-static (from Alpine repo):
curl -fSL -o build/test-binaries/busybox-static.apk \
    https://dl-cdn.alpinelinux.org/alpine/v3.21/main/aarch64/busybox-static-1.37.0-r13.apk
mkdir -p build/test-binaries/extracted
tar xzf build/test-binaries/busybox-static.apk -C build/test-binaries/extracted/

# bash-static (from GitHub):
curl -fSL -o build/test-binaries/bash-static \
    https://github.com/robxu9/bash-static/releases/download/5.2.015-1.2.3-2/bash-linux-aarch64

# 3. Push and test
./build/test-push.sh test

# 4. For download testing, pre-cache the rootfs on host (device DNS may not resolve easycli.sh):
curl -fSL -o build/test-binaries/alpine-aarch64-pd-v4.37.0.tar.xz \
    https://easycli.sh/proot-distro/alpine-aarch64-pd-v4.37.0.tar.xz
adb push build/test-binaries/alpine-aarch64-pd-v4.37.0.tar.xz \
    /data/local/tmp/pr-test/usr/var/lib/proot-distro/dlcache/

# 5. Install and login
adb shell "export PATH=/data/local/tmp/pr-test/usr/bin APP_PREFIX=/data/local/tmp/pr-test/usr \
    APP_HOME=/data/local/tmp/pr-test/home APP_PACKAGE=id.or.oo.pr PROOT_NO_SECCOMP=1 && \
    bash proot-distro install alpine && echo 'whoami && uname -a && exit' | bash proot-distro login alpine"
```
