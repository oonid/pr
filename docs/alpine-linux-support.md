# Alpine Linux on Android via proot

> Historical record of the Alpine bring-up (Phase 5, targetSdk 28). Several details have changed since: `linkat` now returns `-EPERM`, the set*id handlers return 0, sessions run as fake root (uid 0), and targetSdk is 35. The current seccomp handlers are in docs/proot-improvement.md §2.

## Status: Login + package management + openssh work

Alpine 3.23.3 installs, logs in, and runs `apk update`/`apk add`/`apk del` successfully from
the app process with targetSdk=28. `vim --version`, `curl --version`, and `ssh -V` all work.

## What works

- **Install**: pr-cli downloads, extracts, and configures Alpine rootfs end-to-end
- **Login**: proot execs `/bin/sh -l`, shell prompt appears, commands execute
- **id**: `uid=10515(aid_root)` — proot fake root works
- **uname -a**: `Linux localhost 6.17.0-pr ... aarch64`
- **cat /etc/os-release**: Alpine Linux v3.23.3
- **busybox wget HTTP**: Downloads files over HTTP successfully (511KB tested)
- **busybox ssl_client**: Executes and performs TLS handshake successfully
- **fork/exec/pipe/socket**: All basic POSIX operations work inside proot
- **Remove**: `pr-cli remove alpine` wipes rootfs, exit code 0
- **Re-install**: Second install works after remove
- **apk update / apk add / apk del**: All package operations work (fixed in T5.2)
- **apk add openssh**: Installs with 0 errors on fresh Alpine (fixed)
- **vim --version**: Works with full ncurses support (fixed)
- **curl --version**: Works with full TLS/HTTP2 (tested)
- **ssh -V**: OpenSSH works after `apk add openssh` (fixed)

## Remaining issues

### busybox wget HTTPS

```
wget: can't execute 'ssl_client': Function not implemented
```

Busybox wget tries to `execvp("ssl_client")` to handle HTTPS. The external
`/usr/bin/ssl_client` binary (from LibreSSL, 67KB, dynamically linked against
`libssl.so.3` and `libcrypto.so.3`) can be executed directly, but busybox's
internal spawn mechanism fails with ENOSYS inside proot from the app process.

**Root cause**: Alpine's busybox is compiled without internal TLS support
(`FEATURE_WGET_HTTPS` is disabled or uses NOMMU path). Busybox wget always
spawns the external `ssl_client` binary via `vfork`+`execvp` for HTTPS.
The ENOSYS comes from proot's handling of the `execvp` syscall in the
traced child process, combined with the Android zygote's seccomp filter.

## Seccomp environment

| Context | Seccomp | Notes |
|---------|---------|-------|
| App process (`untrusted_app`) | Seccomp: 2, Filters: 1 | Zygote-installed BPF filter |
| `run-as` (`runas_app`) | Seccomp: 0, Filters: 0 | No filter |

### Blocked syscalls (from zygote seccomp filter, aarch64)

These syscalls trigger SIGSYS (SECCOMP_RET_TRAP) from the zygote's seccomp filter:

| Syscall | Number | SIGSYS handler | Strategy |
|---------|--------|---------------|----------|
| `getcwd` | 17 | Yes | Read `tracee->fs->cwd`, write to tracee buffer, return length |
| `chdir` | 49 | Yes | Translate path, update `tracee->fs->cwd`, return 0 |
| `fchdir` | 50 | Yes | Resolve dirfd to path, update cwd, return 0 |
| `linkat` | 37 | Yes | Translate paths, try renameat(), fallback to copy+delete |
| `faccessat2` | 439 | Yes | Downgrade to `faccessat` (drop flags) |
| `renameat2` | 276 | Yes | Downgrade to `renameat` (drop flags) |
| `process_madvise` | 440 | Yes | Return 0 (noop — advisory) |
| `setgid` | 144 | Yes (default) | Return -ENOSYS |
| `setuid` | 146 | Yes (default) | Return -ENOSYS |
| `memfd_create` | 279 | Yes (default) | Return -ENOSYS |
| `openat` | 56 | Yes (default) | Return **-ENOENT** (not -ENOSYS!) |
| `fstatat64` | 79 | Yes (default) | Return **-ENOENT** (not -ENOSYS!) |

