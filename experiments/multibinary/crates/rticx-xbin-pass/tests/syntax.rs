//! Acceptance tests for the native cross-binary task syntax.
//!
//! - the project topology (`cores`/`core_ids`/`external_cores`) comes from
//!   `rticx.toml` and is injected into the arguments by the pass (M7-T2); the
//!   native `#[sw_task(..)]` + `impl RticSwTask { type SpawnInput = …; }`
//!   receiver syntax parses into declarations with the documented defaults;
//! - `spawn_by` in an application whose topology declares external cores
//!   resolves in the global namespace: in-app ids are mapped back to local
//!   indexes, external ids classify the task as a cross receiver and unknown
//!   ids are rejected;
//! - without external cores the native local reading is kept;
//! - `init = generated` is accepted for parity with the native `#[sw_task]`
//!   syntax (the pass always generates the receiver's init); other `init`
//!   values are rejected;
//! - unknown and malformed keys fail with precise errors, and `#[app(core_ids)]`
//!   / `#[app(external_cores)]` are rejected with the migration errors;
//! - metadata mode strips the parsed receiver attributes, codegen mode is
//!   covered by the codegen snapshot tests;
//! - a distribution *without* the extension pass only warns about the `#[app]`
//!   extensions (it never errors on them).

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{RticMacroBuilder, RticPass};
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

/// The topology of a two-core application with the global mapping `[4, 5]`
/// and the external core `7`.
fn mapped_topology() -> AppTopology {
    AppTopology::new(vec![4, 5], vec![7])
}

/// The arguments of a two-core application; the core mapping comes from the
/// project topology (M7-T2).
fn mapped_args() -> TokenStream {
    quote!(device = mypac)
}

/// Like [`mapped_args`] with the cross lines of the receiver fixtures: local
/// core 0 carries `(7, 1)`, local core 1 carries `(7, 3)` (M6.5-T1).
fn receiver_args() -> TokenStream {
    quote!(device = mypac, ipc_dispatchers = [[IRQ_IN_0], [IRQ_IN_1]])
}

/// Like [`mapped_args`] with the cross line of a single receiver on local core
/// 0 (`(7, 3)`); local core 1 has no cross line.
fn one_line_args() -> TokenStream {
    quote!(device = mypac, ipc_dispatchers = [[IRQ_IN_0], []])
}

#[test]
fn receiver_syntax_parses_all_keys_and_defaults() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            struct Msg;

            #[sw_task(core = 1, priority = 3, capacity = 2, spawn_by = 7, init = generated)]
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
    let (_dir, manifest) = manifest_for(mapped_topology(), receiver_args(), app_mod);

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
    assert_eq!(bare.core, 0, "receiver `core` defaults to local core 0");
    assert_eq!(bare.spawn_by, 7, "the global producer id is recorded");
    assert_eq!(bare.input_type, "ipc_types::BareMsg");

    let full = &manifest.receivers[1];
    assert_eq!(full.priority, 3);
    assert_eq!(full.capacity, 2);
    assert_eq!(full.core, 1, "receiver `core` is a local index");
    assert_eq!(full.spawn_by, 7);
    assert_eq!(full.input_type_name(), "FullReq");
}

#[test]
fn identity_default_keeps_the_native_local_spawn_by() {
    // Without `external_cores` the native local-index reading applies: the
    // task is not a cross receiver and `spawn_by` stays local. The identity
    // `core_ids` default keeps existing syntax unchanged.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(core = 0, spawn_by = 1)]
            struct Local;

            impl RticSwTask for Local {
                type SpawnInput = u32;
            }
        }
    };
    let args = quote!(device = mypac);
    let (_dir, manifest) = manifest_for(AppTopology::identity(2), args.clone(), app_mod.clone());
    assert_eq!(
        manifest.core_ids.as_deref(),
        Some(&[0, 1][..]),
        "the identity mapping is recorded"
    );
    assert!(manifest.receivers.is_empty(), "no cross receiver");

    let (_, out) = run(AppTopology::identity(2), args, app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(tokens.contains("spawn_by = 1"), "kept local: {tokens}");
}

