# Background Sessions Implementation Plan

> Work through the tasks in order. Each step is a checkbox (`- [ ]`); each task ends with a commit.

**Goal:** Keep a container's terminal session running after its screen closes, under a foreground service, so the Hermes gateway can run 24/7 (design: `docs/design/background-sessions.md`).

**Architecture:** An app-wide `SessionManager` (owned by `App`) holds one `ManagedSession` per container: the PTY, the termlib emulator on its own `HandlerThread`, and a reader thread that always drains output. A thin `SessionService` (foreground, `specialUse`) only keeps the process alive, owns the notification and holds the optional wake lock. `TerminalActivity` becomes a view that attaches to a session and asks End / Keep running / Cancel on Back.

**Tech Stack:** Kotlin, Jetpack Compose (BOM 2025.05.00), kotlinx-coroutines 1.9.0 (`StateFlow`), termlib (`vendor/termlib`, read-only), `:proot-engine` (`ProotLauncher`, unchanged), JUnit 4.13.2 for JVM unit tests.

## Global Constraints

- `minSdk` 28, `targetSdk` 35, `compileSdk` 36 (unchanged).
- Foreground service type `specialUse` with a `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` property; never `dataSync`.
- One session per container alias; a session is shown by at most one screen.
- Only End (prompt or notification) and the program exiting end a session; closing a screen never does.
- "Keep CPU awake" is off by default, kept in memory only, and reset whenever the running count returns to 0.
- `vendor/termlib`, `ProotLauncher`, `PtyNative`, `ptyjni.c`, pr-cli and proot are not modified.
- Code style (AGENTS.md): no comments unless explicitly requested; commit types `feat|fix|chore|docs|test`, informative body, no `Co-authored-by` trailers.
- Do not mention device model numbers or serials in commits, docs or the PR.
- Build and test from `android/`: `./gradlew :app:testDebugUnitTest`, `./gradlew assembleDebug`.

---

## File Structure

| File | Responsibility |
|------|----------------|
| `android/app/src/main/java/id/or/oo/pr/session/Pty.kt` | `Pty` interface: what a session needs from a PTY |
| `android/app/src/main/java/id/or/oo/pr/session/ManagedSession.kt` | One session: reader thread, exit state, screen attachment, ending with fallbacks |
| `android/app/src/main/java/id/or/oo/pr/session/SessionManager.kt` | One session per alias; running list; foreground start and stop; ended notifications; keep-awake state |
| `android/app/src/main/java/id/or/oo/pr/session/ProotPty.kt` | `Pty` over `ProotLauncher.Session`; kills the login shell through `/proc` |
| `android/app/src/main/java/id/or/oo/pr/session/ProotSessionFactory.kt` | Starts a session: `ProotLauncher`, emulator on its own `HandlerThread` |
| `android/app/src/main/java/id/or/oo/pr/session/SessionNotifications.kt` | Channels, the ongoing notification, the "session ended" notification |
| `android/app/src/main/java/id/or/oo/pr/session/SessionService.kt` | Foreground service, notification actions, wake lock |
| `android/app/src/main/java/id/or/oo/pr/session/AndroidSessionHooks.kt` | Real `ForegroundController` and `EndedNotifier` |
| `android/app/src/main/java/id/or/oo/pr/SessionBanners.kt` | List-screen banners: battery exemption, background unavailable |
| `android/app/src/main/res/drawable/ic_notification.xml` | Monochrome notification icon |
| `android/app/src/test/java/id/or/oo/pr/session/*.kt` | JVM unit tests and `FakePty` |
| Modified: `App.kt`, `TerminalActivity.kt`, `MainActivity.kt`, `AndroidManifest.xml`, `app/build.gradle.kts`, `docs/android-apk-architecture.md` | Wiring, screen, list, manifest, build, docs |

---

### Task 1: `Pty` and `ManagedSession`

**Files:**
- Modify: `android/app/build.gradle.kts` (dependencies)
- Create: `android/app/src/main/java/id/or/oo/pr/session/Pty.kt`
- Create: `android/app/src/main/java/id/or/oo/pr/session/ManagedSession.kt`
- Create: `android/app/src/test/java/id/or/oo/pr/session/FakePty.kt`
- Create: `android/app/src/test/java/id/or/oo/pr/session/Eventually.kt`
- Test: `android/app/src/test/java/id/or/oo/pr/session/ManagedSessionTest.kt`

**Interfaces:**
- Produces:
  - `interface Pty { fun read(buf: ByteArray): Int; fun write(data: ByteArray): Int; fun resize(rows: Int, cols: Int); fun sendEof(); fun kill(); fun forceKill(); fun close(); val exitCode: Int? }`
  - `class ManagedSession(alias: String, pty: Pty, emulator: TerminalEmulator?, output: (ByteArray, Int, Int) -> Unit, onEnded: (ManagedSession) -> Unit, endGraceMs: Long = 2_000, exitCodeWaitMs: Long = 2_000, release: () -> Unit = {})` with `val alias`, `val emulator`, `val exit: StateFlow<ManagedSession.Exit?>`, `val endRequested: Boolean`, `val hasScreen: Boolean`, `fun start()`, `fun write(data: ByteArray)`, `fun resize(rows: Int, cols: Int)`, `fun attach(screen: Screen): Screen?`, `fun detach(screen: Screen)`, `fun end()`
  - `fun interface ManagedSession.Screen { fun close() }`, `data class ManagedSession.Exit(val code: Int?)`
  - Test helpers `FakePty`, `eventually(timeoutMs: Long = 3_000, condition: () -> Boolean)`

- [ ] **Step 1: Add the JVM test dependency**

In `android/app/build.gradle.kts`, inside `dependencies { … }`, after the coroutines lines, add:

```kotlin
    testImplementation("junit:junit:4.13.2")
```

- [ ] **Step 2: Write `Pty`**

`android/app/src/main/java/id/or/oo/pr/session/Pty.kt`:

```kotlin
package id.or.oo.pr.session

interface Pty {
    fun read(buf: ByteArray): Int
    fun write(data: ByteArray): Int
    fun resize(rows: Int, cols: Int)
    fun sendEof()
    fun kill()
    fun forceKill()
    fun close()
    val exitCode: Int?
}
```

`read` returns the number of bytes read, `0` when nothing was read, and `-1` once the terminal is closed. `kill` ends the program the session started; `forceKill` ends whatever is left.

- [ ] **Step 3: Write the test helpers**

`android/app/src/test/java/id/or/oo/pr/session/FakePty.kt`:

```kotlin
package id.or.oo.pr.session

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.LinkedBlockingQueue

class FakePty : Pty {
    private val chunks = LinkedBlockingQueue<ByteArray>()
    private val end = ByteArray(0)

    val written = CopyOnWriteArrayList<String>()
    val resizes = CopyOnWriteArrayList<Pair<Int, Int>>()

    @Volatile var ignoresEof = false
    @Volatile var ignoresKill = false
    @Volatile var eofSent = false
    @Volatile var killed = false
    @Volatile var forceKilled = false
    @Volatile var closed = false

    @Volatile
    override var exitCode: Int? = null

    fun emit(text: String) = chunks.put(text.toByteArray())

    fun exit(code: Int) {
        exitCode = code
        chunks.put(end)
    }

    override fun read(buf: ByteArray): Int {
        val chunk = chunks.take()
        if (chunk === end) return -1
        System.arraycopy(chunk, 0, buf, 0, chunk.size)
        return chunk.size
    }

    override fun write(data: ByteArray): Int {
        written += String(data)
        return data.size
    }

    override fun resize(rows: Int, cols: Int) {
        resizes += rows to cols
    }

    override fun sendEof() {
        eofSent = true
        if (!ignoresEof) exit(0)
    }

    override fun kill() {
        killed = true
        if (!ignoresKill) exit(137)
    }

    override fun forceKill() {
        forceKilled = true
        exit(137)
    }

    override fun close() {
        closed = true
    }
}
```

