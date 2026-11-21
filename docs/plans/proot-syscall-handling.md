# proot syscall handling: Implementation Plan

> Execute this plan task by task, with a review after each. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** proot never lets a guest syscall that takes a path reach the kernel untranslated, and never answers a blocked syscall in a way that changes what it does; a caller always gets either the right result or ENOSYS, which every libc already handles as "old kernel".

**Architecture:** two places change in `src/proot/src`. The enter stage (`syscall/enter.c`, `syscall/sysnum.c`) answers ENOSYS for path syscalls proot cannot translate, so the answer is the same whether or not the zygote filter traps the call. The SIGSYS handler (`tracee/seccomp.c`) stops rewriting calls whose arguments it cannot carry over: `clone3` and `openat2` get ENOSYS, and `renameat2` and `faccessat2` are downgraded only when no flags are set. New guest tests in `src/proot-integration-test` pin each behaviour.

**Tech Stack:** C (proot, NDK r27c through `scripts/build.sh`), Rust 1.97.1 (the guest test binary, `libc` 0.2), pr-cli embeds the test binary.

**Spec:** `docs/audit-2026-11-16.md` (local): P11, P1, P14, P17, P22, P23; the details in `docs/audit-2026-11-16-details/findings-proot.md` and `verify-proot.md`.

## Findings this plan relies on

