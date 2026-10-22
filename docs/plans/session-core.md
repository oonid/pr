# Session Core Library Implementation Plan

> Execute this plan task by task, with a review after each. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move pr's generic background-session code out of `:app` into an Android library, `:session-core`, so another app can run PRoot sessions in the background without copying code, and add a headless command session; pr behaves exactly as before.

**Architecture:** `:session-core` (package `id.or.oo.pr.session`) holds `SessionManager`, `ManagedSession`, `SessionService` and its notifications, End's `/proc` scan, `ProcessRunner`, `PrCliCommand` and a new `PrBootstrap`. The host couplings become interfaces the `Application` implements: `ProotHost` (already in `:proot-engine`) and a new `SessionHost` (the `SessionManager`, the notification icon, the intent that opens the app); notification actions are named after the package. A new `CommandSessionFactory` runs `pr-cli login <alias> --user root [--custom-bind host:guest]... -- '<command>'` through `ProotLauncher.startCustomSession` with no emulator, its output going to a host sink; `SessionManager.open(alias, factory)` takes the factory per call, so terminal and command sessions share one manager, one session per alias, and end the same way. pr's `App` implements both host interfaces and keeps its interactive path (`open(alias)` with `ProotSessionFactory`) unchanged.

**Tech Stack:** Kotlin 2.1.0, AGP 8.7.3, Gradle 8.11.1, JVM 17, kotlinx-coroutines 1.9.0, androidx.core 1.15.0, Compose BOM 2025.05.00 (`ui-graphics` only, for `Color`), termlib (`:termlib`, `vendor/termlib` read-only), `:proot-engine` (`ProotHost`, `ProotLauncher`), JUnit 4.13.2, AGP test fixtures, AGP unit-test coverage (JaCoCo).

**Design:** `docs/design/session-core.md`

## Global Constraints

- Branch `refactor/session-core`; every command below runs from the repository root unless it starts with `cd android`.
- New module `:session-core` at `android/session-core/`: `com.android.library` + `org.jetbrains.kotlin.android`, `namespace = "id.or.oo.pr.session"`, `compileSdk = 36`, `minSdk = 28`, `abiFilters += listOf("arm64-v8a")`, Java and Kotlin `jvmTarget` 17.
- Every moved and new Kotlin file uses package `id.or.oo.pr.session`, including `ProcessRunner`, `PrCliCommand`, `DistroCommand` and the new `CommandRunner`.
- `:session-core` dependencies, exactly: `api(project(":proot-engine"))`, `api(project(":termlib"))`, `api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")`, `implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")`, `implementation("androidx.core:core-ktx:1.15.0")`, `implementation(platform("androidx.compose:compose-bom:2025.05.00"))`, `implementation("androidx.compose.ui:ui-graphics")`, `testImplementation("junit:junit:4.13.2")`, `testFixturesImplementation("junit:junit:4.13.2")`.
- `android/gradle.properties` gains `android.experimental.enableTestFixturesKotlinSupport=true` (Kotlin test fixtures); apps that only build `:session-core`'s main code do not need it.
- pr's behaviour is unchanged: notification texts, channel ids `sessions_running` and `sessions_ended`, running notification id `1`, actions `id.or.oo.pr.action.END_ALL` and `id.or.oo.pr.action.TOGGLE_WAKE`, wake-lock tag `pr:sessions`, preferences `pr_prefs` with keys `bootstrap_version` and `bootstrapped`, `BOOTSTRAP_VERSION = 8`, the `SessionService` class name `id.or.oo.pr.session.SessionService`.
- Not modified: `vendor/`, `android/proot-engine/`, `android/termlib/`, `src/pr-cli/`, `src/proot/`.
- Code style (AGENTS.md): no comments; follow each file's existing patterns.
- Commits: type `feat|fix|chore|docs|test`, subject plus a body wrapped at 72 columns or fewer, no trailers. Stage explicit paths only, never `git add -A` or `git add .`: the untracked `android/proot-engine/.cxx/` and `src/pr-cli/src/bootstrap.rs` stay out.
- No device model numbers, marketing names or serials anywhere; adb commands use bare `adb`.
- Coverage gate: AGP's `enableUnitTestCoverage` on `:session-core`'s debug build writes `android/session-core/build/reports/coverage/test/debug/report.xml`. Every new or changed unit that runs on the JVM stays at 80% of lines or more: `SessionActions`, `PrCliCommand`, `NativeLinks`, `CommandSessionFactory`, `SessionManager`, `ProcessRunner`. Android-framework glue (`SessionService`, `SessionNotifications.running` and `ended`, `AndroidSessionHooks`, `ProotPty`, `ProotSessionFactory`, `PrBootstrap.run`) had no JVM coverage in `:app` either; Task 7 checks it on a phone.
- Test-count baseline (`cd android && ./gradlew :app:testDebugUnitTest`, 128 tests): `SessionManagerTest` 29, `ManagedSessionTest` 23, `ProotProcessesTest` 15, `ProcStatTest` 11, `ServiceStateTest` 11, `SessionNotificationsTest` 2, `ProcessRunnerTest` 8, `DistroOperationsTest` 27, `InstalledContainersTest` 2. Moved classes keep their counts.
- Per-class counts are printed with, from `android/`: `for f in <module>/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done`.

## File Structure

| File | Responsibility |
|------|----------------|
| Create `android/session-core/build.gradle.kts` | The library module: namespace, SDKs, ABI, dependencies, test fixtures, unit-test coverage |
| Modify `android/settings.gradle.kts` | Include `:session-core` |
| Modify `android/gradle.properties` | Enable Kotlin test fixtures |
| Modify `android/app/build.gradle.kts` | Depend on `:session-core`; test against its fixtures |
| Move `session/Pty.kt` | `Pty` interface (unchanged) |
| Move `session/ManagedSession.kt` | One session: reader thread, exit, screens, End (unchanged) |
| Move `session/SessionManager.kt` | One session per alias; `open` gains a per-call factory (Task 5) |
| Move `session/ServiceState.kt` | Service state machine (unchanged) |
| Move `session/ProotProcesses.kt` | `/proc` scan with `ProcFs` (unchanged) |
| Move `session/ProotPty.kt` | `Pty` over `ProotLauncher.Session` (unchanged) |
| Move `session/ProotSessionFactory.kt` | pr's interactive terminal session (unchanged) |
| Move `session/SessionService.kt` | Foreground service; reads `SessionHost`, actions from the package name |
| Move `session/SessionNotifications.kt` | Channels and notifications; icon and open intent from `SessionHost` |
| Move `session/AndroidSessionHooks.kt` | Real `ForegroundController`, `EndedNotifier`, battery check (unchanged) |
| Move `ProcessRunner.kt` → `session/ProcessRunner.kt` | Child process runner; package `id.or.oo.pr.session`; implements `CommandRunner` |
| Move `PrCliCommand.kt` → `session/PrCliCommand.kt` | Runs pr-cli for a `ProotHost` |
| Create `session/SessionHost.kt` | `SessionHost` interface and the `Context.sessionHost` lookup |
| Create `session/SessionActions.kt` | Notification action strings built from a package name |
| Create `session/CommandRunner.kt` | What `PrCliCommand` needs from a runner |
| Create `session/DistroCommand.kt` | `DistroCommand`, moved out of `DistroOperations.kt` |
| Create `session/NativeLinks.kt` | Links in `usr/bin` to the native libraries and busybox applets |
| Create `session/PrBootstrap.kt` | First-run and upgrade bootstrap for any host; `bootstrapEnv` |
| Create `session/Bind.kt` | One `--custom-bind` pair |
| Create `session/CommandSessionFactory.kt` | Headless command session; `loginArgs` |
| Move `app/src/main/assets/scripts/bootstrap.sh`, `app/src/main/assets/bin/busybox.applets` | Now `session-core/src/main/assets/...`, read by `PrBootstrap` |
| Move tests `session/SessionManagerTest`, `ManagedSessionTest`, `ServiceStateTest`, `ProotProcessesTest`, `ProcStatTest`, `SessionNotificationsTest`, `FakeProcFs`, and `ProcessRunnerTest` | `android/session-core/src/test/java/id/or/oo/pr/session/` |
| Move `session/FakePty.kt`, `session/Eventually.kt` | `android/session-core/src/testFixtures/java/id/or/oo/pr/session/`, shared with `:app`'s tests |
| Create tests `SessionActionsTest`, `PrCliCommandTest`, `NativeLinksTest`, `PrBootstrapTest`, `CommandSessionTest` | New JVM tests in `:session-core` |
| Modify `android/app/src/main/java/id/or/oo/pr/App.kt` | Implements `ProotHost` and `SessionHost`; runs `PrBootstrap` |
| Modify `android/app/src/main/java/id/or/oo/pr/DistroOperations.kt`, `android/app/src/test/java/id/or/oo/pr/DistroOperationsTest.kt` | Import `DistroCommand` from the module |
| Unchanged `android/app/src/main/AndroidManifest.xml` | `.session.SessionService` still resolves to `id.or.oo.pr.session.SessionService` |
| Modify `.github/workflows/ci.yml` | Run `:session-core:testDebugUnitTest` |
| Modify `docs/android-apk-architecture.md`, `README.md`, `AGENTS.md`, `docs/design/session-core.md` | Module layout, test counts, how other apps include the module, status |

All `session/...` main paths above are `android/app/src/main/java/id/or/oo/pr/session/` before and `android/session-core/src/main/java/id/or/oo/pr/session/` after; test paths likewise under `src/test/java/...`.

---

### Task 1: Module scaffold and the coupling-free moves

**Files:**
- Create: `android/session-core/build.gradle.kts`
- Modify: `android/settings.gradle.kts`, `android/gradle.properties`, `android/app/build.gradle.kts`, `android/app/src/main/java/id/or/oo/pr/PrCliCommand.kt`
- Move: `Pty.kt`, `ManagedSession.kt`, `SessionManager.kt`, `ServiceState.kt`, `ProotProcesses.kt`, `ProotPty.kt`, `ProotSessionFactory.kt`, `ProcessRunner.kt`
- Move tests: `SessionManagerTest.kt`, `ManagedSessionTest.kt`, `ServiceStateTest.kt`, `ProotProcessesTest.kt`, `ProcStatTest.kt`, `FakeProcFs.kt`, `ProcessRunnerTest.kt`
- Move fixtures: `FakePty.kt`, `Eventually.kt`

