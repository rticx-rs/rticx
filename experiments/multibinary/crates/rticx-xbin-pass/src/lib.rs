//! Compilation pass for cross-binary tasks in the RTICX multi-binary
//! extension.
//!
//! The pass has two modes (see `multibinary-multicore-plan.md` §7 and §8):
//!
//! - **metadata mode** (phase 1, `cargo xbin sync`): when
//!   `RTICX_XBIN_META_OUT` is set, it writes `<app>.xbin.json` describing the
//!   cross-binary tasks declared by this application;
//! - **codegen mode** (phase 2): it reads the driver-generated
//!   `system.json`, filters it by this application's cores, and emits sender
//!   stubs / receiver dispatchers.
//!
//! At M0 the pass is only a skeleton: it is registered like any other
//! [`RticPass`] and currently leaves the application untouched.

use proc_macro2::TokenStream;
use rticx_core::{InfoBus, RticPass};
use syn::ItemMod;

/// The cross-binary compilation pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct XbinPass;

impl XbinPass {
    /// Creates the pass.
    pub fn new() -> Self {
        Self
    }
}

impl RticPass for XbinPass {
    fn subscribe(&mut self, _info_bus: InfoBus) {}

    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        // TODO(M1-T2/M1-T3): metadata-mode manifest emit and attribute
        // parsing/stripping. Until then the pass is a no-op.
        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        "rticx-xbin-pass"
    }
}

#[cfg(test)]
mod tests {
    use quote::quote;
    use rticx_core::RticPass;

    use super::XbinPass;

    #[test]
    fn run_pass_is_identity() {
        let pass = XbinPass::new();
        let app_mod: syn::ItemMod = syn::parse_quote! {
            mod app {
                #[task]
                fn a() {}
            }
        };
        let input_args = quote!(device = X, cores = 2);
        let (args, out) = pass
            .run_pass(input_args.clone(), app_mod.clone())
            .expect("no-op pass cannot fail");
        assert_eq!(args.to_string(), input_args.to_string());
        assert_eq!(out, app_mod);
        assert_eq!(pass.pass_name(), "rticx-xbin-pass");
    }
}
