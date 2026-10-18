# Android APK Architecture

Phase 4 builds the Android application that wraps the proot-distro infrastructure from Phases 1-3 into a standalone APK (`id.or.oo.pr`). It implements app bootstrapping (asset extraction, native lib setup), a Compose-based distro management UI, a terminal emulator using ConnectBot's termlib, and a PTY-based process execution bridge.

Status: **Complete** (commits `f71d547..f2eb29d`)

---

## Architecture

```
android/                                    # Android project root
├── build.gradle.kts                        # Root: AGP 8.7.3, Kotlin 2.1.0, Compose plugin
├── settings.gradle.kts                     # Includes :app and :termlib modules
├── app/
│   ├── build.gradle.kts                    # App module: Compose BOM 2024.12.01, CMake, termlib dep
│   └── src/main/
│       ├── AndroidManifest.xml             # Permissions, activities, extractNativeLibs
│       ├── cpp/
│       │   ├── CMakeLists.txt              # Builds libptyjni.so
│       │   └── ptyjni.c                    # PTY JNI: fork/exec with /dev/ptmx (183 lines)
│       ├── jniLibs/arm64-v8a/
│       │   └── libproot.so                 # Patched proot binary (2.5MB, from Phase 1)
│       ├── assets/
│       │   ├── bin/busybox                 # Static busybox (1.1MB, from Phase 3)
│       │   ├── bin/bash                    # Static bash (2.3MB, from Phase 3)
│       │   ├── scripts/proot-distro.sh     # With @APP_PREFIX@ template (3051 lines)
│       │   ├── scripts/bootstrap.sh        # POSIX sh setup (184 lines)
│       │   └── plugins/                    # 17 distro plugins
│       ├── java/id/or/oo/pr/
│       │   ├── App.kt                      # Application: bootstrap on first launch (188 lines)
│       │   ├── MainActivity.kt             # Compose: distro list, install/remove (270 lines)
│       │   ├── TerminalActivity.kt         # Compose: terminal emulator (115 lines)
│       │   ├── ProotLauncher.kt            # PTY session manager (132 lines)
│       │   └── PtyNative.kt                # JNI declarations for PTY (43 lines)
│       └── res/                            # Theme, icons, strings
├── termlib/
│   ├── build.gradle.kts                    # Thin wrapper: pulls source from vendor/termlib submodule
│   └── proguard-rules.pro                  # Empty (required by Android library plugin)
vendor/termlib/                             # Git submodule: ConnectBot terminal library
└── android/gradle/wrapper/                 # Gradle 8.11.1 wrapper
```

---

## Runtime Directory Layout

After first launch, App.kt creates this structure inside the app's private data directory:

```
/data/data/id.or.oo.pr/files/
├── usr/                                    # APP_PREFIX
│   ├── bin/
│   │   ├── busybox                         # From assets/bin/busybox (1.1MB)
│   │   ├── bash                            # From assets/bin/bash (2.3MB)
│   │   ├── proot                           # From jniLibs/libproot.so (2.5MB)
│   │   ├── bootstrap.sh                    # From assets/scripts/bootstrap.sh
│   │   ├── sh -> busybox                   # 311 applet symlinks created by bootstrap.sh
│   │   ├── awk -> busybox
│   │   ├── tar -> busybox
│   │   └── ... (308 more)
│   ├── scripts/
│   │   └── proot-distro.sh                 # From assets/scripts/proot-distro.sh
│   ├── etc/proot-distro/
│   │   ├── alpine.sh                       # 17 distro plugins from assets/plugins/
│   │   ├── debian.sh
│   │   └── ...
│   ├── var/lib/proot-distro/
│   │   ├── dlcache/                        # Download cache for rootfs tarballs
│   │   └── installed-rootfs/               # Extracted distro rootfs (populated on install)
│   │       └── alpine/
│   └── tmp/
└── home/                                   # APP_HOME
```

---

## Tasks

### T4.1 — Create Android project structure

Commit: `f71d547`

