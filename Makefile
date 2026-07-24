.PHONY: all ci fmt fmt-check clippy test qemu qemu-armv7m qemu-armv6m

CRATES := rtic-core \
          rtic-spsc \
          compilation-passes/rtic-sw-pass \
          compilation-passes/rtic-auto-assign \
          compilation-passes/rtic-deadline-pass

# Default target: run everything CI would run.
all: fmt-check test clippy

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
# QEMU playground (cortex-m-rtic)
# -----------------------------------------------------------------------------
# Boots the cortex-m-rtic examples under QEMU's `lm3s6965evb` (Cortex-M3)
# machine. Each example terminates itself via `debug::exit` from
# `cortex-m-semihosting`, so these targets fail (non-zero) unless the example
# reaches its expected shared-counter value under RTIC's SRP locking.
#
# Requires `qemu-system-arm` on PATH (e.g. `sudo apt-get install -y
# qemu-system-arm`) and the `thumbv7m-none-eabi` / `thumbv6m-none-eabi` Rust
# targets. Not part of `all`/`ci` so a missing QEMU install doesn't break the
# host-only check/test/clippy jobs.
qemu: qemu-armv7m qemu-armv6m

qemu-armv7m:
	@$(MAKE) -C distributions/cortex-m-rtic qemu-armv7m

qemu-armv6m:
	@$(MAKE) -C distributions/cortex-m-rtic qemu-armv6m
