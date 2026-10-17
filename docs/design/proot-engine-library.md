# Design Spec: Extract `:proot-engine` Android Library

**Date:** 2026-09-13
**Topic:** Proot Engine Library Refactoring

## 1. Goal & Context
The goal is to extract the core Linux execution engine (proot, ptyjni, ProotLauncher) from `pr.oo.or.id`'s `:app` module into a standalone Android Library module named `:proot-engine`. 

Currently, consumer apps like `rs.oo.or.id` duplicate the `.so` binaries and Kotlin files using a manual copy approach (the "Fast Lane"). By extracting `:proot-engine`, consumer apps can consume the engine dynamically via Gradle Composite Builds with zero file duplication.

## 2. Architecture & Module Boundaries

### New Module: `:proot-engine`
This module will be a self-contained Android Library containing:
- **Kotlin API:** `ProotLauncher.kt`, `PtyNative.kt` (moved from `:app`, under `id.or.oo.pr.engine`)
- **New Interface:** `ProotHost.kt` (to decouple `ProotLauncher` from `pr`'s `App` class)
- **C JNI Code:** `ptyjni.c`, `CMakeLists.txt` (moved from `:app`)
- **Native Binaries:** `libproot.so`, `libproot-loader.so`, `libpr-cli.so`, `libbusybox.so` (moved from `:app`'s `jniLibs`)

### The `ProotHost` Interface
To allow `ProotLauncher` to work in any app context without being tightly coupled to `id.or.oo.pr.App`, we introduce an interface:

```kotlin
package id.or.oo.pr.engine

import java.io.File

interface ProotHost {
    val prefixDir: File
    val homeDir: File
    val packageName: String
    val cacheDir: File
}
```

- `ProotLauncher` will be refactored from `class ProotLauncher(private val app: App)` to `class ProotLauncher(private val host: ProotHost)`.
- In `pr.oo.or.id`, `App.kt` will implement `ProotHost` (it already exposes these 4 properties).

### Thin UI Shell: `:app`
The `:app` module in `pr.oo.or.id` becomes a thin UI shell. It will:
- Add a dependency: `implementation(project(":proot-engine"))`
- Remove its local `jniLibs/` and `cpp/` directories.
- Retain its Compose-based UI (`MainActivity.kt`, `TerminalActivity.kt`, `App.kt`).

## 3. Build System Changes
- `pr.oo.or.id/scripts/build.sh` (and related tooling) will be updated to output the compiled `.so` binaries to `android/proot-engine/src/main/jniLibs/arm64-v8a/` instead of `android/app/src/main/jniLibs/arm64-v8a/`.
- `pr.oo.or.id/android/settings.gradle.kts` will `include(":proot-engine")`.

## 4. Implementation Phases

**Phase A: Extraction (in `pr.oo.or.id` repository)**
1. Create the `:proot-engine` module and configure its `build.gradle.kts`.
2. Introduce `ProotHost.kt`.
3. Move `ProotLauncher.kt`, `PtyNative.kt`, `ptyjni.c`, `CMakeLists.txt`, and `jniLibs/` to `:proot-engine`.
4. Refactor `ProotLauncher` to use `ProotHost`. Update `App.kt` to implement `ProotHost`.
5. Update `TerminalActivity.kt` import paths.
6. Update `scripts/build.sh` and related docs (`AGENTS.md`, `android-apk-architecture.md`).
7. Verify that `pr`'s APK builds and behaves identically.

**Phase B: Consumption (in `rs.oo.or.id` repository)**
*(To be done after Phase A is merged)*
1. Update `rs.oo.or.id/settings.gradle.kts` to configure a composite build for `:proot-engine` and `:termlib`.
2. Have `RsApp` implement `ProotHost`.
3. Delete all duplicated files from `rs.oo.or.id` (`engine/` package, `jniLibs/`, `termlib/`).
4. Verify `rs`'s APK builds perfectly utilizing the composite dependency.

## 5. End State
Single source of truth for the proot execution engine. Any updates to proot, bash, busybox, or `pr-cli` instantly propagate to `rs.oo.or.id` (and any future clients) without manual file synchronization.
