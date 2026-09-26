//! Acceptance tests for the task attribute syntax (M1-T3).
//!
//! Covers the confirmed syntax of `multibinary-multicore-plan.md` §6.4/§6.5:
//!
//! - `#[app(core_ids = [..], external_cores = [..])]` and the
//!   `#[cross_bin_task(..)]` / `#[cross_bin_spawn(..)]` task attributes parse
//!   into declarations with the documented defaults;
//! - unknown and malformed keys fail with precise errors;
//! - the extension pass strips its syntax before the core pass runs;
//! - a distribution *without* the extension pass only warns about the `#[app]`
//!   extensions (it never errors on them).

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{RticMacroBuilder, RticPass};
use rticx_xbin_pass::XbinPass;
use rticx_xbin_proto::AppManifest;

/// Runs the pass (no metadata file) and returns its output.
fn run(args: TokenStream, app_mod: syn::ItemMod) -> syn::Result<(TokenStream, syn::ItemMod)> {
    XbinPass::disabled().run_pass(args, app_mod)
}

/// Runs the pass and returns the error message of a rejected input.
fn rejected(args: TokenStream, app_mod: syn::ItemMod) -> String {
    run(args, app_mod)
        .expect_err("the pass must reject this syntax")
        .to_string()
}

/// Runs the pass in metadata mode and returns the parsed manifest.
fn manifest_for(args: TokenStream, app_mod: syn::ItemMod) -> (tempfile::TempDir, AppManifest) {
    let dir = tempfile::tempdir().expect("tempdir");
    XbinPass::with_manifest(dir.path(), "app", "app")
        .run_pass(args, app_mod)
        .expect("metadata pass succeeds");
    let source =
        std::fs::read_to_string(dir.path().join("app.xbin.json")).expect("manifest is written");
    (
        dir,
        AppManifest::from_json(&source).expect("manifest parses"),
    )
}

#[test]
fn receiver_syntax_parses_all_keys_and_defaults() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_task(core = 1, priority = 3, capacity = 2, spawned_by = [0, 2])]
            struct Full;

            impl CrossBinTask for Full {
                type Input = ipc_types::FullReq;
                fn exec(&mut self, _input: Self::Input) {}
            }

            #[cross_bin_task]
            struct Bare;

            impl CrossBinTask for Bare {
                type Input = ipc_types::BareMsg;
            }
        }
    };
    let (_dir, manifest) = manifest_for(
        quote!(device = mypac, cores = 2, core_ids = [4, 5]),
        app_mod,
    );

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
    assert_eq!(bare.spawned_by, None);
    assert_eq!(bare.input_type, "ipc_types::BareMsg");

    let full = &manifest.receivers[1];
    assert_eq!(full.priority, 3);
    assert_eq!(full.capacity, 2);
    assert_eq!(full.core, 1, "receiver `core` is a local index");
    assert_eq!(full.spawned_by.as_deref(), Some(&[0, 2][..]));
    assert_eq!(full.input_type_name(), "FullReq");
}

#[test]
fn sender_syntax_parses_all_keys_and_defaults() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
            struct WithInput;

            impl CrossBinSpawn for WithInput {
                type Input = ipc_types::EncryptReq;
            }

            #[cross_bin_spawn(core = 2)]
            struct WithoutInput;
        }
    };
    let (_dir, manifest) = manifest_for(quote!(device = mypac, cores = 1, core_ids = [0]), app_mod);

    assert_eq!(manifest.senders.len(), 2);
    let with_input = &manifest.senders[0];
    assert_eq!(with_input.name, "WithInput");
    assert_eq!(with_input.core, 1, "sender `core` is the global target id");
    assert_eq!(with_input.priority, 3);
    assert_eq!(with_input.capacity, 2);
    assert_eq!(with_input.input_type_name(), Some("EncryptReq"));

    let without_input = &manifest.senders[1];
    assert_eq!(without_input.name, "WithoutInput");
    assert_eq!(without_input.core, 2);
    assert_eq!(without_input.priority, 1, "priority defaults to 1");
    assert_eq!(without_input.capacity, 1, "capacity defaults to 1");
    assert_eq!(without_input.input_type, None, "sender input is optional");
}

#[test]
fn extension_syntax_is_stripped_before_the_next_pass() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[derive(Debug)]
            #[cross_bin_spawn(core = 1, priority = 3)]
            struct EncryptTask;
        }
    };
    let (args, out) = run(
        quote!(
            device = mypac,
            cores = 1,
            core_ids = [0],
            external_cores = [1]
        ),
        app_mod,
    )
    .expect("pass succeeds");

    let args = args.to_string();
    assert!(args.contains("device"), "{args}");
    assert!(args.contains("cores"), "{args}");
    assert!(!args.contains("core_ids"), "consumed: {args}");
    assert!(!args.contains("external_cores"), "consumed: {args}");

    let out = out.to_token_stream().to_string();
    assert!(!out.contains("cross_bin_spawn"), "stripped: {out}");
    assert!(
        out.contains("derive"),
        "unrelated attributes survive: {out}"
    );
    assert!(out.contains("struct EncryptTask"), "{out}");
}

#[test]
fn unknown_app_arguments_are_left_for_other_distributions() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1)]
            struct EncryptTask;
        }
    };
    let (args, _) =
        run(quote!(device = mypac, cores = 1, custom_key = 7), app_mod).expect("pass succeeds");
    assert!(
        args.to_string().contains("custom_key"),
        "arguments owned by other distributions are not consumed: {args}"
    );
}

