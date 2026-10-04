//! M3-T3/M6-T1 acceptance: the generated init hooks configure the shared
//! memory view and zero the FIFOs produced by the core.
//!
//! The sender fixture expands in codegen mode (no core pass: its expansion is
//! a plain module), is written into a throwaway cargo binary together with a
//! stub `ipc_types.rs` next to its `system.json` and the in-tree mock, and is
//! **run** on the host: the generated, argument-less
//! `__rticx_xbin_init_fifos_core0` must zero exactly the FIFOs produced by
//! core 0 (M6-T1), leaving its peer's FIFOs untouched. The shared-memory
//! configuration is inlined at `BeforeInit` (D1) and is a no-op on the host,
//! so no configure wrapper function is generated. Generated code arms no
//! doorbell (M6.5: the router IRQs are enabled and prioritized by the core
//! pass's used-IRQ machinery). Boot sequencing between cores is
//! distribution-owned and is not part of these hooks.
//!
//! The hooks are private to the `#[app]` module, so the fixture declares
//! `pub` wrappers next to them for the harness to call.

use std::path::{Path, PathBuf};
use std::process::Command;

use quote::{ToTokens, format_ident, quote};
use rticx_core::RticPass;
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use rticx_xbin_proto::SystemView;

/// A two-application system view: `app-m7` (global core 0) spawns
/// `EncryptTask` on `app-m4` (global core 1) at priority 3, and `app-m4`
/// spawns `DecryptTask` back on `app-m7` at priority 4. The reverse direction
/// adds a second FIFO produced by the other core, so the executed test proves
/// that a core initializes exactly the FIFOs it produces (M6-T1).
const SYSTEM_JSON: &str = r#"{
  "schema_version": 2,
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
      "name": "DecryptTask",
      "receiver_core": 0,
      "spawner_core": 1,
      "priority": 4,
      "capacity": 1,
      "input_type": "EncryptReq",
      "fifo": { "source": 1, "target": 0, "pool": "p01", "offset": 0, "elem_size": 12, "depth": 2 }
    },
    {
      "id": 2,
      "name": "EncryptTask",
      "receiver_core": 1,
      "spawner_core": 0,
      "priority": 3,
      "capacity": 2,
      "input_type": "EncryptReq",
      "fifo": { "source": 0, "target": 1, "pool": "p01", "offset": 88, "elem_size": 12, "depth": 3 }
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
      "used": 188
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
    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt)
    }

    fn current_global_core_id(&self) -> syn::Expr {
        syn::parse_quote!(__rticx_xbin_backend().global_core_id())
    }

    fn ipc_base_override(&self, _view_core: u32, source: u32, target: u32) -> Option<syn::Expr> {
        // The host mock pools are heap-backed, not at the synced addresses.
        Some(syn::parse_quote!(
            __rticx_xbin_backend().pool_base(#source, #target)
        ))
    }

    fn ring_doorbell_fn(&self, source: u32, target: u32, mut template: syn::ItemFn) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().doorbell_send(#source, #target, task_id)
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
            __rticx_xbin_backend().take_message(#source, #target)
        });
        template
    }
}

