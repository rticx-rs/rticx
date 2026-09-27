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

    fn ring_doorbell_fn(&self, source: u32, target: u32, mut template: syn::ItemFn) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().doorbell_send(#source, #target, task_id)
        });
        template
    }

    fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident {
        format_ident!("__xbin_router_{source}_{target}")
    }

    fn read_doorbell_msg_fn(
        &self,
        target: u32,
        source: u32,
        mut template: syn::ItemFn,
    ) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().take_message(#source, #target)
        });
        template
    }

    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        Some(syn::parse_quote!(__XbinInterrupt))
    }
}

/// The mock distribution's `#[app]` attribute macro.
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let parsed_args = TokenStream2::from(args.clone());
    let mut app_mod = parse_macro_input!(input as ItemMod);
    inject_backend_helper(&mut app_mod, global_core(&parsed_args));
    inject_interrupt_support(&mut app_mod, &parsed_args);

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

/// Injects the fixture interrupt enum and the stand-in pend function the
/// generated router calls (M6.5-T3).
///
/// A real distribution points `custom_interrupt_path` at its PAC and gets
/// `__rticx_local_irq_pend[_core{N}]` from `rticx-sw-pass`; the mock binds no
/// software pass, so it provides both itself. The enum's variants are the last
/// path segments of the application's `ipc_dispatchers` entries, so
/// `#interrupt_type::#entry` in the generated pend call resolves.
fn inject_interrupt_support(app_mod: &mut ItemMod, args: &TokenStream2) {
    let variants = ipc_dispatcher_variants(args);
    let Some((_, items)) = app_mod.content.as_mut() else {
        return;
    };
    items.push(syn::parse_quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        pub enum __XbinInterrupt {
            #(#variants,)*
        }
    });
    items.push(syn::parse_quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub fn __rticx_local_irq_pend(_irq: __XbinInterrupt) {}
    });
}

/// Collects the last path segment of every `ipc_dispatchers` entry.
fn ipc_dispatcher_variants(args: &TokenStream2) -> Vec<syn::Ident> {
    let Ok(attr) = RticAttr::parse_from_tokens(args.clone(), format_ident!("app")) else {
        return Vec::new();
    };
    let Some(expr) = attr.get_expr("ipc_dispatchers") else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    collect_dispatcher_idents(expr, &mut variants);
    variants
}

/// Recursively collects interrupt path idents from an `ipc_dispatchers`
/// expression (flat or per-core nested arrays).
fn collect_dispatcher_idents(expr: &Expr, out: &mut Vec<syn::Ident>) {
    match expr {
        Expr::Array(array) => {
            for element in &array.elems {
                collect_dispatcher_idents(element, out);
            }
        }
        Expr::Path(path) if path.qself.is_none() => {
            if let Some(segment) = path.path.segments.last() {
                out.push(segment.ident.clone());
            }
        }
        _ => {}
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
