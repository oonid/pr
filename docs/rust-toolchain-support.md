# Rust Toolchain Support (vfork/CLONE_VM and link2symlink Fixes)

Date: 2026-04-16
Status: T8.1 Complete, T8.2 Complete, T8.3 Complete, T8.4 Complete
Device: Samsung SM-XXXXX (Galaxy X), Android 16 (SDK 36), aarch64
Companion docs: `docs/proot-improvement.md`, `docs/important-notes.md`

---

## Problem Statement

The Rust toolchain (`cargo build`, `rustc`) failed inside proot due to two independent issues:

1. **vfork/CLONE_VM**: Rust's process spawning uses `clone(CLONE_VM|CLONE_VFORK|SIGCHLD)`.
   proot cannot handle `CLONE_VM` (shared memory) or `CLONE_VFORK` (parent blocks until
   child execs) — both assume kernel-level semantics that proot can't virtualize.

2. **GCC prefix resolution**: `cc -print-search-dirs` returned `/.l2s/../lib/gcc/...`
   instead of `/usr/lib/gcc/...`. GCC's `make_relative_prefix()` uses `realpath(argv[0])`
   which calls `readlink()` internally. The link2symlink extension's two-level symlink
   chain leaked `.l2s.` paths through `readlink()`, causing GCC to compute a wrong
   installation prefix.

---

## T8.1 — Strip CLONE_VM/CLONE_VFORK from clone syscalls

### Root Cause

Rust's standard library (`std::sys::unix::process::process_unix`) spawns child processes
using:

```c
clone(CLONE_VM | CLONE_VFORK | SIGCHLD, ...)
```

- `CLONE_VM` — parent and child share the same memory space. Proot cannot virtualize
  this since it intercepts syscalls via ptrace, not by virtualizing memory.
- `CLONE_VFORK` — parent blocks until child calls `execve()`. Proot's ptrace-based
  interception breaks this guarantee because the child's syscalls are also traced.

Without these flags, the call becomes `clone(SIGCHLD)` — equivalent to `fork()` — which
proot handles correctly.

### Fix

**File**: `src/proot/src/syscall/enter.c` (lines 150-171)

Added `PR_clone` and `PR_clone3` handlers in `translate_syscall_enter()` that strip
`CLONE_VM` and `CLONE_VFORK` from the clone flags argument **when `CLONE_THREAD` is NOT
set**. The `CLONE_THREAD` guard is critical — thread creation uses `CLONE_VM` legitimately
and must not be disturbed.

```c
case PR_clone:
case PR_clone3: {
    word_t flags = peek_reg(tracee, ORIGINAL, SYSARG_1);
    if (!(flags & CLONE_THREAD)) {
        flags &= ~(CLONE_VM | CLONE_VFORK);
        poke_reg(tracee, SYSARG_1, flags);
    }
    break;
}
```

### Verification

