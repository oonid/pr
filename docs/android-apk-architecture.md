# Android APK Architecture

Phase 4 builds the Android application that wraps the proot-distro infrastructure from Phases 1-3 into a standalone APK (`id.or.oo.pr`). It implements app bootstrapping (asset extraction, native lib setup), a Compose-based distro management UI, a terminal emulator using ConnectBot's termlib, and a PTY-based process execution bridge.

Status: **Complete** (commits `f71d547..f2eb29d`)

The T4.x sections below are the Phase 4 record; for current behaviour see "Session lifetime and Back" and "OCI Container Paths vs Legacy Plugin Paths".

---

## Architecture

```
android/                                    # Android project root
├── build.gradle.kts                        # Root: AGP 8.7.3, Kotlin 2.1.0, Compose plugin
├── settings.gradle.kts                     # Includes :app, :proot-engine, :session-core and :termlib modules
├── gradle.properties                       # Kotlin test fixtures on (for :session-core)
├── app/
│   ├── build.gradle.kts                    # App module: Compose BOM 2025.05.00, :proot-engine, :session-core and termlib deps
│   └── src/
│       ├── main/
│       │   ├── AndroidManifest.xml         # Permissions, activities, SessionService, extractNativeLibs
│       │   ├── java/id/or/oo/pr/
│       │   │   ├── App.kt                  # Application: ProotHost and SessionHost; runs PrBootstrap; owns SessionManager and DistroOperations
│       │   │   ├── MainActivity.kt         # Compose: container list, output sheet of DistroOperations
│       │   │   ├── SessionBanners.kt       # Keep CPU awake switch, battery and background-error banners
│       │   │   ├── InstalledContainers.kt  # Which aliases are installed (OCI rootfs or legacy dir)
│       │   │   ├── DistroOperations.kt     # App-scoped install/remove/test runner
│       │   │   └── TerminalActivity.kt     # Compose: a view onto a ManagedSession
│       │   └── res/                        # Theme, icons (ic_notification), strings
│       └── test/                           # JVM unit tests (:app:testDebugUnitTest)
├── session-core/                           # Android library: background sessions, pr-cli runner, bootstrap
│   ├── build.gradle.kts                    # api :proot-engine, :termlib, coroutines; test fixtures; unit-test coverage
│   └── src/
│       ├── main/assets/                    # bin/busybox.applets, scripts/bootstrap.sh (POSIX sh setup)
│       ├── main/java/id/or/oo/pr/session/  # SessionManager, ManagedSession, SessionService, SessionHost,
│       │                                   # CommandSessionFactory, PrCliCommand, ProcessRunner, PrBootstrap
│       ├── test/                           # JVM unit tests (:session-core:testDebugUnitTest)
│       └── testFixtures/                   # FakePty and Eventually (also used by :app's tests)
├── proot-engine/                           # Android library: ProotHost, ProotLauncher, PtyNative
│   ├── build.gradle.kts                    # CMake for libptyjni.so
│   └── src/
│       ├── main/cpp/                       # ptyjni.c: PTY fork/exec with /dev/ptmx
│       ├── main/jniLibs/arm64-v8a/         # libproot, libproot-loader, libpr-cli, libbusybox (.so)
│       ├── test/                           # JVM unit tests
│       └── androidTest/                    # Instrumented tests (PTY bridge, session reaping)
├── termlib/
│   ├── build.gradle.kts                    # Thin wrapper: pulls source from vendor/termlib submodule
│   └── proguard-rules.pro                  # Empty (required by Android library plugin)
└── gradle/wrapper/                         # Gradle 8.11.1 wrapper
vendor/termlib/                             # Git submodule: ConnectBot terminal library
```

`cpp/`, `jniLibs/`, `ProotLauncher.kt` and `PtyNative.kt` moved from `app/` to `proot-engine/`
(`docs/design/proot-engine-library.md`), and pr-cli replaced `proot-distro.sh`. `session/`,
`ProcessRunner.kt`, `PrCliCommand.kt`, the bootstrap in `App.kt` and the assets moved from `app/`
to `session-core/` (`docs/design/session-core.md`).

---

## The `:session-core` Library

