//! M4-T3 acceptance: the documented negative cases fail with their documented
//! error messages.
//!
//! Two families of negative cases are covered end to end:
//!
//! - **build-phase freshness** over a copy of the checked-in `fixtures/e2e`
//!   project: a plain `cargo build` without a synced view (missing sync), a
//!   source changed after `sync` (source hash mismatch) and a hand-edited
//!   `system.json` (topology hash mismatch). These run the real proc-macro
//!   path (project discovery and environment-based mode selection), not the
//!   pass in isolation;
//! - **merge/validation** through `cargo xbin sync` over generated metadata
//!   projects: a priority line shared by two source cores, an input type
//!   missing from the IDL and a region too small for its task FIFOs.
//!
//! Every assertion checks the exact documented message, so a wording change
//! in the user guide or the error enum fails here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rticx_xbin_driver::{SYSTEM_FILE, sync};
use tempfile::tempdir;

fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

// ---------------------------------------------------------------------------
// Build-phase freshness over a copy of `fixtures/e2e`
// ---------------------------------------------------------------------------

/// Copies the checked-in end-to-end fixture into a fresh temporary directory.
///
/// `target/` is skipped, and dependency paths that leave the fixture root (the
/// in-tree workspace crates) are rewritten to absolute paths so the copy is a
/// self-contained project. Paths inside the fixture stay relative, so the copy
/// uses its own application sources.
fn fixture_copy() -> tempfile::TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/e2e")
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
fn cargo_build(root: &Path, package: &str, bin: &str) -> Output {
    Command::new(cargo())
        .args(["build", "--package", package, "--bin", bin])
        .current_dir(root)
        .env_remove("RTICX_XBIN_META_OUT")
        .env_remove("RTICX_XBIN_SYSTEM")
        .output()
        .expect("failed to run cargo build")
}

#[test]
fn plain_build_rejects_a_missing_or_stale_sync() {
    let dir = fixture_copy();
    let root = dir.path().join("e2e");

    // -- missing sync: a cross-binary application with no synced view --------
    let output = cargo_build(&root, "app-m7", "m7");
    assert!(
        !output.status.success(),
        "a plain build without `cargo xbin sync` must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("failed to read the system view"),
        "{stderr}"
    );
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");

    // -- source changed after sync ------------------------------------------
    sync(&root).expect("the copied fixture syncs");
    let source_path = root.join("app-m7/src/main.rs");
    let source = std::fs::read_to_string(&source_path).expect("app-m7 source");
    let edited = source.replace(
        "mod app {",
        "mod app {\n    const __XBIN_NEGATIVE: u32 = 1;\n",
    );
    assert_ne!(edited, source, "the fixture source shape changed");
    std::fs::write(&source_path, edited).expect("edit app-m7 source");

    let output = cargo_build(&root, "app-m7", "m7");
    assert!(
        !output.status.success(),
        "a source changed after sync must fail a plain build"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("changed since the last `cargo xbin sync`"),
        "{stderr}"
    );
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");

    // -- view edited after sync ---------------------------------------------
    // `app-m4`'s own source is still the synced one; only the view is stale.
    let system_path = root.join("target/rticx-xbin").join(SYSTEM_FILE);
    let view = std::fs::read_to_string(&system_path).expect("system.json");
    let tampered = view.replacen("\"priority\": 3", "\"priority\": 4", 1);
    assert_ne!(tampered, view, "the fixture view has a task priority");
    std::fs::write(&system_path, tampered).expect("tamper with the view");

    let output = cargo_build(&root, "app-m4", "m4");
    assert!(
        !output.status.success(),
        "an edited system view must fail a plain build"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is stale"), "{stderr}");
    assert!(stderr.contains("`topology_hash`"), "{stderr}");
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");
}

// ---------------------------------------------------------------------------
// Merge/validation negatives through `cargo xbin sync`
// ---------------------------------------------------------------------------

/// IDL with one 12-byte message, referenced by the generated projects.
const ENCRYPT_IDL: &str = "schema = 1\n\n\
     [message.EncryptReq]\nfields = { addr = \"u32\", len = \"u32\", key = \"u32\" }\n";

/// `app-m7` (global core 0) produces `Alpha` on global core 1.
const PRODUCER_ALPHA: &str = r#"
use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [0], external_cores = [1])]
mod app {}

fn main() {}
"#;

/// `app-m5` (global core 2) produces `Beta` on global core 1.
const PRODUCER_BETA: &str = r#"
use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [2], external_cores = [1])]
mod app {}

fn main() {}
"#;

/// `app-m7` produces `EncryptTask` on global core 1.
const PRODUCER_ENCRYPT: &str = r#"
use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [0], external_cores = [1])]
mod app {}

fn main() {}
"#;

/// `app-m4` (global core 1) receives `Alpha` and `Beta` on the same priority.
const RECEIVER_ALPHA_BETA: &str = r#"
use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [1], external_cores = [0, 2])]
mod app {
    trait RticSwTask {
        type SpawnInput;
        fn exec(&mut self, input: Self::SpawnInput);
    }

    struct EncryptReq;

    #[sw_task(priority = 3, capacity = 1, spawn_by = 0)]
    struct Alpha;

    impl RticSwTask for Alpha {
        type SpawnInput = EncryptReq;
        fn exec(&mut self, _input: Self::SpawnInput) {}
    }

    #[sw_task(priority = 3, capacity = 1, spawn_by = 2)]
    struct Beta;

    impl RticSwTask for Beta {
        type SpawnInput = EncryptReq;
        fn exec(&mut self, _input: Self::SpawnInput) {}
    }
}

fn main() {}
"#;

