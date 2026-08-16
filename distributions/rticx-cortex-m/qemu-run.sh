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
APP_DIR="$SCRIPT_DIR/examples-apps"

case "$APP" in
    armv7m) TARGET_FLAGS="" ;;
    armv6m) TARGET_FLAGS="--target thumbv6m-none-eabi" ;;
    *) echo "usage: $0 {armv7m|armv6m}" >&2; exit 2 ;;
esac

if ! command -v qemu-system-arm >/dev/null 2>&1; then
    echo "error: qemu-system-arm not found on PATH." >&2
    echo "       install it with e.g. 'sudo apt-get install -y qemu-system-arm'." >&2
    exit 2
fi

echo ">>> building and running $APP examples under QEMU"
# Run from the app dir so its `.cargo/config.toml` (target triple + QEMU runner)
# is discovered by Cargo — `--manifest-path` alone would ignore it and fall back
# to the host target. The armv6m build is selected purely by the target triple
# (see `examples-apps/Cargo.toml`).
cd ${APP_DIR}
cargo build --example hello_rtic --features swtasks $TARGET_FLAGS
timeout --foreground 30s bash -c "cargo run --example hello_rtic --features swtasks $TARGET_FLAGS"

echo ""
echo ">>> running sw_queue_depth"
cargo build --example sw_queue_depth --features swtasks $TARGET_FLAGS
timeout --foreground 30s bash -c "cargo run --example sw_queue_depth --features swtasks $TARGET_FLAGS"

echo ""
echo ">>> running async_ping_pong with async feature"
cargo build --example async_ping_pong --features async $TARGET_FLAGS
timeout --foreground 60s bash -c "cargo run --example async_ping_pong --features async $TARGET_FLAGS"

echo ""
echo ">>> running async_queue_depth with async feature"
cargo build --example async_queue_depth --features async $TARGET_FLAGS
timeout --foreground 60s bash -c "cargo run --example async_queue_depth --features async $TARGET_FLAGS"

cd -