#[test]
fn task_attributes_only_apply_to_structs() {
    let function: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_task(priority = 3)]
            fn not_a_struct() {}
        }
    };
    let error = rejected(quote!(device = mypac), function);
    assert!(error.contains("must be applied to a struct"), "{error}");

    let enumeration: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1)]
            enum NotAStruct {}
        }
    };
    let error = rejected(quote!(device = mypac), enumeration);
    assert!(error.contains("must be applied to a struct"), "{error}");
}

#[test]
fn both_task_attributes_on_one_item_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_task(priority = 3)]
            #[cross_bin_spawn(core = 1)]
            struct EncryptTask;
        }
    };
    let error = rejected(quote!(device = mypac), app_mod);
    assert!(
        error.contains("at most one of `#[cross_bin_task]` and `#[cross_bin_spawn]`"),
        "{error}"
    );
}

#[test]
fn unknown_task_keys_are_rejected_with_the_supported_list() {
    let receiver: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_task(priority = 3, bogus = 1)]
            struct EncryptTask;
        }
    };
    let error = rejected(quote!(device = mypac), receiver);
    assert!(
        error.contains(
            "unknown argument `bogus`; expected one of: priority, capacity, spawned_by, core"
        ),
        "{error}"
    );

    let sender: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1, bogus = 1)]
            struct EncryptTask;
        }
    };
    let error = rejected(quote!(device = mypac), sender);
    assert!(
        error.contains("unknown argument `bogus`; expected one of: core, priority, capacity"),
        "{error}"
    );
}

#[test]
fn duplicate_task_keys_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1, core = 2)]
            struct EncryptTask;
        }
    };
    let error = rejected(quote!(device = mypac), app_mod);
    assert!(
        error.contains("duplicate argument `core` in `cross_bin_spawn`"),
        "{error}"
    );
}

#[test]
fn multi_segment_task_keys_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1, foo::bar = 2)]
            struct EncryptTask;
        }
    };
    let error = rejected(quote!(device = mypac), app_mod);
    assert!(
        error.contains("`cross_bin_spawn` arguments must use single-segment keys"),
        "{error}"
    );
}

#[test]
fn malformed_task_values_are_rejected() {
    let cases: Vec<(syn::ItemMod, &str)> = vec![
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_task(priority = "high")]
                    struct Task;
                }
            },
            "`priority` must be an integer literal",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_task(capacity = 0)]
                    struct Task;
                    impl CrossBinTask for Task {
                        type Input = ipc_types::Msg;
                    }
                }
            },
            "The `capacity` argument must be at least 1.",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_task(spawned_by = 3)]
                    struct Task;
                    impl CrossBinTask for Task {
                        type Input = ipc_types::Msg;
                    }
                }
            },
            "`spawned_by` must be an array of integers",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_task(priority = 3)]
                    struct Task;
                }
            },
            "must have an `impl CrossBinTask for Task` block declaring `type Input = …;`",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_spawn(priority = 3)]
                    struct Task;
                }
            },
            "sender stubs must declare the global target core id",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_spawn]
                    struct Task;
                }
            },
            "sender stubs must declare the global target core id",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_spawn(core = "one")]
                    struct Task;
                }
            },
            "`core` must be an integer literal",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_spawn(core = 1, capacity = 0)]
                    struct Task;
                }
            },
            "The `capacity` argument must be at least 1.",
        ),
        (
            syn::parse_quote! {
                mod app {
                    #[cross_bin_task = "receiver"]
                    struct Task;
                }
            },
            "expected attribute of the form `#[name(key = value, ...)]`",
        ),
    ];

    for (app_mod, expected) in cases {
        let error = rejected(quote!(device = mypac), app_mod);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn receiver_core_must_be_a_local_core_index() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_task(core = 2, priority = 3)]
            struct EncryptTask;

            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;
            }
        }
    };
    let error = rejected(
        quote!(device = mypac, cores = 2, core_ids = [4, 5]),
        app_mod,
    );
    assert!(
        error.contains("declares local `core = 2`, but the application has only 2 local core(s)"),
        "{error}"
    );
}

#[test]
fn duplicate_input_impls_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;
            }

            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;
            }
        }
    };
    let error = rejected(quote!(device = mypac), app_mod);
    assert!(
        error.contains("duplicate `impl CrossBinTask` for `EncryptTask`"),
        "{error}"
    );
}

#[test]
fn malformed_app_extensions_are_rejected() {
    let cases: Vec<(TokenStream, &str)> = vec![
        (
            quote!(device = mypac, core_ids = 1),
            "`core_ids` must be an array of integers",
        ),
        (
            quote!(device = mypac, core_ids = [x]),
            "`core_ids` entries must be integer literals",
        ),
        (
            quote!(
                device = mypac,
                cores = 1,
                core_ids = [0],
                external_cores = [0]
            ),
            "`external_cores` lists global core id 0, which this application owns via `core_ids`",
        ),
        (
            quote!(
                device = mypac,
                cores = 1,
                core_ids = [0],
                external_cores = [1, 1]
            ),
            "`external_cores` lists global core id 1 more than once",
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
        let error = rejected(args, app_mod);
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn duplicate_app_arguments_are_rejected() {
    let app_mod: syn::ItemMod = syn::parse_quote!(
        mod app {}
    );
    let error = rejected(quote!(device = mypac, device = other), app_mod);
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
    // A distribution that does not bind the extension pass: `core_ids` and
    // `external_cores` survive into the core pass, which must warn (the
    // `#[deprecated]` trick) instead of erroring.
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
    assert!(code.contains("rticx_warn_unknown_app_core_ids"), "{code}");
    assert!(
        code.contains("rticx_warn_unknown_app_external_cores"),
        "{code}"
    );

    // With the extension pass bound the same arguments are consumed, so no
    // warning items are generated.
    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::disabled());
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
    assert!(!code.contains("rticx_warn_unknown_app"), "{code}");
}
