# Important Notes

See also:
- `docs/rust-toolchain-support.md` — Rust toolchain support (vfork/CLONE_VM fix, link2symlink readlink fix)
- `docs/targetsdk35-compatibility.md` — targetSdk 35 compatibility (SELinux W^X bypass, seccomp analysis)
- `docs/rust-port-pr-cli.md` — Replace proot-distro.sh with Rust binary
- `docs/alpine-linux-support.md` — Alpine Linux investigation and fixes
- `docs/integration-tests.md` — Integration test suite (46 tests)
- `docs/proot-improvement.md` — Our proot fork vs vendor/proot and vendor/termux-proot

---

## Android W^X / SELinux / targetSdk Restrictions

Date: 2026-04-15
Device: Samsung, Android 16 (SDK 36), aarch64 ()
Context: Discovered during T5.2/T6.9 integration testing

### What is W^X

Android enforces Write-XOR-Execute (W^X) policy on app processes. The kernel blocks `execve()` on files located in app-writable directories (labeled `app_data_file` by SELinux). This prevents apps from downloading and running arbitrary code.

### The Three Blockers (targetSdk 29+)

When proot runs from the app process (`u:r:untrusted_app:s0`) with targetSdk >= 29:

```
proot error: execve("/bin/sh"): Permission denied            ← W^X blocks execve on app_data_file
proot error: can't chmod '/data/.../proot-XXXX': Function not implemented  ← seccomp ENOSYS
proot error: can't chdir to '/': Function not implemented     ← seccomp ENOSYS
```

1. **execve Permission denied**: Proot's child tries to exec `/bin/sh` which resolves to the rootfs in `/data/data/...`. SELinux blocks it because the file has the `app_data_file` label.

2. **chmod ENOSYS**: Proot's tracer code needs to `chmod()` a temp file. The Android zygote's seccomp filter returns ENOSYS (Function not implemented).

3. **chdir ENOSYS**: Proot's tracer code needs to `chdir()` into the rootfs. Same seccomp filter blocks it.

### Why run-as Works But App Doesn't

```
run-as id.or.oo.pr   → SELinux context: u:r:runas_app:s0:c3,c258,c512,c768  → proot works
app process          → SELinux context: u:r:untrusted_app:s0:c3,c258,c512,c768  → proot blocked
```

`runas_app` has different SELinux policies than `untrusted_app`. The W^X and seccomp restrictions only apply to `untrusted_app`.

### targetSdk Threshold

Tested on device:

| targetSdk | proot login | Notes |
|-----------|-------------|-------|
| 28 | **WORKS** | No W^X enforcement. Proot can execve, chmod, chdir freely. |
| 29+ | **WORKS** | Uses PROOT_LOADER mechanism (Phase 7) to bypass W^X. Requires unbundled loader binary in nativeLibraryDir. |

**Conclusion**: targetSdk 35 works via the PROOT_LOADER mechanism. The loader (`libproot-loader.so`) lives in nativeLibraryDir where SELinux allows execve, and proot uses it to exec guest binaries without hitting W^X.

### nativeLibraryDir is the Exception

Files in the APK's native library directory can be executed even from `untrusted_app`:

```
/data/app/~~<random>/<package>-<random>/lib/arm64/  → SELinux allows execve()
/data/data/<package>/files/                          → SELinux denies execve()
```

This is why `libproot.so`, `libbusybox.so`, `libpr-cli.so` can all be exec'd — they're in nativeLibraryDir. But the rootfs binaries in `/data/data/.../installed-rootfs/` cannot.

### Why Proot Bind Mounts Don't Help

Proot's `--bind` is virtual — it only translates paths in ptrace syscall interception. When the kernel actually performs the `execve()`, it sees the real filesystem path (in `/data/data/...`), not the virtual guest path. So binding `libbusybox.so:/bin/sh` doesn't help — the kernel still tries to exec the real rootfs file.

### Why the Fork Matters

The forkPty() JNI function forks the app process. The child inherits `untrusted_app` SELinux context AND the zygote's seccomp filter. This is different from `run-as` which creates a new process in `runas_app` context.

### Implications for Future Android Versions