`android/app/src/test/java/id/or/oo/pr/session/Eventually.kt`:

```kotlin
package id.or.oo.pr.session

import org.junit.Assert.fail

fun eventually(timeoutMs: Long = 3_000, condition: () -> Boolean) {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (!condition()) {
        if (System.currentTimeMillis() > deadline) fail("condition not met within $timeoutMs ms")
        Thread.sleep(10)
    }
}
```

- [ ] **Step 4: Write the failing tests**

`android/app/src/test/java/id/or/oo/pr/session/ManagedSessionTest.kt`:

```kotlin
package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CopyOnWriteArrayList

class ManagedSessionTest {

    private val pty = FakePty()
    private val output = StringBuffer()
    private val ended = CopyOnWriteArrayList<ManagedSession>()

    private fun session(endGraceMs: Long = 100) = ManagedSession(
        alias = "alpine",
        pty = pty,
        emulator = null,
        output = { buf, off, len -> output.append(String(buf, off, len)) },
        onEnded = { ended += it },
        endGraceMs = endGraceMs,
        exitCodeWaitMs = 100,
    ).also { it.start() }

    @Test
    fun drainsOutputWithNoScreenAttached() {
        val s = session()
        pty.emit("hello ")
        pty.emit("world")
        eventually { output.toString() == "hello world" }
        assertFalse(s.hasScreen)
    }

    @Test
    fun programExitRecordsTheCodeAndClosesThePty() {
        val s = session()
        pty.exit(3)
        eventually { ended.isNotEmpty() }
        assertEquals(ManagedSession.Exit(3), s.exit.value)
        assertTrue(pty.closed)
        assertSame(s, ended.single())
        assertFalse(s.endRequested)
    }

    @Test
    fun endSendsEofAndMarksTheEndRequested() {
        val s = session()
        s.end()
        eventually { s.exit.value != null }
        assertTrue(pty.eofSent)
        assertFalse(pty.killed)
        assertTrue(s.endRequested)
    }

    @Test
    fun endKillsTheProgramWhenItIgnoresEof() {
        pty.ignoresEof = true
        val s = session()
        s.end()
        eventually { s.exit.value != null }
        assertTrue(pty.killed)
        assertFalse(pty.forceKilled)
        assertEquals(ManagedSession.Exit(137), s.exit.value)
    }

    @Test
    fun endForceKillsWhenTheKillDoesNotEndIt() {
        pty.ignoresEof = true
        pty.ignoresKill = true
        val s = session()
        s.end()
        eventually { s.exit.value != null }
        assertTrue(pty.forceKilled)
    }

    @Test
    fun inputAndResizeAfterExitAreIgnored() {
        val s = session()
        pty.exit(0)
        eventually { s.exit.value != null }
        s.write("ls\n".toByteArray())
        s.resize(30, 90)
        assertTrue(pty.written.isEmpty())
        assertTrue(pty.resizes.isEmpty())
    }

    @Test
    fun inputAndResizeReachThePtyWhileRunning() {
        val s = session()
        s.write("ls\n".toByteArray())
        s.resize(30, 90)
        assertEquals(listOf("ls\n"), pty.written)
        assertEquals(listOf(30 to 90), pty.resizes)
    }

    @Test
    fun attachReturnsTheScreenItReplaces() {
        val s = session()
        val first = ManagedSession.Screen {}
        val second = ManagedSession.Screen {}
        assertNull(s.attach(first))
        assertSame(first, s.attach(second))
        assertNull(s.attach(second))
        assertTrue(s.hasScreen)
    }

    @Test
    fun detachOnlyRemovesTheAttachedScreen() {
        val s = session()
        val first = ManagedSession.Screen {}
        val second = ManagedSession.Screen {}
        s.attach(first)
        s.attach(second)
        s.detach(first)
        assertTrue(s.hasScreen)
        s.detach(second)
        assertFalse(s.hasScreen)
    }
}
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests 'id.or.oo.pr.session.ManagedSessionTest'`
Expected: compilation FAILS with `Unresolved reference: ManagedSession`.

- [ ] **Step 6: Write `ManagedSession`**

`android/app/src/main/java/id/or/oo/pr/session/ManagedSession.kt`:

```kotlin
package id.or.oo.pr.session

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.connectbot.terminal.TerminalEmulator
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class ManagedSession(
    val alias: String,
    private val pty: Pty,
    val emulator: TerminalEmulator?,
    private val output: (ByteArray, Int, Int) -> Unit,
    private val onEnded: (ManagedSession) -> Unit,
    private val endGraceMs: Long = 2_000,
    private val exitCodeWaitMs: Long = 2_000,
    private val release: () -> Unit = {},
) {
    fun interface Screen {
        fun close()
    }

    data class Exit(val code: Int?)

    private val _exit = MutableStateFlow<Exit?>(null)
    val exit: StateFlow<Exit?> = _exit

    @Volatile
    var endRequested = false
        private set

    private val ended = CountDownLatch(1)
    private val screenLock = Any()
    private var screen: Screen? = null

    val hasScreen: Boolean
        get() = synchronized(screenLock) { screen != null }

    fun start() {
        thread(isDaemon = true, name = "pty-reader-$alias") { readLoop() }
    }

    fun write(data: ByteArray) {
        if (_exit.value == null) pty.write(data)
    }

    fun resize(rows: Int, cols: Int) {
        if (_exit.value == null) pty.resize(rows, cols)
    }

    fun attach(screen: Screen): Screen? = synchronized(screenLock) {
        val previous = this.screen
        this.screen = screen
        previous?.takeIf { it !== screen }
    }

    fun detach(screen: Screen) {
        synchronized(screenLock) {
            if (this.screen === screen) this.screen = null
        }
    }

    fun end() {
        if (_exit.value != null) return
        endRequested = true
        pty.sendEof()
        thread(isDaemon = true, name = "pty-end-$alias") {
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.kill()
            if (ended.await(endGraceMs, TimeUnit.MILLISECONDS)) return@thread
            pty.forceKill()
        }
    }

    private fun readLoop() {
        val buf = ByteArray(8192)
        while (true) {
            val n = try {
                pty.read(buf)
            } catch (e: Exception) {
                -1
            }
            if (n < 0) break
            if (n > 0) output(buf, 0, n)
        }
        val deadline = System.currentTimeMillis() + exitCodeWaitMs
        while (pty.exitCode == null && System.currentTimeMillis() < deadline) Thread.sleep(20)
        pty.close()
        release()
        _exit.value = Exit(pty.exitCode)
        ended.countDown()
        onEnded(this)
    }
}
```