/// The owner fixture application: it declares nothing (the pass generates the
/// `EncryptTask` stub from the view) plus the mock backend its generated code
/// calls and `pub` wrappers over the generated private hooks.
fn owner_app() -> syn::ItemMod {
    syn::parse_quote! {
        pub mod app {
            use rticx_xbin_mock::{MockBackend, MockSystem};

            fn __rticx_xbin_system() -> &'static MockSystem {
                static SYSTEM: std::sync::LazyLock<MockSystem> =
                    std::sync::LazyLock::new(|| {
                        let mut system = MockSystem::new();
                        system
                            .add_pool(0, 1, 4096)
                            .expect("the mock IPC pool fits");
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

            pub fn test_system() -> &'static MockSystem {
                __rticx_xbin_system()
            }

            pub fn test_init_fifos() {
                __rticx_xbin_init_fifos_core0();
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
        .join("../mock/rticx-xbin-mock")
        .canonicalize()
        .expect("the mock crate ships with the workspace");

    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"xbin-init-hooks\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             publish = false\n\n\
             [dependencies]\n\
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
        "// @generated by the M3-T3/M6-T1 init-hooks test\n\
         #![allow(dead_code, unused_imports, unused_variables, non_snake_case, \
         non_upper_case_globals, static_mut_refs)]\n\n\
         include!(\"generated.rs\");\n\n\
         use rticx_xbin_rt::Fifo;\n\n\
         fn msg(addr: u32) -> app::ipc_types::EncryptReq {\n\
             app::ipc_types::EncryptReq { addr, len: 2, key: 3 }\n\
         }\n\n\
         fn main() {\n\
              let system = app::test_system();\n\n\
              // Dirty both task FIFOs: they live in the same shared `p01`\n\
              // pool but are produced by different cores, so\n\
              // `init_fifos_core0` must zero exactly the `0 -> 1` FIFO and\n\
              // leave the `1 -> 0` one untouched (M6-T1).\n\
             let out_base = system.pool_base(0, 1).expect(\"pool {0, 1}\");\n\
             // `EncryptTask` is the second task of the shared `p01` pool.\n\
             let out_fifo = unsafe {\n\
                 Fifo::<app::ipc_types::EncryptReq, 3usize>::view_at(out_base + 88)\n\
             };\n\
             let in_base = system.pool_base(1, 0).expect(\"pool {1, 0}\");\n\
             // `DecryptTask` is the first task of the shared `p01` pool.\n\
             let in_fifo = unsafe {\n\
                 Fifo::<app::ipc_types::EncryptReq, 2usize>::view_at(in_base)\n\
             };\n\
             assert!(unsafe { (*out_fifo).enqueue(msg(1)) }.is_ok());\n\
             assert!(unsafe { (*in_fifo).enqueue(msg(2)) }.is_ok());\n\
             assert_eq!(unsafe { (*out_fifo).len() }, 1, \"one dirty element\");\n\
             assert_eq!(unsafe { (*in_fifo).len() }, 1, \"one dirty element\");\n\n\
              // Each FIFO is zeroed by its producer core: core 0 produces the\n\
              // 0->1 FIFO and leaves the 1->0 FIFO to core 1 (M6-T1).\n\
              app::test_init_fifos();\n\
              assert_eq!(unsafe { (*out_fifo).len() }, 0, \"the 0->1 FIFO is zeroed\");\n\
              assert_eq!(\n\
                  unsafe { (*in_fifo).len() },\n\
                  1,\n\
                  \"a core initializes only the FIFOs it produces\"\n\
              );\n\n\
              println!(\"xbin: fifos initialized\");\n\
         }\n",
    )
    .expect("harness source");
}

/// The generated `ipc_types.rs` stand-in: the fixture's `EncryptReq` type, as
/// `cargo xbin sync` emits it next to `system.json` (M1-T7). The pass re-emits
/// it into the `#[app]` module.
const IPC_TYPES_RS: &str = "\
// @generated by `cargo xbin sync`\n\
#[repr(C)]\n\
#[derive(Clone, Copy)]\n\
pub struct EncryptReq {\n\
    pub addr: u32,\n\
    pub len: u32,\n\
    pub key: u32,\n\
}\n\
const _: () = {\n\
    assert!(core::mem::size_of::<EncryptReq>() == 12);\n\
    assert!(core::mem::align_of::<EncryptReq>() == 4);\n\
};\n\
unsafe impl CrossCoreMessage for EncryptReq {}\n";

#[test]
fn mock_app_initializes_only_its_produced_fifos() {
    let dir = tempfile::tempdir().expect("tempdir");
    let system = dir.path().join("system.json");
    std::fs::write(&system, owner_system()).expect("system.json");
    std::fs::write(dir.path().join("ipc_types.rs"), IPC_TYPES_RS).expect("ipc_types.rs");

    let expanded = expand(&system);
    assert!(
        !expanded.contains("__rticx_xbin_configure_shared_memory"),
        "the configure wrapper function must be gone (D1): {expanded}"
    );
    assert!(
        expanded.contains("__rticx_xbin_init_fifos_core0"),
        "the producer FIFO initializer is missing: {expanded}"
    );
    assert!(
        !expanded.contains("doorbell_setup"),
        "generated code must not arm doorbells (M6.5-T4): {expanded}"
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
        "the generated init hooks failed to initialize the mock app's produced FIFOs:\n\
         stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("xbin: fifos initialized"),
        "the harness did not run the FIFO initializer"
    );
}
