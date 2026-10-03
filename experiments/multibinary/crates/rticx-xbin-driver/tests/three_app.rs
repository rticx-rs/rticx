//! M6-T1 acceptance: the three-application fixture builds with
//! `cargo xbin build`.
//!
//! `fixtures/three-app` is a standalone workspace with three binaries in a
//! fully connected cross-binary topology: `app-m7` owns global core 0,
//! `app-m4` global core 1 and `app-m5` global core 2, and every ordered pair
//! has its own region. Each application is therefore both a producer (its
//! generated sender stubs spawn tasks on the other two cores) and a receiver
//! (it declares `#[sw_task]` cross-binary receivers on priority lines that are
//! disjoint between the two source cores). The fixture's own mock distribution
//! runs the real core pass on `MockCoreBackend`, so the generated phase-2 code
//! compiles and links on the host.

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

    // Phase 2 really ran in every producer direction: each binary carries the
    // generated FIFO view diagnostics of the regions it spawns into.
    for (bin, direction) in [
        ("m7", "0 -> 1"),
        ("m7", "0 -> 2"),
        ("m5", "2 -> 1"),
        ("m5", "2 -> 0"),
        ("m4", "1 -> 0"),
        ("m4", "1 -> 2"),
    ] {
        assert!(
            binary_contains(
                &binaries.join(bin),
                &format!("`cargo xbin sync` allocated the `({direction})` IPC region"),
            ),
            "the {bin} binary does not contain the generated FIFO view for `{direction}`"
        );
    }

    // Each receiver binary carries one per-pair router and one line dispatcher
    // per cross line (M6.5-T3/T4).
    let receiver_symbols: [(&str, &[&str]); 3] = [
        (
            "m4",
            &[
                "RTICX_XBIN_ROUTER0_TO1",
                "RTICX_XBIN_ROUTER2_TO1",
                "RTICX_XBIN_DISPATCHER0_TO1_P3",
                "RTICX_XBIN_DISPATCHER0_TO1_P5",
                "RTICX_XBIN_DISPATCHER2_TO1_P4",
                "RTICX_XBIN_DISPATCHER2_TO1_P6",
            ],
        ),
        (
            "m7",
            &[
                "RTICX_XBIN_ROUTER1_TO0",
                "RTICX_XBIN_ROUTER2_TO0",
                "RTICX_XBIN_DISPATCHER1_TO0_P3",
                "RTICX_XBIN_DISPATCHER2_TO0_P6",
            ],
        ),
        (
            "m5",
            &[
                "RTICX_XBIN_ROUTER0_TO2",
                "RTICX_XBIN_ROUTER1_TO2",
                "RTICX_XBIN_DISPATCHER0_TO2_P5",
                "RTICX_XBIN_DISPATCHER1_TO2_P3",
            ],
        ),
    ];
    for (bin, symbols) in receiver_symbols {
        let binary = binaries.join(bin);
        for symbol in symbols {
            assert!(
                binary_contains(&binary, symbol),
                "the {bin} binary does not contain the generated `{symbol}` task"
            );
        }
    }

    // The synced view places one task per receiver, sorted by name, on the
    // receiver's disjoint priority line.
    let system = outcome
        .sync
        .system
        .as_ref()
        .expect("sync emits the system view");
    assert_eq!(
        system
            .tasks
            .iter()
            .map(|task| (
                task.name.as_str(),
                task.spawner_core,
                task.receiver_core,
                task.priority,
            ))
            .collect::<Vec<_>>(),
        [
            ("AckTask", 1, 0, 3),
            ("ConfigTask", 0, 2, 5),
            ("DigestTask", 0, 1, 5),
            ("EncryptTask", 0, 1, 3),
            ("HeartbeatTask", 2, 0, 6),
            ("SensorTask", 2, 1, 4),
            ("StatusTask", 1, 2, 3),
            ("TelemetryTask", 2, 1, 6),
        ]
    );

    // Both `0->1` and `2->1` pack two FIFOs; the four reverse/side directions
    // fit one each.
    for region in &system.regions {
        let expected = if (region.source, region.target) == (0, 1)
            || (region.source, region.target) == (2, 1)
        {
            256
        } else {
            128
        };
        assert_eq!(
            region.size, expected,
            "region `{} -> {}` has the wrong size",
            region.source, region.target
        );
    }

    // One doorbell per `(source, target, priority)` line.
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
        [
            (0, 1, 3, 0),
            (0, 1, 5, 1),
            (0, 2, 5, 0),
            (1, 0, 3, 0),
            (1, 2, 3, 1),
            (2, 0, 6, 1),
            (2, 1, 4, 2),
            (2, 1, 6, 3),
        ]
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
