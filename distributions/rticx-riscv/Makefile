.PHONY: all ci fmt fmt-check clippy examples


all: fmt-check clippy examples

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	RUSTFLAGS="-Dwarnings" cargo clippy --features "slic","mecall-backend"
	RUSTFLAGS="-Dwarnings" cargo clippy --features "slic","clint-backend"
	RUSTFLAGS="-Dwarnings" cargo clippy --features "esp32c3"
	RUSTFLAGS="-Dwarnings" cargo clippy --features "esp32c6"

examples:
	make -C examples/slic-examples
	make -C examples/esp32c3-examples

