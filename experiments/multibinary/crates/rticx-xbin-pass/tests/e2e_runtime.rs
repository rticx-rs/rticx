//! M4-T2 acceptance: spawn from the sender application, execute on the
//! receiver application's generated doorbell dispatcher, verify the input
//! value, and observe the FIFO backpressure semantics.
//!
//! Both fixture applications are expanded with the **real** phase-2 pipeline —
//! the sender through [`XbinPass`] (its expansion is a plain module: no core
//! pass runs and the fixture provides `__rticx_interrupt_free`), the receiver
//! through the full [`RticMacroBuilder`] on `MockCoreBackend` (so the receiver
//! struct becomes a core task and the dispatcher becomes a hardware task) —
//! and written into one throwaway binary together with the in-tree
//! `rticx-xbin-mock` runtime.
//!
//! The binary is `#![no_main]`: the receiver expansion generates the project
//! entry (`main`) inside its `#[app]` module, so running it executes the real
//! boot sequence (`init`, `post_init` init hooks, idle). The idle task then
//! drives the cross-binary scenario:
//!
//! 1. it finishes the mock boot (the sender owns global core 0, so it calls
//!    the sender's `configure`/`init_shared`/`mark_ready` hooks);
//! 2. `EncryptTask::cross_spawn(input)` enqueues and rings the doorbell; the
//!    input is not executed until the dispatcher runs;
//! 3. calling the generated doorbell ISR (`__xbin_doorbell_1_0`) drains the
//!    FIFO and calls the receiver task's `exec`, which records the input;
//! 4. a full FIFO (capacity 2, depth 3) makes `cross_spawn` return
//!    `Err(Some(input))` with the rejected input; draining frees it again;
//! 5. repeated spawn/drain cycles wrap the ring and keep the order.
//!
//! Both applications share one process-global [`MockSystem`] through
//! `crate::backend_for`, so the sender and receiver see the same region,
//! doorbell and ready/epoch state, exactly like two cores in one project.
//!
//! [`MockSystem`]: rticx_xbin_mock::MockSystem

use std::path::{Path, PathBuf};
use std::process::Command;

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{RticMacroBuilder, RticPass};
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use rticx_xbin_proto::SystemView;

/// A two-application system view: `app-m7` (global core 0) spawns
/// `EncryptTask` on `app-m4` (global core 1) at priority 3, capacity 2.
const SYSTEM_JSON: &str = r#"{
  "schema_version": 1,
  "rticx_generation": "0.2",
  "topology_hash": "0x2b7f11101722fb4e",
  "layout_hash": "0x59021e1c399c4ddb",
  "apps": [
    {
      "package": "app-m7",
      "target": { "kind": "bin", "name": "m7" },
      "source_hash": "0x26a74c0569235dd7",
      "core_ids": [0],
      "external_cores": [1]
    },
    {
      "package": "app-m4",
      "target": { "kind": "bin", "name": "m4" },
      "source_hash": "0x70b1fa36c7b61323",
      "core_ids": [1],
      "external_cores": [0]
    }
  ],
  "cores": [
    { "global_id": 0, "app": "app-m7", "local_index": 0 },
    { "global_id": 1, "app": "app-m4", "local_index": 0 }
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
      "spawner_cores": [0],
      "priority": 3,
      "capacity": 2,
      "input_type": "EncryptReq",
      "fifo": { "source": 0, "target": 1, "offset": 0, "elem_size": 12, "depth": 3 }
    }
  ],
  "regions": [
    {
      "source": 0,
      "target": 1,
      "base_from_source": "0x30040000",
      "base_from_target": "0x30040000",
      "size": 4096
    }
  ],
  "doorbells": [
    { "source": 0, "target": 1, "priority": 3, "line": 0 }
  ]
}
"#;

/// Code-generation backend of the harness.
struct TestBackend;

impl XbinPassBackend for TestBackend {
    fn backend(&self) -> syn::Expr {
        syn::parse_quote!(__rticx_xbin_backend())
    }

    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt)
    }

    fn dispatcher_irq(&self, target: u32, line: u32) -> syn::Ident {
        format_ident!("__xbin_doorbell_{target}_{line}")
    }

    fn doorbell_irq(&self, target: u32, line: u32) -> u16 {
        (target * 16 + line) as u16
    }
}

