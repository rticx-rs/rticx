# MMRTIC: Multicore, Distributions and Compilation Passes

This repository contains a from scratch rewrite of the RTIC (Real-Time Interrupt-driven Concurrency) framework for Rust. The goal is to make RTIC more maintainable, extensible, and portable by separating the generic proc-macro logic from target-specific details and by allowing new language features to be added as external, reusable compilation passes.

The result is a small core framework (`rtic-core`) plus a growing ecosystem of **compilation passes** and **distributions**:

- **Compilation passes** are independent crates that transform RTIC syntax.
- **Distributions** are target-specific crates that implement backend traits, register the passes they want, and expose the final `#[rtic::app]` macro.

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
| `compilation_passes/rtic-sw-pass/` | `rtic-sw-pass` | Software tasks pass: dispatchers, message queues, `spawn`, `spawn_from`. |
| `compilation_passes/rtic-auto-assign/` | `rtic-auto-assign` | Automatic `core = N` assignment based on shared resource usage. |
| `compilation_passes/rtic-deadline-pass/` | `rtic-deadline-pass` | Converts `deadline = D` attributes into RTIC priorities. |
| `distributions/rp2040-rtic/` | `rp2040-rtic` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ distribution. |
| `distributions/stm32-renode-rtic/` | `stm32-renode-rtic` | Renode-simulated multicore STM32F1C3-like distribution. |
| `distributions/rtic-hippo/` | `rtic-hippo` | Single-core RISC-V Hippomenes MCU distribution. |
| `distributions/atalanta-rtic/` | `atalanta-rtic` | Single-core RISC-V Atalanta MCU distribution. |
| `distributions/distribution-template/` | `distribution-template` | Conceptual starting point for new distributions. |
| `compilation-tests/` | `compilation-tests` | Embedded example apps and comparison baselines. |
| `microamp_experimental/` | `microamp_experimental` | μAMP (asymmetric multiprocessing) shared-memory support. |

## Supported distributions

| Distribution | Target | Features |
|--------------|--------|----------|
| `rp2040-rtic` | Raspberry Pi Pico / RP2040 (dual-core Cortex-M0+) | `autoassign`, `swtasks` |
| `stm32-renode-rtic` | Renode-simulated multicore STM32F1C3-like | `multibin` |
| `rtic-hippo` | Single-core RISC-V Hippomenes MCU | `deadline-pass` |
| `atalanta-rtic` | Single-core RISC-V Atalanta MCU | `deadline-pass` |

## Quick start

There is no root `Cargo.toml`; each crate is built independently. The fastest way to see the framework in action is to build one of the `rp2040-rtic` examples:

```bash
cd distributions/rp2040-rtic
cargo build --example hello_rtic
cargo build --example ping_pong
```

## Examples

- [Single-core RTIC application with software tasks](distributions/rp2040-rtic/examples/hello_rtic.rs)
- [Multicore ping-pong with cross-core communication](distributions/rp2040-rtic/examples/ping_pong.rs)

## Documentation

Full user and distributor guides are available in the [project wiki](WIKI_URL_PLACEHOLDER).

## Academic Publications

- [Master thesis: Modular and Multicore RTIC](https://trepo.tuni.fi/bitstream/10024/162037/2/MadaouiZakaria.pdf)
- [Paper: Towards modularity of the Rust RTIC real-time scheduling framework](https://ieeexplore.ieee.org/document/10752441)
- [Paper: Modular RTIC: Lightweight Real Time for Customized Architectures](https://www.diva-portal.org/smash/get/diva2:1993122/FULLTEXT01.pdf)