/// `app-m4` receives `EncryptTask` whose input is absent from the IDL.
const RECEIVER_UNKNOWN_TYPE: &str = r#"
use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [1], external_cores = [0])]
mod app {
    trait RticSwTask {
        type SpawnInput;
        fn exec(&mut self, input: Self::SpawnInput);
    }

    struct MissingRequest;

    #[sw_task(priority = 3, capacity = 1, spawn_by = 0)]
    struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = MissingRequest;
        fn exec(&mut self, _input: Self::SpawnInput) {}
    }
}

fn main() {}
"#;

/// `app-m4` receives `EncryptTask` with the canonical fixture shape.
const RECEIVER_ENCRYPT: &str = r#"
use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [1], external_cores = [0])]
mod app {
    trait RticSwTask {
        type SpawnInput;
        fn exec(&mut self, input: Self::SpawnInput);
    }

    struct EncryptReq;

    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = EncryptReq;
        fn exec(&mut self, _input: Self::SpawnInput) {}
    }
}

fn main() {}
"#;

/// Writes a metadata-only project: one Cargo package per `(package, bin,
/// source)` triple, all depending on the checked-in `metadata-macro` stand-in,
/// plus `rticx.toml` and `ipc-types.toml`.
fn write_metadata_project(root: &Path, config: &str, idl: &str, apps: &[(&str, &str, &str)]) {
    let macro_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/metadata/metadata-macro")
        .canonicalize()
        .expect("the metadata fixture ships with the driver crate");

    let members = apps
        .iter()
        .map(|(package, _, _)| format!("\"{package}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{members}]\n"),
    )
    .expect("workspace manifest");

    for (package, bin, source) in apps {
        let dir = root.join(package);
        std::fs::create_dir_all(dir.join("src")).expect("create package");
        std::fs::write(
            dir.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                 publish = false\n\n\
                 [[bin]]\nname = \"{bin}\"\npath = \"src/main.rs\"\n\n\
                 [dependencies]\nmetadata-macro = {{ path = \"{}\" }}\n",
                macro_path.display()
            ),
        )
        .expect("package manifest");
        std::fs::write(dir.join("src/main.rs"), source).expect("app source");
    }

    std::fs::write(root.join("rticx.toml"), config).expect("rticx.toml");
    std::fs::write(root.join("ipc-types.toml"), idl).expect("ipc-types.toml");
}

#[test]
fn sync_rejects_a_priority_conflict_between_sources() {
    let dir = tempdir().expect("tempdir");
    let config = "schema = 1\n\n\
         [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
         core_ids = [0]\n\n\
         [[application]]\npackage = \"app-m4\"\ntarget = { kind = \"bin\", name = \"m4\" }\n\
         core_ids = [1]\n\n\
         [[application]]\npackage = \"app-m5\"\ntarget = { kind = \"bin\", name = \"m5\" }\n\
         core_ids = [2]\n\n\
         [ipc.regions]\n\
         \"0->1\" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }\n\
         \"2->1\" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }\n";
    write_metadata_project(
        dir.path(),
        config,
        ENCRYPT_IDL,
        &[
            ("app-m7", "m7", PRODUCER_ALPHA),
            ("app-m4", "m4", RECEIVER_ALPHA_BETA),
            ("app-m5", "m5", PRODUCER_BETA),
        ],
    );

    let error = sync(dir.path())
        .expect_err("two source cores sharing a priority line must be rejected")
        .to_string();
    assert_eq!(
        error,
        "tasks `Alpha` (spawned by core 0) and `Beta` (spawned by core 2) share priority 3 on \
         core 1; priority lines from different source cores must be disjoint"
    );
}

#[test]
fn sync_rejects_an_unknown_input_type() {
    let dir = tempdir().expect("tempdir");
    let config = "schema = 1\n\n\
         [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
         core_ids = [0]\n\n\
         [[application]]\npackage = \"app-m4\"\ntarget = { kind = \"bin\", name = \"m4\" }\n\
         core_ids = [1]\n\n\
         [ipc.regions]\n\
         \"0->1\" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }\n";
    write_metadata_project(
        dir.path(),
        config,
        ENCRYPT_IDL,
        &[
            ("app-m7", "m7", PRODUCER_ENCRYPT),
            ("app-m4", "m4", RECEIVER_UNKNOWN_TYPE),
        ],
    );

    let error = sync(dir.path())
        .expect_err("an input type absent from the IDL must be rejected")
        .to_string();
    assert_eq!(
        error,
        "task `EncryptTask` in `app-m4` uses type `MissingRequest`, which is not declared in \
         `ipc-types.toml`"
    );
}

#[test]
fn sync_rejects_region_overflow() {
    let dir = tempdir().expect("tempdir");
    // `EncryptReq` (12 bytes) at capacity 2 needs 64 + 3 * 12 = 100 bytes.
    let config = "schema = 1\n\n\
         [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
         core_ids = [0]\n\n\
         [[application]]\npackage = \"app-m4\"\ntarget = { kind = \"bin\", name = \"m4\" }\n\
         core_ids = [1]\n\n\
         [ipc.regions]\n\
         \"0->1\" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 99 }\n";
    write_metadata_project(
        dir.path(),
        config,
        ENCRYPT_IDL,
        &[
            ("app-m7", "m7", PRODUCER_ENCRYPT),
            ("app-m4", "m4", RECEIVER_ENCRYPT),
        ],
    );

    let error = sync(dir.path())
        .expect_err("a task FIFO larger than its region must be rejected")
        .to_string();
    assert_eq!(
        error,
        "task `EncryptTask` does not fit the `0->1` region: 100 bytes needed, 99 available"
    );
}
