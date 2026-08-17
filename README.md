# RTICX: eXtensible Realtime Interrupt Driven Concurrency Framework

[![crates.io](https://img.shields.io/crates/v/rticx-core)](https://crates.io/crates/rticx-core)
[![wiki](https://img.shields.io/badge/docs-wiki-red)](https://github.com/rticx-rs/rticx/wiki/)
[![CI](https://github.com/rticx-rs/rticx/actions/workflows/ci.yml/badge.svg)](https://github.com/rticx-rs/rticx/actions/workflows/ci.yml)
[![QEMU](https://github.com/rticx-rs/rticx/actions/workflows/qemu.yml/badge.svg)](https://github.com/rticx-rs/rticx/actions/workflows/qemu.yml)


This is a from-scratch rewrite of the [original RTIC framework](https://github.com/rtic-rs/rtic).

## Motivation

[RTIC](https://github.com/rtic-rs/rtic) is arguably one of the best embedded
Rust frameworks out there, with exceptional guarantees such as **deadlock-free**
execution and blazing-fast scheduling thanks to SRP and hardware-offloaded
scheduling. However, its monolithic architecture is showing its limits and 
is becoming increasingly hard to extend and maintain.

RTIC being a framework that provides the majority of its
functionality through a Rust proc-macro is by itself a major learning curve
for any contributor. Furthermore, the amount of parsed, analyzed, validated,
and generated code is huge compared to other Rust proc-macros, which operate on
small pieces of code rather than the entire user application. To make matters
worse, the RTIC proc-macro has to emit hardware-specific code, so each new hardware
port adds more proc-macro logic. As a result, the RTIC codebase is growing
uncontrollably, the maintenance burden is becoming much higher, and
contributing requires a very thorough understanding of this complex codebase.
It also supports only single-core hardware and doesn't account for **multicore**
targets, which would enable a vast range of new applications.

## Goal

This project started as [a research project](#academic-publications)
with the goal of making RTIC more maintainable, extensible, and easily portable
to new hardware architectures (including multicore) in order to reduce the
barrier of entry for contributors and maintainers who wish to introduce new
syntax features and hardware ports.

The main idea is to break down RTIC's monolithic codebase by separating the
generic proc-macro logic (RTIC syntax) from target-specific details (interrupt
handling, system initialization, etc). Furthermore, the proc-macro logic is
split into core and addons: the core captures only the SRP Tasks/Resources
model, and everything else (software tasks, async/await, etc..) is implemented
as external addons.

The result is a small core framework (`rticx-core`) plus a growing ecosystem of
**compilation passes** and **distributions**:

- **Compilation passes** are independent crates that transform and expand user
  application syntax.
- **Distributions** are target-specific crates that implement backend traits,
  register the passes they want, and expose the final `#[<distro>::app]` macro.

In addition, the user application syntax (henceforth referred to as RTICX
syntax) has been refactored to provide less magic and a more idiomatic Rust
experience while preserving the core concepts of the original RTIC framework
(the Tasks and Resources model).

## Features

Just like the [original RTIC framework](https://github.com/rtic-rs/rtic), the following features are supported:

- **Tasks** as the unit of concurrency:
    - interrupt-driven (hardware tasks)
    - spawned on demand (lightweight software tasks or async/await tasks)

- **Message passing** between tasks at spawn time.

- **A timer queue**: async software tasks can delay or schedule themselves for future execution, enabling periodic tasks.

- **Preemptive multitasking** through task priorities.

- **Efficient and data race free memory sharing** through fine-grained *priority based* critical sections.

- **Deadlock free execution** guaranteed at compile time. This is a stronger guarantee than what's provided by [the standard `Mutex` abstraction](https://doc.rust-lang.org/std/sync/struct.Mutex.html)

- **Minimal scheduling overhead**. The task scheduler has minimal software footprint; the hardware does the bulk of the scheduling.

- **Highly efficient memory usage**: All the tasks share a single call stack and there's no hard dependency on a dynamic memory allocator.

- **All Cortex-M devices are supported** (BASEPRI on armv7+, source masking on
  armv6-m).

- **Most RISC-V microcontrollers are supported** (any SLIC-based MCU, plus
  ESP32-C3 / ESP32-C6).

- A task model amenable to known WCET (Worst Case Execution Time) analysis and scheduling analysis techniques.

On top of the original framework, RTICX adds:

- **Single-binary multicore support**: Extended Syntax and hardware support for single firmware multicore platforms like the **rp2040** 

- **Simplified, more idiomatic Rust syntax**: less magic, cleaner code, same functionality.

- **Choice of software task flavors**: RTICv1-style lightweight tasks (no
  async) or RTICv2-style async/await tasks.

- **Easier hardware ports and contributions**: new hardware ports and syntax extensions are easier than ever. The don't require forking the framework nor fully understanding how it works. See the distributor guide in the [project wiki](https://github.com/rticx-rs/rticx/wiki/).

- **`rticx-expand`**: a debug tool that expands any RTICX application into
  fully executable source (for GDB debugging, security vetting, etc.).

## Architecture

- **`rticx-core`** provides the:
    - parser for the simple Task/Resources model,
    - resource-ceiling analysis (SRP)
    - code generation for hardware tasks/resources/init/idle
    - foundation of multicore-support
    - `RticMacroBuilder`, `InfoBus` APIs for chaining passes and exchanging information.
    - Trait definitions for compilation passes and parsing and codegen helpers like `RticAttr`
- **Compilation passes** implement the `RticPass` trait and run before or after the core pass as pure syntax-to-syntax transformations.
- **Distributions** provide the low-level hardware bindings via the `CorePassBackend` trait (and optional pass-specific backends), select which passes to use, and re-export the generated `#[<distro>::app]` macro.

## Documentation

Full user and distributor guides are available in the [project wiki](https://github.com/rticx-rs/rticx/wiki/).

## Supported distributions (Maintained by RTICX team)

| Distribution | Target | Link |
|--------------|--------|----------|
| `rticx-cortex-m` | Single-core Cortex-M (armv6-m and armv7-m and above) | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-cortex-m |
| `rticx-riscv` | Single-core riscv with generic SLIC interrupt controller/ esp32c3/ esp32c6 | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-riscv |
| `rticx-rp2040` | Raspberry Pi Pico / RP2040 (dual-core Cortex-M0+) | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-rp2040 |

## Experimental distributions (Research)
RTICX has been actively used in academic research since its early experimental days. Its modular architecture makes porting to new hardware and custom SoCs straightforward, and lets researchers experiment with exotic syntax extensions without modifying the core, or even fully understanding how it works.

| Distribution | Target | Link |
|--------------|--------|----------|
| `rticx-hippo` | Single-core RISC-V Hippomenes MCU | https://github.com/rticx-rs/rticx-hippo |
| `rticx-atalanta` | Single-core RISC-V Atalanta MCU | https://github.com/rticx-rs/rticx-atalanta |

## Acknowledgements

While RTICX is a from-scratch rewrite of RTIC's macro and core logic, several
parts of this repository, notably the hardware exports and target backends in
the cortex-m and riscv distributions, have been backported from the upstream [RTIC](https://github.com/rtic-rs/rtic)
codebase. Many thanks to the RTIC community; a large share of the credit for
these parts goes to its maintainers and contributors.

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

The examples are located in `distributions/rticx-cortex-m/examples-apps`. You can modify them, rebuild and run on qemu:

## Examples

- [ARM Cortex-m playground: SysTick hw task + spawned sw task + SRP lock ](distributions/rticx-cortex-m/examples-apps/examples/hello_rtic.rs)
- [ARM Cortex-m playground: Async and Monotonics example](distributions/rticx-cortex-m/examples-apps/examples/async_ping_pong.rs)
- [ARM Cortex-m playground: Async Priority 0 tasks](distributions/rticx-cortex-m/examples-apps/examples/async_prio0.rs)
- [RISCV playground: Async Ping Pong](distributions/rticx-riscv/examples/esp32c3-examples/examples/async_ping_pong.rs)
- [RP2040 multicore ping-pong](distributions/rticx-rp2040/example-apps/src/bin/ping_pong.rs)

## Academic Publications

- [Master thesis: Modular and Multicore RTIC](https://trepo.tuni.fi/bitstream/10024/162037/2/MadaouiZakaria.pdf)
- [Paper: Towards modularity of the Rust RTIC real-time scheduling framework](https://ieeexplore.ieee.org/document/10752441)
- [Paper: Modular RTIC: Lightweight Real Time for Customized Architectures](https://www.diva-portal.org/smash/get/diva2:1993122/FULLTEXT01.pdf)
- [Other publications](https://ltu.diva-portal.org/smash/resultList.jsf?aq2=%5B%5B%5D%5D&af=%5B%5D&searchType=SIMPLE&sortOrder2=title_sort_asc&query=RTIC&language=en&aq=%5B%5B%5D%5D&sf=all&aqe=%5B%5D&sortOrder=author_sort_asc&onlyFullText=false&noOfRows=50&dswid=8093)
