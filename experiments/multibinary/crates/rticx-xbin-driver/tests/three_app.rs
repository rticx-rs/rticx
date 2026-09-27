//! M6-T1 acceptance: the three-application fixture builds with
//! `cargo xbin build`.
//!
//! `fixtures/three-app` is a standalone workspace with two producer binaries
//! (`app-m7` on global core 0, `app-m5` on global core 2) and one receiver
//! binary (`app-m4` on global core 1). The receiver declares two native
//! `#[sw_task]` receivers on disjoint priority lines, so phase 2 generates two
//! line dispatchers and two per-pair doorbell routers; both producers declare
//! nothing and receive their generated sender stubs from the synced view. The
//! fixture's own mock distribution runs the real core pass on
//! `MockCoreBackend`, so the generated phase-2 code compiles and links on the
//! host.

use std::path::{Path, PathBuf};

use rticx_xbin_driver::build;

/// Root of the checked-in three-application fixture.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/three-app")
        .canonicalize()
        .expect("the three-app fixture ships with the driver crate")
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
fn binary_contains(binary: &Path, needle: &str) -> bool {
    let bytes = std::fs::read(binary).expect("read the built binary");
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[test]
fn cargo_xbin_build_builds_the_three_applications() {
    let root = fixture_root();

    let outcome = build(&root).expect("cargo xbin build succeeds");

    assert_eq!(outcome.sync.applications().len(), 3);
    assert_eq!(
        outcome
            .builds
            .iter()
            .map(|build| (build.package.as_str(), build.target.as_str()))
            .collect::<Vec<_>>(),
        [("app-m7", "m7"), ("app-m5", "m5"), ("app-m4", "m4")],
        "builds follow `rticx.toml` order"
    );

    let binaries = target_dir(&root).join("debug");
    for bin in ["m7", "m5", "m4"] {
        assert!(
            binaries.join(bin).is_file(),
            "`cargo xbin build` produced no `{bin}` binary below `{}`",
            binaries.display()
        );
    }

    // Phase 2 really ran in both producer binaries: each contains the
    // generated FIFO view diagnostic of its own `(source -> target)` region.
    assert!(
        binary_contains(
            &binaries.join("m7"),
            "`cargo xbin sync` allocated the `(0 -> 1)` IPC region",
        ),
        "the m7 binary does not contain the generated FIFO view"
    );
    assert!(
        binary_contains(
            &binaries.join("m5"),
            "`cargo xbin sync` allocated the `(2 -> 1)` IPC region",
        ),
        "the m5 binary does not contain its generated FIFO view"
    );

    // The receiver binary carries two dispatcher lines and two per-pair
    // routers, one per producer core (M6.5-T3/T4).
    let m4 = binaries.join("m4");
    for symbol in [
        "RTICX_XBIN_ROUTER0_TO1",
        "RTICX_XBIN_ROUTER2_TO1",
        "RTICX_XBIN_DISPATCHER0_TO1_P3",
        "RTICX_XBIN_DISPATCHER2_TO1_P4",
    ] {
        assert!(
            binary_contains(&m4, symbol),
            "the receiver binary does not contain the generated `{symbol}` task"
        );
    }

    // The synced view places one task per producer direction, on disjoint
    // priority lines.
    let system = outcome
        .sync
        .system
        .as_ref()
        .expect("sync emits the system view");
    assert_eq!(system.tasks.len(), 2);
    assert_eq!(system.tasks[0].name, "EncryptTask");
    assert_eq!(system.tasks[0].fifo.source, 0);
    assert_eq!(system.tasks[0].fifo.target, 1);
    assert_eq!(system.tasks[1].name, "SensorTask");
    assert_eq!(system.tasks[1].fifo.source, 2);
    assert_eq!(system.tasks[1].fifo.target, 1);
    assert_eq!(
        system
            .doorbells
            .iter()
            .map(|doorbell| (
                doorbell.source,
                doorbell.target,
                doorbell.priority,
                doorbell.line
            ))
            .collect::<Vec<_>>(),
        [(0, 1, 3, 0), (2, 1, 4, 1)]
    );

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
