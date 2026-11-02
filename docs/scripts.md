# Build & Test Scripts

All scripts live in `scripts/` and should be run from the project root.

## `scripts/build.sh` — Build proot for Android

Cross-compiles proot using the Android NDK. Downloads NDK r27c automatically if not found.

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

Output: build/out/<arch>/proot
```

### Quick start

```bash
scripts/build.sh --arch=arm64 --skip-ndk --skip-talloc
```

### Build pipeline

1. Set up NDK standalone toolchain (API 28)
2. Build libtalloc as static library (from `vendor/samba/`)
3. Build proot with NDK clang (statically linked)
4. Fix TLS alignment (Android Bionic requires 64-byte)
5. Verify binary (ELF, static, correct arch)

## On-device tests

The on-device integration suite runs inside a container through `pr-cli test <distro>`, from the app (Test in a container's menu) or over adb; see [integration-tests.md](integration-tests.md).
