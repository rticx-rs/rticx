.PHONY: all ci fmt fmt-check clippy test \
        test-default test-multipac test-multibin test-multipac-multibin

export RUSTFLAGS := -Dwarnings

all: fmt-check clippy test

ci: all

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --all-targets --all-features

test: test-default test-multipac test-multibin test-multipac-multibin

test-default:
	cargo test

test-multipac:
	cargo test --features multipac

test-multibin:
	cargo test --features multibin

test-multipac-multibin:
	cargo test --features multipac,multibin
