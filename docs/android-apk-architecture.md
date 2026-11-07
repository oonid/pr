# Android APK Architecture

Phase 4 builds the Android application that wraps the proot-distro infrastructure from Phases 1-3 into a standalone APK (`id.or.oo.pr`). It implements app bootstrapping (asset extraction, native lib setup), a Compose-based distro management UI, a terminal emulator using ConnectBot's termlib, and a PTY-based process execution bridge.

Status: **Complete** (commits `f71d547..f2eb29d`)

The T4.x sections below are the Phase 4 record; for current behaviour see "Session lifetime and Back" and "OCI Container Paths vs Legacy Plugin Paths".

---

## Architecture

```
android/                                    # Android project root
├── build.gradle.kts                        # Root: AGP 9.4.1, Kotlin 2.4.21, Compose plugin
├── settings.gradle.kts                     # Includes :app, :proot-engine, :session-core and :termlib modules
├── gradle.properties                       # AndroidX, Gradle JVM memory
├── app/
│   ├── build.gradle.kts                    # App module: Compose BOM 2026.09.00, :proot-engine, :session-core and termlib deps
│   └── src/
│       ├── main/
│       │   ├── AndroidManifest.xml         # Permissions, activities, SessionService, extractNativeLibs
│       │   ├── java/id/or/oo/pr/
│       │   │   ├── App.kt                  # Application: ProotHost and SessionHost; runs PrBootstrap; owns SessionManager and DistroOperations
│       │   │   ├── MainActivity.kt         # Compose: container list, output sheet of DistroOperations
│       │   │   ├── SessionBanners.kt       # Keep CPU awake switch; battery, notifications-off and background-error (Retry) banners
│       │   │   ├── NotificationAccess.kt   # When the notifications banner shows, and whether Allow asks again or opens settings
│       │   │   ├── InstalledContainers.kt  # Which aliases are installed (OCI rootfs or legacy dir, without an install marker)
│       │   │   ├── DistroOperations.kt     # App-scoped install/remove/test runner
│       │   │   └── TerminalActivity.kt     # Compose: a view onto a ManagedSession
│       │   └── res/                        # Theme, icons (ic_notification), strings
│       └── test/                           # JVM unit tests (:app:testDebugUnitTest)
├── session-core/                           # Android library: background sessions, pr-cli runner, bootstrap
│   ├── build.gradle.kts                    # api :proot-engine, :termlib, coroutines; test fixtures; unit-test coverage
│   └── src/
│       ├── main/java/id/or/oo/pr/session/  # SessionManager, ManagedSession, SessionService, SessionHost,
│       │                                   # CommandSessionFactory, PrCliCommand, ProcessRunner, PrBootstrap
│       ├── test/                           # JVM unit tests (:session-core:testDebugUnitTest)
│       └── testFixtures/                   # FakePty and Eventually (also used by :app's tests)
├── proot-engine/                           # Android library: ProotHost, ProotLauncher, PtyNative
│   ├── build.gradle.kts                    # CMake for libptyjni.so
│   └── src/
│       ├── main/cpp/                       # ptyjni.c: PTY fork/exec with /dev/ptmx (O_CLOEXEC, IUTF8 set)
│       ├── main/jniLibs/arm64-v8a/         # libproot, libproot-loader, libpr-cli (.so)
│       ├── test/                           # JVM unit tests
│       └── androidTest/                    # Instrumented tests (PTY bridge, session reaping)
├── termlib/
│   ├── build.gradle.kts                    # Thin wrapper: pulls source from vendor/termlib submodule
│   └── proguard-rules.pro                  # Empty (required by Android library plugin)
└── gradle/wrapper/                         # Gradle 9.8.2 wrapper
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
`ProcessRunner`, and `PrBootstrap`, which prepares `files/usr` (prefix directories and links to
the native libraries) without starting a shell. Everything is in package
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
  - AGP 9.4.1 or later with its built-in Kotlin and compileSdk 37, because the modules compile
    their Kotlin through AGP and no longer apply the Kotlin Android plugin;
  - `packaging.jniLibs.useLegacyPackaging = true` in the app and `android:extractNativeLibs="true"`
    in its manifest, because library packaging settings do not reach the app;
  - the Kotlin compose plugin declared in the root build, because `:termlib` applies it;
  - pr's `vendor/termlib` submodule initialised (`git clone --recursive`, or
    `git submodule update --init --recursive`);
  - the `INTERNET` permission, and `SessionService` declared in the app's manifest as above.

A command session's shell contract:

- the command runs as `/bin/sh -l -c '<command>'` with cwd `/`, HOME and USER from passwd, and
  a clean environment: PATH, the container's `/etc/environment`, TERM, COLORTERM, LANG, TMPDIR,
  the Android variables pr-cli has, and proot's own `PROOT_*` settings;
- set any working directory or environment inside the command, for example
  `cd /srv && exec env K=V server`;
- busybox `sh` (Alpine) execs the last simple command, so a lone server command replaces the
  shell; with dash or bash, start it with `exec`.

A command session's sink contract:

- the sink is called on the reader thread with a reused 8 KiB buffer; it must copy what it keeps
  and never block;
- the bytes are PTY output: CRLF line ends, ANSI escapes, UTF-8 that may be split across reads,
  and a 24x80 window that `ManagedSession.resize` can change;
- a sink that throws loses that chunk, and the failure is logged once;
- the sink lives as long as the session, which can outlive every Activity, so it must be
  application-scoped (for example, a log buffer the `Application` owns), never tied to an
  Activity or a ViewModel.

End timing: End runs four steps, each skipped once the session has exited:

1. SIGTERM, then the signal wait, at most 10 s by default; End waits only when its `/proc` scan
   finds processes to signal, and stops waiting once they have exited;
2. EOF, then a 2 s grace;
3. SIGKILL to proot's children (the login shell), then 2 s more;
4. the force kill of every tracee and proot, with the waits for an exit code (2 s, then 0.5 s
   after killing a shell that left none).

`ManagedSession.endWorstCaseMs(signalWaitMs)` adds these up, with 1 s for the `/proc` scans:
17.5 s with the default wait. A server that needs longer to drain sets its own wait:
`CommandSessionFactory(host, container, command, sink, binds, signalWaitMs = 65_000)` replaces
the 10 s for that session only; `signalWaitMs` must be between 1 and 3600000 (one hour). Any wait
a host does on `Ending`, `end(key)` or `endAll` for a session with its own `signalWaitMs` must use
`ManagedSession.endTimeoutMs(signalWaitMs)`, the worst case plus 2.5 s of headroom, not
`ManagedSession.END_TIMEOUT_MS`, which is `endTimeoutMs()` and covers only the default 10 s wait
(20 s).

The notification actions are `<package>.action.END_ALL` and `<package>.action.TOGGLE_WAKE`.

Two kinds of session share one `SessionManager`, keyed by a session key. `open(key)` uses the
factory the manager was built with (pr's `ProotSessionFactory`: a terminal with a termlib
emulator, where the key is the container alias). `ProotSessionFactory(host, binds)` adds the same
`--custom-bind host:guest` pairs to every terminal it opens, and `open` fails when a bind's host
path is missing. Binds apply when a terminal starts: an open session keeps the binds it started
with, and an `open` of its key through a factory with other binds returns that existing session.
`open(key, CommandSessionFactory(host, container, command, sink, binds))` runs
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
its key until its screen attaches again or detaches without `retainEnded`). A blank command or a missing pr-cli fails early. Each bind's host path must exist: proot would only warn
and run the session without that bind, so `open` returns `Failed("... no bind source <path>")`
instead. `SessionManager.running` lists session keys, not
containers, so a host that removes or resets containers maps keys to containers itself.

A missing container does not fail `open`: it returns `Opened`, then the session ends with exit 1
and pr-cli's message that the distribution is not installed reaches the sink. Exit 1 can
also be any other pr-cli failure. `ManagedSession.endRequested` tells an End apart from an exit on
its own.

Notifications: `POST_NOTIFICATIONS` must also be requested at runtime on API 33 and later, or the
ended notifications are dropped. The running notification counts the sessions that run ("1 session
running", "2 sessions running") and names them; a session whose End has started
(`SessionManager.ending`) is named apart until it exits, as in "alpine · Stopping web-server…",
and when every session is ending the title reads "Stopping 1 session…" or "Stopping N
sessions…". When the CPU is kept awake and the app is not exempt from battery
optimisation, the running notification says "Battery saver may stop sessions; allow in the app",
which assumes the app offers that exemption (`REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`).

Reserved identities: the library uses the same names in every app, so an app must not use them for
anything else:

- the wake-lock tag `pr:sessions`;
- the preferences file `pr_prefs`, where `PrBootstrap` keeps its bootstrap state;
- notification id 1, the running notification, and the channel ids `sessions_running` and
  `sessions_ended`;
- the prefix paths `$prefix/scripts`, `$prefix/bin/bash`, `$prefix/bin/pr-test`,
  `$prefix/bin/proot-distro`, `$prefix/bin/busybox`, `bootstrap.sh` and the old busybox applet
  links, which `NativeLinks` deletes on every launch as names left by older pr builds.

---

## Runtime Directory Layout

After first launch, `PrBootstrap` (`:session-core`, run from `App.onCreate`) prepares this structure inside the app's private data directory:

```
/data/data/id.or.oo.pr/files/
├── usr/                                    # APP_PREFIX
│   ├── bin/
│   │   ├── proot   -> <nativeLibraryDir>/libproot.so
│   │   └── pr-cli  -> <nativeLibraryDir>/libpr-cli.so
│   ├── etc/pr/
│   ├── tmp/
│   └── var/lib/pr/
│       ├── installed-rootfs/
│       ├── dlcache/
│       └── containers/                     # Created by pr-cli
│           └── <alias>/rootfs/
└── home/                                   # APP_HOME
```

`NativeLinks` makes the two links into the native library directory on every launch. It also
removes what older builds left: `bin/bash`, `bin/pr-test`, `bin/proot-distro`, `bin/busybox`,
`bin/bootstrap.sh`, every link in `bin` whose target is `busybox` (the old applet links), and the
`usr/scripts` directory. When the bootstrap version changes, `PrBootstrap` creates `tmp`, `etc/pr`,
the two `var/lib/pr` directories and `home` with `File.mkdirs()`; if one cannot be created, the
version is not recorded and the next launch tries again.

---

## Tasks

### T4.1 — Create Android project structure

Commit: `f71d547`

- Package: `id.or.oo.pr`
- Min SDK 28, Target SDK 35, compileSdk 36
- Language: Kotlin 2.1.0 with AGP 8.7.3, Gradle 8.11.1
- Permissions: `INTERNET`, `FOREGROUND_SERVICE` (no storage permission: the guest's `/sdcard` shows what Android's scoped storage lets the app see)
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
Fedora, Ubuntu) plus any other installed container; Manjaro, openSUSE and Rocky Linux
are suggestions under **Images** on the custom-image card. Install, remove and test run `pr-cli`
through `DistroOperations` (see "Session lifetime and Back").

### T4.4 — Implement TerminalActivity

Commit: `f2eb29d`

Full terminal emulator using ConnectBot's termlib (115 lines):

- Creates `TerminalEmulator` via `TerminalEmulatorFactory.create()` with:
  - 24 rows x 80 cols initial size
  - Dark theme: background `#1a1a2e`, foreground `Color.White`, font `12sp`
  - `onKeyboardInput` callback: queues the bytes on the session's writer thread (`pty-writer-<alias>`), which writes them to the PTY master fd in order, so a paste into a program that is not reading never blocks the main thread
  - `onResize` callback: forwards `TerminalDimensions` changes to `session.resize(rows, cols)` so apps inside proot (nano, htop, shell prompts) see the correct terminal size when the soft keyboard opens or the device rotates
