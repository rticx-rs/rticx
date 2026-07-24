# Getting Started

This guide walks you through building your first RTIC application using an existing distribution.

## Prerequisites

- A working Rust toolchain (the latest stable release is recommended).
- The target toolchain for your chosen distribution. For example, for the RP2040 examples you need:
  ```bash
  rustup target add thumbv6m-none-eabi
  ```
- Required tools for running Renode-based examples if you use `stm32-renode-rtic`.

## Choose a distribution

This repository ships several reference distributions. Pick one that matches your hardware or simulator:

| Distribution | Target | Good for |
|--------------|--------|----------|
| `rp2040-rtic` | Raspberry Pi Pico / RP2040 | Getting started, single-core and multicore examples |
| `stm32-renode-rtic` | Renode-simulated STM32F1C3-like | Multicore simulation |
| `rtic-hippo` | RISC-V Hippomenes | Single-core RISC-V experiments |
| `atalanta-rtic` | RISC-V Atalanta | Single-core RISC-V experiments |

## Build an example

There is no root `Cargo.toml`. Each crate is built independently.

### RP2040 examples

```bash
cd distributions/rp2040-rtic
cargo build --example hello_rtic
cargo build --example ping_pong
```

The `swtasks` feature is enabled by the example dev-dependencies, so you do not need to pass `--features`.

### Renode STM32 multicore example

`stm32-renode-rtic` uses the `multibin` feature, so each core is compiled separately:

```bash
cd distributions/stm32-renode-rtic
RUSTFLAGS='--cfg core="0"' cargo build --example ping_pong
RUSTFLAGS='--cfg core="1"' cargo build --example ping_pong
```

Then run the Renode emulation script provided in the distribution.

### Running unit tests

The only crate in this repository with host-runnable unit tests is `rtic-spsc`:

```bash
cd rtic-spsc
cargo test
```

## Create your own application

1. Create a new Rust binary crate that depends on one of the distributions, e.g.:
   ```toml
   [dependencies]
   rp2040-rtic = { path = "path/to/distributions/rp2040-rtic" }
   ```
2. Write an RTIC application using the attributes described in the [Syntax Reference](User-Guide-Syntax).
3. Build and flash/run as appropriate for your target.

## Next steps

- Read the [Syntax Reference](User-Guide-Syntax) to learn the supported attributes.
- Check the [Supported Distributions](User-Guide-Supported-Distributions) page for feature flags and target details.
