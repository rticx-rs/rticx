.PHONY: all ci fmt fmt-check clippy test distros qemu qemu-armv7m qemu-armv6m qemu-slic qemu-espc3 check-versions

# rticx-cortex-m's lib does not compile for the host target (BASEPRI path is
# armv7-m only); it is exercised through its own Makefile with real targets
# (see `distros` and `qemu-*` below).
WS_EXCLUDES := --exclude rticx-cortex-m --exclude rticx-cortex-m-macro


# Default target: run everything CI would run.
all: fmt-check test clippy distros

# Alias for CI.
ci: all

# -----------------------------------------------------------------------------
# Versioning
# -----------------------------------------------------------------------------

check-versions:
	./tools/check-versions.sh

# -----------------------------------------------------------------------------
# Formatting
# -----------------------------------------------------------------------------

fmt:
	cargo fmt --all
	make -C distributions/rticx-riscv fmt
	make -C distributions/rticx-rp2040 fmt

fmt-check:
	cargo fmt --all --check

# -----------------------------------------------------------------------------
# Clippy (warnings treated as errors via RUSTFLAGS)
# -----------------------------------------------------------------------------

clippy:
	RUSTFLAGS="-Dwarnings" cargo clippy --workspace $(WS_EXCLUDES) --all-targets --all-features
	make -C distributions/rticx-cortex-m clippy

# -----------------------------------------------------------------------------
# Tests
# -----------------------------------------------------------------------------

test:
	cargo test --workspace $(WS_EXCLUDES) --exclude rticx-auto-assign
	cargo test -p rticx-auto-assign -- --test-threads=1

# -----------------------------------------------------------------------------
# Distributions (riscv & rp2040 are standalone crates, not workspace members;
# cortex-m is a member but its host build is impossible, hence per-dir cmds)
# -----------------------------------------------------------------------------

distros:
	make -C distributions/rticx-cortex-m clippy fmt-check
	make -C distributions/rticx-riscv clippy fmt-check
	make -C distributions/rticx-rp2040 clippy fmt-check examples # build examples because we don't have qemu for rp2040

# -----------------------------------------------------------------------------
# QEMU playground (rticx-cortex-m + rticx-riscv)
# -----------------------------------------------------------------------------
# Boots the rticx-cortex-m examples under QEMU's `lm3s6965evb` (Cortex-M3)
# machine, the rticx-riscv SLIC examples under `sifive_e`, and the ESP32-C3
# examples under the Espressif QEMU fork. Each example terminates itself via
# `debug::exit` semihosting, so these targets fail (non-zero) unless the
# example reaches its expected state under RTIC's SRP locking.
#
# Requires `qemu-system-arm` on PATH (e.g. `sudo apt-get install -y
# qemu-system-arm`) and the `thumbv7m-none-eabi` / `thumbv6m-none-eabi` /
# `riscv32imc-unknown-none-elf` Rust targets. `qemu-espc3` additionally needs
# `espflash`, `esptool`, and the Espressif QEMU fork
# (https://github.com/espressif/qemu) whose `qemu-system-riscv32` must be on
# PATH or pointed to via `QEMU_SYSTEM_RISCV32`:
#
#   QEMU_SYSTEM_RISCV32=~/qemu/bin/qemu-system-riscv32 make qemu-espc3
#
# Not part of `all`/`ci` so a missing QEMU install doesn't break the
# host-only check/test/clippy jobs.
qemu: qemu-armv7m qemu-armv6m qemu-slic qemu-espc3

qemu-armv7m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv7m

qemu-armv6m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv6m

qemu-slic:
	@$(MAKE) -C distributions/rticx-riscv/examples/slic-examples

qemu-espc3:
	@$(MAKE) -C distributions/rticx-riscv/examples/esp32c3-examples all