- Package: `id.or.oo.pr`
- Min SDK 28, Target SDK 35, compileSdk 36
- Language: Kotlin 2.1.0 with AGP 8.7.3, Gradle 8.11.1
- Permissions: `INTERNET`, `FOREGROUND_SERVICE`, `MANAGE_EXTERNAL_STORAGE`
- arm64-v8a only (`abiFilters`)
- AppCompat theme with dark action bar, adaptive icon (navy #1a1a2e + red triangle)
- Placeholder MainActivity with `activity_main.xml` layout
- Debug APK builds successfully (13MB before assets)

### T4.2 — Implement BootstrapService

Commit: `0bf00eb`

Implemented as `App.kt` (Application class, runs on every app launch):

- Checks `SharedPreferences` for `bootstrap_version` against `BOOTSTRAP_VERSION` constant
- If version mismatch or first run, performs full bootstrap:
  1. Creates directory structure: `bin/`, `etc/proot-distro/`, `scripts/`, `plugins/`, `home/`, `tmp/`
  2. Copies `assets/bin/busybox` → `files/usr/bin/busybox`, sets executable+readable
  3. Copies `assets/bin/bash` → `files/usr/bin/bash`, sets executable+readable
  4. Copies `jniLibs/libproot.so` (from `nativeLibraryDir`) → `files/usr/bin/proot`
  5. Copies `assets/scripts/bootstrap.sh` → `files/usr/bin/bootstrap.sh`
  6. Copies `assets/scripts/proot-distro.sh` → `files/usr/scripts/proot-distro.sh`
  7. Copies all `assets/plugins/*.sh` → `files/usr/etc/proot-distro/`
  8. Executes `/system/bin/sh bootstrap.sh` with environment:
     - `APP_PREFIX`, `APP_HOME`, `APP_PACKAGE`, `PATH`, `PROOT_NO_SECCOMP=1`, `HOME`
- `bootstrap.sh` (Phase 3) handles: `chmod 755` on binaries, 311 busybox applet symlinks, bash symlink override, shebang template `@APP_PREFIX@` replacement
- Idempotent: incrementing `BOOTSTRAP_VERSION` triggers re-bootstrap
- Fallback: tries alternate ABI split path if `nativeLibraryDir` doesn't contain `libproot.so`

### T4.3 — Implement MainActivity

Commit: `f2eb29d`

Compose-based distro management UI (270 lines):

- **Distro list**: reads plugin `.sh` files from `files/usr/etc/proot-distro/`, parses `DISTRO_NAME=` for display names, checks `files/usr/var/lib/proot-distro/installed-rootfs/<name>/` for install status
- **Install**: launches coroutine on `Dispatchers.IO`, runs `proot-distro install <name>` via ProotLauncher, streams live output to monospace text view
- **Login**: launches `TerminalActivity` with `distro` extra via `Intent`
- **Remove**: same as install but runs `proot-distro remove <name>`
- **UI**: Material3, `LazyColumn` with `Card` rows, `CircularProgressIndicator` during operations, download/play/delete icons
- **Coroutine safety**: all PTY I/O on `Dispatchers.IO`, state updates via `withContext(Dispatchers.Main)`

### T4.4 — Implement TerminalActivity

Commit: `f2eb29d`

Full terminal emulator using ConnectBot's termlib (115 lines):

- Creates `TerminalEmulator` via `TerminalEmulatorFactory.create()` with:
  - 24 rows x 80 cols initial size
  - Dark theme: background `#1a1a2e`, foreground `Color.White`, font `12sp`
  - `onKeyboardInput` callback: writes bytes to PTY master fd
  - `onResize` callback: forwards `TerminalDimensions` changes to `session.resize(rows, cols)` so apps inside proot (nano, htop, shell prompts) see the correct terminal size when the soft keyboard opens or the device rotates
- Starts PTY session via `ProotLauncher.startSession(distroName)`
- Spawns reader thread (`"pty-reader"`) that:
  - Reads 8KB chunks from PTY master fd in a loop
  - Feeds data to `emulator.writeInput(buf, 0, n)`
  - Exits on EOF or error
- Renders using termlib's `Terminal()` composable with `keyboardEnabled=true` and `Modifier.systemBarsPadding()` to prevent the first rows of output being hidden behind the camera notch and Android status bar
- Lifecycle: `onDestroy()` closes session and interrupts reader thread
- Back press: `OnBackPressedCallback` triggers cleanup and `finish()`. `dispatchKeyEvent` routes
  Back to it before the view tree, because the IME and Compose otherwise take the key-down; with
  the keyboard up, the first Back only hides it, and with a text selection active, Back clears the
  selection (through termlib's `SelectionController`) instead of leaving
- Manifest: `windowSoftInputMode=adjustResize` + `configChanges="orientation|screenSize|keyboard|keyboardHidden"` — prevents activity destruction and PTY detachment when the soft keyboard appears

### T4.5 — Implement ProotLauncher

Commit: `f2eb29d`

PTY-based process execution bridge with two layers:

**PtyNative.kt** (43 lines) — JNI declarations for `libptyjni.so`:
- `forkPty(cmd, args, envVars, rows, cols)` → returns PTY master fd
- `read(fd, buf, offset, length)` → reads from PTY
- `write(fd, buf, offset, length)` → writes to PTY
- `resize(fd, rows, cols)` → `ioctl(TIOCSWINSZ)`
- `close(fd)` → closes fd
- `getPid()` → returns last forked child pid
- `waitPid(pid)` → `waitpid(WNOHANG)`

**ptyjni.c** (183 lines) — C implementation:
- `nativeForkPty`: opens `/dev/ptmx`, calls `grantpt`/`unlockpt`, sets `TIOCSWINSZ`, `fork()`s, child does `setsid()` + opens slave pts + `dup2` to stdin/stdout/stderr + `setenv` for all env vars + `execv` the command; parent stores child pid and returns master fd
- `nativeRead`/`nativeWrite`: thin wrappers around `read()`/`write()` with `EAGAIN`/`EINTR` handling
- `nativeResize`: `ioctl(TIOCSWINSZ)` struct winsize
- `nativeClose`: `close(fd)`
- `nativeWaitPid`: `waitpid(WNOHANG)`, returns exit code or 0 if still running
- `nativeGetPid`: returns static `last_child_pid`

**ProotLauncher.kt** (132 lines) — Kotlin session manager:
- `startSession(distro, user, isolated, rows, cols)`: forks bash with `proot-distro.sh login <distro> --user <user> [--isolated]` args and full environment
- `startCustomSession(args: List<String>, rows, cols)`: forks directly with a pre-parsed argument list — avoids brittle `split(" ")` used by `runCommand()`. Use this when arguments contain spaces or quoted strings (e.g. install pipelines, custom login commands)
- `runCommand(command)`: forks `bash -c "source proot-distro.sh; proot-distro <command>"` for install/remove
- Environment variables set on every session:
  - `APP_PREFIX`, `APP_HOME`, `APP_PACKAGE` — app identity
  - `PATH` — includes app's `bin/` + `/system/bin:/system/xbin`
  - `PROOT_NO_SECCOMP=1` — required for Android 16 (Phase 1 finding)
  - `TERM=xterm-256color` — full color support
  - `HOME`, `LANG=en_US.UTF-8`
- `Session` inner class: wraps master fd with `read()`/`write()`/`resize()`/`close()`

### T4.6 — Bundle proot as native library

Commit: `0bf00eb`

- Proot binary placed at `app/src/main/jniLibs/arm64-v8a/libproot.so` (2.5MB)
- Android package manager extracts to `nativeLibraryDir` automatically
- `android:extractNativeLibs="true"` in manifest: ensures extraction (not loaded from APK)
- `useLegacyPackaging=true` in `packaging.jniLibs`: prevents compression for faster extraction
- App.kt copies from `nativeLibraryDir/libproot.so` to `files/usr/bin/proot` during bootstrap
- Named `libproot.so` to comply with Android's JNI library naming convention (even though it's not a shared library)

### T4.7 — Bundle assets

Commit: `0bf00eb`

All assets bundled in APK under `app/src/main/assets/`:

| Path | Size | Source |
|---|---|---|
| `bin/busybox` | 1.1MB | Alpine busybox-static v1.37.0-r14 (Phase 3, T3.1) |
| `bin/bash` | 2.3MB | robxu9/bash-static v5.2.015 (Phase 3, T3.2) |
| `scripts/proot-distro.sh` | 3051 lines | Standalone port (Phase 2, T2.1-T2.10) |
| `scripts/bootstrap.sh` | 184 lines | First-run setup (Phase 3, T3.3) |
| `plugins/*.sh` | 17 files | Distro plugins (Phase 2, T2.1) |

Total APK size: ~15MB (debug, uncompressed assets).

---

## Build Configuration

### Dependencies

| Dependency | Version | Purpose |
|---|---|---|
| AGP | 8.7.3 | Android build |
| Kotlin | 2.1.0 | Language |
| Compose BOM | 2024.12.01 | Compose version management |
| Material3 | (BOM) | UI components |
| activity-compose | 1.9.3 | Compose in activities |
| lifecycle-runtime-ktx | 2.8.7 | Lifecycle |
| coroutines | 1.9.0 | Async I/O |
| termlib | (local module) | Terminal emulator |
| CMake | 3.22.1 | Native build (ptyjni + libvterm) |

### Termlib Integration

ConnectBot's termlib (`vendor/termlib`) is integrated as a **thin Gradle wrapper module** (`android/termlib/`):

- The wrapper `build.gradle.kts` uses our AGP 8.7.3 + Kotlin 2.1.0 (termlib upstream uses AGP 9.1.0 + Kotlin 2.3.20)
- Sources pulled via `sourceSets["main"].java.srcDirs(file("../../vendor/termlib/lib/src/main/java"))`
- CMake path points to `vendor/termlib/lib/src/main/cpp/CMakeLists.txt` (builds libvterm + `jni_cb_term.so`)
- Namespace: `org.connectbot.terminal` (matches upstream)
- This approach keeps the submodule pristine while building with our compatible toolchain

---

## Key Design Decisions

### Why not embed Termux's terminal view?

termlib was chosen over Termux's terminal view or Rin's Rust engine because:
- No Rust toolchain required (uses existing NDK + CMake)
- Compose-native (`Terminal()` composable, not `AndroidView` wrapping a `View`)
- Clean separation: termlib handles display, we handle process execution
- ConnectBot's libvterm is battle-tested (15+ years, used by Neovim/Kitty)
- Apache 2.0 license

### Why a PTY JNI shim instead of ProcessBuilder?

`ProcessBuilder` on Android provides raw I/O streams, not a PTY. proot spawns interactive shells that require:
- Tab completion, arrow keys, color escape sequences
- Ctrl+C (SIGINT), Ctrl+Z (SIGTSTP)
- Terminal resize via `ioctl(TIOCSWINSZ)`
- Real-time character-by-character output

The 183-line `ptyjni.c` provides proper PTY via `/dev/ptmx` + `fork()` + `exec()`.

### Why run install/remove in PTY sessions?

`proot-distro.sh` uses colors, progress indicators, and interactive confirmation. Running these in a PTY session and capturing the output gives users a live terminal-like experience in the output view.

### Session lifetime and Back

Sessions live in `SessionManager` (`app/.../session/`), owned by `App`: one `ManagedSession` per
container holds the PTY, the termlib emulator, and a reader thread that keeps draining output with
no screen attached. The emulator runs on the main looper because termlib resizes on the caller's
thread and is not safe with its updates on another looper. `TerminalActivity` attaches to a session and
detaches when it closes; a session is shown by at most one screen. `SessionService`, a `specialUse`
foreground service, keeps the process alive while any session runs and owns the notification (End
all, Keep CPU awake). Design: `docs/design/background-sessions.md`.

| Action | Session |
|--------|---------|
| Back (after the keyboard and any selection) | Asks: End session / Keep running / Cancel |
| Home, switching apps, swiping the app away | Keeps running |
| `exit` in the shell, or the program ends | Ends; with no visible screen (none attached, or hidden by Home), a "session ended" notification is posted |
| Dark mode or theme change while the terminal is open | The recreated terminal reattaches to the same session, ended or not |
| End session, or End all in the notification | Ends: SIGTERM and SIGCONT to every process in the session except the login shell (including daemons that left the shell's tree), up to 10 s for them to exit (less once they have), then EOF, then the login shell is killed (proot's `--kill-on-exit` takes the rest); if proot must be killed, every tracee is SIGKILLed first, then proot. At worst about 17.5 s; Remove and "Ending the previous session…" wait up to 20 s for it |
| Android kills the process | Every session ends; nothing is restored; a terminal restored from recents starts a fresh session |

"Keep CPU awake", in the notification and in the container list, holds a partial wake lock (screen
off). A process with a foreground service keeps its wake lock and network in Doze; vendor battery
savers can still stop an app that is not exempt from battery optimization, so the container list
offers the exemption.

Two limits remain for a 24/7 session: Android 12+ kills app child processes beyond 32 system-wide
(the phantom process limit; the developer option "Disable child process restrictions" lifts it on
Android 14+, `settings_enable_monitor_phantom_procs` on 12L and 13, and `device_config` on 12), so a
gateway that spawns many processes can lose some; and Android 13+ Task Manager "Stop" in the
notification shade kills the process and every session.

---

## Follow-up Items

- **T5.1**: Verify distro plugin TARBALL_URLs are accessible and audit `distro_setup()` hooks
- **T5.2-T5.6**: Integration testing on device (Alpine, Debian, Ubuntu, backup/restore, isolated mode)
- **Build test**: APK has not been built with termlib integration yet — Gradle sync + `assembleDebug` needed to verify module resolution
- **Background sessions**: done (`docs/design/background-sessions.md`). Not covered: several sessions
  per container, start at boot, restoring sessions after the process is killed
- **Compose theme**: Currently uses default Material3 — should adopt app branding (#1a1a2e navy, #e94560 red)
- ~~**Terminal resize**: TerminalActivity does not yet observe Compose layout size changes to call `session.resize()` + `emulator.resize()` — terminal starts at 24x80 regardless of screen size~~ **Resolved**: `onResize` callback added to `TerminalEmulatorFactory.create()`; forwarded to `session.resize()`.

### OCI Container Paths vs Legacy Plugin Paths

Two installation methods produce rootfs in different locations:

| Method | Command | Rootfs location |
|--------|---------|-----------------|
| OCI reference | `pr-cli install docker.io/library/alpine:latest` | `var/lib/pr/containers/<alias>/rootfs/` |
| Legacy plugin | `proot-distro.sh install alpine` | `var/lib/pr/installed-rootfs/<distro>/` |

Code checking for distro existence (e.g. MainActivity's `isInstalled` check) must use the path corresponding to the installation method. The OCI path is used by `pr-cli`; the legacy path is used by the shell-script era (Phase 2). If both methods are in use, check both paths.
