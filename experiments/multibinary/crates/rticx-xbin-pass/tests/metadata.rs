//! Acceptance tests for the metadata mode (M1-T2, M5.5): a fixture
//! application run through the pass produces the expected `<target>.xbin.json`
//! from the native `#[sw_task]` receiver syntax.
//!
//! Environment-variable tests are serialized behind [`ENV_LOCK`] because the
//! process environment is global.

use std::sync::Mutex;

use proc_macro2::TokenStream;
use quote::quote;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{RticMacroBuilder, RticPass};
use rticx_xbin_pass::{
    CachePolicy, IpcPool, META_OUT_ENV, PhysicalCore, PoolId, XbinPass, XbinPassBackend,
};
use rticx_xbin_proto::AppManifest;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A receiver application: declares the native cross receiver task it
/// executes for a remote core.
fn receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            struct EncryptReq;

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    }
}

/// A producer application: since M5.5 it declares nothing (the pass generates
/// its sender stubs from the synced view).
fn producer_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {}
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

/// Like [`run_manifest`], but binds a distribution backend so the manifest
/// records its capability binding (M6.9-T2).
fn run_manifest_with_backend<B: XbinPassBackend + 'static>(
    backend: B,
    app_mod: syn::ItemMod,
    args: TokenStream,
    package: &str,
    target: &str,
) -> (tempfile::TempDir, AppManifest) {
    let dir = tempfile::tempdir().expect("tempdir");
    let pass = XbinPass::with_manifest(dir.path(), package, target).with_backend(backend);
    pass.run_pass(args, app_mod)
        .expect("metadata pass succeeds");

    let path = dir.path().join(AppManifest::file_name(target));
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing manifest {}: {error}", path.display()));
    let manifest = AppManifest::from_json(&source).expect("manifest parses");
    (dir, manifest)
}

/// The code-generation bindings that metadata mode never calls; keeping them
/// inert lets the capability tests implement only the two queries of M6.9-T2.
macro_rules! inert_bindings {
    () => {
        fn backend(&self) -> syn::Expr {
            syn::parse_quote!(backend())
        }

        fn rt_path(&self) -> syn::Path {
            syn::parse_quote!(xbin_rt)
        }

        fn ring_doorbell_fn(
            &self,
            _source: u32,
            _target: u32,
            template: syn::ItemFn,
        ) -> syn::ItemFn {
            template
        }

        fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident {
            quote::format_ident!("__router_{source}_{target}")
        }

        fn read_doorbell_msg_fn(
            &self,
            _target: u32,
            _source: u32,
            template: syn::ItemFn,
        ) -> syn::ItemFn {
            template
        }
    };
}

/// A backend that leaves [`XbinPassBackend::physical_core`] and
/// [`XbinPassBackend::ipc_pools`] at their trait defaults: the identity
/// mapping and no pools.
struct IdentityBackend;

impl XbinPassBackend for IdentityBackend {
    inert_bindings!();
}

/// A multi-core backend with an explicit physical-core mapping and pools,
/// used to pin the manifest extraction (M6.9-T2).
struct CapabilityBackend {
    /// Local core index -> physical core id.
    physical: Vec<u32>,
}

impl XbinPassBackend for CapabilityBackend {
    inert_bindings!();

    fn physical_core(&self, local_core: u32) -> PhysicalCore {
        PhysicalCore(
            self.physical
                .get(local_core as usize)
                .copied()
                .unwrap_or(local_core),
        )
    }

    fn ipc_pools(&self, local_core: u32) -> Vec<IpcPool> {
        match local_core {
            // Deliberately unsorted: the manifest sorts by `(peer, id)`.
            0 => vec![
                pool(
                    "axi",
                    10,
                    0x2400_0000,
                    0x2400_0000,
                    8192,
                    CachePolicy::NormalCacheableShareable,
                ),
                pool(
                    "sram3",
                    8,
                    0x3004_0000,
                    0x1004_0000,
                    4096,
                    CachePolicy::NormalNonCacheableShareable,
                ),
            ],
            1 => vec![pool(
                "sram1",
                7,
                0x3002_0000,
                0x1002_0000,
                1024,
                CachePolicy::NormalNonCacheableShareable,
            )],
            _ => Vec::new(),
        }
    }
}

