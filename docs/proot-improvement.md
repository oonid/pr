# proot Improvements: Our Fork vs vendor/proot and vendor/termux-proot

This document compares our proot (`src/proot/src/`) against the two vendor references:
- **vendor/proot** — upstream proot v5.4.0
- **vendor/termux-proot** — Termux's fork v5.1.0

Our fork is derived from **vendor/termux-proot** with extensive custom modifications for running
inside an Android app process that has the zygote's seccomp BPF filter active (Seccomp: 2).

## Why modifications were needed

Android's zygote installs a seccomp BPF filter on all forked app processes. This filter blocks
certain syscalls using two mechanisms:

1. **SECCOMP_RET_TRAP** — sends SIGSYS to the tracee. Proot catches this via ptrace and
   handles it in our SIGSYS handler (`tracee/seccomp.c`).
2. **SECCOMP_RET_ERRNO** — silently returns ENOSYS to the caller. No signal is generated,
   so proot's SIGSYS handler never fires.

Additionally, proot's own BPF seccomp filter conflicts with the zygote's filter, so we disable
proot's filter entirely and rely on ptrace-based syscall interception (`PTRACE_SYSCALL`) for
all syscall translation.

---

## 1. New Files (not in either vendor)

| File | Purpose |
|------|---------|
| `tracee/seccomp.c` | SIGSYS handler for Android zygote seccomp-blocked syscalls |
| `tracee/seccomp.h` | Seccomp handler declarations |
| `tracee/statx.c` | `statx()` syscall emulation on kernels that lack it |
| `tracee/statx.h` | statx structures and declarations |
| `path/f2fs-bug.c` | Workaround for F2FS filesystem lstat() stale data bug |
| `path/f2fs-bug.h` | F2FS bug declarations |
| `tls-align.c` | Forces 64-byte TLS alignment for Android/Bionic compatibility |
| `build.h` | Auto-generated build configuration |
| `loader/loader-info.c` | Auto-generated loader symbol addresses extracted via `readelf` |

Note: `tracee/seccomp.c/h`, `tracee/statx.c/h`, and `path/f2fs-bug.c/h` are inherited from
vendor/termux-proot with extensive modifications. The others are entirely new.

---

## 2. SIGSYS Handlers (`tracee/seccomp.c`)

vendor/proot has no `seccomp.c`. vendor/termux-proot has one with ~30 handlers for syscalls
like `setsockopt`, `socket`, `bind`, `connect`, `mount`, `chroot`, `clone`, `fork`, etc.

### Handlers we added beyond termux

| Handler | Syscall | Number (arm64) | Strategy |
|---------|---------|----------------|----------|
| `PR_chdir` | `chdir` | 49 | Translate path, update `tracee->fs->cwd` via talloc, return success |
| `PR_fchdir` | `fchdir` | 50 | Resolve dirfd to path, update cwd, return success |
| `PR_linkat` | `linkat` | 37 | Translate paths, try `renameat()`, fallback to copy+delete on EXDEV/EACCES |
| `PR_getcwd` | `getcwd` | 17 | Read `tracee->fs->cwd`, write to tracee buffer, return length |
| `PR_faccessat2` | `faccessat2` | 439 | Downgrade to `faccessat` (drop flags arg) and restart |
| `PR_faccessat` | `faccessat` | 48 | Return 0 (noop — zygote blocks even the base `faccessat` on some kernels) |
| `PR_renameat2` | `renameat2` | 276 | Downgrade to `renameat` (drop flags arg) and restart |
| `PR_openat2` | `openat2` | 437 | Downgrade to `openat` (clear SYSARG_5 size=0) and restart |
| `PR_process_madvise` | `process_madvise` | 440 | Return 0 (noop — advisory syscall) |
| `PR_fchmodat` | `fchmodat` | 53 | Return 0 (noop — seccomp blocks this on some kernels) |
| `PR_clone3` | `clone3` | 435 | Convert clone3 args struct to clone register args, return -ENOSYS for CLONE_THREAD, strip CLONE_VM/CLONE_VFORK otherwise |
| `PR_clone` | `clone` | 220 | Return -ENOSYS for CLONE_THREAD, strip CLONE_VM/CLONE_VFORK otherwise |
| `PR_setuid` | `setuid` | 146 | Return 0 (noop — fake_id0 handles UID semantics) |
| `PR_setgid` | `setgid` | 144 | Return 0 (noop — fake_id0 handles GID semantics) |
| `PR_setreuid` | `setreuid` | 145 | Return 0 (noop) |
| `PR_setregid` | `setregid` | 143 | Return 0 (noop) |
| `PR_setfsuid` | `setfsuid` | 151 | Return 0 (noop) |
| `PR_setfsgid` | `setfsgid` | 152 | Return 0 (noop) |

### Handler details

**chdir**: The zygote seccomp filter blocks `chdir`. Without this handler, `cd /tmp` fails.
The handler reads the path from SYSARG_1, translates it through proot's path translation,
updates the tracee's current working directory (`tracee->fs->cwd`) using talloc string
duplication, and sets the result to 0.

**fchdir**: Same as chdir but uses a directory file descriptor. Resolves the fd to a path
using `readlink("/proc/self/fd/N")`, translates, and updates cwd.

