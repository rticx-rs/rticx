//! Acceptance tests for cross-binary code generation (M3-T1 sender side,
//! M3-T2 receiver side, M5.5 native syntax).
//!
//! The pass runs in codegen mode over a fixture `system.json`: for every view
//! task whose `spawner_core` belongs to the application it emits the sender
//! stub (`pub struct <Task>;`), the FIFO view helper and the `cross_spawn`
//! impl; for every native `#[sw_task]` cross receiver it rewrites the struct
//! into a core `#[task(.., task_trait = RticSwTask)]`, emits the
//! `SpawnInput: CrossCoreMessage` assertion, its consumer FIFO view and a
//! doorbell dispatcher draining it. The tests snapshot the generated sections
//! (the `assert_section_present` approach of the root pass tests) and cover
//! the documented stale-view and configuration errors.
//!
//! Environment-variable tests are serialized behind [`ENV_LOCK`] because the
//! process environment is global.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use rticx_core::{MainInjectionPoint, RticPass};
use rticx_xbin_pass::{AppTopology, SYSTEM_ENV, XbinPass, XbinPassBackend};
use rticx_xbin_proto::{Hash64, IPC_TYPES_FILE, IpcTypes, SystemView, layout_hash, parse_idl_str};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Saves and restores environment variables around a test that sets them.
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set_all(vars: &[(&'static str, &std::ffi::OsStr)]) -> Self {
        let saved = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe { std::env::set_var(name, value) };
        }
        Self { saved }
    }

    /// Removes `names` from the environment, restoring them on drop.
    ///
    /// Models compilers that leave a variable unset: rust-analyzer's
    /// proc-macro server never defines `CARGO_BIN_NAME`.
    fn unset_all(names: &[&'static str]) -> Self {
        let saved = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in names {
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe { std::env::remove_var(name) };
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

/// A two-application system view: `app-m7` (global core 0) spawns
/// `EncryptTask` on `app-m4` (global core 1), priority 3, capacity 2.
const SYSTEM_JSON: &str = r#"{
  "schema_version": 3,
  "rticx_generation": "0.2",
  "topology_hash": "0x2b7f11101722fb4e",
  "layout_hash": "0x59021e1c399c4ddb",
  "apps": [
    {
      "package": "app-m7",
      "target": { "kind": "bin", "name": "m7" },
      "interface_hash": "0x26a74c0569235dd7",
      "core_ids": [0],
      "external_cores": [1]
    },
    {
      "package": "app-m4",
      "target": { "kind": "bin", "name": "m4" },
      "interface_hash": "0x70b1fa36c7b61323",
      "core_ids": [1],
      "external_cores": [0]
    }
  ],
  "cores": [
    { "global_id": 0, "physical_core": 0, "app": "app-m7", "local_index": 0 },
    { "global_id": 1, "physical_core": 1, "app": "app-m4", "local_index": 0 }
  ],
  "types": [
    {
      "name": "EncryptReq",
      "kind": "message",
      "size": 12,
      "align": 4,
      "fields": [
        { "name": "addr", "ty": "u32", "offset": 0 },
        { "name": "len", "ty": "u32", "offset": 4 },
        { "name": "key", "ty": "u32", "offset": 8 }
      ]
    }
  ],
  "tasks": [
    {
      "id": 1,
      "name": "EncryptTask",
      "receiver_core": 1,
      "spawner_core": 0,
      "priority": 3,
      "capacity": 2,
      "input_type": "EncryptReq",
      "fifo": { "source": 0, "target": 1, "pool": "p01", "offset": 0, "elem_size": 12, "depth": 3 }
    }
  ],
  "pools": [
    {
      "id": "p01",
      "core_a": 0,
      "core_b": 1,
      "base_from_a": "0x30040000",
      "base_from_b": "0x30040000",
      "budget": 4096,
      "used": 44
    }
  ],
  "doorbells": [
    { "source": 0, "target": 1, "priority": 3, "line": 0 }
  ]
}
"#;

/// The code-generation backend used by the snapshots.
///
/// The snapshots are never compiled, so `current_global_core_id` and
/// `configure_shared_memory` emit inspectable expressions; `ipc_base_override`
/// stays unset, exercising the literal synced-address path (D3).
struct TestBackend;

impl XbinPassBackend for TestBackend {
    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt)
    }

    fn current_global_core_id(&self) -> syn::Expr {
        syn::parse_quote!(__mock_xbin_backend().global_core_id())
    }

    fn configure_shared_memory(&self, _local_core: u32) -> Option<TokenStream> {
        Some(quote! {
            __mock_xbin_configure();
        })
    }

    fn ring_doorbell_fn(&self, source: u32, target: u32, mut template: syn::ItemFn) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __mock_xbin_backend().doorbell_send(#source, #target, task_id)
        });
        template
    }

    fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident {
        format_ident!("XbinRouter{source}To{target}")
    }

    fn read_doorbell_msg_fn(
        &self,
        target: u32,
        source: u32,
        mut template: syn::ItemFn,
    ) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __mock_xbin_backend().take_message(#source, #target)
        });
        template
    }
}

/// The producer fixture application: it declares nothing; the pass generates
/// the `EncryptTask` stub from the view (M5.5).
fn sender_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {}
    }
}

fn sender_args() -> TokenStream {
    quote!(device = mypac)
}

/// The producer fixture's topology: local core 0 -> global 0, global core 1
/// visible (M7-T2).
fn sender_topology() -> AppTopology {
    AppTopology::new(vec![0], vec![1])
}

/// The receiver fixture application: one native cross receiver with its
/// `impl RticSwTask { type SpawnInput }` block.
fn receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
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
    }
}

fn receiver_args() -> TokenStream {
    quote!(device = mypac, dispatchers = [IRQ0])
}

/// The receiver fixture's topology: local core 0 -> global 1, global core 0
/// visible (M7-T2).
fn receiver_topology() -> AppTopology {
    AppTopology::new(vec![1], vec![0])
}

/// Like [`receiver_args`] for an application without cross declarations: the
/// pass-owned pool must be empty (one entry per cross line, M6.5-T1).
fn receiver_args_without_cross_declarations() -> TokenStream {
    quote!(device = mypac, dispatchers = [])
}

/// The generated `ipc_types.rs` stand-in: the fixture view's `EncryptReq`
/// type, as `cargo xbin sync` would emit it (M1-T7).
const IPC_TYPES_RS: &str = "\
// @generated by `cargo xbin sync`\n\
#[repr(C)]\n\
#[derive(Clone, Copy)]\n\
pub struct EncryptReq {\n\
    pub addr: u32,\n\
    pub len: u32,\n\
    pub key: u32,\n\
}\n\
pub const SIZE_ENCRYPT_REQ: usize = 12;\n\
pub const ALIGN_ENCRYPT_REQ: usize = 4;\n\
pub const LAYOUT_HASH: u64 = 0x59021e1c399c4ddb;\n\
unsafe impl CrossCoreMessage for EncryptReq {}\n";

/// A `rticx.toml` listing the sender fixture application, so `from_env`
/// discovers a project root (and therefore an `ipc-types.toml` location).
const PROJECT_TOML: &str = "schema = 1\n\n\
     [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
     core_ids = [0]\nexternal_cores = [1]\n";

/// The end-to-end fixture IDL: one `EncryptReq { addr: u32, len: u32, key: u32 }`
/// message, as `crates/mock/fixtures/e2e/ipc-types.toml` declares it.
const IDL_TOML: &str = "schema = 1\n\n\
     [message.EncryptReq]\nfields = { addr = \"u32\", len = \"u32\", key = \"u32\" }\n";

/// Writes a `system.json` and the generated `ipc_types.rs` next to it into a
/// fresh directory, as `cargo xbin sync` does, and returns them.
fn write_system(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let (dir, path) = write_system_without_ipc_types(contents);
    std::fs::write(dir.path().join(IPC_TYPES_FILE), IPC_TYPES_RS).expect("ipc_types.rs");
    (dir, path)
}

/// Like [`write_system`], but writes only the view: used to exercise the
/// missing `ipc_types.rs` hard error (M1-T7).
fn write_system_without_ipc_types(contents: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.json");
    std::fs::write(&path, contents).expect("system.json");
    (dir, path)
}

/// Records the interface fingerprint of every `(package, args, module,
/// topology)` patch in the fixture view and returns the resealed JSON.
fn with_interface_hashes(
    contents: &str,
    patches: &[(&str, &TokenStream, &syn::ItemMod, AppTopology)],
) -> String {
    let mut view = SystemView::from_json(contents).expect("fixture JSON");
    for (package, args, app_mod, topology) in patches {
        let application = view
            .apps
            .iter_mut()
            .find(|application| application.package == *package)
            .unwrap_or_else(|| panic!("fixture has no application `{package}`"));
        application.interface_hash =
            rticx_xbin_pass::app_interface_hash(args, app_mod, Some(topology))
                .expect("the fixture patch parses");
    }
    view.seal();
    view.to_json()
}

/// Writes a fixture view whose producer application records the fingerprint of
/// `(sender_args, sender_app)`, as `cargo xbin sync` would (M3-T4).
fn write_sender_system() -> (tempfile::TempDir, PathBuf) {
    write_system(&with_interface_hashes(
        SYSTEM_JSON,
        &[("app-m7", &sender_args(), &sender_app(), sender_topology())],
    ))
}

/// Like [`write_sender_system`] for the receiver fixture application.
fn write_receiver_system() -> (tempfile::TempDir, PathBuf) {
    write_system(&with_interface_hashes(
        SYSTEM_JSON,
        &[(
            "app-m4",
            &receiver_args(),
            &receiver_app(),
            receiver_topology(),
        )],
    ))
}

