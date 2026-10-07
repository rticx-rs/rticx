//! M4-T3 acceptance: the documented negative cases fail with their documented
//! error messages.
//!
//! Two families of negative cases are covered end to end:
//!
//! - **build-phase freshness** over a copy of the checked-in `crates/mock/fixtures/e2e`
//!   project: a plain `cargo build` without a synced view (missing sync), a
//!   source changed after `sync` (source hash mismatch), a hand-edited
//!   `system.json` (topology hash mismatch) and an edited `rticx.toml`
//!   (resolved topology vs. the synced view). These run the real proc-macro
//!   path (project discovery and environment-based mode selection), not the
//!   pass in isolation;
//! - **merge/validation** through `cargo xbin sync` over generated metadata
//!   projects: the moved `#[app(core_ids)]`/`#[app(external_cores)]` keys, a
//!   priority line shared by two source cores, an input type missing from the
//!   IDL and a leftover `[ipc.regions]` table (removed in M6.9-T3).
//!
//! Every assertion checks the exact documented message, so a wording change
//! in the user guide or the error enum fails here.

mod common;

use std::path::Path;

use rticx_xbin_driver::{SYSTEM_FILE, sync};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Build-phase freshness over a copy of `crates/mock/fixtures/e2e`
// ---------------------------------------------------------------------------

#[test]
fn plain_build_rejects_a_missing_or_stale_sync() {
    let dir = common::fixture_copy();
    let root = dir.path().join("e2e");

    // -- missing sync: a cross-binary application with no synced view --------
    let output = common::cargo_build(&root, "app-m7", "m7");
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

    // -- interface changed after sync ---------------------------------------
    // The cross-binary fingerprint covers the receiver set: dropping the
    // receiver declaration is an interface change the per-field
    // `resolve_receiver` checks cannot report (the source declares no receiver
    // at all), so the fingerprint is the guard that fires.
    sync(&root).expect("the copied fixture syncs");
    let source_path = root.join("app-m4/src/main.rs");
    let source = std::fs::read_to_string(&source_path).expect("app-m4 source");
    let edited = source.replace(
        "    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]\n",
        "",
    );
    assert_ne!(edited, source, "the fixture source shape changed");
    std::fs::write(&source_path, &edited).expect("edit app-m4 source");

    let output = common::cargo_build(&root, "app-m4", "m4");
    assert!(
        !output.status.success(),
        "an interface changed after sync must fail a plain build"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("interface changed since the last `cargo xbin sync`"),
        "{stderr}"
    );
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");

    // Restore the synced source for the view-edited case below.
    std::fs::write(&source_path, &source).expect("restore app-m4 source");

    // -- view edited after sync ---------------------------------------------
    // `app-m4`'s own source is back to the synced one; only the view is stale.
    let system_path = root.join("target/rticx-xbin").join(SYSTEM_FILE);
    let view = std::fs::read_to_string(&system_path).expect("system.json");
    let tampered = view.replacen("\"priority\": 3", "\"priority\": 4", 1);
    assert_ne!(tampered, view, "the fixture view has a task priority");
    std::fs::write(&system_path, tampered).expect("tamper with the view");

    let output = common::cargo_build(&root, "app-m4", "m4");
    assert!(
        !output.status.success(),
        "an edited system view must fail a plain build"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is stale"), "{stderr}");
    assert!(stderr.contains("`topology_hash`"), "{stderr}");
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");
}

#[test]
fn plain_build_rejects_a_topology_edited_after_sync() {
    let dir = common::fixture_copy();
    let root = dir.path().join("e2e");
    sync(&root).expect("the copied fixture syncs");

    // `rticx.toml` is the single topology source: editing it without a new
    // sync leaves the resolved mapping out of step with the synced view.
    let config_path = root.join("rticx.toml");
    let config = std::fs::read_to_string(&config_path).expect("rticx.toml");
    let edited = config.replacen("core_ids = [0]", "core_ids = [5]", 1);
    assert_ne!(edited, config, "the fixture maps app-m7 to global core 0");
    std::fs::write(&config_path, edited).expect("edit rticx.toml");

    let output = common::cargo_build(&root, "app-m7", "m7");
    assert!(
        !output.status.success(),
        "an edited rticx.toml must fail a plain build"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
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

#[app(device = fixture)]
mod app {}

fn main() {}
"#;

/// `app-m5` (global core 2) produces `Beta` on global core 1.
const PRODUCER_BETA: &str = r#"
use metadata_macro::app;

#[app(device = fixture)]
mod app {}

fn main() {}
"#;

/// `app-m7` produces `EncryptTask` on global core 1.
const PRODUCER_ENCRYPT: &str = r#"
use metadata_macro::app;

#[app(device = fixture)]
mod app {}

fn main() {}
"#;

/// `app-m4` (global core 1) receives `Alpha` and `Beta` on the same priority.
const RECEIVER_ALPHA_BETA: &str = r#"
use metadata_macro::app;

#[app(
    device = fixture,
    dispatchers = [IRQ0, IRQ1]
)]
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

#[app(
    device = fixture,
    dispatchers = [IRQ0]
)]
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