- Starts PTY session via `ProotLauncher.startSession(distroName)`
- Spawns reader thread (`"pty-reader"`) that:
  - Reads 8KB chunks from PTY master fd in a loop
  - Feeds data to `emulator.writeInput(buf, 0, n)`
  - Exits on EOF or error
- Renders using termlib's `Terminal()` composable with `keyboardEnabled=true`, `Modifier.systemBarsPadding()` so the first rows are not hidden behind the camera notch and status bar, and `imePadding()` so the soft keyboard shrinks the terminal instead of covering its last rows (at targetSdk 35 the window is edge to edge and `adjustResize` no longer resizes it)
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

All assets bundled in APK under `assets/` (then in `app/`; the APK no longer ships any assets):

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
| AGP | 9.4.1 | Android build, with its built-in Kotlin support |
| Kotlin | 2.4.21 | Language |
| Gradle | 9.8.2 | Build tool (wrapper) |
| Compose BOM | 2026.09.00 | Compose version management |
| Material3 | (BOM) | UI components |
| activity-compose | 1.9.3 | Compose in activities |
| lifecycle-runtime-ktx | 2.8.7 | Lifecycle |
| coroutines | 1.9.0 | Async I/O |
| termlib | (local module) | Terminal emulator |
| CMake | 3.22.1 | Native build (ptyjni + libvterm) |

