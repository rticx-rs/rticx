//! M3-T3 acceptance: the generated init hooks reach the ready state on the
//! mock backend.
//!
//! The sender fixture expands in codegen mode (no core pass: its expansion is
//! a plain module), is written into a throwaway cargo binary together with a
//! stub `ipc-types` crate and the in-tree mock, and is **run** on the host:
//! the generated `__rticx_xbin_init_shared` must zero the FIFO indices and
//! publish the shared state, and the generated
//! `__rticx_xbin_mark_ready_core0` must arm the receiver's doorbell and mark
//! the owner core ready.
//!
//! The hooks are private to the `#[app]` module, so the fixture declares
//! `pub` wrappers next to them for the harness to call.

use std::path::{Path, PathBuf};
use std::process::Command;

use quote::{ToTokens, format_ident, quote};
use rticx_core::RticPass;
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use rticx_xbin_proto::SystemView;

/// A two-application system view: `app-m7` (global core 0, the owner) spawns
/// `EncryptTask` on `app-m4` (global core 1) at priority 3, and `app-m4`
/// spawns `DecryptTask` back on `app-m7` at priority 4. The reverse direction
/// gives the owner a doorbell targeting itself, so the executed test covers
/// both the `init_shared` FIFO zeroing and the `mark_ready` doorbell arming.
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
      "name": "DecryptTask",
      "receiver_core": 0,
      "spawner_cores": [1],
      "priority": 4,
      "capacity": 1,
      "input_type": "EncryptReq",
      "fifo": { "source": 1, "target": 0, "offset": 0, "elem_size": 12, "depth": 2 }
    },
    {
      "id": 2,
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
    },
    {
      "source": 1,
      "target": 0,
      "base_from_source": "0x30041000",
      "base_from_target": "0x30041000",
      "size": 4096
    }
  ],
  "doorbells": [
    { "source": 0, "target": 1, "priority": 3, "line": 0 },
    { "source": 1, "target": 0, "priority": 4, "line": 0 }
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
        format_ident!("XbinDoorbell{target}_{line}")
    }

    fn doorbell_irq(&self, target: u32, line: u32) -> u16 {
        100 + (target * 10 + line) as u16
    }
}

