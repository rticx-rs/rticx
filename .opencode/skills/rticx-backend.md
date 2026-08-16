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
8. Collect injections from all passes by calling `pass.main_injection(&point, core)` for each `MainInjectionPoint` and each core.
9. Run `CodeGen::new(core_backend, &parsed_app, &analysis).with_injections(&injections).run()`.
10. If `RTICX_EXPAND` is set, write the final expansion to `target/rticx-expand/` via `rticx-core/src/expand_log.rs`.

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

    fn main_injection(&self, _point: &MainInjectionPoint, _core: u32) -> Option<TokenStream2> {
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

Passes use `main_injection` to inject code (e.g. variable declarations) directly into the entry function body, targeting specific cores. Since the entry functions are `-> !`, injected locals live forever on the stack.
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
