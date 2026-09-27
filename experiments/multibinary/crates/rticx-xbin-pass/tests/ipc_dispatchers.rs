//! M6.5-T1 acceptance: the pass-owned `ipc_dispatchers` pool.
//!
//! Covers the per-core pool syntax (`[..]` for one core, `[[..], ..]` for
//! several), the strip in both modes, the count validation against the cross
//! lines of the native `#[sw_task]` receivers, uniqueness and disjointness
//! with the software pass's `dispatchers`, and that `rticx-sw-pass`'s
//! `assign_dispatchers` only ever sees its own entries.

use proc_macro2::TokenStream;
use quote::quote;
use rticx_core::RticMacroBuilder;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
use rticx_xbin_pass::{RticPass, XbinPass};

/// A single-core application with one native cross receiver: one cross line
/// `(source 7, priority 3)` on local core 0.
fn cross_receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    }
}

/// The `#[app]` arguments of a single-core application seeing global core 7,
/// with `ipc` as its pool expression.
fn receiver_args(ipc: TokenStream) -> TokenStream {
    quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [7],
        ipc_dispatchers = #ipc
    )
}

/// Runs the syntax-only pass over the receiver fixture.
fn run(ipc: TokenStream) -> syn::Result<(TokenStream, syn::ItemMod)> {
    XbinPass::disabled().run_pass(receiver_args(ipc), cross_receiver_app())
}

/// Returns the error message of a rejected pool.
fn rejected(ipc: TokenStream) -> String {
    run(ipc)
        .expect_err("the pass must reject this pool")
        .to_string()
}

#[test]
fn flat_pool_is_accepted_and_stripped_in_both_modes() {
    // A flat array is the single-core shape; the pool is consumed so the
    // core/software/async passes never see it, while `dispatchers` stays for
    // the software pass.
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [7],
        dispatchers = [SW_IRQ],
        ipc_dispatchers = [XBIN_IRQ]
    );

    let dir = tempfile::tempdir().expect("tempdir");
    for (label, pass) in [
        ("syntax-only", XbinPass::disabled()),
        (
            "metadata",
            XbinPass::with_manifest(dir.path(), "app", "app"),
        ),
    ] {
        let (stripped, _) = pass
            .run_pass(args.clone(), cross_receiver_app())
            .unwrap_or_else(|error| panic!("{label} pass: {error}"));
        let stripped = stripped.to_string();
        assert!(
            !stripped.contains("ipc_dispatchers"),
            "{label}: the pool must be stripped: {stripped}"
        );
        assert!(
            !stripped.contains("external_cores"),
            "{label}: `external_cores` stays pass-owned: {stripped}"
        );
        assert!(
            stripped.contains("dispatchers"),
            "{label}: the software pool stays for `rticx-sw-pass`: {stripped}"
        );
    }
}

#[test]
fn nested_pool_is_accepted_per_core() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7)]
            struct OnCore0;

            impl RticSwTask for OnCore0 {
                type SpawnInput = ipc_types::Msg;
            }

            #[sw_task(core = 1, priority = 1, spawn_by = 7)]
            struct OnCore1;

            impl RticSwTask for OnCore1 {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let args = quote!(
        device = mypac,
        cores = 2,
        core_ids = [4, 5],
        external_cores = [7],
        ipc_dispatchers = [[IRQ_0], [IRQ_1]]
    );
    XbinPass::disabled()
        .run_pass(args, app_mod)
        .expect("one nested list per core with one entry per cross line");
}

#[test]
fn pool_without_cross_lines_must_be_empty() {
    let app_mod: syn::ItemMod = syn::parse_quote!(
        mod app {}
    );
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [7],
        ipc_dispatchers = []
    );
    XbinPass::disabled()
        .run_pass(args, app_mod)
        .expect("an empty pool has one entry per (zero) cross line");
}

#[test]
fn insufficient_pool_names_the_missing_line() {
    let error = rejected(quote!([]));
    assert!(
        error.contains("has the cross line `(source 7, priority 3)`"),
        "{error}"
    );
    assert!(
        error.contains("declare one interrupt per cross line"),
        "{error}"
    );
}

#[test]
fn extra_pool_entries_are_rejected() {
    let error = rejected(quote!([IRQ_A, IRQ_B]));
    assert!(
        error.contains(
            "declares 2 entries for local core 0, but the application has only 1 \
                        cross-binary line(s)"
        ),
        "{error}"
    );
    assert!(error.contains("remove the extra entries"), "{error}");
}

#[test]
fn duplicate_pool_entries_are_rejected() {
    let error = rejected(quote!([IRQ_A, IRQ_A]));
    assert!(
        error.contains("lists `IRQ_A` more than once for local core 0"),
        "{error}"
    );
    assert!(
        error.contains("each cross line needs its own interrupt line"),
        "{error}"
    );
}