**getcwd**: The zygote blocks `getcwd`. The handler reads proot's tracked cwd from
`tracee->fs->cwd`, verifies the path still exists via `translate_path()`, writes the cwd
string to the tracee's output buffer, and returns the string length. Without this handler,
`apk` trigger scripts that call `getcwd()` fail with ENOSYS.

### aarch64 SYSARG_1/x0 clobber fix

On aarch64, `SYSARG_1` (x0) and `SYSARG_RESULT` (x0) share the same physical register.
At SIGSYS time, the kernel may have modified x0 before proot reads it. This caused handlers
to read clobbered values — e.g. `fchdir(3)` appeared as `fchdir(0)` (stdin).

**Fix**: All handlers that read `SYSARG_1` now use `ORIGINAL` register version (saved by
`save_current_regs(ORIGINAL_SECCOMP_REWRITE)` right after `fetch_regs()`) instead of
`CURRENT`. Affected handlers: `PR_chdir`, `PR_fchdir`, `PR_linkat`. `SYSARG_2`-`SYSARG_6`
(x1-x5) are NOT affected since they don't overlap with the result register.

**linkat**: Alpine's `apk` uses `linkat` for atomic file replacement (create temp file,
hard-link to target, unlink temp). SELinux blocks hard links on `app_data_file`, and
cross-directory renames fail with EXDEV due to Android FBE encryption contexts. The handler
tries `renameat()` first. If that fails with EXDEV or EACCES, it copies the file content
via a read/write loop and unlinks the original.

**faccessat2**: Modern musl uses `faccessat2` (with flags) for `access()`. The zygote blocks
it. The handler clears SYSARG_4 (flags) to downgrade to `faccessat` (without flags), which
the kernel allows.

**renameat2**: Same pattern — musl uses `renameat2` for `rename()`. Downgraded to `renameat`.

**process_madvise**: Advisory memory advice syscall. Safe to noop.

**setuid/setgid/setreuid/setregid/setfsuid/setfsgid**: The zygote blocks all UID/GID-changing
syscalls. fake_id0 extension handles the semantic translation, so these handlers just return 0.

**clone3**: Converts the clone3 args struct (passed via pointer) to individual register args for
the clone syscall. Maps `args_ptr+offset` fields to SYSARG_1-5. Returns -ENOSYS if CLONE_THREAD
is set (thread creation inside proot is not supported). Strips CLONE_VM and CLONE_VFORK to
convert vfork-style spawns into regular fork().

**clone**: Same logic as clone3 but for the legacy clone syscall.

### Critical infrastructure: `push_specific_regs(true)` in seccomp handler

In `restart_syscall_after_seccomp()`, our version calls `push_specific_regs(tracee, true)`
(passing `including_sysnum = true`), while termux passes `false`. This is essential for all
downgrade handlers (`PR_faccessat2→PR_faccessat`, `PR_openat2→PR_openat`, `PR_renameat2→PR_renameat`,
`PR_clone3→PR_clone`) — without `true`, the `set_sysnum()` syscall number change would not take
effect when the kernel processes the registers.

### Enhanced default case

The `default:` case in the SIGSYS handler differentiates between syscalls:

- **`PR_openat` and `PR_fstatat64`**: Returns `-ENOENT` instead of `-ENOSYS`. This is critical
  because musl's ldso `path_open()` treats ENOENT as "try next path" but treats ENOSYS (and
  all other errors) as "abort search" (returns -2, inhibiting further path lookup). Without
  this, a single openat hitting seccomp on a non-existent library path (e.g.
  `/usr/lib/perl5/core_perl/CORE/libncursesw.so.6`) would prevent the ldso from finding the
  real library at `/usr/lib/libncursesw.so.6`, causing "Error loading shared library: Function
  not implemented".

- **All other unknown syscalls**: Returns `-ENOSYS` and logs to
  `/data/data/id.or.oo.pr/cache/sigsys-log.txt` for diagnostics.

---

## 3. Event Loop (`tracee/event.c`)

### Architecture change

| Aspect | vendor/proot | vendor/termux-proot | Our version |
|--------|-------------|--------------------|----|
| Seccomp install | Installs BPF via `enable_syscall_filtering()` | Disabled | **Disabled** (replaced with `(void) tracee;`) |
| Kernel version detection | Two functions: `<4.8` and `>=4.8` | Single function | Single function, no version check |
| Default restart | `PTRACE_CONT` when seccomp handles exit | Mixed | **Always `PTRACE_SYSCALL`** |
| SIGSYS handling | None | None | Full handling in event loop |

### Key changes

1. **Proot's BPF filter disabled**: The `enable_syscall_filtering()` call is replaced with
   a no-op. Proot's own seccomp filter conflicts with the zygote's filter.

2. **Always `PTRACE_SYSCALL`**: Proot intercepts every syscall enter and exit via ptrace.
   This is slower than using seccomp acceleration but ensures reliability on Android where
   the zygote's filter is already present.

3. **SIGSYS handler in event loop**: New case in the `switch(signal)` block:
   - Checks `siginfo.si_code == SYS_SECCOMP`
   - Delegates to `handle_seccomp_event()` from `tracee/seccomp.c`
   - Handles `skip_next_seccomp_signal` for void syscalls where seccomp would fire
     on the replaced -1 syscall number

