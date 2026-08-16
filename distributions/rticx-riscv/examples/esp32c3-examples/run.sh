#!/bin/bash

if [ $# -eq 0 ]; then
  echo "No arguments supplied! Provide path to ELF as argument"
fi


DEBUG_FLAGS=""
if [ "${2:-}" == "debug" ]; then
  DEBUG_FLAGS=" -s -S "
fi


outputfilenamecargo=$1
outputfilename="$outputfilenamecargo".bin

logfile=qemu.log
qemuoutputfile=qemuoutput.log


# wget https://github.com/espressif/qemu/releases/download/esp-develop-9.2.2-20260417/qemu-riscv32-softmmu-esp_develop_9.2.2_20260417-x86_64-linux-gnu.tar.xz
# tar -xf qemu-riscv32-sot....tar.xz
# TODO: will have docker for this or expect it to be part of path
qemuexec="${QEMU_SYSTEM_RISCV32:-qemu-system-riscv32}"

# Create a temporary directory for all output files
tempdir=$(mktemp -d) || exit 1

# Building ESP32-C3 image
espflash save-image --chip esp32c3 --merge "$outputfilenamecargo" "$outputfilename" 1>&2

# Get stats
esptool image_info  "$outputfilename" 1>&2

# Run in QEMU
$qemuexec -nographic -monitor tcp:127.0.0.1:55555,server,nowait -icount 3 -machine esp32c3 -semihosting -drive file="$outputfilename",if=mtd,format=raw ${DEBUG_FLAGS}

# # 
# $qemuexec -nographic -monitor tcp:127.0.0.1:55555,server,nowait -icount 3 -machine esp32c3 -drive file="$outputfilename",if=mtd,format=raw -serial file:"$tempdir/$logfile" >"$tempdir"/$qemuoutputfile 2>&1 &

# qemupid=$!

# # Let it run
# sleep 3s

# # Kill QEMU nicely by sending 'q' (quit) over tcp
# echo q | nc -N 127.0.0.1 55555 >>"$tempdir"/$qemuoutputfile 2>&1
# # Output that will be compared must be printed to stdout

# sleep 0.1s
# # If still running, try again nicely
# pgrep -af "qemu-system.*esp32c3.*" >/dev/null 2>&1 && echo q | nc -N 127.0.0.1 55555 >>"$tempdir"/$qemuoutputfile 2>&1

# # Ask a bit more firmly with SIGTERM
# pgrep -af "qemu-system.*esp32c3.*" >/dev/null 2>&1 && kill $qemupid >/dev/null 2>&1

# pgrep -af "qemu-system.*esp32c3.*" >/dev/null 2>&1 && sleep 0.1s >/dev/null 2>&1

# # Time to die
# pgrep -af "qemu-system.*esp32c3.*" >/dev/null 2>&1 && kill -9 $qemupid >/dev/null 2>&1

# # Make boot phase silent, for debugging change, run with e.g.  $ `env DEBUGGING=true` cargo xtask....
# if [ -n "${DEBUGGING}" ]; then
#   # Debugging: strip leading "I (xyz)" where xyz is an incrementing number, and esp_image specifics
#   sed -e 's/esp_image: .*$/esp_image: REDACTED/' -e 's/I\s\([0-9]*\)(.*)/\1/' <"$tempdir"/$logfile
# else
#   tail -n +12 "$tempdir/$logfile" | sed -e '/I\s\([0-9]*\)(.*)/d'
# fi

# mv "$tempdir/$logfile" "$(basename "$outputfilename")"-$logfile
# mv "$tempdir/$qemuoutputfile" "$(basename "$outputfilename")"-$qemuoutputfile
# rm -r "$tempdir"

