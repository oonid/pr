# Build & Test Scripts

All scripts live in `scripts/` and should be run from the project root.

## `scripts/build.sh` — Build proot for Android

Cross-compiles proot using the Android NDK. Downloads NDK r27c automatically if not found: `curl -fL` into a `.partial` file, a SHA-256 check against the pinned `NDK_SHA256` (whose zip matches the SHA-1 in Google's SDK repository manifest), then extraction into a temporary directory that is moved into place only when complete.

```
scripts/build.sh [OPTIONS]

Options:
  --arch=ARCH      Target: arm64, arm, or all (default: all)
  --ndk-path=PATH  Path to existing NDK (skips download)
  --skip-talloc    Skip libtalloc build (use existing)
  --skip-ndk       Skip NDK setup (already configured)
  --clean          Clean build output
  -v, --verbose    Verbose output
  -h, --help       Show help

Environment:
  NDK_PATH         Path to Android NDK (overrides --ndk-path)
  PROOT_NDK_DIR    Directory for NDK download (default: build/ndk/)

Output: build/out/<arch>/proot and build/out/<arch>/loader
```

Copy them to `android/proot-engine/src/main/jniLibs/<abi>/libproot.so` and `libproot-loader.so`
by hand; the script does not.

### Quick start

```bash
scripts/build.sh --arch=arm64 --skip-ndk --skip-talloc
```

### Build pipeline

1. Set up the sysroot from the NDK (API 28). `build/sysroot/<arch>/.toolchain` records the NDK path and clang version; a different NDK or clang recreates the sysroot
2. Build libtalloc as a static library (from `vendor/samba/`). `libtalloc.a.id` records the toolchain and flags, so a new compiler or flag rebuilds it, not only a newer source file
3. Build proot with NDK clang (statically linked), with `-ffile-prefix-map` so no build-machine path ends up in the binaries, then strip proot and the loader
4. Fix TLS alignment (Android Bionic requires 64-byte), for 64-bit ELF only
5. Verify binary (ELF, static, correct arch)

## `scripts/build-pr-cli.sh` — Build pr-cli for Android

Builds the guest test binary (`src/proot-integration-test`) and then pr-cli (`src/pr-cli`), which
embeds it, for `aarch64-linux-android` in release mode. It passes
`--remap-path-prefix` for `$CARGO_HOME` (to `/cargo`) and the Rust sysroot (to `/rust`) through
`cargo --config`, which adds to the rustflags in `.cargo/config.toml` instead of replacing them, so
panic locations name `/cargo/registry/...` rather than the build machine's home directory. It copies
pr-cli to `android/proot-engine/src/main/jniLibs/arm64-v8a/libpr-cli.so`, checks the copy matches,
and fails if `$CARGO_HOME`, the Rust sysroot or the project path is still in the binary. CI uses it.

Both crates link with the NDK r27c that `build.sh` downloads and checks into `build/ndk/android-ndk-r27c`: their `.cargo/config.toml` names it by a path relative to the crate, so run `build.sh` once first. CI links that directory to the r27c it installs with `sdkmanager`.

## On-device tests

The on-device integration suite runs inside a container through `pr-cli test <distro>`, from the app (Test in a container's menu) or over adb; see [integration-tests.md](integration-tests.md).