The PTY is closed only here, by the reader thread after its last read, so the file descriptor can never be reused under a pending read.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests 'id.or.oo.pr.session.ManagedSessionTest'`
Expected: BUILD SUCCESSFUL, 9 tests passed.

- [ ] **Step 8: Commit**

```bash
git add android/app/build.gradle.kts android/app/src/main/java/id/or/oo/pr/session/Pty.kt android/app/src/main/java/id/or/oo/pr/session/ManagedSession.kt android/app/src/test/java/id/or/oo/pr/session/
git commit -m "feat: add ManagedSession, a terminal session that outlives its screen" -m "A ManagedSession owns a PTY and a reader thread that keeps draining output with no screen attached, records the exit code, tracks the one screen that shows it, and ends with EOF, then a kill of the program, then a forced kill, closing the PTY only after the reader's last read. JVM unit tests through a fake PTY."
```

---

### Task 2: `SessionManager`

**Files:**
- Create: `android/app/src/main/java/id/or/oo/pr/session/SessionManager.kt`
- Test: `android/app/src/test/java/id/or/oo/pr/session/SessionManagerTest.kt`

**Interfaces:**
- Consumes: `ManagedSession`, `ManagedSession.Screen`, `ManagedSession.Exit`, `FakePty`, `eventually` (Task 1).
- Produces:
  - `class SessionManager(factory: SessionFactory, foreground: ForegroundController, notifier: EndedNotifier)`
  - `fun interface SessionManager.SessionFactory { fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession }` (throws on failure; returns a session whose `start()` has not been called)
  - `interface SessionManager.ForegroundController { fun start(); fun stop() }`
  - `fun interface SessionManager.EndedNotifier { fun sessionEnded(alias: String, exitCode: Int?) }`
  - `sealed interface SessionManager.OpenResult` with `data class Opened(val session: ManagedSession)` and `data class Failed(val reason: String)`
  - `val running: StateFlow<List<String>>`, `val keepCpuAwake: MutableStateFlow<Boolean>`, `val backgroundError: StateFlow<String?>`
  - `fun open(alias: String): OpenResult`, `fun attach(session: ManagedSession, screen: ManagedSession.Screen)`, `fun detach(session: ManagedSession, screen: ManagedSession.Screen)`, `fun end(alias: String)`, `fun endAll()`

- [ ] **Step 1: Write the failing tests**

`android/app/src/test/java/id/or/oo/pr/session/SessionManagerTest.kt`:

```kotlin
package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

class SessionManagerTest {

    private val ptys = ConcurrentHashMap<String, FakePty>()
    private val created = AtomicInteger()
    private val foregroundCalls = CopyOnWriteArrayList<String>()
    private val endedNotices = CopyOnWriteArrayList<Pair<String, Int?>>()
    private var failNext = false
    private var foregroundFails = false

    private val factory = SessionManager.SessionFactory { alias, onEnded ->
        if (failNext) throw IllegalStateException("pr-cli not found")
        created.incrementAndGet()
        val pty = FakePty()
        ptys[alias] = pty
        ManagedSession(alias, pty, null, { _, _, _ -> }, onEnded, endGraceMs = 100, exitCodeWaitMs = 100)
    }

    private val foreground = object : SessionManager.ForegroundController {
        override fun start() {
            if (foregroundFails) throw IllegalStateException("not allowed")
            foregroundCalls += "start"
        }

        override fun stop() {
            foregroundCalls += "stop"
        }
    }

    private val manager = SessionManager(factory, foreground) { alias, code -> endedNotices += alias to code }

    private fun open(alias: String) =
        (manager.open(alias) as SessionManager.OpenResult.Opened).session

    @Test
    fun openingARunningContainerReturnsTheSameSession() {
        val first = open("alpine")
        val second = open("alpine")
        assertSame(first, second)
        assertEquals(1, created.get())
        assertEquals(listOf("alpine"), manager.running.value)
    }

    @Test
    fun foregroundStartsWithTheFirstSessionAndStopsWithTheLast() {
        open("alpine")
        open("ubuntu")
        assertEquals(listOf("start"), foregroundCalls)
        manager.end("alpine")
        eventually { manager.running.value == listOf("ubuntu") }
        assertEquals(listOf("start"), foregroundCalls)
        ptys.getValue("ubuntu").exit(0)
        eventually { manager.running.value.isEmpty() }
        assertEquals(listOf("start", "stop"), foregroundCalls)
    }

    @Test
    fun keepCpuAwakeResetsWhenTheLastSessionEnds() {
        open("alpine")
        manager.keepCpuAwake.value = true
        manager.endAll()
        eventually { manager.running.value.isEmpty() }
        assertEquals(false, manager.keepCpuAwake.value)
    }

    @Test
    fun programExitWithNoScreenPostsTheEndedNotice() {
        open("alpine")
        ptys.getValue("alpine").exit(1)
        eventually { endedNotices.isNotEmpty() }
        assertEquals(listOf("alpine" to 1), endedNotices)
    }

    @Test
    fun endingASessionPostsNoEndedNotice() {
        open("alpine")
        manager.end("alpine")
        eventually { manager.running.value.isEmpty() }
        Thread.sleep(50)
        assertTrue(endedNotices.isEmpty())
    }

    @Test
    fun anEndedSessionStaysWhileAScreenShowsIt() {
        val session = open("alpine")
        val screen = ManagedSession.Screen {}
        manager.attach(session, screen)
        ptys.getValue("alpine").exit(2)
        eventually { session.exit.value != null }
        assertTrue(endedNotices.isEmpty())
        assertTrue(manager.running.value.isEmpty())
        manager.detach(session, screen)
        val next = open("alpine")
        assertNotSame(session, next)
        assertEquals(2, created.get())
    }

    @Test
    fun attachingANewScreenClosesThePreviousOne() {
        val session = open("alpine")
        val closed = CountDownLatch(1)
        manager.attach(session, ManagedSession.Screen { closed.countDown() })
        manager.attach(session, ManagedSession.Screen {})
        assertEquals(0L, closed.count)
    }

    @Test
    fun aFailedStartIsReportedAndStartsNothing() {
        failNext = true
        val result = manager.open("alpine")
        assertEquals(SessionManager.OpenResult.Failed("pr-cli not found"), result)
        assertTrue(manager.running.value.isEmpty())
        assertTrue(foregroundCalls.isEmpty())
    }

    @Test
    fun aForegroundFailureIsReportedAndTheSessionStillOpens() {
        foregroundFails = true
        open("alpine")
        assertEquals(listOf("alpine"), manager.running.value)
        assertEquals("not allowed", manager.backgroundError.value)
    }

    @Test
    fun concurrentOpensCreateOneSession() {
        val start = CountDownLatch(1)
        val opened = CopyOnWriteArrayList<ManagedSession>()
        val threads = (1..8).map {
            thread {
                start.await()
                opened += open("alpine")
            }
        }
        start.countDown()
        threads.forEach { it.join() }
        assertEquals(1, created.get())
        assertEquals(1, opened.toSet().size)
    }

    @Test
    fun endAllEndsEverySession() {
        open("alpine")
        open("ubuntu")
        manager.endAll()
        eventually { manager.running.value.isEmpty() }
        assertTrue(ptys.values.all { it.eofSent })
    }