/// Like [`write_sender_system`] for both fixture applications.
fn write_both_system() -> (tempfile::TempDir, PathBuf) {
    write_system(&with_interface_hashes(
        SYSTEM_JSON,
        &[
            ("app-m7", &sender_args(), &sender_app(), sender_topology()),
            (
                "app-m4",
                &receiver_args(),
                &receiver_app(),
                receiver_topology(),
            ),
        ],
    ))
}

/// Writes the fixture view with a fresh `topology_hash` but no re-recorded
/// interface hashes.
///
/// Used by tests whose error is raised before the freshness checks; the
/// hard-coded `interface_hash` values of [`SYSTEM_JSON`] then never match, which
/// is irrelevant for those tests.
fn write_base_system() -> (tempfile::TempDir, PathBuf) {
    write_system(&system_with(|_| {}))
}

/// Parses [`SYSTEM_JSON`], lets `mutate` edit the value and re-serializes the
/// **resealed** view.
fn system_with(mutate: impl FnOnce(&mut serde_json::Value)) -> String {
    system_with_patches(mutate, &[])
}

/// Like [`system_with`], additionally recording the interface fingerprints of
/// `patches` (in the same view) before sealing.
fn system_with_patches(
    mutate: impl FnOnce(&mut serde_json::Value),
    patches: &[(&str, &TokenStream, &syn::ItemMod, AppTopology)],
) -> String {
    let mut value: serde_json::Value = serde_json::from_str(SYSTEM_JSON).expect("fixture JSON");
    mutate(&mut value);
    with_interface_hashes(&serde_json::to_string(&value).expect("JSON"), patches)
}

/// Parses [`SYSTEM_JSON`], lets `mutate` edit the raw JSON and re-serializes
/// it **without** resealing, producing a stale `topology_hash`.
fn system_tampered(mutate: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut view: serde_json::Value = serde_json::from_str(SYSTEM_JSON).expect("fixture JSON");
    mutate(&mut view);
    serde_json::to_string(&view).expect("JSON")
}

/// Runs the codegen pass over the producer fixture and returns the generated
/// module as a token string.
fn generate(path: &Path) -> syn::Result<String> {
    let pass = XbinPass::with_system(path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend);
    let (_, module) = pass.run_pass(sender_args(), sender_app())?;
    Ok(module.to_token_stream().to_string())
}

/// Generates and expects success.
fn generate_ok(path: &Path) -> String {
    generate(path).expect("sender code generation succeeds")
}

/// Runs the codegen pass over the receiver fixture and returns the generated
/// module as a token string.
fn generate_receiver(path: &Path) -> syn::Result<String> {
    let pass = XbinPass::with_system(path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend);
    let (_, module) = pass.run_pass(receiver_args(), receiver_app())?;
    Ok(module.to_token_stream().to_string())
}

/// Receiver generation and expected success.
fn generate_receiver_ok(path: &Path) -> String {
    generate_receiver(path).expect("receiver code generation succeeds")
}

/// Asserts that `expected` is a contiguous token substring of `generated`.
fn assert_section_present(generated: &str, expected: TokenStream, label: &str) {
    let expected = expected.to_string();
    assert!(
        generated.contains(&expected),
        "missing expected section `{label}` in the generated output\n\
         expected:\n{expected}\n\n\
         generated:\n{generated}"
    );
}

#[test]
fn sender_codegen_snapshot() {
    let (_dir, path) = write_sender_system();
    let generated = generate_ok(&path);

    // The producer source declares nothing: the pass generates the stub.
    assert!(
        generated.contains("pub struct EncryptTask ;"),
        "the generated sender stub is missing: {generated}"
    );

    // ---- FIFO view ----
    // The view is argument-less and bakes the synced pool base literal (the
    // backend leaves `ipc_base_override` unset, D3).
    assert_section_present(
        &generated,
        quote! {
            fn __rticx_xbin_fifo_EncryptTask () -> * mut rticx_xbin_rt :: Fifo < ipc_types :: EncryptReq , 3usize >
        },
        "FIFO view signature",
    );
    assert_section_present(
        &generated,
        quote! {
            const __RTICX_XBIN_SOURCE_CORE : u32 = 0u32 ;
            const __RTICX_XBIN_TARGET_CORE : u32 = 1u32 ;
            const __RTICX_XBIN_FIFO_OFFSET : usize = 0usize ;
        },
        "FIFO view constants",
    );
    assert_section_present(
        &generated,
        quote! {
            rticx_xbin_rt :: Fifo :: < ipc_types :: EncryptReq , 3usize > :: view_at (
                0x30040000usize + __RTICX_XBIN_FIFO_OFFSET ,
            )
        },
        "literal synced pool base",
    );

    // The FIFO is pinned to its distro pool: the id and shared budget are
    // emitted as constants and the FIFO is asserted to fit (M6.9-T6).
    assert!(
        generated.contains("__RTICX_XBIN_POOL_ID : & str = \"p01\""),
        "the pool id is not pinned: {generated}"
    );
    assert!(
        generated.contains("__RTICX_XBIN_POOL_BUDGET : usize = 4096usize"),
        "the pool budget is not pinned: {generated}"
    );
    assert!(
        generated.contains("__RTICX_XBIN_FIFO_OFFSET + 44usize <= __RTICX_XBIN_POOL_BUDGET"),
        "the FIFO is not asserted to fit its pool: {generated}"
    );

    // ---- sender API ----
    assert_section_present(
        &generated,
        quote! {
            pub fn cross_spawn (
                input : ipc_types :: EncryptReq ,
            ) -> Result < () , Option < ipc_types :: EncryptReq > >
        },
        "cross_spawn signature",
    );

    // ---- pair ring function (M6.5-T2) ----
    assert_section_present(
        &generated,
        quote! {
            fn __rticx_xbin_ring_0_1 (task_id : u32) -> Result < () , () > {
                __mock_xbin_backend () . doorbell_send (0u32 , 1u32 , task_id)
            }
        },
        "ring function with the backend-filled body",
    );
}

#[test]
fn sender_codegen_ring_and_error_semantics() {
    let (_dir, path) = write_sender_system();
    let generated = generate_ok(&path);

    // The generated code no longer threads a runtime backend value: it calls
    // the argument-less FIFO view and the backend's core-id expression
    // directly (D4, D5).
    assert!(
        !generated.contains("CrossBinBackend"),
        "the runtime trait must not appear in generated code: {generated}"
    );
    assert!(
        !generated.contains("let __rticx_xbin_backend"),
        "the generated spawner must not bind a backend value: {generated}"
    );

    // Global-core guard and interrupt-free enqueue.
    assert_section_present(
        &generated,
        quote! {
            if __mock_xbin_backend () . global_core_id () != __RTICX_XBIN_SOURCE_CORE {
                return Err (Some (input)) ;
            }
        },
        "global-core guard",
    );
    assert!(
        generated.contains(
            "__rticx_interrupt_free (| | -> Result < () , Option < ipc_types :: EncryptReq > > {"
        ),
        "missing interrupt-free section:\n{generated}"
    );
    assert_section_present(
        &generated,
        quote! { let __rticx_xbin_fifo = __rticx_xbin_fifo_EncryptTask () ; },
        "argument-less FIFO view call",
    );
    assert_section_present(
        &generated,
        quote! {
            if let Err (input) = unsafe { (* __rticx_xbin_fifo) . enqueue (input) } {
                return Err (Some (input)) ;
            }
        },
        "enqueue error semantics",
    );

    // The ring call publishes the task id of the synced task and maps a
    // failed notification to `Err(None)` (the input is already enqueued).
    assert_section_present(
        &generated,
        quote! {
            const __RTICX_XBIN_TASK_ID : u32 = 1u32 ;
        },
        "task id constant",
    );
    assert_section_present(
        &generated,
        quote! {
            match __rticx_xbin_ring_0_1 (__RTICX_XBIN_TASK_ID) {
                Ok (()) => Ok (()) ,
                Err (()) => Err (None) ,
            }
        },
        "ring call and its `Err(None)` semantics",
    );
}

/// The generated spawner does not gate on the target's readiness: boot
/// sequencing is distribution-owned, so `cross_spawn` only guards the caller's
/// core, enqueues and rings.
#[test]
fn sender_codegen_has_no_target_ready_gate() {
    let (_dir, path) = write_sender_system();
    let generated = generate_ok(&path);

    assert!(
        !generated.contains("ReadyCache") && !generated.contains("__RTICX_XBIN_READY"),
        "the generated spawner still gates on target readiness: {generated}"
    );
    assert!(
        !generated.contains("shared_state"),
        "the generated spawner still consults the shared readiness state: {generated}"
    );
    assert!(
        !generated.contains("has not marked itself ready"),
        "the spawn documentation still names a not-ready target: {generated}"
    );
}

#[test]
fn sender_codegen_layout_assertions() {
    let (_dir, path) = write_sender_system();
    let generated = generate_ok(&path);

    assert_section_present(
        &generated,
        quote! {
            const _ : () = assert ! (
                core :: mem :: size_of :: < ipc_types :: EncryptReq > () == 12usize ,
                "the IDL layout of the spawn input changed; run `cargo xbin sync`" ,
            ) ;
        },
        "size assertion",
    );
    assert_section_present(
        &generated,
        quote! {
            const _ : () = assert ! (
                core :: mem :: align_of :: < ipc_types :: EncryptReq > () == 4usize ,
                "the IDL layout of the spawn input changed; run `cargo xbin sync`" ,
            ) ;
        },
        "align assertion",
    );
}

