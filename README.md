# pr

Android app that runs Linux distributions via [proot](https://github.com/proot-me/proot) — no root, no Termux required.

## What it does

- Install and run Linux distributions (Alpine, Debian, Ubuntu, and 5 more) on any Android device
- Native OCI container image support (`docker.io/library/...`)
- Full package manager support: `apk`, `apt-get`
- Compile and run C, Rust, and other programs inside the guest
- targetSdk 35 — Play Store compatible

## The `:proot-engine` Library

The core execution environment has been decoupled into a standalone Android Library (`:proot-engine`). Consumer apps (like interactive coding environments, IDEs, or terminal apps) can now integrate a full Linux execution engine via Gradle composite builds without duplicating native binaries or Kotlin bridge code.

## Supported distributions

Alpine (latest & edge), Debian (stable & testing), Ubuntu, Arch Linux, Fedora, OpenSUSE, Manjaro, Rocky Linux

## How it works

**proot** uses Linux `ptrace()` to intercept syscalls and translate filesystem paths, creating a virtual root filesystem without actual root privileges.

The project consists of three layers:
- **Patched proot** (C) — Handles Android-specific seccomp filters, SELinux, and W^X restrictions.
- **pr-cli** (Rust) — High-performance, zero-copy CLI that replaces the original `proot-distro.sh` bash script. Supports native OCI extraction and robust quoting.
- **Android APK / Library** (Kotlin + Compose) — The `:proot-engine` exposes a clean Kotlin API (`ProotHost`, `ProotLauncher`), while `:app` provides a Jetpack Compose terminal UI with soft-keyboard resize support.

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
cd src/proot-integration-test && cargo build --target aarch64-linux-android --release

# 3. Build pr-cli (host-side)
cd src/pr-cli && cargo build --target aarch64-linux-android --release

# 4. Copy binaries to the proot-engine library
cp build/out/arm64/proot android/proot-engine/src/main/jniLibs/arm64-v8a/libproot.so
cp build/out/arm64/loader android/proot-engine/src/main/jniLibs/arm64-v8a/libproot-loader.so
cp src/pr-cli/target/aarch64-linux-android/release/pr-cli \
   android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so

# 5. Build Android App and Engine AAR
cd android && ./gradlew assembleDebug
```

## Testing

### On-device (requires connected Android device)

```bash
# Install Alpine or Debian via the app UI, then:
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test alpine
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test debian
```

**54 tests** across 11 suites: distro, clone, readlink, gcc, rust, git, pipe, general, ssh, chroot, fakeroot

### Host-side (pr-cli unit tests)

```bash
cd src/pr-cli && cargo test
```

## Project structure

```
src/proot/                  # Patched proot C source
src/pr-cli/                 # Rust CLI (install, login, OCI pull)
src/proot-integration-test/ # Guest-side test binary (TAP output)
android/proot-engine/       # Standalone Android Library AAR (Engine)
android/app/                # Thin Android UI shell (Compose)
scripts/                    # Host-side build scripts
docs/                       # Technical documentation
```

## Documentation

| Document | Description |
|----------|-------------|
| [`docs/important-notes.md`](docs/important-notes.md) | Critical constraints, seccomp handlers, caveats — read first |
| [`docs/superpowers/specs/2026-09-13-proot-engine-library-design.md`](docs/superpowers/specs/2026-09-13-proot-engine-library-design.md) | Architecture of the extracted `:proot-engine` library |
| [`docs/proot-improvement.md`](docs/proot-improvement.md) | Our proot fork vs upstream and Termux (29 sections) |
| [`docs/targetsdk35-compatibility.md`](docs/targetsdk35-compatibility.md) | How targetSdk 35 works (PROOT_LOADER mechanism) |
| [`docs/rust-toolchain-support.md`](docs/rust-toolchain-support.md) | vfork/CLONE_VM fix, link2symlink readlink fix |
| [`docs/integration-tests.md`](docs/integration-tests.md) | Integration test suite (54 tests) |

## Credits

- [proot](https://github.com/proot-me/proot) — upstream proot v5.4.0 (GPL-2.0)
- [termux-proot](https://github.com/termux/termux-proot) — Termux's proot fork with Android patches (GPL-2.0)
- [proot-distro](https://github.com/termux/termux-packages/tree/master/packages/proot-distro) — distro plugins (GPL-3.0)
