# pr

Android app that runs Linux distributions via [proot](https://github.com/proot-me/proot) — no root, no Termux required.

## What it does

- Install and run Linux distributions on any Android device: the list offers Alpine, Debian, Fedora and Ubuntu, and the custom-image card installs any OCI image, with Manjaro, openSUSE and Rocky Linux one tap away under **Images** (see [Supported distributions](#supported-distributions))
- Native OCI container image support (`docker.io/library/...`, by tag or pinned as `name[:tag]@sha256:<digest>`): manifests and layers are checked against their digests, and the layer cache is verified before reuse
- Hard links in an image are stored once, the way proot's link2symlink stores them, instead of as copies: a fresh Ubuntu container takes about 130 MB instead of 1.2 GB
- Downloads cope with slow and flaky networks: a stall timeout instead of a limit on the whole transfer, retries with a growing delay, a renewed registry token for long pulls, and archives that enter the cache only when complete
- Background sessions: a session keeps running after its screen closes, for example a long-running agent or server, under a `specialUse` foreground service with an ongoing notification (End all, Keep CPU awake)
  - the notification counts the running sessions and names a session being ended as "Stopping <name>…" until it exits
  - Back asks End session / Keep running / Cancel; End sends SIGTERM to every process in the session, gives them up to 10 s to exit, then closes the shell and proot
  - "Keep CPU awake" holds a partial wake lock with the screen off; toggle it from the notification, or from the switch pinned above the list, shown while a session runs and background running is available
  - while Keep CPU awake is on and the app is not exempt from battery optimization, a banner offers the exemption, which guards against vendor battery savers
  - a session that ends on its own (not through End) while its screen is not visible posts a "session ended (exit N)" notification
  - the Android 12+ phantom-process limit and the Android 13+ Task Manager "Stop" can still end sessions (see [Session lifetime and Back](docs/android-apk-architecture.md#session-lifetime-and-back))
- Background installs: install, remove and test run under the same foreground service, holding a wake lock until they finish, with Cancel in the notification
- Install, remove and test containers from the list, with live output
  - Cancel stops `pr-cli` and everything it started; a cancelled install whose alias is known (every list install, and a custom install given an alias) removes its partial container, while a custom install without an alias leaves it for you to remove
  - Remove ends the container's running session first
  - an install or remove that is killed partway leaves the name marked as unfinished: the list shows it as "Incomplete" with a Remove button, and `pr-cli` will not log into it (see [Unfinished installs](docs/android-apk-architecture.md#unfinished-installs))
  - sessions and operations survive rotation and dark-mode changes
- Full package manager support: `apk`, `apt-get`
- Compile and run C, Rust, and other programs inside the guest
- targetSdk 35 — Play Store compatible

## The `:proot-engine` Library

The core execution environment has been decoupled into a standalone Android Library (`:proot-engine`). Consumer apps (like interactive coding environments, IDEs, or terminal apps) can now integrate a full Linux execution engine via Gradle composite builds without duplicating native binaries or Kotlin bridge code.

## Supported distributions

The container list offers Alpine (latest & edge), Debian (stable & testing), Fedora and Ubuntu. Manjaro, openSUSE and Rocky Linux are one tap away under **Images** on the custom-image card, which installs any OCI image reference. Every image in the list has an arm64 build; Arch Linux is not offered because its official image is published for amd64 only.

On the command line, `pr-cli install <alias>` installs the same images under the same names: `alpine`, `alpine-edge`, `debian`, `debian-testing`, `fedora`, `ubuntu`, `manjaro`, `opensuse` and `rockylinux`. A plugin file `etc/pr/<alias>.sh` takes precedence over a catalog name.

## How it works

**proot** uses Linux `ptrace()` to intercept syscalls and translate filesystem paths, creating a virtual root filesystem without actual root privileges.

The project consists of three layers:
- **Patched proot** (C) — Handles Android-specific seccomp filters, SELinux, and W^X restrictions.
- **pr-cli** (Rust) — High-performance, zero-copy CLI that replaces the original `proot-distro.sh` bash script. Supports native OCI extraction and robust quoting: `pr-cli login <distro> -- <args>` runs one argument as a shell command and several as a program with its arguments, in a clean environment.
- **Android APK / Library** (Kotlin + Compose) — The `:proot-engine` exposes a clean Kotlin API (`ProotHost`, `ProotLauncher`), while `:app` provides a Jetpack Compose terminal UI with soft-keyboard resize support, an app-wide session manager that keeps sessions running under a foreground service, and an app-scoped runner for install, remove and test.

### Android compatibility

Android enforces several restrictions on app processes:
- **W^X (Write-XOR-Execute)**: Prevents executing files in app-writable directories
- **SELinux**: Blocks certain filesystem operations
- **Zygote seccomp**: Blocks 18+ syscalls via BPF filter

Our engine handles all of these:
- SIGSYS handlers intercept blocked syscalls and emulate them in userspace
- The `PROOT_LOADER` mechanism uses `nativeLibraryDir` to bypass W^X
- Fake root (`--change-id=0:0`) makes `dpkg` and `apt-get` work without real root
- `CLONE_VM`/`CLONE_VFORK` stripping enables Rust's `cargo build` to work inside proot

## Building

### Prerequisites

- Android SDK with NDK r27c (auto-downloaded by build script)
- Rust toolchain with `aarch64-linux-android` target
- Java 17+

### Build steps

```bash
# 1. Build proot C binaries
scripts/build.sh --arch=arm64

# 2. Build test binary (guest-side)
(cd src/proot-integration-test && cargo build --target aarch64-linux-android --release)

# 3. Build pr-cli (host-side)
(cd src/pr-cli && cargo build --target aarch64-linux-android --release)

# 4. Copy binaries to the proot-engine library
cp build/out/arm64/proot android/proot-engine/src/main/jniLibs/arm64-v8a/libproot.so
cp build/out/arm64/loader android/proot-engine/src/main/jniLibs/arm64-v8a/libproot-loader.so
cp src/pr-cli/target/aarch64-linux-android/release/pr-cli \
   android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so

# 5. Build Android App and Engine AAR
(cd android && ./gradlew assembleDebug)
```

## Testing

CI runs the host-side unit suites (`cargo test` for pr-cli, `:app:testDebugUnitTest` and `:session-core:testDebugUnitTest`) on pushes to `main` and on pull requests, then builds the APK.

### Android unit tests (JVM)

```bash
cd android && ./gradlew :session-core:testDebugUnitTest :app:testDebugUnitTest
```

**199 tests in `:session-core`**: session lifetime (`SessionManager` with its sessions and tasks, `ManagedSession`, `ServiceState`, notifications and their actions), command sessions (`CommandSessionFactory`, `Bind`, end policy, signal wait), terminal binds, the factories' constructors as a Java host sees them, End's `/proc` scan through a fake process table (`ProotProcesses`), `PrCliCommand`, the bootstrap's links (`NativeLinks`) and `ProcessRunner`, which runs real `/bin/sh` processes and skips on hosts without `/proc`. **36 tests in `:app`**: the list's operations (`DistroOperations`, `InstalledContainers`). `./gradlew :proot-engine:testDebugUnitTest` runs the engine's 4 JVM tests.

### Host-side (pr-cli unit tests)

```bash
# pr-cli embeds the guest test binary, so build it first
(cd src/proot-integration-test && cargo build --target aarch64-linux-android --release)
(cd src/pr-cli && cargo test)
```

**324 tests**: 295 library unit tests, 15 in the binaries and 14 plugin-parser integration tests.

### Instrumented (requires connected Android device)

```bash
cd android && ./gradlew :proot-engine:connectedDebugAndroidTest
```

**8 tests** of the PTY bridge (`PtyNative`) and session reaping.

### On-device integration suite (requires connected Android device)

```bash
# Install Alpine or Debian via the app UI, then:
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test alpine
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test debian
```

In the app, **Test** in an installed container's menu runs the same suite. **57 tests** across 11 suites: distro, clone, readlink, gcc, rust, git, pipe, general, ssh, chroot, fakeroot; Test also runs readlink again under isolation, 66 results in all

## Project structure

```
src/proot/                  # Patched proot C source
src/pr-cli/                 # Rust CLI (install, login, OCI pull)
src/proot-integration-test/ # Guest-side test binary (TAP output)
android/proot-engine/       # Standalone Android Library AAR (Engine)
android/app/                # Android UI shell (Compose): container list, terminal, list operations
android/session-core/       # Android library: background sessions, command sessions, pr-cli runner, bootstrap
scripts/                    # Host-side build scripts
docs/                       # Technical documentation
```

## Documentation

| Document | Description |
|----------|-------------|
| [`docs/important-notes.md`](docs/important-notes.md) | Critical constraints, seccomp handlers, caveats — read first |
| [`docs/android-apk-architecture.md`](docs/android-apk-architecture.md) | Android app architecture; "Session lifetime and Back" covers sessions and list operations |
| [`docs/design/background-sessions.md`](docs/design/background-sessions.md) | Design of background sessions, End, Cancel and the operation runner |
| [`docs/design/proot-engine-library.md`](docs/design/proot-engine-library.md) | Architecture of the extracted `:proot-engine` library |
| [`docs/design/session-core.md`](docs/design/session-core.md) | Design of the `:session-core` library and command sessions |
| [`docs/proot-improvement.md`](docs/proot-improvement.md) | Our proot fork vs upstream and Termux (35 sections) |
| [`docs/targetsdk35-compatibility.md`](docs/targetsdk35-compatibility.md) | How targetSdk 35 works (PROOT_LOADER mechanism) |
| [`docs/rust-toolchain-support.md`](docs/rust-toolchain-support.md) | vfork/CLONE_VM fix, link2symlink readlink fix |
| [`docs/integration-tests.md`](docs/integration-tests.md) | Integration test suite (57 tests) |
| [`docs/plans/`](docs/plans/) | Historical implementation plans; the design docs and the code are authoritative |

## Credits

- [proot](https://github.com/proot-me/proot) — upstream proot v5.4.0 (GPL-2.0)
- [termux-proot](https://github.com/termux/termux-proot) — Termux's proot fork with Android patches (GPL-2.0)
- [proot-distro](https://github.com/termux/termux-packages/tree/master/packages/proot-distro) — distro plugins (GPL-3.0)