4. **`seccomp_after_ptrace_enter` detection**: Runtime detection of whether the zygote's
   SIGSYS arrives before or after proot's ptrace syscall-enter notification. This varies
   by kernel version. Controlled by `PROOT_ASSUME_NEW_SECCOMP` env var and auto-detected
   on first `PTRACE_EVENT_SECCOMP`.

5. **Signal suppression during chains**: When proot is executing a chain of syscalls,
   signals are suppressed and queued in `tracee->chain.suppressed_signal` for redelivery
   after the chain completes.

6. **SYSCALL_AVOIDER skip condition simplified**: The guard `seccomp_after_ptrace_enter` was
   removed from the `SYSCALL_AVOIDER` check. In termux, SIGSYS from the avoider was only
   skipped when `seccomp_after_ptrace_enter` was true. Our version always skips it — the
   avoider is proot's own voided syscall, and the SIGSYS is always a false positive from
   the zygote filter firing on the modified -1 syscall number.

---

## 4. Syscall Translation (`syscall/syscall.c`)

### Key changes

1. **`set_sysarg_data()` made public**: Changed from `static` to `extern` so the SIGSYS
   handler can use it.

2. **Syscall number change workaround**: On ARM64, `PTRACE_SETREGSET(NT_ARM_SYSTEM_CALL)`
   sometimes fails to change the syscall number. When this happens, proot makes the
   original syscall fail with invalid args (all 6 args set to -1), then re-launches the
   translated syscall via the chain mechanism.

3. **`push_specific_regs()`**: Replaces `push_regs()`. Takes a boolean `including_sysnum`
   parameter. When false, skips the `PTRACE_SETREGSET` call that changes the syscall number,
   allowing register changes without changing the syscall number.

4. **Chain workaround state machine**: `sysnum_workaround_state` tracks three states:
   `INACTIVE`, `PROCESS_FAULTY_CALL`, `PROCESS_REPLACED_CALL`. This handles the sequence
   where the original syscall fails, then the chained replacement runs.

5. **`restart_current_syscall_as_chained()`**: Queues the current (modified) syscall as a
   chained syscall at the front of the queue when `push_regs` fails to change the syscall
   number.

---

## 5. Syscall Exit Handling (`syscall/exit.c`)

### New handlers

| Handler | Purpose |
|---------|---------|
| `PR_void` result preservation | After SIGSYS handler rewrites syscall to PR_void, preserves the modified SYSARG_RESULT back to the exit |
| Empty path `readlinkat` | When `readlinkat` gets empty path (""), resolves via `readlink_proc_pid_fd()` |
| `PR_execveat` exit | Delegates to `translate_execve_exit()` |
| `PR_utime` ENOSYS fix | Retries with `utimensat` via `fix_and_restart_enosys_syscall()` |
| `PR_statfs`/`PR_statfs64` tmpfs faking | Writes `TMPFS_MAGIC` (0x01021994) for `/dev/shm` path |
| `PR_statx` handling | Delegates to `handle_statx_syscall()` from `tracee/statx.c` |
| `PR_ioctl` FICLONE fix | Changes EACCES to EOPNOTSUPP for FICLONE ioctl (from termux) |
| `PR_readlink`/`PR_readlinkat` `.l2s.` hiding | After `detranslate_path()`, applies `substitute_binding()` to replace detranslated path with host-side binding path for absolute paths, then returns EINVAL if the result's basename starts with `L2S_PREFIX` (`.l2s.`, so it holds whether L2S files live in `PROOT_L2S_DIR` or beside the original) — makes link2symlink's fake symlinks invisible to `readlink()`, preserving original hard-link semantics |

### From termux
- Negative result debug logging (first 50 calls)
- `PR_renameat2` handling (we removed this since seccomp handler downgrades it)

---

## 6. Syscall Tables

### `syscall/sysnums-arm64.h` — 16 new entries

The arm64 syscall table had a gap from ~245 to 434. We filled in entries so proot can
recognize these syscalls when intercepted via ptrace or SIGSYS:

```
[277] = PR_seccomp
[278] = PR_getrandom
[279] = PR_memfd_create
[280] = PR_bpf
[281] = PR_execveat
[282] = PR_userfaultfd
[283] = PR_membarrier
[284] = PR_mlock2
[285] = PR_copy_file_range
[286] = PR_preadv2
[287] = PR_pwritev2
[288] = PR_pkey_mprotect
[289] = PR_pkey_alloc
[290] = PR_pkey_free
[437] = PR_openat2
[440] = PR_process_madvise
```

### `syscall/sysnums-x86_64.h` — 17 new entries

Same set plus `PR_kexec_file_load` (320) for x86_64.

### `syscall/sysnums.list` — 25 new names

Added syscall names for all new entries plus `statx` and `openat2`. Moved `statx` to alphabetical
position (was at end in vendor/proot). Removed `utimensat_time64`.

---

## 7. Memory Operations (`tracee/mem.c`, `tracee/mem.h`)

### POKEDATA workaround

On some ARM64 Android kernels, `ptrace(PTRACE_POKEDATA)` silently fails (returns -1 with
errno unchanged). This is a known kernel bug. Our workaround:

