//! Acceptance tests for the metadata mode (M1-T2): a fixture application run
//! through the pass produces the expected `<target>.xbin.json`.
//!
//! Environment-variable tests are serialized behind [`ENV_LOCK`] because the
//! process environment is global.

use std::sync::Mutex;

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{RticMacroBuilder, RticPass};
use rticx_xbin_pass::{META_OUT_ENV, XbinPass};
use rticx_xbin_proto::{AppManifest, Hash64};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A receiver application: declares the task it executes for a remote core.
fn receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
            struct EncryptTask;

            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::Input) {}
            }
        }
    }
}

/// A sender application: declares the stub with which it spawns the task.
fn sender_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
            struct EncryptTask;

            impl CrossBinSpawn for EncryptTask {
                type Input = ipc_types::EncryptReq;
            }
        }
    }
}

fn run_manifest(
    app_mod: syn::ItemMod,
    args: TokenStream,
    package: &str,
    target: &str,
) -> (tempfile::TempDir, AppManifest) {
    let dir = tempfile::tempdir().expect("tempdir");
    let pass = XbinPass::with_manifest(dir.path(), package, target);
    assert!(pass.is_metadata_mode());

    let (stripped_args, _out_mod) = pass
        .run_pass(args, app_mod)
        .expect("metadata pass succeeds");
    assert!(
        stripped_args.to_string().contains("device"),
        "core arguments are handed over"
    );

    let path = dir.path().join(AppManifest::file_name(target));
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing manifest {}: {error}", path.display()));
    let manifest = AppManifest::from_json(&source).expect("manifest parses");
    (dir, manifest)
}

#[test]
fn receiver_fixture_produces_the_expected_manifest() {
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [0]
    );
    let (_dir, manifest) = run_manifest(receiver_app(), args, "app-m4", "m4");

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.package, "app-m4");
    assert_eq!(manifest.target.name, "m4");
    assert_eq!(manifest.cores, 1);
    assert_eq!(manifest.core_ids.as_deref(), Some(&[1][..]));
    assert_eq!(manifest.external_cores, &[0][..]);
    assert_eq!(manifest.types, vec!["ipc_types::EncryptReq".to_string()]);

    assert_eq!(manifest.senders.len(), 0);
    assert_eq!(manifest.receivers.len(), 1);
    let receiver = &manifest.receivers[0];
    assert_eq!(receiver.name, "EncryptTask");
    assert_eq!(receiver.priority, 3);
    assert_eq!(receiver.capacity, 2);
    assert_eq!(receiver.core, 0, "receiver core defaults to 0");
    assert_eq!(receiver.spawned_by.as_deref(), Some(&[0][..]));
    assert_eq!(receiver.input_type, "ipc_types::EncryptReq");
    assert_eq!(receiver.input_type_name(), "EncryptReq");

    let json = manifest.to_json();
    assert!(json.contains("\"schema_version\": 1"), "{json}");
    assert!(json.contains("\"kind\": \"bin\""), "{json}");
    assert!(json.contains("\"source_hash\": \"0x"), "{json}");
}

#[test]
fn sender_fixture_produces_the_expected_manifest() {
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [0],
        external_cores = [1]
    );
    let (_dir, manifest) = run_manifest(sender_app(), args, "app-m7", "m7");

    assert_eq!(manifest.receivers.len(), 0);
    assert_eq!(manifest.senders.len(), 1);
    let sender = &manifest.senders[0];
    assert_eq!(sender.name, "EncryptTask");
    assert_eq!(sender.core, 1, "sender `core` is the global target core id");
    assert_eq!(sender.priority, 3);
    assert_eq!(sender.capacity, 2);
    assert_eq!(sender.input_type.as_deref(), Some("ipc_types::EncryptReq"));
    assert_eq!(sender.input_type_name(), Some("EncryptReq"));
}

#[test]
fn manifests_are_deterministic() {
    let args = quote!(device = mypac, cores = 1, core_ids = [1]);
    let (first_dir, _) = run_manifest(receiver_app(), args.clone(), "app-m4", "m4");
    let (second_dir, _) = run_manifest(receiver_app(), args, "app-m4", "m4");

    let first = std::fs::read(first_dir.path().join("m4.xbin.json")).expect("first manifest");
    let second = std::fs::read(second_dir.path().join("m4.xbin.json")).expect("second manifest");
    assert_eq!(
        first, second,
        "identical inputs must produce identical bytes"
    );
}

#[test]
fn source_hash_covers_the_source_before_stripping() {
    let app_mod = receiver_app();
    let args = quote!(device = mypac, cores = 1, core_ids = [1]);

    let mut expected_source = args.to_string();
    expected_source.push('\n');
    expected_source.push_str(&app_mod.to_token_stream().to_string());
    let expected = Hash64::of(expected_source.as_bytes());

    let (dir, manifest) = run_manifest(app_mod, args, "app-m4", "m4");
    assert_eq!(manifest.source_hash, expected);

    // Reading the manifest back from disk yields the same hash.
    let on_disk = std::fs::read_to_string(dir.path().join("m4.xbin.json")).expect("manifest");
    assert_eq!(
        AppManifest::from_json(&on_disk).expect("parse").source_hash,
        expected
    );
}

