.PHONY: all ci fmt fmt-check clippy examples

export RUSTFLAGS := -Dwarnings

all: fmt-check clippy examples

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --features "slic","mecall-backend"
	cargo clippy --features "slic","clint-backend"
	cargo clippy --features "esp32c3"
	cargo clippy --features "esp32c6"

examples:
# 	cargo build --examples