`:session-core` holds what an app needs to run PRoot sessions in the background: `SessionManager`,
`ManagedSession`, `SessionService` and its notifications, End's `/proc` scan, `PrCliCommand` over
`ProcessRunner`, and `PrBootstrap`, which prepares `files/usr` (prefix directories, links to the
native libraries and busybox applets, `bootstrap.sh`). Everything is in package
`id.or.oo.pr.session`. Design: `docs/design/session-core.md`.

An app that uses it:

- implements `ProotHost` and `SessionHost` (`sessions`, `notificationIcon`, `openAppIntent()`) in
  its `Application`, and calls `PrBootstrap(this, this, version).run()` from `onCreate`;
- declares the service in its own manifest, with its own `specialUse` subtype text, and the
  permissions `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_SPECIAL_USE`, `WAKE_LOCK` and
  `POST_NOTIFICATIONS`:

  ```xml
  <service
      android:name="id.or.oo.pr.session.SessionService"
      android:exported="false"
      android:foregroundServiceType="specialUse">
      <property
          android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"
          android:value="..." />
  </service>
  ```

- includes the three modules from a pr checkout, the way rs.oo.or.id includes `:proot-engine`
  (`prRoot` found from its `prCandidates`), and depends on `project(":session-core")`; the module
  refers to the other two by these names:

  ```kotlin
  include(":termlib")
  project(":termlib").projectDir = java.io.File(prRoot, "android/termlib")
  include(":proot-engine")
  project(":proot-engine").projectDir = java.io.File(prRoot, "android/proot-engine")
  include(":session-core")
  project(":session-core").projectDir = java.io.File(prRoot, "android/session-core")
  ```

- sets up the build for those modules:
  - `android.experimental.enableTestFixturesKotlinSupport=true` in `gradle.properties`; running
    `test`, `check` or `build` across `:session-core` needs it, building the app alone does not;
  - `packaging.jniLibs.useLegacyPackaging = true` in the app and `android:extractNativeLibs="true"`
    in its manifest, because library packaging settings do not reach the app;
  - the Kotlin compose plugin declared in the root build, because `:termlib` applies it;
  - pr's `vendor/termlib` submodule initialised (`git clone --recursive`, or
    `git submodule update --init --recursive`);
  - the `INTERNET` permission, and `SessionService` declared in the app's manifest as above.

A command session's shell contract:

- the command runs as `/bin/sh -l -c '<command>'` with cwd `/`, HOME from passwd, and an
  environment built from PATH, `/etc/environment`, TERM, LANG and TMPDIR;
- set any working directory or environment inside the command, for example
  `cd /srv && exec env K=V server`;
- busybox `sh` execs the last simple command, so a lone server command replaces the shell.

A command session's sink contract:

- the sink is called on the reader thread with a reused 8 KiB buffer; it must copy what it keeps
  and never block;
- the bytes are PTY output: CRLF line ends, ANSI escapes, UTF-8 that may be split across reads,
  and a 24x80 window that `ManagedSession.resize` can change;
- a sink that throws loses that chunk, and the failure is logged once;
- the sink lives as long as the session, which can outlive every Activity, so it must be
  application-scoped (for example, a log buffer the `Application` owns), never tied to an
  Activity or a ViewModel.

End timing: after SIGTERM, End waits at most 10 s for the session to exit; it then sends EOF,
waits a 2 s grace and sends SIGKILL. A server that needs longer to drain is killed.

The notification actions are `<package>.action.END_ALL` and `<package>.action.TOGGLE_WAKE`.

Two kinds of session share one `SessionManager`, keyed by a session key. `open(key)` uses the
factory the manager was built with (pr's `ProotSessionFactory`: a terminal with a termlib
emulator, where the key is the container alias). `open(key, CommandSessionFactory(host, container,
command, sink, binds))` runs
`pr-cli login <container> --user root [--custom-bind host:guest]... -- <command>` with no
emulator, where `<command>` is ONE argument: pass the shell text as is, for example
`"cd /srv && exec server"`, and do not quote it again inside the Kotlin string. The key only
names the session, so `web-server` can run in container `debian` beside a terminal into the same
container. Its output goes to `sink`, which gets a reused buffer and must
copy what it keeps. Both kinds appear in the running notification and end with End and End all in
the same way, except that End follows an `EndPolicy`: a terminal spares its login shell
(`SPARE_LOGIN_SHELL`), a command session signals every tracee (`SIGNAL_EVERY_TRACEE`), so an
exec'd server gets SIGTERM. `open` returns `Failed` when the key is held by a running or retained
ended session of another kind. A same-kind `open` on a running or retained key returns the
existing session and ignores the new factory's command, binds and sink; to change any of them,
call `end(key)`, wait for the session's `exit`, then `open` again (a retained ended session holds
its key until a `detach` without `retainEnded`). A blank
command or a missing pr-cli fails early. Each bind's host path must exist: proot would only warn
and run the session without that bind, so `open` returns `Failed("... no bind source <path>")`
instead. `SessionManager.running` lists session keys, not
containers, so a host that removes or resets containers maps keys to containers itself.