- **Android 15+**: Google may further restrict ptrace or native library execution
- **Samsung Knox**: Some firmware mounts `/data/data/<pkg>/files/` with `noexec` flag
- **Yama ptrace_scope**: Must be 0 or 1 for proot to work (Samsung defaults to 1, which allows parent→child ptrace)

### Potential Workarounds (Future Research)

1. **Static ELF Loader** (`libexecloader.so`): A custom no_std Rust binary in nativeLibraryDir that maps target ELF into memory via mmap (not execve). Proot rewrites `execve("/bin/sh")` to `execve("libexecloader.so", ["/bin/sh"])`. Solves the execve blocker but NOT the chmod/chdir seccomp blockers.

2. **Guest dynamic linker as proxy**: Copy `ld-musl-aarch64.so.1` into nativeLibraryDir. Proot rewrites execve to go through the linker. Unlikely to work because the linker needs a full Linux ABI.

3. **targetSdk 35 with PROOT_LOADER**: Current working solution. The unbundled loader binary in nativeLibraryDir handles execve for guest binaries. Play Store compatible.

### Zygote Seccomp Filter (targetSdk 35, aarch64)

Even at targetSdk=35, the Android zygote installs a BPF seccomp filter on the app process
(`Seccomp: 2, Seccomp_filters: 1`). This filter persists even with `PROOT_NO_SECCOMP=1`
(which only disables proot's own seccomp filter, not the zygote's).

**Blocked syscalls** (trigger SIGSYS from zygote filter):

| Syscall | Number | Handler | Strategy |
|---------|--------|---------|----------|
| `getcwd` | 17 | `PR_getcwd` | Read `tracee->fs->cwd`, write to tracee buffer, return length |
| `chdir` | 49 | `PR_chdir` | Translate path, update `tracee->fs->cwd`, return 0 |
| `fchdir` | 50 | `PR_fchdir` | Resolve dirfd to path, update cwd, return 0 |
| `linkat` | 37 | `PR_linkat` | Try `renameat()`, fallback to copy+delete on EXDEV/EACCES |
| `renameat2` | 276 | `PR_renameat2` | Downgrade to `renameat` (drop flags) and restart |
| `faccessat2` | 439 | `PR_faccessat2` | Downgrade to `faccessat` (drop flags) and restart |
| `faccessat` | 48 | `PR_faccessat` | Return 0 (noop) |
| `openat2` | 437 | `PR_openat2` | Downgrade to `openat` (clear size arg) and restart |
| `process_madvise` | 440 | `PR_process_madvise` | Return 0 (noop — advisory) |
| `fchmodat` | 53 | `PR_fchmodat` | Return 0 (noop) |
| `clone3` | 435 | `PR_clone3` | Convert args to clone, strip CLONE_VM/CLONE_VFORK |
| `clone` | 220 | `PR_clone` | Strip CLONE_VM/CLONE_VFORK |
| `setuid` | 146 | `PR_setuid` | Return 0 (noop — fake_id0 handles semantics) |
| `setgid` | 144 | `PR_setgid` | Return 0 (noop) |
| `setreuid`/`setregid`/`setfsuid`/`setfsgid` | various | respective | Return 0 (noop) |
| `memfd_create` | 279 | default | Return -ENOSYS |
| `openat` | 56 | default | Return **-ENOENT** (not -ENOSYS) |
| `fstatat64` | 79 | default | Return **-ENOENT** (not -ENOSYS) |

Note: original enosys_test incorrectly used 281 for process_madvise; 281 is execveat on arm64.

**aarch64 x0 clobber bug**: On aarch64, `SYSARG_1` and `SYSARG_RESULT` both map to register
x0. At SIGSYS time the kernel may clobber x0 before proot reads it. All handlers that read
`SYSARG_1` use `ORIGINAL` register version (saved right after `fetch_regs()`) instead of
`CURRENT`. `SYSARG_2`-`SYSARG_6` (x1-x5) are unaffected. Without this fix, `fchdir(fd)`
appeared as `fchdir(0)` because x0 was clobbered to 0.

