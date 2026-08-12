.PHONY: all run run-async run-led-toggler run-multitasker run-hello run-ping-pong 
all: build-async build-led-toggler build-multitasker build-hello build-ping-pong


run-async:
	cargo run --bin async_multitasker --features async

build-async:
	cargo build --bin async_multitasker --features async

run-led-toggler:
	cargo run --bin led_toggler_mmrtic --features swtasks

build-led-toggler:
	cargo build --bin led_toggler_mmrtic --features swtasks

run-multitasker:
	cargo run --bin mmrtic_multitasker_external_tasks --features swtasks

build-multitasker:
	cargo build --bin mmrtic_multitasker_external_tasks --features swtasks

run-hello:
	cargo run --bin hello_rtic --features swtasks

build-hello:
	cargo build --bin hello_rtic --features swtasks

run-ping-pong:
	cargo run --bin ping_pong --features swtasks

build-ping-pong:
	cargo build --bin ping_pong --features swtasks