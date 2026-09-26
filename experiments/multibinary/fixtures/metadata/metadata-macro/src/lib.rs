//! Minimal `#[app]` stand-in for the metadata fixture (`fixtures/metadata`).
//!
//! It runs only the cross-binary metadata pass, which is enough for
//! `cargo xbin sync` to collect `<target>.xbin.json` on the host. The real
//! distributions run the full core pass and are exercised in M3/M4.

use proc_macro::TokenStream;
use rticx_xbin_pass::{RticPass, XbinPass};

/// Runs the cross-binary pass and emits the stripped module.
///
/// The `#[app]` arguments (e.g. `device`) are consumed but not used: no core
/// pass runs here.
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = proc_macro2::TokenStream::from(args);
    let app_mod = syn::parse_macro_input!(input as syn::ItemMod);
    match XbinPass::from_env().run_pass(args, app_mod) {
        Ok((_core_args, app_mod)) => quote::quote!(#app_mod).into(),
        Err(error) => error.to_compile_error().into(),
    }
}
