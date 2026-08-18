.PHONY: all ci fmt fmt-check clippy test distros qemu qemu-armv7m qemu-armv6m check-versions

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
# Distributions (rticx-riscv / rticx-rp2040 moved to their own repositories;
# cortex-m stays in-tree as the reference distro but its host build is
# impossible, hence per-dir cmds)
# -----------------------------------------------------------------------------

distros:
	make -C distributions/rticx-cortex-m clippy fmt-check

# -----------------------------------------------------------------------------
# QEMU playground (rticx-cortex-m)
# -----------------------------------------------------------------------------
# Boots the rticx-cortex-m examples under QEMU's `lm3s6965evb` (Cortex-M3)
# machine. 
#
# Requires `qemu-system-arm` on PATH (e.g. `sudo apt-get install -y
# qemu-system-arm`) and the `thumbv7m-none-eabi` / `thumbv6m-none-eabi`
# Rust targets.
#
# Not part of `all`/`ci` so a missing QEMU install doesn't break the
# host-only check/test/clippy jobs.
qemu: qemu-armv7m qemu-armv6m

qemu-armv7m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv7m

qemu-armv6m:
	@$(MAKE) -C distributions/rticx-cortex-m qemu-armv6m