#[test]
fn topology_hash_and_system_view_are_embedded() {
    let (_dir, path) = write_sender_system();
    let stored = SystemView::from_json(&std::fs::read_to_string(&path).expect("system.json"))
        .expect("fixture system view");
    let generated = generate_ok(&path);

    // The `TOPOLOGY_HASH` anchor of plan §7.3 (M3-T4).
    assert!(
        generated.contains(&format!(
            "__RTICX_XBIN_TOPOLOGY_HASH : u64 = 0x{:016x}u64",
            stored.topology_hash.get()
        )),
        "the synced topology hash is not embedded as a const: {generated}"
    );

    // `include_str!` of the view anchors rustc's dep-info, so editing
    // `system.json` rebuilds the application.
    let canonical = path.canonicalize().expect("the fixture view exists");
    let view_lit = syn::LitStr::new(
        canonical.to_str().expect("UTF-8 fixture path"),
        Span::call_site(),
    );
    assert_section_present(
        &generated,
        quote! {
            const __RTICX_XBIN_SYSTEM_JSON : & str = include_str ! (#view_lit) ;
        },
        "system view include",
    );
}

#[test]
fn ipc_types_trait_and_module_are_injected() {
    let (_dir, path) = write_sender_system();
    let generated = generate_ok(&path);

    // The marker trait is injected next to the module, so the re-emitted
    // `unsafe impl CrossCoreMessage for … {}` resolves without the runtime
    // crate (M1-T7).
    assert!(
        generated.contains("pub unsafe trait CrossCoreMessage : Copy + 'static { }"),
        "the injected marker trait is missing: {generated}"
    );
    assert!(
        generated.contains("pub mod ipc_types"),
        "the injected `ipc_types` module is missing: {generated}"
    );
    assert!(
        generated.contains("use super :: CrossCoreMessage ;"),
        "the module does not import the injected trait: {generated}"
    );
    assert!(
        generated.contains("pub struct EncryptReq"),
        "the synced IDL types are not re-emitted: {generated}"
    );
    assert!(
        generated.contains("unsafe impl CrossCoreMessage for EncryptReq { }"),
        "the re-emitted marker impl is missing: {generated}"
    );
    assert!(
        generated.contains("const __RTICX_XBIN_IPC_TYPES_RS : & str = include_str !"),
        "the `ipc_types.rs` dep-info anchor is missing: {generated}"
    );
}

#[test]
fn missing_ipc_types_file_is_reported() {
    // A view with types but no generated module (a plain `cargo build` after a
    // stale/absent `cargo xbin sync`) is a hard error, not a silent empty
    // module (M1-T7).
    let (_dir, path) = write_system_without_ipc_types(&with_interface_hashes(
        SYSTEM_JSON,
        &[("app-m7", &sender_args(), &sender_app(), sender_topology())],
    ));

    let error = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect_err("a view with types needs the generated module")
        .to_string();
    assert!(error.contains("ipc_types.rs"), "{error}");
    assert!(error.contains("cargo xbin sync"), "{error}");
}

#[test]
fn user_defined_ipc_types_module_is_rejected() {
    // The pass injects a `ipc_types` module; a user item of that name would
    // otherwise fail with rustc's generic "defined multiple times" error.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            mod ipc_types {}
        }
    };
    let (_dir, path) = write_system(&with_interface_hashes(
        SYSTEM_JSON,
        &[("app-m7", &sender_args(), &app_mod, sender_topology())],
    ));

    let error = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend)
        .run_pass(sender_args(), app_mod)
        .expect_err("a user `ipc_types` module must be rejected")
        .to_string();
    assert!(error.contains("injects a `ipc_types` module"), "{error}");
}

#[test]
fn user_defined_cross_core_message_is_rejected() {
    // The pass injects the `CrossCoreMessage` marker trait next to the module.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait CrossCoreMessage {}
        }
    };
    let (_dir, path) = write_system(&with_interface_hashes(
        SYSTEM_JSON,
        &[("app-m7", &sender_args(), &app_mod, sender_topology())],
    ));

    let error = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend)
        .run_pass(sender_args(), app_mod)
        .expect_err("a user `CrossCoreMessage` item must be rejected")
        .to_string();
    assert!(
        error.contains("injects a `CrossCoreMessage` trait"),
        "{error}"
    );
}

#[test]
fn stale_topology_hash_is_rejected() {
    // Editing a semantic field without re-running the driver leaves the stored
    // hash pointing at the previous contents: a hard error with the
    // `cargo xbin sync` hint (M3-T4).
    let tampered = system_tampered(|view| {
        view["tasks"][0]["priority"] = serde_json::json!(4);
    });
    let (_dir, path) = write_system(&tampered);

    let error = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect_err("a tampered view must be rejected")
        .to_string();
    assert!(error.contains("is stale"), "{error}");
    assert!(error.contains("`topology_hash`"), "{error}");
    assert!(error.contains("run `cargo xbin sync`"), "{error}");
}

#[test]
fn stale_application_interface_is_rejected() {
    // The recorded fingerprint pins the cross-binary interface: dropping the
    // receiver declaration between `sync` and a plain `cargo build` must be
    // caught by the `interface changed` error (M3-T4). Per-field edits are
    // reported by `resolve_receiver`'s more specific messages first.
    let (_dir, path) = write_receiver_system();
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };

    let error = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend)
        .run_pass(receiver_args(), app_mod)
        .expect_err("an interface changed after sync must be rejected")
        .to_string();
    assert!(
        error.contains("interface changed since the last `cargo xbin sync`"),
        "{error}"
    );
    assert!(error.contains("run `cargo xbin sync`"), "{error}");
}

#[test]
fn receiver_codegen_snapshot() {
    let (_dir, path) = write_receiver_system();
    let generated = generate_receiver_ok(&path);

    // The native `#[sw_task]` is replaced by the core `#[task]` shape, which
    // is exactly what the core pass consumes (`task_trait` external path).
    assert!(
        !generated.contains("sw_task"),
        "the native attribute must be replaced: {generated}"
    );
    assert_section_present(
        &generated,
        quote! {
            #[task(priority = 3, core = 0, task_trait = RticSwTask, init = generated)]
            struct EncryptTask;
        },
        "receiver task attribute",
    );

    // ---- `SpawnInput: CrossCoreMessage` assertion ----
    assert_section_present(
        &generated,
        quote! {
            __rticx_xbin_assert_cross_core_message :: < < EncryptTask as RticSwTask > :: SpawnInput > () ;
        },
        "CrossCoreMessage assertion",
    );
    assert_section_present(
        &generated,
        quote! {
            fn __rticx_xbin_assert_cross_core_message < T : CrossCoreMessage > () {}
        },
        "CrossCoreMessage assertion bound",
    );

    // ---- consumer FIFO view ----
    assert_section_present(
        &generated,
        quote! {
            fn __rticx_xbin_fifo_EncryptTask () -> * mut rticx_xbin_rt :: Fifo < ipc_types :: EncryptReq , 3usize >
        },
        "receiver FIFO view signature",
    );
    assert_section_present(
        &generated,
        quote! {
            const __RTICX_XBIN_SOURCE_CORE : u32 = 0u32 ;
            const __RTICX_XBIN_TARGET_CORE : u32 = 1u32 ;
            const __RTICX_XBIN_FIFO_OFFSET : usize = 0usize ;
        },
        "receiver FIFO view constants",
    );
    assert!(
        generated.contains("consumer view of the `EncryptTask` FIFO"),
        "{generated}"
    );

    // ---- line dispatcher (M6.5-T3) ----
    // The dispatcher is bound to the line's `dispatchers` entry, the same
    // interrupt the router pends.
    assert_section_present(
        &generated,
        quote! {
            #[task(binds = IRQ0, priority = 3, core = 0, init = generated)]
            pub struct __RticxXbinDispatcher0To1P3;
        },
        "dispatcher task attribute",
    );
    assert_section_present(
        &generated,
        quote! { impl RticTask for __RticxXbinDispatcher0To1P3 },
        "dispatcher exec header",
    );
    assert!(
        !generated.contains("let __rticx_xbin_backend"),
        "the generated dispatcher must not bind a backend value: {generated}"
    );

    // The dispatcher drains its ready queue, then the FIFO of each popped
    // task until empty (a duplicate notification is a no-op). The FIFO view is
    // argument-less and bakes the synced address.
    assert_section_present(
        &generated,
        quote! {
            let mut __rticx_xbin_ready = unsafe { __rticx_xbin_ready_0_1_p3.split().1 };
            while let Some(__rticx_xbin_task) = __rticx_xbin_ready.dequeue() {
                match __rticx_xbin_task {
                    __RticxXbinLine0To1P3::EncryptTask => {
                        let __rticx_xbin_fifo = __rticx_xbin_fifo_EncryptTask();
                        unsafe {
                            while let Some(input) = (*__rticx_xbin_fifo).dequeue() {
                                ENCRYPT_TASK.assume_init_mut().exec(input);
                            }
                        }
                    }
                }
            }
        },
        "dispatcher ready-queue drain loop",
    );
}

#[test]
fn receiver_codegen_line_queue_snapshot() {
    let (_dir, path) = write_receiver_system();
    let generated = generate_receiver_ok(&path);

    // One task enum and one ready queue per line; the queue is sized to the
    // sum of the line's task capacities plus the ring buffer's spare slot
    // (M6.5-T3).
    assert_section_present(
        &generated,
        quote! {
            #[derive(Clone, Copy)]
            pub enum __RticxXbinLine0To1P3 {
                EncryptTask,
            }
        },
        "line task enum",
    );
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_xbin_ready_0_1_p3:
                rticx_xbin_rt::Queue<__RticxXbinLine0To1P3, 3usize> = rticx_xbin_rt::Queue::new();
        },
        "line ready queue",
    );
}

