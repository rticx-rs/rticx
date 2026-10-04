//! `#[app]` macro of the shared mock distribution.
//!
//! It mirrors what a real distribution does, but with host-testable pieces:
//!
//! - runs the **core pass** on [`MockCoreBackend`], so no target-specific
//!   hardware bindings are needed;
//! - binds the **cross-binary pass** ([`XbinPass::from_env`]) with a backend
//!   that points the generated code at `xbin-mock-runtime`, then the
//!   **software pass** ([`SoftwarePass`]) with a backend that generates the
//!   `RticSwTask` trait and the `__rticx_local_irq_pend` the router calls
//!   (M6.5-T5);
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
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
use rticx_xbin_pass::{IpcPool, PhysicalCore, XbinPass, XbinPassBackend};
use syn::{Expr, ItemMod, Lit, parse_macro_input};
use xbin_mock_capability::fixture_pools;

/// Code-generation backend of the mock distribution.
///
/// The generated code reaches the runtime through `xbin-mock-runtime` and
/// calls the injected `__rticx_xbin_backend()` helper for its backend value.
/// The backend also carries the application's `core_ids`, so its capability
/// binding ([`XbinPassBackend::physical_core`],
/// [`XbinPassBackend::ipc_pools`], M6.9-T1) can map a local core index to the
/// fixture's physical core.
struct MockDistroBackend {
    /// Local core index -> global core id (`core_ids = [..]`), empty when the
    /// application leaves the identity default.
    core_ids: Vec<u32>,
}

impl MockDistroBackend {
    /// Physical core of local core `local_core` (M6.9-T1).
    ///
    /// The mock's physical-core vocabulary is the global core id, so the two
    /// applications running on the endpoints of a dual agree on it. A local
    /// index outside the declared mapping falls back to the identity, matching
    /// the trait default.
    fn physical(&self, local_core: u32) -> PhysicalCore {
        PhysicalCore(
            self.core_ids
                .get(local_core as usize)
                .copied()
                .unwrap_or(local_core),
        )
    }
}

