//! Behaviour tests for the `cargo-xbin` driver (M0-T6, M1-T4).
//!
//! These exercise the library entry points directly with explicit project
//! roots. Metadata collection against real packages is covered end to end by
//! `tests/fixture.rs`; CLI parsing and process exit codes by `tests/cli.rs`.

use std::path::{Path, PathBuf};

use rticx_xbin_driver::{SYSTEM_FILE, build, find_project_root, output_dir, sync};
use rticx_xbin_proto::{
    FileChange, GENERATED_CRATE_NAME, Hash64, IpcTypes, RTICX_GENERATION, SystemView, layout_hash,
};
use tempfile::tempdir;

/// A valid project without applications: `sync` reads it but invokes no
/// Cargo, because there is no metadata to collect.
const EMPTY_PROJECT: &str = "schema = 1\n";

/// IDL with a single 12-byte message, used by the generation tests below.
const ENCRYPT_IDL: &str = "schema = 1\n\n\
     [message.EncryptReq]\nfields = { addr = \"u32\", len = \"u32\", key = \"u32\" }\n";

fn write_project(dir: &Path) {
    std::fs::write(dir.join("rticx.toml"), EMPTY_PROJECT).expect("write manifest");
}

/// Writes a project without applications plus an IDL, so `sync` exercises
/// generation without invoking Cargo.
fn write_typed_project(dir: &Path) {
    write_project(dir);
    std::fs::write(dir.join("ipc-types.toml"), ENCRYPT_IDL).expect("write idl");
}

#[test]
fn output_dir_is_target_rticx_xbin() {
    assert_eq!(
        output_dir(Path::new("/proj")),
        PathBuf::from("/proj/target/rticx-xbin")
    );
}

#[test]
fn sync_without_manifest_is_a_noop_that_creates_the_layout() {
    let dir = tempdir().expect("tempdir");
    // Even with an IDL present, generation is driven by `rticx.toml`.
    std::fs::write(dir.path().join("ipc-types.toml"), ENCRYPT_IDL).expect("write idl");
    let outcome = sync(dir.path()).expect("sync succeeds");

    assert!(dir.path().join("target/rticx-xbin").is_dir());
    assert!(!dir.path().join(GENERATED_CRATE_NAME).exists());
    assert_eq!(outcome.project_root, dir.path());
    assert_eq!(outcome.output_dir, dir.path().join("target/rticx-xbin"));
    assert_eq!(outcome.manifest_path, None);
    assert_eq!(outcome.config, None);
    assert!(outcome.idl.is_none());
    assert!(outcome.merged.is_none());
    assert!(outcome.system.is_none());
    assert!(outcome.ipc_types.is_none());
    assert!(outcome.applications().is_empty());
    assert_eq!(outcome.region_count(), 0);
    assert!(outcome.manifests.is_empty());
    assert!(
        !dir.path()
            .join("target/rticx-xbin")
            .join(SYSTEM_FILE)
            .exists(),
        "without a project manifest there is no system view to emit"
    );
}

#[test]
fn sync_reads_the_project_manifest() {
    let dir = tempdir().expect("tempdir");
    write_project(dir.path());
    let manifest = dir.path().join("rticx.toml");

    let outcome = sync(dir.path()).expect("sync succeeds");

    assert_eq!(outcome.manifest_path.as_deref(), Some(manifest.as_path()));
    assert!(outcome.config.is_some());
    assert!(outcome.idl.is_some(), "an absent IDL becomes an empty one");
    assert!(
        outcome.ipc_types.is_none(),
        "without an IDL there is no crate to generate"
    );
    assert!(!dir.path().join(GENERATED_CRATE_NAME).exists());
    let merged = outcome.merged.as_ref().expect("an empty project merges");
    assert!(merged.tasks().is_empty());
    assert!(outcome.applications().is_empty());
    assert_eq!(outcome.region_count(), 0);
    assert!(outcome.manifests.is_empty());
    assert!(dir.path().join("target/rticx-xbin").is_dir());

    // Even an empty project gets its sealed system view (M1-T6).
    let system = outcome.system.as_ref().expect("an empty project emits one");
    let path = outcome.output_dir.join(SYSTEM_FILE);
    let bytes = std::fs::read_to_string(&path).expect("system.json is written");
    assert_eq!(bytes, system.to_json());
    assert!(system.verify_topology_hash());
    assert!(system.tasks.is_empty());
    assert!(system.doorbells.is_empty());
    assert_eq!(system.rticx_generation, RTICX_GENERATION);
    assert_eq!(
        system.layout_hash,
        Hash64::new(layout_hash(&IpcTypes::empty()).expect("layout hash"))
    );
    assert_eq!(
        SystemView::from_json(&bytes).expect("system.json parses"),
        *system
    );
}

#[test]
fn sync_reports_an_invalid_manifest() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 2\n").expect("write manifest");

    let error = sync(dir.path())
        .expect_err("schema is rejected")
        .to_string();
    assert_eq!(
        error,
        "unsupported rticx.toml schema version 2; this tool supports schema = 1"
    );
}

