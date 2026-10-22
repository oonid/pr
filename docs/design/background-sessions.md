# Design Spec: Background Sessions

**Date:** 2026-10-17
**Topic:** Keep terminal sessions running after leaving their screen
**Baseline:** tag `pre-background-sessions` (main `86d9e93`, before this work)

## 1. Goal & Context

A terminal session (the shell `pr-cli login` starts in a container, and everything started from
it) was owned by `TerminalActivity` before this work: the PTY, the `TerminalEmulator` holding the
screen, and the reader thread. Closing the screen ended the session; Back ran `cleanup()` (the
current behaviour is in `android-apk-architecture.md`, "Session lifetime and Back"). Because
pr-cli starts proot with `--kill-on-exit`, nothing started in the session outlives it.

The main use is a long-running server, such as an AI agent's gateway (Hermes is one example), that
must keep working with the screen off for hours. Ordinary terminal multitasking (leave a build running, come back later) comes with
it.

Success means: a session keeps running after leaving its screen and after swiping the app away
from recents, survives deep sleep when the user asks for it, can be reattached with its screen
contents, and ends only when the user ends it or its program exits.

## 2. Decisions

| Question | Decision | Alternatives considered |
|----------|----------|-------------------------|
| Main goal | A long-running server 24/7 first (e.g. an agent gateway such as Hermes); covers multitasking | Multitasking only |
| Sessions per container | One; open reattaches. More shells via tmux inside the container | Several, with a session list or tabs |
| Who owns sessions | An app-wide `SessionManager`; a thin foreground service keeps the process alive (approach B) | A: a bound service owns the sessions. C: sessions in a separate process with IPC |
| Back | Prompt every time: End session / Keep running / Cancel | Keep running plus a setting; End plus a separate Detach action |
| CPU in deep sleep | "Keep CPU awake" toggle in the notification and in the container list, off by default, not remembered; the battery-optimization exemption, which the app offers, guards against vendor battery savers | Always hold a wake lock; no wake lock |
| Service type | `specialUse` | `dataSync` (Android 15 limits it to 6 hours a day) |
| Session ended while away | Post a "session ended (exit N)" notification. Away means no visible screen: Home, switching apps, swiping the app away, or no screen at all | Silent |

Approach B follows Android's "Guide to app architecture": activities and services are entry points
the system can destroy, and state lives in a layer they use. Reliability is the same as A, since
both keep sessions in one process kept alive by the same foreground service; B avoids a service that
is both started and bound, and keeps the session logic testable on the JVM.

## 3. Components

### `SessionManager` (new, app module)

Owned by `App`, lives as long as the process. Holds at most one `ManagedSession` per container
alias.

- `open(alias): OpenResult` returns `Opened(session)` with the running session for the container,
  or starts one through `ProotLauncher.startSession`. A failure to start is returned as
  `Failed(reason)`, not thrown past the caller. A session that is ending is returned as
  `Ending(session)`; `open` never blocks, since it runs on the main thread.
- `attach(session, screen)` and `detach(session, screen)` connect a screen to a session;
  `setVisible(session, screen, visible)` records whether that screen is on screen.
  `detach(session, screen, retainEnded = true)` is for an activity being recreated: the session stays
  in the map even if it has ended, and `open` returns it until a screen attaches to it.
- `end(alias)` and `endAll()` close sessions.
- `running: StateFlow<List<String>>` (the aliases of running sessions) for the list screen and the
  notification; `keepCpuAwake` and `backgroundError` are state flows too.
- When the number of running sessions goes from 0 to 1 it starts `SessionService`. The service
  watches `running` and stops itself when it empties; the manager never stops it, so the stop
  always happens on the service's main thread after its `startForeground`.
- All changes to the session map are serialised: concurrent `open` and `end` for one container
  yield either the old session or a new one, never two.
- Holds only the application context.

Seams, so the logic runs in JVM unit tests (the real emulator uses native code):

- a session factory (real: `ProotSessionFactory`, which starts the PTY through `ProotLauncher` and
  creates the emulator through `TerminalEmulatorFactory`; tests: a fake PTY and no emulator);