**Interfaces:**
- Consumes (unchanged): `interface ProotHost { val prefixDir: File; val homeDir: File; val packageName: String; val cacheDir: File; val dnsServers: List<String> }`, `ProotLauncher(host: ProotHost).startSession(distroName: String, user: String = "root", isolated: Boolean = false, rows: Int = 50, cols: Int = 200): ProotLauncher.Session?`, `TerminalEmulatorFactory.create(...)`.
- Produces (same signatures, new module): `interface Pty`, `class ManagedSession(alias: String, pty: Pty, emulator: TerminalEmulator?, output: (ByteArray, Int, Int) -> Unit, onEnded: (ManagedSession) -> Unit, endGraceMs: Long = END_GRACE_MS, signalWaitMs: Long = SIGNAL_WAIT_MS, exitCodeWaitMs: Long = EXIT_CODE_WAIT_MS, killExitWaitMs: Long = KILL_EXIT_WAIT_MS)`, `class SessionManager(factory: SessionFactory, foreground: ForegroundController, notifier: EndedNotifier)` with `fun open(alias: String): OpenResult`, `class ServiceState`, `interface ProcFs`, `object SystemProcFs`, `class ProotProcesses(proc: ProcFs = SystemProcFs)`, `class ProotPty(session: ProotLauncher.Session, processes: ProotProcesses = ProotProcesses()) : Pty`, `class ProotSessionFactory(host: ProotHost) : SessionManager.SessionFactory`, `class ProcessRunner(parentPid: Int, signal: (pid: Int, signal: Int) -> Unit, processes: ProotProcesses = ProotProcesses(), stopWaitMs: Long = STOP_WAIT_MS)` with `suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int`.
- Test fixtures (consumed by `:app`'s `DistroOperationsTest`): `class FakePty : Pty`, `fun eventually(timeoutMs: Long = 3_000, condition: () -> Boolean)`.

- [ ] **Step 1: Register the module**

`android/settings.gradle.kts`, before:

```kotlin
include(":app")
include(":proot-engine")
include(":termlib")
```

after:

```kotlin
include(":app")
include(":proot-engine")
include(":session-core")
include(":termlib")
```

`android/gradle.properties`, before:

```properties
android.useAndroidX=true
org.gradle.jvmargs=-Xmx2048m -Dfile.encoding=UTF-8
```

after:

```properties
android.useAndroidX=true
org.gradle.jvmargs=-Xmx2048m -Dfile.encoding=UTF-8
android.experimental.enableTestFixturesKotlinSupport=true
```

Create `android/session-core/build.gradle.kts`:

```kotlin
plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "id.or.oo.pr.session"
    compileSdk = 36

    defaultConfig {
        minSdk = 28
        ndk { abiFilters += listOf("arm64-v8a") }
    }

    buildTypes {
        debug {
            enableUnitTestCoverage = true
        }
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    testFixtures {
        enable = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    api(project(":proot-engine"))
    api(project(":termlib"))
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation(platform("androidx.compose:compose-bom:2025.05.00"))
    implementation("androidx.compose.ui:ui-graphics")

    testImplementation("junit:junit:4.13.2")
    testFixturesImplementation("junit:junit:4.13.2")
}
```

`api` because `ProotHost` (`ProotSessionFactory`, `PrCliCommand`), `TerminalEmulator` (`ManagedSession.emulator`) and `StateFlow` (`SessionManager`) are in the public signatures. `ui-graphics` is the only Compose artifact used (`Color` in `ProotSessionFactory`); no Compose compiler plugin is needed because the module has no `@Composable`. `isReturnDefaultValues` lets `android.util.Log` calls run in JVM tests (Task 3). The module has no `AndroidManifest.xml`: AGP 8 takes the namespace from the build file, and each app declares the service itself (design §3).

`android/app/build.gradle.kts`, before:

```kotlin
    implementation(project(":proot-engine"))
    implementation(project(":termlib"))

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    testImplementation("junit:junit:4.13.2")
}
```

after:

```kotlin
    implementation(project(":proot-engine"))
    implementation(project(":termlib"))
    implementation(project(":session-core"))

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    testImplementation("junit:junit:4.13.2")
    testImplementation(testFixtures(project(":session-core")))
}
```

- [ ] **Step 2: Move the tests and the shared fakes**

```bash
mkdir -p android/session-core/src/test/java/id/or/oo/pr/session \
         android/session-core/src/testFixtures/java/id/or/oo/pr/session
for f in SessionManagerTest ManagedSessionTest ServiceStateTest ProotProcessesTest ProcStatTest FakeProcFs; do
  git mv android/app/src/test/java/id/or/oo/pr/session/$f.kt \
         android/session-core/src/test/java/id/or/oo/pr/session/$f.kt
done
git mv android/app/src/test/java/id/or/oo/pr/ProcessRunnerTest.kt \
       android/session-core/src/test/java/id/or/oo/pr/session/ProcessRunnerTest.kt
for f in FakePty Eventually; do
  git mv android/app/src/test/java/id/or/oo/pr/session/$f.kt \
         android/session-core/src/testFixtures/java/id/or/oo/pr/session/$f.kt
done
```

Edit `android/session-core/src/test/java/id/or/oo/pr/session/ProcessRunnerTest.kt` (package only; the two imports become same-package). Before:

```kotlin
package id.or.oo.pr

import id.or.oo.pr.session.SystemProcFs
import id.or.oo.pr.session.processState
import kotlinx.coroutines.Dispatchers
```

after:

```kotlin
package id.or.oo.pr.session

import kotlinx.coroutines.Dispatchers
```

No other test file changes; `DistroOperationsTest` keeps `import id.or.oo.pr.session.FakePty` and `import id.or.oo.pr.session.eventually`, now served by the fixtures.

- [ ] **Step 3: Run the moved tests and expect them to fail**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest
```

Expected: `BUILD FAILED`; Kotlin reports `Unresolved reference` for `Pty` (fixtures) and for `SessionManager`, `ManagedSession`, `ProcessRunner`, `ProotProcesses` (tests), because the main sources are still in `:app`.

- [ ] **Step 4: Move the main sources**

```bash
mkdir -p android/session-core/src/main/java/id/or/oo/pr/session
for f in Pty ManagedSession SessionManager ServiceState ProotProcesses ProotPty ProotSessionFactory; do
  git mv android/app/src/main/java/id/or/oo/pr/session/$f.kt \
         android/session-core/src/main/java/id/or/oo/pr/session/$f.kt
done
git mv android/app/src/main/java/id/or/oo/pr/ProcessRunner.kt \
       android/session-core/src/main/java/id/or/oo/pr/session/ProcessRunner.kt
```

Edit `android/session-core/src/main/java/id/or/oo/pr/session/ProcessRunner.kt`. Before:

```kotlin
package id.or.oo.pr

import id.or.oo.pr.session.ProotProcesses
import kotlinx.coroutines.CancellationException
```

after:

```kotlin
package id.or.oo.pr.session

import kotlinx.coroutines.CancellationException
```

Edit `android/app/src/main/java/id/or/oo/pr/PrCliCommand.kt` (it stays in `:app` until Task 3). Before:

```kotlin
import android.os.Process
import android.util.Log
import kotlinx.coroutines.Dispatchers
```

after:

```kotlin
import android.os.Process
import android.util.Log
import id.or.oo.pr.session.ProcessRunner
import kotlinx.coroutines.Dispatchers
```

`SessionService.kt`, `SessionNotifications.kt` and `AndroidSessionHooks.kt` stay in `android/app/src/main/java/id/or/oo/pr/session/` (same package, other module) until Task 2.

- [ ] **Step 5: Run the module's tests and expect them to pass with unchanged counts**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest
for f in session-core/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
```

Expected: `BUILD SUCCESSFUL`; 97 tests, 0 failures: `ManagedSessionTest` 23, `ProcessRunnerTest` 8, `ProcStatTest` 11, `ProotProcessesTest` 15, `ServiceStateTest` 11, `SessionManagerTest` 29.

- [ ] **Step 6: Run the app's tests**

```bash
cd android && ./gradlew :app:testDebugUnitTest
for f in app/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
```

Expected: `BUILD SUCCESSFUL`; 31 tests, 0 failures: `DistroOperationsTest` 27, `InstalledContainersTest` 2, `id.or.oo.pr.session.SessionNotificationsTest` 2. With Step 5, 128 in all, as before. (Delete `android/app/build/test-results` first if result files from before the move are still there.)

- [ ] **Step 7: Build the APK**

```bash
cd android && ./gradlew assembleDebug
```

Expected: `BUILD SUCCESSFUL`.

- [ ] **Step 8: Commit**

```bash
git add android/settings.gradle.kts android/gradle.properties android/app android/session-core
git commit -F - <<'EOF'
chore: move session code into a :session-core library

Add the :session-core Android library (namespace id.or.oo.pr.session)
and move into it the session classes that do not depend on pr's App,
with ProcessRunner and their JVM tests. FakePty and eventually become
test fixtures that :app's DistroOperationsTest still uses.
EOF
```

---

### Task 2: `SessionHost`, actions from the package name, and the service move

**Files:**
- Create: `android/session-core/src/main/java/id/or/oo/pr/session/SessionHost.kt`, `.../session/SessionActions.kt`
- Test: `android/session-core/src/test/java/id/or/oo/pr/session/SessionActionsTest.kt`
- Move: `SessionService.kt`, `SessionNotifications.kt`, `AndroidSessionHooks.kt`, `SessionNotificationsTest.kt`
- Modify: `android/app/src/main/java/id/or/oo/pr/App.kt`
- Unchanged: `android/app/src/main/AndroidManifest.xml`

**Interfaces:**
- Produces:
  - `interface SessionHost { val sessions: SessionManager; val notificationIcon: Int; fun openAppIntent(): Intent }`
  - `internal val Context.sessionHost: SessionHost` (`applicationContext as SessionHost`)
  - `object SessionActions { fun endAll(packageName: String): String; fun toggleWake(packageName: String): String }`
- Removes: `SessionService.ACTION_END_ALL`, `SessionService.ACTION_TOGGLE_WAKE` (no users outside `session/`).
- Unchanged signatures: `SessionNotifications.createChannels(context: Context)`, `SessionNotifications.running(context: Context, aliases: List<String>, keepAwake: Boolean, exempt: Boolean): Notification`, `SessionNotifications.ended(context: Context, alias: String, exitCode: Int?): Notification`, `fun runningTitle(aliases: List<String>): String`, `ServiceForegroundController(context: Context)`, `NotificationEndedNotifier(context: Context)`, `fun Context.isIgnoringBatteryOptimizations(): Boolean`.
- `App` becomes `class App : Application(), ProotHost, SessionHost`.

- [ ] **Step 1: Write the failing test**

`android/session-core/src/test/java/id/or/oo/pr/session/SessionActionsTest.kt`:

```kotlin
package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Test

class SessionActionsTest {

    @Test
    fun prKeepsItsActionNames() {
        assertEquals("id.or.oo.pr.action.END_ALL", SessionActions.endAll("id.or.oo.pr"))
        assertEquals("id.or.oo.pr.action.TOGGLE_WAKE", SessionActions.toggleWake("id.or.oo.pr"))
    }

    @Test
    fun anotherAppGetsActionsUnderItsOwnPackage() {
        assertEquals("org.example.gateway.action.END_ALL", SessionActions.endAll("org.example.gateway"))
        assertEquals("org.example.gateway.action.TOGGLE_WAKE", SessionActions.toggleWake("org.example.gateway"))
    }
}
```

- [ ] **Step 2: Run it and expect it to fail**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest --tests 'id.or.oo.pr.session.SessionActionsTest'
```

Expected: `BUILD FAILED`, `Unresolved reference 'SessionActions'`.

- [ ] **Step 3: Write `SessionHost` and `SessionActions`**

`android/session-core/src/main/java/id/or/oo/pr/session/SessionHost.kt`:

```kotlin
package id.or.oo.pr.session

import android.content.Context
import android.content.Intent

interface SessionHost {
    val sessions: SessionManager
    val notificationIcon: Int
    fun openAppIntent(): Intent
}

internal val Context.sessionHost: SessionHost
    get() = applicationContext as SessionHost
```

`android/session-core/src/main/java/id/or/oo/pr/session/SessionActions.kt`:

```kotlin
package id.or.oo.pr.session

object SessionActions {
    fun endAll(packageName: String): String = "$packageName.action.END_ALL"

    fun toggleWake(packageName: String): String = "$packageName.action.TOGGLE_WAKE"
}
```

- [ ] **Step 4: Move the service, notifications and hooks**

```bash
for f in SessionService SessionNotifications AndroidSessionHooks; do
  git mv android/app/src/main/java/id/or/oo/pr/session/$f.kt \
         android/session-core/src/main/java/id/or/oo/pr/session/$f.kt
done
git mv android/app/src/test/java/id/or/oo/pr/session/SessionNotificationsTest.kt \
       android/session-core/src/test/java/id/or/oo/pr/session/SessionNotificationsTest.kt
```

`AndroidSessionHooks.kt` and `SessionNotificationsTest.kt` are unchanged.

- [ ] **Step 5: Replace `SessionService.kt`**

`android/session-core/src/main/java/id/or/oo/pr/session/SessionService.kt` (the `App` import, the companion constants and the `App` cast go; the action match uses `SessionActions`):

```kotlin
package id.or.oo.pr.session

import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.ServiceCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

class SessionService : Service() {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var wakeLock: PowerManager.WakeLock? = null
    private val state = ServiceState()
    private val sessions: SessionManager get() = sessionHost.sessions

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        SessionNotifications.createChannels(this)
        scope.launch {
            combine(sessions.running, sessions.keepCpuAwake) { running, awake -> running to awake }
                .collect { (running, awake) -> execute(state.onRunningChanged(running, awake)) }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        execute(state.onStartCommand(startId, sessions.running.value, sessions.keepCpuAwake.value))
        when (intent?.action) {
            SessionActions.endAll(packageName) -> sessions.endAll()
            SessionActions.toggleWake(packageName) -> sessions.setKeepCpuAwake(!sessions.keepCpuAwake.value)
        }
        return START_NOT_STICKY
    }

    private fun execute(actions: List<ServiceState.Action>) {
        for (action in actions) {
            when (action) {
                is ServiceState.Action.StartForeground -> {
                    ServiceCompat.startForeground(
                        this,
                        SessionNotifications.RUNNING_ID,
                        notification(action.running, action.awake),
                        if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0,
                    )
                    sessions.foregroundStarted()
                }
                is ServiceState.Action.Notify -> getSystemService(NotificationManager::class.java)
                    .notify(SessionNotifications.RUNNING_ID, notification(action.running, action.awake))
                is ServiceState.Action.WakeLock -> setWakeLock(action.on)
                is ServiceState.Action.Stop -> {
                    ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
                    stopSelfResult(action.startId)
                }
            }
        }
    }

    override fun onDestroy() {
        scope.cancel()
        setWakeLock(false)
        super.onDestroy()
    }

    private fun notification(running: List<String>, awake: Boolean) =
        SessionNotifications.running(this, running, awake, isIgnoringBatteryOptimizations())

    private fun setWakeLock(on: Boolean) {
        val held = wakeLock
        if (on && held == null) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "pr:sessions")
                .apply { acquire() }
        } else if (!on && held != null) {
            if (held.isHeld) held.release()
            wakeLock = null
        }
    }
}
```

- [ ] **Step 6: Replace `SessionNotifications.kt`**

`android/session-core/src/main/java/id/or/oo/pr/session/SessionNotifications.kt` (texts unchanged; icon, open intent and action names come from the host):

```kotlin
package id.or.oo.pr.session

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat

object SessionNotifications {
    const val RUNNING_ID = 1
    private const val CHANNEL_RUNNING = "sessions_running"
    private const val CHANNEL_ENDED = "sessions_ended"

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_RUNNING, "Running sessions", NotificationManager.IMPORTANCE_LOW)
        )
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ENDED, "Session ended", NotificationManager.IMPORTANCE_DEFAULT)
        )
    }

    fun running(context: Context, aliases: List<String>, keepAwake: Boolean, exempt: Boolean): Notification {
        val title = runningTitle(aliases)
        val details = buildList {
            if (aliases.isNotEmpty()) add(aliases.joinToString(", "))
            if (keepAwake) add("CPU kept awake")
            if (keepAwake && !exempt) add("Battery saver may stop sessions; allow in the app")
        }.joinToString(" · ")
        return NotificationCompat.Builder(context, CHANNEL_RUNNING)
            .setSmallIcon(context.sessionHost.notificationIcon)
            .setContentTitle(title)
            .setContentText(details)
            .setStyle(NotificationCompat.BigTextStyle().bigText(details))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openList(context))
            .addAction(0, "End all", serviceAction(context, SessionActions.endAll(context.packageName)))
            .addAction(
                0,
                if (keepAwake) "Let CPU sleep" else "Keep CPU awake",
                serviceAction(context, SessionActions.toggleWake(context.packageName)),
            )
            .build()
    }

    fun ended(context: Context, alias: String, exitCode: Int?): Notification {
        val code = exitCode?.toString() ?: "unknown"
        return NotificationCompat.Builder(context, CHANNEL_ENDED)
            .setSmallIcon(context.sessionHost.notificationIcon)
            .setContentTitle("$alias: session ended (exit $code)")
            .setContentIntent(openList(context))
            .setAutoCancel(true)
            .build()
    }

    private fun openList(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        context.sessionHost.openAppIntent()
            .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE,
    )

    private fun serviceAction(context: Context, action: String): PendingIntent = PendingIntent.getService(
        context,
        action.hashCode(),
        Intent(context, SessionService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE,
    )
}

fun runningTitle(aliases: List<String>): String = when (aliases.size) {
    0 -> "Stopping…"
    1 -> "1 session running"
    else -> "${aliases.size} sessions running"
}
```

- [ ] **Step 7: Make `App` the `SessionHost`**

`android/app/src/main/java/id/or/oo/pr/App.kt`. Imports, before:

```kotlin
import android.app.Application
import android.content.Context
import android.content.SharedPreferences
```

after:

```kotlin
import android.app.Application
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
```

before:

```kotlin
import id.or.oo.pr.session.ServiceForegroundController
import id.or.oo.pr.session.SessionManager
```

after:

```kotlin
import id.or.oo.pr.session.ServiceForegroundController
import id.or.oo.pr.session.SessionHost
import id.or.oo.pr.session.SessionManager
```

Class header, before:

```kotlin
class App : Application(), ProotHost {
```

after:

```kotlin
class App : Application(), ProotHost, SessionHost {
```

Sessions and the two new members, before:

```kotlin
    val sessions: SessionManager by lazy {
        SessionManager(
            ProotSessionFactory(this),
            ServiceForegroundController(this),
            NotificationEndedNotifier(this),
        )
    }

    val containers: InstalledContainers by lazy { InstalledContainers(prefixDir) }
```

after:

```kotlin
    override val sessions: SessionManager by lazy {
        SessionManager(
            ProotSessionFactory(this),
            ServiceForegroundController(this),
            NotificationEndedNotifier(this),
        )
    }

    override val notificationIcon: Int get() = R.drawable.ic_notification

    override fun openAppIntent(): Intent = Intent(this, MainActivity::class.java)

