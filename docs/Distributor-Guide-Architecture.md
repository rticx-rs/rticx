# Architecture

This page describes the modular architecture of the RTIC rewrite and how the core framework, compilation passes, and distributions interact.

## Three layers

```
┌─────────────────────────────────────────────┐
│  User application                             │
│  #[rtic::app(...)]                            │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│  Distribution                                 │
│  - RticMacroBuilder                           │
│  - CorePassBackend impl                       │
│  - Selected passes (pre/post core)            │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│  Compilation passes                           │
│  - RticPass implementations                   │
│  - Pure syntax-to-syntax transformations      │
└──────────────────┬──────────────────────────┘
                   │
┌──────────────────▼──────────────────────────┐
│  rtic-core                                    │
│  - Parse #[rtic::app]                         │
│  - Run SRP analysis                           │
│  - Generate code via CorePassBackend          │
└─────────────────────────────────────────────┘
```

## `rtic-core`

`rtic-core` is the bottom layer. It is responsible for:

- Parsing the `#[rtic::app]` attribute and the annotated module into an AST (`App`).
- Running resource-ceiling analysis under the Stack Resource Policy (SRP).
- Generating the final Rust code for tasks, resources, `init`, `idle`, and interrupt dispatchers.

To keep `rtic-core` target-agnostic, the actual hardware-specific code generation is delegated to a backend trait.

## `CorePassBackend`

`CorePassBackend` is the interface a distribution implements to provide hardware-specific details:

| Method | Purpose |
|--------|---------|
| `post_init(...)` | Code inserted after init and task initialization. |
| `generate_resource_proxy_lock_impl(...)` | Body of the `lock` function for shared resources. |
| `generate_global_definitions(...)` | Extra constants, imports, or helpers at global scope. |
| `wrap_task_execution(...)` | Wrap the task `exec` call inside an interrupt handler. |
| `entry_name(core)` | Name of the entry function for each core. |
| `populate_idle_loop()` | Custom body for the default idle loop. |
| `generate_interrupt_free_fn(...)` | Implement the global critical-section function. |
| `pre_codegen_validation(...)` | Target-specific validation before codegen. |
| `default_task_priority()` | Fallback priority when omitted. |
| `entry_attrs()` | Attributes injected onto entry points. |
| `task_attrs()` | Attributes injected onto task interrupt handlers. |
| `multibin_shared_macro_path()` | Path to the shared-memory macro when `multibin` is enabled. |

## Compilation passes

A compilation pass is a crate that implements `RticPass`:

```rust
pub trait RticPass {
    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;

    fn pass_name(&self) -> &str;
}
```

Passes are pure syntax-to-syntax transformations. They receive the macro arguments and the annotated module, transform them, and return the updated pair. Passes are registered with `RticMacroBuilder` either before or after the core pass.

### Pre-core passes

Run before the module is parsed by `rtic-core`. They are used to transform high-level syntax into the core RTIC syntax that the core pass understands. For example, the software-tasks pass converts `#[sw_task]` and `spawn` calls into hardware tasks and dispatcher queues.

### Post-core passes

Run after the core code generation. They can inspect or augment the generated output.

## `RticMacroBuilder` pipeline

Every distribution builds its macro by constructing an `RticMacroBuilder`:

```rust
let mut builder = RticMacroBuilder::new(my_backend);
builder.bind_pre_core_pass(SoftwarePass::new(my_sw_backend));
builder.bind_pre_core_pass(AutoAssignPass);
let tokens = builder.build_rtic_macro(args, input);
```

The pipeline order inside `build_rtic_macro` is:

1. Reset the default task priority from the backend.
2. Run pre-core passes in insertion order.
3. Parse the module with `App::parse(args, app_mod)`.
4. Run SRP analysis.
5. Call `CorePassBackend::pre_codegen_validation`.
6. Run code generation via `CodeGen::new(core_backend, &parsed_app, &analysis).run()`.
7. Run post-core passes in insertion order.
8. If `debug_expand` is enabled, write the expansion to `examples/{binary_name}_expanded.rs`.

## Multicore model

RTIC applications can target multiple cores. The core compilation pass generates per-core entry points, interrupt handlers, and shared resource proxies. The backend decides:

- How each core is started (e.g., RP2040 starts core 1 from `post_init`).
- How cross-core tasks are dispatched (e.g., via `rtic-sw-pass` and `spawn_from`).
- How shared resources are protected across cores (e.g., interrupt masking, threshold-based locking, or multi-binary shared memory).

For multi-binary systems, the `multibin` feature splits the expanded output into per-core guarded sections. The `multipac` feature allows each core to use a different PAC path.

## Next steps

- [Writing Compilation Passes](Distributor-Guide-Writing-Compilation-Passes)
- [Writing Distributions](Distributor-Guide-Writing-Distributions)
- [Multibin and Multipac](Distributor-Guide-Multibin-Multipac)
