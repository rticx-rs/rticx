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
	cargo test

