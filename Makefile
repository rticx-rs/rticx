.PHONY: all ci \
        fmt-check fmt-check-core fmt-check-spsc \
        clippy clippy-core clippy-spsc \
        test test-core test-core-multipac test-core-multibin test-core-multipac-multibin test-spsc

export RUSTFLAGS := -Dwarnings

# Default target: run everything CI would run.
all: fmt-check test clippy

# Alias for CI.
ci: all

# -----------------------------------------------------------------------------
# Formatting checks
# -----------------------------------------------------------------------------

fmt: 
	cd rtic-core && cargo fmt --check
	cd rtic-spsc && cargo fmt --check
fmt-check: fmt-check-core fmt-check-spsc

fmt-check-core:
	cd rtic-core && cargo fmt --check

fmt-check-spsc:
	cd rtic-spsc && cargo fmt --check

# -----------------------------------------------------------------------------
# Clippy (warnings treated as errors via RUSTFLAGS)
# -----------------------------------------------------------------------------

clippy: clippy-core clippy-spsc

clippy-core:
	cd rtic-core && cargo clippy --all-targets --all-features

clippy-spsc:
	cd rtic-spsc && cargo clippy --all-targets

# -----------------------------------------------------------------------------
# Tests
# -----------------------------------------------------------------------------

test: test-core test-core-multipac test-core-multibin test-core-multipac-multibin test-spsc

test-core:
	cd rtic-core && cargo test

test-core-multipac:
	cd rtic-core && cargo test --features multipac

test-core-multibin:
	cd rtic-core && cargo test --features multibin

test-core-multipac-multibin:
	cd rtic-core && cargo test --features multipac,multibin

test-spsc:
	cd rtic-spsc && cargo test
