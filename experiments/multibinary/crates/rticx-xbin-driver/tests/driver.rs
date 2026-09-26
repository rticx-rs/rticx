//! Behaviour tests for the `cargo-xbin` driver skeleton (M0-T6).
//!
//! These exercise the library entry points directly with explicit project
//! roots; CLI parsing and process exit codes are covered by `tests/cli.rs`.

use std::path::{Path, PathBuf};

use rticx_xbin_driver::{build, find_project_root, output_dir, sync};
use tempfile::tempdir;

const PROJECT: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4", triple = "thumbv7em-none-eabihf" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"#;

fn write_project(dir: &Path) {
    std::fs::write(dir.join("rticx.toml"), PROJECT).expect("write manifest");
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
    let outcome = sync(dir.path()).expect("sync succeeds");

    assert!(dir.path().join("target/rticx-xbin").is_dir());
    assert_eq!(outcome.project_root, dir.path());
    assert_eq!(outcome.output_dir, dir.path().join("target/rticx-xbin"));
    assert_eq!(outcome.manifest_path, None);
    assert_eq!(outcome.config, None);
    assert!(outcome.applications().is_empty());
    assert_eq!(outcome.region_count(), 0);
}

#[test]
fn sync_parses_the_project_manifest() {
    let dir = tempdir().expect("tempdir");
    write_project(dir.path());
    let manifest = dir.path().join("rticx.toml");

    let outcome = sync(dir.path()).expect("sync succeeds");

    assert_eq!(outcome.manifest_path.as_deref(), Some(manifest.as_path()));
    assert_eq!(outcome.applications().len(), 2);
    assert_eq!(outcome.region_count(), 1);
    assert!(dir.path().join("target/rticx-xbin").is_dir());
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

    assert_eq!(outcome.sync.applications().len(), 2);
    assert!(dir.path().join("target/rticx-xbin").is_dir());
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
