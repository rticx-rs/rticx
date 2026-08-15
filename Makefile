.PHONY: all ci fmt fmt-check clippy test qemu qemu-armv7m qemu-armv6m

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
# QEMU playground (rticx-cortex-m)
# -----------------------------------------------------------------------------
# Boots the rticx-cortex-m examples under QEMU's `lm3s6965evb` (Cortex-M3)
# machine. Each example terminates itself via `debug::exit` from
# `cortex-m-semihosting`, so these targets fail (non-zero) unless the example
# reaches its expected shared-counter value under RTIC's SRP locking.
#
# Requires `qemu-system-arm` on PATH (e.g. `sudo apt-get install -y
# qemu-system-arm`) and the `thumbv7m-none-eabi` / `thumbv6m-none-eabi` Rust
# targets. Not part of `all`/`ci` so a missing QEMU install doesn't break the
# host-only check/test/clippy jobs.
qemu: qemu-armv7m qemu-armv6m qemu-slic

qemu-armv7m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv7m

qemu-armv6m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv6m

qemu-slic:
	@$(MAKE) -C distributions/rticx-riscv/examples/slic-examples

distros: 
	make -C distributions/rticx-cortex-m clippy fmt-check
	make -C distributions/rticx-riscv clippy fmt-check
	make -C distributions/rticx-rp2040 all
# FIXME: enable this once CI supports esp32 qemu
# 	make -C distributions/rticx-riscv/examples/esp32c3-examples