/// Builds one capability pool for the tests.
fn pool(
    id: &str,
    peer: u32,
    base_local: u32,
    base_peer: u32,
    budget: u32,
    policy: CachePolicy,
) -> IpcPool {
    IpcPool {
        id: PoolId::new(id),
        peer: PhysicalCore::new(peer),
        base_local,
        base_peer,
        budget,
        policy,
    }
}

#[test]
fn receiver_fixture_produces_the_expected_manifest() {
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [0],
        ipc_dispatchers = [IRQ0]
    );
    let (_dir, manifest) = run_manifest(receiver_app(), args, "app-m4", "m4");

    assert_eq!(manifest.schema_version, 2);
    assert_eq!(manifest.package, "app-m4");
    assert_eq!(manifest.target.name, "m4");
    assert_eq!(manifest.cores, 1);
    assert_eq!(manifest.core_ids.as_deref(), Some(&[1][..]));
    assert_eq!(manifest.external_cores, &[0][..]);
    assert_eq!(manifest.types, vec!["EncryptReq".to_string()]);

    assert_eq!(manifest.receivers.len(), 1);
    let receiver = &manifest.receivers[0];
    assert_eq!(receiver.name, "EncryptTask");
    assert_eq!(receiver.priority, 3);
    assert_eq!(receiver.capacity, 2);
    assert_eq!(receiver.core, 0, "receiver core defaults to 0");
    assert_eq!(receiver.spawn_by, 0, "the global producer core id");
    assert_eq!(receiver.input_type, "EncryptReq");
    assert_eq!(receiver.input_type_name(), "EncryptReq");

    assert!(
        manifest.capabilities.is_empty(),
        "no distribution backend is bound, so the manifest carries no capability binding"
    );

    let json = manifest.to_json();
    assert!(json.contains("\"schema_version\": 2"), "{json}");
    assert!(json.contains("\"spawn_by\": 0"), "{json}");
    assert!(!json.contains("senders"), "no sender declarations: {json}");
    assert!(json.contains("\"kind\": \"bin\""), "{json}");
    assert!(json.contains("\"source_hash\": \"0x"), "{json}");
}

#[test]
fn manifest_records_the_resolved_core_ids() {
    // A declared mapping is recorded as written.
    let (_dir, manifest) = run_manifest(
        receiver_app(),
        quote!(
            device = mypac,
            cores = 2,
            core_ids = [4, 5],
            external_cores = [0],
            ipc_dispatchers = [[IRQ0], []]
        ),
        "app-m4",
        "m4",
    );
    assert_eq!(manifest.core_ids.as_deref(), Some(&[4, 5][..]));

    // Without the key, the manifest records the identity default the core
    // pass resolves (M5-T3), so the driver always validates it against
    // `rticx.toml`.
    let (_dir, manifest) = run_manifest(receiver_app(), quote!(device = mypac), "app-m4", "m4");
    assert_eq!(manifest.core_ids.as_deref(), Some(&[0][..]));
}

#[test]
fn producer_fixture_produces_an_empty_manifest() {
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [0],
        external_cores = [1]
    );
    let (_dir, manifest) = run_manifest(producer_app(), args, "app-m7", "m7");

    assert!(manifest.receivers.is_empty());
    assert!(manifest.types.is_empty());
    assert_eq!(manifest.external_cores, [1]);
}

#[test]
fn manifests_are_deterministic() {
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [0],
        ipc_dispatchers = [IRQ0]
    );
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
    let args = quote!(
        device = mypac,
        cores = 1,
        core_ids = [1],
        external_cores = [0],
        ipc_dispatchers = [IRQ0]
    );

    // The hash covers the arguments and module exactly as the user wrote
    // them, before any pass-owned attribute is stripped.
    let expected = rticx_xbin_pass::app_source_hash(&args, &app_mod);

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
    assert!(
        XbinPass::from_env().is_metadata_mode(),
        "detected at the macro level"
    );

    // The pass is deliberately not run here: a `from_env` metadata-mode pass
    // terminates the compiler once the manifest is written, which would kill
    // the test harness. The real `cargo check` path — including the manifest
    // naming from `CARGO_PKG_NAME`/`CARGO_BIN_NAME` — is covered end to end by
    // the driver's `crates/mock/fixtures/metadata` sync test (M7-T2).
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

            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(XbinPass::with_manifest(dir.path(), "app-m4", "m4"));
    let code = builder.build_rtic_macro2(
        quote!(
            device = mypac,
            core_ids = [1],
            external_cores = [0],
            ipc_dispatchers = [IRQ0]
        ),
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
            #[sw_task(priority = 2, spawn_by = 0)]
            struct Zeta;

            impl RticSwTask for Zeta {
                type SpawnInput = ipc_types::Msg;
            }

            #[sw_task(priority = 1, spawn_by = 0)]
            struct Alpha;

            impl RticSwTask for Alpha {
                type SpawnInput = ipc_types::Msg;
            }
        }
    };
    XbinPass::with_manifest(dir.path(), "app", "app")
        .run_pass(
            quote!(
                device = mypac,
                cores = 1,
                core_ids = [1],
                external_cores = [0],
                ipc_dispatchers = [IRQ0, IRQ1]
            ),
            app_mod,
        )
        .expect("pass succeeds");

    let manifest = AppManifest::from_json(
        &std::fs::read_to_string(dir.path().join("app.xbin.json")).expect("manifest"),
    )
    .expect("parse");
    let names: Vec<&str> = manifest
        .receivers
        .iter()
        .map(|receiver| receiver.name.as_str())
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

