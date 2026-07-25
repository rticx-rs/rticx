.PHONY: all ci fmt fmt-check clippy test

export RUSTFLAGS := -Dwarnings

all: fmt-check clippy test

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --all-targets --all-features

test:
	# Tests are run single-threaded because `APP_CORES` is a process-global
	# `AtomicU32` that is read across the parse->run boundary, so multi-core
	# and single-core tests race when run in parallel.
	cargo test -- --test-threads=1
