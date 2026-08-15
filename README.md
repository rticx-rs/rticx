# RTICX: eXtensible Realtime Interrupt Driven Concurrency Framework

[![crates.io](https://img.shields.io/crates/v/rticx-core)](https://crates.io/crates/rticx-core)
[![wiki](https://img.shields.io/badge/docs-wiki-red)](https://github.com/rticx-rs/rticx/wiki/)
[![CI](https://github.com/rticx-rs/rticx/actions/workflows/ci.yml/badge.svg)](https://github.com/rticx-rs/rticx/actions/workflows/ci.yml)
[![QEMU](https://github.com/rticx-rs/rticx/actions/workflows/qemu.yml/badge.svg)](https://github.com/rticx-rs/rticx/actions/workflows/qemu.yml)


This is a from scratch rewrite of the [original RTIC framework](https://github.com/rtic-rs/rtic). The goal is to make it more maintainable, extensible, and easily portable to new hardware architectures (including multicore) in order to to reduce the barrier of entry for contributors and maintainers who wish to introduce newer syntax features and hardware ports.

The main idea is to breakdown RTIC's monolithic codebase by separating the generic proc-macro logic (RTIC syntax) from target-specific details (Interrupt handling, system initialization.. etc). Furthermore, the proc-macro logic is split to core and addons, where the core logic captures only the SRP Tasks/Resources model and the rest will be external addons like software tasks and async/await..etc.

The result is a small core framework (`rticx-core`) plus a growing ecosystem of **compilation passes** and **distributions**:

- **Compilation passes** are independent crates that transform and expand user application syntax.
- **Distributions** are target-specific crates that implement backend traits, register the passes they want, and expose the final `#[<distro>::app]` macro.

In addition, the user application syntax (Referred to now as RTICX syntax) has been refactored to provide less magic and more idiomatic Rust experience while preserving the core concepts of the original RTIC framework (Tasks and Resources model). 

This repository maintains the core framework and a set of reference distributions. New hardware distributions and fancier syntax extensions are developed as out-of-tree crates and are not hosted here.

## Architecture

- **`rticx-core`** provides the parser, resource-ceiling analysis (SRP), code generation for tasks/resources/init/idle, and the `RticMacroBuilder` API for chaining passes.
- **Compilation passes** implement the `RticPass` trait and run before or after the core pass as pure syntax-to-syntax transformations.
- **Distributions** provide the low-level hardware bindings via the `CorePassBackend` trait (and optional pass-specific backends), select which passes to use, and re-export the generated `#[<distro>::app]` macro.

## Documentation

Full user and distributor guides are available in the [project wiki](https://github.com/rticx-rs/rticx/wiki/).

## Repository layout

| Path | Crate / Directory | Role |
|------|-------------------|------|
| `rticx-core/` | `rticx-core` | Core parser, analysis, codegen, and `RticMacroBuilder`. |
| `rticx-spsc/` | `rticx-spsc` | `no_std` single-producer single-consumer queue used by the software tasks pass. |
| `rticx-async/` | `rticx-async` | `no_std` async runtime: `ExecSlot` future storage, `make_channel!` macro, MPSC channels, waker infrastructure.  |
| `compilation-passes/rticx-async-pass/` | `rticx-async-pass` | Async/Await software tasks pass: executors, message queues, `spawn`, `spawn_from`. |
| `compilation-passes/rticx-sw-pass/` | `rticx-sw-pass` | Vanilla software tasks pass: dispatchers, message queues, `spawn`, `spawn_from`. |
| `compilation-passes/rticx-auto-assign/` | `rticx-auto-assign` | Automatic `core = N` assignment based on shared resource usage. |
| `compilation-passes/rticx-deadline-pass/` | `rticx-deadline-pass` | Converts `deadline = D` attributes into RTICX priorities. |
| `tools/rticx-expand/` | `rticx-expand` | `cargo rticx-expand` subcommand: prints the complete expanded source (user code preserved) to stdout like `cargo expand` (`--merge` splices it into the source file for inspection and GDB stepping, `restore` reverts); `--expand-passes <dir>` snapshots the module after every pass for diffing. |
| `distributions/rticx-cortex-m/` | `rticx-cortex-m` | Single-core Cortex-M (armv6-m and armv7-m and above) distribution. |
| `distributions/rticx-riscv/` | `rticx-riscv` | Single-core riscv with generic SLIC interrupt controller/ esp32c3/ esp32c6 |
| `distributions/rticx-rp2040/` | `rticx-rp2040` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ distribution. |

## Supported distributions

| Distribution | Target | Link |
|--------------|--------|----------|
| `rticx-cortex-m` | Single-core Cortex-M (armv6-m and armv7-m and above) | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-cortex-m |
| `rticx-riscv` | Single-core riscv with generic SLIC interrupt controller/ esp32c3/ esp32c6 | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-riscv |
| `rticx-rp2040` | Raspberry Pi Pico / RP2040 (dual-core Cortex-M0+) | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-rp2040 |
| `rticx-hippo` | Single-core RISC-V Hippomenes MCU | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-hippo |
| `rticx-atalanta` | Single-core RISC-V Atalanta MCU | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-atalanta |

## Quick start

The fastest way to see the framework in action is the `rticx-cortex-m` QEMU playground, which exercises real Cortex-M core-peripheral
init (SysTick), a hardware task bound to the `SysTick` exception, and a software task on an NVIC dispatcher that acquires a shared resource
through RTIC's SRP `lock`.

```bash
# Prereqs: qemu-system-arm and the two Cortex-M Rust targets
sudo apt-get install -y qemu-system-arm
rustup target add thumbv7m-none-eabi thumbv6m-none-eabi

make qemu-armv7m
```

The examples are located in `distributions/rticx-cortex-m/example-apps`. You can modify them, rebuild and run on qemu:

## Examples

- [ARM Cortex-m playground: SysTick hw task + spawned sw task + SRP lock ](distributions/rticx-cortex-m/example-apps/armv7m-app/examples/hello_rtic.rs)
- [ARM Cortex-m playground: Async and Monotonics example](distributions/rticx-cortex-m/example-apps/armv7m-app/examples/async_ping_pong.rs)
- [ARM Cortex-m playground: Async Priority 0 tasks](distributions/rticx-cortex-m/example-apps/armv7m-app/examples/async_prio0.rs)
- [RISCV playground: Async Ping Pong](distributions/rticx-riscv/examples/esp32c3-examples/examples/async_ping_pong.rs)
- [Single binary multicore ping-pong](distributions/rticx-rp2040/example-apps/src/bin/ping_pong.rs)

## Academic Publications

- [Master thesis: Modular and Multicore RTIC](https://trepo.tuni.fi/bitstream/10024/162037/2/MadaouiZakaria.pdf)
- [Paper: Towards modularity of the Rust RTIC real-time scheduling framework](https://ieeexplore.ieee.org/document/10752441)
- [Paper: Modular RTIC: Lightweight Real Time for Customized Architectures](https://www.diva-portal.org/smash/get/diva2:1993122/FULLTEXT01.pdf)
- [Other publications](https://ltu.diva-portal.org/smash/resultList.jsf?aq2=%5B%5B%5D%5D&af=%5B%5D&searchType=SIMPLE&sortOrder2=title_sort_asc&query=RTIC&language=en&aq=%5B%5B%5D%5D&sf=all&aqe=%5B%5D&sortOrder=author_sort_asc&onlyFullText=false&noOfRows=50&dswid=8093)
