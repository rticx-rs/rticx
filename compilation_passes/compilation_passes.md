### RTIC Compilation passes

An rtic compilation pass is is represented in rust by a type that implements the `RticPass` trait:
```rust
use proc_macro2::TokenStream as TokenStream2;
pub trait RticPass {
    fn run_pass(
        self, args: TokenStream2, app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;
}
```

The trait has one method that is quite similar to an attribute proc macro definition. It takes a tokenstream representing the #[rtic::app(..)] attribute and the second argument represent the user RTIC application. The returned value represents the expanded inputs.

The idea here is that a compilation pass does't expand the entire rtic application, but instead it implements a specific feature-set of the RTIC framework. Then multiple (compatible) passes are chained together to form the whole proc-macro logic for expanding a user application.

For example, a compilation pass may only understand how to expand monotonics, then the output is provided to the next pass which understand how to describe software tasks interms of hardware tasks and message queues. Finally, the output is fed to the lowest level pass which knows how to generate the SRP model from hardware tasks and resources.

This approach allows developing compilation passes contained within their own crates and maintain them separately, then an RTIC distribution crate, can select and integrate a set of passes to form an rtic proc-macro crate with a given set of features provided by the combined passes.

Compilation passes are usually written in a hardware agnostic fashion, and the target specific details can be provided through what we call `Backend` traits implementations. Each pass may have an associated `Backend` trait that list some functions which a distribution implements to guide the pass on how to generate code that is directly related to the target hardware. One example of a **compilation pass** and its associated **Backend trait** is the `core compilation pass` provided by `rtic-core` crate. The backend trait for that pass is called `rtic_core::CorePassBackend`.
