# AGENTS.md — RTIC Modular Rewrite

This document is the primary orientation guide for AI coding agents and human contributors working on this repository.

---

## 1. Project Vision & Architecture Overview

This repository is a complete, modular rewrite of the RTIC (Real-Time Interrupt-driven Concurrency) framework for Rust. The rewrite separates the RTIC macro into reusable, target-agnostic pieces and keeps target-specific logic in pluggable **distributions**.

The central design is:

- **`rtic-core`** provides the core procedural macro logic: parsing the RTIC syntax, resource ceiling analysis (SRP model), and code generation for tasks, resources, `init`, and `idle`. It also exposes a **Builder API** that lets external compilation passes be chained before or after the core pass.
- **Compilation passes** are independent crates that transform the RTIC syntax. They are pure syntax-to-syntax transformations.
- **Distributions** are target-specific crates that:
  - implement the backend traits defined by `rtic-core` (and optionally by passes);
  - register the passes they want to use;
  - expose the final `#[<distro>::app]` attribute macro (e.g. `#[rp2040_rtic::app]`).

This architecture makes it easy to add new hardware targets, new scheduling passes, or new syntax extensions without touching the core framework.

### Important structural note

There is **no root `Cargo.toml`**. The repository is a collection of independent crates linked by relative path dependencies. Each crate must be built and tested separately.

---

## 2. Crate & Directory Map

### Core crates

| Crate | Path | Role |
|-------|------|------|
| `rtic-core` | `rtic-core/` | Core compilation pass, parser, analysis, codegen, and the `RticMacroBuilder` API. |
| `rtic-spsc` | `rtic-spsc/` | `no_std` single-producer single-consumer queue used by the software tasks pass. |

### Compilation passes

| Crate | Path | Role |
|-------|------|------|
| `rtic-sw-pass` | `compilation-passes/rtic-sw-pass/` | Software tasks pass: dispatchers, message queues, `spawn`, `spawn_from`. |
| `rtic-auto-assign` | `compilation-passes/rtic-auto-assign/` | Automatic `core = N` assignment for tasks based on shared resource usage. |
| `rtic-deadline-pass` | `compilation-passes/rtic-deadline-pass/` | Converts `deadline = D` attributes into RTIC priorities. |

### Distributions

| Distribution | Path | Target | Notes |
|--------------|------|--------|-------|
| `rp2040-rtic` | `distributions/rp2040-rtic/` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ | Single binary; starts core 1 from `post_init`. |
| `stm32-renode-rtic` | `distributions/stm32-renode-rtic/` | Renode-simulated multicore STM32F1C3-like | Multi-binary. |
| `rtic-hippo` | `distributions/rtic-hippo/` | Single-core RISC-V Hippomenes MCU | Uses threshold-based (`mintthresh`) locking. |
| `cortex-m-rtic` | `distributions/cortex-m-rtic/` | Single-core Cortex-M (armv6-m and armv7-m and above) | BASEPRI locking by default; `armv6m` feature switches to interrupt source masking. `swtasks` enabled by default. |
| `atalanta-rtic` | `distributions/atalanta-rtic/` | Single-core RISC-V Atalanta MCU (SoC-Hub) | Includes PCS (Parallel Context Stacking) support via `pcs-pass`. |
| `distribution-template` | `distributions/distribution-template/` | Reference/template to be copy-pasted when creating new distributions | N/A |


---

## 3. Core Abstractions & Key Traits

### `#[<distro>::app]` entry point

The attribute macro is **not** defined in `rtic-core`. Each distribution defines it in its own `*-macro` crate and re-exports it under the distribution name (e.g. `#[rp2040_rtic::app]`). For example:

- `distributions/rp2040-rtic/rp2040-rtic-macro/src/lib.rs` defines the proc macro.
- `distributions/rp2040-rtic/src/lib.rs` re-exports it as `pub use rp2040_rtic_macro::app;`.

### The `RticMacroBuilder` pipeline

`rtic-core` exposes `RticMacroBuilder` in `rtic-core/src/lib.rs`. A distribution constructs an instance, registers passes, and calls `build_rtic_macro` from its proc macro.

```rust
pub struct RticMacroBuilder {
    core: Box<dyn CorePassBackend>,
    pre_core_passes: Vec<Box<dyn RticPass>>,
    post_core_passes: Vec<Box<dyn RticPass>>,
}
```

Key methods:

| Method | Purpose |
|--------|---------|
| `new<T: CorePassBackend>(core_impl: T)` | Create a builder with the target-specific backend. |
| `bind_pre_core_pass<P: RticPass>(pass: P)` | Register a pass that runs before parsing and core codegen. |
| `bind_post_core_pass<P: RticPass>(pass: P)` | Register a pass that runs after parsing and core codegen. |
| `build_rtic_macro(self, args, input) -> TokenStream` | Execute the full pipeline and return the expanded code. |

