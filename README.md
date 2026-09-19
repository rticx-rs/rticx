# RTICX: eXtensible Realtime Interrupt Driven Concurrency Framework

[![crates.io](https://img.shields.io/crates/v/rticx-core)](https://crates.io/crates/rticx-core)
[![wiki](https://img.shields.io/badge/docs-wiki-red)](https://github.com/rticx-rs/rticx/wiki/)
[![CI](https://github.com/rticx-rs/rticx/actions/workflows/ci.yml/badge.svg)](https://github.com/rticx-rs/rticx/actions/workflows/ci.yml)
[![QEMU](https://github.com/rticx-rs/rticx/actions/workflows/qemu.yml/badge.svg)](https://github.com/rticx-rs/rticx/actions/workflows/qemu.yml)

This is a rewrite of the [RTIC framework](https://github.com/rtic-rs/rtic) with a focus on modularity, cleaner application syntax and ease of hardware ports. It supports most single-core Cortex-M and RISC-V MCUs as well as the dual-core RP2040.

## Supported distributions
The following distributions are maintained by the RTICX team/organization: 

| Distribution | Target | Link | Cargo |
|--------------|--------|----------|-----|
| `rticx-cortex-m` | Single-core Cortex-M (armv6-m, armv7-m and above) | https://github.com/rticx-rs/rticx/tree/main/distributions/rticx-cortex-m | `cargo add rticx-cortex-m` |
| `rticx-riscv` | Single-core RISC-V (generic SLIC interrupt controller, ESP32-C3, ESP32-C6) | https://github.com/rticx-rs/rticx-riscv | `cargo add rticx-riscv` |
| `rticx-rp2040` | Raspberry Pi Pico / RP2040 (dual-core Cortex-M0+) | https://github.com/rticx-rs/rticx-rp2040 | `cargo add rticx-rp2040` |

## A taste of RTICX

The RTIC syntax has been simplified:

```rust
#[rticx_cortex_m::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6])]
mod app {
    #[shared]
    struct Shared {
        counter: u32,
    }

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        /* start SysTick */
        (Shared { counter: 0 }, TaskInits { tick: Tick { ticks : 0 } })
    }

    /// Hardware task, runs on every SysTick interrupt
    #[task(binds = SysTick, priority = 1, shared = [counter])]
    struct Tick {
      ticks: u32;
    }
    impl RticTask for Tick {
        fn exec(&mut self) {
            self.ticks+=1;
            let _ = Worker::spawn(self.ticks);
        }
    }

    /// Async task, can await timers/channels, locks shared state deadlock-free
    #[async_task(priority = 2, shared = [counter], init = generated)]
    struct Worker;
    impl RticAsyncTask for Worker {
        type SpawnInput = u32;
        async fn exec(&mut self, ticks: u32) {
            self.shared().counter.lock(|c| *c += ticks);
        }
    }
}
```

See the framework in action in ~2 minutes, no hardware needed.

```bash
# Prereqs: qemu-system-arm and the two Cortex-M Rust targets
sudo apt-get install -y qemu-system-arm
rustup target add thumbv7m-none-eabi thumbv6m-none-eabi

make qemu-armv7m
```


## Why RTICX?

[RTIC](https://github.com/rtic-rs/rtic) is one of the best embedded Rust
frameworks offering exceptional guarantees such as **deadlock-free**
execution and blazing-fast scheduling thanks to SRP and hardware-offloaded
scheduling. However, its monolithic architecture is showing its limits and 
is becoming increasingly hard to contribute to, extend and maintain.

RTICX keeps the guarantees and breaks the monolith:

- **A small core** (`rticx-core`) captures only the SRP Tasks/Resources model, making it ideal for certifications and security reviews.
- **Compilation passes** are independent crates that transform and expand user
  syntax (software tasks, async/await, etc.) which users can opt-in/out from.
- **Distributions** are target-specific crates that implement backend traits,
  register the passes they want, and expose the final `#[app]` macro.

The payoff: new hardware ports (including **multicore**) and new syntax
features no longer require forking the framework or understanding its entire
codebase.

## Features

On top of the original RTIC framework, RTICX adds:

- **Single-binary multicore support**: extended syntax and hardware support for
  single-firmware multicore platforms like the **RP2040**.
- **Simplified, more idiomatic Rust syntax**: less magic, cleaner code, same
  functionality.
- **Choice of software task flavors**: RTICv1-style lightweight tasks (no
  async) or RTICv2-style async/await tasks.
- **Easier hardware ports and contributions**: new ports and syntax extensions
  don't require forking the framework nor fully understanding how it works.
  See the distributor guide in the [project wiki](https://github.com/rticx-rs/rticx/wiki/).
- **`rticx-expand`**: a debug tool that expands any RTICX application into
  fully executable source (for GDB debugging, security vetting, etc.).

And, just like the [original RTIC framework](https://github.com/rtic-rs/rtic):

- **Tasks** as the unit of concurrency: interrupt-driven (hardware tasks) or
  spawned on demand (lightweight software tasks, async/await tasks).
- **Message passing** between tasks at spawn time.
- **A timer queue**: async tasks can delay or schedule themselves for future
  execution, enabling periodic tasks.
- **Preemptive multitasking** through task priorities.
- **Data-race-free memory sharing** through fine-grained, priority-based
  critical sections.
- **Deadlock-free execution guaranteed at compile time**, stronger than
  [the standard `Mutex` abstraction](https://doc.rust-lang.org/std/sync/struct.Mutex.html).
- **Minimal scheduling overhead**: the hardware does the bulk of the scheduling.
- **Highly efficient memory usage**: all tasks share a single call stack, with
  no hard dependency on a dynamic memory allocator.
- **All Cortex-M devices supported** (BASEPRI on armv7+, source masking on
  armv6-m) and **most RISC-V microcontrollers** (any SLIC-based MCU, plus
  ESP32-C3 / ESP32-C6).
- A task model amenable to known WCET and scheduling analysis techniques.


## Documentation

Full user and distributor guides are available in the [project wiki](https://github.com/rticx-rs/rticx/wiki/).

### Creating apps

Pick a distribution for your target, add its `#[app]` macro to your
crate, and write tasks. The [User Guide](https://github.com/rticx-rs/rticx/wiki/User-Guide)
walks you through [getting started](https://github.com/rticx-rs/rticx/wiki/User-Guide-Getting-Started),
the full syntax, and debugging.


### Coming from RTICv2?

The [RTICv2 to RTICX Migration Guide](https://github.com/rticx-rs/rticx/blob/main/.opencode/skills/rticv2-to-rticx-migration/SKILL.md) doubles as a step by step guide and an LLM skill. Point your favorite LLM at it (or load it as the `rticv2-to-rticx-migration` skill) to have the bulk of the port done for you.

### Creating distributions & passes

Want to run RTICX on new hardware, or extend the syntax? Distributions and
compilation passes are ordinary crates, no fork needed. The
[Distributor Guide](https://github.com/rticx-rs/rticx/wiki/Distributor-Guide)
covers [writing distributions](https://github.com/rticx-rs/rticx/wiki/Distributor-Guide-Writing-Distributions)
and [writing compilation passes](https://github.com/rticx-rs/rticx/wiki/Distributor-Guide-Writing-Compilation-Passes).

## Architecture

See [Distributor Guide: Architecture](https://github.com/rticx-rs/rticx/wiki/Distributor-Guide-Architecture)

## Examples

- [ARM Cortex-M playground: SysTick hw task + spawned sw task + SRP lock](distributions/rticx-cortex-m/examples-apps/examples/hello_rtic.rs)
- [ARM Cortex-M playground: Async and Monotonics example](distributions/rticx-cortex-m/examples-apps/examples/async_ping_pong.rs)
- [ARM Cortex-M playground: Async Priority 0 tasks](distributions/rticx-cortex-m/examples-apps/examples/async_prio0.rs)
- [RISC-V playground: Async Ping Pong](https://github.com/rticx-rs/rticx-riscv/blob/main/examples/esp32c3-examples/examples/async_ping_pong.rs)
- [RP2040 multicore ping-pong](https://github.com/rticx-rs/rticx-rp2040/blob/main/example-apps/src/bin/ping_pong.rs)
- [RTICv2 examples migrated to RTICX](https://github.com/rticx-rs/rticx-examples)

## API compatibility and versioning

See [`COMPATIBILITY.md`](COMPATIBILITY.md).

## Experimental distributions (Research)
RTICX has been actively used in academic research since its early experimental days. Its modular architecture makes porting to new hardware and custom SoCs straightforward, and lets researchers experiment with exotic syntax extensions without modifying the core, or even fully understanding how it works.

| Distribution | Target | Link |
|--------------|--------|----------|
| `rticx-hippo` | Single-core RISC-V Hippomenes MCU | https://github.com/rticx-rs/rticx-hippo |
| `rticx-atalanta` | Single-core RISC-V Atalanta MCU | https://github.com/rticx-rs/rticx-atalanta |

## Acknowledgements

While RTICX is a from-scratch rewrite of RTIC's macro and core logic, several
parts of this repository and other repositories in this organization, notably the hardware exports and target backends in
the cortex-m and riscv distributions and all community examples, have been backported from the upstream [RTIC](https://github.com/rtic-rs/rtic)
codebase. Many thanks to the RTIC community; a large share of the credit for these parts goes to its maintainers and contributors.

## Academic Publications

- [Master thesis: Modular and Multicore RTIC](https://trepo.tuni.fi/bitstream/10024/162037/2/MadaouiZakaria.pdf)
- [Paper: Towards modularity of the Rust RTIC real-time scheduling framework](https://ieeexplore.ieee.org/document/10752441)
- [Paper: Modular RTIC: Lightweight Real Time for Customized Architectures](https://www.diva-portal.org/smash/get/diva2:1993122/FULLTEXT01.pdf)
- [Other publications](https://ltu.diva-portal.org/smash/resultList.jsf?aq2=%5B%5B%5D%5D&af=%5B%5D&searchType=SIMPLE&sortOrder2=title_sort_asc&query=RTIC&language=en&aq=%5B%5B%5D%5D&sf=all&aqe=%5B%5D&sortOrder=author_sort_asc&onlyFullText=false&noOfRows=50&dswid=8093)
