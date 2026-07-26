# AGENTS.md — RTIC Modular Rewrite

This document is the primary orientation guide for AI coding agents and human contributors working on this repository.

---

## 1. Project Vision & Architecture Overview

This repository is a complete, modular rewrite of the RTIC (Real-Time Interrupt-driven Concurrency) framework. The rewrite separates the original RTIC proc-macro into reusable, target-agnostic pieces and keeps target-specific logic in pluggable **distributions**.

The central design is:

- **`rticx-core`** provides the core procedural macro logic: parsing the RTICX syntax, resource ceiling analysis (SRP model), and code generation for tasks, resources, `init`, and `idle`. It also exposes a **Builder API** that lets external compilation passes be chained before or after the core pass.
- **Compilation passes** are independent crates that transform the RTICX syntax. They are pure syntax-to-syntax transformations.
- **Distributions** are target-specific crates that:
  - implement the backend traits defined by `rticx-core` (and optionally by passes);
  - register the passes they want to use;
  - expose the final `#[<distro>::app]` attribute macro (e.g. `#[rticx_rp2040::app]`).

This architecture makes it easy to add new hardware targets, new scheduling passes, or new syntax extensions without touching the core framework.

### Important structural note

There is **no root `Cargo.toml`**. The repository is a collection of independent crates linked by relative path dependencies. Each crate must be built and tested separately.

---

## 2. Crate & Directory Map

### Core crates

| Crate | Path | Role |
|-------|------|------|
| `rticx-core` | `rticx-core/` | Core compilation pass, parser, analysis, codegen, and the `RticMacroBuilder` API. |
| `rticx-spsc` | `rticx-spsc/` | `no_std` single-producer single-consumer queue used by the software tasks pass. |

### Compilation passes

| Crate | Path | Role |
|-------|------|------|
| `rticx-sw-pass` | `compilation-passes/rticx-sw-pass/` | Software tasks pass: dispatchers, message queues, `spawn`, `spawn_from`. |
| `rticx-auto-assign` | `compilation-passes/rticx-auto-assign/` | Automatic `core = N` assignment for tasks based on shared resource usage. |
| `rticx-deadline-pass` | `compilation-passes/rticx-deadline-pass/` | Converts `deadline = D` attributes into RTICX priorities. |

### Distributions

| Distribution | Path | Target | Notes |
|--------------|------|--------|-------|
| `rticx-rp2040` | `distributions/rticx-rp2040/` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ | Single binary; starts core 1 from `post_init`. |
| `rticx-stm32-renode` | `distributions/rticx-stm32-renode/` | Renode-simulated multicore STM32F1C3-like | Multi-binary. |
| `rticx-hippo` | `distributions/rticx-hippo/` | Single-core RISC-V Hippomenes MCU | Uses threshold-based (`mintthresh`) locking. |
| `rticx-cortex-m` | `distributions/rticx-cortex-m/` | Single-core Cortex-M (armv6-m and armv7-m and above) | BASEPRI locking by default; `armv6m` feature switches to interrupt source masking. `swtasks` enabled by default. |
| `rticx-riscv` | `distributions/rticx-riscv/` | Single-core riscv with generic SLIC interrupt controller/ esp32c3/ esp32c6 | See README.md of the distro |
| `rticx-atalanta` | `distributions/rticx-atalanta/` | Single-core RISC-V Atalanta MCU (SoC-Hub) | Includes PCS (Parallel Context Stacking) support via `pcs-pass`. |
| `distribution-template` | `distributions/distribution-template/` | Reference/template to be copy-pasted when creating new distributions | N/A |


---

## 3. Core Abstractions & Key Traits

### `#[<distro>::app]` entry point

The attribute macro is **not** defined in `rticx-core`. Each distribution defines it in its own `*-macro` crate and re-exports it under the distribution name (e.g. `#[rticx_rp2040::app]`). For example:

- `distributions/rticx-rp2040/rticx-rp2040-macro/src/lib.rs` defines the proc macro.
- `distributions/rticx-rp2040/src/lib.rs` re-exports it as `pub use rticx_rp2040_macro::app;`.

### The `RticMacroBuilder` pipeline

`rticx-core` exposes `RticMacroBuilder` in `rticx-core/src/lib.rs`. A distribution constructs an instance, registers passes, and calls `build_rtic_macro` from its proc macro.

```rust
pub struct RticMacroBuilder {
    core: Box<dyn CorePassBackend>,
    pre_std_passes: Vec<Box<dyn RticPass>>,
    info_bus: InfoBus,
}
```

Key methods:

| Method | Purpose |
|--------|---------|
| `new<T: CorePassBackend + 'static>(core_impl: T)` | Create a builder with the target-specific backend. Owns a fresh `InfoBus`. |
| `bind_pre_core_pass<P: RticPass + 'static>(pass: P)` | Register a pass that runs before parsing and core codegen. |
| `build_rtic_macro(self, args, input) -> TokenStream` | Execute the full pipeline and return the expanded code. |
| `build_rtic_macro2(self, args, app_mod) -> TokenStream2` | Same as `build_rtic_macro` but on `proc_macro2` types, for tests/tooling. |
| `info_bus(&self) -> &InfoBus` | Read access to the builder's shared `InfoBus`. |