Pipeline order inside `build_rtic_macro`:

1. Reset `DEFAULT_TASK_PRIORITY` from the backend.
2. Run `pre_core_passes` in insertion order.
3. Parse the module with `App::parse(args, app_mod)`.
4. Run `Analysis::run(&mut parsed_app)` for resource ceiling analysis.
5. Call `CorePassBackend::pre_codegen_validation`.
6. Run `CodeGen::new(core_backend, &parsed_app, &analysis).run()`.
7. If the `debug_expand` feature is enabled, write the expanded code to `examples/{binary_name}_expanded.rs`.

### The `RticPass` trait

Every compilation pass implements `RticPass` (in `rtic-core/src/lib.rs`):

```rust
pub trait RticPass {
    fn run(&self, args: TokenStream, app_mod: ItemMod) -> (TokenStream, ItemMod);
}
```

Passes receive the macro arguments and the annotated module, and return transformed versions. They are pure syntax-to-syntax transformations.

### `CorePassBackend`

`CorePassBackend` (in `rtic-core/src/backend.rs`) is the target-specific interface used by the core code generation phase.

Notable methods:

| Method | Purpose |
|--------|---------|
| `post_init(...)` | Code inserted after `init` and task initialization, before `idle`. Used to enable interrupts, wake secondary cores, etc. |
| `generate_resource_proxy_lock_impl(...)` | Fills the body of the `lock` function for each shared resource proxy. |
| `generate_global_definitions(...)` | Extra constants, `use` statements, or helper functions at global scope. |
| `wrap_task_execution(...)` | Wrap the task `exec` call inside an interrupt handler. |
| `entry_name(core: u32) -> Ident` | Name of the entry function for each core. |
| `populate_idle_loop()` | Custom body for the default idle loop. |
| `generate_interrupt_free_fn(...)` | Implements the global critical-section function. |
| `pre_codegen_validation(...)` | Target-specific validation before codegen. |
| `default_task_priority() -> u16` | Fallback priority when the user omits one. |
| `entry_attrs() -> Vec<Attribute>` | Attributes injected onto entry points (e.g., `#[riscv_rt::entry]`). |
| `task_attrs() -> Vec<Attribute>` | Attributes injected onto task interrupt handlers. |

### `SwPassBackend`

`SwPassBackend` (in `compilation-passes/rtic-sw-pass/src/software_pass/mod.rs`) is the backend extension for the software tasks pass.

Required methods:

| Method | Purpose |
|--------|---------|
| `queue_path(&self) -> syn::Path` | Path to the SPSC queue type used by the software-tasks pass (typically `<distro>::export::Queue`). |
| `generate_local_pend_fn(&self, core: u32, empty_body_fn: ItemFn) -> ItemFn` | Fill the per-core core-local interrupt-pending function used by `spawn`. |
| `generate_cross_pend_fn(&self, core: u32, empty_body_fn: ItemFn) -> Option<ItemFn>` | Fill the per-target-core cross-core interrupt-pending function used by `spawn_from`. Returns `None` on single-core targets. |

Default method:

| Method | Purpose |
|--------|---------|
| `custom_interrupt_path(&self, core: u32) -> Option<syn::Path>` | Path to the concrete dispatcher interrupt type. Defaults to `pac[core]::Interrupt`; return a custom path if the PAC's enum is not at the default location or if the target exposes interrupts differently (e.g. an enum re-export or module). |

### Syntax attributes

Core RTIC syntax attributes are parsed in `rtic-core/src/parser/ast.rs`:

- `#[app(device = path, cores = N, dispatchers = [...])]` — single PAC.
- `#[app(cores = N)]` — number of cores (default 1).
- `#[app(dispatchers = [irq0, irq1, ...])]` — single-core dispatchers.
- `#[app(dispatchers = [[irq0], [irq1], ...])]` — per-core dispatchers (used by `rtic-sw-pass`).
- `#[task(binds = IRQ, priority = N, shared = [...], core = N)]` — hardware or software task.
- `#[shared(core = N)]` — shared resource struct.
- `#[init(core = N)]` — initialization task.
- `#[idle(core = N)]` — idle task.
- `#[task(..., task_trait = CustomTrait)]` — allows a pass to plug in a custom task trait.

Software-task specific attributes are parsed in `compilation-passes/rtic-sw-pass/src/software_pass/parse/ast.rs`:

- `#[sw_task(priority = N, shared = [...], core = N, spawn_by = M)]`
- `spawn_by = M` controls which core may spawn this task.

Auto-assign and deadline passes read:

- `#[task(core = N)]` / `#[sw_task(core = N)]` — explicit core assignment.
- `#[task(deadline = D)]` / `#[sw_task(deadline = D)]` — deadline for priority conversion.

