.PHONY: all ci fmt fmt-check clippy test

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
