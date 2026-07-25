//! Shared helpers for the `rticx-deadline-pass` integration tests.

#![allow(dead_code)]

use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::quote;
use rticx_core::parse_utils::RticAttr;
use rticx_deadline_pass::deadline_pass::{App, DeadlineToPriorityPass};

/// Single-core macro arguments with one PAC path.
pub fn single_core_args() -> TokenStream {
    quote!(device = mypac)
}

/// Multi-core macro arguments with one PAC path and `n` cores.
pub fn multi_core_args(n: u32) -> TokenStream {
    quote!(device = mypac, cores = #n)
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

/// Parse args + items and run the deadline pass analysis.
pub fn analyze(args: TokenStream, items: TokenStream) -> syn::Result<App> {
    let app_mod = app_mod(items);
    let params = RticAttr::parse_from_tokens(args).expect("params parse");
    let mut parsed = App::parse(&params, app_mod).expect("app parse");
    let pass = DeadlineToPriorityPass::new(255); // High max_priority for tests
    pass.analyze(&mut parsed);
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

/// Asserts that `result` is an `Err` whose message contains `substr`, without
/// requiring the `Ok` variant to implement `Debug`.
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
// Utility: stringify an ItemMod for codegen assertions
// ---------------------------------------------------------------------------

/// Convenience: render an `ItemMod` into its token-stream string form.
pub fn mod_to_string(item_mod: &syn::ItemMod) -> String {
    item_mod.to_token_stream().to_string()
}