- `cargo build` on hello-world project: compiles successfully (~2s)
- vfork test: `vfork()` + `execve("/usr/bin/cc")` succeeds (before the fix, this hung
  forever because proot couldn't handle `CLONE_VFORK`)
  ```c
  #include <unistd.h>
  #include <stdio.h>
  #include <sys/wait.h>

  int main() {
      pid_t pid = vfork();
      if (pid == 0) {
          char *argv[] = {"cc", "/tmp/test.c", "-o", "/tmp/test_vf", NULL};
          char *envp[] = {NULL};
          execve("/usr/bin/cc", argv, envp);
          _exit(127);
      }
      int st;
      waitpid(pid, &st, 0);
      printf("vfork+execve: status=%d\n", WEXITSTATUS(st));
      return 0;
  }
  ```
- Regression tests: `apk`, `vim`, `gcc` all work
- Committed as `2342418`

---

## T8.2 — Fix GCC prefix resolution (link2symlink readlink leak)

### Root Cause

Alpine packages use hard links for identical files. Android's F2FS filesystem has a bug
where `lstat()` returns stale data after hard link operations. proot's link2symlink
extension works around this by replacing hard links with two-level symlinks:

```
/usr/bin/gcc → /.l2s/.l2s..apk.<hash>.<n> → /.l2s/<actual_file>
```

This breaks GCC's prefix resolution:

1. GCC calls `lrealpath(argv[0])` → `realpath("/usr/bin/gcc")`
2. `realpath()` calls `readlink("/usr/bin/gcc")` to follow the symlink
3. link2symlink's `translated_path()` follows the entire symlink chain, replacing the
   path with the final regular file `/.l2s/<actual_file>`
4. The kernel calls `readlink()` on a regular file → EINVAL... but actually the flow
   was more subtle (see below)
5. OR: the kernel reads the symlink, proot detranslates, and the guest sees a `/.l2s/`
   path that leaks the internal link2symlink structure

### Investigation: The Data Flow

The link2symlink extension intercepts the `TRANSLATED_PATH` event in `translate_path()`.
The `translated_path()` function (link2symlink.c:492-542) follows the two-level symlink
chain and replaces the host path with the final target. This happens for ALL syscalls
(except a small skip list: unlink, link, rename).

For `readlink("/usr/bin/gcc")`:
1. proot translates `/usr/bin/gcc` to host path `.../alpine/usr/bin/gcc`
2. `translated_path()` runs → follows the chain → replaces path with
   `.../alpine/.l2s/<actual_file>` (a regular file, not a symlink)
3. Kernel does `readlink()` on a regular file → EINVAL
4. `realpath()` sees EINVAL → treats path as non-symlink → uses the resolved path
   from step 2, which contains `/.l2s/` after detranslation

For GCC specifically, `realpath(argv[0])` was returning a `/.l2s/`-prefixed path,
which caused `make_relative_prefix()` to compute the installation prefix as
`/.l2s/..` instead of `/usr`.

### Fix — Two Parts

#### Part A: Fix `/proc/self/exe` (pre-requisite, not the GCC issue)

The `/proc/self/exe` virtual path was also leaking `.l2s.` paths because `tracee->exe`
was computed from the link2symlink-resolved host path.

**Files changed**:
- `src/proot/src/tracee/tracee.h` — Added `char host_exe_before_l2s[PATH_MAX]` field
- `src/proot/src/path/path.c` — Save `result` to `host_exe_before_l2s` before
  `notify_extensions(TRANSLATED_PATH, ...)` fires
- `src/proot/src/execve/enter.c` — Use `host_exe_before_l2s` instead of `host_path`
  when computing `tracee->new_exe` via `detranslate_path()`

#### Part B: Fix readlink to hide `.l2s.` symlinks

The fix makes `.l2s.` symlinks invisible to `readlink()` — `readlink()` returns EINVAL
(as if the path is not a symlink). Since the original files were hard links (not symlinks),
this is semantically correct from the guest's perspective.

**File**: `src/proot/src/extension/link2symlink/link2symlink.c`

Added `PR_readlink` and `PR_readlinkat` to the skip list in `translated_path()`. This
prevents link2symlink from following the symlink chain when the syscall is readlink.
The kernel sees the actual `.l2s.` symlink on disk and returns its target.

**File**: `src/proot/src/syscall/exit.c`

After `detranslate_path()` in the readlink exit handler, check if the basename of the
detranslated result starts with `L2S_PREFIX` (`.l2s.`, defined in `extension/extension.h`).
If so, return `-EINVAL` — making the symlink invisible to the guest.

```c
const char *l2s_name = strrchr(referee, '/');
if (strncmp(l2s_name != NULL ? l2s_name + 1 : referee,
	    L2S_PREFIX, strlen(L2S_PREFIX)) == 0) {
    status = -EINVAL;
    break;
}
```

The first version matched `/.l2s/` in the path, which only holds when `PROOT_L2S_DIR`
points at `<rootfs>/.l2s`. Without that directory link2symlink stores the intermediate
beside the original (`/usr/bin/.l2s..apk.<hash>`), and `realpath /usr/bin/cc` leaked it.

This works because:
- `realpath()` calls `readlink()` on each path component
- When `readlink()` returns EINVAL, `realpath()` treats the component as a regular
  file/directory (not a symlink) and keeps the canonical path as-is
- For `/usr/bin/gcc`, `realpath()` returns `/usr/bin/gcc` (correct)
- GCC's `make_relative_prefix()` computes the prefix as `/usr` (correct)

### Buffer Size Subtlety

During testing, `busybox readlink /usr/bin/gcc` returned just `/` (1 byte). This was
because busybox's readlink applet passes a small buffer (80 bytes). The symlink target
is an absolute host path (~147 bytes). After the kernel truncates to 80 bytes, proot
strips the 79-byte rootfs prefix, leaving only 1 byte: `/`. This is a busybox limitation,
not our bug — glibc/musl `readlink()` uses PATH_MAX (4096) buffers, so GCC's `realpath()`
works correctly.

### Verification

```
$ cc -print-search-dirs
install: /usr/lib/gcc/aarch64-alpine-linux-musl/15.2.0/    ← correct

$ echo "int main(){return 0;}" > /tmp/test.c && cc -o /tmp/test /tmp/test.c && /tmp/test
SUCCESS

$ cargo build
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
```

All without `COMPILER_PATH` or `LIBRARY_PATH` workarounds.

Part A was verified independently with a /proc/self/exe test:
```c
// Test binary compiled as /usr/bin/exe_test, installed via:
//   cc -o /tmp/exe_test exe_test.c
//   cp /tmp/exe_test files/.../alpine/usr/bin/exe_test
// Result (before Part A): /proc/self/exe = /.l2s/<path>   (WRONG)
// Result (after Part A):  /proc/self/exe = /usr/bin/exe_test  (CORRECT)
char buf[PATH_MAX];
ssize_t n = readlink("/proc/self/exe", buf, sizeof(buf) - 1);
printf("/proc/self/exe = %s\n", buf);
```

### Files Changed (T8.2, both parts)

| File | Change |
|------|--------|
| `src/proot/src/tracee/tracee.h` | Added `#include <limits.h>`, `host_exe_before_l2s[PATH_MAX]` field |
| `src/proot/src/path/path.c` | Save `result` to `host_exe_before_l2s` before TRANSLATED_PATH event |
| `src/proot/src/execve/enter.c` | Use `host_exe_before_l2s` for `new_exe` computation |
| `src/proot/src/extension/link2symlink/link2symlink.c` | Add `PR_readlink`/`PR_readlinkat` to `translated_path()` skip list |
| `src/proot/src/syscall/exit.c` | Return EINVAL when readlink result contains `/.l2s/` |

---

## T8.3 — Fix si_syscall=-1 SIGSYS Suppression (Complete)

### Root Cause

All remaining test failures (rustc compile, cargo build, git init) shared one root cause:
spurious SIGSYS from the zygote seccomp filter firing on syscall numbers that proot had
already modified to -1 (PR_void/SYSCALL_AVOIDER).

The event sequence:

```
1. Tracee executes SVC instruction (syscall entry)
2. Kernel ptrace: SIGTRAP|0x80 (syscall-enter-stop) → proot handles it
3. proot modifies registers (possibly sets syscall to -1 via set_sysnum(PR_void))
4. proot restarts with PTRACE_SYSCALL
5. Kernel re-executes SVC with MODIFIED registers
6. Zygote seccomp filter fires on the MODIFIED syscall number
7. If modified syscall == -1 → NOT in allowlist → SECCOMP_RET_TRAP → SIGSYS
8. proot catches SIGSYS, reads x8 → gets stale value (e.g. 56 = openat, or whatever)
```

The kernel reports `si_syscall=-1` (invalid/no syscall) in `siginfo_t`, but proot reads
the register value which is stale/misleading. The old code only suppressed this when
`seccomp_after_ptrace_enter` was true, but that flag was never set — the zygote uses
`SECCOMP_RET_TRAP` (not `SECCOMP_RET_TRACE`), so `PTRACE_EVENT_SECCOMP` never fires
and `seccomp_detected` stays false.

### Fix

**File**: `src/proot/src/tracee/event.c` (line 655)

```c
// Before (only suppressed when seccomp_after_ptrace_enter was true):
if (tracee->skip_next_seccomp_signal ||
    (seccomp_after_ptrace_enter && siginfo.si_syscall == SYSCALL_AVOIDER)) {

// After (always suppress when kernel says syscall was -1):
if (tracee->skip_next_seccomp_signal ||
    siginfo.si_syscall == SYSCALL_AVOIDER) {
```

When `si_syscall == -1`, the kernel is telling us the blocked syscall was -1 (no valid
syscall). This only happens when proot has already modified the registers. The SIGSYS is
spurious — proot already dealt with this syscall. Swallowing it is safe because
`si_syscall == -1` never occurs for a legitimate syscall request.

### Additional Fixes Applied

**arm64 syscall number update** (`seccomp.c`): Changed `push_specific_regs(tracee, false)`
to `push_specific_regs(tracee, true)` in `restart_syscall_after_seccomp()`. On arm64,
the syscall number is stored in `NT_ARM_SYSTEM_CALL` (separate from x8 in general
registers). The `false` parameter skipped this update, making all SIGSYS syscall
conversions (openat2→openat, faccessat2→faccessat, etc.) ineffective on arm64.

**PR_openat2 handler** (`seccomp.c` + `sysnums-arm64.h`): Added openat2 (syscall 437) to
the syscall table and SIGSYS handler. Converts openat2 → openat, drops resolve flags.

**PR_faccessat handler** (`seccomp.c`): Returns 0 (proot fakes root, access() always succeeds).

**SIGSYS log truncation** (`cli.c`): Truncate the SIGSYS log file at proot startup (fopen "w")
so each run starts with a clean log.

### Result

37/37 ALL PASS on Alpine:
distro(8) clone(5) readlink(6) gcc(3) rust(4) git(3) pipe(3) general(5)

All previous theories were WRONG: pipe2 blocked by seccomp, linkat/utimensat,
CLONE_THREAD, execve blocked. All were the same si_syscall=-1 root cause.

---

## Key Insights

### readlink is the vulnerability in link2symlink

The link2symlink extension transparently replaces hard links with symlinks. For most
syscalls (open, stat, exec), this is invisible — `translated_path()` resolves the chain
and the guest sees the final file. But `readlink()` is specifically designed to *not*
follow symlinks — it reads the symlink target itself. When `translated_path()` resolves
the chain before the kernel sees it, `readlink()` operates on the wrong file type.

The fix: skip `translated_path()` for readlink (so the kernel sees the real symlink),
then in the exit handler, detect `.l2s.` paths in the result and return EINVAL. This
effectively makes `.l2s.` symlinks invisible to the guest — `readlink()` says "not a
symlink", `realpath()` treats the path as a regular file, and the original hard-link
semantics are preserved.

### /proc/self/exe vs argv[0]

GCC uses `lrealpath(argv[0])` (not `/proc/self/exe`) to compute its installation prefix.
Part A (fixing `/proc/self/exe`) was necessary but not sufficient for GCC. Part B
(fixing readlink) was the actual fix for the GCC issue.
