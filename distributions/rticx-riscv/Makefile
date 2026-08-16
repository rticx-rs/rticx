.PHONY: all ci fmt fmt-check clippy examples


all: fmt-check clippy examples

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	RUSTFLAGS="-Dwarnings" cargo clippy --features "slic","mecall-backend" --target riscv32imc-unknown-none-elf
	RUSTFLAGS="-Dwarnings" cargo clippy --features "slic","clint-backend" --target riscv32imc-unknown-none-elf
	RUSTFLAGS="-Dwarnings" cargo clippy --features "esp32c3" --target riscv32imc-unknown-none-elf
	RUSTFLAGS="-Dwarnings" cargo clippy --features "esp32c6" --target riscv32imc-unknown-none-elf

examples: examples-slic examples-esp

examples-slic:
	make -C examples/slic-examples

examples-esp:
	make -C examples/esp32c3-examples