impl XbinPassBackend for MockDistroBackend {
    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(xbin_mock_runtime::xbin_rt)
    }

    fn current_global_core_id(&self) -> syn::Expr {
        // The host mock resolves the core at runtime: the same generated code
        // is expanded for every simulated core, so the guard cannot be baked
        // (D5).
        syn::parse_quote!(__rticx_xbin_backend().global_core_id())
    }

    fn ipc_base_override(&self, _view_core: u32, source: u32, target: u32) -> Option<syn::Expr> {
        // The mock pools are heap-backed, not at the addresses `system.json`
        // records, so the generated FIFO views must look the base up at runtime
        // (D3). Both endpoints see the pool at the same address.
        Some(syn::parse_quote!(
            xbin_mock_runtime::pool_base(#source, #target)
        ))
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

    fn physical_core(&self, local_core: u32) -> PhysicalCore {
        self.physical(local_core)
    }

    fn ipc_pools(&self, local_core: u32) -> Vec<IpcPool> {
        fixture_pools(self.physical(local_core))
    }
}

/// Software-task backend of the mock distribution (M6.5-T5).
///
/// The fixture has no software tasks of its own (the cross receiver is
/// rewritten into a core `#[task]` by the cross-binary pass), but the bound
/// [`SoftwarePass`] still generates the `RticSwTask` trait the receiver
/// implements and the `__rticx_local_irq_pend` the generated router calls.
/// The fixture binaries are built but never run on the host, so pending is a
/// no-op; the in-tree harnesses exercise the runtime dispatch path.
struct MockSwBackend;

impl SwPassBackend for MockSwBackend {
    fn queue_path(&self) -> syn::Path {
        syn::parse_quote!(xbin_mock_runtime::xbin_rt::Queue)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: syn::ItemFn) -> syn::ItemFn {
        // The fixture binaries are built (metadata mode included) but never
        // run: the host has no interrupt controller, so pending is a no-op.
        // The runtime dispatch path is exercised by the in-tree harnesses,
        // whose software-pass backends run the bound handler directly.
        empty_body_fn.block = Box::new(syn::parse_quote!({
            let _ = irq_nbr;
        }));
        empty_body_fn
    }

    fn generate_cross_pend_fn(
        &self,
        _core: u32,
        _empty_body_fn: syn::ItemFn,
    ) -> Option<syn::ItemFn> {
        None
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
    let core_ids = core_ids(&parsed_args);
    inject_backend_helper(&mut app_mod, core_ids.first().copied().unwrap_or(0));
    inject_interrupt_support(&mut app_mod, &parsed_args);

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::from_env().with_backend(MockDistroBackend {
        core_ids: core_ids.clone(),
    }));
    builder.bind_pre_core_pass(SoftwarePass::new(MockSwBackend));
    builder.build_rtic_macro(args, quote!(#app_mod).into())
}

/// Reads the local core index -> global core id mapping from
/// `core_ids = [g0, g1, ..]`.
///
/// The mapping drives both the injected backend helper (the fixture
/// applications declare exactly one core each, so the first entry is the
/// current global core id) and the capability binding's
/// [`physical_core`](XbinPassBackend::physical_core) (M6.9-T1).
fn core_ids(args: &TokenStream2) -> Vec<u32> {
    let Ok(attr) = RticAttr::parse_from_tokens(args.clone(), format_ident!("app")) else {
        return Vec::new();
    };
    let Some(Expr::Array(array)) = attr.get_expr("core_ids") else {
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

/// Injects the fixture interrupt enum (M6.5-T3/T5).
///
/// A real distribution points `custom_interrupt_path` at its PAC; the mock
/// provides the enum itself. Its variants are the last path segments of the
/// application's `ipc_dispatchers` entries, so `#interrupt_type::#entry` in the
/// generated router pend call resolves.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock distribution backend for an application whose `core_ids` are
    /// `core_ids`.
    fn backend(core_ids: &[u32]) -> MockDistroBackend {
        MockDistroBackend {
            core_ids: core_ids.to_vec(),
        }
    }

    /// The mock's physical-core vocabulary is the global core id: the
    /// capability binding maps the local index through `core_ids` (M6.9-T1).
    #[test]
    fn physical_core_maps_the_local_index_through_core_ids() {
        let backend = backend(&[7, 9]);
        assert_eq!(backend.physical_core(0), PhysicalCore(7));
        assert_eq!(backend.physical_core(1), PhysicalCore(9));
        // Outside the declared mapping the local index is the identity,
        // matching the trait default.
        assert_eq!(backend.physical_core(5), PhysicalCore(5));
    }

    /// The fixture topology is fully connected: each of the physical cores
    /// 0, 1 and 2 reaches the other two through one pool each.
    #[test]
    fn fixture_topology_exposes_three_duals() {
        for core in 0..3u32 {
            let pools = backend(&[core]).ipc_pools(0);
            assert_eq!(
                pools.len(),
                2,
                "physical core {core} must reach its two peers"
            );
            let mut peers: Vec<u32> = pools.iter().map(|pool| pool.peer.0).collect();
            peers.sort_unstable();
            let expected: Vec<u32> = (0..3u32).filter(|peer| *peer != core).collect();
            assert_eq!(peers, expected, "physical core {core} reaches {expected:?}");
        }
    }

    /// The entries are ordered by ascending peer, so the capability binding is
    /// deterministic.
    #[test]
    fn pools_are_ordered_by_peer() {
        for core in 0..3u32 {
            let pools = backend(&[core]).ipc_pools(0);
            let peers: Vec<u32> = pools.iter().map(|pool| pool.peer.0).collect();
            let mut sorted = peers.clone();
            sorted.sort_unstable();
            assert_eq!(peers, sorted, "physical core {core} pools are not sorted");
        }
    }

    /// The two endpoints of a dual report the same pool from their own side:
    /// the same id, opposite physical cores, each side's local view equal to
    /// the other's peer view, and the same shared budget and policy
    /// (M6.9-T1).
    #[test]
    fn pools_are_symmetric_across_endpoints() {
        // The fixture applications each run on one physical core, with ids 0,
        // 1 and 2.
        let endpoints: Vec<(u32, Vec<IpcPool>)> = (0..3u32)
            .map(|core| (core, backend(&[core]).ipc_pools(0)))
            .collect();

        for (core, pools) in &endpoints {
            for pool in pools {
                let (_, peer_pools) = endpoints
                    .iter()
                    .find(|(peer, _)| *peer == pool.peer.0)
                    .unwrap_or_else(|| {
                        panic!(
                            "physical core {core} reports a non-fixture peer {}",
                            pool.peer
                        )
                    });
                let mirror = peer_pools
                    .iter()
                    .find(|candidate| candidate.id == pool.id)
                    .unwrap_or_else(|| {
                        panic!(
                            "physical core {core} reports pool `{}` that peer {} does not",
                            pool.id, pool.peer
                        )
                    });

                assert_eq!(mirror.peer, PhysicalCore(*core));
                assert_eq!(mirror.base_local, pool.base_peer);
                assert_eq!(mirror.base_peer, pool.base_local);
                assert_eq!(mirror.budget, pool.budget);
                assert_eq!(mirror.policy, pool.policy);
            }
        }
    }
}
