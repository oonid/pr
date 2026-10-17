# Design Spec: Background Sessions

**Date:** 2026-10-17
**Topic:** Keep terminal sessions running after leaving their screen
**Baseline:** tag `pre-background-sessions` (main `86d9e93`, before this work)

## 1. Goal & Context

A terminal session (the shell `pr-cli login` starts in a container, and everything started from
it) is owned by `TerminalActivity` today: the PTY, the `TerminalEmulator` holding the screen, and
the reader thread. Closing the screen ends the session; Back runs `cleanup()` (see
`android-apk-architecture.md`, "Session lifetime and Back"). Because pr-cli starts proot with
`--kill-on-exit`, nothing started in the session outlives it.

The main use is a long-running server, the Hermes gateway, that must keep working with the screen
off for hours. Ordinary terminal multitasking (leave a build running, come back later) comes with
it.

Success means: a session keeps running after leaving its screen and after swiping the app away
from recents, survives deep sleep when the user asks for it, can be reattached with its screen
contents, and ends only when the user ends it or its program exits.

## 2. Decisions

| Question | Decision | Alternatives considered |
|----------|----------|-------------------------|
| Main goal | Hermes gateway 24/7 first; covers multitasking | Multitasking only |
| Sessions per container | One; open reattaches. More shells via tmux inside the container | Several, with a session list or tabs |
| Who owns sessions | An app-wide `SessionManager`; a thin foreground service keeps the process alive (approach B) | A: a bound service owns the sessions. C: sessions in a separate process with IPC |
| Back | Prompt every time: End session / Keep running / Cancel | Keep running plus a setting; End plus a separate Detach action |
| CPU in deep sleep | "Keep CPU awake" toggle in the notification, off by default, not remembered; surviving Doze also needs the battery-optimization exemption, which the app asks for | Always hold a wake lock; no wake lock |
| Service type | `specialUse` | `dataSync` (Android 15 limits it to 6 hours a day) |
| Session ended while away | Post a "session ended (exit N)" notification | Silent |

Approach B follows Android's "Guide to app architecture": activities and services are entry points
the system can destroy, and state lives in a layer they use. Reliability is the same as A, since
both keep sessions in one process kept alive by the same foreground service; B avoids a service that
is both started and bound, and keeps the session logic testable on the JVM.

## 3. Components

### `SessionManager` (new, app module)

Owned by `App`, lives as long as the process. Holds at most one `ManagedSession` per container
alias.

- `open(alias): ManagedSession` returns the running session for the container, or starts one
  through `ProotLauncher.startSession`. A failure to start is returned to the caller, not thrown
  past it.
- `end(alias)` and `endAll()` close sessions.
- `sessions: StateFlow<List<SessionInfo>>` (the aliases of running sessions) for the list screen
  and the notification.
- When the number of running sessions goes from 0 to 1 it starts `SessionService`; from 1 to 0 it
  stops it. This is the only place the two are kept in step.
- All changes to the session map are serialised: concurrent `open` and `end` for one container
  yield either the old session or a new one, never two.
- Holds only the application context.

Seams, so the logic runs in JVM unit tests (the real emulator uses native code):

- a session starter (real: `ProotLauncher`; tests: an in-memory fake);
- an emulator factory (real: `TerminalEmulatorFactory`);
- a foreground controller (real: starts and stops `SessionService`).

### `ManagedSession` (new, app module)

What `TerminalActivity` holds today, moved out of it:

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

It exposes `write`, `resize`, `attached` (whether a screen shows it) and the exit state. When the
child exits it records the exit code and notifies the manager. The manager stops counting it as
running at once, and drops it when no screen shows it: immediately if none is attached, otherwise
when that screen closes, so the screen can still show the exit code.

### `SessionService` (new foreground service, type `specialUse`)

Holds no session state.

- Keeps the process alive while at least one session runs; returns `START_NOT_STICKY`, and stops
  itself if started with no sessions.
- Owns the ongoing notification and its actions.
- Holds the partial wake lock while "Keep CPU awake" is on.

### `TerminalActivity` (changed)

A view onto a session. It calls `SessionManager.open(alias)`, draws the session's emulator, and
forwards keyboard input and resizes. Closing the screen detaches; it never ends the session. A
session is shown by at most one screen: opening a container whose session another screen shows
attaches it to the new screen, and the old screen closes. Back
keeps its current order: the first press hides the keyboard if it is up, the next clears a text
selection if one is active, and after that it shows the End session / Keep running / Cancel
prompt. If the session has ended, Back returns to the list without a prompt.

### `MainActivity` (changed)

