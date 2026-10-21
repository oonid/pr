#!/system/bin/sh
# bootstrap.sh — First-run setup for standalone pr on Android
#
# This script MUST be POSIX sh compatible (#!/system/bin/sh).
# Called by APK App.kt on first launch.
#
# Environment variables (set by caller):
#   APP_PREFIX  - Base directory (default: /data/data/id.or.oo.pr/files/usr)
#   APP_HOME    - Home directory (default: /data/data/id.or.oo.pr/files/home)
#   APP_PACKAGE - Android package (default: id.or.oo.pr)

set -e

APP_PREFIX="${APP_PREFIX:-/data/data/id.or.oo.pr/files/usr}"
APP_HOME="${APP_HOME:-/data/data/id.or.oo.pr/files/home}"
APP_PACKAGE="${APP_PACKAGE:-id.or.oo.pr}"

BIN_DIR="${APP_PREFIX}/bin"

log() {
    echo "[bootstrap] $*"
}

create_directories() {
    log "Creating directory structure..."
    mkdir -p "${APP_PREFIX}/etc/pr"
    mkdir -p "${APP_PREFIX}/var/lib/pr/installed-rootfs"
    mkdir -p "${APP_PREFIX}/var/lib/pr/dlcache"
    mkdir -p "${APP_HOME}"
    mkdir -p "${APP_PREFIX}/tmp"
}

main() {
    log "Starting bootstrap for ${APP_PACKAGE}"
    create_directories
    log "Bootstrap complete"
}

main "$@"