A missing container does not fail `open`: it returns `Opened`, then the session ends with exit 1
and pr-cli's message that the distribution is not installed reaches the sink. Exit 1 can
also be any other pr-cli failure. `ManagedSession.endRequested` tells an End apart from an exit on
its own.

Notifications: `POST_NOTIFICATIONS` must also be requested at runtime on API 33 and later, or the
ended notifications are dropped. When the CPU is kept awake and the app is not exempt from battery
optimisation, the running notification says "Battery saver may stop sessions; allow in the app",
which assumes the app offers that exemption (`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`).

Reserved identities: the library uses the same names in every app, so an app must not use them for
anything else:

- the wake-lock tag `pr:sessions`;
- the preferences file `pr_prefs`, where `PrBootstrap` keeps its bootstrap state;
- notification id 1, the running notification, and the channel ids `sessions_running` and
  `sessions_ended`;
- the prefix paths `$prefix/scripts`, `$prefix/bin/bash`, `$prefix/bin/pr-test` and
  `$prefix/bin/proot-distro`, which `NativeLinks` deletes on every launch as names left by older
  pr builds.

---

## Runtime Directory Layout

After first launch, `PrBootstrap` (`:session-core`, run from `App.onCreate`) prepares this structure inside the app's private data directory:

```
/data/data/id.or.oo.pr/files/
├── usr/                                    # APP_PREFIX
│   ├── bin/
│   │   ├── busybox -> <nativeLibraryDir>/libbusybox.so
│   │   ├── proot   -> <nativeLibraryDir>/libproot.so
│   │   ├── pr-cli  -> <nativeLibraryDir>/libpr-cli.so
│   │   ├── sh -> busybox                   # busybox applet links, from assets/bin/busybox.applets
│   │   ├── awk -> busybox
│   │   ├── ...
│   │   └── bootstrap.sh                    # From assets/scripts/bootstrap.sh
│   ├── etc/pr/
│   ├── tmp/
│   └── var/lib/pr/
│       ├── installed-rootfs/
│       ├── dlcache/
│       └── containers/                     # Created by pr-cli
│           └── <alias>/rootfs/
└── home/                                   # APP_HOME
```

`NativeLinks` makes the three links into the native library directory on every launch, and
removes a stale `bin/bash`, `bin/pr-test` and `bin/proot-distro`, and the `usr/scripts`
directory, left by older builds. The busybox applet links and `bootstrap.sh` are made when the
bootstrap version changes. `bootstrap.sh` creates `etc/pr`, the two `var/lib/pr` directories,
`home` and `tmp`.

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

