.PHONY: all ci fmt fmt-check clippy test test-default test-multibin

export RUSTFLAGS := -Dwarnings

all: fmt-check clippy test

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --all-targets --all-features

test: test-default test-multibin

test-default:
	cargo test --features proc-macro

test-multibin:
	cargo test --features proc-macro,multibin
