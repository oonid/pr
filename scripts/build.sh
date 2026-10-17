#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/build"
SRC_DIR="${PROJECT_ROOT}/src/proot"

NDK_VERSION="r27c"
NDK_URL_BASE="https://dl.google.com/android/repository"

TALLOC_SRC="${PROJECT_ROOT}/vendor/samba/lib/talloc"
TALLOC_STUB="${PROJECT_ROOT}/src/proot/lib/talloc"

API_LEVEL=28

ARCHS=("arm64" "arm")

declare -A NDK_ARCH TRIPLE
NDK_ARCH[arm64]="arm64"
NDK_ARCH[arm]="arm"
TRIPLE[arm64]="aarch64-linux-android"
TRIPLE[arm]="armv7a-linux-androideabi"

usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Build proot for Android using NDK cross-compilation."
    echo ""
    echo "Options:"
    echo "  --arch=ARCH      Target architecture: arm64, arm, or all (default: all)"
    echo "  --ndk-path=PATH  Path to existing NDK installation (skips download)"
    echo "  --skip-talloc    Skip libtalloc build (use existing)"
    echo "  --skip-ndk       Skip NDK setup (NDK already configured)"
    echo "  --clean          Clean build output"
    echo "  -v, --verbose    Verbose output"
    echo "  -h, --help       Show this help"
    echo ""
    echo "Environment variables:"
    echo "  NDK_PATH         Path to Android NDK (overrides --ndk-path)"
    echo "  PROOT_NDK_DIR    Directory for NDK download (default: build/ndk/)"
    echo ""
    echo "Output: build/out/<arch>/proot"
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

info() {
    echo "==> $*"
}

step() {
    echo ""
    echo "==== $* ===="
    echo ""
}