Current behaviour: the list is a fixed catalog (Alpine latest and edge, Debian stable and testing,
Fedora, Ubuntu) plus any other installed container; Arch Linux, Manjaro, openSUSE and Rocky Linux
are suggestions under **Images** on the custom-image card. Install, remove and test run `pr-cli`
through `DistroOperations` (see "Session lifetime and Back").

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
- Lifecycle: since background sessions, the PTY, emulator and reader thread live in
  `ManagedSession`; `onDestroy()` detaches from the session and never ends it (see "Session
  lifetime and Back")
- Back press: `OnBackPressedCallback` shows the End session / Keep running / Cancel prompt (before
  background sessions it ran cleanup and `finish()`). `dispatchKeyEvent` routes
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

All assets bundled in APK under `assets/` (then in `app/`; today the only assets are `bin/busybox.applets` and `scripts/bootstrap.sh`, in `session-core/src/main/assets/`):

| Path | Size | Source |
|---|---|---|
| `bin/busybox` | 1.1MB | Alpine busybox-static v1.37.0-r14 (Phase 3, T3.1) |
| `bin/bash` | 2.3MB | robxu9/bash-static v5.2.015 (Phase 3, T3.2) |
| `scripts/proot-distro.sh` | 3051 lines | Standalone port (Phase 2, T2.1-T2.10) |
| `scripts/bootstrap.sh` | 39 lines | First-run setup (Phase 3, T3.3) |
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

Current behaviour: install, remove and test run `pr-cli` as a plain child process
(`ProcessRunner`, standard input closed); `PrCliCommand` strips ANSI escapes from each output
line. Terminal sessions and command sessions use a PTY.

### Session lifetime and Back

Sessions live in `SessionManager` (`:session-core`, package `id.or.oo.pr.session`), owned by `App`: one `ManagedSession` per
session key holds the PTY, the termlib emulator (none for a command session), and a reader thread that keeps draining output with
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

Install, remove and test from the list run in `DistroOperations`, also owned by `App`: one
operation at a time, its output and state in a `StateFlow` that the list only renders, so rotating
or switching dark mode keeps the output sheet, the busy state and Remove's wait. Remove of a
container whose session runs shows "Stopping the <name> session…", ends the session and waits up
to 20 s for it; if it still runs, removal is cancelled with "Couldn't stop the <name> session;
removal cancelled." The list refreshes when an operation finishes. An operation does not survive
the process being killed: no foreground service covers it.

While an operation runs, the output sheet offers Cancel (Close is disabled, and Back, swipe-down
and a scrim tap leave the sheet up). Cancel stops `pr-cli` and every process it started (found
through `/proc`: `pr-cli` by its command line among the app's children, then its descendants by
parent links): SIGTERM, up to 2 s, then SIGKILL. The output ends with "Cancelled." During
Remove's wait for a session, Cancel stops the wait and nothing is removed; the session's End
continues. `pr-cli` creates the container directory first and has no SIGTERM handler, so a
cancelled install whose alias is known (every catalog install; a custom install with an alias),
whose container did not exist before the install started and exists after `pr-cli` stops runs
`pr-cli remove <alias>` in the same operation before "Cancelled.". If `pr-cli` had already exited
when Cancel arrived, its result stands and nothing is removed. A custom install without an alias
is not cleaned up: remove its partial container before installing it again. A custom install
with a plugin name (not an OCI image) and an alias writes `etc/pr/<alias>.override.sh` before
its rootfs, so a cancel in between can leave only that file, which `pr-cli remove` does not
delete and which blocks a later install of that alias ("already exists"); catalog and other OCI
installs never take that path. A cancel during `pr-cli remove` may leave a half-removed
container: run Remove again. A cancel or a network drop during a layer download leaves at most
`var/lib/pr/dlcache/sha256_<digest>.partial`, which the next download overwrites: `pr-cli`
renames a layer to its cache name only once its sha256 matches the digest, and hashes a cached
layer again before reusing it ("Cached OCI layer <digest> is damaged; downloading again..."). An
OCI install that fails on its own removes its container before it exits.

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
- ~~**Build test**: APK has not been built with termlib integration yet — Gradle sync + `assembleDebug` needed to verify module resolution~~ **Resolved**: CI builds the APK with termlib on pushes to `main` and on pull requests.
- **Background sessions**: done (`docs/design/background-sessions.md`). A host can now run a terminal
  and a command session in one container under different keys. Not covered: several terminal
  sessions per container, start at boot, restoring sessions after the process is killed
- **Compose theme**: Currently uses default Material3 — should adopt app branding (#1a1a2e navy, #e94560 red)
- ~~**Terminal resize**: TerminalActivity does not yet observe Compose layout size changes to call `session.resize()` + `emulator.resize()` — terminal starts at 24x80 regardless of screen size~~ **Resolved**: `onResize` callback added to `TerminalEmulatorFactory.create()`; forwarded to `session.resize()`.

### OCI Container Paths vs Legacy Plugin Paths

Two installation methods produce rootfs in different locations:

| Method | Command | Rootfs location |
|--------|---------|-----------------|
| OCI reference | `pr-cli install docker.io/library/alpine:latest` | `var/lib/pr/containers/<alias>/rootfs/` |
| Legacy plugin | `proot-distro.sh install alpine` | `var/lib/pr/installed-rootfs/<distro>/` |

Code checking for distro existence (e.g. MainActivity's `isInstalled` check) must use the path corresponding to the installation method. The OCI path is used by `pr-cli`; the legacy path is used by the shell-script era (Phase 2). If both methods are in use, check both paths.
