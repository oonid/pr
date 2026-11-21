# link2symlink store: Implementation Plan

> Execute this plan task by task, with a review after each. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a fake hard link never overwrites, strands or loses another file's data: the store's numbering is bounded and claimed atomically, a failed link leaves everything as it was, the link count changes only when the call that removes a name succeeds, and hard-linking a symlink copies the symlink.

**Architecture:** all in `src/proot/src/extension/link2symlink/link2symlink.c`, plus the matching naming rule in pr-cli's install-time store (`src/pr-cli/src/oci.rs` `move_into_l2s`). `move_and_symlink_path` claims a free intermediate name with an exclusive `symlink()` before moving any data, moves the data with a no-replace rename, and undoes its steps on failure. The rename/unlink handlers record the target's chain at the enter stage and adjust the count at the exit stage only when the call returned 0. New guest tests pin each behaviour.

**Tech Stack:** C (proot), Rust (pr-cli, the guest test binary).

**Spec:** `docs/audit-2026-11-16.md` (local): P12, P18, R39 (install-time limits), and the final review of #63 (the count changes before the call's result is known); details in `docs/audit-2026-11-16-details/findings-proot.md` (X3, X9) and `verify-proot.md`.

## Findings this plan relies on

- The store (`PROOT_L2S_DIR` = `<rootfs>/.l2s`, set by pr-cli for every container) is flat. A hard link turns the source into `name → .l2s/.l2s.<basename>NNNN → .l2s/.l2s.<basename>NNNN.CCCC`, where NNNN numbers the groups sharing a basename and CCCC is the link count.
- `move_and_symlink_path` (`link2symlink.c:51-197`) looks for a free NNNN with `access(F_OK)` up to 999 and then uses the last candidate even when it exists; it renames the source over `<candidate>.0002` (replacing another group's data when that group has two links), then `symlink()` fails with EEXIST and the source is stranded in `.l2s`. Errors are returned as -1, not `-errno`. Two sessions can pick the same free NNNN (check, then rename).
- pr-cli's install-time `move_into_l2s` (`oci.rs`) uses the same names and range (1–999) in the same directory, checks with `symlink_metadata`, and does not check the data name; when no number is free it returns `None` and the caller falls back to a copy.
- For a source that is a real symlink (not an l2s chain), `intermediate` holds the symlink's target and the data is renamed to `<target>0001.0002`, relative to proot's own cwd (`link2symlink.c:83-98`): in the app (cwd `/`) the guest gets EPERM; with a writable cwd the guest's symlink is moved away.
- The extension lowers the count of the target of `rename`, `renameat`, `renameat2` and of `unlink`/`unlinkat` at `SYSCALL_ENTER_END` (`link2symlink.c:695-750`), before the call runs. If the call then fails (`renameat2` with `RENAME_NOREPLACE` onto an existing name gives EEXIST; `RENAME_EXCHANGE` removes nothing; a permission error), the count is one too low, and removing the remaining names later deletes the data while a name still points at it.
- proot runs inside the app process, under the app seccomp filter; `renameat2` is allowed there since Android 8, so proot can use `RENAME_NOREPLACE` itself (`syscall(SYS_renameat2, …)`; bionic's wrapper needs API 30).
- Guests can create files in `/.l2s` directly (it is inside the rootfs), so a guest test can plant entries to reach the limit without making thousands of links.

## Global Constraints

- AGENTS.md: no comments; `vendor/` read only; commits `fix|test|docs|chore: …`, subject ≤ 72 columns, body ≤ 72 columns, no trailers; no device identifiers.
- The naming rule is shared by C and Rust: groups NNNN from 0001 to 9999, data name `<intermediate>.CCCC` with CCCC from 0002 to 9999. Both must change together.
- Builds (proot, test binary, pr-cli, APK) and phone runs: ask first. SIGSYS-independent, so the `run-as` harness is fine for red/green; the final check is Test through the app.

## Review Focus

1. Ordinary hard links (dpkg, git, `cp -al`) still work and still look like hard links (`st_nlink`, same inode) through every name.
2. A failure at any step of linking leaves the source at its path with its content, and leaves no half-made entry in `.l2s`.
3. Two sessions linking files with the same basename at the same time never share a group.
4. `rename`/`unlink` of a hard-linked name: the count drops exactly when the name is gone; failed calls change nothing.
5. Existing containers (stores created by older builds, numbered up to 0999) keep working.

---

### Task 1: Claim the group name atomically, bound it, roll back on failure

**Files:**
- Modify: `src/proot/src/extension/link2symlink/link2symlink.c` (`move_and_symlink_path`)
- Modify: `src/pr-cli/src/oci.rs` (`move_into_l2s`)
- Test: `src/proot-integration-test/src/hardlink.rs` (new suite `hardlink`), `src/proot-integration-test/src/main.rs`, `src/pr-cli/src/oci.rs` tests

**Interfaces:**
- C, first link: for NNNN from 1 to 9999, try `symlink(<intermediate>NNNN.0002, <intermediate>NNNN)`; EEXIST → next; other error → return `-errno`. None free → `-EMLINK`. Then move the data with `renameat2(AT_FDCWD, original, AT_FDCWD, <final>, RENAME_NOREPLACE)`; on failure unlink the claimed intermediate and return `-errno`. Then `symlink(intermediate, original)`; on failure rename the data back to `original` and unlink the intermediate.
- C, later link (count increment): the existing rename of `.CCCC` to `.CCCC+1` uses `RENAME_NOREPLACE`; a count at 9999 → `-EMLINK`. Each failure undoes the earlier steps.
- Every error path returns `-errno` (never -1).
- Rust `move_into_l2s`: the same range (1..=9999) and the same claim-then-move order (`symlink` the intermediate first; `EEXIST` → next); a data name that already exists is treated as taken; none free → `Ok(None)` (the caller's copy fallback stays).

- [ ] **Step 1: Write the failing tests** (suite `hardlink`, probe always true; files under `/tmp/pit-hl-*` and planted entries under `/.l2s/.l2s.pithl*`, removed afterwards):
  - `a full basename is EMLINK and keeps the source`: plant `/.l2s/.l2s.pithlfull0001` … `9999` as symlinks to nowhere (fast), create `/tmp/pit-hl-full/pithlfull` with content "x", `link()` it to `/tmp/pit-hl-full/b`: expect -1/EMLINK, the source still a regular file reading "x".
  - `a stale data name is never overwritten`: plant `/.l2s/.l2s.pithlstale0001.0002` as a regular file reading "other" (no intermediate), hard-link a file named `pithlstale` reading "mine": the link succeeds, both names read "mine", and the planted file still reads "other".
  - `the 1000th group of a basename gets its own data`: plant intermediates `0001`…`0999` each pointing to its own data file reading its number, hard-link a new file `pithlk` reading "new": succeeds, both names read "new", and planted group 0999's data still reads "0999".
  - pr-cli unit tests: `move_into_l2s` skips a taken intermediate and a taken data name; returns `None` when 1..=9999 are taken (use a test helper that plants names, not 9999 real files if slow).
- [ ] **Step 2:** (Ask first.) Build the test binary; run `hardlink` on the phone through `run-as` with today's proot: expect the first three to fail. Run the pr-cli tests on the host: expect the new ones to fail.
- [ ] **Step 3:** Implement the interfaces above.
- [ ] **Step 4:** (Ask first.) Rebuild proot; rerun: all pass; `readlink`, `fakeroot`, `general` unchanged.
- [ ] **Step 5: Commit** `fix: claim a link2symlink group atomically and undo a failed link`.

---

### Task 2: Change the link count only when the name is really gone

**Files:**
- Modify: `src/proot/src/extension/link2symlink/link2symlink.c` (the `SYSCALL_ENTER_END` cases for rename/renameat/renameat2/unlink/unlinkat, `SYSCALL_EXIT_END`, `decrement_link_count`)
- Test: `src/proot-integration-test/src/hardlink.rs`

**Interfaces:**
- At the enter stage, for those syscalls, if the target name (the new path for renames, the path for unlinks) is an l2s chain, remember its intermediate path in the extension's per-tracee state (talloc'd on the extension, as other extensions keep state); do not touch the count.
- `renameat2` with `RENAME_EXCHANGE`: remember nothing (no name is removed).
- At the exit stage, if a remembered intermediate exists and the syscall's result is 0, lower that chain's count (deleting the intermediate and data at 0, as `decrement_link_count` does today); always clear the remembered state.
- `rename(a, b)` where `a` and `b` are the same chain's names: the kernel replaces `b` with `a`'s symlink; the group loses one name, so the count drops by one (unchanged rule).

- [ ] **Step 1: Write the failing tests:**
  - `a failed NOREPLACE rename keeps the link count`: `a` and `b` hard links of one file ("data"); `renameat2(c, b, RENAME_NOREPLACE)` with `c` existing → EEXIST; `unlink(a)`; `b` must still read "data" (today it is dangling).
  - `rename over a hard link drops one name`: `a`, `b` hard links; `rename(c, b)`; `a` still reads the data and `stat(a).st_nlink == 1`.
  - `unlink of the last name removes the data`: `a`, `b` hard links; unlink both; the group's entries are gone from `/.l2s`.
- [ ] **Step 2:** (Ask first.) Run on the phone with the Task 1 proot: expect the first to fail.
- [ ] **Step 3:** Implement the interfaces above.
- [ ] **Step 4:** (Ask first.) Rebuild proot; rerun: all pass.
- [ ] **Step 5: Commit** `fix: lower a link2symlink count only after the name is gone`.

---

### Task 3: Hard-linking a symlink copies the symlink

**Files:**
- Modify: `src/proot/src/extension/link2symlink/link2symlink.c` (`move_and_symlink_path`, the branch for a source that is a symlink but not an l2s chain)
- Test: `src/proot-integration-test/src/hardlink.rs`

**Interfaces:**
- When the source is a symlink whose target's last component does not start with the l2s prefix: read its target and create a new symlink with that target at the destination (translated path); void the syscall with result 0; nothing is renamed. Errors return `-errno`.

- [ ] **Step 1: Write the failing test:** `hard-linking a symlink makes a symlink with the same target`: `symlink("target-pit", a)`; `link(a, b)` (no `AT_SYMLINK_FOLLOW`): returns 0, `readlink(b) == "target-pit"`, `readlink(a) == "target-pit"`, and nothing new appears in `/.l2s` (today: EPERM in the app).
- [ ] **Step 2–4:** red, implement, green (ask before each build and run).
- [ ] **Step 5: Commit** `fix: hard-linking a symlink copies the symlink`.

---

### Task 4: Docs, binaries and the app check

- [ ] **Step 1:** Docs: the link2symlink sections in `docs/important-notes.md` and `docs/proot-improvement.md` (naming and limits, the claim order, counts changed at exit, symlink sources), `docs/integration-tests.md` and AGENTS.md (the `hardlink` suite, counts). Commit `docs: the link2symlink store as it is now`.
- [ ] **Step 2:** (Ask first.) `scripts/build.sh --arch=arm64` and copy proot and the loader; `scripts/build-pr-cli.sh`; APK; one `chore:` commit for the binaries with the toolchain in the body.
- [ ] **Step 3:** (Ask first.) Phone, through the app: Test on Alpine and Ubuntu (all suites, including `hardlink`); in an Ubuntu terminal, `apt-get install -y --reinstall coreutils` (dpkg hard links) completes, and `cp -al /usr/share/doc /tmp/pl && rm -rf /tmp/pl` leaves `/.l2s` with the same entries as before (compare `ls /.l2s | wc -l`).

## Not in this PR

- Old stores whose counts are already wrong (from the enter-stage decrement): not repaired; a `pr-cli` repair command could follow if needed.
- R22 (an image making `.l2s` a symlink) and R27 (`rename` of a container breaking absolute chains): PR 4 and PR 10.