#[test]
fn receiver_codegen_router_snapshot() {
    let (_dir, path) = write_receiver_system();
    let generated = generate_receiver_ok(&path);

    // ---- target-side read function (M6.5-T3) ----
    assert_section_present(
        &generated,
        quote! {
            fn __rticx_xbin_read_0_1() -> Option<u32> {
                __mock_xbin_backend().take_message(0u32, 1u32)
            }
        },
        "read function with the backend-filled body",
    );

    // ---- router (M6.5-T3) ----
    // One router per pair, bound to the pair doorbell at the highest line
    // priority of the pair.
    assert_section_present(
        &generated,
        quote! {
            #[task(binds = XbinRouter0To1, priority = 3, core = 0, init = generated)]
            pub struct __RticxXbinRouter0To1;
        },
        "router task attribute",
    );
    assert_section_present(
        &generated,
        quote! { impl RticTask for __RticxXbinRouter0To1 },
        "router exec header",
    );

    // The router drains the pair's doorbell word, enqueues each id in its
    // line's ready queue and pends the line dispatcher through the software
    // pass's pend function; unknown ids are ignored.
    assert_section_present(
        &generated,
        quote! {
            while let Some(__rticx_xbin_task_id) = __rticx_xbin_read_0_1() {
                match __rticx_xbin_task_id {
                    1u32 => {
                        unsafe {
                            __rticx_xbin_ready_0_1_p3
                                .split()
                                .0
                                .enqueue_unchecked(__RticxXbinLine0To1P3::EncryptTask);
                        }
                        __rticx_local_irq_pend(mypac::Interrupt::IRQ0);
                    }
                    _ => {}
                }
            }
        },
        "router drain and pend loop",
    );
}

/// Receiver args for the two-line fixture: two cross lines on local core 0,
/// listed in ascending `(source, priority)` order (M6.5-T1).
fn two_line_receiver_args() -> TokenStream {
    quote!(device = mypac, dispatchers = [IRQ0, IRQ1])
}

/// Receiver module with two tasks on two priority lines of the same
/// `(0 -> 1)` pair.
fn two_line_receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct AlphaTask;

            impl RticSwTask for AlphaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(priority = 4, capacity = 1, spawn_by = 0)]
            struct BetaTask;

            impl RticSwTask for BetaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    }
}

#[test]
fn router_has_one_match_arm_per_view_task() {
    let args = two_line_receiver_args();
    let app_mod = two_line_receiver_app();
    let view = system_with_patches(
        |view| {
            let tasks = view["tasks"].as_array_mut().expect("tasks");
            tasks[0]["name"] = serde_json::json!("AlphaTask");
            let mut beta = tasks[0].clone();
            beta["id"] = serde_json::json!(2);
            beta["name"] = serde_json::json!("BetaTask");
            beta["priority"] = serde_json::json!(4);
            beta["capacity"] = serde_json::json!(1);
            beta["fifo"]["offset"] = serde_json::json!(64);
            beta["fifo"]["depth"] = serde_json::json!(2);
            tasks.push(beta);
            view["doorbells"]
                .as_array_mut()
                .expect("doorbells")
                .push(serde_json::json!({
                    "source": 0,
                    "target": 1,
                    "priority": 4,
                    "line": 1
                }));
        },
        &[("app-m4", &args, &app_mod, receiver_topology())],
    );
    let (_dir, path) = write_system(&view);
    let pass = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend);
    let (_, module) = pass
        .run_pass(args, app_mod)
        .expect("two-line generation succeeds");
    let generated = module.to_token_stream().to_string();

    // Two lines on the target: two dispatchers bound to distinct pool
    // entries.
    assert_section_present(
        &generated,
        quote! {
            #[task(binds = IRQ0, priority = 3, core = 0, init = generated)]
            pub struct __RticxXbinDispatcher0To1P3;
        },
        "priority-3 dispatcher",
    );
    assert_section_present(
        &generated,
        quote! {
            #[task(binds = IRQ1, priority = 4, core = 0, init = generated)]
            pub struct __RticxXbinDispatcher0To1P4;
        },
        "priority-4 dispatcher",
    );

    // One router per pair, at the highest line priority of the pair, with one
    // match arm and one pend per view task (M6.5-T3).
    assert_section_present(
        &generated,
        quote! {
            #[task(binds = XbinRouter0To1, priority = 4, core = 0, init = generated)]
            pub struct __RticxXbinRouter0To1;
        },
        "router at the maximum line priority",
    );
    assert_section_present(
        &generated,
        quote! {
            1u32 => {
                unsafe {
                    __rticx_xbin_ready_0_1_p3
                        .split()
                        .0
                        .enqueue_unchecked(__RticxXbinLine0To1P3::AlphaTask);
                }
                __rticx_local_irq_pend(mypac::Interrupt::IRQ0);
            }
        },
        "AlphaTask match arm",
    );
    assert_section_present(
        &generated,
        quote! {
            2u32 => {
                unsafe {
                    __rticx_xbin_ready_0_1_p4
                        .split()
                        .0
                        .enqueue_unchecked(__RticxXbinLine0To1P4::BetaTask);
                }
                __rticx_local_irq_pend(mypac::Interrupt::IRQ1);
            }
        },
        "BetaTask match arm",
    );
}

#[test]
fn receiver_codegen_layout_assertions() {
    let (_dir, path) = write_receiver_system();
    let generated = generate_receiver_ok(&path);

    assert_section_present(
        &generated,
        quote! {
            const _ : () = assert ! (
                core :: mem :: size_of :: < ipc_types :: EncryptReq > () == 12usize ,
                "the IDL layout of the spawn input changed; run `cargo xbin sync`" ,
            ) ;
        },
        "size assertion",
    );
    assert_section_present(
        &generated,
        quote! {
            const _ : () = assert ! (
                core :: mem :: align_of :: < ipc_types :: EncryptReq > () == 4usize ,
                "the IDL layout of the spawn input changed; run `cargo xbin sync`" ,
            ) ;
        },
        "align assertion",
    );
}

#[test]
fn owner_codegen_init_hooks_snapshot() {
    let (_dir, path) = write_sender_system();
    let generated = generate_ok(&path);

    // The configure hook is inlined at `BeforeInit`, not a generated function
    // (D1); only the per-core FIFO initializer is emitted.
    assert!(
        !generated.contains("__rticx_xbin_configure_shared_memory"),
        "the configure wrapper function must be gone: {generated}"
    );

    // `app-m7` produces the `0 -> 1` FIFO, so its core 0 gets its own
    // argument-less FIFO initializer, run before `post_init` (M6-T1).
    assert_section_present(
        &generated,
        quote! { fn __rticx_xbin_init_fifos_core0 () },
        "producer FIFO init header",
    );
    assert!(
        generated.contains("Zeroes the ring indices of every task FIFO produced by this core"),
        "{generated}"
    );
    assert_section_present(
        &generated,
        quote! {
            const __RTICX_XBIN_SOURCE_CORE : u32 = 0u32 ;
            const __RTICX_XBIN_TARGET_CORE : u32 = 1u32 ;
            const __RTICX_XBIN_FIFO_OFFSET : usize = 0usize ;
        },
        "producer FIFO constants",
    );
    assert_section_present(
        &generated,
        quote! {
            (* rticx_xbin_rt :: Fifo :: < ipc_types :: EncryptReq , 3usize > :: view_at (
                0x30040000usize + __RTICX_XBIN_FIFO_OFFSET ,
            ))
            . init () ;
        },
        "producer FIFO zeroing",
    );
    // No generated code arms doorbells (M6.5-T4): the router IRQs are
    // configured by the core pass's used-IRQ machinery.
    assert!(
        !generated.contains("doorbell_setup"),
        "the generated init hooks must not arm doorbells: {generated}"
    );
}

#[test]
fn receiver_codegen_init_hooks_snapshot() {
    let (_dir, path) = write_receiver_system();
    let generated = generate_receiver_ok(&path);

    // `app-m4` (global core 1) has no generated configure function: the
    // distribution tokens are inlined at `BeforeInit` (D1), and no generated
    // code arms the doorbell line (M6.5-T4).
    assert!(
        !generated.contains("__rticx_xbin_init_shared"),
        "a non-owner application must not initialize the shared state: {generated}"
    );
    assert!(
        !generated.contains("__rticx_xbin_configure_shared_memory"),
        "the configure wrapper function must be gone: {generated}"
    );
    assert!(
        !generated.contains("doorbell_setup"),
        "the generated init hooks must not arm doorbells: {generated}"
    );
}

