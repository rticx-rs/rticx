# MMRTIC: Multicore, Distributions and Compilation Passes

This repository contains a from scratch rewrite of the RTIC (Real-Time Interrupt-driven Concurrency) framework for Rust. The goal is to make RTIC more maintainable, extensible, and portable by separating the generic proc-macro logic from target-specific details and by allowing new language features to be added as external, reusable compilation passes.

The result is a small core framework (`rtic-core`) plus a growing ecosystem of **compilation passes** and **distributions**:

- **Compilation passes** are independent crates that transform RTIC syntax.
- **Distributions** are target-specific crates that implement backend traits, register the passes they want, and expose the final `#[rtic::app]` macro.

In addition, the user application syntax has been refactored to provide less magic and more idiomatic Rust experience while preserving the core concepts behind RTIC (Tasks and Resources model). 

This repository maintains the core framework and a set of reference distributions. New hardware distributions are developed as out-of-tree crates and are not hosted here.

## Architecture

- **`rtic-core`** provides the parser, resource-ceiling analysis (SRP), code generation for tasks/resources/init/idle, and the `RticMacroBuilder` API for chaining passes.
- **Compilation passes** implement the `RticPass` trait and run before or after the core pass as pure syntax-to-syntax transformations.
- **Distributions** provide the low-level hardware bindings via the `CorePassBackend` trait (and optional pass-specific backends), select which passes to use, and re-export the generated `#[rtic::app]` macro.

## Repository layout

| Path | Crate / Directory | Role |
|------|-------------------|------|
| `rtic-core/` | `rtic-core` | Core parser, analysis, codegen, and `RticMacroBuilder`. |
| `rtic-spsc/` | `rtic-spsc` | `no_std` single-producer single-consumer queue used by the software tasks pass. |
| `compilation-passes/rtic-sw-pass/` | `rtic-sw-pass` | Software tasks pass: dispatchers, message queues, `spawn`, `spawn_from`. |
| `compilation-passes/rtic-auto-assign/` | `rtic-auto-assign` | Automatic `core = N` assignment based on shared resource usage. |
| `compilation-passes/rtic-deadline-pass/` | `rtic-deadline-pass` | Converts `deadline = D` attributes into RTIC priorities. |
| `distributions/cortex-m-rtic/` | `cortex-m-rtic` | Single-core Cortex-M (armv6-m and armv7-m and above) distribution. |
| `distributions/rp2040-rtic/` | `rp2040-rtic` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ distribution. |
| `distributions/stm32-renode-rtic/` | `stm32-renode-rtic` | Renode-simulated multicore STM32F1C3-like distribution. |
| `distributions/rtic-hippo/` | `rtic-hippo` | Single-core RISC-V Hippomenes MCU distribution. |
| `distributions/atalanta-rtic/` | `atalanta-rtic` | Single-core RISC-V Atalanta MCU distribution. |
| `distributions/distribution-template/` | `distribution-template` | Conceptual starting point for new distributions. |

## Supported distributions

| Distribution | Target | Features |
|--------------|--------|----------|
| `cortex-m-rtic` | Single-core Cortex-M (armv6-m and armv7-m and above) | `swtasks` (default), `armv6m` — runnable under QEMU |
| `rp2040-rtic` | Raspberry Pi Pico / RP2040 (dual-core Cortex-M0+) | `autoassign`, `swtasks` |
| `stm32-renode-rtic` | Renode-simulated multicore STM32F1C3-like | N/A |
| `rtic-hippo` | Single-core RISC-V Hippomenes MCU | `deadline-pass` |
| `atalanta-rtic` | Single-core RISC-V Atalanta MCU | `deadline-pass` |

## Quick start

The fastest way to see the framework in action is the `cortex-m-rtic` QEMU playground, which exercises real Cortex-M core-peripheral
init (SysTick), a hardware task bound to the `SysTick` exception, and a software task on an NVIC dispatcher that acquires a shared resource
through RTIC's SRP `lock`.

```bash
# Prereqs: qemu-system-arm and the two Cortex-M Rust targets
sudo apt-get install -y qemu-system-arm
rustup target add thumbv7m-none-eabi thumbv6m-none-eabi

# Run both locking codepaths; fails (non-zero) if either misbehaves
make qemu
```

The examples are located in `distributions/cortex-m-rtic/example-apps`. You modify them, rebuild and run on qemu:

```bash
cd distributions/cortex-m-rtic/example-apps/armv7m-app && cargo run --example hello_rtic
cd distributions/cortex-m-rtic/example-apps/armv6m-app && cargo run --example hello_rtic
```

## Examples

- [QEMU-runnable RTIC playground: SysTick hw task + spawned sw task + SRP lock + `debug::exit`](distributions/cortex-m-rtic/example-apps/armv7m-app/examples/hello_rtic.rs)
- [Single-core RTIC application with software tasks](distributions/rp2040-rtic/examples/hello_rtic.rs)
- [Multicore ping-pong with cross-core communication](distributions/rp2040-rtic/examples/ping_pong.rs)

## Documentation

Full user and distributor guides are available in the [project wiki](https://github.com/zakimadaoui/rtic-mc-experiments/wiki).

## Academic Publications

- [Master thesis: Modular and Multicore RTIC](https://trepo.tuni.fi/bitstream/10024/162037/2/MadaouiZakaria.pdf)
- [Paper: Towards modularity of the Rust RTIC real-time scheduling framework](https://ieeexplore.ieee.org/document/10752441)
- [Paper: Modular RTIC: Lightweight Real Time for Customized Architectures](https://www.diva-portal.org/smash/get/diva2:1993122/FULLTEXT01.pdf)