#[test]
fn env_detection_switches_on_metadata_mode() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");

    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }
    impl EnvGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let saved = vec![(name, std::env::var(name).ok())];
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe { std::env::set_var(name, value) };
            Self { saved }
        }
        fn set_all(names: &[(&'static str, String)]) -> Self {
            let saved = names
                .iter()
                .map(|(name, _)| (*name, std::env::var(name).ok()))
                .collect();
            for (name, value) in names {
                // SAFETY: the process environment is guarded by ENV_LOCK.
                unsafe { std::env::set_var(name, value) };
            }
            Self { saved }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                // SAFETY: the process environment is guarded by ENV_LOCK.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    // Without the trigger variable the pass stays disabled.
    let _unset = EnvGuard::set(META_OUT_ENV, "");
    // SAFETY: the process environment is guarded by ENV_LOCK.
    unsafe { std::env::remove_var(META_OUT_ENV) };
    assert!(!XbinPass::from_env().is_metadata_mode());

    let _env = EnvGuard::set_all(&[
        (META_OUT_ENV, dir.path().display().to_string()),
        ("CARGO_PKG_NAME", "fixture-pkg".to_string()),
        ("CARGO_BIN_NAME", "fixture-bin".to_string()),
    ]);
    let pass = XbinPass::from_env();
    assert!(pass.is_metadata_mode(), "detected at the macro level");

    let (_, _) = pass
        .run_pass(quote!(device = mypac, core_ids = [1]), receiver_app())
        .expect("pass succeeds");

    let manifest = AppManifest::from_json(
        &std::fs::read_to_string(dir.path().join("fixture-bin.xbin.json"))
            .expect("manifest named after CARGO_BIN_NAME"),
    )
    .expect("parse");
    assert_eq!(manifest.package, "fixture-pkg");
    assert_eq!(manifest.target.name, "fixture-bin");
}

#[test]
fn full_pipeline_writes_the_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app_mod: syn::ItemMod = syn::parse_quote! {
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

            #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
            struct EncryptTask;

            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::Input) {}
            }
        }
    };

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::with_manifest(dir.path(), "app-m4", "m4"));
    let code = builder.build_rtic_macro2(
        quote!(device = mypac, core_ids = [1], external_cores = [0]),
        app_mod,
        None,
    );
    assert!(
        !code.to_string().contains("compile_error"),
        "pipeline failed: {code}"
    );

    let manifest = AppManifest::from_json(
        &std::fs::read_to_string(dir.path().join("m4.xbin.json")).expect("manifest"),
    )
    .expect("parse");
    assert_eq!(manifest.receivers.len(), 1);
    assert_eq!(manifest.receivers[0].name, "EncryptTask");
}

#[test]
fn manifest_declaration_order_is_normalized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_spawn(core = 2, priority = 2)]
            struct Zeta;

            #[cross_bin_spawn(core = 1, priority = 1)]
            struct Alpha;
        }
    };
    XbinPass::with_manifest(dir.path(), "app", "app")
        .run_pass(quote!(device = mypac), app_mod)
        .expect("pass succeeds");

    let manifest = AppManifest::from_json(
        &std::fs::read_to_string(dir.path().join("app.xbin.json")).expect("manifest"),
    )
    .expect("parse");
    let names: Vec<&str> = manifest
        .senders
        .iter()
        .map(|sender| sender.name.as_str())
        .collect();
    assert_eq!(names, ["Alpha", "Zeta"]);
}

#[test]
fn malformed_extensions_are_rejected_with_precise_errors() {
    let cases: Vec<(TokenStream, &str)> = vec![
        (
            quote!(device = mypac, core_ids = [1, 1, 2], cores = 3),
            "lists global core id 1 more than once",
        ),
        (
            quote!(device = mypac, external_cores = 3),
            "`external_cores` must be an array of integers",
        ),
        (
            quote!(device = mypac, cores = 0),
            "The `cores` argument must be at least 1.",
        ),
    ];

    for (args, expected) in cases {
        let error = XbinPass::disabled()
            .run_pass(
                args,
                syn::parse_quote!(
                    mod app {}
                ),
            )
            .expect_err("must be rejected")
            .to_string();
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn cross_attributes_must_be_on_structs() {
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[cross_bin_task(priority = 3)]
            fn not_a_struct() {}
        }
    };
    let error = XbinPass::disabled()
        .run_pass(quote!(device = mypac), app_mod)
        .expect_err("must be rejected")
        .to_string();
    assert!(error.contains("must be applied to a struct"), "{error}");
}
