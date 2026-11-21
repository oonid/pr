# PTY sessions on Android 9 to 12L: Implementation Plan

> Execute this plan task by task, with a review after each. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every terminal and command session starts on Android 9 to 12L, as it does on 13 and later: the forked child closes inherited descriptors without a syscall that the app seccomp filter traps there.

**Architecture:** `ptyjni.c` decides how to close descriptors before it forks: `close_range` only where the filter allows it (API 33+), and a `close()` loop up to the descriptor limit otherwise or when the kernel lacks `close_range`. The PTY library is built for x86_64 as well when a test property asks for it, so its instrumented contract tests can run on an x86_64 Android 11 emulator; the shipped AAR and APK stay arm64-only. CI also runs `:proot-engine`'s JVM tests.

**Tech Stack:** C (`android/proot-engine/src/main/cpp/ptyjni.c`, NDK through AGP and CMake), Kotlin instrumented tests (`PtyNativeContractTest`), Gradle, the Android emulator (x86_64 system image, KVM).

**Spec:** `docs/audit-2026-11-16.md` (local): A14, and the `:proot-engine` part of B14; details in `docs/audit-2026-11-16-details/findings-libraries.md` (S1, S7) and `verify-proot.md`.

## Findings this plan relies on

- `exec_child` resets every signal to `SIG_DFL` (`reset_signals`), then `close_inherited_fds` calls `syscall(SYS_close_range, 3, ~0U, 0)` and falls back to a `close()` loop only when that returns non-zero (`ptyjni.c:170-181,191`).
- The app seccomp filter allows `close_range` from `android-13.0.0_r1` (bionic `c9c0ebde3`); `android-12.1.0_r27` lacks it. The filter's action for anything not allowed is `SECCOMP_RET_TRAP`, so on API 28–32 the child gets SIGSYS before `execve` and dies with exit 159 (128 + 31), whatever the kernel supports. On API 33+ a kernel older than 5.9 returns ENOSYS and the loop runs.
- `minSdk` is 28 in all four modules. Nothing below Android 14 has been run.
- This machine is x86_64 with KVM; the emulator cannot run arm64 system images on it. `PtyNativeContractTest` forks `/system/bin/sh` directly (no proot), so an x86_64 image exercises exactly the code in question once `libptyjni.so` is built for x86_64. `abiFilters` is `arm64-v8a` only (`proot-engine/build.gradle.kts:12`).
- `adb run-as` processes have no app seccomp filter; instrumented tests run in the app's own process (zygote-forked), so they do.
- CI runs `:app` and `:session-core` JVM tests but not `:proot-engine`'s (`.github/workflows/ci.yml:91-96`).
- The `clone3` change merged in #63 (audit P1) needs proot and an arm64 guest, so it cannot be checked on an x86_64 emulator; see "Not in this PR".

## Global Constraints

- AGENTS.md applies: no comments; `vendor/` read only; commits `fix|test|chore|docs: …`, body ≤ 72 columns, no trailers; no device identifiers, bare `adb` in docs.
- The fork child stays async-signal-safe: everything it needs (whether to use `close_range`, the descriptor limit) is computed in the parent before `fork()`.
- The shipped AAR and APK stay arm64-only; x86_64 is built only when the test property is set.
- Ask before: installing the SDK command-line tools and an emulator system image (about 1 GB download), creating the emulator, and each Gradle instrumented-test run.

## Review Focus

1. The loop's upper bound: `RLIMIT_NOFILE` read in the parent, capped so a huge limit cannot make each session start take seconds.
2. API 33+ with a kernel older than 5.9 (`close_range` → ENOSYS) still closes every inherited descriptor.
3. Nothing changes on the phone (Android 17): the contract tests and the app's sessions behave as before.

---

### Task 1: Close inherited descriptors without a trapped syscall

**Files:**
- Modify: `android/proot-engine/src/main/cpp/ptyjni.c`
- Modify: `android/proot-engine/build.gradle.kts` (the test-only ABI property)
- Test: `android/proot-engine/src/androidTest/java/id/or/oo/pr/engine/PtyNativeContractTest.kt`

**Interfaces:**
- Before `fork()`: `bool use_close_range = android_get_device_api_level() >= 33;` and `int fd_limit` = `RLIMIT_NOFILE` `rlim_cur`, clamped to [1024, 65536] (65536 when unlimited or unreadable).
- `close_inherited_fds(bool use_close_range, int fd_limit)`: if `use_close_range` and `syscall(SYS_close_range, 3, ~0U, 0) == 0`, return; otherwise `close()` every fd from 3 to `fd_limit - 1`.
- `build.gradle.kts`: when the Gradle property `prTestAbis` is set (e.g. `-PprTestAbis=x86_64`), its comma-separated ABIs are added to `abiFilters`; otherwise only `arm64-v8a`.

- [ ] **Step 1: Write the failing test.** `aSessionStartsAndExitsCleanly`: start `/system/bin/sh -c 'exit 7'` through `PtyNative`, wait with `waitPid`, expect exit code 7 (today on API 30 it is 159). Keep `childHasOnlyTheTerminalDescriptors`, which also fails there today.
- [ ] **Step 2:** (Ask first.) Run `:proot-engine:connectedDebugAndroidTest -PprTestAbis=x86_64` on an x86_64 Android 11 (API 30) emulator. Expected: every test that starts a child fails, the new one with exit 159.
- [ ] **Step 3:** Implement the interfaces above.
- [ ] **Step 4:** Run the same on the API 30 emulator, and `:proot-engine:connectedDebugAndroidTest` on the phone (Android 17). Expected: all pass on both.
- [ ] **Step 5: Commit** `fix: close a session's inherited descriptors without close_range on Android 12 and older`.

---

### Task 2: CI runs the engine's JVM tests

**Files:**
- Modify: `.github/workflows/ci.yml`, `AGENTS.md` (Testing section), `docs/android-apk-architecture.md` or `docs/design/proot-engine-library.md` where tests are listed

- [ ] **Step 1:** Add a step running `./gradlew :proot-engine:testDebugUnitTest` next to the `:session-core` one.
- [ ] **Step 2:** Document the `prTestAbis` property and the emulator run (x86_64 image, KVM; arm64 images do not run on x86_64 hosts; `adb run-as` has no seccomp filter) in AGENTS.md's Testing section.
- [ ] **Step 3: Commit** `chore: run the proot-engine JVM tests` and `docs: running the PTY contract tests on an emulator`.

---

### Task 3: Phone check and the PR

- [ ] **Step 1:** (Ask first.) `./gradlew assembleDebug`, install on the phone, open a terminal on Alpine, run a command, End the session.
- [ ] **Step 2:** The pull request lists the emulator and phone results, without device names.

## Not in this PR

- Checking the `clone3` change from #63 on Android 9–13: it needs proot and an arm64 guest, so an arm64 device on Android 13 or older (or a cloud device), not this emulator.
- A16 (`Session.close()` while a reader is blocked), A15 (cancel), A23, A24: the libraries PR.
