# Build/test/run helpers for the rticx-stm32h7 distribution.
#
# The distribution is a member of the experimental multi-binary workspace
# (`experiments/multibinary`), but its `no_std` library only compiles for the
# Cortex-M targets, so every build here names `--target` and exactly one of the
# `cm7`/`cm4` features.

CARGO ?= cargo
# `cargo xbin` runs the phase-1 metadata collection and phase-2 codegen.
CARGO_XBIN ?= cargo xbin
M7_TRIPLE := thumbv7em-none-eabihf
M4_TRIPLE := thumbv7em-none-eabi
APP_DIR := examples-apps
M7_ELF := $(APP_DIR)/target/$(M7_TRIPLE)/debug/cm7
M4_ELF := $(APP_DIR)/target/$(M4_TRIPLE)/debug/cm4

.PHONY: all fmt fmt-check clippy build examples renode clean

all: fmt-check clippy build examples

fmt:
	$(CARGO) fmt -p rticx-stm32h7 -p rticx-stm32h7-macro -p rticx-stm32h7-bindings
	cd $(APP_DIR) && $(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt -p rticx-stm32h7 -p rticx-stm32h7-macro -p rticx-stm32h7-bindings --check
	cd $(APP_DIR) && $(CARGO) fmt --all --check

clippy:
	RUSTFLAGS="-Dwarnings" $(CARGO) clippy -p rticx-stm32h7 --target $(M7_TRIPLE) --features cm7
	RUSTFLAGS="-Dwarnings" $(CARGO) clippy -p rticx-stm32h7 --target $(M4_TRIPLE) --features cm4

build:
	$(CARGO) build -p rticx-stm32h7 --target $(M7_TRIPLE) --features cm7
	$(CARGO) build -p rticx-stm32h7 --target $(M4_TRIPLE) --features cm4

# Build the two acceptance binaries through the cross-binary driver, including
# the linked-ELF pool verification (M6.9-T7).
examples:
	cd $(APP_DIR) && $(CARGO_XBIN) build --verify-elf

# Run the acceptance demo under the in-tree Renode harness.
# Usage: make renode [SECONDS=4]
SECONDS ?= 4
renode: examples
	./renode/run.sh "$(CURDIR)/$(M7_ELF)" "$(CURDIR)/$(M4_ELF)" $(SECONDS)

clean:
	cd $(APP_DIR) && $(CARGO) clean