- The app seccomp filter (bionic `SECCOMP_ALLOWLIST_*`, default action `SECCOMP_RET_TRAP`) allows, by release tag: `renameat2` from `android-8.0.0_r1` (correction: this plan first said `android-11.0.0_r1`, and the audit said Android 9/10 trapped it; bionic's whitelist has had it since Android 8), `clone3` from `android-14.0.0_r29`, and `openat2`, `faccessat2`, `fchmodat2`, `setxattrat`, `getxattrat`, `listxattrat`, `removexattrat` from `android-16.0.0_r3`. Above proot's arm64 table (last entry 440) it allows only those five path syscalls plus `epoll_pwait2` (441), `landlock_*` (444–446) and `mseal` (462). Everything else above 440 is trapped, and the SIGSYS `default:` case already answers ENOSYS.
- `syscall/enter.c` has no case for `openat2`, so on Android 16 r3+ and 17 its path reaches the kernel untranslated (resolved on the host, not in the rootfs). On Android 9–15 the SIGSYS handler restarts it as `openat` with the `open_how` pointer as flags (`tracee/seccomp.c:706-710`).
- `syscall/sysnum.c:95` uses `index > sysnums.length`; it must be `>=`.
- When the enter stage returns a negative status, `syscall/syscall.c:154-158` voids the call (`PR_void`, kernel number -1) and stores the status as the result. Where the filter traps -1, `tracee/event.c:657` suppresses that SIGSYS. So an enter-stage ENOSYS reaches the guest on every Android version.
- The `clone3` SIGSYS case strips `CLONE_VM`/`CLONE_VFORK`, passes the lowest stack address as the stack pointer and swaps `tls`/`child_tid` (`tracee/seccomp.c:721-743`). glibc ≥ 2.34 falls back to `clone` only on ENOSYS. `clone` itself is allowed by every filter, so its SIGSYS case (`:745-756`) is never reached.
- `renameat2` and `faccessat2` cases drop their flags. libcs call them only when flags are set (with 0 they use `renameat` and `faccessat`), so the downgrade runs exactly when it does harm. `faccessat2` is also translated with REGULAR at the enter stage (`syscall/enter.c:437,446`), so `AT_SYMLINK_NOFOLLOW` is lost on Android 16+ too.
- glibc calls `rseq` (293, not in proot's table) at every process start; it is trapped on every version and reaches the logging `default:` case each time.
- The guest test binary is a bionic program run inside proot (`src/proot-integration-test`, suites in `src/main.rs`, raw syscalls through `libc::syscall`). The `gcc` suite compiles C with the distro's own libc, so it exercises glibc on Ubuntu and musl on Alpine.

## Global Constraints

- AGENTS.md applies: no comments; `vendor/` read only; commits `fix|test|docs|chore: …`, body ≤ 72 columns, no trailers; no device identifiers, bare `adb` in docs.
- This PR changes AGENTS.md constraint 3 ("never remove them"): SIGSYS handlers stay for syscalls the filter blocks and that proot can answer correctly; a handler that rewrites a call it cannot carry over is replaced by ENOSYS.
- Test on the phone (Android 17) for every task; the Android 9–15 paths are checked on an emulator in PR 2.
- Builds: one `scripts/build.sh --arch=arm64` for proot and one `scripts/build-pr-cli.sh` for the test binary and pr-cli, after the last code task; ask before running them. The committed `libproot.so` and `libpr-cli.so` are updated in their own `chore:` commit.

## Review Focus

1. A guest program that opens files normally (glibc and musl `open`, `fopen`, coreutils) still works on Android 17: nothing in this PR may touch `openat`.
2. Syscalls above 440 that are allowed and take no path (`epoll_pwait2`, `landlock_*`, `mseal`) still reach the kernel.
3. glibc `posix_spawn`, `system()` and `popen()` keep working on Android 17 (clone3 allowed there) and get ENOSYS-then-clone on older versions.
4. `mv -n` onto an existing file never overwrites it on any version.
5. The SIGSYS log is not written through a symlink and does not grow by a line per process start.

---

### Task 1: ENOSYS at the enter stage for path syscalls proot cannot translate

**Files:**
- Modify: `src/proot/src/syscall/sysnum.c` (bound check), `src/proot/src/syscall/enter.c`
- Test: `src/proot-integration-test/src/syscalls.rs` (new suite `syscalls`), `src/proot-integration-test/src/main.rs`

**Interfaces:**
- `translate_sysnum`: `index >= sysnums.length` returns `PR_void`.
- `translate_syscall_enter`: returns `-ENOSYS` when the original kernel number is 437 (`openat2`), or is above 440 and not one of 441, 444, 445, 446, 462. This runs before the `switch`, so no extension or translation runs for these calls.

- [ ] **Step 1: Write the failing tests** (suite `syscalls`, probe always true):
  - `openat2 is ENOSYS and leaves the file intact`: write 4 bytes to `/tmp/pit-sc-file`, call `syscall(437, AT_FDCWD, path, &how{flags: O_RDONLY}, 24)` 20 times, expect `-1/ENOSYS` each time and the file still 4 bytes long.
  - `fchmodat2 is ENOSYS`: `syscall(452, AT_FDCWD, path, 0o600, 0)` → `-1/ENOSYS`, mode unchanged.
  - `setxattrat is ENOSYS`: `syscall(463, AT_FDCWD, path, 0, name, &args, size)` → `-1/ENOSYS`.
  - `a path-free syscall above the table still works`: `syscall(441, epfd, events, 1, &zero_timeout, NULL, 8)` on a fresh epoll fd returns 0.
- [ ] **Step 2:** Build the test binary only (`cargo build --release --target aarch64-linux-android` in the test crate), copy it to the app's cache directory as `pit` (`adb push` then `run-as … cp`), and run `pr-cli login alpine -- "/tmp/pit syscalls"` with the current proot, as `cmd_test.rs` does. Expect the `openat2` and `fchmodat2` tests to fail (Android 17 lets them through).
- [ ] **Step 3:** Implement the two changes above.
- [ ] **Step 4:** Rebuild proot (one build), run the suite on the phone. Expect all four to pass, and every other suite unchanged.
- [ ] **Step 5: Commit** `fix: refuse path syscalls proot cannot translate`.

---

### Task 2: SIGSYS handlers that cannot carry a call over answer ENOSYS

**Files:**
- Modify: `src/proot/src/tracee/seccomp.c`, `src/proot/src/syscall/enter.c` (`faccessat2` translation)
- Test: `src/proot-integration-test/src/syscalls.rs`, `src/proot-integration-test/src/gcc.rs`

**Interfaces:**
- `PR_clone3`: `set_result_after_seccomp(tracee, -ENOSYS)` (no logging). The `PR_clone` case is removed. The `PR_openat2` case is removed (Task 1 voids it before the filter).
- `PR_renameat2`: when the original `SYSARG_5` (flags) is 0, downgrade to `renameat` as today; otherwise `-ENOSYS`.
- `PR_faccessat2`: when the original `SYSARG_4` (flags) is 0, downgrade to `faccessat` as today; otherwise `-ENOSYS`.
- Enter stage, `PR_faccessat2`: translate with `SYMLINK` when `SYSARG_4` has `AT_SYMLINK_NOFOLLOW`, else `REGULAR`.
- Kernel number 293 (`rseq`) in the SIGSYS handler: `-ENOSYS` without logging.

- [ ] **Step 1: Write the tests** (they pass on Android 17 already for `clone3` and `renameat2`, whose calls the filter allows there; on Android 9–14 they are checked on the emulator in PR 2):
  - `renameat2 NOREPLACE never overwrites`: files `a` ("A") and `b` ("B"); `syscall(276, AT_FDCWD, a, AT_FDCWD, b, RENAME_NOREPLACE)` must fail with EEXIST or ENOSYS, and `b` must still read "B".
  - `faccessat2 NOFOLLOW on a dangling symlink`: `syscall(439, AT_FDCWD, link, F_OK, AT_SYMLINK_NOFOLLOW)` returns 0 or ENOSYS, never ENOENT (fails on Android 17 today, through the REGULAR translation).
  - gcc suite, `system("true") returns 0`: compile and run `int main(){return system("true");}` with the distro's cc; expect exit 0.
  - gcc suite, `popen reads a child's output`: `popen("echo pit","r")` reads "pit".
- [ ] **Step 2:** Run on the phone with the Task 1 proot: expect the `faccessat2` test to fail, the others to pass.
- [ ] **Step 3:** Implement the interfaces above.
- [ ] **Step 4:** Rebuild proot, run on the phone: all pass, other suites unchanged.
- [ ] **Step 5: Commit** `fix: answer ENOSYS where a blocked call cannot be carried over`.

---

### Task 3: The SIGSYS log: no symlinks, one line per syscall, the tracee's name

**Files:**
- Modify: `src/proot/src/tracee/seccomp.c` (`sigsys_log_path` users, the `default:` case, the start-up trim)

**Interfaces:**
- The log is opened with `open(path, O_WRONLY|O_APPEND|O_CREAT|O_NOFOLLOW|O_CLOEXEC, 0600)` and trimmed with `fstat`/`ftruncate` on that descriptor; a symlink at the path makes logging a no-op.
- Each kernel number is logged once per proot process (a fixed-size bitmap over 0–1023; numbers above are logged every time).
- `comm=` is read from `/proc/<tracee pid>/comm`.

Moving the log out of the directory guests see as `/tmp` belongs to PR 5 (the per-container `/tmp`).

- [ ] **Step 1:** No automated test fits (the log is proot's own file, outside what the guest suites can observe reliably); the check is part of Task 5: in a guest, make `/tmp/sigsys-log.txt` a symlink to `/tmp/pit-sc-target`, start a second session and run a few programs; the target must stay empty. Before the change it grows by a line per glibc program.
- [ ] **Step 2:** Implement.
- [ ] **Step 3: Commit** `fix: write the SIGSYS log once per syscall and never through a symlink`.

---

### Task 4: Docs and the AGENTS.md constraint

**Files:**
- Modify: `AGENTS.md` (constraints 3 and 5), `docs/important-notes.md` (handler table, file list), `docs/proot-improvement.md` (§ handler tables, the faccessat2/renameat2/clone3/openat2 notes, §390), `docs/integration-tests.md` (the new `syscalls` suite and the two gcc tests, totals)

- [ ] **Step 1:** Rewrite the handler tables from the code after Tasks 1–3: per syscall, the release from which the filter allows it, and what proot does.
- [ ] **Step 2:** Constraint 3: "SIGSYS handlers stay for syscalls the filter blocks and proot can answer correctly; otherwise ENOSYS." Constraint 5: name the handlers that read ORIGINAL registers instead of claiming all do.
- [ ] **Step 3: Commit** `docs: the syscall handling as it is now`.

---

### Task 5: Binaries and the phone check

- [ ] **Step 1:** (Ask first.) `scripts/build.sh --arch=arm64`, copy proot and the loader to `jniLibs`; `scripts/build-pr-cli.sh`; `cd android && rm -rf app/build proot-engine/build && ./gradlew assembleDebug`.
- [ ] **Step 2: Commit** `chore: update proot and pr-cli with the syscall fixes`, noting Rust and NDK versions in the body.
- [ ] **Step 3:** Phone (Android 17): install; Test on Alpine and Ubuntu. Expect all earlier suites as before (73 runs) plus the new `syscalls` suite and two gcc tests passing. In an Ubuntu guest: `python3 -c 'import os;print(os.system("true"))'` prints 0; `mv -n a b` with both present leaves `b`. The Task 3 log check.
- [ ] **Step 4:** Results go in the pull request, without device names or serials.

## Not in this PR

- P21 (handlers for `chdir`, `fchdir`, `getcwd`, `faccessat`, `fchmodat`, `openat`/`fstatat` that the filter never reaches): left in place, marked in the docs; removing them needs a counter run across a full test pass.
- P13 (the log directory is the guests' `/tmp`): PR 5.
- `process_madvise` answering 0 without doing anything: advisory, left as is.
