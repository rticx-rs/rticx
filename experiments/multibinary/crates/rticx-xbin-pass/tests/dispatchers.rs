//! Acceptance tests for the shared `dispatchers` pool.
//!
//! The xbin pass no longer owns a separate `ipc_dispatchers` argument: it
//! **steals the front** of the software pass's `dispatchers` argument — one
//! entry per cross line of the application, in ascending `(source, priority)`
//! order — and leaves the remainder in the arguments for
//! `rticx-sw-pass`/`rticx-async-pass`. The list is **flat** (an application
//! owns exactly one core; the nested one-array-per-core shape was removed).
//! Covers the flat shape, the error when the list is too short, duplicate
//! rejection inside the stolen prefix, surplus entries (local dispatchers), the
//! removed `ipc_dispatchers` key staying untouched, and that
//! `rticx-sw-pass`'s `assign_dispatchers` only ever sees the leftovers.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use rticx_core::RticMacroBuilder;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::parse_utils::RticAttr;
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
use rticx_xbin_pass::{AppTopology, RticPass, XbinPass};

/// A single-core application with one native cross receiver: one cross line
/// `(source 7, priority 3)`.
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

/// A single-core application with two cross lines: `(source 7, priority 1)`
/// and `(source 7, priority 3)`.
fn two_line_cross_receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7)]
            struct High;

            impl RticSwTask for High {
                type SpawnInput = ipc_types::Msg;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(priority = 1, spawn_by = 7)]
            struct Low;

            impl RticSwTask for Low {
                type SpawnInput = ipc_types::Msg;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    }
}

/// The `#[app]` arguments of a single-core application with
/// `dispatchers = <dispatchers>`.
fn receiver_args(dispatchers: TokenStream) -> TokenStream {
    quote!(
        device = mypac,
        dispatchers = #dispatchers
    )
}

/// The receiver's topology: the application owns global core `1`, so
/// `spawn_by = 7` names another binary's core.
fn receiver_topology() -> AppTopology {
    AppTopology::new(1)
}

/// Runs the syntax-only pass over the receiver fixture.
fn run(dispatchers: TokenStream) -> syn::Result<(TokenStream, syn::ItemMod)> {
    XbinPass::disabled()
        .with_topology(receiver_topology())
        .run_pass(receiver_args(dispatchers), cross_receiver_app())
}

/// Returns the error message of a rejected `dispatchers` argument.
fn rejected(dispatchers: TokenStream) -> String {
    run(dispatchers)
        .expect_err("the pass must reject this dispatchers argument")
        .to_string()
}

/// The `dispatchers` expression left in the returned arguments, rendered as
/// canonical (whitespace-free) tokens (`""` when the key is absent).
fn remaining_dispatchers(args: TokenStream) -> String {
    RticAttr::parse_from_tokens(args, format_ident!("app"))
        .expect("the returned args stay parseable")
        .get_expr("dispatchers")
        .map(|expr| expr.to_token_stream().to_string().replace(' ', ""))
        .unwrap_or_default()
}

#[test]
fn flat_dispatchers_steal_the_cross_prefix_and_keep_the_remainder() {
    // One cross line: the first entry is stolen, the rest stay for the
    // software pass.
    let (args, _) = run(quote!([XBIN_IRQ, SW_IRQ])).expect("one cross line steals one entry");
    assert_eq!(remaining_dispatchers(args), "[SW_IRQ]");
}

#[test]
fn nested_dispatchers_are_rejected() {
    // An application owns exactly one core: the nested one-array-per-core shape
    // was removed.
    let error = rejected(quote!([[IRQ_0], [IRQ_1]]));
    assert!(
        error.contains("must be a flat list of interrupt paths"),
        "{error}"
    );
    assert!(
        error.contains("the nested one-array-per-core shape was removed"),
        "{error}"
    );
}

