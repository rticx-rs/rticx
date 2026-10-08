//! Minimal `#[app]` stand-in for the metadata fixture (`crates/mock/fixtures/metadata`).
//!
//! It runs only the cross-binary metadata pass, which is enough for
//! `cargo xbin sync` to collect `<target>.xbin.json` on the host. The real
//! distributions run the full core pass and are exercised by the build
//! fixtures.
//!
//! The stand-in binds the mock distribution's capability table
//! (shared through `xbin-mock-capability`), so the fixture manifests carry the
//! physical core ids and IPC pools `sync` needs to emit `system.json` schema 2.
//! Only `physical_core`/`ipc_pools` are reached in metadata mode; the codegen
//! bindings below are never called and exist to satisfy the trait.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;
use rticx_xbin_pass::{
    AppTopology, IpcPool, PhysicalCore, RticPass, XbinPass, XbinPassBackend, resolve_app_topology,
};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, Lit, MetaNameValue, Token};
use xbin_mock_capability::fixture_pools;

/// Capability binding of the metadata-only stand-in.
///
/// The mock's physical-core vocabulary is the global core id, so the local
/// index is mapped through the application's resolved `rticx.toml` topology
/// exactly like the real mock distribution does.
struct MetadataCapabilityBackend {
    /// Local core index -> global core id, resolved from `rticx.toml`;
    /// empty for the identity default.
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
    fn current_global_core_id(&self) -> syn::Expr {
        // Never reached: the fixture runs only the metadata pass.
        syn::parse_quote!(unreachable!())
    }

    fn rt_path(&self) -> syn::Path {
        // Placeholder: metadata mode halts before any codegen, so no runtime
        // path is ever emitted. It deliberately does not name the injected
        // `ipc_types` module, which holds the IDL message types, not the
        // runtime.
        syn::parse_quote!(xbin_mock_runtime)
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
    let topology = match app_topology(&args) {
        Ok(topology) => topology,
        Err(error) => return error.to_compile_error().into(),
    };
    let backend = MetadataCapabilityBackend {
        core_ids: topology.core_ids().to_vec(),
    };
    match XbinPass::from_env()
        .with_backend(backend)
        .run_pass(args, app_mod)
    {
        Ok((_core_args, app_mod)) => quote::quote!(#app_mod).into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Resolves the application's topology from the nearest `rticx.toml`.
///
/// A standalone project with no `rticx.toml` falls back to the identity
/// mapping over the declared `cores` (default 1), exactly like the pass.
fn app_topology(args: &TokenStream2) -> syn::Result<AppTopology> {
    let package = std::env::var("CARGO_PKG_NAME").ok();
    let target = std::env::var("CARGO_BIN_NAME").ok();
    let resolved = match package.as_deref() {
        Some(package) => resolve_app_topology(package, target.as_deref())?,
        None => None,
    };
    Ok(match resolved {
        Some(resolved) => resolved.topology,
        None => AppTopology::identity(declared_cores(args)),
    })
}

/// Reads the declared `cores` argument (default 1) for the no-project
/// fallback.
fn declared_cores(args: &TokenStream2) -> u32 {
    let Ok(entries) = Punctuated::<MetaNameValue, Token![,]>::parse_terminated.parse2(args.clone())
    else {
        return 1;
    };
    let Some(value) = entries
        .iter()
        .find(|entry| entry.path.is_ident("cores"))
        .map(|entry| &entry.value)
    else {
        return 1;
    };
    match value {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Int(int) => int.base10_parse().unwrap_or(1),
            _ => 1,
        },
        _ => 1,
    }
}