### Termlib Integration

ConnectBot's termlib (`vendor/termlib`) is integrated as a **thin Gradle wrapper module** (`android/termlib/`):

- The wrapper `build.gradle.kts` builds with our AGP 9.4.1 + Kotlin 2.4.21 and our Compose BOM
- Sources and resources pulled via `sourceSets { getByName("main") }`: `kotlin.directories` adds `vendor/termlib/lib/src/main/java`, `res.directories` adds `vendor/termlib/lib/src/main/res` (the selection menu strings)
- Pinned at termlib 0.3.14; it logs through `slf4j-api`, and with no SLF4J provider in the app those logs are dropped
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

`ptyjni.c` provides proper PTY via `/dev/ptmx` + `fork()` + `exec()`.

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
foreground service, keeps the process alive while any session or task runs and owns the notification (End
all, Keep CPU awake, and Cancel while a task runs). Design: `docs/design/background-sessions.md`.

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
removal cancelled." The list refreshes when an operation finishes.

Each operation holds a task in `SessionManager` (`beginTask`, `endTask`) for as long as it runs. A
task starts `SessionService` like a session does, so the process keeps running in the foreground
with the screen off or the app in the background. While a task runs the service holds a partial
wake lock, so a download continues with the screen off, and the notification names the task
("Installing debian:stable…") with a Cancel action that cancels the operation like the sheet's
Cancel. The task ends when the operation finishes, fails or is cancelled. If Android kills the
process anyway, the operation ends with it, and the install marker shows the name as "Incomplete".

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
| Catalog name | `pr-cli install debian` (the app's image for that name, see `catalog.rs`) | `var/lib/pr/containers/<alias>/rootfs/` |
| Plugin file | `pr-cli install <alias>` with `etc/pr/<alias>.sh` present | `var/lib/pr/installed-rootfs/<alias>/` |

Code checking for distro existence (`InstalledContainers` in the app) must check both paths: `pr-cli` uses the OCI path for images and catalog names, and the legacy path for plugin installs.

#### Unfinished installs

`pr-cli` claims a name before it creates the container's directory, by creating `.<alias>.pr-cli-installing` beside it (`var/lib/pr/containers/` for an OCI install, `var/lib/pr/installed-rootfs/` for a plugin install) and holding an exclusive `flock` on it. It removes the file as the last step of the install. The file sits outside the container's directory, so a backup or copy of the container never includes it, but a guest started with the app's directory bound in can still see it.

- A name with the file is not installed: `login`, `test`, `backup`, `copy`, `rename` and `reset` refuse it, and `list` shows it as `busy` or `interrupted`, even before its directory exists.
- An install refuses a name that has an unfinished install or removal of the other kind (OCI or plugin) until that one is removed.
- If the lock is free, the install or removal was killed. `install` takes the lock over, removes what was left and starts again; `remove` takes it over and deletes the leftovers.
- If the lock is held, another `pr-cli` is using the name, and `install`, `remove`, `rename` and `restore` refuse.
- `remove` takes the lock before it deletes anything, also for a container that is installed, and removes the file only once the directory is gone. A killed or cancelled remove leaves the name interrupted, and running `remove` again finishes it.
- `restore` takes the lock of every container in the archive before it moves any of them, and releases each one once its container is in place. It deletes the containers it replaced only after that, so a kill during the slow delete leaves no marker. It also clears a marker left by a killed install.

`InstalledContainers` skips a name with the file and reports it through `unfinished()`; the list shows such a name as "Incomplete" with a Remove button. `DistroOperations` removes a cancelled install only when its marker exists (`isUnfinished`). A cancelled install always leaves the file behind, and a cancel that lands after the install finished leaves the container alone. Another app that checks for installed containers itself should skip names with the file too. Containers installed before the marker existed have no file and count as installed.

#### Image references and digests

`pr-cli install` takes an OCI reference as `name[:tag]` or `name[:tag]@sha256:<64 hex>`. With a digest the tag is only a label and is dropped, and a digest that is not sha256 with 64 hex digits is refused before any request. A manifest fetched by digest is checked against it: the manifest named by a pinned reference, and the manifest picked from an index for the device's architecture, which must match the digest the index lists. A mismatch stops the install. Layers were already checked against their digests.

#### Downloads

Image layers and plugin rootfs archives use one HTTP client: 30 s to connect, and a 60 s limit on a stall (no data), not on the whole transfer, so a slow but steady download finishes. A dropped connection, a stall, HTTP 5xx or 429, or a layer that fails its digest is retried with a growing delay (layers: 4 attempts from 2 s; archives: 3 from 5 s); other 4xx answers stop at once. A layer request that gets 401 with a Bearer challenge fetches a fresh registry token once and keeps it for the following layers, since registry tokens expire within minutes. Both download to a `.partial` file and rename it into place only when complete, so a killed download never leaves a file that a later install takes as cached. Rootfs archives are cached as `<first 16 hex digits of sha256(url)>-<file name>`, so two URLs with the same file name do not share an entry; archives cached under the old name are no longer used and `pr-cli clear-cache` removes them. A rootfs archive may be a gzip, xz or plain tar; the format is read from its first bytes, as for image layers. When extraction fails, the cached archive is deleted only if no SHA256 vouched for it, so a verified archive is not downloaded again after a failure that was not its fault (no space left, for example).

#### Hard links in images

Android denies `link()` to apps. When `pr-cli` extracts an OCI layer or a plugin tarball in place, it stores each hard link the way proot's `--link2symlink` does: the data once in `<rootfs>/.l2s/.l2s.<name>NNNN.CCCC`, where `CCCC` counts the names, and every name a symlink to the `.l2s.<name>NNNN` symlink beside it. proot shows these to the guest as hard links (link count, one inode, `unlink` of one name), and `/proc/self/exe` names the link a program was run by. A fresh `ubuntu:latest`, whose coreutils is one multi-call binary hard-linked under each command name, takes about 130 MB instead of about 1.2 GB of copies. The symlinks hold absolute host paths, as the ones proot makes at runtime do. A symlink counts as part of a chain only when both of its links stay in that rootfs's `.l2s` directory, so an image cannot point the chain elsewhere. `restore` extracts into a staging directory first and still copies hard links.
