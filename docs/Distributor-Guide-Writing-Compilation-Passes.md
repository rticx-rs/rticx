# Writing Compilation Passes

This page explains how to write a new compilation pass that can be plugged into an RTIC distribution.

## What is a compilation pass?

A compilation pass is a self-contained crate that implements a subset of RTIC functionality. It transforms RTIC syntax into RTIC syntax, usually simplifying the input so that the next pass or the core pass can understand it.

For example, the software-tasks pass transforms `#[sw_task]` and `spawn()` calls into hardware tasks and dispatcher interrupts. The deadline pass converts `deadline = D` into `priority = N`. The auto-assign pass infers `core = N` from shared resource usage.

## The `RticPass` trait

Every pass implements `RticPass` from `rtic-core`:

```rust
use proc_macro2::TokenStream as TokenStream2;
use syn::ItemMod;

pub trait RticPass {
    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;

    fn pass_name(&self) -> &str;
}
```

- `args` — the token stream of the `#[rtic::app(...)]` attribute arguments.
- `app_mod` — the annotated module.
- The return value is the transformed `(args, app_mod)`.
- `pass_name` is used in error messages to identify which pass failed.

## Pass ordering

Passes can be registered as **pre-core** or **post-core** passes:

```rust
use rtic_core::RticMacroBuilder;

let mut builder = RticMacroBuilder::new(my_backend);
builder.bind_pre_core_pass(MyPass);
builder.bind_post_core_pass(MyPostPass);
```

- Pre-core passes run before `rtic-core` parses the module. Use them to expand high-level syntax into core RTIC syntax.
- Post-core passes run after code generation. Use them to inspect or augment the generated token stream.

## Pass-specific backend traits

If a pass needs target-specific information, it can define its own backend trait. The distribution implements this trait and passes the implementation to the pass constructor.

For example, `rtic-sw-pass` defines `SwPassBackend`:

```rust
pub trait SwPassBackend {
    fn generate_local_pend_fn(&self, empty_body_fn: ItemFn) -> ItemFn;
    fn generate_cross_pend_fn(&self, empty_body_fn: ItemFn) -> Option<ItemFn>;
    fn custom_interrupt_path(&self, core: u32) -> Option<syn::Path> { None }
}
```

- `generate_local_pend_fn` fills the body of the core-local interrupt-pending function used by `spawn`.
- `generate_cross_pend_fn` fills the cross-core interrupt-pending function used by `spawn_from`. Returns `None` on single-core targets.
- `custom_interrupt_path` optionally overrides the default PAC interrupt path.

## Anatomy of a pass crate

A typical pass crate contains:

1. A `Cargo.toml` with `rtic-core` as a dependency.
2. A public type implementing `RticPass`.
3. Optionally, a public backend trait for target-specific hooks.
4. A public constructor that accepts the backend trait implementation.

## Example skeleton

```rust
use proc_macro2::TokenStream as TokenStream2;
use syn::ItemMod;
use rtic_core::RticPass;

pub struct MyPass;

impl RticPass for MyPass {
    fn run_pass(
        &self,
        args: TokenStream2,
        mut app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)> {
        // Inspect and transform app_mod and args here
        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        "my-pass"
    }
}
```

## Testing a pass

Because passes are pure syntax transformers, you can test them by feeding them a parsed `ItemMod` and asserting on the output. For passes that change the module items, you can also write `trybuild`-style compilation tests as distribution examples.

## Next steps

- [Writing Distributions](Distributor-Guide-Writing-Distributions) — plug your pass into a distribution.
- [Distributor Guide Architecture](Distributor-Guide-Architecture) — understand the full pipeline.
