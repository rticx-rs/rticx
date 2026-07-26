# RTICX: eXtensible Realtime Interrupt Driven Concurrency Framework

This repository contains a from scratch rewrite of the [original RTIC framework](https://github.com/rtic-rs/rtic). The goal is to make it more maintainable, extensible, and easily portable to new hardware architectures (including multicore). The main idea is to separate the generic proc-macro logic from target-specific details and by allowing new language features to be added as external, reusable compilation passes.

The result is a small core framework (`rticx-core`) plus a growing ecosystem of **compilation passes** and **distributions**:

- **Compilation passes** are independent crates that transform and expand user application syntax.
- **Distributions** are target-specific crates that implement backend traits, register the passes they want, and expose the final `#[<distro>::app]` macro.

In addition, the user application syntax (Referred to now as RTICX syntax) has been refactored to provide less magic and more idiomatic Rust experience while preserving the core concepts of the original RTIC framework (Tasks and Resources model). 

This repository maintains the core framework and a set of reference distributions. New hardware distributions are developed as out-of-tree crates and are not hosted here.

## Architecture

- **`rticx-core`** provides the parser, resource-ceiling analysis (SRP), code generation for tasks/resources/init/idle, and the `RticMacroBuilder` API for chaining passes.
- **Compilation passes** implement the `RticPass` trait and run before or after the core pass as pure syntax-to-syntax transformations.
- **Distributions** provide the low-level hardware bindings via the `CorePassBackend` trait (and optional pass-specific backends), select which passes to use, and re-export the generated `#[<distro>::app]` macro.

## Repository layout

| Path | Crate / Directory | Role |
|------|-------------------|------|
| `rticx-core/` | `rticx-core` | Core parser, analysis, codegen, and `RticMacroBuilder`. |
| `rticx-spsc/` | `rticx-spsc` | `no_std` single-producer single-consumer queue used by the software tasks pass. |
| `compilation-passes/rticx-sw-pass/` | `rticx-sw-pass` | Software tasks pass: dispatchers, message queues, `spawn`, `spawn_from`. |
| `compilation-passes/rticx-auto-assign/` | `rticx-auto-assign` | Automatic `core = N` assignment based on shared resource usage. |
| `compilation-passes/rticx-deadline-pass/` | `rticx-deadline-pass` | Converts `deadline = D` attributes into RTICX priorities. |
| `distributions/rticx-cortex-m/` | `rticx-cortex-m` | Single-core Cortex-M (armv6-m and armv7-m and above) distribution. |
|  `distributions/rticx-riscv/` | `rticx-riscv` | Single-core riscv with generic SLIC interrupt controller/ esp32c3/ esp32c6 |
| `distributions/rticx-rp2040/` | `rticx-rp2040` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ distribution. |
| `distributions/rticx-stm32-renode/` | `rticx-stm32-renode` | Renode-simulated multicore STM32F1C3-like distribution. |
| `distributions/rticx-hippo/` | `rticx-hippo` | Single-core RISC-V Hippomenes MCU distribution. |
| `distributions/rticx-atalanta/` | `rticx-atalanta` | Single-core RISC-V Atalanta MCU distribution. |
| `distributions/distribution-template/` | `distribution-template` | Conceptual starting point for new distributions. |

## Supported distributions

| Distribution | Target | Features |
|--------------|--------|----------|
| `rticx-cortex-m` | Single-core Cortex-M (armv6-m and armv7-m and above) | `swtasks` (default), `armv6m` — runnable under QEMU |
| `rticx-riscv` | Single-core riscv with generic SLIC interrupt controller/ esp32c3/ esp32c6 | See README.md of the distro |
| `rticx-rp2040` | Raspberry Pi Pico / RP2040 (dual-core Cortex-M0+) | `autoassign`, `swtasks` |
| `rticx-stm32-renode` | Renode-simulated multicore STM32F1C3-like | N/A |
| `rticx-hippo` | Single-core RISC-V Hippomenes MCU | `deadline-pass` |
| `rticx-atalanta` | Single-core RISC-V Atalanta MCU | `deadline-pass` |

## Quick start

The fastest way to see the framework in action is the `rticx-cortex-m` QEMU playground, which exercises real Cortex-M core-peripheral
init (SysTick), a hardware task bound to the `SysTick` exception, and a software task on an NVIC dispatcher that acquires a shared resource
through RTIC's SRP `lock`.

```bash
# Prereqs: qemu-system-arm and the two Cortex-M Rust targets
sudo apt-get install -y qemu-system-arm
rustup target add thumbv7m-none-eabi thumbv6m-none-eabi

# Run both locking codepaths; fails (non-zero) if either misbehaves
make qemu
```

The examples are located in `distributions/rticx-cortex-m/example-apps`. You modify them, rebuild and run on qemu:

```bash
cd distributions/rticx-cortex-m/example-apps/armv7m-app && cargo run --example hello_rtic
cd distributions/rticx-cortex-m/example-apps/armv6m-app && cargo run --example hello_rtic
```

## Examples

- [QEMU-runnable RTICX playground: SysTick hw task + spawned sw task + SRP lock + `debug::exit`](distributions/rticx-cortex-m/example-apps/armv7m-app/examples/hello_rtic.rs)
- [Single-core RTICX application with software tasks](distributions/rticx-rp2040/examples/hello_rtic.rs)
- [Multicore ping-pong with cross-core communication](distributions/rticx-rp2040/examples/ping_pong.rs)

## Documentation

Full user and distributor guides are available in the [project wiki](https://github.com/zakimadaoui/rtic-mc-experiments/wiki).

## Academic Publications

- [Master thesis: Modular and Multicore RTIC](https://trepo.tuni.fi/bitstream/10024/162037/2/MadaouiZakaria.pdf)
- [Paper: Towards modularity of the Rust RTIC real-time scheduling framework](https://ieeexplore.ieee.org/document/10752441)
- [Paper: Modular RTIC: Lightweight Real Time for Customized Architectures](https://www.diva-portal.org/smash/get/diva2:1993122/FULLTEXT01.pdf)
