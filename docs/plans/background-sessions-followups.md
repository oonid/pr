# Background Sessions Follow-ups Plan

Follow-ups from PR #16 (`docs/design/background-sessions.md`). Each task is test-first where the logic runs on the JVM,
reviewed on its own, and verified on a device at the end.

## Decisions

| Topic | Decision |
|-------|----------|
| End's drain window | Up to 10 s after SIGTERM/SIGCONT for the signalled processes to exit (was 3 s); the wait still stops as soon as they have exited |
| Remove wait and "Ending…" give-up | Derived from End's worst case instead of fixed 10 s values |
| Tests for Android-side logic | Small seams and JVM tests: `/proc` reads behind an interface; the service's foreground/stop decisions in a plain class |
| Install / remove / test from the list | An app-scoped operation runner owned by `App`, one operation at a time, state exposed as a `StateFlow`; the list only renders it, so recreation (rotation, dark mode) loses nothing |
| Running notification with no session | Never titled "0 sessions running" |

## Global Constraints

- No code comments (AGENTS.md). `vendor/` is read-only.
- Commit types `feat|fix|chore|docs|test`; no trailers, no AI mention, no device identifiers.
- No new dependencies.
- Lock order stays `SessionManager.lock` → screen lock; `ptyLock` separate. `open()` never blocks the main thread.
- Every task ends with `cd android && ./gradlew :app:testDebugUnitTest assembleDebug` green.

## Task 1: Service decisions in a plain class; no "0 sessions running"

`SessionService` keeps the Android calls; the decisions move to a JVM-testable class (for example `ServiceState`) that
takes events and returns actions:
- `onStartCommand(startId, running)` → start foreground (always first), record `lastStartId`, then stop with that id if
  `running` is empty;
- `onRunningChanged(running, awake)` → wake lock on/off, notify, or stop with `lastStartId` once foreground and empty;
- stops always use `stopSelfResult(lastStartId)`; nothing stops before the first `startForeground`.
`SessionService` executes the returned actions. Tests cover: start then immediate stop when empty; a start arriving while
stopping keeps the service (newer id); conflated `[a] → [] → [a]` keeps it; the wake lock follows `awake && running`.
The notification posted by a `startForeground` with an empty list does not say "0 sessions running" (for example
"Stopping…").

## Task 2: `/proc` behind a seam

`ProotPty`'s scanning (`TracerPid` tracees, children files of every task, the stat-scan fallback, `alive()`) reads through
a small interface (pid list, status text, stat text, task ids, children text) with the real `/proc` as the default. JVM
tests with a fake process table cover: tracee selection excluding the login shell and proot; another proot's tracees
untouched; a daemon reparented to init still found by `TracerPid`; fallback to the descendant walk when a status can't
be read; a pid vanishing mid-scan; zombie/dead counted as exited; `forceKill` targets ending with proot.

## Task 3: 10 s drain window; derived timeouts

`signalWaitMs` default 10 000. One place defines End's worst case (signal wait + EOF grace + kill grace + exit-code wait,
plus the scan margin) and the derived timeout used by Remove's wait (`MainActivity`, or the runner after Task 4) and the
"Ending…" give-up (`TerminalActivity.ENDING_WAIT_MS`); no other hard-coded 10 s for these. Tests: the wait stops as soon
as the signalled pids exit; the derived timeout exceeds the worst case. Docs: the End row (design §4, architecture) and
the Remove/Ending rows.

## Task 4: App-scoped operation runner

A class owned by `App` (like `SessionManager`) runs `pr-cli` install / remove / test with an injected command seam (the
real one wraps today's `runDistroCommand`) and an injected scope/dispatcher for tests. State: the current operation
(kind, alias, display name, output lines, running/finished, exit). One operation at a time (a second start is refused).
Remove ends a running session first and waits up to the derived timeout (Task 3), cancelling with
"Couldn't stop the <name> session; removal cancelled." as today. When an operation finishes, the list refreshes.
`MainActivity` renders the state (output sheet, busy state, per-row loading) and keeps only UI-local state (the text
fields, the sheet's visibility) in `rememberSaveable`. JVM tests with a fake command and fake sessions: output streaming,
refusal while busy, Remove's end-then-remove and cancel paths, finish state.

## Task 5: Docs, device checks, PR

Design and architecture docs updated for every change above. Device checks: rotate and toggle dark mode during an
install, a remove and a test (output and busy state survive; no second operation can start); End with a program that
takes 5 s to drain on SIGTERM (it finishes cleanly); End with a program ignoring SIGTERM (ends within the worst case);
Remove and "Ending…" still behave; a login failing at once shows no "0 sessions running". Then push and open the PR.