    @Test
    fun backgroundErrorIsNullWhenTheServiceStarts() {
        open("alpine")
        assertNull(manager.backgroundError.value)
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests 'id.or.oo.pr.session.SessionManagerTest'`
Expected: compilation FAILS with `Unresolved reference: SessionManager`.

- [ ] **Step 3: Write `SessionManager`**

`android/app/src/main/java/id/or/oo/pr/session/SessionManager.kt`:

```kotlin
package id.or.oo.pr.session

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

class SessionManager(
    private val factory: SessionFactory,
    private val foreground: ForegroundController,
    private val notifier: EndedNotifier,
) {
    fun interface SessionFactory {
        fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession
    }

    interface ForegroundController {
        fun start()
        fun stop()
    }

    fun interface EndedNotifier {
        fun sessionEnded(alias: String, exitCode: Int?)
    }

    sealed interface OpenResult {
        data class Opened(val session: ManagedSession) : OpenResult
        data class Failed(val reason: String) : OpenResult
    }

    private val lock = Any()
    private val sessions = LinkedHashMap<String, ManagedSession>()

    private val _running = MutableStateFlow<List<String>>(emptyList())
    val running: StateFlow<List<String>> = _running

    val keepCpuAwake = MutableStateFlow(false)

    private val _backgroundError = MutableStateFlow<String?>(null)
    val backgroundError: StateFlow<String?> = _backgroundError

    fun open(alias: String): OpenResult {
        val session = synchronized(lock) {
            val existing = sessions[alias]
            if (existing != null && existing.exit.value == null) return OpenResult.Opened(existing)
            if (existing != null) sessions.remove(alias)
            val created = try {
                factory.create(alias, ::onEnded)
            } catch (e: Exception) {
                return OpenResult.Failed(e.message ?: e.javaClass.simpleName)
            }
            sessions[alias] = created
            updateRunning()
            created
        }
        session.start()
        return OpenResult.Opened(session)
    }

    fun attach(session: ManagedSession, screen: ManagedSession.Screen) {
        session.attach(screen)?.close()
    }

    fun detach(session: ManagedSession, screen: ManagedSession.Screen) {
        synchronized(lock) {
            session.detach(screen)
            if (session.exit.value != null && !session.hasScreen && sessions[session.alias] === session) {
                sessions.remove(session.alias)
            }
        }
    }

    fun end(alias: String) {
        synchronized(lock) { sessions[alias] }?.end()
    }

    fun endAll() {
        synchronized(lock) { sessions.values.toList() }.forEach { it.end() }
    }

    private fun onEnded(session: ManagedSession) {
        var notify = false
        synchronized(lock) {
            if (sessions[session.alias] !== session) return
            if (!session.hasScreen) {
                sessions.remove(session.alias)
                notify = !session.endRequested
            }
            updateRunning()
        }
        if (notify) notifier.sessionEnded(session.alias, session.exit.value?.code)
    }

    private fun updateRunning() {
        val before = _running.value.size
        val now = sessions.values.filter { it.exit.value == null }.map { it.alias }
        _running.value = now
        if (before == 0 && now.isNotEmpty()) startForeground()
        if (before > 0 && now.isEmpty()) {
            keepCpuAwake.value = false
            foreground.stop()
        }
    }

    private fun startForeground() {
        try {
            foreground.start()
            _backgroundError.value = null
        } catch (e: Exception) {
            _backgroundError.value = e.message ?: e.javaClass.simpleName
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd android && ./gradlew :app:testDebugUnitTest`
Expected: BUILD SUCCESSFUL; `ManagedSessionTest` 9 and `SessionManagerTest` 12 tests passed.

- [ ] **Step 5: Commit**

```bash
git add android/app/src/main/java/id/or/oo/pr/session/SessionManager.kt android/app/src/test/java/id/or/oo/pr/session/SessionManagerTest.kt
git commit -m "feat: add SessionManager, one background session per container" -m "SessionManager keeps one ManagedSession per container alias, publishes the running aliases, starts the foreground service at 0 to 1 running sessions and stops it at 1 to 0 (resetting keep-awake), reports a foreground start failure instead of failing the session, posts a session-ended notice only for a program that exited with no screen attached and no End request, and keeps an ended session while a screen still shows it. Changes are serialised, so concurrent opens create one session."
```

---

### Task 3: Real PTY and session factory

**Files:**
- Create: `android/app/src/main/java/id/or/oo/pr/session/ProotPty.kt`
- Create: `android/app/src/main/java/id/or/oo/pr/session/ProotSessionFactory.kt`
- Test: `android/app/src/test/java/id/or/oo/pr/session/ProcStatTest.kt`

**Interfaces:**
- Consumes: `Pty`, `ManagedSession`, `SessionManager.SessionFactory` (Tasks 1-2); `ProotLauncher(host: ProotHost)`, `ProotLauncher.startSession(distroName: String): ProotLauncher.Session?`, `ProotLauncher.Session` (`pid`, `exitCode`, `read`, `write`, `resize`, `close`).
- Produces:
  - `class ProotPty(session: ProotLauncher.Session) : Pty`
  - `fun parentPid(stat: String): Int?` (top-level, `ProotPty.kt`)
  - `class ProotSessionFactory(host: ProotHost) : SessionManager.SessionFactory`

- [ ] **Step 1: Write the failing test**

`android/app/src/test/java/id/or/oo/pr/session/ProcStatTest.kt`:

```kotlin
package id.or.oo.pr.session

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProcStatTest {

    @Test
    fun readsTheParentPidAfterTheCommandName() {
        assertEquals(1794, parentPid("1804 (sh) S 1794 1804 1794 34816 1804 4194560 900"))
    }

    @Test
    fun handlesSpacesAndParenthesesInTheCommandName() {
        assertEquals(7, parentPid("42 (a (b) c) R 7 42 42 0 -1 4194304"))
    }

    @Test
    fun rejectsAMalformedLine() {
        assertNull(parentPid("garbage"))
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests 'id.or.oo.pr.session.ProcStatTest'`
Expected: compilation FAILS with `Unresolved reference: parentPid`.

- [ ] **Step 3: Write `ProotPty`**

`android/app/src/main/java/id/or/oo/pr/session/ProotPty.kt`:

```kotlin
package id.or.oo.pr.session

import android.os.Process
import id.or.oo.pr.engine.ProotLauncher
import java.io.File

class ProotPty(private val session: ProotLauncher.Session) : Pty {

    override fun read(buf: ByteArray): Int = session.read(buf)

    override fun write(data: ByteArray): Int = session.write(data)

    override fun resize(rows: Int, cols: Int) {
        session.resize(rows, cols)
    }

    override fun sendEof() {
        session.write(byteArrayOf(0x04))
    }

    override fun kill() {
        if (session.exitCode != null) return
        childrenOf(session.pid).forEach { Process.sendSignal(it, Process.SIGNAL_KILL) }
    }

    override fun forceKill() {
        if (session.exitCode == null) Process.sendSignal(session.pid, Process.SIGNAL_KILL)
    }

    override fun close() {
        session.close()
    }

    override val exitCode: Int?
        get() = session.exitCode

    private fun childrenOf(pid: Int): List<Int> {
        val listed = runCatching { File("/proc/$pid/task/$pid/children").readText() }
            .getOrNull()
            ?.trim()
            ?.split(' ')
            ?.mapNotNull { it.toIntOrNull() }
        if (!listed.isNullOrEmpty()) return listed
        return File("/proc").listFiles().orEmpty()
            .mapNotNull { it.name.toIntOrNull() }
            .filter { child -> runCatching { parentPid(File("/proc/$child/stat").readText()) }.getOrNull() == pid }
    }
}

fun parentPid(stat: String): Int? {
    val afterName = stat.substringAfterLast(')', missingDelimiterValue = "").trim()
    return afterName.split(' ').getOrNull(1)?.toIntOrNull()
}
```

`kill` signals the session's first tracee, the login shell, which is proot's only direct child: with `--kill-on-exit` proot then kills every process of the session and exits. `forceKill` signals proot itself, which only happens if that did not end the session.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests 'id.or.oo.pr.session.ProcStatTest'`
Expected: BUILD SUCCESSFUL, 3 tests passed.

- [ ] **Step 5: Write `ProotSessionFactory`**

`android/app/src/main/java/id/or/oo/pr/session/ProotSessionFactory.kt`:

```kotlin
package id.or.oo.pr.session

import android.os.HandlerThread
import androidx.compose.ui.graphics.Color
import id.or.oo.pr.engine.ProotHost
import id.or.oo.pr.engine.ProotLauncher
import org.connectbot.terminal.TerminalEmulatorFactory

class ProotSessionFactory(private val host: ProotHost) : SessionManager.SessionFactory {

    override fun create(alias: String, onEnded: (ManagedSession) -> Unit): ManagedSession {
        val session = ProotLauncher(host).startSession(alias)
            ?: throw IllegalStateException("could not start a terminal for $alias")
        val pty = ProotPty(session)
        val emulatorThread = HandlerThread("emulator-$alias").apply { start() }
        lateinit var managed: ManagedSession
        val emulator = TerminalEmulatorFactory.create(
            looper = emulatorThread.looper,
            initialRows = 24,
            initialCols = 80,
            defaultForeground = Color.White,
            defaultBackground = Color(0xFF1a1a2e),
            onKeyboardInput = { data -> managed.write(data) },
            onResize = { dims -> managed.resize(dims.rows, dims.columns) },
        )
        managed = ManagedSession(
            alias = alias,
            pty = pty,
            emulator = emulator,
            output = { buf, off, len -> emulator.writeInput(buf, off, len) },
            onEnded = onEnded,
            release = { emulatorThread.quitSafely() },
        )
        return managed
    }
}
```

- [ ] **Step 6: Build**

Run: `cd android && ./gradlew :app:compileDebugKotlin`
Expected: BUILD SUCCESSFUL.

- [ ] **Step 7: Commit**

```bash
git add android/app/src/main/java/id/or/oo/pr/session/ProotPty.kt android/app/src/main/java/id/or/oo/pr/session/ProotSessionFactory.kt android/app/src/test/java/id/or/oo/pr/session/ProcStatTest.kt
git commit -m "feat: start background sessions through ProotLauncher" -m "ProotPty adapts ProotLauncher.Session. Its kill signals the login shell, proot's only direct child, so proot's --kill-on-exit ends every process of the session; killing proot itself would leave its tracees running, so that is only the last resort. ProotSessionFactory runs each session's termlib emulator on its own HandlerThread, since termlib builds a snapshot on its looper after every burst of output."
```

---

### Task 4: Foreground service and notifications

**Files:**
- Create: `android/app/src/main/res/drawable/ic_notification.xml`
- Create: `android/app/src/main/java/id/or/oo/pr/session/SessionNotifications.kt`
- Create: `android/app/src/main/java/id/or/oo/pr/session/SessionService.kt`
- Create: `android/app/src/main/java/id/or/oo/pr/session/AndroidSessionHooks.kt`
- Modify: `android/app/src/main/AndroidManifest.xml`
- Modify: `android/app/src/main/java/id/or/oo/pr/App.kt`

**Interfaces:**
- Consumes: `SessionManager` and its nested interfaces (Task 2), `ProotSessionFactory` (Task 3).
- Produces:
  - `App.sessions: SessionManager` (lazy)
  - `object SessionNotifications { fun createChannels(context: Context); fun running(context: Context, aliases: List<String>, keepAwake: Boolean, exempt: Boolean): Notification; fun ended(context: Context, alias: String, exitCode: Int?): Notification; const val RUNNING_ID: Int }`
  - `class SessionService : Service` with `companion object { const val ACTION_END_ALL; const val ACTION_TOGGLE_WAKE }`
  - `class ServiceForegroundController(context: Context) : SessionManager.ForegroundController`
  - `class NotificationEndedNotifier(context: Context) : SessionManager.EndedNotifier`
  - `fun Context.isIgnoringBatteryOptimizations(): Boolean` (top-level in `AndroidSessionHooks.kt`)

- [ ] **Step 1: Add the notification icon**

`android/app/src/main/res/drawable/ic_notification.xml`:

```xml
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="24"
    android:viewportHeight="24">
    <path
        android:fillColor="#FFFFFFFF"
        android:pathData="M4,5L11,12L4,19L5.4,20.4L13.8,12L5.4,3.6ZM12,18L20,18L20,20L12,20Z" />
</vector>
```

- [ ] **Step 2: Write `SessionNotifications`**

`android/app/src/main/java/id/or/oo/pr/session/SessionNotifications.kt`:

```kotlin
package id.or.oo.pr.session

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import id.or.oo.pr.MainActivity
import id.or.oo.pr.R

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
        val title = if (aliases.size == 1) "1 session running" else "${aliases.size} sessions running"
        val details = buildList {
            add(aliases.joinToString(", "))
            if (keepAwake) add("CPU kept awake")
            if (keepAwake && !exempt) add("Deep sleep will pause sessions; allow in the app")
        }.joinToString(" · ")
        return NotificationCompat.Builder(context, CHANNEL_RUNNING)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(details)
            .setStyle(NotificationCompat.BigTextStyle().bigText(details))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openList(context))
            .addAction(0, "End all", serviceAction(context, SessionService.ACTION_END_ALL))
            .addAction(
                0,
                if (keepAwake) "Let CPU sleep" else "Keep CPU awake",
                serviceAction(context, SessionService.ACTION_TOGGLE_WAKE),
            )
            .build()
    }

    fun ended(context: Context, alias: String, exitCode: Int?): Notification {
        val code = exitCode?.toString() ?: "unknown"
        return NotificationCompat.Builder(context, CHANNEL_ENDED)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("$alias: session ended (exit $code)")
            .setContentIntent(openList(context))
            .setAutoCancel(true)
            .build()
    }

    private fun openList(context: Context): PendingIntent = PendingIntent.getActivity(
        context,
        0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE,
    )

    private fun serviceAction(context: Context, action: String): PendingIntent = PendingIntent.getService(
        context,
        action.hashCode(),
        Intent(context, SessionService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE,
    )
}
```

- [ ] **Step 3: Write `SessionService`**

`android/app/src/main/java/id/or/oo/pr/session/SessionService.kt`:

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
import id.or.oo.pr.App
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch

class SessionService : Service() {

    companion object {
        const val ACTION_END_ALL = "id.or.oo.pr.action.END_ALL"
        const val ACTION_TOGGLE_WAKE = "id.or.oo.pr.action.TOGGLE_WAKE"
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var wakeLock: PowerManager.WakeLock? = null
    private val sessions: SessionManager get() = (application as App).sessions

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        SessionNotifications.createChannels(this)
        scope.launch {
            combine(sessions.running, sessions.keepCpuAwake) { running, awake -> running to awake }
                .collect { (running, awake) ->
                    setWakeLock(awake && running.isNotEmpty())
                    if (running.isNotEmpty()) {
                        getSystemService(NotificationManager::class.java)
                            .notify(SessionNotifications.RUNNING_ID, notification(running, awake))
                    }
                }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val running = sessions.running.value
        ServiceCompat.startForeground(
            this,
            SessionNotifications.RUNNING_ID,
            notification(running, sessions.keepCpuAwake.value),
            if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0,
        )
        when (intent?.action) {
            ACTION_END_ALL -> sessions.endAll()
            ACTION_TOGGLE_WAKE -> sessions.keepCpuAwake.value = !sessions.keepCpuAwake.value
        }
        if (running.isEmpty()) stopSelf()
        return START_NOT_STICKY
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

`startForeground` is called on every start before anything else, because a service started with `startForegroundService` must call it even when it then stops.

- [ ] **Step 4: Write the Android hooks**

`android/app/src/main/java/id/or/oo/pr/session/AndroidSessionHooks.kt`:

```kotlin
package id.or.oo.pr.session

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

class ServiceForegroundController(private val context: Context) : SessionManager.ForegroundController {
    override fun start() {
        ContextCompat.startForegroundService(context, Intent(context, SessionService::class.java))
    }

    override fun stop() {
        context.stopService(Intent(context, SessionService::class.java))
    }
}

class NotificationEndedNotifier(private val context: Context) : SessionManager.EndedNotifier {
    override fun sessionEnded(alias: String, exitCode: Int?) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) return
        SessionNotifications.createChannels(context)
        NotificationManagerCompat.from(context)
            .notify("ended:$alias".hashCode(), SessionNotifications.ended(context, alias, exitCode))
    }
}

fun Context.isIgnoringBatteryOptimizations(): Boolean =
    getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(packageName)
```

- [ ] **Step 5: Register the service and permissions**

In `android/app/src/main/AndroidManifest.xml`, after the existing `FOREGROUND_SERVICE` permission line, add:

```xml
    <uses-permission android:name="android.permission.FOREGROUND_SERVICE_SPECIAL_USE" />
    <uses-permission android:name="android.permission.WAKE_LOCK" />
    <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />
    <uses-permission android:name="android.permission.REQUEST_IGNORE_BATTERY_OPTIMIZATIONS" />
```

and, inside `<application>`, after the `TerminalActivity` element, add:

```xml
        <service
            android:name=".session.SessionService"
            android:exported="false"
            android:foregroundServiceType="specialUse">
            <property
                android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"
                android:value="Linux terminal sessions the user started, kept running in the background" />
        </service>
```

- [ ] **Step 6: Own the manager in `App`**

In `android/app/src/main/java/id/or/oo/pr/App.kt`, add the imports:

```kotlin
import id.or.oo.pr.session.NotificationEndedNotifier
import id.or.oo.pr.session.ProotSessionFactory
import id.or.oo.pr.session.ServiceForegroundController
import id.or.oo.pr.session.SessionManager
```

and, after `override val cacheDir …`, add:

```kotlin
    val sessions: SessionManager by lazy {
        SessionManager(
            ProotSessionFactory(this),
            ServiceForegroundController(this),
            NotificationEndedNotifier(this),
        )
    }
```

- [ ] **Step 7: Build and run the unit tests**

Run: `cd android && ./gradlew :app:testDebugUnitTest assembleDebug`
Expected: BUILD SUCCESSFUL; 24 unit tests passed.

- [ ] **Step 8: Commit**

```bash
git add android/app/src/main/res/drawable/ic_notification.xml android/app/src/main/java/id/or/oo/pr/session/SessionNotifications.kt android/app/src/main/java/id/or/oo/pr/session/SessionService.kt android/app/src/main/java/id/or/oo/pr/session/AndroidSessionHooks.kt android/app/src/main/AndroidManifest.xml android/app/src/main/java/id/or/oo/pr/App.kt
git commit -m "feat: keep sessions alive with a specialUse foreground service" -m "SessionService keeps the process alive while sessions run, shows the running-sessions notification with End all and Keep CPU awake, holds a partial wake lock while the toggle is on, and stops at zero sessions. A session that exits with no screen attached posts a session-ended notification when notifications are allowed. App owns the SessionManager. specialUse is used because Android 15 limits dataSync to 6 hours a day."
```

---

### Task 5: `TerminalActivity` as a view onto a session

**Files:**
- Modify (rewrite): `android/app/src/main/java/id/or/oo/pr/TerminalActivity.kt`

**Interfaces:**
- Consumes: `App.sessions`, `SessionManager.open/attach/detach/end`, `SessionManager.OpenResult`, `ManagedSession` (`emulator`, `exit`), `ManagedSession.Screen`.
- Produces: nothing new; `MainActivity` still starts it with `putExtra("distro", alias)`.

- [ ] **Step 1: Replace `TerminalActivity.kt`**

The keyboard (IME) handling and the Back key routing are unchanged from main; what changes is that the session comes from `SessionManager`, closing the screen detaches, and Back asks first.

`android/app/src/main/java/id/or/oo/pr/TerminalActivity.kt`:

```kotlin
package id.or.oo.pr

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import id.or.oo.pr.session.ManagedSession
import id.or.oo.pr.session.SessionManager
import org.connectbot.terminal.SelectionController
import org.connectbot.terminal.Terminal

class TerminalActivity : ComponentActivity() {

    private var session: ManagedSession? = null
    private val screen = ManagedSession.Screen { finish() }
    private var initialImeRequested = false
    private var initialImeShown = false
    private var backPressStarted = false
    private var selectionController: SelectionController? = null
    private val showLeavePrompt = mutableStateOf(false)

    private val notificationPermission =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val alias = intent.getStringExtra("distro") ?: run {
            finish()
            return
        }
        val sessions = (application as App).sessions

        when (val result = sessions.open(alias)) {
            is SessionManager.OpenResult.Failed -> {
                setContent { StartFailed(alias, result.reason) }
                onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
                    override fun handleOnBackPressed() = finish()
                })
                return
            }
            is SessionManager.OpenResult.Opened -> {
                session = result.session
                sessions.attach(result.session, screen)
            }
        }
        val current = session!!
        requestNotificationPermissionOnce()

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (current.exit.value != null) finish() else showLeavePrompt.value = true
            }
        })

        setContent {
            val exit by current.exit.collectAsState()
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize(), color = Color(0xFF1a1a2e)) {
                    Box(modifier = Modifier.fillMaxSize()) {
                        Terminal(
                            terminalEmulator = current.emulator!!,
                            modifier = Modifier.fillMaxSize().systemBarsPadding(),
                            initialFontSize = 12.sp,
                            backgroundColor = Color(0xFF1a1a2e),
                            foregroundColor = Color.White,
                            keyboardEnabled = true,
                            showSoftKeyboard = true,
                            // The library moves focus to its Compose node first; re-target the IME after it.
                            onTerminalTap = { window.decorView.post { showKeyboard() } },
                            onSelectionControllerAvailable = { selectionController = it },
                        )
                        exit?.let { ended ->
                            Text(
                                text = "Session ended (exit ${ended.code ?: "unknown"})",
                                color = Color.White,
                                modifier = Modifier
                                    .align(Alignment.BottomCenter)
                                    .fillMaxWidth()
                                    .background(Color(0xCC000000))
                                    .navigationBarsPadding()
                                    .padding(12.dp),
                            )
                        }
                    }
                    if (showLeavePrompt.value) {
                        LeavePrompt(
                            onEnd = {
                                showLeavePrompt.value = false
                                sessions.end(alias)
                                finish()
                            },
                            onKeep = {
                                showLeavePrompt.value = false
                                finish()
                            },
                            onCancel = { showLeavePrompt.value = false },
                        )
                    }
                }
            }
        }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode != KeyEvent.KEYCODE_BACK) return super.dispatchKeyEvent(event)
        when (event.action) {
            KeyEvent.ACTION_DOWN -> if (event.repeatCount == 0) backPressStarted = true
            KeyEvent.ACTION_UP -> {
                if (backPressStarted && !event.isCanceled) {
                    val selection = selectionController
                    if (selection != null && selection.isSelectionActive) selection.clearSelection()
                    else onBackPressedDispatcher.onBackPressed()
                }
                backPressStarted = false
            }
        }
        return true
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && !initialImeRequested && session != null) {
            // Once per screen: with a hardware keyboard the IME never shows, and later focus gains
            // (returning from another app) must not force it open.
            initialImeRequested = true
            ensureInitialKeyboard(attempt = 0)
        }
    }

    // The terminal library asks for the IME once, 100 ms after composition. If the activity's open
    // transition has not finished, WindowManager drops that request (ImeTracker:
    // PHASE_WM_ABORT_SHOW_IME_POST_LAYOUT) and nothing asks again, so the keyboard never appears.
    // Re-ask from the focused window until the IME is actually visible once.
    private fun ensureInitialKeyboard(attempt: Int) {
        window.decorView.postDelayed({
            if (isFinishing || initialImeShown) return@postDelayed
            val imeVisible = ViewCompat.getRootWindowInsets(window.decorView)
                ?.isVisible(WindowInsetsCompat.Type.ime()) == true
            if (imeVisible) {
                initialImeShown = true
            } else if (attempt < 5) {
                showKeyboard()
                ensureInitialKeyboard(attempt + 1)
            }
        }, 300)
    }

    // The IME attaches to the library's input view (the one text editor in the window), not to
    // whatever Compose node holds focus.
    private fun showKeyboard() {
        val editor = findTextEditor(window.decorView) ?: return
        editor.requestFocus()
        WindowCompat.getInsetsController(window, editor).show(WindowInsetsCompat.Type.ime())
    }

    private fun findTextEditor(view: View): View? {
        if (view.onCheckIsTextEditor()) return view
        if (view is ViewGroup) {
            for (i in 0 until view.childCount) {
                findTextEditor(view.getChildAt(i))?.let { return it }
            }
        }
        return null
    }

    private fun requestNotificationPermissionOnce() {
        if (Build.VERSION.SDK_INT < 33) return
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        ) return
        val prefs = getSharedPreferences("pr_prefs", Context.MODE_PRIVATE)
        if (prefs.getBoolean("notifications_asked", false)) return
        prefs.edit().putBoolean("notifications_asked", true).apply()
        notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    override fun onDestroy() {
        session?.let { (application as App).sessions.detach(it, screen) }
        super.onDestroy()
    }
}

@androidx.compose.runtime.Composable
private fun LeavePrompt(onEnd: () -> Unit, onKeep: () -> Unit, onCancel: () -> Unit) {
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text("Leave the terminal?") },
        text = { Text("Keep running leaves the session working in the background. End session stops it and everything started in it.") },
        confirmButton = { TextButton(onClick = onKeep) { Text("Keep running") } },
        dismissButton = {
            Row {
                TextButton(onClick = onEnd) { Text("End session") }
                TextButton(onClick = onCancel) { Text("Cancel") }
            }
        },
    )
}

@androidx.compose.runtime.Composable
private fun StartFailed(alias: String, reason: String) {
    MaterialTheme {
        Surface(modifier = Modifier.fillMaxSize(), color = Color(0xFF1a1a2e)) {
            Box(modifier = Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                Text("Couldn't start $alias: $reason", color = Color.White)
            }
        }
    }
}
```

The three IME comments are carried over from main unchanged; no new comments are added.

- [ ] **Step 2: Build and run the unit tests**

Run: `cd android && ./gradlew :app:testDebugUnitTest assembleDebug`
Expected: BUILD SUCCESSFUL; 24 unit tests passed.

- [ ] **Step 3: Verify on the device**

Install and launch:

```bash
cd android && adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am force-stop id.or.oo.pr && adb shell monkey -p id.or.oo.pr -c android.intent.category.LAUNCHER 1
```

Check, recording each result for the PR:

1. Open alpine, type `echo ok`: the output shows and typing works, so the emulator on its own `HandlerThread` renders and takes input. If it does not render or input is lost, change `looper = emulatorThread.looper` in `ProotSessionFactory` to `looper = android.os.Looper.getMainLooper()`, remove the `HandlerThread` and its `release`, and note the fallback in the PR.
2. Back twice (keyboard, then prompt) → **Keep running**: the list shows; `adb shell run-as id.or.oo.pr sh -c 'ls /proc/*/cmdline'` still finds the session's proot (child of the app pid).
3. Reopen alpine: the earlier output is still on screen.
4. In alpine run `sleep 1000`, Back twice → **End session**: within 5 seconds no proot or `sleep` process of the app remains.
5. Open alpine, type `exit`: "Session ended (exit 0)" shows; one Back returns to the list without a prompt.
6. From the list, Home, open the notification, tap it, open alpine again: one terminal screen only (the previous one closed).

- [ ] **Step 4: Commit**

```bash
git add android/app/src/main/java/id/or/oo/pr/TerminalActivity.kt
git commit -m "feat: make the terminal screen a view onto a background session" -m "TerminalActivity gets its session from SessionManager, attaches to it (closing any other screen showing it) and detaches when it closes; closing the screen no longer ends the session. Back keeps its order (keyboard, then selection) and then asks End session / Keep running / Cancel; once the session has ended it shows the exit code and Back returns without asking. A session that cannot start shows the reason. The notification permission is requested once, the first time a session starts."
```

---

### Task 6: Running mark and banners in the container list

**Files:**
- Modify: `android/app/build.gradle.kts` (dependencies)
- Create: `android/app/src/main/java/id/or/oo/pr/SessionBanners.kt`
- Modify: `android/app/src/main/java/id/or/oo/pr/MainActivity.kt` (`DistroListScreen`, `DistroCard`, `onRemove`)

**Interfaces:**
- Consumes: `App.sessions` (`running`, `keepCpuAwake`, `backgroundError`, `end`), `Context.isIgnoringBatteryOptimizations()` (Task 4).
- Produces: `@Composable fun SessionBanners(app: App)`; `DistroCard` gains `isRunning: Boolean`.

- [ ] **Step 1: Add the lifecycle Compose dependency**

In `android/app/build.gradle.kts`, after `implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")`, add:

```kotlin
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
```

- [ ] **Step 2: Write `SessionBanners`**

`android/app/src/main/java/id/or/oo/pr/SessionBanners.kt`:

```kotlin
package id.or.oo.pr

import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import id.or.oo.pr.session.isIgnoringBatteryOptimizations

@SuppressLint("BatteryLife")
@Composable
fun SessionBanners(app: App) {
    val context = LocalContext.current
    val keepAwake by app.sessions.keepCpuAwake.collectAsState()
    val backgroundError by app.sessions.backgroundError.collectAsState()
    val resumes = remember { mutableIntStateOf(0) }
    LifecycleResumeEffect(Unit) {
        resumes.intValue++
        onPauseOrDispose {}
    }
    val exempt = remember(resumes.intValue, keepAwake) { context.isIgnoringBatteryOptimizations() }

    Column {
        if (keepAwake && !exempt) {
            Banner(
                text = "Deep sleep will pause background sessions until battery optimization is turned off for this app.",
                action = "Allow",
                onAction = {
                    context.startActivity(
                        Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                            .setData(Uri.parse("package:${context.packageName}"))
                    )
                },
            )
        }
        backgroundError?.let { reason ->
            Banner(text = "Background running unavailable: $reason", action = null, onAction = {})
        }
    }
}

@Composable
private fun Banner(text: String, action: String?, onAction: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
    ) {
        Column(modifier = Modifier.padding(12.dp)) {
            Text(text, style = MaterialTheme.typography.bodyMedium)
            if (action != null) TextButton(onClick = onAction) { Text(action) }
        }
    }
}
```

- [ ] **Step 3: Show the running mark and banners in `MainActivity`**

In `DistroListScreen` (`MainActivity.kt`), after `val sheetState = …`, add:

```kotlin
    val running by app.sessions.running.collectAsState()
```

with the import `import androidx.compose.runtime.collectAsState` if it is not already imported.

In the `LazyColumn`, before the existing `item { CustomImageInstallCard(…) }`, add:

```kotlin
            item { SessionBanners(app) }
```

In `items(distros) { distro -> DistroCard(…) }`, add the argument after `isLoading = loadingDistro == distro.alias,`:

```kotlin
                    isRunning = distro.alias in running,
```

Replace the start of `onRemove` so a running session ends before the container is removed. The current block starts:

```kotlin
                    onRemove = {
                        if (loadingDistro == null) {
                            loadingDistro = distro.alias
                            operationLabel = "Removing"
                            operationDistroName = distro.displayName
                            showOutput = true
                            outputLines = listOf("Removing ${distro.displayName}…")
                            scope.launch {
                                runDistroCommand(app, listOf("remove", distro.alias)) { line ->
```

Change the `scope.launch {` line and the line after it to:

```kotlin
                            scope.launch {
                                if (distro.alias in app.sessions.running.value) {
                                    app.sessions.end(distro.alias)
                                    withTimeoutOrNull(5_000) {
                                        app.sessions.running.first { distro.alias !in it }
                                    }
                                }
                                runDistroCommand(app, listOf("remove", distro.alias)) { line ->
```

with the imports `import kotlinx.coroutines.flow.first` and `import kotlinx.coroutines.withTimeoutOrNull`.

In `DistroCard`, add the parameter after `isLoading: Boolean,`:

```kotlin
    isRunning: Boolean,
```

and replace its status `Text(text = if (distro.isInstalled) "● Installed" else "○ Not installed", …)` with:

```kotlin
                Text(
                    text = when {
                        isRunning -> "● Running"
                        distro.isInstalled -> "● Installed"
                        else -> "○ Not installed"
                    },
                    style = MaterialTheme.typography.labelSmall,
                    color = if (distro.isInstalled)
                        MaterialTheme.colorScheme.primary
                    else
                        MaterialTheme.colorScheme.outline
                )
```

- [ ] **Step 4: Build and run the unit tests**

Run: `cd android && ./gradlew :app:testDebugUnitTest assembleDebug`
Expected: BUILD SUCCESSFUL; 24 unit tests passed.

- [ ] **Step 5: Verify on the device**

1. Open alpine, Keep running: the alpine row shows "● Running".
2. Notification → **Keep CPU awake**: the list (after returning to it) shows the battery banner; **Allow** opens the system dialog; after allowing, the banner disappears on return.
3. Remove alpine while it runs (overflow → Remove): the session ends first, then the removal output shows. Reinstall alpine afterwards (`pr-cli install docker.io/library/alpine:latest --override-alias alpine` from the list) for the later checks.

- [ ] **Step 6: Commit**

```bash
git add android/app/build.gradle.kts android/app/src/main/java/id/or/oo/pr/SessionBanners.kt android/app/src/main/java/id/or/oo/pr/MainActivity.kt
git commit -m "feat: show running sessions and background banners in the container list" -m "Containers with a running session show Running. When Keep CPU awake is on and the app is not exempt from battery optimization, a banner explains that Doze will pause sessions and opens the system's exemption dialog; a foreground start failure shows its reason. Removing a container ends its session first."
```

---

### Task 7: Docs and full verification

**Files:**
- Modify: `docs/android-apk-architecture.md` ("Session lifetime and Back", Follow-up Items)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Update `docs/android-apk-architecture.md`**

Replace the section `### Session lifetime and Back` (from its heading to the line before `---` / the next `##` heading) with:

```markdown
### Session lifetime and Back

Sessions live in `SessionManager` (`app/.../session/`), owned by `App`: one `ManagedSession` per
container holds the PTY, the termlib emulator on its own `HandlerThread`, and a reader thread that
keeps draining output with no screen attached. `TerminalActivity` attaches to a session and
detaches when it closes; a session is shown by at most one screen. `SessionService`, a `specialUse`
foreground service, keeps the process alive while any session runs and owns the notification (End
all, Keep CPU awake). Design: `docs/design/background-sessions.md`.

| Action | Session |
|--------|---------|
| Back (after the keyboard and any selection) | Asks: End session / Keep running / Cancel |
| Home, switching apps, swiping the app away | Keeps running |
| `exit` in the shell, or the program ends | Ends; with no screen attached, a "session ended" notification is posted |
| End session, or End all in the notification | Ends: EOF, then the login shell is killed (proot's `--kill-on-exit` takes the rest), then proot |
| Android kills the process | Every session ends; nothing is restored |

"Keep CPU awake" holds a partial wake lock (screen off). Doze ignores wake locks and suspends network
for apps that are not exempt from battery optimization, so a 24/7 session also needs the exemption,
which the container list offers.
```

and in Follow-up Items replace the "Background sessions" bullet with:

```markdown
- **Background sessions**: done (`docs/design/background-sessions.md`). Not covered: several sessions
  per container, start at boot, restoring sessions after the process is killed
```

- [ ] **Step 2: Run every suite**

```bash
cd android && ./gradlew :app:testDebugUnitTest :proot-engine:connectedDebugAndroidTest assembleDebug
cd ../src/pr-cli && cargo test
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test alpine
adb shell run-as id.or.oo.pr files/usr/bin/pr-cli test ubuntu
```

Expected: 24 unit tests; proot-engine 8 instrumented tests; 169 host tests; `pr-cli test` 55/55 on both. Then, from an in-app alpine terminal (copy `files/usr/tmp/pit` to `cache/pit` first): `/tmp/pit all > /tmp/pit.log 2>&1` → 55 passed.

- [ ] **Step 3: Device lifecycle checks (record results for the PR)**

1. In alpine: `while true; do date +%s >> /tmp/counter; sleep 1; done`, Back twice → Keep running; after 60 s `adb shell run-as id.or.oo.pr tail -1 cache/counter` keeps advancing.
2. Reopen alpine: the loop's screen is still there; Ctrl-C stops it.
3. Restart the loop, Keep running, swipe the app away from recents: the counter keeps advancing.
4. Notification → Keep CPU awake, screen off for 2 minutes: the counter keeps advancing. Then `adb shell dumpsys battery unplug && adb shell dumpsys deviceidle force-idle`: without the exemption the counter stops advancing; grant the exemption (banner → Allow), repeat: it keeps advancing. Finish with `adb shell dumpsys deviceidle unforce && adb shell dumpsys battery reset`.
5. Open ubuntu, run `sleep 5; exit 3`, then Back twice → Keep running before the 5 seconds pass. After 5 s, `adb shell dumpsys notification --noredact | grep "ubuntu: session ended (exit 3)"` finds the notification.
6. Two sessions running → notification End all: both end, the ongoing notification disappears (`adb shell dumpsys notification | grep sessions_running` finds nothing).
7. Revoke the notification permission (`adb shell pm revoke id.or.oo.pr android.permission.POST_NOTIFICATIONS`), run check 1 again: the counter still advances.

- [ ] **Step 4: Target check: the Hermes gateway**

This needs the model provider set up as in the Android phase (`adb reverse tcp:8317 tcp:8317` and the provider key in `/root/hermes-proot-data/.env` of the `hermes` container). The key was removed from the device afterwards: **stop and ask the user before putting it back**, and remove it again when the check is done.

1. Open the `hermes` container and start the check loop; each round starts the gateway if needed and makes one model call:

   ```sh
   while true; do echo "== $(date +%s)"; hermes chat --oneshot "Reply with the single word OK"; echo "rc=$?"; sleep 300; done >> /tmp/gateway-check.log 2>&1
   ```

2. Back twice → Keep running; notification → Keep CPU awake; the list banner → Allow (exemption granted).
3. `adb shell dumpsys battery unplug && adb shell dumpsys deviceidle force-idle`.
4. After 30 minutes: `adb shell run-as id.or.oo.pr cat cache/gateway-check.log` shows at least 6 rounds after the force-idle time, each with `OK` and `rc=0`.
5. `adb shell dumpsys deviceidle unforce && adb shell dumpsys battery reset`; reopen `hermes`, Ctrl-C the loop, End session; remove the key and the `adb reverse`.

- [ ] **Step 5: Commit, push and open the PR**

```bash
git add docs/android-apk-architecture.md
git commit -m "docs: describe background sessions in the APK architecture" -m "Session lifetime and Back now describes SessionManager, the foreground service, the Back prompt, how End stops a session, and why the 24/7 case needs the battery-optimization exemption."
git push -u origin feat/background-sessions
```

Write the PR body to `../pr-body.md` (outside the repository) with these sections, filled with the recorded results (no device model or serial):

```markdown
Implements `docs/design/background-sessions.md`, following `docs/plans/background-sessions.md`.

## What changes
- SessionManager / ManagedSession / SessionService / TerminalActivity / container list, one line each.

## Verification
- Unit tests: 24 passed (ManagedSession 9, SessionManager 12, ProcStat 3).
- Suites: proot-engine instrumented 8/8; host `cargo test` 169; `pr-cli test alpine` and `ubuntu` 55/55; in-app `pit all` 55/55.
- Device checks 1-7 (Task 7 Step 3), each with its result.
- Task 5 Step 3 checks, including whether the emulator runs on its own HandlerThread or fell back to the main looper.
- Gateway check: rounds, timestamps, results.
```

Then:

```bash
gh pr create --base main --head feat/background-sessions --title "feat: keep terminal sessions running in the background" --body-file ../pr-body.md
```
