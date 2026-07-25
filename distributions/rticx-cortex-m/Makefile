.PHONY: all ci qemu qemu-armv7m qemu-armv6m

all: qemu

ci: all

qemu: qemu-armv7m qemu-armv6m

qemu-armv7m:
	./qemu-run.sh armv7m

qemu-armv6m:
	./qemu-run.sh armv6m