#[test]
fn init_hooks_are_wired_into_the_entry_functions() {
    let (_dir, path) = write_both_system();

    let pass = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend);
    pass.run_pass(sender_args(), sender_app())
        .expect("sender code generation succeeds");

    // Every core configures its region view before the user `init` runs: the
    // distribution's tokens are inlined (D1).
    let before_init = pass
        .main_injection(&MainInjectionPoint::BeforeInit, 0)
        .expect("every core configures the shared memory before init");
    assert_section_present(
        &before_init.to_string(),
        quote! { __mock_xbin_configure () ; },
        "configure injection",
    );

    // The producing core zeroes the FIFOs it produces before `post_init`,
    // inside the interrupt-free critical section.
    let before_post_init = pass
        .main_injection(&MainInjectionPoint::BeforePostInit, 0)
        .expect("the producer initializes its FIFOs before post_init");
    assert_section_present(
        &before_post_init.to_string(),
        quote! { __rticx_xbin_init_fifos_core0 () ; },
        "producer FIFO init injection",
    );
    assert!(
        pass.main_injection(&MainInjectionPoint::BeforePostInit, 1)
            .is_none(),
        "local core 1 does not exist in this application"
    );

    // Boot sequencing is distribution-owned: no core signals readiness
    // before the idle loop anymore.
    assert!(
        pass.main_injection(&MainInjectionPoint::BeforeIdle, 0)
            .is_none(),
        "the framework no longer injects a readiness hook at idle"
    );
    assert!(
        pass.main_injection(&MainInjectionPoint::BeforeIdle, 1)
            .is_none(),
        "no local core injects at idle"
    );

    // The receiver application produces no FIFO, but wires its own configure
    // hook.
    let receiver = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend);
    receiver
        .run_pass(receiver_args(), receiver_app())
        .expect("receiver code generation succeeds");
    let before_init = receiver
        .main_injection(&MainInjectionPoint::BeforeInit, 0)
        .expect("the receiver configures the shared memory before init");
    assert_section_present(
        &before_init.to_string(),
        quote! { __mock_xbin_configure () ; },
        "receiver configure injection",
    );
    assert!(
        receiver
            .main_injection(&MainInjectionPoint::BeforePostInit, 0)
            .is_none(),
        "a pure receiver produces no FIFO to initialize"
    );
    assert!(
        receiver
            .main_injection(&MainInjectionPoint::BeforeIdle, 0)
            .is_none(),
        "the receiver no longer signals readiness"
    );
}

#[test]
fn fifo_init_follows_the_producing_core() {
    let receiver_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, capacity = 2, spawn_by = 5)]
            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };
    let receiver_args = quote!(device = mypac, dispatchers = [IRQ0]);
    let receiver_topology = AppTopology::new(vec![2], vec![5]);
    let sender_mod: syn::ItemMod = syn::parse_quote! {
        mod app {}
    };
    let sender_args = quote!(device = mypac);
    let sender_topology = AppTopology::new(vec![5], vec![2]);

    // app-m7 owns global core 5 and app-m4 owns global core 2: only the
    // producer core 5 initializes the `5 -> 2` FIFO, independently of the
    // global core ordering and of the declaration order (M6-T1).
    let view = system_with_patches(
        |view| {
            view["apps"][0]["core_ids"] = serde_json::json!([5]);
            view["apps"][0]["external_cores"] = serde_json::json!([2]);
            view["apps"][1]["core_ids"] = serde_json::json!([2]);
            view["apps"][1]["external_cores"] = serde_json::json!([5]);
            view["cores"][0]["global_id"] = serde_json::json!(5);
            view["cores"][1]["global_id"] = serde_json::json!(2);
            view["tasks"][0]["receiver_core"] = serde_json::json!(2);
            view["tasks"][0]["spawner_core"] = serde_json::json!(5);
            view["tasks"][0]["fifo"]["source"] = serde_json::json!(5);
            view["tasks"][0]["fifo"]["target"] = serde_json::json!(2);
            view["pools"][0]["core_a"] = serde_json::json!(2);
            view["pools"][0]["core_b"] = serde_json::json!(5);
            view["doorbells"][0]["source"] = serde_json::json!(5);
            view["doorbells"][0]["target"] = serde_json::json!(2);
        },
        &[
            (
                "app-m4",
                &receiver_args,
                &receiver_mod,
                receiver_topology.clone(),
            ),
            ("app-m7", &sender_args, &sender_mod, sender_topology.clone()),
        ],
    );
    let (_dir, path) = write_system(&view);

    let receiver = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology.clone())
        .with_backend(TestBackend);
    let (_, module) = receiver
        .run_pass(receiver_args, receiver_mod)
        .expect("the core-2 application generates code");
    let receiver_tokens = module.to_token_stream().to_string();
    assert!(
        !receiver_tokens.contains("__rticx_xbin_init_shared"),
        "the removed shared-state initializer is not generated: {receiver_tokens}"
    );
    assert!(
        !receiver_tokens.contains("__rticx_xbin_configure_shared_memory"),
        "the configure wrapper function must be gone: {receiver_tokens}"
    );
    assert!(
        receiver
            .main_injection(&MainInjectionPoint::BeforePostInit, 0)
            .is_none(),
        "the pure receiver produces no FIFO to initialize"
    );

    let sender = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology)
        .with_backend(TestBackend);
    let (_, module) = sender
        .run_pass(sender_args, sender_mod)
        .expect("the core-5 application generates code");
    let sender_tokens = module.to_token_stream().to_string();
    // The core-5 producer initializes its own `5 -> 2` FIFO before its
    // `post_init`, independently of the global core ordering (M6-T1).
    assert!(
        sender_tokens.contains("__rticx_xbin_init_fifos_core0"),
        "the producer must initialize its region: {sender_tokens}"
    );
    let before_post_init = sender
        .main_injection(&MainInjectionPoint::BeforePostInit, 0)
        .expect("the producer core initializes its FIFO before post_init");
    assert_section_present(
        &before_post_init.to_string(),
        quote! { __rticx_xbin_init_fifos_core0 () ; },
        "producer FIFO init injection",
    );
}

#[test]
fn metadata_mode_never_rewrites_receivers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pass = XbinPass::with_manifest(dir.path(), "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend);
    let (_, module) = pass
        .run_pass(receiver_args(), receiver_app())
        .expect("metadata pass succeeds");
    let generated = module.to_token_stream().to_string();

    assert!(
        !generated.contains("sw_task"),
        "the parsed attribute is stripped: {generated}"
    );
    assert!(!generated.contains("task_trait"), "{generated}");
    assert!(generated.contains("struct EncryptTask ;"), "{generated}");
    assert!(
        generated.contains("impl RticSwTask for EncryptTask"),
        "the user impl stays: {generated}"
    );
}

#[test]
fn stub_name_collision_is_reported() {
    let (_dir, path) = write_sender_system();
    // The collision check runs before the freshness checks, so the source
    // hash of the fixture view does not matter here.
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            struct EncryptTask;
        }
    };
    let error = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend)
        .run_pass(sender_args(), app_mod)
        .expect_err("a stub name collision must be rejected")
        .to_string();
    assert!(
        error.contains("generates `pub struct EncryptTask;` for the task spawned by global core 0"),
        "{error}"
    );
    assert!(
        error.contains("already defines an item with that name"),
        "{error}"
    );
}

#[test]
fn missing_system_view_is_reported() {
    let pass = XbinPass::with_system("/nonexistent/system.json", "app-m7", "m7")
        .with_topology(sender_topology())
        .with_backend(TestBackend);
    let error = pass
        .run_pass(sender_args(), sender_app())
        .expect_err("unsynced application must fail")
        .to_string();
    assert!(error.contains("failed to read the system view"), "{error}");
    assert!(error.contains("cargo xbin sync"), "{error}");
}

#[test]
fn missing_producer_backend_is_reported() {
    let (_dir, path) = write_base_system();
    let error = XbinPass::with_system(&path, "app-m7", "m7")
        .with_topology(sender_topology())
        .run_pass(sender_args(), sender_app())
        .expect_err("a producer without a backend must fail")
        .to_string();
    assert!(
        error.contains(
            "the synced system view spawns cross-binary tasks from this application, but the \
             distribution did not configure a cross-binary code-generation backend"
        ),
        "{error}"
    );
}

