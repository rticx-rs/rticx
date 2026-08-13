# RTICX Backend Reference

Detailed API reference for writing compilation passes and distribution backends.

Prerequisite: run `git submodule update --init` from the repo root to access
the wiki at `wiki/`. The wiki contains procedural guides and architecture context that complement this trait-level reference.

---

## Pipeline Order

Inside `RticMacroBuilder::build_rtic_macro2`:

1. Call `core.subscribe(info_bus.clone())` — the target backend receives the `InfoBus` before anyone else.
2. For each **pre-core pass** in insertion order:
   1. Call `pass.subscribe(info_bus.clone())` (guaranteed before `run_pass`).
   2. Call `pass.run_pass(args, app_mod) -> syn::Result<(TokenStream2, ItemMod)>`; on error, emit a compile error mentioning `pass.pass_name()`.
3. Parse the module with `App::parse(args, app_mod)`.
4. Publish the parsed app to the `InfoBus` under `rticx_core::App`.
5. Run `Analysis::run(&mut parsed_app)` for resource ceiling analysis.
6. Publish the analysis to the `InfoBus` under `rticx_core::Analysis`.
7. Call `CorePassBackend::pre_codegen_validation`.
8. Collect injections from all passes by calling `pass.main_injection(&point)` for each `MainInjectionPoint`.
9. Run `CodeGen::new(core_backend, &parsed_app, &analysis).with_injections(&injections).run()`.
10. If `debug_expand` is enabled, write expanded code to disk.

> Only **pre-core** passes are supported. Passes that need to react after core codegen can take the final TokenStream emitted by build_rti_macro() and make further changes.

---

## The `RticPass` Trait

Every compilation pass implements `RticPass` (defined in `rticx-core/src/lib.rs`):

```rust
pub trait RticPass {
    fn subscribe(&mut self, info_bus: InfoBus);

    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;

    fn pass_name(&self) -> &str;

    fn main_injection(&self, _point: &MainInjectionPoint) -> Option<TokenStream2> {
        None
    }
}
```

Passes receive the macro arguments and the annotated module, and return transformed versions. They are pure syntax-to-syntax transformations. `subscribe` is the only place where a pass can obtain a (clonable) handle to the shared `InfoBus`.

---

## `MainInjectionPoint`

```rust
pub enum MainInjectionPoint {
    BeforeInit,      // inside interrupt_free, before system_init
    BeforePostInit,  // inside interrupt_free, before post_init
    BeforeIdle,      // after interrupt_free, before idle loop
    ...
}
```

Passes use `main_injection` to inject code (e.g. variable declarations) directly into `main()`'s body. Since `main() -> !`, injected locals live forever on the stack.
---

## `CorePassBackend` Trait

Defined and fully documented in `rticx-core/src/backend.rs`. The target-specific interface used by the core code generation phase. 

## `SwPassBackend` Trait

Defined and fully documented in `compilation-passes/rticx-sw-pass/src/software_pass/mod.rs`:

## `AsyncPassBackend` Trait

Defined and fully documented in `compilation-passes/rticx-async-pass/src/lib.rs`:
---

## `InfoBus` API

`InfoBus` (in `rticx-core/src/info_bus.rs`, re-exported from `rticx-core`) is the shared information bus for typed data exchange during a single macro expansion. The `RticMacroBuilder` owns the bus and hands clones to backends and passes via `subscribe`.

| Method | Purpose |
|--------|---------|
| `publish<T: Any>(&self, entry: impl ToString, value: T) -> Result<(), Error>` | Store a typed value under a string key. Returns `EntryOccupied` if key already exists (entries are write-once). |
| `get<T: 'static>(&self, entry: &str) -> Result<Rc<T>, Error>` | Retrieve and downcast. Returns `EntryNotFound` if missing or `InvalidTargetType` if type mismatch. |

**Conventions:**

- `InfoBus` is `Clone` — every clone shares the same underlying `Arc`.
- Entry keys are namespace-prefixed: `crate_name::TypeName`. The core pass publishes `rticx_core::App` and `rticx_core::Analysis`; the software-tasks pass publishes `rticx_sw_pass::App` and `rticx_sw_pass::Analysis` (exported as constants `INFO_APP` / `INFO_ANALYSIS`).
- Entries are **write-once**: a second `publish` to an existing key is an error.
- Subscribe ordering: core backend first, then each pre-core pass in insertion order, **before** its `run_pass` is invoked.

Error variants: see `rticx-core/src/errors.rs`.

---

## Syntax Attributes

### Core (parsed in `rticx-core/src/parser/ast.rs`)

| Attribute | Description |
|-----------|-------------|
| `#[app(device = path)]` | Single PAC crate path. |
| `#[app(cores = N)]` | Number of cores (default 1). |
| `#[app(dispatchers = [irq0, ...])]` | Single-core dispatchers. |
| `#[app(dispatchers = [[irq0], [irq1], ...])]` | Per-core dispatchers (multi-core). |
| `#[task(binds = IRQ, priority = N, shared = [...], core = N)]` | Hardware or software task. |
| `#[shared(core = N)]` | Shared resource struct. |
| `#[init(core = N)]` | Initialization task. |
| `#[idle(core = N)]` | Idle task. |
| `#[task(..., task_trait = CustomTrait)]` | Custom task trait override. |

### Software-task pass (`rticx-sw-pass`)

| Attribute | Description |
|-----------|-------------|
| `#[sw_task(priority = N, shared = [...], core = N, spawn_by = M)]` | Software task. `spawn_by` controls which core may spawn this task. |

### Async-task pass (`rticx-async-pass`)

| Attribute | Description |
|-----------|-------------|
| `#[async_task(priority = N, core = N, spawn_by = M)]` | Async software task. Priority 0 runs on the idle executor. |

### Other passes

| Attribute | Used by |
|-----------|---------|
| `#[task(core = N)]` | `rticx-auto-assign` (reads/writes explicit core assignment) |
| `#[task(deadline = D)]` | `rticx-deadline-pass` (converts to priority) |

---

## Distribution Architecture Pattern

Every distribution follows the same structure:

```
distributions/<name>/
├── Cargo.toml           # Library crate: re-exports the proc-macro, provides export module
├── src/
│   ├── lib.rs           # pub use <name>_macro::app; compile_error! guards
│   └── export/
│       └── mod.rs       # Re-exports: rticx_sw_pass::export::*, target-specific runtime fns, async_rt
├── rticx-macro/
│   ├── Cargo.toml       # proc-macro = true crate
│   └── src/
│       └── lib.rs       # BackendImpl (CorePassBackend), SwBackendImpl, AsyncPassBackendImpl, entry point
└── examples/
    └── .../
        ├── Cargo.toml   # Example app depending on the distro crate
        └── examples/
            └── *.rs     # RTICX application examples
```

### Export module responsibilities

The `export` module must re-export:
1. `rticx_sw_pass::export::*` — provides the SPSC `Queue` type for sw/async task inputs.
2. Target-specific runtime functions: `run(prio, f)`, `lock(ptr, ceiling, f)`, `pend(irq)`, `unpend(irq)`, `enable(irq, prio, cpu_int_id)`.
3. The platform interrupt type (e.g. `pub use esp32c3::Interrupt`).
4. Critical-section helpers (e.g. `pub use riscv::interrupt`).
5. Optionally: `pub use rticx_async as async_rt;` when the `async` feature is enabled.