#[test]
fn local_spawn_by_stays_local_with_an_identity_mapping_and_external_cores() {
    // An application that declares `external_cores` resolves `spawn_by` in
    // the global namespace; with the identity mapping, global ids equal
    // local indexes, so `spawn_by = 1` stays 1.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(core = 0, spawn_by = 1)]
            struct Local;

            impl RticSwTask for Local {
                type SpawnInput = u32;
            }
        }
    };
    let args = quote!(device = mypac);
    let topology = AppTopology::new(vec![0, 1], vec![7]);
    let (_dir, manifest) = manifest_for(topology.clone(), args.clone(), app_mod.clone());
    assert!(
        manifest.receivers.is_empty(),
        "in-app, not a cross receiver"
    );

    let (_, out) = run(topology, args, app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(tokens.contains("spawn_by = 1"), "kept local: {tokens}");
}

#[test]
fn in_app_global_spawn_by_is_mapped_back_to_the_local_index() {
    // `spawn_by = 5` is one of this application's own `core_ids`: an in-app
    // cross-core task, mapped to local index 1 for the software pass.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(core = 0, spawn_by = 5)]
            struct InApp;

            impl RticSwTask for InApp {
                type SpawnInput = u32;
            }
        }
    };
    let (_dir, manifest) = manifest_for(mapped_topology(), mapped_args(), app_mod.clone());
    assert!(manifest.receivers.is_empty(), "not a cross receiver");

    let (_, out) = run(mapped_topology(), mapped_args(), app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(
        tokens.contains("spawn_by = 1"),
        "the global id is mapped to the local index: {tokens}"
    );
    assert!(!tokens.contains("spawn_by = 5"), "{tokens}");
}

#[test]
fn external_spawn_by_classifies_and_strips_the_receiver() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(priority = 3, spawn_by = 7)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };

    // The manifest records the global producer.
    let (_dir, manifest) = manifest_for(mapped_topology(), one_line_args(), app_mod.clone());
    assert_eq!(manifest.receivers.len(), 1);
    assert_eq!(manifest.receivers[0].spawn_by, 7);
    assert_eq!(manifest.types, ["ipc_types::Msg"]);

    // The emitted module strips `spawn_by` so the software pass never sees an
    // external id.
    let (_, out) = run(mapped_topology(), one_line_args(), app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(!tokens.contains("spawn_by"), "{tokens}");
    assert!(tokens.contains("impl RticSwTask for Cross"), "{tokens}");
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
    let tokens = metadata_module(mapped_topology(), one_line_args(), app_mod);

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
fn unknown_spawn_by_core_is_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[sw_task(spawn_by = 9)]
            struct Cross;

            impl RticSwTask for Cross {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    let error = rejected(mapped_topology(), mapped_args(), app_mod);
    assert!(
        error.contains("receiver `Cross` declares `spawn_by = 9`"),
        "{error}"
    );
    assert!(error.contains("not a known core"), "{error}");
    assert!(error.contains("core_ids"), "{error}");
    assert!(error.contains("external_cores"), "{error}");
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
    let error = rejected(mapped_topology(), mapped_args(), app_mod);
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
                    #[sw_task(core = 2, spawn_by = 7)]
                    struct Task;

                    impl RticSwTask for Task {
                        type SpawnInput = ipc_types::Msg;
                    }
                }
            },
            "declares local `core = 2`, but the application has only 2 local core(s)",
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
        let error = rejected(mapped_topology(), mapped_args(), app_mod);
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
    let error = rejected(mapped_topology(), mapped_args(), app_mod);
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
    let error = rejected(mapped_topology(), mapped_args(), app_mod);
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
    let error = rejected(mapped_topology(), mapped_args(), app_mod);
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
    let error = rejected(mapped_topology(), mapped_args(), app_mod);
    assert!(
        error.contains("duplicate `impl RticSwTask` for `EncryptTask`"),
        "{error}"
    );
}

