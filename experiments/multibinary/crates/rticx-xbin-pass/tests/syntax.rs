//! Acceptance tests for the native cross-binary task syntax.
//!
//! - the project topology (the application's single `core_id`) comes from
//!   `rticx.toml` and is injected into the arguments by the pass; the
//!   native `#[sw_task(..)]` + `impl RticSwTask { type SpawnInput = …; }`
//!   receiver syntax parses into declarations with the documented defaults;
//! - `spawn_by` always names a **global** producer core id, so any
//!   `#[sw_task(spawn_by = G)]` with `G != core_id` is a cross-binary receiver;
//!   `spawn_by == core_id` is a core-local task and a hard error;
//! - a `#[sw_task]` without `spawn_by` stays a plain local software task;
//! - `init = generated` is accepted for parity with the native `#[sw_task]`
//!   syntax (the pass always generates the receiver's init); other `init`
//!   values are rejected;
//! - metadata mode strips the parsed receiver attributes, codegen mode is
//!   covered by the codegen snapshot tests;
//! - a distribution *without* the extension pass only warns about the `#[app]`
//!   extensions (it never errors on them).

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_core::RticPass;
use rticx_xbin_pass::{AppTopology, XbinPass};
use rticx_xbin_proto::AppManifest;

/// Runs the pass (no metadata file) with `topology` and returns its output.
fn run(
    topology: AppTopology,
    args: TokenStream,
    app_mod: syn::ItemMod,
) -> syn::Result<(TokenStream, syn::ItemMod)> {
    XbinPass::disabled()
        .with_topology(topology)
        .run_pass(args, app_mod)
}

/// Runs the pass and returns the error message of a rejected input.
fn rejected(topology: AppTopology, args: TokenStream, app_mod: syn::ItemMod) -> String {
    run(topology, args, app_mod)
        .expect_err("the pass must reject this syntax")
        .to_string()
}

/// Runs the pass in metadata mode and returns the parsed manifest.
fn manifest_for(
    topology: AppTopology,
    args: TokenStream,
    app_mod: syn::ItemMod,
) -> (tempfile::TempDir, AppManifest) {
    let dir = tempfile::tempdir().expect("tempdir");
    XbinPass::with_manifest(dir.path(), "app", "app")
        .with_topology(topology)
        .run_pass(args, app_mod)
        .expect("metadata pass succeeds");
    let source =
        std::fs::read_to_string(dir.path().join("app.xbin.json")).expect("manifest is written");
    (
        dir,
        AppManifest::from_json(&source).expect("manifest parses"),
    )
}

/// Runs the pass in metadata mode and returns the emitted module as tokens.
fn metadata_module(topology: AppTopology, args: TokenStream, app_mod: syn::ItemMod) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_, module) = XbinPass::with_manifest(dir.path(), "app", "app")
        .with_topology(topology)
        .run_pass(args, app_mod)
        .expect("metadata pass succeeds");
    module.to_token_stream().to_string()
}

/// The topology of the receiver fixtures: the application's own global core is
/// `1`, so a `spawn_by = 7` names another binary's core `7`.
fn receiver_topology() -> AppTopology {
    AppTopology::new(1)
}

/// The arguments of the receiver fixtures: the cross line `(7, priority)`.
fn receiver_args() -> TokenStream {
    quote!(device = mypac, dispatchers = [IRQ_IN])
}

/// Like [`receiver_args`] for a fixture with two cross lines: `(7, 1)` and
/// `(7, 3)`, so `dispatchers` declares one entry per line.
fn two_line_args() -> TokenStream {
    quote!(device = mypac, dispatchers = [IRQ_LOW, IRQ_HIGH])
}

#[test]
fn receiver_syntax_parses_all_keys_and_defaults() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            struct Msg;

            #[sw_task(priority = 3, capacity = 2, spawn_by = 7, init = generated)]
            struct Full;

            impl RticSwTask for Full {
                type SpawnInput = ipc_types::FullReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(spawn_by = 7)]
            struct Bare;

            impl RticSwTask for Bare {
                type SpawnInput = ipc_types::BareMsg;
            }
        }
    };
    let (_dir, manifest) = manifest_for(receiver_topology(), two_line_args(), app_mod);

    assert_eq!(
        manifest
            .receivers
            .iter()
            .map(|receiver| receiver.name.as_str())
            .collect::<Vec<_>>(),
        ["Bare", "Full"],
        "declarations are sorted by name"
    );

    let bare = &manifest.receivers[0];
    assert_eq!(bare.priority, 1, "priority defaults to 1");
    assert_eq!(bare.capacity, 1, "capacity defaults to 1");
    assert_eq!(bare.spawn_by, 7, "the global producer id is recorded");
    assert_eq!(bare.input_type, "ipc_types::BareMsg");

    let full = &manifest.receivers[1];
    assert_eq!(full.priority, 3);
    assert_eq!(full.capacity, 2);
    assert_eq!(full.spawn_by, 7);
    assert_eq!(full.input_type_name(), "FullReq");
}

#[test]
fn any_spawn_by_is_a_cross_binary_receiver() {
    // With one core per binary there is no in-app cross-core task anymore:
    // any `spawn_by` that is not the application's own core names a producer
    // in another binary.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 9)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let (_dir, manifest) = manifest_for(receiver_topology(), receiver_args(), app_mod.clone());
    assert_eq!(manifest.receivers.len(), 1);
    assert_eq!(manifest.receivers[0].spawn_by, 9);

    let (_, out) = run(receiver_topology(), receiver_args(), app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(!tokens.contains("spawn_by"), "{tokens}");
}

#[test]
fn spawn_by_naming_the_own_core_id_is_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 1)]
            struct Local;
        }
    };
    let error = rejected(receiver_topology(), quote!(device = mypac), app_mod);
    assert!(
        error.contains("`Local` declares `spawn_by = 1`, which is this application's own core id"),
        "{error}"
    );
    assert!(error.contains("it is a core-local task"), "{error}");
    assert!(error.contains("drop `spawn_by`"), "{error}");
}

