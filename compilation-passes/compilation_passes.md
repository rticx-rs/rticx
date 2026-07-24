### RTIC Compilation Passes

An RTIC compilation pass is represented by a Rust type that implements the `RticPass` trait defined in `rtic-core`:

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

The trait has a single transformation method that is quite similar to an attribute proc macro. It takes a token stream representing the `#[rtic::app(..)]` attribute arguments and the annotated module as the user wrote it. The returned value is the transformed `(args, app_mod)` pair.

A compilation pass should not expand the entire RTIC application on its own. Instead, it implements and expands specific parts of the entire application. Multiple compatible passes are chained together to form the full proc-macro logic for expanding the full user application.

For example, a compilation pass may only understand how to expand monotonics; the output is provided to the next pass, which understands how to describe software tasks in terms of hardware tasks and message queues. Finally, the output is fed to the lowest-level pass, which knows how to generate the SRP model from hardware tasks and resources.

This approach allows developing compilation passes in their own crates and maintaining them separately. An RTIC distribution then selects and integrates a set of passes to form an RTIC proc-macro crate with a given set of features.

Compilation passes are usually written in a hardware-agnostic fashion. Target-specific details can be provided through backend traits defined by the pass. Each pass may have an associated backend trait that lists functions a distribution implements to guide the pass on how to generate code directly related to the target hardware.

One example of a compilation pass and its associated backend trait is the **core compilation pass** provided by the `rtic-core` crate. Its backend trait is `rtic_core::CorePassBackend`. Another example is the `rtic-sw-pass` crate, which defines `SwPassBackend` for software-task support.