#[test]
fn malformed_app_extensions_are_rejected() {
    let cases: Vec<(TokenStream, &str)> = vec![
        (
            quote!(device = mypac, core_ids = 1),
            "`core_ids` moved to `rticx.toml`",
        ),
        (
            quote!(device = mypac, core_ids = [x]),
            "`core_ids` moved to `rticx.toml`",
        ),
        (
            quote!(device = mypac, external_cores = [1, 1]),
            "`external_cores` moved to `rticx.toml`",
        ),
        (
            quote!(device = mypac, cores = "two"),
            "`cores` must be an integer literal",
        ),
    ];

    for (args, expected) in cases {
        let app_mod: syn::ItemMod = syn::parse_quote!(
            mod app {}
        );
        let error = rejected(mapped_topology(), args, app_mod);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn declared_cores_must_agree_with_the_project_mapping() {
    let app_mod: syn::ItemMod = syn::parse_quote!(
        mod app {}
    );
    let error = XbinPass::with_system("/nonexistent/system.json", "app", "app")
        .with_topology(AppTopology::new(vec![4, 5], vec![7]))
        .run_pass(quote!(device = mypac, cores = 1), app_mod)
        .expect_err("`cores` must equal `core_ids.len()`")
        .to_string();
    assert!(error.contains("`app` declares `cores = 1`"), "{error}");
    assert!(
        error.contains("rticx.toml maps 2 core(s) via `core_ids`"),
        "{error}"
    );
    assert!(error.contains("single-binary multicore"), "{error}");
}

#[test]
fn duplicate_app_arguments_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote!(
        mod app {}
    );
    let error = rejected(
        mapped_topology(),
        quote!(device = mypac, device = other),
        app_mod,
    );
    assert!(
        error.contains("duplicate argument `device` in `app`"),
        "{error}"
    );
}

/// A minimal application acceptable to the core pass (no cross-binary
/// declarations).
fn plain_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits { uart_task: UartTask, idle: Idle })
            }

            #[task(binds = UART, priority = 2, shared = [counter])]
            struct UartTask;

            impl RticTask for UartTask {
                fn exec(&mut self) {}
            }

            #[idle]
            struct Idle;

            impl RticIdleTask for Idle {
                fn exec(&mut self) -> ! {
                    loop {}
                }
            }
        }
    }
}

#[test]
fn other_distributions_only_warn_about_app_extensions() {
    // A distribution that does not bind the extension pass: `external_cores`
    // survives into the core pass, which must warn (the `#[deprecated]` trick)
    // instead of erroring. `core_ids` is native to `rticx-core` since M5, so
    // it is consumed without a warning even without the extension pass.
    let builder = RticMacroBuilder::new(MockCoreBackend);
    let code = builder
        .build_rtic_macro2(
            quote!(
                device = mypac,
                cores = 1,
                core_ids = [0],
                external_cores = [1]
            ),
            plain_app(),
            None,
        )
        .to_string();
    assert!(!code.contains("compile_error"), "{code}");
    assert!(!code.contains("rticx_warn_unknown_app_core_ids"), "{code}");
    assert!(
        code.contains("rticx_warn_unknown_app_external_cores"),
        "{code}"
    );

    // With the extension pass bound the arguments are consumed, so no warning
    // items are generated (M7-T2: `core_ids`/`external_cores` are resolved
    // from `rticx.toml`).
    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::disabled().with_topology(AppTopology::identity(1)));
    let code = builder
        .build_rtic_macro2(
            quote!(device = mypac, ipc_dispatchers = []),
            plain_app(),
            None,
        )
        .to_string();
    assert!(!code.contains("compile_error"), "{code}");
    assert!(!code.contains("rticx_warn_unknown_app"), "{code}");
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
    let (_, out) = run(mapped_topology(), one_line_args(), app_mod).expect("pass succeeds");
    let tokens = out.to_token_stream().to_string();
    assert!(tokens.contains("shared = [counter]"), "{tokens}");
}
