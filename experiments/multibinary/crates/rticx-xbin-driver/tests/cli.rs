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

/// Runs `cargo-xbin` with extra environment variables (for the browser opener).
fn run_with_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(BIN);
    command.args(args).current_dir(dir);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("run cargo-xbin")
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
        assert!(stdout.contains("verify"), "stdout: {stdout}");
    }

    let output = run(dir.path(), &["xbin", "sync", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("--html"), "stdout: {stdout}");
    assert!(stdout.contains("--html-open"), "stdout: {stdout}");

    let output = run(dir.path(), &["xbin", "build", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("--verify-elf"), "stdout: {stdout}");

    let output = run(dir.path(), &["xbin", "verify", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("--release"), "stdout: {stdout}");
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
fn build_verify_elf_is_a_noop_without_a_project_and_succeeds() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "build", "--verify-elf"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join("target/rticx-xbin").is_dir());
}

#[test]
fn verify_without_a_project_manifest_errors() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "verify"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(
        stderr.contains("no `rticx.toml` project manifest"),
        "{stderr}"
    );
}

#[test]
fn verify_without_a_synced_view_errors_clearly() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");

    let output = run(dir.path(), &["xbin", "verify"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(stderr.contains("run `cargo xbin sync` first"), "{stderr}");
}

#[test]
fn verify_rejects_a_stale_view_without_building() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");
    std::fs::create_dir_all(dir.path().join("target/rticx-xbin")).expect("output dir");
    let view = rticx_xbin_proto::SystemView::empty("0.2");
    std::fs::write(
        dir.path().join("target/rticx-xbin/system.json"),
        view.to_json(),
    )
    .expect("write view");

    let output = run(dir.path(), &["xbin", "verify"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(stderr.contains("is stale"), "{stderr}");
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");
}

#[test]
fn sync_reports_generated_ipc_types_changes() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");
    std::fs::write(
        dir.path().join("ipc-types.toml"),
        "schema = 1\n\n[message.EncryptReq]\nfields = { addr = \"u32\" }\n",
    )
    .expect("write idl");

    let first = run(dir.path(), &["xbin", "sync"]);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stderr = String::from_utf8(first.stderr).expect("utf-8");
    assert!(stderr.contains("created ipc-types/Cargo.toml"), "{stderr}");
    assert!(stderr.contains("created ipc-types/src/lib.rs"), "{stderr}");

    let second = run(dir.path(), &["xbin", "sync"]);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stderr = String::from_utf8(second.stderr).expect("utf-8");
    assert!(stderr.contains("ipc-types is up to date"), "{stderr}");
    assert!(!stderr.contains("created"), "{stderr}");
}

#[test]
fn sync_html_writes_the_visualization() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");

    let output = run(dir.path(), &["xbin", "sync", "--html"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let path = dir.path().join("target/rticx-xbin/system.html");
    let html = std::fs::read_to_string(&path).expect("system.html is written");
    assert!(html.contains("RTICX multi-binary system view"), "{html}");
    assert!(html.contains("id=\"xbin-view\""), "{html}");
    assert!(
        html.contains("function drawArrows"),
        "the script is inlined"
    );

    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(
        stderr.contains("wrote target/rticx-xbin/system.html"),
        "{stderr}"
    );
}

/// Root of the checked-in end-to-end fixture.
fn e2e_fixture_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/fixtures/e2e")
        .canonicalize()
        .expect("the end-to-end fixture ships with the driver crate")
}

#[test]
fn sync_html_over_the_fixture_renders_pool_panels() {
    let root = e2e_fixture_root();
    let output = run(&root, &["xbin", "sync", "--html"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let html = std::fs::read_to_string(root.join("target/rticx-xbin/system.html"))
        .expect("system.html is written");
    assert!(
        html.contains("\"id\":\"mock-0-1\""),
        "the fixture's distro pool is a panel (M6.9-T8/T9)"
    );
    assert!(
        html.contains("\"budget\":4096"),
        "the panel carries the shared pool budget"
    );
}

#[test]
fn sync_html_open_implies_html_and_uses_the_configured_opener() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(dir.path().join("rticx.toml"), "schema = 1\n").expect("write manifest");

    // `true` ignores its argument and exits 0, so no browser is launched.
    let output = run_with_env(
        dir.path(),
        &["xbin", "sync", "--html-open"],
        &[("RTICX_XBIN_OPENER", "true")],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        dir.path().join("target/rticx-xbin/system.html").is_file(),
        "--html-open also writes the file"
    );
}

#[test]
fn sync_html_without_a_project_errors_clearly() {
    let dir = tempdir().expect("tempdir");
    let output = run(dir.path(), &["xbin", "sync", "--html"]);
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(stderr.contains("cannot render `--html`"), "{stderr}");
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