#[test]
fn stale_receiver_declarations_are_rejected() {
    // (application args, topology, module, expected error fragment)
    let cases: Vec<(TokenStream, AppTopology, syn::ItemMod, &str)> = vec![
        (
            receiver_args(),
            receiver_topology(),
            syn::parse_quote! {
                mod app {
                    trait RticSwTask {
                        type SpawnInput;
                        fn exec(&mut self, input: Self::SpawnInput);
                    }

                    #[sw_task(priority = 4, capacity = 2, spawn_by = 0)]
                    struct EncryptTask;

                    impl RticSwTask for EncryptTask {
                        type SpawnInput = ipc_types::EncryptReq;
                        fn exec(&mut self, _input: Self::SpawnInput) {}
                    }
                }
            },
            "declares priority 4, but the synced system view has 3",
        ),
        (
            receiver_args(),
            receiver_topology(),
            syn::parse_quote! {
                mod app {
                    trait RticSwTask {
                        type SpawnInput;
                        fn exec(&mut self, input: Self::SpawnInput);
                    }

                    #[sw_task(priority = 3, capacity = 1, spawn_by = 0)]
                    struct EncryptTask;

                    impl RticSwTask for EncryptTask {
                        type SpawnInput = ipc_types::EncryptReq;
                        fn exec(&mut self, _input: Self::SpawnInput) {}
                    }
                }
            },
            "declares capacity 1, but the synced system view has 2",
        ),
        (
            receiver_args(),
            receiver_topology(),
            syn::parse_quote! {
                mod app {
                    trait RticSwTask {
                        type SpawnInput;
                        fn exec(&mut self, input: Self::SpawnInput);
                    }

                    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
                    struct EncryptTask;

                    impl RticSwTask for EncryptTask {
                        type SpawnInput = ipc_types::OtherReq;
                        fn exec(&mut self, _input: Self::SpawnInput) {}
                    }
                }
            },
            "declares input type `ipc_types::OtherReq`, but the synced system view has `EncryptReq`",
        ),
        (
            receiver_args(),
            receiver_topology(),
            syn::parse_quote! {
                mod app {
                    trait RticSwTask {
                        type SpawnInput;
                        fn exec(&mut self, input: Self::SpawnInput);
                    }

                    #[sw_task(priority = 3, capacity = 2, spawn_by = 2)]
                    struct EncryptTask;

                    impl RticSwTask for EncryptTask {
                        type SpawnInput = ipc_types::EncryptReq;
                        fn exec(&mut self, _input: Self::SpawnInput) {}
                    }
                }
            },
            "declares `spawn_by = 2`, which is not a known core",
        ),
        (
            quote!(device = mypac, dispatchers = [IRQ0]),
            AppTopology::new(vec![7], vec![0]),
            receiver_app(),
            "maps its local cores to [7], but the synced system view has [1]",
        ),
        (
            quote!(device = mypac, dispatchers = [[], [IRQ0]]),
            AppTopology::new(vec![1, 7], vec![0]),
            syn::parse_quote! {
                mod app {
                    trait RticSwTask {
                        type SpawnInput;
                        fn exec(&mut self, input: Self::SpawnInput);
                    }

                    #[sw_task(core = 1, priority = 3, capacity = 2, spawn_by = 0)]
                    struct EncryptTask;

                    impl RticSwTask for EncryptTask {
                        type SpawnInput = ipc_types::EncryptReq;
                        fn exec(&mut self, _input: Self::SpawnInput) {}
                    }
                }
            },
            "declares local core 1, which maps to global core 7, but the synced system view runs \
             it on global core 1",
        ),
    ];

    // The `cores = 2` case needs a two-entry mapping in the app entry.
    let two_cores = system_with(|view| {
        view["apps"][1]["core_ids"] = serde_json::json!([1, 7]);
    });
    let (_dir, path) = write_base_system();
    let (_two_dir, two_path) = write_system(&two_cores);

    for (index, (args, topology, app_mod, expected)) in cases.into_iter().enumerate() {
        let path = if index == 5 { &two_path } else { &path };
        let error = XbinPass::with_system(path, "app-m4", "m4")
            .with_topology(topology)
            .with_backend(TestBackend)
            .run_pass(args, app_mod)
            .expect_err("stale declaration must be rejected")
            .to_string();
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
        if !expected.contains("not a known core") {
            assert!(error.contains("cargo xbin sync"), "{error}");
        }
    }
}

#[test]
fn a_view_task_without_receiver_declaration_is_rejected() {
    let stale = system_with(|view| {
        let mut second = view["tasks"][0].clone();
        second["name"] = serde_json::json!("OtherTask");
        second["id"] = serde_json::json!(2);
        view["tasks"].as_array_mut().expect("tasks").push(second);
    });
    let (_dir, path) = write_system(&stale);
    let error = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend)
        .run_pass(receiver_args(), receiver_app())
        .expect_err("an undrained view task must be rejected")
        .to_string();
    assert!(
        error.contains(
            "has task `OtherTask` for application `app-m4`, but the source declares \
                        no cross-binary receiver for it"
        ),
        "{error}"
    );
    assert!(error.contains("cargo xbin sync"), "{error}");
}

#[test]
fn a_task_in_an_unknown_pool_is_rejected() {
    let stale = system_with(|view| {
        view["tasks"][0]["fifo"]["pool"] = serde_json::json!("missing");
    });
    let (_dir, path) = write_system(&stale);
    let error = generate(&path)
        .expect_err("a task in an unknown pool must be rejected")
        .to_string();
    assert!(
        error.contains("places task `EncryptTask` in pool `missing`, which has no `pools[]` entry"),
        "{error}"
    );
    assert!(error.contains("cargo xbin sync"), "{error}");
}

#[test]
fn a_view_without_a_pool_needs_a_base_override() {
    // A project without a capability table (no `pools[]`, no task pool id)
    // cannot bake a literal address and the distribution provides no override,
    // so the pass must fail with the sync hint instead of emitting a view into
    // memory the distribution never reserved (D3).
    let stale = system_with(|view| {
        view["tasks"][0]["fifo"]["pool"] = serde_json::Value::Null;
        view["pools"] = serde_json::json!([]);
    });
    let (_dir, path) = write_system(&stale);
    let error = generate(&path)
        .expect_err("a pool-less view without an override must be rejected")
        .to_string();
    assert!(
        error.contains("has no IPC pool for the `(0 -> 1)` direction"),
        "{error}"
    );
    assert!(error.contains("cargo xbin sync"), "{error}");
}

#[test]
fn a_receiver_without_doorbell_is_rejected() {
    let stale = system_with(|view| {
        view["doorbells"] = serde_json::json!([]);
    });
    let (_dir, path) = write_system(&stale);
    let error = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend)
        .run_pass(receiver_args(), receiver_app())
        .expect_err("a task without doorbell line must be rejected")
        .to_string();
    assert!(
        error.contains("has no doorbell for `EncryptTask` (0 -> 1 at priority 3)"),
        "{error}"
    );
    assert!(error.contains("cargo xbin sync"), "{error}");
}

#[test]
fn missing_receiver_backend_is_reported() {
    let (_dir, path) = write_base_system();
    let error = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .run_pass(receiver_args(), receiver_app())
        .expect_err("a receiver without a backend must fail")
        .to_string();
    assert!(
        error.contains(
            "this application declares cross-binary receivers, but the distribution did not \
             configure a cross-binary code-generation backend"
        ),
        "{error}"
    );
}

#[test]
fn unknown_application_is_rejected() {
    let (_dir, path) = write_base_system();
    let error = XbinPass::with_system(&path, "app-m0", "m0")
        .with_topology(sender_topology())
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect_err("undeclared application must fail")
        .to_string();
    assert!(
        error.contains("has no application `app-m0` (target `m0`)"),
        "{error}"
    );
}

#[test]
fn applications_without_cross_declarations_still_load_the_view() {
    // `app-m4` neither produces a view task nor declares a receiver: it still
    // loads the view and gets its init hooks (M5.5 removed the gate).
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            struct Plain;
        }
    };
    let (_dir, path) = write_system(&with_interface_hashes(
        SYSTEM_JSON,
        &[(
            "app-m4",
            &receiver_args_without_cross_declarations(),
            &app_mod,
            receiver_topology(),
        )],
    ));
    let pass = XbinPass::with_system(&path, "app-m4", "m4")
        .with_topology(receiver_topology())
        .with_backend(TestBackend);
    let (_, module) = pass
        .run_pass(receiver_args_without_cross_declarations(), app_mod)
        .expect("a plain application still loads the view (M5.5)");
    let generated = module.to_token_stream().to_string();
    assert!(!generated.contains("cross_spawn"), "{generated}");
    assert!(
        !generated.contains("__rticx_xbin_configure_shared_memory"),
        "the configure tokens are inlined at `BeforeInit`, not a generated function (D1): {generated}"
    );
    assert!(
        !generated.contains("__rticx_xbin_init_fifos_core0"),
        "a non-producing application initializes no FIFO: {generated}"
    );
}

#[test]
fn codegen_mode_is_detected_from_the_environment() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    let system = dir.path().join("system.json");
    // No `rticx.toml` above the crate: the pass falls back to the identity
    // topology over `#[app(cores)]` (one local core, no external cores).
    std::fs::write(
        &system,
        with_interface_hashes(
            SYSTEM_JSON,
            &[(
                "app-m7",
                &sender_args(),
                &sender_app(),
                AppTopology::identity(1),
            )],
        ),
    )
    .expect("system.json");
    std::fs::write(dir.path().join(IPC_TYPES_FILE), IPC_TYPES_RS).expect("ipc_types.rs");

    // Without the variables, the pass is syntax-only in this workspace (no
    // `rticx.toml` above the crate).
    assert!(!XbinPass::from_env().is_codegen_mode());

    let _env = EnvGuard::set_all(&[
        (SYSTEM_ENV, system.as_os_str()),
        ("CARGO_PKG_NAME", std::ffi::OsStr::new("app-m7")),
        ("CARGO_BIN_NAME", std::ffi::OsStr::new("m7")),
    ]);
    let pass = XbinPass::from_env();
    assert!(pass.is_codegen_mode());
    assert!(!pass.is_metadata_mode());
    let (_, module) = pass
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect("codegen succeeds");
    assert!(module.to_token_stream().to_string().contains("cross_spawn"));
}

#[test]
fn codegen_mode_resolves_the_application_without_cargo_bin_name() {
    // rust-analyzer's proc-macro server does not set `CARGO_BIN_NAME` (its
    // `inject_cargo_package_env` leaves the variable out), unlike cargo. The
    // pass must still resolve the application by package so the IDE expands
    // `#[app]` instead of reporting a spurious
    // "no application `app-m7` (target `app-m7`); run `cargo xbin sync`".
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    let system = dir.path().join("system.json");
    // No `rticx.toml`: the resolved topology is the identity over one core.
    std::fs::write(
        &system,
        with_interface_hashes(
            SYSTEM_JSON,
            &[(
                "app-m7",
                &sender_args(),
                &sender_app(),
                AppTopology::identity(1),
            )],
        ),
    )
    .expect("system.json");
    std::fs::write(dir.path().join(IPC_TYPES_FILE), IPC_TYPES_RS).expect("ipc_types.rs");

    let _env = EnvGuard::set_all(&[
        (SYSTEM_ENV, system.as_os_str()),
        ("CARGO_PKG_NAME", std::ffi::OsStr::new("app-m7")),
    ]);
    let _unset = EnvGuard::unset_all(&["CARGO_BIN_NAME"]);

    let pass = XbinPass::from_env();
    assert!(pass.is_codegen_mode());
    let (_, module) = pass
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect("a package-only environment still resolves the application");
    assert!(module.to_token_stream().to_string().contains("cross_spawn"));
}