**openat/fstatat64 ENOENT fix**: The SIGSYS `default:` handler returns `-ENOENT` instead of
`-ENOSYS` for these two syscalls. This is critical because musl's ldso `path_open()` treats
ENOENT as "continue searching" but ENOSYS as "abort all search". Without this fix, a single
failed openat on a non-existent path would prevent the dynamic linker from finding the real
library.

**Verified working** (app process with seccomp: 2):
- Alpine: `apk update`, `apk add vim gcc rust cargo git openssh` — all 0 errors
- Debian: `apt update && apt install vim gcc rustc cargo git openssh-client` — all 0 errors
- Full test suite (41 tests including ssh suite) on both Alpine and Debian

### Key Files

- `android/app/build.gradle.kts` — `targetSdk = 35` (works via PROOT_LOADER mechanism)
- `src/proot/src/tracee/event.c` — seccomp patch (skip proot's own seccomp filter)
- `src/proot/src/tracee/seccomp.c` — SIGSYS handlers (18 handlers, x0 clobber fix, ENOENT for openat/fstatat64)
- `src/pr-cli/src/login.rs` — proot invocation with PROOT_LOADER, PROOT_TMP_DIR, arg0("proot"), nativeLibraryDir paths
- `src/pr-cli/src/shared.rs` — `build_proot_args()` with `--change-id=0:0`, `--kernel-release`, bind mounts
- `docs/alpine-linux-support.md` — Alpine-specific investigation and seccomp findings
- `docs/proot-improvement.md` — Full comparison of our proot vs vendor versions

---

## Debian dpkg / apt Quirks on Android

`apt-get install` inside proot on Android hits two failures that do not occur on a native
Linux system. Both stem from Android SELinux (`untrusted_app` domain) blocking filesystem
operations that dpkg and package postinst scripts rely on.

See `docs/proot-improvement.md §28` for the proot C-level fixes (lchown/utimensat ENOENT).
This section covers the CLI-level workarounds in `src/pr-cli/src/cmd_test.rs`.

### Failure 1 — dpkg unpack: lchown/utimensat ENOENT on `.dpkg-new` files

**Symptom:**
```
error setting ownership of /usr/bin/perlthanks.dpkg-new: No such file or directory
error setting timestamp of /usr/bin/perlthanks.dpkg-new: No such file or directory
```

**Cause:** link2symlink converts dpkg's `link()` calls into L2S symlink chains. proot's
`translated_path()` dereferences those chains for `lchown`/`utimensat`, handing the kernel
a deep app-data path. Android SELinux returns ENOENT (masqueraded EPERM) for such paths.
dpkg aborts on ENOENT.

**Fix:** proot C changes in `chown.c`, `fake_id0.c`, `link2symlink.c` (see §28 in
`proot-improvement.md`). No CLI workaround needed for this failure.

---

### Failure 2 — openssh-client postinst: groupadd cannot lock /etc/group

**Symptom:**
```
groupadd: /etc/group.23430 file stat error: No such file or directory
groupadd: cannot lock /etc/group; try again later.
fatal: `/sbin/groupadd -g 101 _ssh' returned error code 10. Exiting.
dpkg: error processing package openssh-client (--configure):
  installed openssh-client package post-installation script subprocess returned error exit status 82
```

**Cause:** `groupadd` locks `/etc/group` by creating a hard link `/etc/group.NNNNN`. Android
SELinux (`untrusted_app`) blocks `link()` outright with EPERM, so link2symlink intercepts it
and creates an L2S symlink chain instead. `groupadd` then calls `stat("/etc/group.NNNNN")` to
verify the link — but the L2S symlink points into the app cache dir (a different filesystem),
which is inaccessible to the stat call inside proot. groupadd exits with code 10 ("cannot lock").

**Root cause of the cross-filesystem issue:** L2S metadata is stored in `PROOT_L2S_DIR`. If
that directory is on a different filesystem from the rootfs (e.g. Android cache dir vs the
rootfs partition), hard-linking from L2S content back to the rootfs fails. This is pre-empted
by `install_tools()` pre-creating `.l2s/` inside the rootfs itself.

---

### Failure 3 — openssh-client postinst: chgrp '_ssh' invalid group

**Symptom:**
```
chgrp: invalid group: '_ssh'
dpkg: error processing package openssh-client (--configure): ...error exit status 1
```

**Cause:** The postinst calls `groupadd -g 101 _ssh` to create the group, then immediately
calls `chgrp _ssh /some/file`. With groupadd stubbed out (see fix below), the stub exits 0
but does not write to `/etc/group`, so `chgrp` cannot resolve the group name.

---

### Solution — two-pass apt install with groupadd stub

`install_tools()` in `src/pr-cli/src/cmd_test.rs` runs two apt-get passes for Debian:

**Pass 1** — install main tools normally:
```sh
apt-get install -y vim gcc rustc cargo git
```
None of these packages have postinst scripts that call groupadd.

**Pass 2** — prepare environment, then install openssh-client:
```sh
# 1. Replace groupadd/groupdel with no-op stubs.
#    Real groupadd cannot work on Android (link() blocked by SELinux).
printf '#!/bin/sh\nexit 0\n' > /usr/sbin/groupadd && chmod +x /usr/sbin/groupadd
printf '#!/bin/sh\nexit 0\n' > /usr/sbin/groupdel && chmod +x /usr/sbin/groupdel

# 2. Manually append _ssh and _sshd to /etc/group and /etc/gshadow.
#    This satisfies the chgrp call in the postinst without needing groupadd
#    to actually write the entries.
grep -q '^_ssh:'  /etc/group   || echo '_ssh:x:101:'  >> /etc/group
grep -q '^_sshd:' /etc/group   || echo '_sshd:x:102:' >> /etc/group
grep -q '^_ssh:'  /etc/gshadow || echo '_ssh:!::'     >> /etc/gshadow
grep -q '^_sshd:' /etc/gshadow || echo '_sshd:!::'    >> /etc/gshadow

# 3. Now openssh-client installs cleanly.
apt-get install -y openssh-client
```

The groupadd/groupdel stubs are left in place permanently. Real `groupadd` cannot work in
this environment (link() is always blocked), so the stubs are the correct long-term state.

### Key file

`src/pr-cli/src/cmd_test.rs` — `install_tools()`, `"apt"` branch.

---

---

## pr-cli Caveats

### `touch` silently failed inside proot (fixed)

`touch` and Python's `Path.touch()` call `utimensat` on the path first and create
the file only when it returns ENOENT. fake_id0's utimes EXIT handler forced every
error to 0 (for dpkg on L2S symlinks), so `touch /new/file` exited 0 without
creating the file. The handler now lets ENOENT through when the target does not
exist on the host (`docs/proot-improvement.md §28`); the integration test
"touch creates a missing file" covers it. Proot binaries built before this fix
still need `echo > /path/to/file`.

### Ubuntu 26.04 coreutils: `unknown program 'libproot-loader'` (fixed)

uutils coreutils 0.8 names its utility from `AT_EXECFN` as read through
`prctl(PR_GET_AUXV)` / `/proc/self/auxv`, which held the loader's path. Proot now
serves the loader-patched auxv from both (`docs/proot-improvement.md §30`). Older
proot builds break every coreutils command and `dpkg --configure` on Ubuntu 26.04.

### L2S links and the two app-data paths (fixed)

link2symlink stores absolute host paths. The app sees its data as `/data/user/0/<pkg>`;
`adb shell run-as` starts pr-cli without `APP_PREFIX`, so it used `/data/data/<pkg>` and bound
only that. proot resolves a symlink's absolute target as a guest path, so under run-as every
L2S link made by the app (gcc, cc, ...) failed `execve` with ENOENT while `stat` still worked.
`build_proot_args()` now also binds the other path of the app directory (`app_dir_alias()`).

### Container DNS follows the active network

pr-cli used to write `8.8.8.8` / `8.8.4.4` (install) or `8.8.8.8` / `1.1.1.1` (login) into
`/etc/resolv.conf`, and musl/glibc query those directly, so networks that drop direct public DNS
broke apk/apt inside containers while Android itself still resolved. The app now passes the
active network's DNS servers as `APP_DNS_SERVERS`; pr-cli writes up to two of them first, then
the public servers as fallback (three nameservers at most), and refreshes the file at each login.
A resolv.conf without pr-cli's marker line (and not an old pr-cli default) is left alone. IPv6
servers are passed only when the network has an IPv6 default route; otherwise (seen: IPv6-only
DNS on mobile data) glibc would wait out its timeout on them before every lookup. The file is
replaced atomically (temporary file + rename), and one that cannot be read as text is left alone.

### Login user: home only, always fake root and `/bin/sh`

**Current behaviour** (`src/pr-cli/src/login.rs`, `shared.rs`):

- `pr-cli login <distro> --user <name>` looks the user up in the container's `/etc/passwd`
  and fails if the user is missing, but only the **home directory** is used, as `HOME`.
  The session always starts in `/` (`--cwd=/` in `build_proot_args()`).
- The session always starts `/bin/sh -l`; the user's login shell from `/etc/passwd` is
  ignored (Ubuntu's `/bin/bash` for root, for example).
- PRoot always gets `--change-id=0:0`, so every user, `--user alice` included, runs as fake
  root: `id -u` is 0 and new files appear owned by root.

**History:** `2f2bdbc` started the passwd shell in the passwd home with
`--change-id=<uid>:<gid>`; `96c383d` (making login work under targetSdk 28 / W^X) replaced
that with `/bin/sh -l`, `--cwd=/` and `--change-id=0:0` without recording why. AGENTS.md
Critical Constraint 7 now requires `--change-id=0:0`: dpkg and other tools check
`getuid() == 0`.

**What could change, and what stands in the way:**

- **Start in the home directory:** pass the passwd home as `--cwd`. Low risk.
- **Login shell:** try the passwd shell and fall back to `/bin/sh -l`. The check has to run
  inside the session (for example `/bin/sh -l -c 'exec "$shell" -l || exec /bin/sh -l'`),
  not on the host: shells are often absolute symlinks (`/bin/sh -> /bin/busybox`) or L2S
  links that point outside the rootfs when followed from the host, and under W^X the mode
  bits do not say whether a file can be executed.
- **Non-root users:** not a matter of passing `--change-id=<uid>:<gid>`. It conflicts with
  Constraint 7, and fake_id0 is built around fake root:
  - every file owned by the real app uid, which is the whole rootfs, would appear owned by
    the user, and the kernel lets the app uid write all of it, so the user could edit
    `/etc/passwd`: there is no permission separation to gain;
  - `chown()` is answered with the caller's uid (`chown.c`), which non-root callers read as
    an error, and the utimes / permission-error masking applies only when `euid == 0`.
  It would need fake_id0 to keep its own per-file ownership, not just a flag.
- **Groups:** this build is not USERLAND, so `getgroups()` returns the app's real Android
  gids (inet 3003 and others), which are not in `/etc/group`; `id` shows them.

Each of these changes what existing scripts see (`id`, `$SHELL`, `$PWD`), so it needs its own
tests and a device run.

### Double shell wrapping via `pr-cli login`

`pr-cli login <distro> -- <args>` internally prepends `/bin/sh -l -c "<args.join(' ')\>"`
before calling proot. If callers explicitly pass `/bin/sh -c` as trailing args, the
command gets double-wrapped:

```
/bin/sh -l -c "/bin/sh -c command"
```

The inner quotes are stripped during `.join(' ')`, breaking the inner shell's
argument parsing.

**Rule:** Pass the raw command string directly as the trailing argument — never
wrap it in `/bin/sh -c` yourself:

```bash
# WRONG — double-wrapped, inner quotes stripped:
pr-cli login alpine -- /bin/sh -c "apk update && apk add rust"

# CORRECT — pr-cli adds the shell wrapper itself:
pr-cli login alpine -- "apk update && apk add rust"
```

---

## Project Name: pr

**pr** stands for **PRoot** — which is short for **ptrace-based root**.

PRoot is a user-space implementation of `chroot`, `mount --bind`, and `binfmt_misc` that uses Linux's `ptrace()` system call to intercept and translate filesystem paths. This allows running Linux distributions inside a directory without actual root privileges — exactly what this app does on Android.

The app package `id.or.oo.pr` inherits the name: a standalone Android APK that runs Linux distros via proot, with no Termux dependency.
