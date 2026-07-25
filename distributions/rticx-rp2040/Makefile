.PHONY: all ci fmt fmt-check clippy examples

export RUSTFLAGS := -Dwarnings

all: fmt-check clippy examples

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --all-targets --all-features

examples:
	cargo build --examples