#[test]
fn build_runs_sync_first() {
    let dir = tempdir().expect("tempdir");
    write_project(dir.path());

    let outcome = build(dir.path()).expect("build succeeds");

    assert!(outcome.sync.config.is_some());
    assert!(outcome.sync.system.is_some());
    assert!(dir.path().join("target/rticx-xbin").is_dir());
    assert!(outcome.sync.output_dir.join(SYSTEM_FILE).is_file());
}

#[test]
fn build_without_manifest_is_a_noop() {
    let dir = tempdir().expect("tempdir");
    let outcome = build(dir.path()).expect("build succeeds");
    assert!(outcome.sync.config.is_none());
    assert!(dir.path().join("target/rticx-xbin").is_dir());
}

#[test]
fn find_project_root_walks_up_to_the_manifest() {
    let dir = tempdir().expect("tempdir");
    let nested = dir.path().join("crates/app/src");
    std::fs::create_dir_all(&nested).expect("mkdirs");

    assert_eq!(find_project_root(&nested), None);
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");
    assert_eq!(find_project_root(&nested).as_deref(), Some(dir.path()));
}

// -- M1-T7: `ipc-types` generation, change detection and reporting ---------

#[test]
fn sync_generates_the_ipc_types_crate_and_then_no_ops() {
    let dir = tempdir().expect("tempdir");
    write_typed_project(dir.path());

    let first = sync(dir.path()).expect("sync succeeds");
    let ipc_types = first.ipc_types.as_ref().expect("the IDL generates a crate");
    assert_eq!(ipc_types.path, dir.path().join(GENERATED_CRATE_NAME));
    assert!(ipc_types.status.changed());
    assert_eq!(
        ipc_types
            .status
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.change))
            .collect::<Vec<_>>(),
        [
            ("Cargo.toml", FileChange::Created),
            ("src/lib.rs", FileChange::Created),
        ]
    );

    let lib_rs =
        std::fs::read_to_string(ipc_types.path.join("src/lib.rs")).expect("generated lib.rs");
    assert!(lib_rs.contains("pub struct EncryptReq {"), "{lib_rs}");
    assert!(
        lib_rs.contains("pub const SIZE_ENCRYPT_REQ: usize = 12;"),
        "{lib_rs}"
    );
    assert!(
        lib_rs.contains("pub const LAYOUT_HASH: u64 = 0x"),
        "{lib_rs}"
    );

    let cargo_toml =
        std::fs::read_to_string(ipc_types.path.join("Cargo.toml")).expect("generated Cargo.toml");
    assert!(
        cargo_toml.contains("rticx-xbin-rt = { path = "),
        "{cargo_toml}"
    );
    assert!(
        !cargo_toml.contains("path = \"/"),
        "the dependency path must stay relative so the crate is portable:\n{cargo_toml}"
    );

    // M1-T7 acceptance: an unchanged IDL is a no-op.
    let second = sync(dir.path()).expect("sync succeeds");
    let ipc_types = second
        .ipc_types
        .as_ref()
        .expect("the IDL generates a crate");
    assert!(ipc_types.status.is_noop(), "{:?}", ipc_types.status);
    assert!(
        ipc_types
            .status
            .files
            .iter()
            .all(|file| file.change == FileChange::Unchanged)
    );
    assert_eq!(
        std::fs::read_to_string(ipc_types.path.join("src/lib.rs")).expect("lib.rs"),
        lib_rs,
        "a no-op sync must not rewrite the generated files"
    );
}

#[test]
fn sync_updates_the_ipc_types_crate_when_the_idl_changes() {
    let dir = tempdir().expect("tempdir");
    write_typed_project(dir.path());

    let first = sync(dir.path()).expect("sync succeeds");
    let before = std::fs::read_to_string(
        first
            .ipc_types
            .as_ref()
            .expect("generated")
            .path
            .join("src/lib.rs"),
    )
    .expect("lib.rs");

    std::fs::write(
        dir.path().join("ipc-types.toml"),
        format!("{ENCRYPT_IDL}\n[message.SensorBatch]\nfields = {{ tag = \"u8\" }}\n"),
    )
    .expect("edit idl");

    let second = sync(dir.path()).expect("sync succeeds");
    let ipc_types = second.ipc_types.as_ref().expect("generated");
    assert!(ipc_types.status.changed());
    assert_eq!(
        ipc_types
            .status
            .changed_files()
            .map(|file| (file.path.as_str(), file.change))
            .collect::<Vec<_>>(),
        [("src/lib.rs", FileChange::Updated)]
    );

    let after = std::fs::read_to_string(ipc_types.path.join("src/lib.rs")).expect("lib.rs");
    assert_ne!(after, before, "the generated crate follows the IDL");
    assert!(after.contains("pub struct SensorBatch {"), "{after}");
}
