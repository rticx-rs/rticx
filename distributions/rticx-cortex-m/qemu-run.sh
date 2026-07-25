#!/usr/bin/env bash
# Build and run a rticx-cortex-m example under QEMU's `lm3s6965evb` machine.
#
# Usage:
#   ./qemu-run.sh armv7m   # thumbv7m, BASEPRI locking  (default)
#   ./qemu-run.sh armv6m   # thumbv6m, interrupt source masking
#
# Requires: `qemu-system-arm` on PATH, and the `thumbv7m-none-eabi` /
# `thumbv6m-none-eabi` Rust targets installed.
set -euo pipefail

APP="${1:-armv7m}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$APP" in
    armv7m) APP_DIR="$SCRIPT_DIR/example-apps/armv7m-app" ;;
    armv6m) APP_DIR="$SCRIPT_DIR/example-apps/armv6m-app" ;;
    *) echo "usage: $0 {armv7m|armv6m}" >&2; exit 2 ;;
esac

if ! command -v qemu-system-arm >/dev/null 2>&1; then
    echo "error: qemu-system-arm not found on PATH." >&2
    echo "       install it with e.g. 'sudo apt-get install -y qemu-system-arm'." >&2
    exit 2
fi

echo ">>> building and running $APP example under QEMU"
# Run from the app dir so its `.cargo/config.toml` (target triple + QEMU runner)
# is discovered by Cargo — `--manifest-path` alone would ignore it and fall back
# to the host target.
cd ${APP_DIR}
cargo build --example hello_rtic
timeout --foreground 30s bash -c 'cargo run --example hello_rtic' 
cd -