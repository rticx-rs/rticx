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
use xbin_mock_capability::fixture_pools;

/// Capability binding of the metadata-only stand-in.
///
/// The mock's physical-core vocabulary is the global core id, so the
/// application's single resolved `rticx.toml` `core_id` is reported directly.
struct MetadataCapabilityBackend {
    /// The application's single global core id.
    core_id: u32,
}

impl MetadataCapabilityBackend {
    /// Physical core of this application.
    fn physical(&self) -> PhysicalCore {
        PhysicalCore(self.core_id)
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

    fn physical_core(&self, _local_core: u32) -> PhysicalCore {
        self.physical()
    }

    fn ipc_pools(&self, _local_core: u32) -> Vec<IpcPool> {
        fixture_pools(self.physical())
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
    let topology = match app_topology() {
        Ok(topology) => topology,
        Err(error) => return error.to_compile_error().into(),
    };
    let backend = MetadataCapabilityBackend {
        core_id: topology.core_id(),
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
/// A standalone project with no `rticx.toml` falls back to
/// [`AppTopology::identity`] (`core_id = 0`), exactly like the pass.
fn app_topology() -> syn::Result<AppTopology> {
    let package = std::env::var("CARGO_PKG_NAME").ok();
    let target = std::env::var("CARGO_BIN_NAME").ok();
    let resolved = match package.as_deref() {
        Some(package) => resolve_app_topology(package, target.as_deref())?,
        None => None,
    };
    Ok(match resolved {
        Some(resolved) => resolved.topology,
        None => AppTopology::identity(),
    })
}
