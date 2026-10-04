#!/usr/bin/env bash
#
# Run a dual-binary STM32H7 project (Cortex-M7 + Cortex-M4) on the dual-core
# Renode platform, headless, with USART1 (M7) and USART2 (M4) routed to the
# Renode log.
#
# Usage:
#   ./run.sh <cortex-m7.elf> <cortex-m4.elf> [seconds]
#
# The M7 image boots from flash bank 1 (0x0800_0000) and must release the M4
# through RCC_GCR.BOOT_C2; the M4 image boots from flash bank 2 (0x0810_0000)
# once released.
set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <cortex-m7.elf> <cortex-m4.elf> [seconds]" >&2
    exit 2
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Resolve the images to absolute paths *before* changing directory below:
# Renode would otherwise look them up relative to this script's directory.
m7_elf="$(readlink -f "$1")"
m4_elf="$(readlink -f "$2")"
run_seconds="${3:-3}"

for elf in "$m7_elf" "$m4_elf"; do
    if [ ! -f "$elf" ]; then
        echo "error: firmware image not found: $elf" >&2
        exit 2
    fi
done

if ! command -v renode >/dev/null 2>&1; then
    echo "error: renode not found on PATH" >&2
    exit 2
fi

# `$ORIGIN` in the .resc resolves the platform relative to the script, so run
# from this directory with the upstream-relative path.
cd "$here"
exec renode --console --disable-gui \
    -e "\$m7_elf=@$m7_elf" \
    -e "\$m4_elf=@$m4_elf" \
    -e 'i @scripts/single-node/stm32h7_dualcore.resc' \
    -e 'showAnalyzer sysbus.usart1' \
    -e 'showAnalyzer sysbus.usart2' \
    -e 'start' \
    -e "python \"import time; time.sleep($run_seconds)\"" \
    -e 'quit'