check_deps() {
    local missing=()
    for cmd in make python3 readelf file curl; do
        if ! command -v "$cmd" &>/dev/null; then
            missing+=("$cmd")
        fi
    done
    if [ ${#missing[@]} -gt 0 ]; then
        die "Missing required tools: ${missing[*]}"
    fi
}

setup_ndk() {
    local ndk_path="${1:-}"

    if [ -n "$ndk_path" ] && [ -d "$ndk_path" ]; then
        NDK="$ndk_path"
        info "Using existing NDK at: $NDK"
        return
    fi

    local ndk_dir="${PROOT_NDK_DIR:-${BUILD_DIR}/ndk}"
    local ndk_install="${ndk_dir}/android-ndk-${NDK_VERSION}"
    local ndk_zip="android-ndk-${NDK_VERSION}-linux.zip"

    if [ -d "$ndk_install" ]; then
        NDK="$ndk_install"
        info "Using cached NDK at: $NDK"
        return
    fi

    step "Downloading Android NDK ${NDK_VERSION}"

    mkdir -p "$ndk_dir"

    if [ ! -f "${ndk_dir}/${ndk_zip}" ]; then
        info "Downloading ${ndk_zip} ..."
        curl -L -o "${ndk_dir}/${ndk_zip}" \
            "${NDK_URL_BASE}/${ndk_zip}" \
            || die "Failed to download NDK"
    fi

    info "Extracting NDK ..."
    unzip -q "${ndk_dir}/${ndk_zip}" -d "$ndk_dir" \
        || die "Failed to extract NDK"

    NDK="$ndk_install"
    info "NDK installed at: $NDK"
}

get_ndk_bin_dir() {
    if [ -z "${NDK:-}" ]; then
        NDK="${BUILD_DIR}/ndk/android-ndk-${NDK_VERSION}"
    fi
    echo "${NDK}/toolchains/llvm/prebuilt/linux-x86_64/bin"
}

get_sysroot() {
    local arch="$1"
    echo "${BUILD_DIR}/sysroot/${arch}"
}

setup_sysroot() {
    local arch="$1"
    local triple="${TRIPLE[$arch]}"
    local sysroot
    sysroot="$(get_sysroot "$arch")"

    if [ -d "${sysroot}/usr/lib" ]; then
        info "Sysroot for ${arch} already exists"
        return
    fi

    step "Setting up sysroot for ${arch}"

    local ndk_sysroot="${NDK}/toolchains/llvm/prebuilt/linux-x86_64/sysroot"

    mkdir -p "${sysroot}/usr/lib" "${sysroot}/usr/include"

    cp -a "${ndk_sysroot}/usr/include" "${sysroot}/usr/"
    cp -a "${ndk_sysroot}/usr/lib/${triple}" "${sysroot}/usr/lib/"

    info "Sysroot created at: ${sysroot}"
}

build_talloc() {
    local arch="$1"
    local triple="${TRIPLE[$arch]}"
    local sysroot
    sysroot="$(get_sysroot "$arch")"
    local talloc_lib="${sysroot}/usr/lib/libtalloc.a"
    local talloc_inc="${sysroot}/usr/include/talloc.h"

    if [ -f "$talloc_lib" ] && [ -f "$talloc_inc" ] \
        && [ "$talloc_lib" -nt "${TALLOC_STUB}/replace.h" ] \
        && [ "$talloc_lib" -nt "${TALLOC_SRC}/talloc.c" ] \
        && [ "$talloc_lib" -nt "${TALLOC_SRC}/talloc.h" ]; then
        info "libtalloc already built for ${arch}"
        return
    fi

    step "Building libtalloc for ${arch}"

    if [ ! -f "${TALLOC_SRC}/talloc.c" ]; then
        die "talloc source not found at: ${TALLOC_SRC}\nEnsure vendor/samba submodule is initialized."
    fi

    local ndk_bin
    ndk_bin="$(get_ndk_bin_dir)"

    local cc="${ndk_bin}/${triple}${API_LEVEL}-clang"
    local ar="${ndk_bin}/llvm-ar"
    local ranlib="${ndk_bin}/llvm-ranlib"

    [ -x "$cc" ] || die "CC not found: $cc"
    [ -x "$ar" ] || die "AR not found: $ar"

    local build_dir="${BUILD_DIR}/talloc-build/${arch}"
    rm -rf "$build_dir"
    mkdir -p "$build_dir"

    info "Compiling talloc.c for ${triple} ..."
    "${cc}" \
        --sysroot="${sysroot}" \
        -I"${TALLOC_STUB}" \
        -I"${TALLOC_SRC}" \
        -I"${sysroot}/usr/include" \
        -DNO_CONFIG_H=1 \
        -D__STDC_WANT_LIB_EXT1__=1 \
        -O2 -Wall \
        -c "${TALLOC_SRC}/talloc.c" \
        -o "${build_dir}/talloc.o" \
        || die "talloc compile failed for ${arch}"

    info "Creating static library libtalloc.a ..."
    "${ar}" rcs "${talloc_lib}" "${build_dir}/talloc.o" \
        || die "talloc ar failed for ${arch}"
    "${ranlib}" "${talloc_lib}" \
        || die "talloc ranlib failed for ${arch}"

    cp "${TALLOC_SRC}/talloc.h" "${talloc_inc}"

    info "libtalloc built: $(du -h "${talloc_lib}" | cut -f1)"
}

build_proot() {
    local arch="$1"
    local triple="${TRIPLE[$arch]}"
    local sysroot
    sysroot="$(get_sysroot "$arch")"
    local out_dir="${BUILD_DIR}/out/${arch}"

    step "Building proot for ${arch}"

    mkdir -p "$out_dir"

    local ndk_bin
    ndk_bin="$(get_ndk_bin_dir)"

    local cc="${ndk_bin}/${triple}${API_LEVEL}-clang"
    local strip="${ndk_bin}/llvm-strip"
    local objcopy="${ndk_bin}/llvm-objcopy"
    local objdump="${ndk_bin}/llvm-objdump"

    [ -x "$cc" ] || die "CC not found: $cc"
    [ -x "$strip" ] || die "STRIP not found: $strip"
    [ -x "$objcopy" ] || die "OBJCOPY not found: $objcopy"
    [ -x "$objdump" ] || die "OBJDUMP not found: $objdump"

    local cflags="--sysroot=${sysroot} -I${sysroot}/usr/include"
    local ldflags="--sysroot=${sysroot} -L${sysroot}/usr/lib -ltalloc -static -Wl,-z,noexecstack,-z,max-page-size=16384"

    info "Building with CC=${cc##*/} ..."

    make -C "${SRC_DIR}/src" \
        clean 2>/dev/null || true

    make -C "${SRC_DIR}/src" \
        CC="$cc" \
        STRIP="$strip" \
        OBJCOPY="$objcopy" \
        OBJDUMP="$objdump" \
        CFLAGS="-Wall -Wextra -O2 ${cflags}" \
        LDFLAGS="$ldflags" \
        GIT=true \
        proot \
        || die "proot build failed for ${arch}"

    if [ ! -f "${SRC_DIR}/src/proot" ]; then
        die "proot binary not found after build"
    fi

    cp "${SRC_DIR}/src/proot" "${out_dir}/proot"
    chmod 755 "${out_dir}/proot"

    if [ -f "${SRC_DIR}/src/loader/loader" ]; then
        cp "${SRC_DIR}/src/loader/loader" "${out_dir}/loader"
        chmod 755 "${out_dir}/loader"
    fi

    info "Built: ${out_dir}/proot ($(du -h "${out_dir}/proot" | cut -f1))"
}

fix_tls_alignment() {
    local arch="$1"
    local binary="${BUILD_DIR}/out/${arch}/proot"

    local tls_align_hex
    tls_align_hex=$(readelf -W -l "$binary" 2>/dev/null \
        | awk '/^  TLS/{print $NF}' \
        | sed 's/0x//')

    if [ -z "$tls_align_hex" ]; then
        info "No TLS segment found (skipping alignment fix)"
        return
    fi

    local tls_align=$((16#$tls_align_hex))

    if [ "$tls_align" -ge 64 ]; then
        info "TLS alignment already ${tls_align}-byte (OK)"
        return
    fi

    info "Fixing TLS alignment: ${tls_align} -> 64 (Android Bionic requirement)"

    python3 -c "
import struct, sys

with open('${binary}', 'rb') as f:
    data = bytearray(f.read())

e_phoff = struct.unpack_from('<Q', data, 32)[0]
e_phentsize = struct.unpack_from('<H', data, 54)[0]
e_phnum = struct.unpack_from('<H', data, 56)[0]

PT_TLS = 7
for i in range(e_phnum):
    off = e_phoff + i * e_phentsize
    p_type = struct.unpack_from('<I', data, off)[0]
    if p_type == PT_TLS:
        struct.pack_into('<Q', data, off + 48, 64)
        break

with open('${binary}', 'wb') as f:
    f.write(data)
" || die "Failed to fix TLS alignment"
}

verify_binary() {
    local arch="$1"
    local out_dir="${BUILD_DIR}/out/${arch}"
    local binary="${out_dir}/proot"

    step "Verifying proot binary for ${arch}"

    if [ ! -f "$binary" ]; then
        die "Binary not found: $binary"
    fi

    info "file(1) output:"
    file "$binary"

    info "ELF header:"
    readelf -h "$binary" | grep -E "Class|Machine|Type"

    info "TLS alignment:"
    readelf -l "$binary" 2>/dev/null | awk '/^  TLS/{print $0}'

    info "Dynamic section (should be empty for static):"
    if readelf -d "$binary" 2>/dev/null | grep -q "NEEDED"; then
        echo "WARNING: Binary has dynamic dependencies!"
        readelf -d "$binary" | grep NEEDED
    else
        echo "OK: No dynamic dependencies (statically linked)"
    fi

    info "Size: $(du -h "$binary" | cut -f1)"
}

do_clean() {
    step "Cleaning build output"
    rm -rf "${BUILD_DIR}/out" "${BUILD_DIR}/talloc-build"
    make -C "${SRC_DIR}/src" clean 2>/dev/null || true
    info "Cleaned."
}

main() {
    local target_arch="all"
    local ndk_path="${NDK_PATH:-}"
    local skip_talloc=false
    local skip_ndk=false
    local clean=false
    local verbose=false

    for arg in "$@"; do
        case "$arg" in
            --arch=*)
                target_arch="${arg#--arch=}"
                ;;
            --ndk-path=*)
                ndk_path="${arg#--ndk-path=}"
                ;;
            --skip-talloc)
                skip_talloc=true
                ;;
            --skip-ndk)
                skip_ndk=true
                ;;
            --clean)
                clean=true
                ;;
            -v|--verbose)
                verbose=true
                export V=1
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                die "Unknown option: $arg\n$(usage)"
                ;;
        esac
    done

    if $clean; then
        do_clean
        exit 0
    fi

    check_deps

    if [ "$target_arch" = "all" ]; then
        TARGET_ARCHS=("${ARCHS[@]}")
    else
        TARGET_ARCHS=("$target_arch")
    fi

    echo "========================================="
    echo "  proot Android Build"
    echo "========================================="
    echo "  Target:  ${TARGET_ARCHS[*]}"
    echo "  API:     ${API_LEVEL}"
    echo "  Source:  ${SRC_DIR}"
    echo "  Output:  ${BUILD_DIR}/out/"
    echo "========================================="

    if ! $skip_ndk; then
        setup_ndk "$ndk_path"
    fi

    for arch in "${TARGET_ARCHS[@]}"; do
        if ! $skip_ndk; then
            setup_sysroot "$arch"
        fi

        if ! $skip_talloc; then
            build_talloc "$arch"
        fi

        build_proot "$arch"
        fix_tls_alignment "$arch"
        verify_binary "$arch"
    done

    step "Build complete"
    for arch in "${TARGET_ARCHS[@]}"; do
        local binary="${BUILD_DIR}/out/${arch}/proot"
        if [ -f "$binary" ]; then
            echo "  ${arch}: ${binary} ($(du -h "$binary" | cut -f1))"
        fi
    done
}

main "$@"