The `openat` and `fstatat64` SIGSYS returns `-ENOENT` instead of `-ENOSYS` because musl's
ldso `path_open()` treats ENOENT as "continue searching other paths" but ENOSYS as "abort
all path search". See "Fix Applied: vim" section below for details.

### Why openat sometimes gets blocked by seccomp

Most `openat` calls succeed fine. The zygote's seccomp only blocks certain `openat` calls —
specifically those where proot has already translated the path and the translated path hits
a seccomp rule. The exact trigger condition is unclear (may depend on the translated path
length, specific path prefix, or register state after proot's path translation). The
important thing is that returning ENOENT instead of ENOSYS makes the failure non-fatal.

## run-as vs app process comparison

| Operation | run-as (no seccomp) | App process (seccomp: 2) |
|-----------|--------------------|-------------------------|
| `apk update` | Works | Works (fixed) |
| `apk add vim` | Works | Works (fixed) |
| `apk del vim` | Works | Works (fixed) |
| `apk add openssh` | Works | Works (fixed) |
| `vim --version` | Works | Works (fixed) |
| `curl --version` | Works | Works (fixed) |
| `ssh -V` | Works | Works (fixed) |
| `busybox wget HTTP` | Works | Works |
| `busybox wget HTTPS` | Works | ENOSYS (can't exec ssl_client) |
| `ssl_client` direct exec | Works | Works |
| `/bin/sh -c ssl_client` | Works | Works |
| `execvp("ssl_client")` from busybox | Works | ENOSYS |

## Alpine rootfs details

### ssl_client binary

- Path: `/usr/bin/ssl_client`
- Size: 67KB
- Type: ELF64 dynamic, aarch64
- Linker: `/lib/ld-musl-aarch64.so.1`
- Dependencies: `libssl.so.3`, `libcrypto.so.3`, `libc.musl-aarch64.so.1`
- Note: This is from the LibreSSL/openssl package, NOT busybox's ssl_client applet

### busybox

- Version: 1.37.0
- Size: 919KB (dynamic, musl-linked)
- TLS: No internal TLS (`tls_handshake` not in binary)
- HTTPS strategy: Spawns external `ssl_client` via `execvp`
- `ssl_client` string present (applet reference)

### apk

- Path: `/sbin/apk`
- Dependencies: `libapk.so.3.0.0`, `libz.so.1`, `libc.musl-aarch64.so.1`
- libapk includes built-in libfetch with OpenSSL 3.x support
- Uses `TLS_client_method`, `libssl.so.3` for HTTPS
- Uses `posix_spawnp` for script execution

### Environment inside proot login shell

```
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
HOME=/root
USER=root
LANG=en_US.UTF-8
TERM=xterm-256color
TMPDIR=/tmp
```

These are set correctly via `/etc/profile` sourced by `/bin/sh -l`.

## Investigation approach

A Rust syscall tester (`enosys_test`) was built using the same NDK cross-compilation
toolchain as pr-cli. It tests individual syscalls by calling `libc::syscall()` directly
and checking for ENOSYS. The binary is pushed to Alpine's `/root/` and executed inside
proot from the app process via the terminal.

Additionally, a syscall tracer was temporarily added to proot's `translate_syscall()`
in `syscall/syscall.c` to log every syscall enter/exit with register values. Combined
with enhanced SIGSYS logging that shows register args and file paths, this revealed
the exact openat call and path that caused the vim failure.

**Important tracer lesson**: On aarch64, `SYSARG_1` and `SYSARG_RESULT` both map to
`regs[0]` (x0). The tracer must run AFTER `fetch_regs()`, otherwise cached register
values are stale from the previous syscall stage. Reading `SYSARG_RESULT` at EXIT
before `fetch_regs()` returns the enter-stage `SYSARG_1` value instead.

**Danger**: Calling `read_path()` in the tracer (to log file paths for openat) causes
the proot parent process to crash with SIGSEGV (signal 11). This happens because
accessing tracee memory via ptrace during critical transitions (like execve) is unsafe.
Do NOT add path reading to the tracer again.

## Fix Applied (T5.2): apk operations

Root cause identified: the zygote's seccomp filter blocks `faccessat2` (439) and
`renameat2` (276) on arm64. When these syscalls are blocked, the kernel sends SIGSYS
to the tracee. proot catches the SIGSYS in `handle_seccomp_event_common()` (seccomp.c),
but the handler had no cases for `faccessat2` or `renameat2`, so they hit the `default:`
case which returned ENOSYS.

Modern musl (Alpine's libc) uses `faccessat2` for access() and `renameat2` for rename()
when the kernel supports them. This is why `apk` (which uses musl) failed while
`busybox wget` (which may use older syscall variants) worked.

**Fix**: Added downgrade handlers in `seccomp.c`:
- `faccessat2(dirfd, path, mode, flags)` → `faccessat(dirfd, path, mode)` (drop flags)
- `renameat2(olddirfd, oldpath, newdirfd, newpath, flags)` → `renameat(olddirfd, oldpath, newdirfd, newpath)` (drop flags)
- `process_madvise(...)` → return 0 (advisory, safe to noop)

Also added `PR_process_madvise` to the syscall tables (sysnums.list, sysnums-arm64.h,
sysnums-x86_64.h). Note: the original enosys_test used wrong syscall number 281 for
process_madvise (281 is actually execveat on arm64); corrected to 440.

## Fix Applied (T5.2): vim --version

Root cause: the zygote's seccomp filter blocks `openat` (56) and `fstatat64` (79) via
SIGSYS in certain cases. proot's `default:` SIGSYS handler returned `-ENOSYS`. In musl's
ldso `path_open()` (dynlink.c:871), `open()` errors are handled by a switch statement:

```c
switch (errno) {
case ENOENT:
case ENOTDIR:
case EACCES:
case ENAMETOOLONG:
    break;        // try next path
default:
    return -2;    // inhibit ALL further search
}
```

ENOSYS falls into `default:`, which returns -2 and prevents the ldso from searching
other paths. The library exists at `/usr/lib/libncursesw.so.6` but the ldso aborted
search after failing on `/usr/lib/perl5/core_perl/CORE/libncursesw.so.6`.

**Fix**: Changed the SIGSYS `default:` handler to return `-ENOENT` for `PR_openat`
and `PR_fstatat64`. ENOENT is in the safe list, so the ldso continues searching and
finds the library at the correct path.

**SIGSYS events during a `vim --version` run** (from sigsys-log.txt):
```
SIGSYS: kernel_num=144 pr=317 args=[...]          # setgid → -ENOSYS (harmless)
SIGSYS: kernel_num=146 pr=338 args=[...]          # setuid → -ENOSYS (harmless)
SIGSYS: kernel_num=79 pr=90 args=[...]            # fstatat64 → -ENOENT (search continues)
SIGSYS: kernel_num=56 pr=223 args=[...] path="..."  # openat → -ENOENT (search continues)
```

**Files changed**:
- `src/proot/src/tracee/seccomp.c` — SIGSYS downgrade handlers + ENOENT fix for default case

## Fix Applied (T5.2): openssh / busybox trigger

Root cause: two bugs prevented `apk add openssh` from completing successfully.

**Bug 1 — aarch64 x0 clobber**: On aarch64, `SYSARG_1` and `SYSARG_RESULT` both map to
register x0. At SIGSYS time, the kernel clobbers x0 before proot reads it. The fchdir
handler read `dirfd=0` (stdin) instead of the real directory fd, causing `translate_path()`
to return ENOTDIR (-20). Same issue affected `PR_chdir` (path pointer) and `PR_linkat`
(olddirfd).

**Fix**: Changed `PR_chdir`, `PR_fchdir`, `PR_linkat` to read `SYSARG_1` from `ORIGINAL`
register version (saved by `save_current_regs(ORIGINAL_SECCOMP_REWRITE)` right after
`fetch_regs()`) instead of `CURRENT`. `SYSARG_2`-`SYSARG_6` (x1-x5) are unaffected since
they don't overlap with the result register.

**Bug 2 — missing getcwd handler**: The zygote blocks `getcwd` (syscall 17). Without a
handler, the default case returned -ENOSYS. The busybox trigger script calls `getcwd()`
during `busybox --install -s`, which failed silently.

**Fix**: Added `PR_getcwd` SIGSYS handler that reads proot's tracked cwd
(`tracee->fs->cwd`), verifies the path exists via `translate_path()`, writes it to the
tracee's output buffer, and returns the string length. Modeled after the existing
`PR_getcwd` exit handler in `exit.c`.

**Files changed**:
- `src/proot/src/tracee/seccomp.c` — x0 clobber fix + PR_getcwd handler