#[test]
fn a_task_without_spawn_by_stays_local() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 2)]
            struct Local;

            impl RticSwTask for Local {
                type SpawnInput = u32;
            }
        }
    };
    let (_dir, manifest) =
        manifest_for(receiver_topology(), quote!(device = mypac), app_mod.clone());
    assert!(manifest.receivers.is_empty(), "not a cross receiver");

    let (_, out) =
        run(receiver_topology(), quote!(device = mypac), app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(
        tokens.contains("sw_task"),
        "the local task attribute is left untouched: {tokens}"
    );
}

#[test]
fn receiver_core_key_is_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7, core = 0)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let error = rejected(receiver_topology(), receiver_args(), app_mod);
    assert!(
        error.contains("must not declare `core`"),
        "a receiver always runs on the application's single core: {error}"
    );
    assert!(error.contains("global core 1"), "{error}");
}

#[test]
fn metadata_strips_the_cross_receiver_attributes() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, capacity = 2, spawn_by = 7)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let tokens = metadata_module(receiver_topology(), receiver_args(), app_mod);

    assert!(!tokens.contains("sw_task"), "{tokens}");
    assert!(!tokens.contains("spawn_by"), "{tokens}");
    assert!(
        tokens.contains("struct Cross ;"),
        "the struct itself stays: {tokens}"
    );
    assert!(
        tokens.contains("impl RticSwTask for Cross"),
        "the user impl stays: {tokens}"
    );
}

#[test]
fn spawn_by_arrays_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(spawn_by = [7, 9])]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let error = rejected(receiver_topology(), receiver_args(), app_mod);
    assert!(
        error.contains("`spawn_by` must be a single core id, not an array"),
        "{error}"
    );
    assert!(error.contains("one producer core"), "{error}");
}

#[test]
fn malformed_task_values_are_rejected() {
    let cases: Vec<(syn::ItemMod, &str)> = vec![
        (
            syn::parse_quote! {
                mod app {
                    #[sw_task(priority = "high", spawn_by = 7)]
                    struct Task;

                    impl RticSwTask for Task {
                        type SpawnInput = ipc_types::Msg;
                    }
                }
            },
            "`priority` must be an integer literal",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[sw_task(priority = 0, spawn_by = 7)]
                    struct Task;

                    impl RticSwTask for Task {
                        type SpawnInput = ipc_types::Msg;
                    }
                }
            },
            "requires `priority >= 1`",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[sw_task(capacity = 0, spawn_by = 7)]
                    struct Task;

                    impl RticSwTask for Task {
                        type SpawnInput = ipc_types::Msg;
                    }
                }
            },
            "The `capacity` argument must be at least 1.",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[sw_task(priority = 3, spawn_by = 7)]
                    struct Task;
                }
            },
            "must have an `impl RticSwTask for Task` block declaring `type SpawnInput = …;`",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[sw_task(spawn_by = 7, init = my_init)]
                    struct Task;

                    impl RticSwTask for Task {
                        type SpawnInput = ipc_types::Msg;
                    }
                }
            },
            "must use `init = generated`; its init is generated by the cross-binary pass",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[sw_task = "receiver"]
                    struct Task;
                }
            },
            "expected attribute of the form `#[name(key = value, ...)]`",
        ),
    ];

    for (app_mod, expected) in cases {
        let error = rejected(receiver_topology(), receiver_args(), app_mod);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn unknown_task_keys_are_rejected_with_the_supported_list() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7, bogus = 1)]
            struct Task;

            impl RticSwTask for Task {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let error = rejected(receiver_topology(), receiver_args(), app_mod);
    assert!(
        error.contains(
            "unknown argument `bogus`; expected one of: priority, capacity, core, spawn_by, init, shared"
        ),
        "{error}"
    );
}

#[test]
fn duplicate_task_keys_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(spawn_by = 7, spawn_by = 9)]
            struct Task;

            impl RticSwTask for Task {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let error = rejected(receiver_topology(), receiver_args(), app_mod);
    assert!(
        error.contains("duplicate argument `spawn_by` in `sw_task`"),
        "{error}"
    );
}

#[test]
fn multi_segment_task_keys_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(spawn_by = 7, foo::bar = 2)]
            struct Task;

            impl RticSwTask for Task {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let error = rejected(receiver_topology(), receiver_args(), app_mod);
    assert!(
        error.contains("`sw_task` arguments must use single-segment keys"),
        "{error}"
    );
}

#[test]
fn duplicate_spawn_input_impls_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
            }

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
            }
        }
    };
    let error = rejected(receiver_topology(), quote!(device = mypac), app_mod);
    assert!(
        error.contains("duplicate `impl RticSwTask` for `EncryptTask`"),
        "{error}"
    );
}

#[test]
fn duplicate_app_arguments_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote!(
        mod app {}
    );
    let error = rejected(
        receiver_topology(),
        quote!(device = mypac, device = other),
        app_mod,
    );
    assert!(
        error.contains("duplicate argument `device` in `app`"),
        "{error}"
    );
}

#[test]
fn shared_is_preserved_for_the_software_pass() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7, shared = [counter])]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let (_, out) = run(receiver_topology(), receiver_args(), app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(tokens.contains("shared = [counter]"), "{tokens}");
}