- a foreground controller (real: starts `SessionService`);
- an ended notifier (real: posts the "session ended" notification).

### `ManagedSession` (new, app module)

What `TerminalActivity` held before this work, moved out of it:

- the `ProotLauncher.Session` (PTY master fd, child pid, reaper);
- the `TerminalEmulator`, which keeps the screen contents while no screen is attached;
- the reader thread, which keeps draining PTY output with no screen attached, so a program that
  writes a lot never blocks on a full PTY buffer;
- the emulator runs on the main looper (`TerminalEmulatorFactory.create(looper = …)`). termlib
  resizes on the caller's thread, the main thread, while it applies output on the emulator's
  looper, and the two are not synchronised; on a separate looper a resize during output can index
  past the new screen and crash the process. So a chatty program with no screen attached still
  costs main-thread time for its screen snapshots.

termlib keeps at most 1000 scrollback lines, so a long-lived emulator's memory is bounded.

It exposes `write`, `resize`, `hasScreen` (whether a screen is attached), `screenVisible` (whether
that screen is visible; a screen is visible when attached until it says otherwise) and the exit
state. When the child exits it records the exit code and notifies the manager. The manager stops
counting it as running at once, and drops it when no screen is attached: immediately if none is,
otherwise when that screen closes, so the screen can still show the exit code. The "session ended"
notification is posted when the user did not end it and no screen is visible.

### `SessionService` (new foreground service, type `specialUse`)

Holds no session state.

- Keeps the process alive while at least one session runs; returns `START_NOT_STICKY`. Every
  `onStartCommand` calls `startForeground` first, then stops the service if no session runs. When
  `running` empties after that, it removes its notification (`stopForeground`) and calls
  `stopSelfResult` with the last start id, so a start that arrives while it stops is not lost.
  These decisions live in `ServiceState`, a plain class that takes the start and running-list events
  and returns the actions (start foreground, notify, wake lock, stop); the service only carries them
  out, so they run in JVM tests.
- Owns the ongoing notification and its actions.
- Holds the partial wake lock while "Keep CPU awake" is on.

### `TerminalActivity` (changed)

A view onto a session. It calls `SessionManager.open(alias)`, draws the session's emulator, and
forwards keyboard input and resizes. It marks its screen visible in `onStart` and hidden in
`onStop`, except when it stops for a configuration change. Closing the screen detaches; it never ends
the session. When the activity is recreated by a configuration change it does not handle (dark mode,
for example), `onDestroy` detaches with `retainEnded`, and the new activity reattaches to the same
session, ended or not; the session stays retained until a screen attaches to it. A
session is shown by at most one screen: opening a container whose session another screen shows
attaches it to the new screen, and the old screen closes. Back
keeps its current order: the first press hides the keyboard if it is up, the next clears a text
selection if one is active, and after that it shows the End session / Keep running / Cancel
prompt. If the session has ended, Back returns to the list without a prompt.

### `DistroOperations` (new, app module)

Owned by `App` like `SessionManager`, so an install, remove or test started from the list survives
the list being recreated (rotation, dark mode). It runs one `pr-cli` operation at a time on its own
scope, through a `DistroCommand` seam (real: `PrCliCommand`, the former `runDistroCommand`), and
exposes it as `operation: StateFlow<Operation?>`: kind, alias, display name, output lines, running
or finished, exit code. An operation does not survive the process being killed: no foreground
service covers it, so Android may kill the process (and the `pr-cli` child) once the app is in the
background with no session running.

- `install`, `installCustom`, `remove` and `test` return `false` while an operation runs;
  `dismiss()` clears a finished operation and is refused while one runs.
- The output ends with "Done.", "Exit code: N" or "ERROR: …"; an invalid custom alias becomes a
  finished operation with the two error lines, and nothing runs.
