//! End-to-end CLI tests for `cargo-xbin` (spawns the real binary).
//!
//! Both invocation spellings are exercised: `cargo-xbin xbin …` as Cargo
//! invokes external subcommands, and a direct `cargo-xbin …` call.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::tempdir;

const BIN: &str = env!("CARGO_BIN_EXE_cargo-xbin");

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run cargo-xbin")
}

#[test]
fn help_lists_the_subcommands() {
    let dir = tempdir().expect("tempdir");
    for args in [&["--help"][..], &["xbin", "--help"][..]] {
        let output = run(dir.path(), args);
        assert!(output.status.success(), "exit: {:?}", output.status.code());
        let stdout = String::from_utf8(output.stdout).expect("utf-8");
        assert!(stdout.contains("cargo xbin"), "stdout: {stdout}");
        assert!(stdout.contains("sync"), "stdout: {stdout}");
        assert!(stdout.contains("build"), "stdout: {stdout}");
    }
}

#[test]
fn sync_creates_the_output_layout() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "sync"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("target/rticx-xbin").is_dir());
}

#[test]
fn sync_is_discovered_from_a_nested_directory() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");
    let nested = dir.path().join("crates/app");
    std::fs::create_dir_all(&nested).expect("mkdirs");

    let output = run(&nested, &["xbin", "sync"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("target/rticx-xbin").is_dir());
}

#[test]
fn build_is_a_noop_without_a_project_and_succeeds() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "build"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("target/rticx-xbin").is_dir());
}

#[test]
fn invalid_manifest_fails_with_the_parser_message() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 2\n").expect("write manifest");

    let output = run(dir.path(), &["xbin", "sync"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(
        stderr.contains("unsupported rticx.toml schema version 2"),
        "stderr: {stderr}"
    );
}

#[test]
fn unknown_subcommand_fails_with_usage() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "frobnicate"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(stderr.contains("frobnicate"), "stderr: {stderr}");
}

#[test]
fn no_subcommand_prints_help_and_fails() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &[]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn version_is_reported() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "--version"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "stdout: {stdout}"
    );
}