Pipeline order inside `build_rtic_macro2` (the proc-macro entry reuses it via `TokenStream` conversion):

1. Reset `DEFAULT_TASK_PRIORITY` from the backend.
2. Call `core.subscribe(info_bus.clone())` — the target backend receives the `InfoBus` before anyone else.
3. For each **pre-core pass** in insertion order:
   1. Call `pass.subscribe(info_bus.clone())` (guaranteed to happen before the pass's other trait methods).
   2. Call `pass.run_pass(args, app_mod) -> syn::Result<(TokenStream2, ItemMod)>`; on error, emit a compile error mentioning `pass.pass_name()`.
4. Parse the module with `App::parse(args, app_mod)`.
5. Publish the parsed app to the `InfoBus` under the key `rticx_core::App`.
6. Run `Analysis::run(&mut parsed_app)` for resource ceiling analysis.
7. Publish the analysis to the `InfoBus` under the key `rticx_core::Analysis`.
8. Call `CorePassBackend::pre_codegen_validation`.
9. Run `CodeGen::new(core_backend, &parsed_app, &analysis).run()`.
10. If the `debug_expand` feature is enabled, write the expanded code to `examples/{binary_name}_expanded.rs`.

> Note: there is no `bind_post_core_pass` anymore — only **pre-core** passes are supported. Passes that need to react after the core codegen must run as the last pre-core pass and inspect the `InfoBus` entries published by the core (e.g. `rticx_core::App`, `rticx_core::Analysis`).

### The `RticPass` trait

Every compilation pass implements `RticPass` (in `rticx-core/src/lib.rs`):

```rust
pub trait RticPass {
    /// Subscribe to the information bus. Guaranteed to be called before
    /// any other method in this trait.
    fn subscribe(&mut self, info_bus: InfoBus);

    /// Runs the (partial) proc-macro logic that extends the basic RTIC syntax.
    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;

    /// Human-readable name/alias used to identify the pass in errors.
    fn pass_name(&self) -> &str;
}
```

Passes receive the macro arguments and the annotated module, and return transformed versions. They are pure syntax-to-syntax transformations. `subscribe` is the only place where a pass can obtain a (clonable) handle to the shared `InfoBus`.

### `CorePassBackend`

`CorePassBackend` (in `rticx-core/src/backend.rs`) is the target-specific interface used by the core code generation phase.

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
| `subscribe(&mut self, _info_bus: InfoBus)` | Default no-op. Called once before any other method, giving the backend a handle to the shared `InfoBus`. |
| `default_task_priority() -> u16` | Fallback priority when the user omits one. |
| `entry_attrs() -> Vec<Attribute>` | Attributes injected onto entry points (e.g., `#[riscv_rt::entry]`). |
| `task_attrs() -> Vec<Attribute>` | Attributes injected onto task interrupt handlers. |

### `SwPassBackend`

`SwPassBackend` (in `compilation-passes/rticx-sw-pass/src/software_pass/mod.rs`) is the backend extension for the software tasks pass.

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
| `subscribe(&mut self, _info_bus: InfoBus)` | Default no-op. Called once before any other method. Passes that wrap a backend forward the bus: `SoftwarePass::subscribe` clones the `InfoBus`, stores it, and calls `self.backend.subscribe(info_bus)`. |

### `InfoBus`

`InfoBus` (in `rticx-core/src/info_bus.rs`, re-exported from `rticx-core`) is the shared information bus that lets compilation passes and backends exchange typed data during a single macro expansion. The `RticMacroBuilder` owns the bus and hands clones to the core backend and each pre-core pass via their `subscribe` methods (see the pipeline above).

```rust
#[derive(Clone)]
pub struct InfoBus {
    infos: Arc<Mutex<HashMap<String, Rc<dyn Any>>>>,
}
```

Key API:

| Method | Purpose |
|--------|---------|
| `InfoBus::new()` | `pub(crate)` — only `RticMacroBuilder` can construct a bus. |
| `publish<T: Any>(&self, entry: impl ToString, value: T) -> Result<(), errors::Error>` | Store a typed value under a string key. Returns `EntryOccupied` if the key already exists (entries are write-once). |
| `get<T: 'static>(&self, entry: &str) -> Result<Rc<T>, errors::Error>` | Retrieve and downcast a value. Returns `EntryNotFound` if missing or `InvalidTargetType` if the stored type does not match `T`. |

Conventions:

- `InfoBus` is `Clone` — every clone shares the same underlying `Arc`, so a value published through one handle is visible to all clones.
- Entry keys are **namespaced by the publishing crate and the type name**: `crate_name::TypeName`. The core pass publishes `rticx_core::App` and `rticx_core::Analysis`; the software-tasks pass publishes `rticx_sw_pass::App` and `rticx_sw_pass::Analysis` (exported as the constants `INFO_APP` / `INFO_ANALYSIS`).
- Entries are **write-once**: a second `publish` to an existing key is an error. This stops passes from silently overwriting each other's data.
- Subscribe ordering matters: the core backend is subscribed first, then each pre-core pass in insertion order, **before** its `run_pass` is invoked. So a later pass can `get` entries published by an earlier pass (or by the core, if it ran last), but the reverse is not true.

Error variants live in `rticx-core/src/errors.rs`: `EntryOccupied`, `EntryNotFound`, `InvalidTargetType`.

### Syntax attributes

Core RTICX syntax attributes are parsed in `rticx-core/src/parser/ast.rs`:

- `#[app(device = path, cores = N, dispatchers = [...])]` — single PAC.
- `#[app(cores = N)]` — number of cores (default 1).
- `#[app(dispatchers = [irq0, irq1, ...])]` — single-core dispatchers.
- `#[app(dispatchers = [[irq0], [irq1], ...])]` — per-core dispatchers (used by `rticx-sw-pass`).
- `#[task(binds = IRQ, priority = N, shared = [...], core = N)]` — hardware or software task.
- `#[shared(core = N)]` — shared resource struct.
- `#[init(core = N)]` — initialization task.
- `#[idle(core = N)]` — idle task.
- `#[task(..., task_trait = CustomTrait)]` — allows a pass to plug in a custom task trait.

Software-task specific attributes are parsed in `compilation-passes/rticx-sw-pass/src/software_pass/parse/ast.rs`:

- `#[sw_task(priority = N, shared = [...], core = N, spawn_by = M)]`
- `spawn_by = M` controls which core may spawn this task.

Auto-assign and deadline passes read:

- `#[task(core = N)]` / `#[sw_task(core = N)]` — explicit core assignment.
- `#[task(deadline = D)]` / `#[sw_task(deadline = D)]` — deadline for priority conversion.

### Feature flags

| Crate | Feature | Effect |
|-------|---------|--------|
| `rticx-core` | `debug_expand` | Writes expanded code to `examples/{binary_name}_expanded.rs`. |
| `rticx-rp2040` | `autoassign` | Enables `rticx-auto-assign`. |
| `rticx-rp2040` | `swtasks` | Enables `rticx-sw-pass`. |
| `rticx-cortex-m` | `swtasks` | Enables `rticx-sw-pass` (on by default). |
| `rticx-cortex-m` | `armv6m` | Selects interrupt source-masking locking (Cortex-M0/M0+/M23). When disabled (default), BASEPRI-based locking is used (armv7-m and above). |
| `rticx-hippo` | `deadline-pass` | Enables `rticx-deadline-pass`. |
| `rticx-atalanta` | `deadline-pass` | Enables `rticx-deadline-pass`. |
| `rticx-atalanta` | `pcs-pass` | Enables the PCS pass. |

---

## 4. Development Workflow & Testing Commands

### Building and testing individual crates

Test the SPSC queue crate
```bash
cd rticx-spsc && cargo test
```

Test rticx-core integration tests using the mock backend
```bash
cd rticx-core

cargo test
```

Build a pass crate
```bash
cd compilation-passes/rticx-sw-pass && cargo build
```

### Building distribution examples

```bash
# RP2040 single-core example
cd distributions/rticx-rp2040
cargo build --example hello_rtic

# RP2040 multicore ping-pong example
cargo build --example ping_pong

# Cortex-M (armv7-m / BASEPRI) example
cd distributions/rticx-cortex-m/example-apps/armv7m-app
cargo build --example hello_rtic

# Cortex-M (armv6-m / source masking) example
cd distributions/rticx-cortex-m/example-apps/armv6m-app
cargo build --example hello_rtic
```

### Running rticx-cortex-m examples under QEMU

The `rticx-cortex-m` examples are runnable under QEMU's `lm3s6965evb` (Cortex-M3)
machine. Each example configures the SysTick core timer, spawns a software task
on every tick, acquires a shared resource through RTIC's SRP `lock`, and once
the counter reaches the target calls `debug::exit(EXIT_SUCCESS)` from
`cortex-m-semihosting`, terminating QEMU with exit code 0 — usable as a CI
pass/fail gate.

From the repository root:

```bash
make qemu
```

### Multi-binary builds for `rticx-stm32-renode`

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
2. Implement the `RticPass` trait from `rticx-core`:
   ```rust
   impl RticPass for YourPass {
       fn subscribe(&mut self, info_bus: InfoBus) {
           // store a clone if you need to publish/read data later
       }

       fn run_pass(
           &self,
           args: TokenStream2,
           app_mod: ItemMod,
       ) -> syn::Result<(TokenStream2, ItemMod)> {
           // transform and return
           Ok((args, app_mod))
       }

       fn pass_name(&self) -> &str { "your-pass" }
   }
   ```
3. Passes are registered as **pre-core** passes with `bind_pre_core_pass` in the distribution macro crate. (There is no post-core pass registration anymore — see the `InfoBus` subsection for how to react to core-published data.)
4. If the pass needs target-specific hooks, define a new backend trait and implement it in the distributions that use the pass.
5. Add a feature flag in the distribution crate and gate the pass registration.

### How to create a new RTICX distribution

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

*Last oriented: 2026-07-26*
