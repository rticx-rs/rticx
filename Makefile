.PHONY: all ci fmt fmt-check clippy test qemu qemu-armv7m qemu-armv6m qemu-slic qemu-espc3

CRATES := rticx-core \
          rticx-spsc \
          rticx-async \
          compilation-passes/rticx-sw-pass \
          compilation-passes/rticx-auto-assign \
          compilation-passes/rticx-deadline-pass \
          compilation-passes/rticx-async-pass \
          tools/rticx-expand

# Default target: run everything CI would run.
all: fmt-check test clippy distros

# Alias for CI.
ci: all

# -----------------------------------------------------------------------------
# Formatting
# -----------------------------------------------------------------------------

fmt:
	@for crate in $(CRATES); do \
		$(MAKE) -C $$crate fmt || exit 1; \
	done
	make -C distributions/rticx-cortex-m fmt
	make -C distributions/rticx-rp2040 fmt
	make -C distributions/rticx-riscv fmt

fmt-check:
	@for crate in $(CRATES); do \
		$(MAKE) -C $$crate fmt-check || exit 1; \
	done

# -----------------------------------------------------------------------------
# Clippy (warnings treated as errors via RUSTFLAGS in each crate Makefile)
# -----------------------------------------------------------------------------

clippy:
	@for crate in $(CRATES); do \
		$(MAKE) -C $$crate clippy || exit 1; \
	done

# -----------------------------------------------------------------------------
# Tests
# -----------------------------------------------------------------------------

test:
	@for crate in $(CRATES); do \
		$(MAKE) -C $$crate test || exit 1; \
	done

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
qemu: qemu-armv7m qemu-armv6m qemu-slic qemu-espc3 qemu-espc3

qemu-armv7m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv7m

qemu-armv6m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv6m

qemu-slic:
	@$(MAKE) -C distributions/rticx-riscv/examples/slic-examples

qemu-espc3:
	make -C distributions/rticx-riscv/examples/esp32c3-examples

distros: 
	make -C distributions/rticx-cortex-m clippy fmt-check
	make -C distributions/rticx-riscv clippy fmt-check
	make -C distributions/rticx-rp2040 clippy fmt-check examples # build examples because we don't have qemu for rp2040