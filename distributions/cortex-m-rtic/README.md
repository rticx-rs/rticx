# `rtic` — Single-core Cortex-M RTIC distribution

This distribution targets single-core Cortex-M microcontrollers, mirroring the
upstream RTIC cortex-m backend. It supports two mutually-exclusive locking
strategies selected at crate-feature time:

| Feature   | Architecture          | Locking mechanism                          |
|-----------|-----------------------|--------------------------------------------|
| *(default)* | armv7-m and above    | BASEPRI register (priority threshold)      |
| `armv6m`  | armv6-m (M0/M0+/M23)  | Interrupt source masking via NVIC ISER/ICER|

Software tasks are enabled by default through the `swtasks` feature; disable it (`--no-default-features`) for a hardware-task-only build.

## Layout

```
cortex-m-rtic/
├── src/                 # `rtic` library: re-exports `app` macro and runtime exports
├── rtic-macro/           # proc-macro crate implementing CorePassBackend + SwPassBackend
├── qemu-run.sh           # build + boot an example under QEMU
└── example-apps/
    ├── armv7m-app/       # thumbv7m example using BASEPRI locking
    └── armv6m-app/       # thumbv6m example using interrupt source masking
```

Both example apps use the *same* `stm32f0::stm32f0x0` PAC and the *same*
`hello_rtic.rs` source. The only differences between them are the build target
triple and the `armv6m` RTIC feature.

## The QEMU playground

The `hello_rtic` example is a simple RTIC application that runs under QEMU and exercises the core primitives provided by this distribution:

1. `#[init]` configures the SysTick timer (`SYST`) to fire periodically.
2. **Hardware task bound to an exception**: `Tick` is bound to the `SysTick` exception handler. On each tick it spawns the software task.
3. **Software task on an NVIC dispatcher**  `Worker` runs off the `TIM6` NVIC interrupt, acquires the shared `counter` through a resource lock, increments it, and once it reaches `TARGET` calls `debug::exit(EXIT_SUCCESS)`.


### Prerequisites

```bash
# QEMU (Linux/Debian; macOS: `brew install qemu`)
sudo apt-get install -y qemu-system-arm

# Rust targets (CI runs `rustup target add` automatically)
rustup target add thumbv7m-none-eabi thumbv6m-none-eabi
```

### Running the examples

```bash
./qemu-run.sh armv7m
./qemu-run.sh armv6m
```

Or, per-example (the `.cargo/config.toml` wires up the QEMU runner, so `cargo
run` both builds and boots QEMU):

```bash
cd example-apps/armv7m-app && cargo run --example hello_rtic
cd example-apps/armv6m-app && cargo run --example hello_rtic
```