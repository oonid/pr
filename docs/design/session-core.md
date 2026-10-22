# Design Spec: Extract `:session-core` Android Library

**Date:** 2026-10-22
**Status:** design approved; implemented (`docs/plans/session-core.md`)

## 1. Goal

Move the generic background-session code out of `:app` into an Android library module,
`:session-core`, so another app can run PRoot sessions in the background the way pr does, without
copying the code. pr's behaviour does not change. The module also gains one generic capability: a
session that runs a given command, with extra bind mounts, without a terminal.

The first other consumer is a separate app that keeps a long-running server (a Hermes Agent
gateway) running in a container. It includes `:proot-engine`, `:termlib` and `:session-core` the
way rs.oo.or.id includes `:proot-engine` today. Nothing in this module is specific to that app.

## 2. What Moves

From `android/app/src/main/java/id/or/oo/pr/` to `android/session-core/`, keeping the Kotlin
package `id.or.oo.pr.session` so pr's imports barely change:

| Code | Notes |
|------|-------|
| `session/SessionManager`, `ManagedSession`, `ServiceState`, `ProotProcesses` (with `ProcFs`), `Pty`, `ProotPty` | Unchanged |
| `session/SessionService`, `SessionNotifications`, `AndroidSessionHooks` | Host couplings removed (§3) |
| `session/ProotSessionFactory` | The interactive terminal session, unchanged |
| `ProcessRunner` | Moves with its package and now implements `CommandRunner` |
| `PrCliCommand` and its `DistroCommand` interface | Take a `ProotHost` instead of pr's `App` |
| The bootstrap in `App.kt` (prefix directories, native-library and busybox links, `bootstrap.sh`) | Becomes a reusable `PrBootstrap`; `assets/scripts/bootstrap.sh` moves with it |

Their JVM tests move with them (`session/*Test`, `ProcessRunnerTest`, and the fakes). pr keeps
`App`, `MainActivity`, `TerminalActivity`, `DistroOperations`, `InstalledContainers` and
`SessionBanners`, and their tests.

## 3. Host Couplings Removed

| Today | In `:session-core` |
|-------|-------------------|
| `SessionService` reads `(application as App).sessions` | The `Application` implements a small `SessionHost` interface that gives the `SessionManager` and the notification settings |
| Action strings `id.or.oo.pr.action.*` | Built from the host's package name |
| Notifications use pr's `R.drawable.ic_notification` and open `MainActivity` | The host gives the small icon and the intent that opens the app |
| `PrCliCommand(app: App)` | `PrCliCommand(host: ProotHost)` |
| The bootstrap reads pr's preferences and version constant | `PrBootstrap(context, host)` with the version passed in |

Notification texts stay as they are. The service stays declared in each app's manifest, because
the `specialUse` subtype text is each app's own justification; the class comes from the module.

## 4. New: Command Sessions

A second session factory runs `pr-cli login <container> --user root [--custom-bind host:guest]... --
'<command>'` through the existing `ProotLauncher.startCustomSession`. Its session has no terminal
emulator; its output goes to a sink the host provides (for example, a bounded log buffer). It
starts, appears in the running notification, and ends with End and drain exactly like a terminal
session, because `ManagedSession` and `SessionManager` treat both the same way.

No change is needed in pr-cli or `:proot-engine`: `login` already accepts repeatable
`--custom-bind` and a trailing command.

**As executed.** The session key and the container are separate.
`CommandSessionFactory(host, container, command, sink, binds)` logs into `container`, and the key
given to `SessionManager.open(key, factory)` only names the session: the `ManagedSession` alias,
the notifications and End. A host can run `web-server` in container `debian` while a
terminal into the same container runs beside it. `open` refuses a key held by a running or retained ended session of
another kind with `OpenResult.Failed`, instead of returning that session. A factory of the same
kind gets the running session back. `SessionManager.running` lists session keys, not containers,
so a host that removes or resets containers must map keys to containers itself.

End follows an explicit `EndPolicy`. Terminal sessions keep `SPARE_LOGIN_SHELL`: the login shell
is never sent SIGTERM. Command sessions use `SIGNAL_EVERY_TRACEE`, which adds proot's direct
children. Alpine's `sh -c` execs the last command, so the server itself is proot's child, and
without this it would be SIGKILLed after the grace period instead of stopping on SIGTERM.

A sink that throws loses only that output and the failure is logged once. `Bind` paths and the
command are checked when built, and a missing pr-cli makes `open` fail. So does a bind whose host
path does not exist, because proot would only warn and run the session without that bind.

## 5. Build and CI

- `android/settings.gradle.kts` includes `:session-core`; `:app` depends on it.
- CI runs `:session-core:testDebugUnitTest` next to `:app:testDebugUnitTest`.
- The module targets arm64-v8a and minSdk 28, like `:proot-engine`.

## 6. Testing

- The moved tests pass unchanged, in the new module.
- New JVM tests: the command session's pr-cli arguments (binds and the command as one argument
  after `--`); a headless session's output reaching its sink; the action strings built from a
  package name; `PrCliCommand` with a fake host.
- Coverage of the touched code stays at or above 80%.
- On a phone, pr behaves as before: open a terminal session, leave it in the background, End it
  from the notification, keep the CPU awake, install and remove a container.

## 7. Out of Scope

Changing rs.oo.or.id to use the module (possible later), any host-specific code, and changes to
pr-cli or `:proot-engine`.
