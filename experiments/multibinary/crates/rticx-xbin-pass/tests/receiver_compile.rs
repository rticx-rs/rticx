//! M3-T2 acceptance: a minimal receiver fixture expands through the core pass
//! and the expansion compiles.
//!
//! The pass runs in codegen mode over a fixture `system.json` inside the full
//! `RticMacroBuilder` pipeline (with `MockCoreBackend`), so the receiver
//! structs are rewritten into core `#[task(..)]` items, the doorbell
//! dispatcher is emitted as a hardware task, and the core pass consumes all of
//! it. The final expansion is then written into a throwaway cargo project
//! (with a stub PAC, the generated `ipc-types` and the in-tree mock backend)
//! and built on the host, which type-checks the generated FIFO views,
//! dispatcher and task-trait calls.

use std::path::{Path, PathBuf};
use std::process::Command;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use rticx_core::RticMacroBuilder;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use rticx_xbin_proto::SystemView;

/// A two-application system view: `app-m7` (global core 0) spawns
/// `EncryptTask` on `app-m4` (global core 1), priority 3, capacity 2.
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

/// Code-generation backend of the compile fixture.
struct CompileBackend;

impl XbinPassBackend for CompileBackend {
    fn backend(&self) -> syn::Expr {
        syn::parse_quote!(__rticx_xbin_backend())
    }

    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt)
    }

    fn dispatcher_irq(&self, target: u32, line: u32) -> syn::Ident {
        format_ident!("__rticx_xbin_doorbell_{target}_{line}")
    }

    fn doorbell_irq(&self, target: u32, line: u32) -> u16 {
        (target * 10 + line) as u16
    }
}

/// The receiver application: one task, its input, and the mock backend used
/// by the generated FIFO view. `__rticx_xbin_backend` lives inside the app
/// module because the generated code calls it there.
fn receiver_app() -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            fn __rticx_xbin_backend() -> rticx_xbin_mock::MockBackend {
                static SYSTEM: std::sync::LazyLock<rticx_xbin_mock::MockSystem> =
                    std::sync::LazyLock::new(|| {
                        let mut system = rticx_xbin_mock::MockSystem::new();
                        system
                            .add_region(0, 1, 4096)
                            .expect("mock IPC region fits");
                        system
                    });
                SYSTEM.backend(1)
            }

            trait CrossBinTask {
                type Input;
                fn exec(&mut self, input: Self::Input);
            }

            #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
            struct EncryptTask;

            impl CrossBinTask for EncryptTask {
                type Input = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::Input) {}
            }

            #[init]
            fn init() -> TaskInits {
                TaskInits {}
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

/// The fixture view with the receiver application's source hash recorded, as
/// `cargo xbin sync` would (M3-T4).
fn receiver_system() -> String {
    let mut view = SystemView::from_json(SYSTEM_JSON).expect("fixture system view");
    let application = view
        .apps
        .iter_mut()
        .find(|application| application.package == "app-m4")
        .expect("fixture app-m4");
    application.source_hash = rticx_xbin_pass::app_source_hash(&receiver_args(), &receiver_app());
    view.seal();
    view.to_json()
}

/// Runs the extension pass and the core pass and returns the expansion.
fn expand(system: &Path) -> String {
    let pass = XbinPass::with_system(system, "app-m4", "m4").with_backend(CompileBackend);
    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(pass);
    builder
        .build_rtic_macro2(receiver_args(), receiver_app(), None)
        .to_string()
}

fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Writes the throwaway project that compiles the expansion.
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
            "[package]\nname = \"xbin-receiver-compile\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
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
    std::fs::write(
        root.join("src/lib.rs"),
        format!(
            "// @generated by the M3-T2 receiver compile test\n\
             #![allow(static_mut_refs, unused_imports, unused_variables, dead_code, \
             non_camel_case_types, non_snake_case)]\n\n{expanded}\n"
        ),
    )
    .expect("expansion");

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
fn receiver_fixture_expands_and_compiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let system = dir.path().join("system.json");
    std::fs::write(&system, receiver_system()).expect("system.json");

    let expanded = expand(&system);

    // The pipeline ran to completion and emitted the M3-T2 items.
    assert!(
        !expanded.contains("compile_error"),
        "pipeline failed: {expanded}"
    );
    assert!(
        expanded.contains("__rticx_xbin_doorbell_1_0"),
        "the doorbell handler is missing: {expanded}"
    );
    assert!(
        expanded.contains("__RticxXbinDispatcher0To1P3"),
        "the dispatcher is missing: {expanded}"
    );
    assert!(
        expanded.contains("implements_cross_bin_task")
            && expanded.contains("impl CrossBinTask for EncryptTask"),
        "the receiver was not turned into a core task: {expanded}"
    );

    // The init hooks are wired into the generated entry function: the
    // receiver is not the owner, so it only arms its doorbell and marks
    // itself ready (M3-T3).
    assert!(
        expanded.contains("fn __rticx_xbin_mark_ready_core0"),
        "the mark-ready hook is missing: {expanded}"
    );
    assert!(
        !expanded.contains("__rticx_xbin_init_shared"),
        "a non-owner application must not initialize the shared state: {expanded}"
    );
    assert!(
        expanded.contains("__rticx_xbin_mark_ready_core0 (& __rticx_xbin_backend ()) ;"),
        "the mark-ready hook is not injected into the entry function: {expanded}"
    );
    assert!(
        expanded.contains("__rticx_xbin_configure_shared_memory (& __rticx_xbin_backend ()) ;"),
        "the configure hook is not injected into the entry function: {expanded}"
    );

    let project = dir.path().join("project");
    write_project(&project, &expanded);

    let status = Command::new(cargo())
        .arg("build")
        .arg("--quiet")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", project.join("target"))
        .status()
        .expect("failed to run cargo");
    assert!(
        status.success(),
        "the expanded receiver fixture failed to compile"
    );
    assert!(
        project
            .join("target/debug/libxbin_receiver_compile.rlib")
            .is_file(),
        "the throwaway crate produced no library"
    );

    // M3-T4: the expansion `include_str!`s the system view, so editing it must
    // make rustc rebuild the crate through its dep-info. Sleep past the
    // mtime resolution some filesystems use before touching the view.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut edited = std::fs::read_to_string(&system).expect("system.json");
    edited.push('\n');
    std::fs::write(&system, edited).expect("edit system.json");
    let rebuild = Command::new(cargo())
        .arg("build")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", project.join("target"))
        .output()
        .expect("failed to run cargo");
    assert!(
        rebuild.status.success(),
        "the rebuild after editing system.json failed:\n{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rebuild.stderr).contains("Compiling xbin-receiver-compile"),
        "editing system.json must trigger a rebuild (include_str! dep-info):\n{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
}