/// The owner fixture application: one sender stub plus the mock backend its
/// generated code calls, and `pub` wrappers over the generated private hooks.
fn owner_app() -> syn::ItemMod {
    syn::parse_quote! {
        pub mod app {
            use rticx_xbin_mock::{MockBackend, MockSystem};

            fn __rticx_xbin_system() -> &'static MockSystem {
                static SYSTEM: std::sync::LazyLock<MockSystem> =
                    std::sync::LazyLock::new(|| {
                        let mut system = MockSystem::new();
                        system
                            .add_region(0, 1, 4096)
                            .expect("the mock IPC region fits");
                        system
                            .add_region(1, 0, 4096)
                            .expect("the mock reverse region fits");
                        system
                    });
                &SYSTEM
            }

            fn __rticx_xbin_backend() -> MockBackend {
                __rticx_xbin_system().backend(0)
            }

            fn __rticx_interrupt_free<R>(f: impl FnOnce() -> R) -> R {
                f()
            }

            #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
            pub struct EncryptTask;

            pub fn test_system() -> &'static MockSystem {
                __rticx_xbin_system()
            }

            pub fn test_configure_shared_memory() {
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

fn owner_args() -> proc_macro2::TokenStream {
    quote!(
        device = mypac,
        cores = 1,
        core_ids = [0],
        external_cores = [1]
    )
}

/// The fixture view with the owner application's source hash recorded, as
/// `cargo xbin sync` would (M3-T4).
fn owner_system() -> String {
    let mut view = SystemView::from_json(SYSTEM_JSON).expect("fixture system view");
    let application = view
        .apps
        .iter_mut()
        .find(|application| application.package == "app-m7")
        .expect("fixture app-m7");
    application.source_hash = rticx_xbin_pass::app_source_hash(&owner_args(), &owner_app());
    view.seal();
    view.to_json()
}

/// Runs the pass over the owner fixture and returns the generated module.
fn expand(system: &Path) -> String {
    let pass = XbinPass::with_system(system, "app-m7", "m7").with_backend(TestBackend);
    let (_, module) = pass
        .run_pass(owner_args(), owner_app())
        .expect("owner code generation succeeds");
    module.to_token_stream().to_string()
}

fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Writes the throwaway binary that executes the generated hooks.
fn write_project(root: &Path, expanded: &str) {
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
            "[package]\nname = \"xbin-init-hooks\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             publish = false\n\n\
             [dependencies]\n\
             ipc-types = {{ path = \"ipc-types\" }}\n\
             rticx-xbin-rt = {{ path = \"{}\" }}\n\
             rticx-xbin-mock = {{ path = \"{}\" }}\n\n\
             [workspace]\n",
            rt.display(),
            mock.display()
        ),
    )
    .expect("project manifest");
    std::fs::write(root.join("src/generated.rs"), format!("{expanded}\n"))
        .expect("generated module");
    std::fs::write(
        root.join("src/main.rs"),
        "// @generated by the M3-T3 init-hooks test\n\
         #![allow(dead_code, unused_imports, unused_variables, non_snake_case, \
         non_upper_case_globals, static_mut_refs)]\n\n\
         include!(\"generated.rs\");\n\n\
         use rticx_xbin_rt::backend::CrossBinBackend;\n\
         use rticx_xbin_rt::Fifo;\n\n\
         fn msg(addr: u32) -> ipc_types::EncryptReq {\n\
             ipc_types::EncryptReq { addr, len: 2, key: 3 }\n\
         }\n\n\
         fn main() {\n\
             let system = app::test_system();\n\
             assert_eq!(system.state().epoch(), 0, \"fresh mock state\");\n\
             app::test_configure_shared_memory();\n\n\
             // Dirty both task FIFOs before initialization: `init_shared` must\n\
             // zero the ring indices of every task, through either endpoint\n\
             // view of their region.\n\
             let out_region = system.backend(0).ipc_region(0, 1).expect(\"region 0->1\");\n\
             let out_fifo = unsafe {\n\
                 Fifo::<ipc_types::EncryptReq, 3usize>::view_at(out_region.base_from_source())\n\
             };\n\
             let in_region = system.backend(0).ipc_region(1, 0).expect(\"region 1->0\");\n\
             let in_fifo = unsafe {\n\
                 Fifo::<ipc_types::EncryptReq, 2usize>::view_at(in_region.base_from_target())\n\
             };\n\
             assert!(unsafe { (*out_fifo).enqueue(msg(1)) }.is_ok());\n\
             assert!(unsafe { (*in_fifo).enqueue(msg(2)) }.is_ok());\n\
             assert_eq!(unsafe { (*out_fifo).len() }, 1, \"one dirty element\");\n\
             assert_eq!(unsafe { (*in_fifo).len() }, 1, \"one dirty element\");\n\n\
             // The owner's own doorbell is not armed before `mark_ready`.\n\
             let backend = system.backend(0);\n\
             assert!(backend.doorbell_ring(0, 0).is_err(), \"unarmed before mark_ready\");\n\n\
             app::test_init_shared();\n\
             assert_eq!(system.state().epoch(), 1, \"init_shared bumps the epoch\");\n\
             assert!(\n\
                 system.state().is_initialized(),\n\
                 \"init_shared publishes the magic\"\n\
             );\n\
             assert_eq!(unsafe { (*out_fifo).len() }, 0, \"the 0->1 FIFO is zeroed\");\n\
             assert_eq!(unsafe { (*in_fifo).len() }, 0, \"the 1->0 FIFO is zeroed\");\n\
             assert!(!system.state().is_ready(0), \"init clears the ready bits\");\n\
             assert!(\n\
                 backend.doorbell_ring(0, 0).is_err(),\n\
                 \"init_shared does not arm the doorbells\"\n\
             );\n\n\
             app::test_mark_ready();\n\
             assert!(system.state().is_ready(0), \"the owner core is ready\");\n\
             assert!(\n\
                 system.state().is_ready_at(0, system.state().epoch()),\n\
                 \"the ready bit belongs to the published epoch\"\n\
             );\n\n\
             // mark_ready arms the doorbell lines targeting this core before\n\
             // publishing its ready bit.\n\
             backend\n\
                 .doorbell_ring(0, 0)\n\
                 .expect(\"mark_ready armed this core's doorbell\");\n\
             assert!(backend.doorbell_take(0, 0));\n\n\
             // The 0 -> 1 doorbell belongs to the receiver core, which arms\n\
             // its own line: the owner does not set it up.\n\
             assert!(\n\
                 backend.doorbell_ring(1, 0).is_err(),\n\
                 \"the receiver arms its own doorbell\"\n\
             );\n\n\
             println!(\"xbin: owner ready\");\n\
         }\n",
    )
    .expect("harness source");

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
         #[derive(Clone, Copy)]\n\
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
fn mock_app_reaches_ready_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let system = dir.path().join("system.json");
    std::fs::write(&system, owner_system()).expect("system.json");

    let expanded = expand(&system);
    assert!(
        expanded.contains("__rticx_xbin_configure_shared_memory"),
        "the configure hook is missing: {expanded}"
    );
    assert!(
        expanded.contains("__rticx_xbin_init_shared"),
        "the owner init hook is missing: {expanded}"
    );
    assert!(
        expanded.contains("__rticx_xbin_mark_ready_core0"),
        "the owner mark-ready hook is missing: {expanded}"
    );
    assert!(
        expanded.contains("doorbell_setup (0u32 , 0u32 , 100u16)"),
        "the owner does not arm the doorbell targeting itself: {expanded}"
    );

    let project = dir.path().join("project");
    write_project(&project, &expanded);

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
        "the generated init hooks failed to bring the mock app to ready:\n\
         stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("xbin: owner ready"),
        "the harness did not reach the ready state"
    );
}