### Feature flags

| Crate | Feature | Effect |
|-------|---------|--------|
| `rtic-core` | `debug_expand` | Writes expanded code to `examples/{binary_name}_expanded.rs`. |
| `rp2040-rtic` | `autoassign` | Enables `rtic-auto-assign`. |
| `rp2040-rtic` | `swtasks` | Enables `rtic-sw-pass`. |
| `cortex-m-rtic` | `swtasks` | Enables `rtic-sw-pass` (on by default). |
| `cortex-m-rtic` | `armv6m` | Selects interrupt source-masking locking (Cortex-M0/M0+/M23). When disabled (default), BASEPRI-based locking is used (armv7-m and above). |
| `rtic-hippo` | `deadline-pass` | Enables `rtic-deadline-pass`. |
| `atalanta-rtic` | `deadline-pass` | Enables `rtic-deadline-pass`. |
| `atalanta-rtic` | `pcs-pass` | Enables the PCS pass. |

---

## 4. Development Workflow & Testing Commands

### Building and testing individual crates

Test the SPSC queue crate
```bash
cd rtic-spsc && cargo test
```

Test rtic-core integration tests using the mock backend
```bash
cd rtic-core

cargo test
```

Build a pass crate
```bash
cd compilation-passes/rtic-sw-pass && cargo build
```

### Building distribution examples

```bash
# RP2040 single-core example
cd distributions/rp2040-rtic
cargo build --example hello_rtic

# RP2040 multicore ping-pong example
cargo build --example ping_pong

# Cortex-M (armv7-m / BASEPRI) example
cd distributions/cortex-m-rtic/example-apps/armv7m-app
cargo build --example hello_rtic

# Cortex-M (armv6-m / source masking) example
cd distributions/cortex-m-rtic/example-apps/armv6m-app
cargo build --example hello_rtic
```

### Running cortex-m-rtic examples under QEMU

The `cortex-m-rtic` examples are runnable under QEMU's `lm3s6965evb` (Cortex-M3)
machine. Each example configures the SysTick core timer, spawns a software task
on every tick, acquires a shared resource through RTIC's SRP `lock`, and once
the counter reaches the target calls `debug::exit(EXIT_SUCCESS)` from
`cortex-m-semihosting`, terminating QEMU with exit code 0 — usable as a CI
pass/fail gate.

From the repository root:

```bash
make qemu
```

### Multi-binary builds for `stm32-renode-rtic`

WIP: unsupported at the moment

### Documentation

If a documentation generation script exists in the root, run it with:

```bash
./gen_doc.sh
```

---

## 5. Guide for Common Tasks

### How to add a new compilation pass

1. Create a new crate under `compilation-passes/<your-pass>/`.
2. Implement the `RticPass` trait from `rtic-core`:
   ```rust
   impl RticPass for YourPass {
       fn run(&self, args: TokenStream, app_mod: ItemMod) -> (TokenStream, ItemMod) {
           // transform and return
       }
   }
   ```
3. Decide whether the pass must run before the core pass (syntax transformation) or after the core pass (codegen augmentation). Register it with `bind_pre_core_pass` or `bind_post_core_pass` in the distribution macro crate.
4. If the pass needs target-specific hooks, define a new backend trait and implement it in the distributions that use the pass.
5. Add a feature flag in the distribution crate and gate the pass registration.

### How to create a new RTIC distribution

1. Copy the **template** distribution `distributions/distribution-template` and rename to `<your-distro>`:
   - `<your-distro>/` — the library crate user applications depend on.
   - `<your-distro>-macro/` — the proc-macro crate defining `#[<your-distro>::app]`.
2. Implement `CorePassBackend` in the macro crate for your target.
3. If you use software tasks, implement `SwPassBackend` (including the mandatory `queue_path()` method) and any other compilation-pass backends you enable.
4. In the macro crate, instantiate `RticMacroBuilder` and register the passes you want:
   ```rust
   let mut builder = RticMacroBuilder::new(MyBackend);
   builder.bind_pre_core_pass(SoftwarePass::new(MySwBackend));
   builder.bind_pre_core_pass(AutoAssignPass); // for multicore only
   let tokens = builder.build_rtic_macro(args, input);
   ```
5. Re-export the macro from the library crate as `pub use <your-distro>_macro::app;`.
6. Add an `export` module in the library crate that re-exports target-specific runtime helpers, `cortex-m` / `riscv` items, and any pass exports. Make sure to expose a queue type at `<your-distro>::export::Queue` unless the backend's `queue_path()` points elsewhere.
7. Add example apps under `<your-distro>/examples/` or `<your-distro>/example-apps/`.

---

*Last oriented: 2026-07-25*