Containers with a running session show a "running" mark; tapping one reattaches. It shows the
banners described in sections 5 and 6.

### Unchanged

`ProotLauncher`, `PtyNative`, `ptyjni.c`, pr-cli and proot.

## 4. Session Lifecycle

| Event | Behaviour |
|-------|-----------|
| Open a container with no session | `SessionManager.open` starts the shell; 0 to 1 starts the service and its notification; the screen attaches |
| Open a container whose session runs | The screen reattaches, shows the current contents, and resizes the session; a screen that showed it before closes |
| Back | Keyboard, then selection, then the prompt. End closes the session; Keep running returns to the list; Cancel stays |
| Home, or switching apps | No prompt; the session keeps running |
| Swipe the app away from recents | Screens are destroyed; the foreground service keeps the process, so sessions keep running |
| The program exits (`exit`, crash) | The session is marked ended with its exit code and no longer counts as running. An attached screen shows "Session ended (exit N)", Back returns without a prompt, and the session is dropped when the screen closes; with no screen attached it is dropped at once and a "session ended" notification is posted |
| "End all" in the notification | Every session ends as with End |
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

### Channels

1. **Running sessions**: low importance, silent, ongoing. Title "N sessions running", text the
   container names, plus "CPU kept awake" when the toggle is on. Tapping opens the list. Actions:
   **End all**, and **Keep CPU awake** / **Let CPU sleep**.
2. **Session ended**: default importance. "alias: session ended (exit N)"; tapping opens the list.

## 6. Wake Lock and Battery Optimization

Two different things stop a background session, and they need different remedies:

- **CPU sleep with the screen off.** Without a wake lock the CPU suspends soon after the screen
  turns off. "Keep CPU awake" holds a `PARTIAL_WAKE_LOCK` in `SessionService` so the CPU keeps
  running while the screen is off.
- **Doze (deep sleep).** After the phone has been idle and unplugged for a while, Doze ignores wake
  locks and suspends network access for every app that is not exempt from battery optimization. A
  foreground service keeps an app out of App Standby, not out of Doze. Only exempt apps may hold
  partial wake locks and use the network during Doze. Vendor lists such as "sleeping apps" can add
  their own limits.

So the 24/7 gateway needs both the toggle and the exemption:

- The toggle is off by default and kept in memory only: each time sessions start from zero it is
  off, so a forgotten toggle cannot drain the battery for days. It is released when toggled off or
  when the last session ends.
- When the toggle is on and the app is not exempt, the notification text adds "Deep sleep will
  pause sessions; allow in the app", and the list shows a banner with **Allow**, which opens the
  system's ignore-battery-optimizations dialog. The dialog opens from the app screen, because
  Android 12+ blocks starting activities from notification actions routed through a service.

## 7. Error Handling

| Failure | Behaviour |
|---------|-----------|
| Session cannot start (pr-cli missing, PTY fork fails) | The terminal screen shows "Couldn't start alias: reason"; nothing is recorded; the service is not started |
| The foreground service cannot start | The session runs while the app is open but is not protected; the list shows "Background running unavailable: reason"; no crash |
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
- the service starts at 0 to 1 and stops at 1 to 0, through End, End all and program exit;
- the "session ended" notification fires only when no screen is attached;
- concurrent open and end for one container never produce two sessions;
- output keeps being drained with no screen attached.

### Existing suites

proot-engine instrumented tests (8), `pr-cli test alpine` and `pr-cli test ubuntu`, the full
integration suite from an in-app terminal, and the 169 host tests stay green.

### Device lifecycle checks (adb; results recorded in the PR)

1. A counter writing a timestamp every second; Keep running; it keeps counting.
2. Reattach shows the screen contents.
3. Swipe the app away from recents; still counting.
4. Screen off with Keep CPU awake on: the counter continues. Then `dumpsys battery unplug` and
   `dumpsys deviceidle force-idle`: without the exemption the counter pauses (Doze ignores the wake
   lock); with the exemption granted it continues.
5. `exit` with no screen attached posts the "session ended" notification.
6. End all ends every session and removes the ongoing notification.
7. With the notification permission denied, sessions still survive.

### Target check

The Hermes gateway in the `hermes` container, Keep running, Keep CPU awake on, the exemption
granted, forced deep sleep: it keeps answering from the host through `adb forward` for at least 30
minutes, and its calls to the model provider keep working.

## 9. Out of Scope

- Several sessions per container (use tmux inside the container).
- Starting sessions or the gateway at boot.
- Restoring sessions after the process is killed.
- Remembering the "Keep CPU awake" state across session starts.
- Sessions in a separate process.