/// The sender fixture application: one `#[cross_bin_spawn]` stub plus the
/// pieces its codegen-only expansion needs and `pub` wrappers over the
/// generated private init hooks.
///
/// The sender is expanded through [`XbinPass`] alone, so no core pass provides
/// `__rticx_interrupt_free` (the pass generated `cross_spawn` calls it): the
/// fixture supplies the host no-op itself, like a distribution would provide
/// the target's critical section.
fn sender_module() -> syn::ItemMod {
    syn::parse_quote! {
        pub mod sender_app {
            fn __rticx_xbin_backend() -> rticx_xbin_mock::MockBackend {
                crate::backend_for(0)
            }

            fn __rticx_interrupt_free<R>(f: impl FnOnce() -> R) -> R {
                f()
            }

            #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
            pub struct EncryptTask;

            pub fn test_configure() {
                __rticx_xbin_configure_shared_memory(&__rticx_xbin_backend());
            }

            pub fn test_init_shared() {
                __rticx_xbin_init_shared(&__rticx_xbin_backend());
            }

            pub fn test_mark_ready() {
                __rticx_xbin_mark_ready_core0(&__rticx_xbin_backend());
            }
        }
    }
}

fn sender_args() -> TokenStream {
    quote!(
        device = pac,
        cores = 1,
        core_ids = [0],
        external_cores = [1]
    )
}

/// The receiver fixture application: the task, a log of executed inputs and
/// the scenario driver in `#[idle]`.
///
/// The receiver runs through the full core pass, so the generated entry boots
/// the application (init, init hooks, idle). The driver runs after the boot
/// sequence, from the idle task, and calls the generated doorbell ISR
/// directly — the mock backend has no interrupt controller, so the idle task
/// stands in for the hardware that would invoke it.
fn receiver_module() -> syn::ItemMod {
    syn::parse_quote! {
        pub mod receiver_app {
            use std::sync::Mutex;

            use rticx_xbin_rt::backend::CrossBinBackend;

            fn __rticx_xbin_backend() -> rticx_xbin_mock::MockBackend {
                crate::backend_for(1)
            }

            fn request(addr: u32) -> ipc_types::EncryptReq {
                ipc_types::EncryptReq {
                    addr,
                    len: addr + 10,
                    key: addr + 20,
                }
            }

            /// Inputs executed by the receiver task, in dispatcher order.
            pub static RECEIVED: Mutex<Vec<ipc_types::EncryptReq>> = Mutex::new(Vec::new());

            trait CrossBinTask {
                type Input;
                fn exec(&mut self, input: Self::Input);
            }

            #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
            pub struct EncryptTask;

            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;

                fn exec(&mut self, input: Self::Input) {
                    RECEIVED
                        .lock()
                        .expect("the receiver log is never poisoned")
                        .push(input);
                }
            }

            pub fn test_configure() {
                __rticx_xbin_configure_shared_memory(&__rticx_xbin_backend());
            }

            pub fn test_mark_ready() {
                __rticx_xbin_mark_ready_core0(&__rticx_xbin_backend());
            }

            #[idle]
            struct Idle;

            impl RticIdleTask for Idle {
                fn exec(&mut self) -> ! {
                    // Complete the mock boot: global core 0 owns the shared
                    // state, so its hooks run here (the receiver's own
                    // `configure` and `mark_ready` ran in the generated entry).
                    crate::sender_app::test_configure();
                    crate::sender_app::test_init_shared();
                    crate::sender_app::test_mark_ready();
                    test_mark_ready();

                    let backend = __rticx_xbin_backend();
                    assert!(backend.shared_state().is_ready(0), "the owner core is ready");
                    assert!(backend.shared_state().is_ready(1), "the receiver core is ready");

                    // -- spawn enqueues and rings; only the dispatcher executes
                    crate::sender_app::EncryptTask::cross_spawn(request(1))
                        .expect("the first spawn enqueues");
                    assert!(
                        backend.doorbell_take(1, 0),
                        "the spawn rings the target doorbell"
                    );
                    assert!(
                        RECEIVED.lock().expect("the receiver log").is_empty(),
                        "nothing executes before the dispatcher runs"
                    );

                    __xbin_doorbell_1_0();
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").as_slice(),
                        &[request(1)],
                        "the dispatcher executed the spawned input"
                    );

                    // -- a full FIFO (capacity 2) rejects the third input
                    crate::sender_app::EncryptTask::cross_spawn(request(2))
                        .expect("the second spawn enqueues");
                    crate::sender_app::EncryptTask::cross_spawn(request(3))
                        .expect("the third spawn enqueues");
                    assert_eq!(
                        crate::sender_app::EncryptTask::cross_spawn(request(4)),
                        Err(Some(request(4))),
                        "a full FIFO returns the input to the spawner"
                    );

                    __xbin_doorbell_1_0();
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").as_slice(),
                        &[request(1), request(2), request(3)],
                        "the dispatcher drains in FIFO order"
                    );

                    // -- the ring wraps: depth (capacity + 1) slots are reused
                    for addr in 10..16 {
                        crate::sender_app::EncryptTask::cross_spawn(request(addr))
                            .expect("a wrapping spawn enqueues");
                        __xbin_doorbell_1_0();
                    }
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").len(),
                        9,
                        "every spawn past the ring depth was executed"
                    );

                    println!("xbin: e2e ok");
                    std::process::exit(0);
                }
            }

            #[init]
            fn init() -> TaskInits {
                TaskInits { idle: Idle }
            }
        }
    }
}

