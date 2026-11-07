# Documentation

The [project README](../README.md) gives the overview, the build steps and the test commands. [AGENTS.md](../AGENTS.md) holds the working rules and constraints for changing the code.

## Current

These describe how pr works now and are updated with the code.

| Document | What it covers |
|----------|----------------|
| [important-notes.md](important-notes.md) | Constraints, the zygote seccomp handlers, dpkg and apt quirks, pr-cli caveats. Read first. |
| [android-apk-architecture.md](android-apk-architecture.md) | The app, `:session-core` and `:proot-engine`; sessions, End and Back; list operations; installs, unfinished installs, downloads, image digests and hard links. |
| [proot-improvement.md](proot-improvement.md) | Our proot fork against upstream proot and Termux's: every patch, with the reason for it. |
| [integration-tests.md](integration-tests.md) | The on-device test suite (`pr-cli test`, Test in the app), suite by suite. |
| [scripts.md](scripts.md) | `scripts/build.sh`, which builds proot and its loader with the NDK. |
| [design/session-core.md](design/session-core.md) | Design of the `:session-core` library and command sessions, with notes on how it was carried out. |

## Design specs

Written before the work and dated; the current documents and the code are authoritative where they differ.

| Document | What it covers |
|----------|----------------|
| [design/background-sessions.md](design/background-sessions.md) | Background sessions, End, Cancel and the operation runner (2026-10-17). |
| [design/proot-engine-library.md](design/proot-engine-library.md) | Extracting the `:proot-engine` library (2026-09-13). |

## Records and explorations

Historical: what was found or done at the time, kept for the reasoning. Each starts with a note on what has changed since.

| Document | What it covers |
|----------|----------------|
| [rust-toolchain-support.md](rust-toolchain-support.md) | The vfork/CLONE_VM, link2symlink readlink and `/proc/self/exe` fixes that made `rustc` and `cargo` work. |
| [targetsdk35-compatibility.md](targetsdk35-compatibility.md) | Running at targetSdk 35 through the `PROOT_LOADER` mechanism. |
| [alpine-linux-support.md](alpine-linux-support.md) | Bringing up Alpine: login, `apk` and the syscalls it needed. |
| [busybox-bash-integration.md](busybox-bash-integration.md) | Phase 3: shipping busybox and bash, since replaced. |
| [proot-binary-build.md](proot-binary-build.md) | Phase 1: the first static proot build for Android. |
| [standalone-proot-distro.md](standalone-proot-distro.md) | Phase 2: the proot-distro shell script, since replaced by pr-cli. |
| [rust-port-pr-cli.md](rust-port-pr-cli.md) | The exploration that led to writing pr-cli in Rust. |
| [porting-exploration.md](porting-exploration.md) | The first exploration of porting proot-distro to Android. |
| [plans/](plans/) | Implementation plans for background sessions, their follow-ups and `:session-core`. |