#[test]
fn overlapping_dispatchers_entries_are_rejected() {
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [7],
        dispatchers = [IRQ_A],
        ipc_dispatchers = [IRQ_A]
    );
    let error = XbinPass::disabled()
        .run_pass(args, cross_receiver_app())
        .expect_err("a line cannot serve both pools")
        .to_string();
    assert!(
        error.contains(
            "`ipc_dispatchers` entry `IRQ_A` for local core 0 is already listed in \
                        `dispatchers`"
        ),
        "{error}"
    );
}

#[test]
fn pool_shape_errors_are_rejected() {
    let cases: Vec<(TokenStream, &str)> = vec![
        (quote!(IRQ_A), "must be an array of interrupt paths"),
        (
            quote!([IRQ_A, IRQ_B]),
            "list of one interrupt array per core when `cores > 1`",
        ),
        (
            quote!([[IRQ_A], 3]),
            "must be interrupt paths or arrays of interrupt paths",
        ),
    ];

    for (ipc, expected) in cases {
        let args = quote!(
            device = mypac,
            cores = 2,
            core_ids = [4, 5],
            external_cores = [7],
            ipc_dispatchers = #ipc
        );
        let app_mod: syn::ItemMod = syn::parse_quote!(
            mod app {}
        );
        let error = XbinPass::disabled()
            .run_pass(args, app_mod)
            .expect_err("malformed pool")
            .to_string();
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }

    // The nested shape must have exactly one list per core.
    let args = quote!(
        device = mypac,
        cores = 3,
        core_ids = [4, 5, 6],
        external_cores = [7],
        ipc_dispatchers = [[IRQ_A], [IRQ_B]]
    );
    let error = XbinPass::disabled()
        .run_pass(
            args,
            syn::parse_quote!(
                mod app {}
            ),
        )
        .expect_err("one list per core")
        .to_string();
    assert!(
        error.contains(
            "number of cores `3` does not match the number of `ipc_dispatchers` lists `2`"
        ),
        "{error}"
    );
}

/// Software-task backend of the pipeline test (mirrors the `core_ids`
/// acceptance test's stub).
struct TestSwBackend;

impl SwPassBackend for TestSwBackend {
    fn queue_path(&self) -> syn::Path {
        syn::parse_quote!(rticx::export::Queue)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: syn::ItemFn) -> syn::ItemFn {
        empty_body_fn.block = Box::new(syn::parse_quote!({
            mock_local_pend(irq_nbr);
        }));
        empty_body_fn
    }

    fn generate_cross_pend_fn(
        &self,
        _core: u32,
        mut empty_body_fn: syn::ItemFn,
    ) -> Option<syn::ItemFn> {
        empty_body_fn.block = Box::new(syn::parse_quote!({
            mock_cross_pend(irq_nbr);
        }));
        Some(empty_body_fn)
    }

    fn current_core_id(&self) -> Option<syn::Expr> {
        Some(syn::parse_quote!(mock_current_core_id()))
    }
}

#[test]
fn software_pass_only_sees_its_own_dispatchers() {
    // Metadata mode: the cross receiver is recorded in the manifest and its
    // attribute stripped before `rticx-sw-pass` runs, so `assign_dispatchers`
    // only ever sees the local software task's `dispatchers` entry.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[init(core = 0)]
            fn init0() -> TaskInitsCore0 {
                TaskInitsCore0 {}
            }

            #[sw_task(priority = 2, core = 0)]
            struct Local;

            impl RticSwTask for Local {
                type SpawnInput = u32;
                fn exec(&mut self, _input: u32) {}
            }

            #[sw_task(priority = 3, spawn_by = 7)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::with_manifest(dir.path(), "app", "app"));
    builder.bind_pre_core_pass(SoftwarePass::new(TestSwBackend));

    let code = builder
        .build_rtic_macro2(
            quote!(
                device = mypac,
                cores = 1,
                core_ids = [1],
                external_cores = [7],
                dispatchers = [SW_IRQ],
                ipc_dispatchers = [XBIN_IRQ]
            ),
            app_mod,
            None,
        )
        .to_string();

    assert!(!code.contains("compile_error"), "pipeline failed: {code}");
    assert!(
        code.contains("mypac :: Interrupt :: SW_IRQ") && code.contains("fn SW_IRQ ()"),
        "`assign_dispatchers` must bind the software pool's entry:\n{code}"
    );
    assert!(
        !code.contains("XBIN_IRQ"),
        "the ipc pool leaked into the software pass:\n{code}"
    );
    assert!(
        !code.contains("ipc_dispatchers"),
        "the ipc pool reached the core pass:\n{code}"
    );
}
