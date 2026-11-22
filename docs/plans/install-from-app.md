# Installs from the app's image field: Implementation Plan

> Execute this plan task by task, with a review after each. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an image reference typed in the app installs correctly or fails before anything is written: an image for another architecture is refused before any layer is downloaded, nothing in an image can point the container's hard-link store or bind sources outside the container, and layers are applied as the OCI image spec says.

**Architecture:** all in pr-cli (`src/pr-cli/src`). `install.rs` fetches and checks the image config after resolving the manifest. `oci.rs` (layer application, whiteouts, `link_inside_root`), `install.rs` (`unpack_entry` for tarballs) and `shared.rs`/`login.rs` (the store, the bind sources, files read at login) refuse or skip paths that an image turned into symlinks. Tests are host unit tests: small tar layers built in the test, and registry steps served by `test_http.rs`.

**Tech Stack:** Rust (pr-cli; `tar` 0.4.45, `flate2`, `reqwest`, `serde_json`), host `cargo test`; one APK for the phone check.

**Spec:** `docs/audit-2026-11-16.md` (local): R21, R22 (with the bind-source part of L10), R23, R35, R36; details in `docs/audit-2026-11-16-details/findings-install.md`, `findings-commands.md` and `verify-prcli.md`.

## Findings this plan relies on

- `resolve_oci_manifest` (`install.rs:410-480`) checks the architecture only when the tag is an index (`select_manifest_descriptor`); a plain manifest is used as is. The config descriptor is required but its blob is never fetched, and `populate_oci_container` records the device architecture as the selected one. An amd64-only image installs, prints "Finished", and fails at login.
- `apply_layer_tar_stream` (`oci.rs:596-683`) unpacks with tar's `unpack_in`: a symlink entry named `.l2s` at the root is created (only its parent is checked), and `.wh..l2s` removes the pre-created store directory, because it was not created by this layer. A later hard-link entry then goes through `link_inside_root` (`oci.rs:685-750`), whose `move_into_l2s` renames data through `<rootfs>/.l2s` wherever it points. The tarball path (`install.rs` `unpack_entry`) replaces the directory with the symlink without needing a whiteout.
- At login `build_proot_args` (`shared.rs:440-546`) sets `PROOT_L2S_DIR` after `Path::is_dir()` (follows symlinks), binds `<rootfs>/tmp` at `/dev/shm`, `<rootfs>/proc/.*` at `/proc/*` and `<rootfs>/sys/.empty` at `/sys/fs/selinux` by host path, so proot follows a symlink there. `login.rs` reads `etc/passwd` and `etc/environment` by plain host path.
- tar's `unpack` replaces an existing path only with `remove_file`: a later layer that turns a directory into a symlink or file, or a file or symlink into a directory, fails the whole install (`unpack_dir` refuses a non-directory). The OCI image spec ("Changeset over existing files") removes the old path unless both are directories.
- `apply_whiteout_if_needed` (`oci.rs:955-997`) uses `trim_start_matches(".wh.")` (strips repeatedly; aufs metadata names `.wh..wh.plnk`, `.wh..wh.aufs`, `.wh..wh.orph` become whiteouts of `plnk`, `aufs`, `orph`) and does not reject `.` or `..` (`a/.wh..` removes `a` itself, including entries this layer created; `.wh...` at the root removes `<rootfs>/..`, emptying `containers/<name>`).
- `apply_layer_blob` (`oci.rs:571-594`) uses `flate2::read::GzDecoder`, which stops at the end of the first gzip member; concatenated or eStargz layers are cut short, sometimes without an error.
- `test_http.rs` serves a scripted sequence of responses; existing install tests (`install.rs` tests around line 2100) already serve manifests through it.

## Global Constraints

- AGENTS.md: no comments; commits `fix|test|docs|chore: …`, subject and body ≤ 72 columns, no trailers; no device identifiers.
- Red first: every behaviour change has a host test that fails before it.
- Messages a user sees name the reference and say what to do (for example "image <ref> is built for linux/amd64; this device needs linux/arm64").
- Builds: host `cargo test`/clippy freely; `scripts/build-pr-cli.sh` and the APK only once, in the last task, after asking.

## Review Focus

1. Catalog images (Alpine, Debian, Ubuntu, Fedora, openSUSE, Rocky) still install: their index entries and configs pass the new check.
2. An image whose config has no `architecture` is installed with a warning, not refused (it cannot be judged).
3. Nothing outside `<rootfs>` is written or bound, whatever symlinks a layer, a tarball or an existing container contains at `.l2s`, `tmp`, `proc/.*`, `sys/.empty`, `etc`.
4. Ordinary images with whiteouts and opaque directories still apply as before.

---

### Task 1: Refuse an image built for another architecture before downloading layers

**Files:**
- Modify: `src/pr-cli/src/install.rs` (`resolve_oci_manifest`, `populate_oci_container`, a small config type)
- Test: `src/pr-cli/src/install.rs` tests

**Interfaces:**
- After the manifest is chosen, fetch `/v2/<repository>/blobs/<config.digest>` with the selected bearer token, refusing a body larger than 1 MiB or one whose digest does not match; parse `os`, `architecture`, `variant`.
- `os` must be `linux` and `architecture` the device's (`arm64` for aarch64), else `Err("image <ref> is built for <os>/<architecture>; this device needs linux/<arch>")` before any layer request. A missing `architecture` prints a warning and continues.
- The recorded `selected_architecture` is the config's when present.