- `remove` ends the container's running session first, as described in section 4.
- `cancel()` cancels the running operation and does nothing when none runs. The output ends with
  "Cancelled.", with no exit code, and a new operation can start. While `pr-cli` runs,
  `PrCliCommand` stops it and everything it started: `ProcessRunner` finds `pr-cli` among the
  app process's children by its exact command line in `/proc` (`android.os.Process.myPid()` is the
  parent; `java.lang.Process` has no public pid accessor) and its descendants through the
  same `/proc` parent links as End's fallback walk (`ProotProcesses`), sends them SIGTERM and
  destroys `pr-cli`, waits up to 2 s, then SIGKILLs whatever is left and destroys `pr-cli`
  forcibly. During Remove's wait for a session, cancelling stops the wait and `remove` never runs;
  the session's End keeps going on its own. If `pr-cli` has already exited when the cancel
  arrives, its result stands: the output ends with "Done." or "Exit code: N", not "Cancelled.".
- A cancelled install cleans up after itself when its alias is known (catalog installs always
  pass `--override-alias`; custom installs when an alias was given): if the container did not
  exist before the install started and exists once `pr-cli` has stopped, the same operation runs
  `pr-cli remove <alias>`, appends its output (and "Exit code: N" or "ERROR: …" if it fails), then
  "Cancelled.". `pr-cli` creates the container's `rootfs` directory first and has no SIGTERM
  handler, so without this the list would show a broken container as installed and a new install
  of the alias would fail with "already installed". "Exists" is the list's own test
  (`InstalledContainers`: the OCI container's `rootfs` directory or the legacy `installed-rootfs`
  entry). A custom install without an alias is not cleaned up: remove its partial container before
  installing it again. A custom install with a plugin name (not an OCI image) and an alias writes
  `etc/pr/<alias>.override.sh` before its rootfs, so a cancel in between can leave only that file;
  `pr-cli remove` does not delete it and it blocks a later install of that alias ("already
  exists"). Catalog and other OCI installs never take that path. A cancel during `pr-cli remove`
  may leave a half-removed container: run Remove again.
- A cancel during a layer download leaves at most `dlcache/sha256_<digest>.partial`. `pr-cli`
  downloads each layer to that name, renames it to `sha256_<digest>` only once its sha256 matches,
  overwrites a leftover `.partial`, and hashes a cached layer before reusing it; a damaged one is
  deleted and downloaded again ("Cached OCI layer <digest> is damaged; downloading again...").
  An OCI install that fails on its own (bad layer, missing `/etc`, config write) removes its
  container in-process before it exits, so the list does not show it.

### `MainActivity` (changed)

Containers with a running session show a "running" mark; tapping one reattaches. It shows the
banners described in sections 5 and 6. The output sheet, its busy state and the row spinner render
`DistroOperations.operation`; the sheet shows while there is an operation. While it runs, the
sheet state refuses to hide on a swipe-down or a scrim tap (`confirmValueChange`), Back does
nothing (`shouldDismissOnBackPress` is off: Material3's Back path hides the sheet without asking
the sheet state), Close is disabled and a Cancel button calls `cancel()`; if the sheet is hidden
anyway, the refused `dismiss()` shows it again. Once the operation has finished, swipe-down, a
scrim tap, Back and Close each call `dismiss()`; the sheet's header then reads "Completed", or
"Cancelled" after a cancel. The list refreshes when an operation starts or finishes and when the
sheet closes. Only the custom image fields are kept by the screen (`rememberSaveable`).

### Unchanged

`ProotLauncher`, `PtyNative`, `ptyjni.c` and proot. pr-cli changes only in its OCI layer
download and cleanup, described under `DistroOperations`.

## 4. Session Lifecycle

| Event | Behaviour |
|-------|-----------|
| Open a container with no session | `SessionManager.open` starts the shell; 0 to 1 starts the service and its notification; the screen attaches |
| Open a container whose session runs | The screen reattaches, shows the current contents, and resizes the session; a screen that showed it before closes |
| Open a container whose session is ending | Any other screen still on that session closes. The screen shows "Ending the previous session…", waits for that session's exit, then, once the screen is started (not while the app is in the background), opens again and starts a fresh session. If the session has not ended within End's timeout (`ManagedSession.END_TIMEOUT_MS`, 20 s), the screen shows "Couldn't stop the previous session."; Back returns to the list in either state |
| Back | Keyboard, then selection, then the prompt. End closes the session; Keep running returns to the list; Cancel stays |
| End (prompt, "End all", Remove) | SIGTERM and SIGCONT to every process in the session except the login shell (including daemons that left the shell's tree), found as the processes proot traces (`TracerPid:` in `/proc/<pid>/status`; if that cannot be read, the login shell's descendants found by walking `/proc`); if any was signalled, up to 10 s for them to exit or for the session to end, stopping as soon as they have. Then EOF to the shell; after 2 s the login shell is killed (proot's `--kill-on-exit` takes the rest); after 2 s more, every process proot traces (the login shell included; if that cannot be read, every descendant of proot) is SIGKILLed, then proot. Killing proot alone would detach tracees that cannot die yet (a shell in D state) and leave them holding the PTY open. A background program such as the gateway gets SIGTERM to shut down cleanly first. At worst 17.5 s (`ManagedSession.endWorstCaseMs()`): 10 s signal wait + 2 s EOF grace + 2 s kill grace + 2 s wait for the exit code + 0.5 s after killing a shell that left no exit code + 1 s for the `/proc` scans. Remove's wait and the "Ending…" give-up use End's timeout, that worst case plus 2.5 s (20 s) |
| Home, or switching apps | No prompt; the session keeps running; the screen stays attached but is not visible |
| A configuration change recreates the screen (dark mode, theme) | The new screen reattaches to the same session; an ended one still shows "Session ended (exit N)" |
| Swipe the app away from recents | Screens are destroyed; the foreground service keeps the process, so sessions keep running |
| The program exits (`exit`, crash) | The session is marked ended with its exit code and no longer counts as running. An attached screen shows "Session ended (exit N)", Back returns without a prompt, and the session is dropped when the screen closes; with no screen attached it is dropped at once. With no visible screen (no screen, or one hidden by Home) a "session ended" notification is posted |
| "End all" in the notification | Every session ends as with End |
| Remove a container whose session runs | The output shows "Stopping the <name> session…"; the session is ended first and removal waits up to End's timeout (20 s) for it to stop; if it still runs, removal is cancelled with "Couldn't stop the <name> session; removal cancelled." Cancel during the wait stops it and ends with "Cancelled." without removing anything; the session's End continues. The wait runs in `DistroOperations`, so recreating the list does not lose it |
| Cancel during an install, remove or test | `pr-cli` and every process it started are stopped (SIGTERM, up to 2 s, then SIGKILL) and the output ends with "Cancelled.". A cancelled install whose container is new and whose alias is known runs `pr-cli remove <alias>` first; a custom install without an alias leaves its partial container for the user to remove. A cancelled remove may leave a half-removed container: run Remove again |
| Last running session ends | The wake lock is released, the service stops, the ongoing notification goes away (even if a screen still shows an ended session) |
| The process is killed (memory, crash, app update) | All sessions end with it; nothing is restored and the service is not restarted |
| Phone reboot | Nothing restarts (start-at-boot is out of scope) |

Only End (prompt or notification) and the program exiting end a session; a screen closing never
does.

## 5. Notifications, Service Type and Permissions

### Manifest

- `FOREGROUND_SERVICE` (present), `FOREGROUND_SERVICE_SPECIAL_USE`, `WAKE_LOCK`,
  `POST_NOTIFICATIONS`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`.
- `SessionService` with `android:foregroundServiceType="specialUse"` and a
  `PROPERTY_SPECIAL_USE_FGS_SUBTYPE` property describing the use (user-started Linux terminal
  sessions). On Google Play this needs a justification; `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` is
  restricted there too. Sideloaded builds are not affected.

### Starting the service

`SessionManager.open` starts it while the user is opening a terminal, so the app is in the
foreground (Android 12+ forbids starting foreground services from the background).
`SessionService` calls `startForeground` at once.

### Notification permission

Requested at runtime (Android 13+) the first time a session starts. If denied, the service still
runs and sessions still survive; only the notification is hidden. The app does not ask again.
The running notification is posted again once the permission is granted, whether from the prompt or
in Settings on returning to the list.

### Channels

1. **Running sessions**: low importance, silent, ongoing. Title "N sessions running" ("Stopping…"
   for a start that finds no session and stops at once), text the container names, plus "CPU kept
   awake" when the toggle is on. A session whose End has started is left out of the count and named
   as "Stopping <name>…" until it exits ("alpine · Stopping web-server…"); when every session is
   ending, the title reads "Stopping 1 session…" or "Stopping N sessions…". Tapping opens the list.
   Actions: **End all**, and **Keep CPU awake** / **Let CPU sleep**.
2. **Session ended**: default importance. "alias: session ended (exit N)"; tapping opens the list.

Tapping either returns to the one existing list (`CLEAR_TOP | SINGLE_TOP`): a terminal on top of it
closes, which detaches its session, and the session keeps running.

## 6. Wake Lock and Battery Optimization

Two different things stop a background session, and they need different remedies:

- **CPU sleep with the screen off.** Without a wake lock the CPU suspends soon after the screen
  turns off. "Keep CPU awake" holds a `PARTIAL_WAKE_LOCK` in `SessionService` so the CPU keeps
  running while the screen is off.
- **Doze (deep sleep).** After the phone has been idle and unplugged for a while, Doze restricts
  wake locks and network access for idle apps, but a process running a foreground service keeps
  both. Measured on an Android 16 phone under `dumpsys deviceidle force-idle`, the session's
  counter and network kept working without the exemption. Vendor battery savers (for example a
  "sleeping apps" list) add their own limits and can still stop an app that is not exempt from
  battery optimization.

So the 24/7 gateway needs the toggle, and the exemption guards against vendor battery savers:

- The toggle is in the notification and, while a session runs, at the top of the container list,
  so it stays reachable when notifications are denied.
- The toggle is off by default and kept in memory only: each time sessions start from zero it is
  off, so a forgotten toggle cannot drain the battery for days. It is released when toggled off or
  when the last session ends.
- When the toggle is on and the app is not exempt, the notification text adds "Battery saver
  may stop sessions; allow in the app", and the list shows a banner ("Some phones' battery savers
  can still stop background sessions. Allow PR to run unrestricted.") with **Allow**, which opens the
  system's ignore-battery-optimizations dialog. The dialog opens from the app screen, because
  Android 12+ blocks starting activities from notification actions routed through a service.
  Once the exemption is granted, returning to the list restarts the service so the notification
  drops the battery note.

Risks that remain for a 24/7 session, outside the app's control:

- **Phantom process limit (Android 12+).** The system kills child processes of apps beyond 32
  across all apps, so a gateway that spawns many processes can lose some of them. On Android 14+
  the developer option "Disable child process restrictions" lifts the limit; on 12L and 13,
  `adb shell settings put global settings_enable_monitor_phantom_procs false` does; on 12 it takes
  `adb shell device_config put activity_manager max_phantom_processes 2147483647`.
- **Task Manager "Stop" (Android 13+).** Stopping the app from the notification shade's active-apps
  list (shown for apps with a foreground service) kills the process, and every session with it.

## 7. Error Handling

| Failure | Behaviour |
|---------|-----------|
| Session cannot start (pr-cli missing, PTY fork fails) | The terminal screen shows "Couldn't start alias: reason"; nothing is recorded; the service is not started |
| The foreground service cannot start | The session runs while the app is open but is not protected; the list shows "Background running unavailable: reason" until a later `startForeground` succeeds; no crash |
| Reader thread error with no exit code | Before the PTY closes, the reader waits up to 2 s for the exit code; if none arrives, the login shell is killed so that proot's `--kill-on-exit` takes the rest; if proot has not exited 0.5 s later, every tracee is SIGKILLed and then proot |
| Notification permission denied | Everything works; the notification is hidden; no repeated requests |
| Reader thread error or PTY hangup | Treated as the session ending; the exit code comes from the reaper |
| Input or resize for an ended session | Ignored |
| The system restarts the service | `START_NOT_STICKY`; with no sessions it stops itself |
| End and open race on one container | Serialised by `SessionManager` |
| The process is killed | Sessions end; documented, not handled |

## 8. Testing

### JVM unit tests (app module gains a JUnit setup)

`SessionManager` with fake seams:

- one session per container; `open` on a running container returns the same session;
- the manager starts the service at 0 to 1, and the service stops itself when the running list empties, through End, End all and program exit;
- the "session ended" notification fires only when no screen is visible, and never after End;
- a recreated screen gets its session back, even one that has ended;
- concurrent open and end for one container never produce two sessions;
- output keeps being drained with no screen attached.

`ProotProcesses`, End's process scan behind `ProotPty`, reads `/proc` through a small `ProcFs`
interface (the real `/proc` by default); a fake process table covers tracee selection, another
proot's tracees, a daemon reparented to init, the descendant-walk fallback, a pid vanishing
mid-scan, zombies counted as exited, and the force-kill order ending with proot.

`DistroOperations` with a fake command and a `SessionManager` on fake PTYs: output streams into
the state in order and ends with "Done.", "Exit code: N" or "ERROR: …"; a second operation and
`dismiss()` are refused while one runs; Remove without a session runs `remove`, with one ends it
first, and cancels with the exact text, never running `remove`, when it does not stop in time; the
custom install's arguments and its invalid-alias error. Cancel during a run ends with "Cancelled.",
no exit code, the command seeing the cancellation and a next operation able to start, also when
the command fails as it is cancelled or the cancel lands before the work has started; cancel
during Remove's wait never runs `remove`; cancel with nothing running does nothing. A cancelled
install runs `remove <alias>` for a container it created (catalog, or custom with an alias), and
nothing for a container that existed before, one that was never created, a custom install without
an alias, or a cancelled test. A command that finishes as it is cancelled keeps "Done." and its
exit code, with no "Cancelled.". `InstalledContainers` against a temporary prefix: the OCI `rootfs`
directory or the legacy directory marks a container installed; a container directory without
`rootfs`, a file and a blank alias do not.

`ProcessRunner`, which runs `pr-cli` for `PrCliCommand`, against real `/bin/sh` processes on the
host: output lines in order and the exit code, the environment, standard input closed (so a reader
of it does not block), a failing read that stops the process before the error is rethrown, and
cancellation stopping the process and its descendants, with SIGKILL after the wait for a tree that
ignores SIGTERM; an exit code already known when the cancellation arrives is returned, not lost,
and so is the exit of a process that ended before the cancel while a descendant still holds its
output (the process's own exit value, so a tree that Cancel killed never reads as a result).
`ProotProcesses` finds a child by its exact command line (not a zombie, not another `pr-cli`
command) and lists every descendant, with and without `children` files.

`ManagedSession` with a fake PTY: End's drain window stops as soon as the signalled programs
exit; the slowest End (every step ignored, no exit code) takes the sum of `endWorstCaseMs()`'s
steps, so a missing step fails.

### Existing suites

proot-engine instrumented tests (8), `pr-cli test alpine` and `pr-cli test ubuntu`, the full
integration suite from an in-app terminal, and the pr-cli host tests (178) stay green.

### Device lifecycle checks (adb; results recorded in the PR)

1. A counter writing a timestamp every second; Keep running; it keeps counting.
2. Reattach shows the screen contents.
3. Swipe the app away from recents; still counting.
4. Screen off with Keep CPU awake on: the counter continues. Then `dumpsys battery unplug` and
   `dumpsys deviceidle force-idle`: the counter and the network continue, with and without the
   exemption (a foreground-service process keeps its wake lock and network in Doze).
5. `exit` with no screen attached posts the "session ended" notification.
   Home, then `exit 3` in the shell before leaving (type `sleep 5; exit 3`, then press Home) posts
   the notice; returning to the terminal shows "Session ended (exit 3)".
6. End all ends every session and removes the ongoing notification.
7. With the notification permission denied, sessions still survive.
8. After `exit 2`, switching dark mode keeps the screen on the ended session ("Session ended
   (exit 2)"), not a fresh login.

### Target check

A long-running agent gateway in its container (the check used the Hermes gateway), Keep running,
Keep CPU awake on, the exemption granted, forced deep sleep: it keeps answering from the host through `adb forward` for at least 30
minutes, and its calls to the model provider keep working.

## 9. Out of Scope

- Several sessions per container (use tmux inside the container).
- Starting sessions or the gateway at boot.
- Restoring sessions after the process is killed.
- Remembering the "Keep CPU awake" state across session starts.
- Sessions in a separate process.
