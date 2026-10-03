//! Minimal `#[app]` stand-in for the metadata fixture (`fixtures/metadata`).
//!
//! It runs only the cross-binary metadata pass, which is enough for
//! `cargo xbin sync` to collect `<target>.xbin.json` on the host. The real
//! distributions run the full core pass and are exercised in M3/M4.
//!
//! Since M6.9-T9 the stand-in binds the mock distribution's capability table
//! (shared through `xbin-mock-capability`), so the fixture manifests carry the
//! physical core ids and IPC pools `sync` needs to emit `system.json` schema 2.
//! Only `physical_core`/`ipc_pools` are reached in metadata mode; the codegen
//! bindings below are never called and exist to satisfy the trait.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;
use rticx_xbin_pass::{IpcPool, PhysicalCore, RticPass, XbinPass, XbinPassBackend};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, Lit, MetaNameValue, Token};
use xbin_mock_capability::fixture_pools;

/// Capability binding of the metadata-only stand-in.
///
/// The mock's physical-core vocabulary is the global core id, so the local
/// index is mapped through the application's `core_ids` exactly like the real
/// mock distribution does (M6.9-T1).
struct MetadataCapabilityBackend {
    /// Local core index -> global core id (`core_ids = [..]`), empty for the
    /// identity default.
    core_ids: Vec<u32>,
}

impl MetadataCapabilityBackend {
    /// Physical core of local core `local_core`.
    fn physical(&self, local_core: u32) -> PhysicalCore {
        PhysicalCore(
            self.core_ids
                .get(local_core as usize)
                .copied()
                .unwrap_or(local_core),
        )
    }
}

impl XbinPassBackend for MetadataCapabilityBackend {
    fn backend(&self) -> syn::Expr {
        // Never reached: the fixture runs only the metadata pass.
        syn::parse_quote!(unreachable!())
    }

    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(ipc_types)
    }

    fn ring_doorbell_fn(&self, _source: u32, _target: u32, template: syn::ItemFn) -> syn::ItemFn {
        template
    }

    fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident {
        format_ident!("__xbin_router_{source}_{target}")
    }

    fn read_doorbell_msg_fn(
        &self,
        _target: u32,
        _source: u32,
        template: syn::ItemFn,
    ) -> syn::ItemFn {
        template
    }

    fn physical_core(&self, local_core: u32) -> PhysicalCore {
        self.physical(local_core)
    }

    fn ipc_pools(&self, local_core: u32) -> Vec<IpcPool> {
        fixture_pools(self.physical(local_core))
    }
}

/// Runs the cross-binary pass and emits the stripped module.
///
/// The `#[app]` arguments (e.g. `device`) are consumed but not used: no core
/// pass runs here.
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = TokenStream2::from(args);
    let app_mod = syn::parse_macro_input!(input as syn::ItemMod);
    let backend = MetadataCapabilityBackend {
        core_ids: core_ids(&args),
    };
    match XbinPass::from_env()
        .with_backend(backend)
        .run_pass(args, app_mod)
    {
        Ok((_core_args, app_mod)) => quote::quote!(#app_mod).into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Reads the local core index -> global core id mapping from
/// `core_ids = [g0, g1, ..]`.
fn core_ids(args: &TokenStream2) -> Vec<u32> {
    let Ok(entries) = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(args.clone())
    else {
        return Vec::new();
    };
    let Some(value) = entries
        .iter()
        .find(|entry| entry.path.is_ident("core_ids"))
        .map(|entry| &entry.value)
    else {
        return Vec::new();
    };
    let Expr::Array(array) = value else {
        return Vec::new();
    };
    array
        .elems
        .iter()
        .filter_map(|element| match element {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Int(int) => int.base10_parse().ok(),
                _ => None,
            },
            _ => None,
        })
        .collect()
}
