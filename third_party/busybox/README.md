# busybox source for libbusybox.so

`android/proot-engine/src/main/jniLibs/arm64-v8a/libbusybox.so` is the
`busybox.static` binary from Alpine Linux's `busybox-static` package,
unchanged. busybox is licensed GPL-2.0-only; this directory is the
corresponding source for that binary.

| | |
|---|---|
| Package | `busybox-static` 1.37.0-r14, Alpine Linux v3.21, aarch64 |
| Package file | `busybox-static-1.37.0-r14.apk`, SHA-256 `6fd7ea97062beb51fa785ba858f823e1dfe4daf6bfa91ff4d5359b1061988c69` |
| Binary | `bin/busybox.static` in that package = `libbusybox.so`, SHA-256 `e383c8bc25a1137b8ee88718cc6df1f1e84c54521d6045fc837385995dcdf031` |
| Built from | aports commit `284d827a2793963e6dcf88324bd1eb81adff579c`, `main/busybox` (named in the package's `.PKGINFO`) |

## Contents

- `busybox-1.37.0.tar.bz2`: the upstream release from
  https://busybox.net/downloads/, matching the SHA-512 in `alpine/APKBUILD`.
- `alpine/`: `main/busybox` of aports at that commit, unchanged: the
  `APKBUILD` build recipe, the patches it applies, and the `busyboxconfig`
  configuration files.

## Rebuilding

On Alpine Linux v3.21 for aarch64 with `alpine-sdk`, put
`busybox-1.37.0.tar.bz2` next to `alpine/APKBUILD` (or let `abuild` fetch
it), then run `abuild -r` in `alpine/`. The `busybox-static` package it
produces contains `bin/busybox.static`.

`scripts/download-busybox.sh` downloads the prebuilt package and checks
both SHA-256 sums above. When it moves to another release, replace this
directory with that release's source.