/// M6.9-T2: a multi-core application records one capability entry per local
/// core, and a backend that leaves `physical_core`/`ipc_pools` at their trait
/// defaults yields the identity physical id and no pools.
#[test]
fn manifest_records_the_identity_capability_default() {
    let (_dir, manifest) = run_manifest_with_backend(
        IdentityBackend,
        syn::parse_quote!(
            mod app {}
        ),
        quote!(device = mypac, cores = 2, core_ids = [4, 5]),
        "app-m4",
        "m4",
    );

    assert_eq!(manifest.schema_version, 2);
    assert_eq!(
        manifest.capabilities.len(),
        2,
        "one capability entry per local core"
    );
    for (local_core, capability) in manifest.capabilities.iter().enumerate() {
        assert_eq!(capability.local_core, local_core as u32);
        assert_eq!(
            capability.physical_core, local_core as u32,
            "the trait default is the identity mapping"
        );
        assert!(
            capability.pools.is_empty(),
            "the trait default exposes no pool"
        );
    }

    let json = manifest.to_json();
    assert!(json.contains("\"physical_core\": 0"), "{json}");
    assert!(!json.contains("\"pools\""), "no pools are recorded: {json}");
}

/// M6.9-T2: the manifest records the bound distribution's physical ids and
/// pools for every local core, with pool entries sorted by `(peer, id)`.
#[test]
fn manifest_records_multi_core_backend_capabilities() {
    let (_dir, manifest) = run_manifest_with_backend(
        CapabilityBackend {
            physical: vec![7, 9],
        },
        syn::parse_quote!(
            mod app {}
        ),
        quote!(device = mypac, cores = 2, core_ids = [7, 9]),
        "app-m4",
        "m4",
    );

    assert_eq!(manifest.capabilities.len(), 2);

    let first = &manifest.capabilities[0];
    assert_eq!(first.local_core, 0);
    assert_eq!(first.physical_core, 7);
    let ids: Vec<&str> = first.pools.iter().map(|pool| pool.id.as_str()).collect();
    assert_eq!(
        ids,
        ["sram3", "axi"],
        "pools are sorted by peer, not by the backend's return order"
    );

    let sram3 = &first.pools[0];
    assert_eq!(sram3.peer, 8);
    assert_eq!(sram3.base_local, 0x3004_0000);
    assert_eq!(sram3.base_peer, 0x1004_0000);
    assert_eq!(sram3.budget, 4096);
    assert_eq!(
        sram3.policy,
        rticx_xbin_proto::PoolCachePolicy::NormalNonCacheableShareable
    );

    let axi = &first.pools[1];
    assert_eq!(axi.peer, 10);
    assert_eq!(axi.budget, 8192);
    assert_eq!(
        axi.policy,
        rticx_xbin_proto::PoolCachePolicy::NormalCacheableShareable
    );

    let second = &manifest.capabilities[1];
    assert_eq!(second.local_core, 1);
    assert_eq!(second.physical_core, 9);
    assert_eq!(second.pools.len(), 1);
    assert_eq!(second.pools[0].id, "sram1");
    assert_eq!(second.pools[0].peer, 7);

    let json = manifest.to_json();
    assert!(json.contains("\"physical_core\": 7"), "{json}");
    assert!(json.contains("\"id\": \"sram3\""), "{json}");
    assert!(
        json.contains("\"policy\": \"normal_cacheable_shareable\""),
        "{json}"
    );
}