/// Writes a metadata-only project: one Cargo package per `(package, bin,
/// source)` triple, all depending on the checked-in `metadata-macro` stand-in,
/// plus `rticx.toml` and `ipc-types.toml`.
fn write_metadata_project(root: &Path, config: &str, idl: &str, apps: &[(&str, &str, &str)]) {
    let macro_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/fixtures/metadata/metadata-macro")
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

/// `rticx.toml` for a single metadata application.
const SINGLE_APP: &str = "schema = 1\n\n\
     [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
     core_ids = [0]\n";

#[test]
fn sync_rejects_the_moved_topology_arguments() {
    // Since M7-T2 the topology lives in `rticx.toml`; the old `#[app]` keys
    // are hard errors that name the migration.
    for (arg, expected) in [
        ("core_ids = [0]", "`core_ids` moved to `rticx.toml`"),
        (
            "external_cores = [1]",
            "`external_cores` moved to `rticx.toml`",
        ),
    ] {
        let dir = tempdir().expect("tempdir");
        let source = format!(
            "use metadata_macro::app;\n\n#[app(device = fixture, {arg})]\nmod app {{}}\n\nfn main() {{}}\n"
        );
        write_metadata_project(
            dir.path(),
            SINGLE_APP,
            ENCRYPT_IDL,
            &[("app-m7", "m7", source.as_str())],
        );

        let error = sync(dir.path())
            .expect_err("a moved topology argument must be rejected at sync")
            .to_string();
        assert!(
            error.contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
}

#[test]
fn sync_rejects_a_priority_conflict_between_sources() {
    let dir = tempdir().expect("tempdir");
    let config = "schema = 1\n\n\
         [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
         core_ids = [0]\nexternal_cores = [1]\n\n\
         [[application]]\npackage = \"app-m4\"\ntarget = { kind = \"bin\", name = \"m4\" }\n\
         core_ids = [1]\nexternal_cores = [0, 2]\n\n\
         [[application]]\npackage = \"app-m5\"\ntarget = { kind = \"bin\", name = \"m5\" }\n\
         core_ids = [2]\nexternal_cores = [1]\n";
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
         core_ids = [0]\nexternal_cores = [1]\n\n\
         [[application]]\npackage = \"app-m4\"\ntarget = { kind = \"bin\", name = \"m4\" }\n\
         core_ids = [1]\nexternal_cores = [0]\n";
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
fn sync_rejects_the_removed_ipc_regions_table() {
    let dir = tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("rticx.toml"),
        "schema = 1\n\n\
         [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\n\
         core_ids = [0]\n\n\
         [ipc.regions]\n\
         \"0->1\" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }\n",
    )
    .expect("rticx.toml");

    let error = sync(dir.path())
        .expect_err("the removed `[ipc.regions]` table must be rejected")
        .to_string();
    assert_eq!(
        error,
        "`[ipc.regions]` was removed: IPC memory is owned by the distribution's capability \
         binding (M6.9), not declared in rticx.toml. Remove the `[ipc.regions]` table"
    );
}
