//! M6-T1 acceptance: the three-application fixture builds with
//! `cargo xbin build`.
//!
//! `crates/mock/fixtures/three-app` is a standalone workspace with three binaries in a
//! fully connected cross-binary topology: `app-m7` owns global core 0,
//! `app-m4` global core 1 and `app-m5` global core 2, and every ordered pair
//! is carried by the mock distribution's pool for that dual. Each application
//! is therefore both a producer (its
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
        .join("../mock/fixtures/three-app")
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

    // Phase 2 really ran in every producer: each binary carries the generated
    // FIFO initializer symbol (which exists nowhere in the fixture sources).
    // The per-direction coverage is asserted through the receiver router and
    // dispatcher symbols below, one per `(source, target)` pair.
    for bin in ["m7", "m5", "m4"] {
        assert!(
            binary_contains(&binaries.join(bin), "__rticx_xbin_init_fifos_core0"),
            "the {bin} binary does not contain the generated FIFO initializer"
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

    // Every unordered pair is one shared pool from the mock distribution;
    // both directions of a dual draw on that pool's 4096-byte budget
    // (M6.9-T4/T5).
    let pools: Vec<(&str, u32, u32)> = system
        .pools
        .iter()
        .map(|pool| (pool.id.as_str(), pool.core_a, pool.core_b))
        .collect();
    assert_eq!(
        pools,
        [("mock-0-1", 0, 1), ("mock-0-2", 0, 2), ("mock-1-2", 1, 2)],
        "one shared pool per dual"
    );
    for pool in &system.pools {
        assert_eq!(
            pool.budget, 4096,
            "pool `{}` carries the distro budget",
            pool.id
        );
        assert!(
            pool.used > 0 && pool.used <= pool.budget,
            "pool `{}` uses {} of {} bytes",
            pool.id,
            pool.used,
            pool.budget
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

    // The generated `ipc_types` module is written into the output directory
    // (M1-T7).
    let ipc_types = outcome
        .sync
        .ipc_types
        .as_ref()
        .expect("the fixture IDL generates the module");
    assert_eq!(
        ipc_types.path,
        outcome
            .sync
            .output_dir
            .join(rticx_xbin_driver::IPC_TYPES_FILE)
    );
    assert!(ipc_types.path.is_file(), "the generated module is on disk");
}