#[test]
fn ipc_dispatchers_is_not_consumed() {
    // The removed key is left in the arguments: the core pass reports it as an
    // unknown `#[app]` argument.
    let args = quote!(
        device = mypac,
        dispatchers = [XBIN_IRQ],
        ipc_dispatchers = [STALE]
    );
    let (args, _) = XbinPass::disabled()
        .with_topology(receiver_topology())
        .run_pass(args, cross_receiver_app())
        .expect("the removed key is left for the core pass");
    let args = args.to_string();
    assert!(
        args.contains("ipc_dispatchers") && args.contains("STALE"),
        "`ipc_dispatchers` must survive: {args}"
    );
}

#[test]
fn insufficient_dispatchers_name_the_missing_line() {
    let error = rejected(quote!([]));
    assert!(
        error.contains("has the cross line `(source 7, priority 3)`"),
        "{error}"
    );
    assert!(
        error.contains("declare one interrupt per cross line") && error.contains("`dispatchers`"),
        "{error}"
    );
}

#[test]
fn surplus_entries_are_local_dispatchers() {
    // Entries beyond the cross lines are not an error: they are the local
    // software/async task dispatchers, left for the next pass.
    let (args, _) =
        run(quote!([XBIN_IRQ, SW_IRQ, SW_IRQ_B])).expect("surplus entries are local dispatchers");
    assert_eq!(remaining_dispatchers(args), "[SW_IRQ,SW_IRQ_B]");
}

#[test]
fn duplicate_stolen_entries_are_rejected() {
    // Two distinct cross lines steal the first two entries; a duplicate in
    // that prefix would bind one interrupt to two lines.
    let args = quote!(device = mypac, dispatchers = [IRQ_A, IRQ_A]);
    let error = XbinPass::disabled()
        .with_topology(receiver_topology())
        .run_pass(args, two_line_cross_receiver_app())
        .expect_err("two lines cannot share one interrupt")
        .to_string();
    assert!(error.contains("lists `IRQ_A` more than once"), "{error}");
    assert!(
        error.contains("each cross line needs its own interrupt line"),
        "{error}"
    );
}

#[test]
fn dispatchers_shape_errors_are_rejected() {
    let cases: Vec<(TokenStream, &str)> = vec![
        (quote!(IRQ_A), "must be an array of interrupt paths"),
        (quote!([IRQ_A, 3]), "must be interrupt paths"),
        (quote!([[IRQ_A], 3]), "must be interrupt paths"),
    ];

    for (dispatchers, expected) in cases {
        let args = quote!(device = mypac, dispatchers = #dispatchers);
        let app_mod: syn::ItemMod = syn::parse_quote!(
            mod app {}
        );
        let error = XbinPass::disabled()
            .with_topology(receiver_topology())
            .run_pass(args, app_mod)
            .expect_err("malformed dispatchers")
            .to_string();
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
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
fn software_pass_only_sees_the_leftover_dispatchers() {
    // The cross line steals the first entry (`XBIN_IRQ`); metadata mode strips
    // the cross receiver before `rticx-sw-pass` runs, so `assign_dispatchers`
    // only ever sees the leftover `SW_IRQ` for the local task. That the stolen
    // entry is what the generated cross dispatcher binds is covered by the
    // codegen snapshot tests in `codegen.rs`.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[init]
            fn init() -> TaskInits {
                TaskInits {}
            }

            #[sw_task(priority = 2)]
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
    builder.bind_pre_core_pass(
        XbinPass::with_manifest(dir.path(), "app", "app").with_topology(receiver_topology()),
    );
    builder.bind_pre_core_pass(SoftwarePass::new(TestSwBackend));

    let code = builder
        .build_rtic_macro2(
            quote!(device = mypac, dispatchers = [XBIN_IRQ, SW_IRQ]),
            app_mod,
            None,
        )
        .to_string();

    assert!(!code.contains("compile_error"), "pipeline failed: {code}");
    assert!(
        code.contains("mypac :: Interrupt :: SW_IRQ") && code.contains("fn SW_IRQ ()"),
        "`assign_dispatchers` must bind the leftover entry:\n{code}"
    );
    assert!(
        !code.contains("XBIN_IRQ"),
        "the stolen cross entry leaked into the software pass:\n{code}"
    );
}
