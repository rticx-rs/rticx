# Writing Distributions

This page explains how to create a new RTIC distribution for a target that is not already covered by the reference distributions.

## Important: new distributions live out-of-tree

This repository only maintains the core framework and a small set of reference distributions. New hardware distributions should be developed in their own crates and repositories. They are not merged into the core project.

## Distribution structure

A distribution consists of two crates:

1. **The library crate** — users depend on this. It re-exports the proc macro and exposes an `export` module with runtime helpers.
2. **The macro crate** — defines the actual `#[rtic::app]` proc macro and implements the backend traits.

Example layout:

```
my-rtic/
├── Cargo.toml
├── src/
│   └── lib.rs          # re-exports app macro and export module
└── my-rtic-macro/
    ├── Cargo.toml
    └── src/
        └── lib.rs      # proc macro + backend impl
```

## Implementing `CorePassBackend`

The macro crate implements `rtic_core::CorePassBackend`. This is the bulk of the target-specific work. Refer to the method table in [Architecture](Distributor-Guide-Architecture) for the full interface.

At minimum, you must implement:

- `generate_resource_proxy_lock_impl` — how shared resources are locked.
- `generate_global_definitions` — any global constants or helper functions.
- `wrap_task_execution` — how a task body is wrapped in an interrupt handler.
- `post_init` — code after initialization.
- `entry_name`, `entry_attrs` — entry point naming and attributes.
- `task_attrs` — attributes injected onto task interrupt handlers.
- `default_task_priority` — fallback task priority.
- `generate_interrupt_free_fn` — the global critical-section function.

## Assembling the macro with `RticMacroBuilder`

```rust
use proc_macro::TokenStream;
use rtic_core::RticMacroBuilder;

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut builder = RticMacroBuilder::new(MyBackend);
    builder.bind_pre_core_pass(SoftwarePass::new(MySwBackend));
    builder.bind_pre_core_pass(AutoAssignPass);
    builder.build_rtic_macro(args, input)
}
```

## Optional: implementing pass backends

If your distribution uses software tasks, implement `SwPassBackend`:

```rust
impl SwPassBackend for MySwBackend {
    fn generate_local_pend_fn(&self, empty_body_fn: ItemFn) -> ItemFn {
        // Fill the local NVIC set-pending function
    }

    fn generate_cross_pend_fn(&self, empty_body_fn: ItemFn) -> Option<ItemFn> {
        // Fill the cross-core pending function, or None for single-core
    }
}
```

## The library crate

The library crate re-exports the macro and provides the `export` module:

```rust
pub use my_rtic_macro::app;

pub mod export {
    // Re-export target runtime helpers, e.g.:
    // pub use cortex_m::peripheral::NVIC;
    // pub use rtic_sw_pass::export::*;
}
```

Users write:

```rust
use my_rtic::app;

#[app(device = ...)]
mod my_app { ... }
```

## Feature flags

Expose the passes you want to enable as Cargo features on the macro crate and the library crate:

```toml
[features]
swtasks = ["rtic-macro/swtasks"]
autoassign = ["rtic-macro/autoassign"]
```

This lets users opt into syntax extensions without paying for them when they are not needed.

## Single-core vs multicore

- For single-core targets, implement only the core backend and ignore cross-core features.
- For multicore targets, you need to handle core entry points, cross-core dispatch, and shared memory. See [Multibin and Multipac](Distributor-Guide-Multibin-Multipac) for multi-binary systems.

## Validation and debugging

Use the `debug_expand` feature of `rtic-core` to write the expanded macro output to `examples/{binary_name}_expanded.rs`:

```toml
[features]
debug_expand = ["rtic-core/debug_expand"]
```

## Reference distributions

Study the existing reference distributions for concrete examples:

- `rp2040-rtic` — dual-core Cortex-M0+ with software tasks.
- `stm32-renode-rtic` — multi-binary multicore build.
- `rtic-hippo` — single-core RISC-V with threshold-based locking.
- `atalanta-rtic` — single-core RISC-V.

## Next steps

- [Multibin and Multipac](Distributor-Guide-Multibin-Multipac) — multi-binary and multi-PAC support.
- [Writing Compilation Passes](Distributor-Guide-Writing-Compilation-Passes) — if you need a new pass for your distribution.
