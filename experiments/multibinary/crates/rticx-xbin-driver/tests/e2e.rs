//! M4-T1 acceptance: the two-application end-to-end fixture builds with
//! `cargo xbin build`.
//!
//! `fixtures/e2e` is a standalone workspace whose `#[app]` macro is provided
//! by its own mock distribution: the full core pass runs on `MockCoreBackend`
//! and the cross-binary pass generates against a mock `CrossBinBackend`, so
//! the sender and receiver applications build and link on the host. The
//! generated code is the real phase-2 output (FIFO views, `cross_spawn`,
//! doorbell dispatcher, init hooks); only the hardware bindings are mocked.

use std::path::{Path, PathBuf};

use rticx_xbin_driver::build;

/// Root of the checked-in end-to-end fixture.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/e2e")
        .canonicalize()
        .expect("the end-to-end fixture ships with the driver crate")
}

/// The directory `cargo build` writes the fixture's binaries to, following
/// `CARGO_TARGET_DIR` when the environment sets one (relative values are
/// resolved against the project root, like Cargo does).
fn target_dir(root: &Path) -> PathBuf {
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
/// The generated phase-2 code carries diagnostic strings that exist nowhere in
/// the fixture sources, so finding one proves the build ran in codegen mode
/// (not metadata mode) and linked the generated code.
fn binary_contains(binary: &Path, needle: &str) -> bool {
    let bytes = std::fs::read(binary).expect("read the built binary");
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn cargo_xbin_build_builds_the_fixture_applications() {
    let root = fixture_root();

    let outcome = build(&root).expect("cargo xbin build succeeds");

    assert_eq!(outcome.sync.applications().len(), 2);
    assert_eq!(
        outcome
            .builds
            .iter()
            .map(|build| (build.package.as_str(), build.target.as_str()))
            .collect::<Vec<_>>(),
        [("app-m7", "m7"), ("app-m4", "m4")],
        "builds follow `rticx.toml` order"
    );

    let binaries = target_dir(&root).join("debug");
    for bin in ["m7", "m4"] {
        assert!(
            binaries.join(bin).is_file(),
            "`cargo xbin build` produced no `{bin}` binary below `{}`",
            binaries.display()
        );
    }

    // Phase 2 really ran: the sender's generated FIFO view and the receiver's
    // generated mark-ready hook both leave their diagnostics in the linked
    // binary.
    assert!(
        binary_contains(
            &binaries.join("m7"),
            "`cargo xbin sync` allocated the `(0 -> 1)` IPC region",
        ),
        "the sender binary does not contain the generated FIFO view"
    );
    assert!(
        binary_contains(
            &binaries.join("m4"),
            "failed to arm doorbell line 0 of global core 1",
        ),
        "the receiver binary does not contain the generated mark-ready hook"
    );

    // The synced view places the single task on the `0->1` direction.
    let system = outcome
        .sync
        .system
        .as_ref()
        .expect("sync emits the system view");
    assert_eq!(system.tasks.len(), 1);
    assert_eq!(system.tasks[0].name, "EncryptTask");
    assert_eq!(system.tasks[0].fifo.source, 0);
    assert_eq!(system.tasks[0].fifo.target, 1);

    // The checked-in generated crate is up to date with the IDL (M1-T7).
    assert!(
        outcome
            .sync
            .ipc_types
            .as_ref()
            .expect("the fixture IDL generates a crate")
            .status
            .is_noop(),
        "the checked-in `ipc-types` must match `ipc-types.toml`; run `cargo xbin sync`"
    );
}