#[test]
fn project_topology_anchor_is_embedded() {
    // A topology resolved from `rticx.toml` embeds the project file with
    // `include_str!`, so a plain `cargo build` tracks topology edits through
    // rustc's dep-info (M7-T2).
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), PROJECT_TOML).expect("rticx.toml");
    let system = dir.path().join("system.json");
    std::fs::write(
        &system,
        with_layout_hash(
            &with_interface_hashes(
                SYSTEM_JSON,
                &[("app-m7", &sender_args(), &sender_app(), sender_topology())],
            ),
            idl_hash(IDL_TOML),
        ),
    )
    .expect("system.json");
    std::fs::write(dir.path().join(IPC_TYPES_FILE), IPC_TYPES_RS).expect("ipc_types.rs");
    std::fs::write(dir.path().join("ipc-types.toml"), IDL_TOML).expect("ipc-types.toml");

    let _env = EnvGuard::set_all(&[
        (SYSTEM_ENV, system.as_os_str()),
        ("CARGO_MANIFEST_DIR", dir.path().as_os_str()),
        ("CARGO_PKG_NAME", std::ffi::OsStr::new("app-m7")),
        ("CARGO_BIN_NAME", std::ffi::OsStr::new("m7")),
    ]);

    let pass = XbinPass::from_env();
    assert!(pass.is_codegen_mode());
    let (_, module) = pass
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect("codegen succeeds");
    let generated = module.to_token_stream().to_string();
    assert!(
        generated.contains("__RTICX_XBIN_PROJECT_TOML"),
        "the `rticx.toml` freshness anchor is missing: {generated}"
    );
}

/// The layout hash of an IDL source, as `cargo xbin sync` records it.
fn idl_hash(source: &str) -> Hash64 {
    Hash64::new(
        layout_hash(&parse_idl_str(source).expect("the fixture IDL parses"))
            .expect("the fixture IDL lays out"),
    )
}

/// The layout hash of a project without an `ipc-types.toml`, as `system_view`
/// records it (it lays out [`IpcTypes::empty`], not `Hash64::ZERO`).
fn empty_idl_hash() -> Hash64 {
    Hash64::new(layout_hash(&IpcTypes::empty()).expect("the empty IDL lays out"))
}

/// Records `layout_hash` in an already-patched fixture view and reseals it, so
/// the recorded layout matches the current IDL the test writes.
fn with_layout_hash(contents: &str, layout_hash: Hash64) -> String {
    let mut view = SystemView::from_json(contents).expect("fixture JSON");
    view.layout_hash = layout_hash;
    view.seal();
    view.to_json()
}

/// Writes a project `from_env` can resolve (`rticx.toml`), the given
/// `system.json` and the generated `ipc_types.rs` into a fresh directory.
///
/// The caller adds `ipc-types.toml` when the test needs one.
fn write_project_dir(system: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), PROJECT_TOML).expect("rticx.toml");
    let path = dir.path().join("system.json");
    std::fs::write(&path, system).expect("system.json");
    std::fs::write(dir.path().join(IPC_TYPES_FILE), IPC_TYPES_RS).expect("ipc_types.rs");
    (dir, path)
}

/// Runs the sender fixture through `from_env` over `dir`/`system` and returns
/// the generated module token string. The caller holds [`ENV_LOCK`].
fn generate_from_project(dir: &Path, system: &Path) -> syn::Result<String> {
    let _env = EnvGuard::set_all(&[
        (SYSTEM_ENV, system.as_os_str()),
        ("CARGO_MANIFEST_DIR", dir.as_os_str()),
        ("CARGO_PKG_NAME", std::ffi::OsStr::new("app-m7")),
        ("CARGO_BIN_NAME", std::ffi::OsStr::new("m7")),
    ]);
    let pass = XbinPass::from_env();
    let (_, module) = pass
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())?;
    Ok(module.to_token_stream().to_string())
}

/// A view whose recorded `layout_hash` matches the project's current IDL
/// builds (M3-T4).
#[test]
fn idl_hash_matching_the_synced_view_passes() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let system = with_layout_hash(
        &with_interface_hashes(
            SYSTEM_JSON,
            &[("app-m7", &sender_args(), &sender_app(), sender_topology())],
        ),
        idl_hash(IDL_TOML),
    );
    let (dir, path) = write_project_dir(&system);
    std::fs::write(dir.path().join("ipc-types.toml"), IDL_TOML).expect("ipc-types.toml");

    generate_from_project(dir.path(), &path).expect("a matching IDL must pass");
}

/// Editing the IDL after `cargo xbin sync` is a hard error naming the IDL and
/// `cargo xbin sync` (M3-T4).
#[test]
fn idl_hash_mismatch_is_reported() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    // The view recorded the empty IDL's layout; the project now has EncryptReq.
    let synced = empty_idl_hash();
    let system = with_layout_hash(
        &with_interface_hashes(
            SYSTEM_JSON,
            &[("app-m7", &sender_args(), &sender_app(), sender_topology())],
        ),
        synced,
    );
    let (dir, path) = write_project_dir(&system);
    std::fs::write(dir.path().join("ipc-types.toml"), IDL_TOML).expect("ipc-types.toml");

    let error = generate_from_project(dir.path(), &path)
        .expect_err("a changed IDL must fail")
        .to_string();
    let live = idl_hash(IDL_TOML);
    assert_eq!(
        error,
        format!(
            "the IDL `ipc-types.toml` changed since the last `cargo xbin sync` (the system view \
             records layout hash {synced}, the current IDL hashes to {live}); run `cargo xbin sync`"
        )
    );
}

/// A project without an `ipc-types.toml` compares against [`IpcTypes::empty`],
/// matching the sentinel `system_view` records; a view that recorded a real
/// layout while the file is gone is stale (M3-T4).
#[test]
fn missing_idl_compares_against_the_empty_idl() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let patches = [("app-m7", &sender_args(), &sender_app(), sender_topology())];

    // No `ipc-types.toml`, and the view recorded the empty IDL's layout.
    let system = with_layout_hash(
        &with_interface_hashes(SYSTEM_JSON, &patches),
        empty_idl_hash(),
    );
    let (dir, path) = write_project_dir(&system);
    generate_from_project(dir.path(), &path).expect("the empty IDL is the no-file sentinel");

    // No `ipc-types.toml`, but the view recorded EncryptReq's layout.
    let synced = idl_hash(IDL_TOML);
    let system = with_layout_hash(&with_interface_hashes(SYSTEM_JSON, &patches), synced);
    let (dir, path) = write_project_dir(&system);
    let error = generate_from_project(dir.path(), &path)
        .expect_err("a deleted IDL must fail")
        .to_string();
    assert_eq!(
        error,
        format!(
            "the IDL `ipc-types.toml` changed since the last `cargo xbin sync` (the system view \
             records layout hash {synced}, the current IDL hashes to {}); run `cargo xbin sync`",
            empty_idl_hash()
        )
    );
}

/// Codegen embeds `ipc-types.toml` with `include_str!` when it exists, so a
/// cargo-cached build re-runs the macro (and the freshness check fires) after
/// an IDL edit; a project without the file emits nothing (M3-T4).
#[test]
fn idl_freshness_anchor_is_emitted_when_the_idl_exists() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let patches = [("app-m7", &sender_args(), &sender_app(), sender_topology())];

    let system = with_layout_hash(
        &with_interface_hashes(SYSTEM_JSON, &patches),
        idl_hash(IDL_TOML),
    );
    let (dir, path) = write_project_dir(&system);
    std::fs::write(dir.path().join("ipc-types.toml"), IDL_TOML).expect("ipc-types.toml");
    let generated = generate_from_project(dir.path(), &path).expect("codegen succeeds");
    assert!(
        generated.contains("const __RTICX_XBIN_IDL : & str = include_str !"),
        "the `ipc-types.toml` dep-info anchor is missing: {generated}"
    );

    let system = with_layout_hash(
        &with_interface_hashes(SYSTEM_JSON, &patches),
        empty_idl_hash(),
    );
    let (dir, path) = write_project_dir(&system);
    let generated = generate_from_project(dir.path(), &path).expect("codegen succeeds");
    assert!(
        !generated.contains("__RTICX_XBIN_IDL"),
        "a project without an IDL must not emit the anchor: {generated}"
    );
}

/// Builds a one-receiver `#[app]` module: a `#[sw_task(<attr>)]` struct named
/// `name` whose `impl RticSwTask` declares `input_type` as its `SpawnInput`.
///
/// The fingerprint tests below vary one declaration at a time through this
/// helper.
fn receiver_module(name: &str, attr: &str, input_type: &str) -> syn::ItemMod {
    let source = format!(
        "\
mod app {{
    trait RticSwTask {{
        type SpawnInput;
        fn exec(&mut self, input: Self::SpawnInput);
    }}

    #[sw_task({attr})]
    struct {name};

    impl RticSwTask for {name} {{
        type SpawnInput = {input_type};
        fn exec(&mut self, _input: Self::SpawnInput) {{}}
    }}
}}"
    );
    syn::parse_str(&source).expect("the generated receiver module parses")
}

/// Parses `(args, app_mod)` with `topology` and returns the interface
/// fingerprint the pass would record, as `cargo xbin sync` does.
fn interface_hash_of(args: &TokenStream, app_mod: &syn::ItemMod, topology: &AppTopology) -> Hash64 {
    rticx_xbin_pass::app_interface_hash(args, app_mod, Some(topology))
        .expect("the fixture interface parses")
}

