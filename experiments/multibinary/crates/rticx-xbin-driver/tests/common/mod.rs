//! Helpers shared by the driver's build-phase integration tests.
//!
//! `fixture_copy` turns the checked-in `crates/mock/fixtures/e2e` project into a
//! self-contained temporary copy and `cargo_build` runs a plain `cargo build`
//! over it with the driver's mode variables removed. `negative.rs` and
//! `freshness.rs` both need them.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::tempdir;

/// The `cargo` executable Cargo itself invoked the tests with.
pub fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Copies the checked-in end-to-end fixture into a fresh temporary directory.
///
/// `target/` is skipped, and dependency paths that leave the fixture root (the
/// in-tree workspace crates) are rewritten to absolute paths so the copy is a
/// self-contained project. Paths inside the fixture stay relative, so the copy
/// uses its own application sources.
pub fn fixture_copy() -> tempfile::TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/fixtures/e2e")
        .canonicalize()
        .expect("the end-to-end fixture ships with the driver crate");
    let dir = tempdir().expect("tempdir");
    let root = dir.path().join("e2e");
    copy_tree(&source, &root);
    for manifest in manifests(&root) {
        let relative = manifest
            .strip_prefix(&root)
            .expect("manifest below the copy");
        rewrite_dependency_paths(&manifest, &source.join(relative));
    }
    dir
}

/// Recursively copies `from` to `to`, skipping Cargo's `target/` directory.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create directory");
    for entry in std::fs::read_dir(from).expect("read directory") {
        let entry = entry.expect("directory entry");
        if entry.file_name() == "target" {
            continue;
        }
        let destination = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), &destination).expect("copy file");
        }
    }
}

/// Returns every `Cargo.toml` below `root`.
fn manifests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read directory") {
            let entry = entry.expect("directory entry");
            let path = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
                found.push(path);
            }
        }
    }
    found
}

/// Rewrites the `path = "…"` values of the copied manifest at `copy_file`:
/// paths that resolve inside the copy stay relative, paths that do not (the
/// in-tree workspace crates) become the absolute path they point at in the
/// original fixture.
fn rewrite_dependency_paths(copy_file: &Path, source_file: &Path) {
    let text = std::fs::read_to_string(copy_file).expect("copied Cargo.toml");
    let copy_dir = copy_file.parent().expect("manifest directory");
    let source_dir = source_file.parent().expect("source manifest directory");

    let mut rewritten = String::with_capacity(text.len());
    let mut rest = text.as_str();
    let marker = "path = \"";
    while let Some(index) = rest.find(marker) {
        let (before, after) = rest.split_at(index + marker.len());
        rewritten.push_str(before);
        let Some(end) = after.find('"') else {
            rewritten.push_str(after);
            rest = "";
            break;
        };
        let (value, tail) = after.split_at(end);
        if copy_dir.join(value).exists() {
            rewritten.push_str(value);
        } else {
            let original = source_dir
                .join(value)
                .canonicalize()
                .unwrap_or_else(|_| panic!("the fixture dependency `{value}` exists"));
            rewritten.push_str(&original.to_string_lossy().replace('\\', "/"));
        }
        rest = tail;
    }
    rewritten.push_str(rest);
    std::fs::write(copy_file, rewritten).expect("rewrite Cargo.toml");
}

/// Runs a plain `cargo build` for one application, with the driver's mode
/// variables removed (they must not leak in from the test environment).
pub fn cargo_build(root: &Path, package: &str, bin: &str) -> Output {
    Command::new(cargo())
        .args(["build", "--package", package, "--bin", bin])
        .current_dir(root)
        .env_remove("RTICX_XBIN_META_OUT")
        .env_remove("RTICX_XBIN_SYSTEM")
        .output()
        .expect("failed to run cargo build")
}
