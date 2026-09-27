//! `#[app]` macro of the fixture's mock distribution (M4-T1).
//!
//! It mirrors what a real distribution does, but with host-testable pieces:
//!
//! - runs the **core pass** on [`MockCoreBackend`], so no target-specific
//!   hardware bindings are needed;
//! - binds the **cross-binary pass** ([`XbinPass::from_env`]) with a backend
//!   that points the generated code at `xbin-mock-runtime`;
//! - injects the `__rticx_xbin_backend()` helper the generated code calls,
//!   returning the backend handle of the application's core over the shared
//!   mock system.
//!
//! The macro works in both pass modes: `cargo xbin sync` sets
//! `RTICX_XBIN_META_OUT` (metadata mode) and `cargo xbin build` sets
//! `RTICX_XBIN_SYSTEM` (codegen mode), exactly like a real distribution.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use rticx_core::RticMacroBuilder;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::parse_utils::RticAttr;
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use syn::{Expr, ItemMod, Lit, parse_macro_input};

/// Code-generation backend of the mock distribution.
///
/// The generated code reaches the runtime through `xbin-mock-runtime` and
/// calls the injected `__rticx_xbin_backend()` helper for its backend value.
struct MockDistroBackend;

impl XbinPassBackend for MockDistroBackend {
    fn backend(&self) -> syn::Expr {
        syn::parse_quote!(__rticx_xbin_backend())
    }

    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(xbin_mock_runtime::xbin_rt)
    }

    fn dispatcher_irq(&self, target: u32, line: u32) -> syn::Ident {
        format_ident!("__xbin_doorbell_{target}_{line}")
    }

    fn doorbell_irq(&self, target: u32, line: u32) -> u16 {
        (target * 16 + line) as u16
    }
}

/// The mock distribution's `#[app]` attribute macro.
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let parsed_args = TokenStream2::from(args.clone());
    let mut app_mod = parse_macro_input!(input as ItemMod);
    inject_backend_helper(&mut app_mod, global_core(&parsed_args));

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::from_env().with_backend(MockDistroBackend));
    builder.build_rtic_macro(args, quote!(#app_mod).into())
}

/// Reads the global core id of the application's (single) core from
/// `core_ids = [g]`, defaulting to `0` like the pass does.
///
/// The fixture applications declare exactly one core each, so the first entry
/// is the current global core id.
fn global_core(args: &TokenStream2) -> u32 {
    let Ok(attr) = RticAttr::parse_from_tokens(args.clone(), format_ident!("app")) else {
        return 0;
    };
    let Some(Expr::Array(array)) = attr.get_expr("core_ids") else {
        return 0;
    };
    let Some(Expr::Lit(lit)) = array.elems.first() else {
        return 0;
    };
    match &lit.lit {
        Lit::Int(int) => int.base10_parse().unwrap_or(0),
        _ => 0,
    }
}

/// Injects the backend helper the generated code calls.
///
/// The helper lives inside the `#[app]` module because that is where the
/// generated code is expanded; it resolves the process-global mock system and
/// returns the backend handle of `core`.
fn inject_backend_helper(app_mod: &mut ItemMod, core: u32) {
    let Some((_, items)) = app_mod.content.as_mut() else {
        return;
    };
    let doc = "Backend handle of this application's core, wired by the mock distribution.";
    let core = syn::LitInt::new(&format!("{core}u32"), Span::call_site());
    items.push(syn::parse_quote! {
        #[doc = #doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn __rticx_xbin_backend() -> xbin_mock_runtime::MockBackend {
            xbin_mock_runtime::backend_for(#core)
        }
    });
}