#[test]
fn interface_hash_ignores_task_bodies_and_doc_comments() {
    // The user's original complaint: an edit inside a task body (or a doc
    // comment) must not make a synced application look stale.
    let topology = receiver_topology();
    let args = receiver_args();

    let original: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            /// The receiver's original documentation.
            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {
                    // original body
                }
            }
        }
    };
    let edited: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            /// A completely different doc comment.
            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {
                    let _different = 1 + 1;
                }
            }
        }
    };

    assert_eq!(
        interface_hash_of(&args, &original, &topology),
        interface_hash_of(&args, &edited, &topology),
    );
}

#[test]
fn interface_hash_ignores_local_task_declarations() {
    // Local `#[sw_task]` declarations (and their priorities) are not part of
    // the cross-binary interface: phase 2 re-validates them from the current
    // source.
    let topology = receiver_topology();
    let args = receiver_args();

    let without: syn::ItemMod = syn::parse_quote! {
        mod app {
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
    let added: syn::ItemMod = syn::parse_quote! {
        mod app {
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

            #[sw_task(priority = 5)]
            struct LocalTask;
        }
    };
    let changed: syn::ItemMod = syn::parse_quote! {
        mod app {
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

            #[sw_task(priority = 7)]
            struct LocalTask;
        }
    };

    let base = interface_hash_of(&args, &without, &topology);
    assert_eq!(
        base,
        interface_hash_of(&args, &added, &topology),
        "adding a local task must not change the interface fingerprint"
    );
    assert_eq!(
        base,
        interface_hash_of(&args, &changed, &topology),
        "changing a local task priority must not change the interface fingerprint"
    );
}

#[test]
fn interface_hash_ignores_shared_and_device() {
    // `shared` and `device`/`pacs` are re-derived in phase 2 from the current
    // source and have no cross-binary layout effect.
    let topology = receiver_topology();
    let args_a = quote!(device = pac_a, dispatchers = [IRQ0]);
    let args_b = quote!(device = pac_b, dispatchers = [IRQ0]);

    let base = receiver_module(
        "EncryptTask",
        "priority = 3, capacity = 2, spawn_by = 0",
        "ipc_types::EncryptReq",
    );
    let shared = receiver_module(
        "EncryptTask",
        "priority = 3, capacity = 2, spawn_by = 0, shared = [counter]",
        "ipc_types::EncryptReq",
    );

    assert_eq!(
        interface_hash_of(&args_a, &base, &topology),
        interface_hash_of(&args_a, &shared, &topology),
        "a receiver's `shared` list must not change the interface fingerprint"
    );
    assert_eq!(
        interface_hash_of(&args_a, &base, &topology),
        interface_hash_of(&args_b, &base, &topology),
        "the `device` PAC must not change the interface fingerprint"
    );
}

#[test]
fn interface_hash_changes_on_receiver_field() {
    // Every cross-binary-sensitive receiver field must move the fingerprint.
    let topology = AppTopology::new(vec![1, 2], vec![0, 3]);
    let args = quote!(device = mypac, dispatchers = [[IRQ0], [IRQ1]]);
    let base = receiver_module(
        "EncryptTask",
        "priority = 3, capacity = 2, spawn_by = 0",
        "ipc_types::EncryptReq",
    );
    let base_hash = interface_hash_of(&args, &base, &topology);

    let variants = [
        (
            "name",
            receiver_module(
                "OtherTask",
                "priority = 3, capacity = 2, spawn_by = 0",
                "ipc_types::EncryptReq",
            ),
        ),
        (
            "priority",
            receiver_module(
                "EncryptTask",
                "priority = 4, capacity = 2, spawn_by = 0",
                "ipc_types::EncryptReq",
            ),
        ),
        (
            "capacity",
            receiver_module(
                "EncryptTask",
                "priority = 3, capacity = 5, spawn_by = 0",
                "ipc_types::EncryptReq",
            ),
        ),
        (
            "core",
            receiver_module(
                "EncryptTask",
                "priority = 3, capacity = 2, spawn_by = 0, core = 1",
                "ipc_types::EncryptReq",
            ),
        ),
        (
            "spawn_by",
            receiver_module(
                "EncryptTask",
                "priority = 3, capacity = 2, spawn_by = 3",
                "ipc_types::EncryptReq",
            ),
        ),
        (
            "input_type",
            receiver_module(
                "EncryptTask",
                "priority = 3, capacity = 2, spawn_by = 0",
                "ipc_types::OtherReq",
            ),
        ),
    ];

    for (field, variant) in variants {
        assert_ne!(
            base_hash,
            interface_hash_of(&args, &variant, &topology),
            "changing the receiver `{field}` must change the interface fingerprint",
        );
    }
}

#[test]
fn interface_hash_changes_on_stolen_dispatcher_and_ignores_surplus() {
    // Only the entries the pass steals for the cross-binary lines matter; the
    // surplus entries are local software-task dispatchers.
    let topology = receiver_topology();
    let module = receiver_module(
        "EncryptTask",
        "priority = 3, capacity = 2, spawn_by = 0",
        "ipc_types::EncryptReq",
    );

    let stolen = quote!(device = mypac, dispatchers = [IRQ_CROSS]);
    let stolen_changed = quote!(device = mypac, dispatchers = [IRQ_OTHER]);
    let surplus_changed = quote!(device = mypac, dispatchers = [IRQ_CROSS, IRQ_LOCAL]);
    let surplus_added = quote!(
        device = mypac,
        dispatchers = [IRQ_CROSS, IRQ_LOCAL, IRQ_MORE]
    );

    let base = interface_hash_of(&stolen, &module, &topology);
    assert_ne!(
        base,
        interface_hash_of(&stolen_changed, &module, &topology),
        "the stolen cross dispatcher identity must change the interface fingerprint"
    );
    assert_eq!(
        base,
        interface_hash_of(&surplus_changed, &module, &topology),
        "a surplus local dispatcher must not change the interface fingerprint"
    );
    assert_eq!(
        base,
        interface_hash_of(&surplus_added, &module, &topology),
        "adding a local dispatcher must not change the interface fingerprint"
    );
}

#[test]
fn interface_hash_changes_on_topology() {
    // The resolved topology is part of the interface the driver merges.
    let args = quote!(device = mypac, dispatchers = []);
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {}
    };

    let base = interface_hash_of(&args, &module, &AppTopology::new(vec![0], vec![1]));
    assert_ne!(
        base,
        interface_hash_of(&args, &module, &AppTopology::new(vec![5], vec![1])),
        "a different `core_ids` mapping must change the interface fingerprint"
    );
    assert_ne!(
        base,
        interface_hash_of(&args, &module, &AppTopology::new(vec![0], vec![1, 2])),
        "a different `external_cores` list must change the interface fingerprint"
    );
}

#[test]
fn interface_hash_ignores_token_spacing() {
    // rustc and rust-analyzer's proc-macro server represent the same token
    // tree with different punctuation `Spacing`; the fingerprint renders the
    // stolen dispatcher paths structurally and must not depend on it.
    use proc_macro2::{Delimiter, Group, Punct, Spacing, TokenStream, TokenTree};

    let topology = receiver_topology();
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct AlphaTask;

            impl RticSwTask for AlphaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(priority = 4, capacity = 2, spawn_by = 0)]
            struct BetaTask;

            impl RticSwTask for BetaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };

    let args = |spacing| {
        let dispatchers = TokenStream::from_iter([
            TokenTree::Ident(format_ident!("IRQ0")),
            TokenTree::Punct(Punct::new(',', spacing)),
            TokenTree::Ident(format_ident!("IRQ1")),
        ]);
        TokenStream::from_iter([
            TokenTree::Ident(format_ident!("device")),
            TokenTree::Punct(Punct::new('=', Spacing::Alone)),
            TokenTree::Ident(format_ident!("mypac")),
            TokenTree::Punct(Punct::new(',', Spacing::Alone)),
            TokenTree::Ident(format_ident!("dispatchers")),
            TokenTree::Punct(Punct::new('=', Spacing::Alone)),
            TokenTree::Group(Group::new(Delimiter::Bracket, dispatchers)),
        ])
    };

    assert_eq!(
        interface_hash_of(&args(Spacing::Joint), &module, &topology),
        interface_hash_of(&args(Spacing::Alone), &module, &topology),
    );
}

#[test]
fn discovered_project_root_without_a_synced_view_is_a_hard_error() {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("rticx.toml"),
        "schema = 1\n\n\
         [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
         core_ids = [0]\nexternal_cores = [1]\n",
    )
    .expect("rticx.toml");

    // A discovered project root selects codegen mode even before the first
    // `cargo xbin sync` wrote the view, so any application fails instead of
    // silently generating no code (M5.5 removed the cross-declaration gate).
    let _env = EnvGuard::set_all(&[
        ("CARGO_MANIFEST_DIR", dir.path().as_os_str()),
        ("CARGO_PKG_NAME", std::ffi::OsStr::new("app-m7")),
        ("CARGO_BIN_NAME", std::ffi::OsStr::new("m7")),
    ]);
    let pass = XbinPass::from_env();
    assert!(pass.is_codegen_mode());

    let error = pass
        .with_backend(TestBackend)
        .run_pass(sender_args(), sender_app())
        .expect_err("an application without a synced view must fail")
        .to_string();
    assert!(error.contains("failed to read the system view"), "{error}");
    assert!(error.contains("run `cargo xbin sync`"), "{error}");
}
