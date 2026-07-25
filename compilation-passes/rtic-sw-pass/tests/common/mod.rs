//! Shared helpers for the `rtic-sw-pass` integration tests.

#![allow(dead_code)]

use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::quote;
use rtic_sw_pass::SwPassBackend;
use syn::{ItemFn, parse_quote};

/// Single-core macro arguments with one PAC path. Used by the parse tests.
pub fn single_core_args() -> TokenStream {
    quote!(device = mypac)
}

/// Two-core macro arguments sharing a single PAC path. Used by the parse tests.
pub fn multi_core_args() -> TokenStream {
    quote!(device = mypac, cores = 2)
}

/// Wraps the given `items` tokenstream in `mod app { ... }` and parses it into
/// an `ItemMod`.
pub fn app_mod(items: TokenStream) -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #items
        }
    }
}

// ---------------------------------------------------------------------------
// Argument & module builders for the analysis / codegen tests
// ---------------------------------------------------------------------------

/// Single-core software-task macro arguments with one dispatcher.
pub fn single_core_sw_args() -> TokenStream {
    quote!(device = mypac, dispatchers = [IRQ0])
}

/// Two-core software-task macro arguments with one dispatcher per core.
pub fn multi_core_sw_args() -> TokenStream {
    quote!(device = mypac, cores = 2, dispatchers = [[IRQ0], [IRQ1]])
}

/// Three-core software-task macro arguments with one dispatcher per core.
/// Used by the negative-analysis test that needs a third spawner core.
pub fn three_core_sw_args() -> TokenStream {
    quote!(
        device = mypac,
        cores = 3,
        dispatchers = [[IRQ0], [IRQ1], [IRQ2]]
    )
}

/// A single-core app module containing one local software task `Foo` (priority
/// 2) and a passthrough struct `Bar`. This is the canonical input for the
/// codegen single-core expansion test.
pub fn single_core_sw_app_module() -> syn::ItemMod {
    app_mod(quote! {
        struct Bar;

        #[sw_task(priority = 2)]
        struct Foo;

        impl RticSwTask for Foo {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self {
                Foo
            }
            fn exec(&mut self, input: u32) {}
        }
    })
}

/// A two-core app module with:
/// - core 0: a local software task `Task0` (priority 2, spawned by core 0).
/// - core 1: a cross-core software task `Cross` (priority 3, core 1, spawned
///   by core 0).
pub fn multi_core_sw_app_module() -> syn::ItemMod {
    app_mod(quote! {
        #[sw_task(priority = 2, core = 0)]
        struct Task0;

        impl RticSwTask for Task0 {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self {
                Task0
            }
            fn exec(&mut self, input: u32) {}
        }

        #[sw_task(priority = 3, core = 1, spawn_by = 0)]
        struct Cross;

        impl RticSwTask for Cross {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self {
                Cross
            }
            fn exec(&mut self, input: u32) {}
        }
    })
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

/// Asserts that `result` is an `Err` whose message contains `substr`, without
/// requiring the `Ok` variant to implement `Debug` (the parsed structs do not).
pub fn assert_err_contains<T>(result: syn::Result<T>, substr: &str) {
    let err = match result {
        Ok(_) => panic!("expected an error, but parsing/analysis succeeded"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains(substr),
        "expected error to contain {substr:?}, got: {err}"
    );
}

/// Asserts that the `expected` tokenstream (rendered to a string) is present
/// as a contiguous substring of the `generated` string. A `label` is used to
/// make failures easier to diagnose.
pub fn assert_section_present(generated: &str, expected: TokenStream, label: &str) {
    let expected = expected.to_string();
    assert!(
        generated.contains(&expected),
        "missing expected section `{label}` in the generated output\n\
         expected:\n{expected}\n\n\
         generated:\n{generated}"
    );
}

// ---------------------------------------------------------------------------
// Mock SwPassBackend
// ---------------------------------------------------------------------------

/// A mock `SwPassBackend` used by the codegen tests.
///
/// When `cross` is `false`, `generate_cross_pend_fn` returns `None` (single
/// core). When `cross` is `true`, it returns a cross-core pend function whose
/// body calls a `mock_cross_pend` stub, allowing tests to assert the cross-core
/// pending path was emitted.
pub struct MockSwBackend {
    pub cross: bool,
}

impl SwPassBackend for MockSwBackend {
    fn queue_path(&self) -> syn::Path {
        parse_quote!(rtic::export::Queue)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            mock_local_pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    fn generate_cross_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> Option<ItemFn> {
        if !self.cross {
            return None;
        }
        let body = parse_quote!({
            mock_cross_pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        Some(empty_body_fn)
    }
}

// ---------------------------------------------------------------------------
// Utility: stringify an ItemMod for codegen assertions
// ---------------------------------------------------------------------------

/// Convenience: render an `ItemMod` into its token-stream string form.
pub fn mod_to_string(item_mod: &syn::ItemMod) -> String {
    item_mod.to_token_stream().to_string()
}
