#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${PROJECT_ROOT}/build"

# Alpine busybox-static package details
# Source: https://dl-cdn.alpinelinux.org/alpine/v3.21/main/aarch64/
# GPL corresponding source: third_party/busybox/ (replace it when the version changes)
BB_VERSION="1.37.0"
BB_RELEASE="r14"
BB_ALPINE="v3.21"
BB_ARCH="aarch64"

BB_APK_NAME="busybox-static-${BB_VERSION}-${BB_RELEASE}.apk"
BB_APK_URL="https://dl-cdn.alpinelinux.org/alpine/${BB_ALPINE}/main/${BB_ARCH}/${BB_APK_NAME}"
BB_APK_SHA256="6fd7ea97062beb51fa785ba858f823e1dfe4daf6bfa91ff4d5359b1061988c69"

BB_BINARY_SHA256="e383c8bc25a1137b8ee88718cc6df1f1e84c54521d6045fc837385995dcdf031"

# Output paths
DL_DIR="${BUILD_DIR}/dl"
OUT_DIR="${BUILD_DIR}/assets/arm64-v8a"
OUT_FILE="${OUT_DIR}/busybox"

usage() {
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Download and prepare static busybox binary from Alpine Linux."
    echo ""
    echo "Options:"
    echo "  --arch=ARCH       Target architecture (default: aarch64)"
    echo "  -f, --force       Re-download even if binary exists"
    echo "  --verify-only     Only verify existing binary, skip download"
    echo "  -h, --help        Show this help"
    echo ""
    echo "Output: ${OUT_FILE}"
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

info() {
    echo "==> $*"
}

verify_binary_file() {
    local binary="$1"

    info "Verifying: $binary"

    [ -f "$binary" ] || die "Binary not found: $binary"

    info "file(1) output:"
    file "$binary"

    local actual_class
    actual_class=$(readelf -h "$binary" 2>/dev/null | grep "Class:" | awk '{print $NF}')
    local actual_machine
    actual_machine=$(readelf -h "$binary" 2>/dev/null | grep "Machine:" | head -1)

    if [ "$actual_class" != "ELF64" ]; then
        die "Expected ELF64, got: $actual_class"
    fi

    if echo "$actual_machine" | grep -qi "aarch"; then
        :
    else
        die "Expected AArch64, got: $actual_machine"
    fi

    if readelf -d "$binary" 2>/dev/null | grep -q "NEEDED"; then
        die "Binary has dynamic dependencies (not static!)"
    fi
    info "OK: Static binary confirmed"

    local tls_align_hex
    tls_align_hex=$(readelf -W -l "$binary" 2>/dev/null \
        | awk '/^  TLS/{print $NF}' \
        | sed 's/0x//')

    if [ -n "$tls_align_hex" ]; then
        local tls_align=$((16#$tls_align_hex))
        if [ "$tls_align" -lt 64 ]; then
            echo "WARNING: TLS alignment is ${tls_align} bytes (need >= 64 for Android Bionic)"
            echo "  Will need post-build fix"
        else
            info "TLS alignment: ${tls_align} bytes (OK)"
        fi
    else
        info "No PT_TLS segment (OK for static binary)"
    fi

    info "Size: $(du -h "$binary" | cut -f1)"
}

verify_sha256() {
    local file="$1"
    local expected="$2"
    local actual
    actual=$(sha256sum "$file" | awk '{print $1}')
    if [ "$actual" != "$expected" ]; then
        die "SHA256 mismatch for $(basename "$file")\n  Expected: $expected\n  Actual:   $actual"
    fi
    info "SHA256 OK: $(basename "$file")"
}

main() {
    local force=false
    local verify_only=false

    for arg in "$@"; do
        case "$arg" in
            -f|--force)
                force=true
                ;;
            --verify-only)
                verify_only=true
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                die "Unknown option: $arg"
                ;;
        esac
    done

    if $verify_only; then
        if [ -f "$OUT_FILE" ]; then
            verify_sha256 "$OUT_FILE" "$BB_BINARY_SHA256"
            verify_binary_file "$OUT_FILE"
            info "Verification complete"
        else
            die "Binary not found: $OUT_FILE"
        fi
        exit 0
    fi

    if [ -f "$OUT_FILE" ] && ! $force; then
        info "Binary already exists: $OUT_FILE"
        verify_sha256 "$OUT_FILE" "$BB_BINARY_SHA256"
        info "Use --force to re-download"
        exit 0
    fi

    mkdir -p "$DL_DIR" "$OUT_DIR"

    local apk_path="${DL_DIR}/${BB_APK_NAME}"

    if [ -f "$apk_path" ] && ! $force; then
        info "Using cached APK: $apk_path"
    else
        info "Downloading Alpine busybox-static ${BB_VERSION}-${BB_RELEASE} ..."
        info "URL: ${BB_APK_URL}"
        curl -fSL -o "$apk_path" "$BB_APK_URL" || die "Download failed"
    fi

    verify_sha256 "$apk_path" "$BB_APK_SHA256"

    info "Extracting busybox from APK ..."
    local tmpdir
    tmpdir=$(mktemp -d)
    tar xzf "$apk_path" -C "$tmpdir" || die "Extraction failed"

    local extracted="${tmpdir}/bin/busybox.static"
    [ -f "$extracted" ] || die "Expected bin/busybox.static not found in APK"

    verify_sha256 "$extracted" "$BB_BINARY_SHA256"

    cp "$extracted" "$OUT_FILE"
    chmod 755 "$OUT_FILE"
    rm -rf "$tmpdir"

    verify_binary_file "$OUT_FILE"

    echo ""
    info "Static busybox ready: ${OUT_FILE}"
    info "  Version:  BusyBox v${BB_VERSION}"
    info "  Source:   Alpine ${BB_ALPINE} ${BB_ARCH} (${BB_APK_NAME})"
    info "  License:  GPL-2.0-only"
}

main "$@"