1. Auto-detects if the workaround is needed at runtime
2. Falls back to executing a small assembly stub (`str x1, [x2]`) in the tracee's address
   space when `PTRACE_POKEDATA` fails
3. Saves/restores tracee registers, blocks signals, sets PC to the stub, runs with
   `PTRACE_CONT`, waits for SIGILL (expected from the stub's trap instruction)
4. Only enabled on ARM64 (`HAS_POKEDATA_WORKAROUND` from `arch.h`)

### Other changes

- `write_data()` takes non-const `Tracee *` (required because workaround modifies tracee state)
- Explicit `errno` clearing before `PTRACE_PEEKDATA` loops to prevent false errors
- `mem_prepare_after_execve()`: Sets stub address from loader's instruction pointer
- `mem_prepare_before_first_execve()`: Sets stub address from inline assembly function
- Removed `STACK_ALIGNMENT` from `alloc_mem()`

---

## 8. Assembly Loader (`loader/assembly.S`)

vendor/termux-proot adds a standalone `pokedata_workaround` function for aarch64. Our
version **removed** this standalone function. Instead, the workaround is embedded inline
in `tracee/mem.c` using GCC inline `__asm()`, integrated directly into memory operations.

---

## 9. TLS Alignment (`tls-align.c`) — NEW

```c
__attribute__((aligned(64))) __thread char _tls_align_dummy[64] __attribute__((used));
```

Forces the TLS block to at least 64-byte alignment. Works around Android/Bionic issues
where insufficient TLS alignment causes problems with SIMD or atomic operations.

---

## 10. Architecture Definitions (`arch.h`)

| Aspect | vendor/proot | Our version |
|--------|-------------|-------------|
| `SYSCALL_AVOIDER` | `-1` for all arches | `-2` default; `222` (tuxcall) on ARM EABI; `-1` on ARM64 |
| `LOADER_ADDRESS` (ARM) | `0x10000000` | `0x20000000` |
| `STACK_ALIGNMENT` | `16` | Removed |
| ARM64 dual-ABI | No | Yes: `SYSNUMS_HEADER2 = "syscall/sysnums-arm.h"` |
| `HAS_POKEDATA_WORKAROUND` | No | Yes on ARM64 |
| `HAS_LOADER_32BIT` | No | Yes on ARM64 |
| statx UID/GID offsets | In arch.h | Moved to `tracee/statx.c` |

---

## 11. Register Operations (`tracee/reg.c`, `tracee/reg.h`)

### New functionality

- **ARM32-on-ARM64 register layout** (`reg_offset_armeabi[]`): Maps register indices to
  correct offsets in ARM EABI register structure when running 32-bit ARM on aarch64.
- **`push_specific_regs()`**: Takes `bool including_sysnum` parameter. When false, skips
  `PTRACE_SETREGSET(NT_ARM_SYSTEM_CALL)` — used by the syscall number workaround.
- **NEW register version `ORIGINAL_SECCOMP_REWRITE`**: Saves register state after the
  seccomp handler modifies them, allowing later restoration.
- **`restore_original_regs_after_seccomp_event` flag**: When set, `push_specific_regs()`
  restores from `ORIGINAL_SECCOMP_REWRITE` instead of `ORIGINAL`.
- **32-bit value truncation**: `poke_reg()` on ARM64 with 32-bit tracee writes only
  lower 32 bits.

---

## 12. Tracee Structure (`tracee/tracee.h`)

New fields:

| Field | Purpose |
|-------|---------|
| `ORIGINAL_SECCOMP_REWRITE` | Register version for seccomp-modified state |
| `last_restart_how` | Remembers previous restart method |
| `restore_original_regs_after_seccomp_event` | Flag for seccomp register handling |
| `skip_next_seccomp_signal` | Silently drop next seccomp SIGSYS |
| `seccomp_already_handled_enter` | Skip next SIGTRAP\|0x80 |
| `chain.sysnum_workaround_state` | State machine for syscall number workaround |
| `chain.suppressed_signal` | Queued signal for redelivery after chain |
| `pokedata_workaround_stub_addr/cancelled/relaunched` | For POKEDATA workaround |
| `is_aarch32` | Tracks 32-bit ARM execution on aarch64 |
| `host_exe` | Host-side executable path |
| `host_exe_before_l2s` | Host path saved before link2symlink's `translated_path()` modifies it — used for correct `/proc/self/exe` computation |
| `skip_proot_loader` | Bypass proot loader for host binaries |

---

## 13. Compatibility (`compat.h`)

Added `SYS_SECCOMP` definition (value 1) when not already defined. This is the `si_code`
value in `siginfo_t` for seccomp-generated signals. Some Android NDK versions don't
define this constant.

---

## 14. Syscall Enter Handling (`syscall/enter.c`)

### New handlers

| Handler | Purpose |
|---------|---------|
| `PR_execveat` | Converts to `PR_execve` when AT_FDCWD, else ENOSYS |
| `PR_statx` | Separate handler with AT_SYMLINK_NOFOLLOW flag handling |
| `PR_memfd_create` | Blocks specific names: `"JITCode:*"` (Qt JIT), `"opcache_lock"` (PHP 8.3), `"lib/apk/exec/*"` (apk-tools v3) |
| `PR_clone`/`PR_clone3` | Strips `CLONE_VM` and `CLONE_VFORK` from clone flags when `CLONE_THREAD` is not set — converts Rust's `vfork`-style process spawns into regular `fork()` |

### Android ioctl translation

Under `#ifdef __ANDROID__`, remaps terminal ioctls:
- `TCSETS+TCSAFLUSH` → `TCSETS+TCSANOW` (Termux patches TCSAFLUSH)
- `TCGETS2/TCSETS2/TCSETSW2/TCSETSF2` → `TCGETS/TCSETS/TCSETSW/TCSETSF` (termios2 → termios)

---

## 15. Syscall Chaining (`syscall/chain.c`)

- **`restart_current_syscall_as_chained()`**: Queues current (modified) syscall as a chained
  syscall at the front of the queue. Used when ptrace can't change the syscall number.
- **Chain always uses `PTRACE_SYSCALL`**: Ensures syscall exit is always intercepted.
- **`register_chained_syscall_internal()`**: Refactored with `bool at_front` parameter for
  inserting at list head.

---

## 16. Path Canonicalization (`path/canon.c`)

- **F2FS bug workaround**: Calls `should_skip_file_access_due_to_f2fs_bug()` before `lstat()`
- **`/linkerconfig` special case**: Returns hardcoded `S_IFDIR` (Android path that can't be
  stat'd but is accessible)
- **Error code fix**: Non-final non-directory components return `-ENOENT` instead of `-errno`
- **Removed initial root binding check**: No longer calls `substitute_binding_stat()` for
  the initial "/" component (was too aggressive)

---

## 17. Execve Handling (`execve/enter.c`, `execve/exit.c`)

### enter.c changes
- **`PROOT_UNBUNDLE_LOADER`**: Loader binary loaded from filesystem instead of embedded via
  `ld -b binary`
- **Loader permissions**: Tightened from `u+r,g+r,o+r,u+x,g+x,o+x` to `u+r,u+x`
- **`host_exe` tracking**: New field set to host path before detranslation
- **`host_exe_before_l2s` for exe**: Uses the host path saved before link2symlink resolution for `/proc/self/exe` computation, preventing `.l2s.` path leaks
- **Android library paths**: Adds `/system/lib` and `/system/lib64` to LD_LIBRARY_PATH
- **Recursive interpreter fix**: Gracefully handles ELF with nested interpreter

### exit.c changes
- **Stack alignment**: Uses `sizeof_word(tracee)` or 16 bytes on ARM64
- **`skip_proot_loader` support**: Returns early without transferring load script
- **Thumb mode fix**: Clears `PSR_T_BIT` on ARM EABI when leaving thumb mode
- **ARM64 `is_aarch32` detection**: Sets after execve based on ELF class

---

## 18. Seccomp BPF Configuration (`syscall/seccomp.c`)

Changes to proot's BPF syscall filter list (though the filter itself is disabled at runtime):

| Syscall | vendor/proot | Our version | Reason |
|---------|-------------|-------------|--------|
| `PR_execveat` | Not present | `FILTER_SYSEXIT` | execveat support |
| `PR_faccessat2` | `0` (no sysexit) | `FILTER_SYSEXIT` | Needs exit translation |
| `PR_ioctl` | Not present | `FILTER_SYSEXIT` (Android-only) | FICLONE fix |
| `PR_memfd_create` | Not present | `0` (Android-only) | Name blocking |
| `PR_statfs`/`PR_statfs64` | `0` | `FILTER_SYSEXIT` | tmpfs faking |
| `PR_statx` | `0` | `FILTER_SYSEXIT` | statx emulation |
| `PR_utime` | `0` | `FILTER_SYSEXIT` | ENOSYS retry |
| `PR_renameat2` | Present | Removed | Handled by seccomp handler |
| `PR_utimensat_time64` | Present | Removed | Not needed |

---

## 19. Resource Limits (`syscall/rlimit.c`)

- Changed `struct rlimit` to `struct rlimit64` for Y2038 safety
- Changed `prlimit()` to `prlimit64()`

---

## 20. Ptrace Emulation (`ptrace/ptrace.c`)

- **Removed `PTRACE_O_TRACESECCOMP` rejection**: vendor/proot returned `-EINVAL` when a
  ptracee requested seccomp tracing. Our version allows it — needed for debuggers like
  gdb running inside proot.
- **`stringify_ptrace()` signature fix**: Handles both glibc (`enum __ptrace_request`) and
  other libc (`int`)

---

## 21. Wait/Ptrace Events (`ptrace/wait.c`)

- **SIGSYS support**: New case sets `handled_by_proot_first = true` and suppression flag
- **SIGSYS suppression logic**: When suppressed, either restarts tracee immediately (new
  seccomp order) or synthesizes syscall-exit event for the ptracer (old seccomp order)
- **Chain integration**: After `update_wait_status()`, calls `chain_next_syscall()` on
  success instead of returning early

---

## 22. New Extensions

| Extension | Flag | Purpose |
|-----------|------|---------|
| `ashmem_memfd` | `--ashmem-memfd` | Emulates `memfd_create()` using Android ashmem |
| `hidden_files` | `-H` | Hides `.proot.*` files from guest |
| `fix_symlink_size` | `-L` | Corrects `lstat()` st_size for symlinks |
| `mountinfo` | Auto | Manages `/proc/mountinfo` content inside proot |
| `port_switch` | `-p` | Maps protected ports to higher ports |
| `sysvipc` | `--sysvipc` | System V IPC emulation (shmget, semget, msgget) |

### Expanded fake_id0

Split from a monolithic file into 17+ separate files (access.c, chmod.c, chown.c, etc.).
Adds socket faking, sendmsg faking, getsockopt faking, chroot support, statx support, and
SIGSYS handling for blocked ID-changing syscalls. Has `#ifdef USERLAND` sections for
running without root (Android).

### Modified link2symlink

- `USERLAND` mode with different prefix (`.proot.l2s.` vs `.l2s.`)
- `PROOT_L2S_DIR` support for custom storage directory
- `handle_linkat_from_proc_fd()` for Android's "deleted" file pattern
- F2FS bug integration
- 32-on-64 stat size fix
- `translated_path()` skip list includes `PR_readlink`/`PR_readlinkat` — prevents link2symlink from resolving its fake symlink chain for readlink syscalls, which would cause the kernel to `readlink()` a regular file instead of the actual symlink

---

## 23. CLI (`cli/proot.h`, `cli/cli.c`)

- Version: `"5.4.0-pr"` (custom suffix)
- Removed: CARE tool, Python extension, `--mixed-mode`, `-n`/`--netcoop`
- Added: `--shm-helper` mode for SysV IPC, `PROOT_VERBOSE` env variable
- Auto-initialize mountinfo unless `PROOT_NO_MOUNTINFO` is set
- termux-exec error hint: Detects `libtermux-exec.so` in `LD_PRELOAD` and suggests unsetting

---

## 24. Build System (`GNUmakefile`)

- Removed CARE, Python, pkg-config dependencies
- `CC`, `STRIP`, `OBJCOPY` use `?=` (overridable from environment)
- `PROOT_UNBUNDLE_LOADER` option for external loader binary
- New object files: `tracee/seccomp.o`, `tracee/statx.o`, `path/f2fs-bug.o`, all new
  extensions, `tls-align.o`
- Loader naming changed from `.elf` to `.exe`
- Loader LDFLAGS added `--rosegment`
- New `loader-info.c` generation rule that extracts loader symbol addresses via `readelf`

---

## 25. Other File Changes

| File | Change |
|------|--------|
| `path/proc.c` | Disabled assertion that fails on some devices (termux #1679) |
| `path/temp.c` | Simplified readdir cleanup; `PROOT_TMP_DIR` fallback change |
| `path/path.c` | Removed `ALREADY_OPENED_FD` notification; saves host path to `tracee->host_exe_before_l2s` before `TRANSLATED_PATH` event |
| `loader/loader.c` | Removed GCC version check for `__builtin_unreachable()` |
| `loader/assembly-arm.h` | `bx` instead of `mov pc`; Thumb mode SYSCALL macro |
| `tracee/abi.h` | ARM64 ABI detection for 32-on-64 mode |
| `extension/kompat/kompat.c` | Relaxed statx flags check; removed renameat2 downgrade |
| `extension/extension.h` | New events: `SIGSYS_OCC`, `LINK2SYMLINK_RENAME`, `LINK2SYMLINK_UNLINK`, `STATX_SYSCALL` |

---

## 26. Architectural Summary

### Seccomp strategy inversion
- **vendor/proot**: Installs seccomp BPF for syscall tracing acceleration
- **Our version**: Disables proot's BPF, relies entirely on ptrace + reactive SIGSYS handling

### Always PTRACE_SYSCALL
Our version intercepts every syscall enter/exit via ptrace. This is slower but reliable
on Android where the zygote's filter is already present and conflicts with proot's.

### SIGSYS-as-syscall-path
Android's seccomp SIGSYS is treated as an alternative syscall entry point. Proot handles
the syscall translation in the SIGSYS handler the same way it would in normal ptrace
syscall-enter.

### POKEDATA workaround
For broken ARM64 kernels where `ptrace(PTRACE_POKEDATA)` silently fails, executes a `str`
instruction directly in the tracee's address space.

### Syscall number change workaround
When `PTRACE_SETREGSET(NT_ARM_SYSTEM_CALL)` fails (can't change syscall number), proot
makes the original syscall fail with invalid args and re-launches the translated syscall
via the chain mechanism.

### 32-bit ARM on ARM64
Full support for running 32-bit ARM binaries inside proot on aarch64, including correct
register layout mapping and dual-ABI syscall number tables.

---

## 27. Solved: openat/fstatat64 SIGSYS → ENOENT

### Problem

`vim --version` failed with "Error loading shared library libncursesw.so.6: Function not
implemented" when run from the app process (zygote seccomp active).

### Root cause

The zygote's seccomp filter occasionally blocks `openat` and `fstatat64` via SECCOMP_RET_TRAP
(SIGSYS). This happens when the openat path points to certain non-existent directories (e.g.
`/usr/lib/perl5/core_perl/CORE/`). Proot's SIGSYS handler had a `default:` case that returned
`-ENOSYS` for all unknown syscalls.

In musl's ldso (`dynlink.c`), `path_open()` calls `open()` for each search path. If `open()`
returns an error NOT in `{ENOENT, ENOTDIR, EACCES, ENAMETOOLONG}`, the function returns -2
which inhibits ALL further path search. ENOSYS is not in that safe list, so the entire library
search was aborted — even though the library existed at a later search path.

### Fix

Changed the SIGSYS `default:` handler to return `-ENOENT` for `PR_openat` and `PR_fstatat64`
instead of `-ENOSYS`. ENOENT IS in musl's safe list, so the ldso continues searching other
paths and successfully finds the library.

### Discovery method

A syscall tracer was temporarily added to `translate_syscall()` in `syscall/syscall.c` that
logs every syscall enter/exit with register values to `/data/data/id.or.oo.pr/cache/syscall-trace.txt`.
Combined with enhanced SIGSYS logging (register args + path via `read_string()`), this revealed
that `openat` for `/usr/lib/perl5/core_perl/CORE/libncursesw.so.6` was the failing call.

**Note**: The tracer must call `trace_log_syscall()` AFTER `fetch_regs()`, not before. On
aarch64, `SYSARG_1` and `SYSARG_RESULT` both map to `regs[0]` (x0). Before `fetch_regs()`,
the cached register values are stale from the previous syscall stage, producing misleading
data (e.g. AT_FDCWD=-100 shown as the "result" when it was actually the stale enter-stage arg).

---

## 28. Solved: lchown/utimensat ENOENT during dpkg unpack

### Problem

`apt-get install` on Debian failed during dpkg's unpack phase with:

```
error setting ownership of /usr/bin/perlthanks.dpkg-new: No such file or directory
error setting timestamp of /usr/bin/perlthanks.dpkg-new: No such file or directory
```

These are `.dpkg-new` files created by dpkg while unpacking a package archive. dpkg
uses `lchown()` to set file ownership and `utimensat()` to set timestamps on each
unpacked file. Both failed with ENOENT.

### Root cause — link2symlink interaction with Android SELinux

dpkg creates `.dpkg-new` files, then renames them over the real path once unpacked.
With `--link2symlink` enabled (required on Android — SELinux blocks real `link()`
outright), every `link()` call that dpkg makes goes through L2S and produces an L2S
symlink chain.

`translated_path()` in `link2symlink.c` has a `TRANSLATED_PATH` event handler that
dereferences L2S symlink chains: when a syscall touches a path that is an L2S symlink,
proot resolves the chain to give the kernel the actual `.l2s/` content file path.

For `lchown` and `utimensat`, this dereference causes the kernel to see a `.l2s/`
content path deep inside the app data directory. Android SELinux (`untrusted_app`
domain) returns ENOENT for `lchown`/`utimensat` on such paths — it is masquerading
EPERM as ENOENT to prevent information leakage.

dpkg treats EPERM as ignorable (can continue) but aborts on ENOENT, so the install
fails.

### Fix — three coordinated changes

**1. `link2symlink.c` — skip L2S dereferencing for chown/utimes syscalls**

Added `lchown`, `chown`, `fchown`, `fchownat`, `utimes`, `utimensat`, `futimesat`,
`utime` to the `translated_path()` exclusion list. When these syscalls touch an L2S
symlink, proot now passes the symlink path itself to the kernel rather than resolving
the chain. The kernel operates on the L2S symlink node (not the content file), avoiding
the deep-app-data path that Android SELinux blocks.

**2. `chown.c` (non-USERLAND) — cancel the chown syscall entirely**

Even with the L2S skip, Android SELinux still blocks `lchown` on the app data dir
with ENOENT. The fix replaces the `lchown` syscall with `getuid` in the ENTER phase
(`set_sysnum(tracee, PR_getuid)`), so the kernel executes `getuid` instead. The kernel
never attempts the ownership change at all.

```c
/* On Android the kernel (SELinux untrusted_app domain) returns ENOENT
 * (a masqueraded EPERM) for chown/lchown on files inside the app data
 * dir.  Fix: replace the syscall with getuid, which fake_id0 will
 * intercept in the EXIT phase and fake to return config->ruid (0). */
set_sysnum(tracee, PR_getuid);
```

**3. `fake_id0.c` — force getuid/void result to 0 in both USERLAND and non-USERLAND**

`getuid` returns the Android app UID (e.g. 10234) as the lchown return value. Callers
(dpkg, tar) see a non-zero result and report "Cannot change ownership". The fix: in
`handle_perm_err_exit_end`, force the result to 0 whenever the current syscall number
is `PR_getuid` or `PR_void` and the result is non-zero. Previously this block was
guarded by `#ifdef USERLAND`, so non-USERLAND builds did not zero it.

For `utimensat`/`utimes`/`futimesat`/`utime`, a separate EXIT handler in the
non-USERLAND code path forces any negative result to 0 when `euid == 0`, except
ENOENT for an absolute path that does not exist on the host (`utime_target_missing()`).
`touch(1)` and Python's `Path.touch()` call `utimensat` first and create the file
only after that ENOENT; zeroing it made them report success without creating anything.
An L2S-converted symlink always exists on the host, so the dpkg/tar case still gets 0.

### Files changed

| File | Change |
|------|--------|
| `extension/link2symlink/link2symlink.c` | Added chown+utimes variants to `translated_path()` exclusion list |
| `extension/fake_id0/chown.c` | non-USERLAND: replace lchown with `PR_getuid` via `set_sysnum` |
| `extension/fake_id0/fake_id0.c` | Removed `#ifdef USERLAND` guard from PR_getuid/PR_void result-zeroing; added non-USERLAND EXIT handler for utimes variants |

---

## 29. Solved: Hardcoded Package Name in Bind Mounts (`src/pr-cli/src/shared.rs`)

Discovered while building `rs.oo.or.id` (a fork of the `pr` engine with a different package name).

### Problem

`build_proot_args()` in `shared.rs` hardcoded `/data/data/id.or.oo.pr/cache` and
`/data/data/id.or.oo.pr` as the bind mount paths passed to proot's `--bind`. Any app
with a different Android package name (e.g. `id.or.oo.rs`) got `Permission denied`
warnings and the bind mounts silently failed.

### Impact on link2symlink

Alpine's `apk add` creates hard links via proot's `link2symlink` mechanism. The `.l2s/`
directory that stores the symlink targets is placed inside the app data directory. Without
the correct bind mount (pointing at the actual app's data dir), proot cannot resolve `.l2s`
symlinks — so binaries installed by `apk add gcc` appear installed (`ls -l /usr/bin/gcc`
shows them) but become invisible to the guest shell:

```
/bin/sh: gcc: not found
```

This was the **root cause of the "linker `cc` not found"** error when running `rustc` on
Alpine (previously misattributed to a missing `cc` symlink and documented as a known
Alpine-specific test failure).

### Fix

Derive `app_dir` dynamically from `APP_PREFIX` instead of hardcoding the package name:

```rust
let app_dir = Path::new(&prefix).parent().unwrap().parent().unwrap();
args.push(format!("--bind={}/cache", app_dir.display()));
args.push(format!("--bind={}", app_dir.display()));
```

`APP_PREFIX` is always `<app_data_dir>/files/usr`, so `.parent().parent()` gives
`<app_data_dir>` regardless of package name.

### Files changed

| File | Change |
|------|--------|
| `src/pr-cli/src/shared.rs` | `build_proot_args()`: derive `app_dir` from `APP_PREFIX` via `.parent().parent()` |

---

## 30. Solved: kernel auxv views expose the loader (Ubuntu 26.04 coreutils)

### Problem

Ubuntu 26.04 ships uutils coreutils 0.8 (`rust-coreutils`) as `/usr/bin/*`. Every
command failed with `coreutils: unknown program 'libproot-loader'`, which also broke
`dpkg --configure` (maintainer scripts call `mkdir`, `chmod`, `env`, ...).

The multicall binary picks the utility from `AT_EXECFN` unless its basename equals
argv[0]'s (`validation::binary_path`). It reads auxv with rustix, which uses
`prctl(PR_GET_AUXV)` and falls back to `/proc/self/auxv`. Both return the kernel's
saved copy, where `AT_EXECFN` is the path the kernel executed: proot's loader.

The loader rewrites auxv only on the initial stack (`AT_PHDR`, `AT_PHNUM`, `AT_ENTRY`,
`AT_BASE`, `AT_EXECFN`), and fake_id0 rewrites the ids there too. `getauxval()` reads
that copy, so most programs never noticed the kernel's views disagreeing on every
patched value, plus `AT_SYSINFO_EHDR`.

### Fix

`execve/exit.c` records the initial-stack auxv address in `LoadInfo.auxv_address`
before the load script moves the stack pointer. The new built-in `auxv` extension
serves that copy from both kernel views:

- `open`/`openat` of `/proc/<pid>/auxv` for a tracee is redirected to a temp file
  holding the tracee's stack auxv (same pattern as the mountinfo extension).
- On `prctl(PR_GET_AUXV)` exit, the buffer is overwritten with the stack auxv,
  zero-padded; the result (the kernel's copy size) is kept.

### Files changed

| File | Change |
|------|--------|
| `execve/execve.h` | `LoadInfo.auxv_address` |
| `execve/exit.c` | Record it at execve exit |
| `extension/auxv/auxv.c` | New extension: `/proc/<pid>/auxv` and `PR_GET_AUXV` |
| `extension/extension.h`, `cli/cli.c`, `GNUmakefile` | Declare, always enable, build it |

The integration test "auxv views match the stack" compares every record of both
views with `getauxval()`.

---

## 31. Solved: SO_PEERCRED for peers in another PRoot session

### Problem

fake_id0's `getsockopt(SO_PEERCRED)` exit handler replaced the peer's uid/gid with fake ids only
when the peer pid was a tracee of the same PRoot. A server started from one `pr-cli login`
session (or by the host side) and a client in another session saw each other with the real
app uid while their own `getuid()` reported 0. Programs that authenticate a Unix-socket peer by
comparing the two rejected each other: Hermes discovery reported `unsafe_control_peer` for a
gateway started from another terminal session.

### Fix

`extension/fake_id0/getsockopt.c`: when the peer is not our tracee, a uid/gid equal to PRoot's
real uid/gid is reported as this tracee's fake euid/egid, the same rule that makes files owned by
the real user appear owned by root. Peers running as any other uid are reported unchanged.

Verified in the Alpine arm64 VM: a server outside PRoot and a client inside it now agree
(`peer_uid=0`, was 1000); integration suite 39/39.