- [ ] **Step 1: Write the failing tests** (served by `test_http`):
  - `a_single_manifest_for_another_architecture_is_refused_before_layers`: plain manifest; config `{"os":"linux","architecture":"amd64"}` → error naming `linux/amd64`, and no layer URL was requested.
  - `a_single_manifest_for_this_architecture_installs`: config `arm64` → layers requested.
  - `a_config_that_does_not_match_its_digest_is_refused`.
  - `a_config_without_an_architecture_is_accepted_with_a_warning`.
- [ ] **Step 2:** `cargo test` — expect the new tests to fail.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` — all pass.
- [ ] **Step 5: Commit** `fix: refuse an image built for another architecture`.

---

### Task 2: Nothing an image ships can point the store or the binds outside the container

**Files:**
- Modify: `src/pr-cli/src/oci.rs` (`apply_layer_tar_stream`, `apply_whiteout_if_needed`, `link_inside_root`), `src/pr-cli/src/install.rs` (`unpack_entry`), `src/pr-cli/src/shared.rs` (`build_proot_args`), `src/pr-cli/src/login.rs` (`etc/passwd`, `etc/environment` reads)
- Test: the same files' test modules

**Interfaces:**
- Layers and tarballs: an entry or whiteout whose first path component is `.l2s` is skipped with one warning per install ("ignoring <n> entries at /.l2s in the image").
- `link_inside_root`: uses the store only when `symlink_metadata(<rootfs>/.l2s)` is a real directory; otherwise falls back to the copy path.
- `build_proot_args`: `PROOT_L2S_DIR` is set only when `<rootfs>/.l2s` is a real directory (lstat); each bind source under the rootfs (`tmp`, `proc/.*`, `sys/.empty`) is bound only when lstat shows the expected type (directory for `tmp`, regular file for the others); otherwise it is skipped and a warning names it.
- `login.rs`: `etc/passwd` and `etc/environment` are read with `read_rootfs_file` (in-root resolution).

- [ ] **Step 1: Write the failing tests:**
  - `a_layer_cannot_replace_the_store_with_a_symlink`: layer with `.wh..l2s`, symlink `.l2s -> <tmp>/outside`, a file, a hard link to it → `<tmp>/outside` stays empty; the store is still a directory.
  - `a_tarball_cannot_replace_the_store_with_a_symlink` (the `extract_tarball` path).
  - `link_inside_root_copies_when_the_store_is_not_a_directory`.
  - `proot_args_skip_bind_sources_that_are_symlinks`: rootfs with `tmp -> <tmp>/outside` and `proc/.stat` a symlink → no `--bind=<rootfs>/tmp:/dev/shm`, no `.stat` bind; `PROOT_L2S_DIR` unset when `.l2s` is a symlink.
  - `login_reads_passwd_inside_the_rootfs`: `etc` a symlink to another directory with its own `passwd` → the rootfs's is used, or the read fails safely.
- [ ] **Step 2–4:** red, implement, green (`cargo test`, clippy, fmt).
- [ ] **Step 5: Commit** `fix: keep the hard-link store and binds inside the container`.

---

### Task 3: Apply layers as the image spec says

**Files:**
- Modify: `src/pr-cli/src/oci.rs` (`apply_layer_tar_stream`, `apply_whiteout_if_needed`, `apply_layer_blob`)
- Test: `src/pr-cli/src/oci.rs` tests

**Interfaces:**
- Before unpacking an entry, if the path exists (lstat) and its type differs from the entry's (a directory against anything that is not a directory, or the reverse), remove it first (`remove_dir_all` for a directory). Directory over directory still merges.
- Whiteouts: strip the `.wh.` prefix once; `.wh..wh..opq` is the opaque marker; any other `.wh..wh.*` is ignored; a target of `.` or `..` (or empty) is ignored with a warning.
- Gzip layers use `flate2::read::MultiGzDecoder`.

- [ ] **Step 1: Write the failing tests:**
  - `a_later_layer_can_turn_a_directory_into_a_symlink`, `…_a_file_into_a_directory`, `…_a_symlink_into_a_directory` (two layers applied in order; the second wins).
  - `whiteout_names_dot_and_dotdot_remove_nothing`: `keep/file` then `keep/.wh..` and `.wh...` at the root → `keep/file` and the rootfs survive.
  - `aufs_metadata_names_are_not_whiteouts`: `plnk` exists; layer has `.wh..wh.plnk` → `plnk` survives.
  - `a_multi_member_gzip_layer_is_applied_whole`: two gzip members, each a tar fragment; both files exist.
- [ ] **Step 2–4:** red, implement, green.
- [ ] **Step 5: Commit** `fix: apply layers as the image spec says`.

---

### Task 4: Docs, binary and the phone check

- [ ] **Step 1:** Docs: `docs/android-apk-architecture.md` (OCI install: the config check; the `.l2s` promise now true), `docs/important-notes.md` where installs are described; test counts in README.md and AGENTS.md. Commit `docs: installs from the app as they are now`.
- [ ] **Step 2:** (Ask first.) `scripts/build-pr-cli.sh`; commit `chore: update pr-cli with the install fixes` (toolchain in the body); APK; install on the phone.
- [ ] **Step 3:** (Ask first.) Phone, through the app (foreground checked before each tap): install `docker.io/library/alpine:latest` under a new alias and run Test; install an amd64-only image by reference (find one whose tag resolves to a plain manifest, e.g. under `docker.io/amd64/`; check with the registry first) and expect the refusal message before any download; remove the test container afterwards.

## Not in this PR

- Applying the image config's `Env` and `WorkingDir` (R21's second half): a behaviour change for logins, its own PR if wanted.
- The tarball strip count (R40) and registry client robustness (R41): CLI paths, later.
- R42 (one entry writer for layers, tarballs and restore): a refactor once these fixes have landed.