fn receiver_args() -> TokenStream {
    quote!(
        device = pac,
        cores = 1,
        core_ids = [1],
        external_cores = [0]
    )
}

/// The fixture view with both applications' source hashes recorded, as
/// `cargo xbin sync` would (M3-T4).
fn system() -> String {
    let mut view = SystemView::from_json(SYSTEM_JSON).expect("fixture system view");
    for (package, args, app_mod) in [
        ("app-m7", &sender_args(), &sender_module()),
        ("app-m4", &receiver_args(), &receiver_module()),
    ] {
        let application = view
            .apps
            .iter_mut()
            .find(|application| application.package == package)
            .expect("fixture application");
        application.source_hash = rticx_xbin_pass::app_source_hash(args, app_mod);
    }
    view.seal();
    view.to_json()
}

/// Expands the sender fixture through the cross-binary pass (no core pass).
fn expand_sender(system: &Path) -> String {
    let pass = XbinPass::with_system(system, "app-m7", "m7").with_backend(TestBackend);
    let (_, module) = pass
        .run_pass(sender_args(), sender_module())
        .expect("sender code generation succeeds");
    module.to_token_stream().to_string()
}

/// Expands the receiver fixture through the cross-binary pass and the full
/// core pass.
fn expand_receiver(system: &Path) -> String {
    let pass = XbinPass::with_system(system, "app-m4", "m4").with_backend(TestBackend);
    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(pass);
    builder
        .build_rtic_macro2(receiver_args(), receiver_module(), None)
        .to_string()
}

fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Writes the throwaway `#![no_main]` binary executing both expansions.
fn write_project(root: &Path, sender: &str, receiver: &str) {
    let rt = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rticx-xbin-rt")
        .canonicalize()
        .expect("the runtime crate ships with the workspace");
    let mock = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rticx-xbin-mock")
        .canonicalize()
        .expect("the mock crate ships with the workspace");

    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"xbin-e2e-runtime\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             publish = false\n\n\
             [dependencies]\n\
             pac = {{ path = \"pac\" }}\n\
             ipc-types = {{ path = \"ipc-types\" }}\n\
             rticx-xbin-rt = {{ path = \"{}\" }}\n\
             rticx-xbin-mock = {{ path = \"{}\" }}\n\n\
             [workspace]\n",
            rt.display(),
            mock.display()
        ),
    )
    .expect("project manifest");
    std::fs::write(root.join("src/sender.rs"), format!("{sender}\n")).expect("sender expansion");
    std::fs::write(root.join("src/receiver.rs"), format!("{receiver}\n"))
        .expect("receiver expansion");
    std::fs::write(
        root.join("src/main.rs"),
        "// @generated by the M4-T2 end-to-end runtime test\n\
         #![no_main]\n\
         #![allow(dead_code, unused_imports, unused_variables, non_snake_case, \
         non_upper_case_globals, static_mut_refs)]\n\n\
         use std::sync::LazyLock;\n\n\
         use rticx_xbin_mock::{MockBackend, MockSystem};\n\n\
         include!(\"sender.rs\");\n\
         include!(\"receiver.rs\");\n\n\
         /// The process-global mock system shared by both applications, so a\n\
         /// spawn on core 0 reaches the dispatcher of core 1.\n\
         fn system() -> &'static MockSystem {\n\
             static SYSTEM: LazyLock<MockSystem> = LazyLock::new(|| {\n\
                 let mut system = MockSystem::new();\n\
                 system\n\
                     .add_region(0, 1, 4096)\n\
                     .expect(\"the fixture declares the `0->1` region\");\n\
                 system\n\
             });\n\
             &SYSTEM\n\
         }\n\n\
         fn backend_for(core: u32) -> MockBackend {\n\
             system().backend(core)\n\
         }\n",
    )
    .expect("harness source");

    std::fs::create_dir_all(root.join("pac/src")).expect("pac dir");
    std::fs::write(
        root.join("pac/Cargo.toml"),
        "[package]\nname = \"pac\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n",
    )
    .expect("pac manifest");
    std::fs::write(root.join("pac/src/lib.rs"), "").expect("pac source");

    std::fs::create_dir_all(root.join("ipc-types/src")).expect("ipc-types dir");
    std::fs::write(
        root.join("ipc-types/Cargo.toml"),
        format!(
            "[package]\nname = \"ipc-types\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             publish = false\n\n\
             [dependencies]\nrticx-xbin-rt = {{ path = \"{}\" }}\n",
            rt.display()
        ),
    )
    .expect("ipc-types manifest");
    std::fs::write(
        root.join("ipc-types/src/lib.rs"),
        "//! Stand-in for the `cargo xbin sync`-generated crate.\n\
         #![no_std]\n\n\
         #[repr(C)]\n\
         #[derive(Clone, Copy, PartialEq, Eq, Debug)]\n\
         pub struct EncryptReq {\n\
             pub addr: u32,\n\
             pub len: u32,\n\
             pub key: u32,\n\
         }\n\n\
         const _: () = {\n\
             assert!(core::mem::size_of::<EncryptReq>() == 12);\n\
             assert!(core::mem::align_of::<EncryptReq>() == 4);\n\
         };\n\n\
         unsafe impl rticx_xbin_rt::CrossCoreMessage for EncryptReq {}\n",
    )
    .expect("ipc-types source");
}

#[test]
fn mock_runtime_spawn_reaches_the_receiver_dispatcher() {
    let dir = tempfile::tempdir().expect("tempdir");
    let system_path = dir.path().join("system.json");
    std::fs::write(&system_path, system()).expect("system.json");

    let sender = expand_sender(&system_path);
    assert!(
        !sender.contains("compile_error"),
        "sender code generation failed: {sender}"
    );
    assert!(
        sender.contains("cross_spawn"),
        "the sender API is missing: {sender}"
    );

    let receiver = expand_receiver(&system_path);
    assert!(
        !receiver.contains("compile_error"),
        "receiver code generation failed: {receiver}"
    );
    assert!(
        receiver.contains("__xbin_doorbell_1_0"),
        "the doorbell ISR is missing: {receiver}"
    );
    assert!(
        receiver.contains("__RticxXbinDispatcher0To1P3"),
        "the dispatcher is missing: {receiver}"
    );

    let project = dir.path().join("project");
    write_project(&project, &sender, &receiver);

    let output = Command::new(cargo())
        .arg("run")
        .arg("--quiet")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", project.join("target"))
        .output()
        .expect("failed to run cargo");
    assert!(
        output.status.success(),
        "the cross-binary end-to-end harness failed:\n\
         stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("xbin: e2e ok"),
        "the harness did not reach the end of the scenario"
    );
}
