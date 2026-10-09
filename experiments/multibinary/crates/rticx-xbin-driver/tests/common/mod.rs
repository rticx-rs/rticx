//! Helpers shared by the driver's integration tests.
//!
//! `fixture_root` locates one of the checked-in fixtures below
//! `crates/mock/fixtures`; `target_dir`/`binary_contains` inspect the binaries a
//! fixture build produced. `fixture_copy` turns the `e2e` fixture into a
//! self-contained temporary copy and `cargo_build` runs a plain `cargo build`
//! over it with the driver's mode variables removed. Every test crate uses a
//! subset, hence the blanket `dead_code` allowance.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};

use tempfile::tempdir;

/// Serializes tests that build or read a shared, checked-in fixture.
///
/// `sync`/`build`/`verify` all run Cargo against the same standalone fixture
/// workspace: `sync` removes and rewrites `target/rticx-xbin/*` (including the
/// per-application `<target>.xbin.json` manifests) while a concurrent build
/// links into `target/`. Two of them at once race; the reproduction was a
/// `RemoveManifest { … NotFound }` from the check-then-remove in
/// `collect_manifest`. Tests that touch a shared fixture (rather than a
/// [`fixture_copy`] or their own tempdir) take this guard.
pub fn fixture_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The `cargo` executable Cargo itself invoked the tests with.
pub fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Root of one of the checked-in fixtures below `crates/mock/fixtures`.
///
/// `name` is the fixture directory, e.g. `"e2e"`, `"metadata"` or
/// `"three-app"`.
pub fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/fixtures")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|_| panic!("the {name} fixture ships with the driver crate"))
}

/// The directory `cargo build` writes a fixture's binaries to, following
/// `CARGO_TARGET_DIR` when the environment sets one (relative values are
/// resolved against the project root, like Cargo does).
pub fn target_dir(root: &Path) -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                dir
            } else {
                root.join(dir)
            }
        }
        None => root.join("target"),
    }
}

/// Returns whether the built binary at `binary` contains `needle`.
///
/// The generated cross-binary code carries diagnostic strings and symbols that
/// exist nowhere in the fixture sources, so finding one proves the build ran in
/// codegen mode (not metadata mode) and linked the generated code.
pub fn binary_contains(binary: &Path, needle: &str) -> bool {
    let bytes = std::fs::read(binary).expect("read the built binary");
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Copies the checked-in end-to-end fixture into a fresh temporary directory.
///
/// `target/` is skipped, and dependency paths that leave the fixture root (the
/// in-tree workspace crates) are rewritten to absolute paths so the copy is a
/// self-contained project. Paths inside the fixture stay relative, so the copy
/// uses its own application sources.
pub fn fixture_copy() -> tempfile::TempDir {
    let source = fixture_root("e2e");
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
