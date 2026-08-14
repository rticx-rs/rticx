.PHONY: all ci qemu qemu-armv7m qemu-armv6m

all: qemu fmt-check clippy examples

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --target thumbv7m-none-eabi
	cargo clippy --target thumbv6m-none-eabi --features armv6m

qemu: qemu-armv7m qemu-armv6m

qemu-armv7m:
	./qemu-run.sh armv7m

qemu-armv6m:
	./qemu-run.sh armv6m