    val containers: InstalledContainers by lazy { InstalledContainers(prefixDir) }
```

`AndroidManifest.xml` is not edited: `android:name=".session.SessionService"` resolves against the app namespace `id.or.oo.pr` to `id.or.oo.pr.session.SessionService`, the class's unchanged fully qualified name, now supplied by `:session-core`. `TerminalActivity` and `SessionBanners` keep `import id.or.oo.pr.session.SessionService`.

- [ ] **Step 8: Run the module's tests and expect them to pass**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest
for f in session-core/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
```

Expected: `BUILD SUCCESSFUL`; 101 tests, 0 failures: the six classes of Task 1 unchanged, `SessionNotificationsTest` 2, `SessionActionsTest` 2.

- [ ] **Step 9: Run the app's tests and build the APK**

```bash
cd android && rm -rf app/build/test-results && ./gradlew :app:testDebugUnitTest assembleDebug
for f in app/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
grep -n 'SessionService' app/build/intermediates/merged_manifest/debug/processDebugMainManifest/AndroidManifest.xml
```

Expected: `BUILD SUCCESSFUL`; `DistroOperationsTest` 27, `InstalledContainersTest` 2 (29 in `:app`, 130 in all); the merged manifest names `android:name="id.or.oo.pr.session.SessionService"`.

- [ ] **Step 10: Commit**

```bash
git add android/app android/session-core
git commit -F - <<'EOF'
chore: give session-core a SessionHost instead of pr's App

SessionService and its notifications read the SessionManager, the
small icon and the intent that opens the app from the Application,
which implements SessionHost. Notification actions are named after the
package, so pr keeps id.or.oo.pr.action.END_ALL and TOGGLE_WAKE.
EOF
```

---

### Task 3: `PrCliCommand` over `ProotHost`

**Files:**
- Create: `android/session-core/src/main/java/id/or/oo/pr/session/CommandRunner.kt`, `.../session/DistroCommand.kt`
- Test: `android/session-core/src/test/java/id/or/oo/pr/session/PrCliCommandTest.kt`
- Move: `android/app/src/main/java/id/or/oo/pr/PrCliCommand.kt` → `android/session-core/src/main/java/id/or/oo/pr/session/PrCliCommand.kt`
- Modify: `.../session/ProcessRunner.kt`, `android/app/src/main/java/id/or/oo/pr/DistroOperations.kt`, `android/app/src/test/java/id/or/oo/pr/DistroOperationsTest.kt`, `android/app/src/main/java/id/or/oo/pr/App.kt`

**Interfaces:**
- Produces:
  - `fun interface CommandRunner { suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int }`
  - `fun interface DistroCommand { suspend fun run(args: List<String>, onLine: (String) -> Unit): Int }` (moved from `id.or.oo.pr`)
  - `class ProcessRunner(...) : CommandRunner` (constructor unchanged)
  - `class PrCliCommand(host: ProotHost, runner: CommandRunner = ProcessRunner(Process.myPid(), Process::sendSignal)) : DistroCommand`
- Consumes: `ProotHost.prefixDir`, `homeDir`, `packageName`, `cacheDir`, `dnsServers`.

- [ ] **Step 1: Write the failing test**

`android/session-core/src/test/java/id/or/oo/pr/session/PrCliCommandTest.kt`:

```kotlin
package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

class PrCliCommandTest {

    private val host = object : ProotHost {
        override val prefixDir = File("/data/data/org.example.gateway/files/usr")
        override val homeDir = File("/data/data/org.example.gateway/files/home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File("/data/data/org.example.gateway/cache")
        override val dnsServers = listOf("192.0.2.53", "2001:db8::53")
    }

    private class FakeRunner(private val lines: List<String>, private val exitCode: Int) : CommandRunner {
        var cmd: List<String>? = null
        var env: Map<String, String>? = null

        override suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int {
            this.cmd = cmd
            this.env = env
            lines.forEach(onLine)
            return exitCode
        }
    }

    @Test
    fun runsPrCliFromTheHostPrefixWithTheHostEnvironment() = runBlocking {
        val runner = FakeRunner(emptyList(), 0)

        PrCliCommand(host, runner).run(listOf("install", "alpine")) { }

        assertEquals(listOf("/data/data/org.example.gateway/files/usr/bin/pr-cli", "install", "alpine"), runner.cmd)
        assertEquals(
            mapOf(
                "APP_PREFIX" to "/data/data/org.example.gateway/files/usr",
                "APP_HOME" to "/data/data/org.example.gateway/files/home",
                "APP_PACKAGE" to "org.example.gateway",
                "PATH" to "/data/data/org.example.gateway/files/usr/bin:/system/bin:/system/xbin",
                "PROOT_NO_SECCOMP" to "1",
                "PROOT_TMP_DIR" to "/data/data/org.example.gateway/cache",
                "HOME" to "/data/data/org.example.gateway/files/home",
                "TERM" to "xterm-256color",
                "TMPDIR" to "/data/data/org.example.gateway/cache",
                "APP_DNS_SERVERS" to "192.0.2.53,2001:db8::53",
            ),
            runner.env,
        )
    }

    @Test
    fun stripsEscapesAndCarriageReturnsAndDropsBlankLines() = runBlocking {
        val runner = FakeRunner(
            listOf("\u001B[1;32mInstalled\u001B[0m\r", "   ", "\u001B]0;pr-cli\u0007Done", ""),
            0,
        )
        val seen = mutableListOf<String>()

        PrCliCommand(host, runner).run(listOf("install", "alpine")) { seen += it }

        assertEquals(listOf("Installed", "Done"), seen)
    }

    @Test
    fun returnsTheRunnerExitCode() = runBlocking {
        assertEquals(3, PrCliCommand(host, FakeRunner(emptyList(), 3)).run(listOf("remove", "alpine")) { })
    }
}
```

- [ ] **Step 2: Run it and expect it to fail**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest --tests 'id.or.oo.pr.session.PrCliCommandTest'
```

Expected: `BUILD FAILED`, `Unresolved reference 'CommandRunner'` and `Unresolved reference 'PrCliCommand'`.

- [ ] **Step 3: Add `CommandRunner` and `DistroCommand`; make `ProcessRunner` a `CommandRunner`**

`android/session-core/src/main/java/id/or/oo/pr/session/CommandRunner.kt`:

```kotlin
package id.or.oo.pr.session

fun interface CommandRunner {
    suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int
}
```

`android/session-core/src/main/java/id/or/oo/pr/session/DistroCommand.kt`:

```kotlin
package id.or.oo.pr.session

fun interface DistroCommand {
    suspend fun run(args: List<String>, onLine: (String) -> Unit): Int
}
```

`android/session-core/src/main/java/id/or/oo/pr/session/ProcessRunner.kt`, before:

```kotlin
    private val stopWaitMs: Long = STOP_WAIT_MS,
) {

    suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int {
```

after:

```kotlin
    private val stopWaitMs: Long = STOP_WAIT_MS,
) : CommandRunner {

    override suspend fun run(cmd: List<String>, env: Map<String, String>, onLine: (String) -> Unit): Int {
```

- [ ] **Step 4: Move `PrCliCommand` and replace it**

```bash
git mv android/app/src/main/java/id/or/oo/pr/PrCliCommand.kt \
       android/session-core/src/main/java/id/or/oo/pr/session/PrCliCommand.kt
```

`android/session-core/src/main/java/id/or/oo/pr/session/PrCliCommand.kt`:

```kotlin
package id.or.oo.pr.session

import android.os.Process
import android.util.Log
import id.or.oo.pr.engine.ProotHost
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

class PrCliCommand(
    private val host: ProotHost,
    private val runner: CommandRunner = ProcessRunner(Process.myPid(), Process::sendSignal),
) : DistroCommand {

    override suspend fun run(args: List<String>, onLine: (String) -> Unit): Int {
        val binDir = File(host.prefixDir, "bin")

        val env = withContext(Dispatchers.IO) {
            mapOf(
                "APP_PREFIX" to host.prefixDir.absolutePath,
                "APP_HOME" to host.homeDir.absolutePath,
                "APP_PACKAGE" to host.packageName,
                "PATH" to "${binDir.absolutePath}:/system/bin:/system/xbin",
                "PROOT_NO_SECCOMP" to "1",
                "PROOT_TMP_DIR" to host.cacheDir.absolutePath,
                "HOME" to host.homeDir.absolutePath,
                "TERM" to "xterm-256color",
                "TMPDIR" to host.cacheDir.absolutePath,
                "APP_DNS_SERVERS" to host.dnsServers.joinToString(","),
            )
        }

        val prCli = File(binDir, "pr-cli").absolutePath
        val cmd = listOf(prCli) + args

        Log.d(TAG, "Running: ${cmd.joinToString(" ")}")
        val exitCode = runner.run(cmd, env) { line ->
            val cleaned = line
                .replace(ANSI_CSI, "")
                .replace(ANSI_OSC, "")
                .replace("\r", "")
                .trim()
            if (cleaned.isNotEmpty()) {
                Log.d(TAG, cleaned)
                onLine(cleaned)
            }
        }
        Log.d(TAG, "Exit code: $exitCode")
        return exitCode
    }

    private companion object {
        const val TAG = "PR"
        val ANSI_CSI = Regex("\\x1B\\[[0-9;]*[mGKHJ]")
        val ANSI_OSC = Regex("\\x1B\\]\\d+;.*?\\x07")
    }
}
```

- [ ] **Step 5: Point `:app` at the moved types**

`android/app/src/main/java/id/or/oo/pr/DistroOperations.kt`, before:

```kotlin
package id.or.oo.pr

import id.or.oo.pr.session.ManagedSession
```

after:

```kotlin
package id.or.oo.pr

import id.or.oo.pr.session.DistroCommand
import id.or.oo.pr.session.ManagedSession
```

and delete these lines (above `class DistroOperations(`):

```kotlin
fun interface DistroCommand {
    suspend fun run(args: List<String>, onLine: (String) -> Unit): Int
}

```

`android/app/src/test/java/id/or/oo/pr/DistroOperationsTest.kt`, before:

```kotlin
import id.or.oo.pr.DistroOperations.Kind
import id.or.oo.pr.session.FakePty
```

after:

```kotlin
import id.or.oo.pr.DistroOperations.Kind
import id.or.oo.pr.session.DistroCommand
import id.or.oo.pr.session.FakePty
```

`android/app/src/main/java/id/or/oo/pr/App.kt`, before:

```kotlin
import id.or.oo.pr.session.NotificationEndedNotifier
import id.or.oo.pr.session.ProotSessionFactory
```

after:

```kotlin
import id.or.oo.pr.session.NotificationEndedNotifier
import id.or.oo.pr.session.PrCliCommand
import id.or.oo.pr.session.ProotSessionFactory
```

`PrCliCommand(this)` in `App.operations` is unchanged: `App` is a `ProotHost`.

- [ ] **Step 6: Run the module's tests and expect them to pass**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest
for f in session-core/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
```

Expected: `BUILD SUCCESSFUL`; 104 tests, 0 failures; `PrCliCommandTest` 3, `ProcessRunnerTest` still 8, the others as in Task 2.

- [ ] **Step 7: Check coverage**

```bash
cd android && ./gradlew :session-core:createDebugUnitTestCoverageReport
python3 -c "import xml.etree.ElementTree as E;r=E.parse('session-core/build/reports/coverage/test/debug/report.xml').getroot();[print(s.get('name'),c.get('covered')+'/'+str(int(c.get('missed'))+int(c.get('covered')))) for s in r.iter('sourcefile') for c in s.findall('counter') if c.get('type')=='LINE']"
```

Expected: `PrCliCommand.kt 35/36`, `ProcessRunner.kt 68/68`, `SessionActions.kt 2/2` (each at or above 80%).

- [ ] **Step 8: Run the app's tests and build the APK**

```bash
cd android && ./gradlew :app:testDebugUnitTest assembleDebug
```

Expected: `BUILD SUCCESSFUL`; `DistroOperationsTest` 27, `InstalledContainersTest` 2.

- [ ] **Step 9: Commit**

```bash
git add android/app android/session-core
git commit -F - <<'EOF'
chore: run pr-cli from session-core against any ProotHost

PrCliCommand and DistroCommand move into :session-core, and
PrCliCommand takes a ProotHost instead of pr's App. ProcessRunner
implements a CommandRunner interface, so PrCliCommand is tested with
a fake host and a fake runner.
EOF
```

---

### Task 4: `PrBootstrap` with `bootstrap.sh` as a module asset

**Files:**
- Create: `android/session-core/src/main/java/id/or/oo/pr/session/NativeLinks.kt`, `.../session/PrBootstrap.kt`
- Test: `android/session-core/src/test/java/id/or/oo/pr/session/NativeLinksTest.kt`, `.../session/PrBootstrapTest.kt`
- Move: `android/app/src/main/assets/scripts/bootstrap.sh` → `android/session-core/src/main/assets/scripts/bootstrap.sh`; `android/app/src/main/assets/bin/busybox.applets` → `android/session-core/src/main/assets/bin/busybox.applets`
- Modify: `android/app/src/main/java/id/or/oo/pr/App.kt`

**Interfaces:**
- Produces:
  - `class PrBootstrap(context: Context, host: ProotHost, version: Int)` with `fun run()`
  - `internal class NativeLinks(prefixDir: File, nativeLibDir: File, symlink: (target: String, link: String) -> Unit)` with `fun ensure()` and `fun applets(names: List<String>): Int`
  - `internal fun bootstrapEnv(host: ProotHost): Map<String, String>`
- Consumes: `Context.getSharedPreferences`, `Context.assets`, `Context.applicationInfo.nativeLibraryDir`, `android.system.Os.symlink(String, String)`, the `ProotHost` paths.
- `App` keeps `private const val BOOTSTRAP_VERSION = 8` and calls `PrBootstrap(this, this, BOOTSTRAP_VERSION).run()`.

`NativeLinks` is `App.ensureNativeLibSymlinks` and `createBusyboxSymlinks` with the link call passed in, so the JVM tests link with `java.nio.file.Files` in a temporary folder; on the phone it is `Os.symlink`, as before.

- [ ] **Step 1: Write the failing tests**

`android/session-core/src/test/java/id/or/oo/pr/session/NativeLinksTest.kt`:

```kotlin
package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.Paths

class NativeLinksTest {

    @get:Rule
    val tmp = TemporaryFolder()

    private lateinit var prefix: File
    private lateinit var libs: File

    private val symlink: (String, String) -> Unit = { target, link ->
        Files.createSymbolicLink(Paths.get(link), Paths.get(target))
    }

    @Before
    fun setUp() {
        prefix = tmp.newFolder("usr")
        libs = tmp.newFolder("lib")
        listOf("libbusybox.so", "libproot.so", "libpr-cli.so").forEach { File(libs, it).writeText("elf") }
    }

    private fun bin(name: String) = File(prefix, "bin/$name")

    private fun target(name: String) = Files.readSymbolicLink(bin(name).toPath()).toString()

    private fun lib(name: String) = File(libs, name).absolutePath

    private fun links(symlink: (String, String) -> Unit = this.symlink) = NativeLinks(prefix, libs, symlink)

    @Test
    fun linksEachNativeLibraryIntoBin() {
        links().ensure()

        assertEquals(lib("libbusybox.so"), target("busybox"))
        assertEquals(lib("libproot.so"), target("proot"))
        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
    }

    @Test
    fun skipsALibraryTheApkDoesNotShip() {
        File(libs, "libproot.so").delete()

        links().ensure()

        assertFalse(Files.exists(bin("proot").toPath(), LinkOption.NOFOLLOW_LINKS))
        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
    }

    @Test
    fun keepsALinkThatAlreadyPointsAtTheLibrary() {
        links().ensure()

        links { _, link -> throw IOException("relinked $link") }.ensure()

        assertEquals(lib("libproot.so"), target("proot"))
    }

    @Test
    fun replacesALinkToAnotherLibraryDirectory() {
        val old = tmp.newFolder("old-lib")
        File(old, "libproot.so").writeText("old")
        bin("proot").parentFile!!.mkdirs()
        symlink(File(old, "libproot.so").absolutePath, bin("proot").absolutePath)

        links().ensure()

        assertEquals(lib("libproot.so"), target("proot"))
    }

    @Test
    fun replacesADanglingLink() {
        bin("pr-cli").parentFile!!.mkdirs()
        symlink(File(tmp.root, "gone/libpr-cli.so").absolutePath, bin("pr-cli").absolutePath)

        links().ensure()

        assertEquals(lib("libpr-cli.so"), target("pr-cli"))
    }

    @Test
    fun removesStaleBinariesAndTheScriptsDirectory() {
        bin("bash").parentFile!!.mkdirs()
        listOf("bash", "pr-test", "proot-distro").forEach { bin(it).writeText("old") }
        File(prefix, "scripts/proot-distro.sh").apply { parentFile!!.mkdirs() }.writeText("old")

        links().ensure()

        listOf("bash", "pr-test", "proot-distro").forEach { assertFalse(bin(it).exists()) }
        assertFalse(File(prefix, "scripts").exists())
    }

    @Test
    fun linksEachAppletToBusyboxAndKeepsExistingFiles() {
        bin("ls").parentFile!!.mkdirs()
        bin("ls").writeText("mine")

        val created = links().applets(listOf("sh", "", "ls", "tar"))

        assertEquals(2, created)
        assertEquals("busybox", target("sh"))
        assertEquals("busybox", target("tar"))
        assertEquals("mine", bin("ls").readText())
    }

    @Test
    fun anAppletThatCannotBeLinkedIsSkipped() {
        bin("sh").parentFile!!.mkdirs()

        val created = links { target, link ->
            if (link.endsWith("/bad")) throw IOException("denied") else symlink(target, link)
        }.applets(listOf("bad", "sh"))

        assertEquals(1, created)
        assertEquals("busybox", target("sh"))
    }
}
```

`android/session-core/src/test/java/id/or/oo/pr/session/PrBootstrapTest.kt`:

```kotlin
package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.File

class PrBootstrapTest {

    private val host = object : ProotHost {
        override val prefixDir = File("/data/data/org.example.gateway/files/usr")
        override val homeDir = File("/data/data/org.example.gateway/files/home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File("/data/data/org.example.gateway/cache")
    }

    @Test
    fun bootstrapShRunsWithTheHostPathsAndSystemBinFirst() {
        assertEquals(
            mapOf(
                "APP_PREFIX" to "/data/data/org.example.gateway/files/usr",
                "APP_HOME" to "/data/data/org.example.gateway/files/home",
                "APP_PACKAGE" to "org.example.gateway",
                "PATH" to "/system/bin:/system/xbin:/data/data/org.example.gateway/files/usr/bin",
                "PROOT_NO_SECCOMP" to "1",
                "HOME" to "/data/data/org.example.gateway/files/home",
            ),
            bootstrapEnv(host),
        )
    }
}
```

- [ ] **Step 2: Run them and expect them to fail**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest --tests 'id.or.oo.pr.session.NativeLinksTest' --tests 'id.or.oo.pr.session.PrBootstrapTest'
```

Expected: `BUILD FAILED`, `Unresolved reference 'NativeLinks'` and `Unresolved reference 'bootstrapEnv'`.

- [ ] **Step 3: Write `NativeLinks`**

`android/session-core/src/main/java/id/or/oo/pr/session/NativeLinks.kt`:

```kotlin
package id.or.oo.pr.session

import android.util.Log
import java.io.File
import java.nio.file.Files

internal class NativeLinks(
    private val prefixDir: File,
    private val nativeLibDir: File,
    private val symlink: (target: String, link: String) -> Unit,
) {
    private val binDir = File(prefixDir, "bin")

    fun ensure() {
        binDir.mkdirs()

        for (name in STALE) {
            val f = File(binDir, name)
            if (f.exists() || Files.isSymbolicLink(f.toPath())) {
                f.delete()
                Log.d(TAG, "Removed stale: $name")
            }
        }
        val scriptsDir = File(prefixDir, "scripts")
        if (scriptsDir.exists()) {
            scriptsDir.deleteRecursively()
            Log.d(TAG, "Removed stale scripts dir")
        }

        for ((name, lib) in LINKS) {
            val link = File(binDir, name)
            val target = File(nativeLibDir, lib)
            if (!target.exists()) {
                Log.w(TAG, "$lib not found in $nativeLibDir")
                continue
            }

            if (link.exists()) {
                if (link.canonicalPath == target.canonicalPath) continue
                link.delete()
            } else if (Files.isSymbolicLink(link.toPath())) {
                link.delete()
            }

            symlink(target.absolutePath, link.absolutePath)
            Log.d(TAG, "Symlink: $name -> ${target.absolutePath}")
        }
    }

    fun applets(names: List<String>): Int {
        var count = 0
        for (applet in names.filter { it.isNotBlank() }) {
            val link = File(binDir, applet)
            if (link.exists()) continue
            try {
                symlink("busybox", link.absolutePath)
                count++
            } catch (e: Exception) {
                Log.w(TAG, "Failed to create symlink for $applet: ${e.message}")
            }
        }
        return count
    }

    private companion object {
        const val TAG = "PR"
        val LINKS = mapOf(
            "busybox" to "libbusybox.so",
            "proot" to "libproot.so",
            "pr-cli" to "libpr-cli.so",
        )
        val STALE = listOf("bash", "pr-test", "proot-distro")
    }
}
```

- [ ] **Step 4: Write `PrBootstrap`**

`android/session-core/src/main/java/id/or/oo/pr/session/PrBootstrap.kt`:

```kotlin
package id.or.oo.pr.session

import android.content.Context
import android.content.SharedPreferences
import android.system.Os
import android.util.Log
import id.or.oo.pr.engine.ProotHost
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

class PrBootstrap(
    private val context: Context,
    private val host: ProotHost,
    private val version: Int,
) {

    fun run() {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val currentVersion = prefs.getInt(KEY_VERSION, 0)
        val links = NativeLinks(host.prefixDir, File(context.applicationInfo.nativeLibraryDir), Os::symlink)

        if (currentVersion < version) {
            bootstrap(prefs, links)
        } else {
            links.ensure()
            Log.i(TAG, "Already bootstrapped (version $currentVersion)")
        }
    }

    private fun bootstrap(prefs: SharedPreferences, links: NativeLinks) {
        Log.i(TAG, "Running bootstrap...")
        val t0 = System.currentTimeMillis()

        try {
            val binDir = File(host.prefixDir, "bin")

            binDir.mkdirs()
            host.homeDir.mkdirs()
            File(host.prefixDir, "tmp").mkdirs()

            links.ensure()
            Log.d(TAG, "Creating busybox applet symlinks...")
            val applets = context.assets.open("bin/busybox.applets").bufferedReader().readLines()
            Log.d(TAG, "Created ${links.applets(applets)} busybox applet symlinks")

            copyAssetFile("scripts/bootstrap.sh", File(binDir, "bootstrap.sh"))

            executeBootstrap()

            prefs.edit()
                .putInt(KEY_VERSION, version)
                .putBoolean(KEY_INITIALIZED, true)
                .apply()

            val elapsed = System.currentTimeMillis() - t0
            Log.i(TAG, "Bootstrap complete (${elapsed}ms)")
        } catch (e: Exception) {
            Log.e(TAG, "Bootstrap failed", e)
        }
    }

    private fun copyAssetFile(assetPath: String, dest: File) {
        context.assets.open(assetPath).use { input ->
            FileOutputStream(dest).use { output ->
                input.copyTo(output)
            }
        }
        dest.setExecutable(true, false)
        dest.setReadable(true, false)
        Log.d(TAG, "Installed: ${dest.name}")
    }

    private fun executeBootstrap() {
        val bootstrapScript = File(host.prefixDir, "bin/bootstrap.sh")
        if (!bootstrapScript.exists()) {
            Log.e(TAG, "bootstrap.sh not found")
            return
        }

        val cmd = arrayOf(
            "/system/bin/sh",
            bootstrapScript.absolutePath,
        )

        val pb = ProcessBuilder(*cmd)
        pb.environment().putAll(bootstrapEnv(host))
        pb.redirectErrorStream(true)

        Log.d(TAG, "Executing: ${cmd.joinToString(" ")}")

        val proc = pb.start()
        val output = proc.inputStream.bufferedReader().readText()
        val exitCode = proc.waitFor()

        if (output.isNotBlank()) {
            for (line in output.lines()) {
                Log.d(TAG, "[bootstrap] $line")
            }
        }

        if (exitCode != 0) {
            throw IOException("bootstrap.sh failed with exit code $exitCode")
        }
    }

    private companion object {
        const val TAG = "PR"
        const val PREFS_NAME = "pr_prefs"
        const val KEY_INITIALIZED = "bootstrapped"
        const val KEY_VERSION = "bootstrap_version"
    }
}

internal fun bootstrapEnv(host: ProotHost): Map<String, String> {
    val binDir = File(host.prefixDir, "bin")
    return mapOf(
        "APP_PREFIX" to host.prefixDir.absolutePath,
        "APP_HOME" to host.homeDir.absolutePath,
        "APP_PACKAGE" to host.packageName,
        "PATH" to "/system/bin:/system/xbin:${binDir.absolutePath}",
        "PROOT_NO_SECCOMP" to "1",
        "HOME" to host.homeDir.absolutePath,
    )
}
```

- [ ] **Step 5: Move the assets**

```bash
mkdir -p android/session-core/src/main/assets/scripts android/session-core/src/main/assets/bin
git mv android/app/src/main/assets/scripts/bootstrap.sh android/session-core/src/main/assets/scripts/bootstrap.sh
git mv android/app/src/main/assets/bin/busybox.applets android/session-core/src/main/assets/bin/busybox.applets
rmdir android/app/src/main/assets/scripts android/app/src/main/assets/bin android/app/src/main/assets
```

Library assets merge into the APK at the same paths (`assets/scripts/bootstrap.sh`, `assets/bin/busybox.applets`).

- [ ] **Step 6: Replace `App.kt`**

`android/app/src/main/java/id/or/oo/pr/App.kt` (the bootstrap code, its constants, `nativeLibDir` and the imports only it used go; `BOOTSTRAP_VERSION` stays 8):

```kotlin
package id.or.oo.pr

import android.app.Application
import android.content.Intent
import android.net.ConnectivityManager
import id.or.oo.pr.engine.NetworkDns
import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.session.NotificationEndedNotifier
import id.or.oo.pr.session.PrBootstrap
import id.or.oo.pr.session.PrCliCommand
import id.or.oo.pr.session.ProotSessionFactory
import id.or.oo.pr.session.ServiceForegroundController
import id.or.oo.pr.session.SessionHost
import id.or.oo.pr.session.SessionManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import java.io.File
import java.net.Inet6Address

class App : Application(), ProotHost, SessionHost {

    companion object {
        private const val BOOTSTRAP_VERSION = 8
    }

    override val prefixDir: File get() = File(filesDir, "usr")

    override val dnsServers: List<String>
        get() {
            val connectivity = getSystemService(ConnectivityManager::class.java) ?: return emptyList()
            val network = connectivity.activeNetwork ?: return emptyList()
            val link = connectivity.getLinkProperties(network) ?: return emptyList()
            val hasIpv6DefaultRoute = link.routes.any {
                it.destination.prefixLength == 0 && it.destination.address is Inet6Address
            }
            return NetworkDns.usableServers(link.dnsServers, hasIpv6DefaultRoute)
        }

    override val homeDir: File get() = File(filesDir, "home")

    @get:JvmName("getProotPackageName")
    override val packageName: String get() = super.getPackageName()
    
    @get:JvmName("getProotCacheDir")
    override val cacheDir: File get() = super.getCacheDir()

    override val sessions: SessionManager by lazy {
        SessionManager(
            ProotSessionFactory(this),
            ServiceForegroundController(this),
            NotificationEndedNotifier(this),
        )
    }

    override val notificationIcon: Int get() = R.drawable.ic_notification

    override fun openAppIntent(): Intent = Intent(this, MainActivity::class.java)

    val containers: InstalledContainers by lazy { InstalledContainers(prefixDir) }

    val operations: DistroOperations by lazy {
        DistroOperations(
            PrCliCommand(this),
            sessions,
            CoroutineScope(SupervisorJob() + Dispatchers.Default),
            containers::contains,
        )
    }

    override fun onCreate() {
        super.onCreate()
        PrBootstrap(this, this, BOOTSTRAP_VERSION).run()
    }
}
```

The line `    ` with four trailing spaces above `@get:JvmName("getProotCacheDir")` is in the file today; keep it so the diff shows only the bootstrap move.

- [ ] **Step 7: Run the module's tests and expect them to pass**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest
for f in session-core/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
```

Expected: `BUILD SUCCESSFUL`; 113 tests, 0 failures; `NativeLinksTest` 8, `PrBootstrapTest` 1, the others as in Task 3.

- [ ] **Step 8: Check coverage**

```bash
cd android && ./gradlew :session-core:createDebugUnitTestCoverageReport
python3 -c "import xml.etree.ElementTree as E;r=E.parse('session-core/build/reports/coverage/test/debug/report.xml').getroot();[print(s.get('name'),c.get('covered')+'/'+str(int(c.get('missed'))+int(c.get('covered')))) for s in r.iter('sourcefile') for c in s.findall('counter') if c.get('type')=='LINE']"
```

Expected: `NativeLinks.kt 44/44`; `PrBootstrap.kt 8/70` (`bootstrapEnv` covered; `run`, the asset copy and `bootstrap.sh` need a device and are checked in Task 7).

- [ ] **Step 9: Run the app's tests, build the APK and check the assets**

```bash
cd android && ./gradlew :app:testDebugUnitTest assembleDebug
unzip -l app/build/outputs/apk/debug/app-debug.apk | grep 'assets/'
```

Expected: `BUILD SUCCESSFUL`; `DistroOperationsTest` 27, `InstalledContainersTest` 2; the listing shows `assets/bin/busybox.applets` and `assets/scripts/bootstrap.sh`.

- [ ] **Step 10: Commit**

```bash
git add android/app android/session-core
git commit -F - <<'EOF'
chore: move the bootstrap into a reusable PrBootstrap

PrBootstrap(context, host, version) prepares the prefix directories,
the links to the native libraries and busybox applets, and runs
bootstrap.sh, which moves to :session-core's assets with
busybox.applets. pr keeps its preferences and bootstrap version 8.
EOF
```

---

### Task 5: Command sessions

**Files:**
- Create: `android/session-core/src/main/java/id/or/oo/pr/session/Bind.kt`, `.../session/CommandSessionFactory.kt`
- Test: `android/session-core/src/test/java/id/or/oo/pr/session/CommandSessionTest.kt`
- Modify: `android/session-core/src/main/java/id/or/oo/pr/session/SessionManager.kt`

**Interfaces:**
- Consumes: `ProotLauncher(host: ProotHost).startCustomSession(args: List<String>, rows: Int = 24, cols: Int = 80): ProotLauncher.Session?`, `ProotPty(session: ProotLauncher.Session)`, `ManagedSession(...)`, test fixtures `FakePty`, `eventually`.
- Produces:
  - `data class Bind(val hostPath: String, val guestPath: String)`
  - `class CommandSessionFactory(host: ProotHost, command: String, sink: (ByteArray, Int, Int) -> Unit, binds: List<Bind> = emptyList(), start: (List<String>) -> Pty? = { args -> ProotLauncher(host).startCustomSession(args)?.let { ProotPty(it) } }) : SessionManager.SessionFactory`
  - `internal fun loginArgs(prefixDir: File, alias: String, command: String, binds: List<Bind>): List<String>`
  - `SessionManager.open(alias: String, factory: SessionFactory = this.factory): OpenResult`

How the two paths coexist: `SessionManager` is still built with one default factory (pr: `ProotSessionFactory`), and pr's `TerminalActivity` and `DistroOperations` still call `open(alias)`, which uses it, so pr's interactive path is unchanged. A host that wants a command session calls `sessions.open(alias, CommandSessionFactory(host, command, sink, binds))`. Both kinds live in the same map, one session per alias: an alias already running is returned whichever factory asks. A command session has `emulator == null`; its reader thread drains output into `sink`, which receives `ManagedSession`'s reused 8 KiB buffer and must copy what it keeps. End, End all, the running notification, the ended notification and the drain all go through `ManagedSession` and `ProotPty` exactly as for a terminal.

- [ ] **Step 1: Write the failing test**

`android/session-core/src/test/java/id/or/oo/pr/session/CommandSessionTest.kt`:

```kotlin
package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

class CommandSessionTest {

    private val host = object : ProotHost {
        override val prefixDir = File("/data/data/org.example.gateway/files/usr")
        override val homeDir = File("/data/data/org.example.gateway/files/home")
        override val packageName = "org.example.gateway"
        override val cacheDir = File("/data/data/org.example.gateway/cache")
    }

    private val command = "httpd -f -p 8080 && echo 'server stopped'"
    private val binds = listOf(
        Bind("/storage/emulated/0/Download", "/root/Download"),
        Bind("/data/data/org.example.host/files/home/srv", "/srv"),
    )

    private val started = CopyOnWriteArrayList<List<String>>()
    private val ptys = CopyOnWriteArrayList<FakePty>()
    private val received = StringBuffer()
    private val ended = CopyOnWriteArrayList<Pair<String, Int?>>()

    private val sessions = SessionManager(
        { alias, onEnded ->
            val pty = FakePty().also { ptys += it }
            ManagedSession(alias, pty, null, { _, _, _ -> }, onEnded, exitCodeWaitMs = 100)
        },
        object : SessionManager.ForegroundController {
            override fun start() {}
        },
    ) { alias, code -> ended += alias to code }

    private fun factory(start: (List<String>) -> Pty? = { args -> started += args; FakePty().also { ptys += it } }) =
        CommandSessionFactory(host, command, { buf, off, len -> received.append(String(buf, off, len)) }, binds, start)

    private fun openCommand(): ManagedSession =
        (sessions.open("debian", factory()) as SessionManager.OpenResult.Opened).session

    @After
    fun tearDown() {
        ptys.forEach { it.exit(0) }
    }

    @Test
    fun theLoginArgumentsCarryEachBindAndTheCommandAsOneArgumentAfterTheSeparator() {
        assertEquals(
            listOf(
                "/data/data/org.example.gateway/files/usr/bin/pr-cli",
                "login",
                "debian",
                "--user",
                "root",
                "--custom-bind",
                "/storage/emulated/0/Download:/root/Download",
                "--custom-bind",
                "/data/data/org.example.host/files/home/srv:/srv",
                "--",
                "httpd -f -p 8080 && echo 'server stopped'",
            ),
            loginArgs(host.prefixDir, "debian", command, binds),
        )
    }

    @Test
    fun withoutBindsTheSeparatorFollowsTheUser() {
        assertEquals(
            listOf("/data/data/org.example.gateway/files/usr/bin/pr-cli", "login", "alpine", "--user", "root", "--", "top -b"),
            loginArgs(host.prefixDir, "alpine", "top -b", emptyList()),
        )
    }

    @Test
    fun theFactoryStartsPrCliWithTheLoginArguments() {
        openCommand()

        assertEquals(listOf(loginArgs(host.prefixDir, "debian", command, binds)), started)
    }

    @Test
    fun aCommandSessionHasNoEmulatorAndItsOutputReachesTheSink() {
        val session = openCommand()

        ptys.single().emit("gateway listening on 8080\n")

        eventually { received.toString() == "gateway listening on 8080\n" }
        assertNull(session.emulator)
        assertEquals(listOf("debian"), sessions.running.value)
    }

    @Test
    fun endStopsACommandSessionTheWayItStopsATerminalSession() {
        openCommand()
        val pty = ptys.single()

        sessions.end("debian")

        eventually { sessions.running.value.isEmpty() }
        assertEquals(listOf("signal", "eof", "close"), pty.calls)
        assertTrue(ended.isEmpty())
    }

    @Test
    fun endAllStopsCommandAndTerminalSessionsTogether() {
        sessions.open("alpine")
        openCommand()

        sessions.endAll()

        eventually { sessions.running.value.isEmpty() }
        assertTrue(ptys.all { it.eofSent })
    }

    @Test
    fun aCommandThatExitsOnItsOwnPostsTheEndedNotification() {
        openCommand()

        ptys.single().exit(1)

        eventually { sessions.running.value.isEmpty() }
        assertEquals(listOf("debian" to 1), ended)
    }

    @Test
    fun aCommandThatCannotStartIsReportedAndNothingRuns() {
        val result = sessions.open("debian", factory { null })

        assertEquals(SessionManager.OpenResult.Failed("could not start a command session for debian"), result)
        assertTrue(sessions.running.value.isEmpty())
    }

    @Test
    fun terminalAndCommandSessionsRunSideBySide() {
        sessions.open("alpine")
        openCommand()

        assertEquals(listOf("alpine", "debian"), sessions.running.value)
        assertEquals(1, started.size)
    }

    @Test
    fun aRunningAliasIsReturnedWhicheverFactoryAsks() {
        val first = openCommand()

        val again = sessions.open("debian", factory())

        assertSame(first, (again as SessionManager.OpenResult.Opened).session)
        assertEquals(1, started.size)
    }
}
```

- [ ] **Step 2: Run it and expect it to fail**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest --tests 'id.or.oo.pr.session.CommandSessionTest'
```

Expected: `BUILD FAILED`, `Unresolved reference 'Bind'`, `Unresolved reference 'CommandSessionFactory'`, `Unresolved reference 'loginArgs'`, and `Too many arguments` for `sessions.open("debian", ...)`.

- [ ] **Step 3: Let `SessionManager.open` take a factory per call**

`android/session-core/src/main/java/id/or/oo/pr/session/SessionManager.kt`, before:

```kotlin
    fun open(alias: String): OpenResult {
```

after:

```kotlin
    fun open(alias: String, factory: SessionFactory = this.factory): OpenResult {
```

The body is unchanged; its `factory.create(alias, ::onEnded)` now uses the parameter.

- [ ] **Step 4: Write `Bind` and `CommandSessionFactory`**

`android/session-core/src/main/java/id/or/oo/pr/session/Bind.kt`:

```kotlin
package id.or.oo.pr.session

data class Bind(val hostPath: String, val guestPath: String)
```

`android/session-core/src/main/java/id/or/oo/pr/session/CommandSessionFactory.kt`:

```kotlin
package id.or.oo.pr.session

import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.engine.ProotLauncher
import java.io.File

class CommandSessionFactory(
    private val host: ProotHost,
    private val command: String,
    private val sink: (ByteArray, Int, Int) -> Unit,
    private val binds: List<Bind> = emptyList(),
    private val start: (List<String>) -> Pty? = { args ->
        ProotLauncher(host).startCustomSession(args)?.let { ProotPty(it) }
    },
) : SessionManager.SessionFactory {

    override fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession {
        val pty = start(loginArgs(host.prefixDir, alias, command, binds))
            ?: throw IllegalStateException("could not start a command session for $alias")
        return ManagedSession(alias = alias, pty = pty, emulator = null, output = sink, onEnded = onEnded)
    }
}

internal fun loginArgs(prefixDir: File, alias: String, command: String, binds: List<Bind>): List<String> =
    listOf(File(prefixDir, "bin/pr-cli").absolutePath, "login", alias, "--user", "root") +
        binds.flatMap { listOf("--custom-bind", "${it.hostPath}:${it.guestPath}") } +
        listOf("--", command)
```

pr-cli's `login` (`src/pr-cli/src/main.rs`, `Commands::Login`) already takes repeatable `--custom-bind` and a trailing command, which `login.rs` runs as `/bin/sh -l -c "<command>"`, so the command goes as one argument and is never wrapped in another `/bin/sh -c` (AGENTS.md, double shell wrapping). `startCustomSession` gives the same environment as `startSession`.

- [ ] **Step 5: Run the module's tests and expect them to pass**

```bash
cd android && ./gradlew :session-core:testDebugUnitTest
for f in session-core/build/test-results/testDebugUnitTest/*.xml; do grep -o 'testsuite name="[^"]*" tests="[0-9]*" skipped="[0-9]*" failures="[0-9]*" errors="[0-9]*"' "$f"; done
```

Expected: `BUILD SUCCESSFUL`; 123 tests, 0 failures: `CommandSessionTest` 10, `ManagedSessionTest` 23, `NativeLinksTest` 8, `PrBootstrapTest` 1, `PrCliCommandTest` 3, `ProcessRunnerTest` 8, `ProcStatTest` 11, `ProotProcessesTest` 15, `ServiceStateTest` 11, `SessionActionsTest` 2, `SessionManagerTest` 29 (unchanged), `SessionNotificationsTest` 2.

- [ ] **Step 6: Check coverage**

```bash
cd android && ./gradlew :session-core:createDebugUnitTestCoverageReport
python3 -c "import xml.etree.ElementTree as E;r=E.parse('session-core/build/reports/coverage/test/debug/report.xml').getroot();[print(s.get('name'),c.get('covered')+'/'+str(int(c.get('missed'))+int(c.get('covered')))) for s in r.iter('sourcefile') for c in s.findall('counter') if c.get('type')=='LINE']"
```

Expected: `CommandSessionFactory.kt 12/14` (the default `start`, which needs a device, is the gap), `Bind.kt 1/1`, `SessionManager.kt 87/88`, `ManagedSession.kt 90/90`, and Tasks 3 and 4's numbers unchanged.

- [ ] **Step 7: Run the app's tests and build the APK**

```bash
cd android && ./gradlew :app:testDebugUnitTest assembleDebug
```

Expected: `BUILD SUCCESSFUL`; `DistroOperationsTest` 27, `InstalledContainersTest` 2.

- [ ] **Step 8: Commit**

```bash
git add android/session-core
git commit -F - <<'EOF'
feat: run a command as a headless session

CommandSessionFactory starts pr-cli login <alias> --user root, with
any --custom-bind pairs and the command as one argument after --,
through ProotLauncher.startCustomSession. The session has no emulator
and writes its output to a sink the host gives. SessionManager.open
takes a factory per call, so terminal and command sessions share one
manager, one session per alias, and end the same way.
EOF
```

**As executed (review fixes):**
- Key versus container: `CommandSessionFactory(host, container, command, sink, binds)` (its internal constructor adds the test seams `startSession`, `processes` and `pty`, which default to `commandPty`) and `loginArgs(prefixDir, container, command, binds)`. The key passed to `open(key, factory)` is only the session's name (the `ManagedSession` alias, the notifications, End), so a terminal for `debian` and a command session `web-server` in container `debian` run together. `SessionManager` records each key's factory class, and `open` returns `Failed("<key> is already open as another kind of session")` when a running or retained session under that key came from another factory class. pr's `open(alias)` is unchanged.
- End policy: `enum class EndPolicy { SPARE_LOGIN_SHELL, SIGNAL_EVERY_TRACEE }`, `ProotProcesses.endTargets(proot, policy = SPARE_LOGIN_SHELL)` and `ProotPty(session, processes, endPolicy = SPARE_LOGIN_SHELL)`. `CommandSessionFactory` selects `SIGNAL_EVERY_TRACEE`, so End also sends SIGTERM to proot's direct children: busybox `sh -c` execs the last command, which makes the server proot's direct child. Terminal sessions keep the old targets.
- The sink is called inside a catch that logs once; `Bind` requires absolute paths and no `:` in the host path; a blank command fails at construction; a missing `bin/pr-cli` makes `open` return `Failed`.
- Counts: session-core 143 tests (`BindTest` 6, `CommandSessionTest` 18, `CommandPtyTest` 2, `ProotProcessesTest` 19, the rest unchanged), app 29. Task 5's "123 tests" line (Step 5) is now 143.

---

### Task 6: CI and docs

**Files:**
- Modify: `.github/workflows/ci.yml`, `docs/android-apk-architecture.md`, `README.md`, `AGENTS.md`, `docs/design/session-core.md`

**Interfaces:** none (no code).

- [ ] **Step 1: Run the module's tests in CI**

`.github/workflows/ci.yml`, before:

```yaml
    - name: Run Android App Unit Tests
      run: |
        cd android
        ./gradlew :app:testDebugUnitTest

```

after:

```yaml
    - name: Run Android App Unit Tests
      run: |
        cd android
        ./gradlew :app:testDebugUnitTest

    - name: Run session-core Unit Tests
      run: |
        cd android
        ./gradlew :session-core:testDebugUnitTest

```

- [ ] **Step 2: Update the module tree in `docs/android-apk-architecture.md`**

Before (lines 14–52):

~~~text
android/                                    # Android project root
├── build.gradle.kts                        # Root: AGP 8.7.3, Kotlin 2.1.0, Compose plugin
├── settings.gradle.kts                     # Includes :app, :proot-engine and :termlib modules
├── app/
│   ├── build.gradle.kts                    # App module: Compose BOM 2025.05.00, :proot-engine and termlib deps
│   └── src/
│       ├── main/
│       │   ├── AndroidManifest.xml         # Permissions, activities, SessionService, extractNativeLibs
│       │   ├── assets/
│       │   │   ├── bin/busybox.applets     # Busybox applets App.kt symlinks at bootstrap
│       │   │   └── scripts/bootstrap.sh    # POSIX sh setup
│       │   ├── java/id/or/oo/pr/
│       │   │   ├── App.kt                  # Application: bootstrap; owns SessionManager and DistroOperations
│       │   │   ├── MainActivity.kt         # Compose: container list, output sheet of DistroOperations
│       │   │   ├── SessionBanners.kt       # Keep CPU awake switch, battery and background-error banners
│       │   │   ├── InstalledContainers.kt  # Which aliases are installed (OCI rootfs or legacy dir)
│       │   │   ├── DistroOperations.kt     # App-scoped install/remove/test runner
│       │   │   ├── PrCliCommand.kt         # Runs pr-cli for DistroOperations
│       │   │   ├── ProcessRunner.kt        # Child process: stdin closed, lines out, stopped on failure
│       │   │   ├── TerminalActivity.kt     # Compose: a view onto a ManagedSession
│       │   │   └── session/                # SessionManager, ManagedSession, SessionService
│       │   └── res/                        # Theme, icons, strings
│       └── test/                           # JVM unit tests (:app:testDebugUnitTest)
├── proot-engine/                           # Android library: ProotHost, ProotLauncher, PtyNative
~~~

and

~~~text
`cpp/`, `jniLibs/`, `ProotLauncher.kt` and `PtyNative.kt` moved from `app/` to `proot-engine/`
(`docs/design/proot-engine-library.md`), and pr-cli replaced `proot-distro.sh`.
~~~

After:

~~~text
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
│       └── testFixtures/                   # FakePty, eventually (also used by :app's tests)
├── proot-engine/                           # Android library: ProotHost, ProotLauncher, PtyNative
~~~

and

~~~text
`cpp/`, `jniLibs/`, `ProotLauncher.kt` and `PtyNative.kt` moved from `app/` to `proot-engine/`
(`docs/design/proot-engine-library.md`), and pr-cli replaced `proot-distro.sh`. `session/`,
`ProcessRunner.kt`, `PrCliCommand.kt`, the bootstrap in `App.kt` and the assets moved from `app/`
to `session-core/` (`docs/design/session-core.md`).
~~~

- [ ] **Step 3: Add the library section to `docs/android-apk-architecture.md`**

Insert before `## Runtime Directory Layout` (after the `---` that follows the note of Step 2):

~~~markdown
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

The notification actions are `<package>.action.END_ALL` and `<package>.action.TOGGLE_WAKE`.

Two kinds of session share one `SessionManager`, one per alias. `open(alias)` uses the factory the
manager was built with (pr's `ProotSessionFactory`: a terminal with a termlib emulator).
`open(alias, CommandSessionFactory(host, command, sink, binds))` runs
`pr-cli login <alias> --user root [--custom-bind host:guest]... -- '<command>'` with no emulator;
its output goes to `sink`, which gets a reused buffer and must copy what it keeps. Both kinds
appear in the running notification and end with End and End all in the same way.

---
~~~

- [ ] **Step 4: Fix the remaining paths in `docs/android-apk-architecture.md`**

Before:

~~~text
After first launch, App.kt creates this structure inside the app's private data directory:
~~~

After:

~~~text
After first launch, `PrBootstrap` (`:session-core`, run from `App.onCreate`) creates this structure inside the app's private data directory:
~~~

Before:

~~~text
All assets bundled in APK under `app/src/main/assets/`:
~~~

After:

~~~text
All assets bundled in APK under `assets/` (then in `app/`; today the only assets are `bin/busybox.applets` and `scripts/bootstrap.sh`, in `session-core/src/main/assets/`):
~~~

Before:

~~~text
Sessions live in `SessionManager` (`app/.../session/`), owned by `App`: one `ManagedSession` per
~~~

After:

~~~text
Sessions live in `SessionManager` (`:session-core`, package `id.or.oo.pr.session`), owned by `App`: one `ManagedSession` per
~~~

- [ ] **Step 5: Update `README.md`**

Before:

~~~markdown
CI runs both host-side unit suites (`cargo test` for pr-cli and `:app:testDebugUnitTest`) on pushes to `main` and on pull requests, then builds the APK.

### App unit tests (JVM)

```bash
cd android && ./gradlew :app:testDebugUnitTest
```

**128 tests**: session lifetime (`SessionManager`, `ManagedSession`, `ServiceState`, notifications), End's `/proc` scan through a fake process table (`ProotProcesses`), the list's operations (`DistroOperations`, `InstalledContainers`) and `ProcessRunner`, which runs real `/bin/sh` processes and skips on hosts without `/proc`. `./gradlew :proot-engine:testDebugUnitTest` runs the engine's 4 JVM tests.
~~~

After:

~~~markdown
CI runs the host-side unit suites (`cargo test` for pr-cli, `:app:testDebugUnitTest` and `:session-core:testDebugUnitTest`) on pushes to `main` and on pull requests, then builds the APK.

### Android unit tests (JVM)

```bash
cd android && ./gradlew :session-core:testDebugUnitTest :app:testDebugUnitTest
```

**143 tests in `:session-core`**: session lifetime (`SessionManager`, `ManagedSession`, `ServiceState`, notifications and their actions), command sessions (`CommandSessionFactory`), End's `/proc` scan through a fake process table (`ProotProcesses`), `PrCliCommand`, the bootstrap's links (`NativeLinks`) and `ProcessRunner`, which runs real `/bin/sh` processes and skips on hosts without `/proc`. **29 tests in `:app`**: the list's operations (`DistroOperations`, `InstalledContainers`). `./gradlew :proot-engine:testDebugUnitTest` runs the engine's 4 JVM tests.
~~~

Before:

~~~text
android/app/.../session/    # Background sessions: SessionManager, ManagedSession, SessionService
~~~

After:

~~~text
android/session-core/       # Android library: background sessions, command sessions, pr-cli runner, bootstrap
~~~

Before:

~~~markdown
| [`docs/design/proot-engine-library.md`](docs/design/proot-engine-library.md) | Architecture of the extracted `:proot-engine` library |
~~~

After:

~~~markdown
| [`docs/design/proot-engine-library.md`](docs/design/proot-engine-library.md) | Architecture of the extracted `:proot-engine` library |
| [`docs/design/session-core.md`](docs/design/session-core.md) | Design of the `:session-core` library and command sessions |
~~~

- [ ] **Step 6: Update `AGENTS.md`**

Before:

~~~markdown
CI runs `cargo test` (pr-cli) and `:app:testDebugUnitTest` on pushes to `main` and on pull requests.

### App unit tests (JVM)

```bash
cd android && ./gradlew :app:testDebugUnitTest
# 128 tests: sessions, End's /proc scan, DistroOperations, ProcessRunner, InstalledContainers
```
~~~

After:

~~~markdown
CI runs `cargo test` (pr-cli), `:app:testDebugUnitTest` and `:session-core:testDebugUnitTest` on pushes to `main` and on pull requests.

### Android unit tests (JVM)

```bash
cd android && ./gradlew :session-core:testDebugUnitTest :app:testDebugUnitTest
# session-core, 143 tests: sessions, command sessions, End's /proc scan, PrCliCommand, ProcessRunner, bootstrap links
# app, 29 tests: DistroOperations, InstalledContainers
```
~~~

- [ ] **Step 7: Mark the design implemented**

`docs/design/session-core.md`, before:

~~~markdown
**Status:** design approved; not implemented
~~~

after:

~~~markdown
**Status:** design approved; implemented (`docs/plans/session-core.md`)
~~~

- [ ] **Step 8: Check that no current doc names the old paths**

```bash
grep -n 'app/\.\.\./session\|app/src/main/assets\|128 tests\|App unit tests' README.md AGENTS.md docs/android-apk-architecture.md
```

Expected: no output. (The historical plans `docs/plans/background-sessions*.md` keep their paths: they are records, marked as such.)

- [ ] **Step 9: Run everything CI runs on the Android side**

```bash
cd android && ./gradlew :app:testDebugUnitTest :session-core:testDebugUnitTest assembleDebug
```

Expected: `BUILD SUCCESSFUL`; 29 + 143 = 172 tests, 0 failures.

- [ ] **Step 10: Commit**

```bash
git add .github/workflows/ci.yml docs/android-apk-architecture.md README.md AGENTS.md docs/design/session-core.md
git commit -F - <<'EOF'
docs: describe :session-core and run its tests in CI

CI runs :session-core:testDebugUnitTest after the app's tests. The
architecture doc, README and AGENTS.md give the new module layout,
the test counts and how another app includes the module and declares
the service; the design is marked implemented.
EOF
```

---

### Task 7: Phone check (design §6)

**Files:** none. **Interfaces:** none. No commit; put the results in the pull request.

A person runs this with one arm64 phone connected (`adb devices` lists exactly one), on which pr is installed with at least one container (for example Alpine).

- [ ] **Step 1: Build and install**

```bash
(cd android && ./gradlew assembleDebug)
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell am force-stop id.or.oo.pr
adb logcat -c
adb shell am start -n id.or.oo.pr/.MainActivity
```

Expected: the container list opens; `adb logcat -d -b crash` prints nothing.

- [ ] **Step 2: Check the upgrade path of the bootstrap**

```bash
adb logcat -d -s PR | grep -E 'Already bootstrapped|Running bootstrap|Bootstrap complete'
adb shell run-as id.or.oo.pr ls -l files/usr/bin/busybox files/usr/bin/proot files/usr/bin/pr-cli files/usr/bin/sh
```

Expected: `Already bootstrapped (version 8)`; `busybox`, `proot` and `pr-cli` link to `lib*.so` in the app's native library directory, and `sh -> busybox`.

- [ ] **Step 3: Check a full bootstrap run**

```bash
adb shell am force-stop id.or.oo.pr
adb shell run-as id.or.oo.pr rm shared_prefs/pr_prefs.xml
adb logcat -c
adb shell am start -n id.or.oo.pr/.MainActivity
adb logcat -d -s PR | grep -E 'Running bootstrap|busybox applet|Installed: bootstrap.sh|\[bootstrap\]|Bootstrap complete|Bootstrap failed'
adb shell run-as id.or.oo.pr cat shared_prefs/pr_prefs.xml
```

Expected: `Running bootstrap...`, `Created 0 busybox applet symlinks` (they already exist), `Installed: bootstrap.sh`, `[bootstrap] Bootstrap complete`, `Bootstrap complete (...ms)`, no `Bootstrap failed`; the preferences hold `bootstrap_version` 8 and `bootstrapped` true. Installed containers are untouched.

- [ ] **Step 4: Open a terminal session and leave it in the background**

In the app, open the Alpine container's terminal, run `echo hi`, then press Home.

```bash
adb shell dumpsys activity services id.or.oo.pr | grep -E 'SessionService|isForeground'
adb shell dumpsys notification --noredact | grep -E 'android.title=.*session|android.text=.*alpine'
```

Expected: `ServiceRecord{... id.or.oo.pr/.session.SessionService}` with `isForeground=true`; the notification "1 session running" with text "alpine", pr's notification icon, and the actions End all and Keep CPU awake. Tapping its body opens the container list.

- [ ] **Step 5: Keep the CPU awake and let it sleep**

Tap Keep CPU awake in the notification.

```bash
adb shell dumpsys power | grep 'pr:sessions'
```

Expected: a `PARTIAL_WAKE_LOCK 'pr:sessions'` line; the notification text adds "CPU kept awake" and the action reads Let CPU sleep. Tap Let CPU sleep, run the command again: no output.

- [ ] **Step 6: End from the notification**

Tap End all in the notification.

```bash
adb shell dumpsys activity services id.or.oo.pr | grep SessionService
adb shell run-as id.or.oo.pr ps -A -o PID,NAME | grep -E 'proot|pr-cli'
```

Expected: within about 20 s the notification goes, both commands print nothing, and no "session ended" notification appears (End was asked for).

- [ ] **Step 7: The ended notification**

Open the Alpine terminal again, run `sleep 5; exit`, and press Home within 5 s.

```bash
adb shell dumpsys notification --noredact | grep 'android.title=alpine: session ended'
```

Expected: `alpine: session ended (exit 0)` with pr's icon; tapping it opens the container list.

- [ ] **Step 8: End from the terminal's Back prompt**

Open the Alpine terminal, press Back, choose End session.

Expected: the list shows again and the running notification goes; `adb shell dumpsys activity services id.or.oo.pr | grep SessionService` prints nothing.

- [ ] **Step 9: Install and remove a container**

In the list, install a catalog container that is not installed (for example Alpine edge); wait for "Done.".

```bash
adb shell run-as id.or.oo.pr ls files/usr/var/lib/pr/containers
```

Expected: its alias is listed. Remove it from its menu; wait for "Done."; run the command again: the alias is gone.

- [ ] **Step 10: No crashes**

```bash
adb logcat -d -b crash
```

Expected: no output.

---

## Self-Review Against the Design

| Design section | Covered by |
|----------------|-----------|
| §1 Goal: generic session code in `:session-core`, pr unchanged, one new capability | Tasks 1–5; pr's behaviour checked by unchanged test counts (Tasks 1–5) and the phone check (Task 7) |
| §2 What moves: `SessionManager`, `ManagedSession`, `ServiceState`, `ProotProcesses` with `ProcFs`, `Pty`, `ProotPty` unchanged | Task 1 |
| §2 `SessionService`, `SessionNotifications`, `AndroidSessionHooks` | Task 2 |
| §2 `ProotSessionFactory` unchanged | Task 1 |
| §2 `ProcessRunner`, package only | Task 1 (package), Task 3 (`CommandRunner`, needed for the fake runner) |
| §2 `PrCliCommand` and `DistroCommand` | Task 3 |
| §2 `PrBootstrap` and `bootstrap.sh` | Task 4 (also `busybox.applets`, which the bootstrap reads) |
| §2 Tests and fakes move; pr keeps `App`, `MainActivity`, `TerminalActivity`, `DistroOperations`, `InstalledContainers`, `SessionBanners` and their tests | Tasks 1–2 (`FakePty` and `eventually` as test fixtures) |
| §3 `SessionHost` from the `Application` | Task 2 |
| §3 Actions from the package name | Task 2 (`SessionActions`, `SessionActionsTest`) |
| §3 Icon and open intent from the host | Task 2 |
| §3 `PrCliCommand(host: ProotHost)` | Task 3 |
| §3 `PrBootstrap(context, host)` with the version passed in | Task 4 |
| §3 Notification texts unchanged; service declared in each app's manifest | Task 2 (texts kept, manifest unchanged), Task 6 (consumer manifest snippet) |
| §4 Command sessions through `startCustomSession`, no emulator, output to a sink, End and drain like a terminal | Task 5 |
| §4 No change to pr-cli or `:proot-engine` | Global Constraints; Task 5 uses existing `login` and `startCustomSession` |
| §5 `settings.gradle.kts` includes `:session-core`; `:app` depends on it | Task 1 |
| §5 CI runs `:session-core:testDebugUnitTest` | Task 6 |
| §5 arm64-v8a and minSdk 28 | Task 1 (`build.gradle.kts`) |
| §6 Moved tests pass unchanged, in the new module | Tasks 1–2 (counts per class; `ProcessRunnerTest` changes only its package line) |
| §6 New JVM tests: login arguments, headless output to the sink, actions from a package name, `PrCliCommand` with a fake host | Task 5, Task 5, Task 2, Task 3 |
| §6 Coverage at or above 80% on touched code | Tasks 3–5 coverage steps; device-only glue in Task 7 |
| §6 Phone check | Task 7 |
| §7 Out of scope: rs.oo.or.id, host-specific code, pr-cli, `:proot-engine` | Not touched; Task 6 documents how another app would include the module |
