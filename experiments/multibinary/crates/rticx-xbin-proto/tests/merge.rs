//! Merge and validation tests.
//!
//! One positive and one negative test per validation rule: core-id
//! consistency, receiver topology, priority disjointness, type existence and
//! pool fit. Negative cases assert the exact error variant (and, for the
//! central rules, the deterministic message).
//!
//! The task topology comes exclusively from the receivers' singular `spawn_by`.
//! A receiver always runs on its own binary's `core_id`, so its `spawn_by`
//! names the producer core directly. The per-direction pool views come from the
//! distro capability binding each manifest records, not from `rticx.toml`.

use rticx_xbin_proto::{
    AppManifest, CoreCapability, FIFO_HEADER, Hash64, IpcTypes, MANIFEST_SCHEMA_VERSION,
    MergeError, PoolDecl, ProjectConfig, ReceiverDecl, TargetRef, TypeKind, merge_project,
    parse_idl_str, parse_project_str,
};

const IDL: &str = r#"
schema = 1

[message.EncryptReq]
fields = { addr = "u32", len = "u32", key = "u32" }

[message.OtherReq]
fields = { id = "u16" }

[enum.State]
variants = ["Idle", "Busy"]
"#;

/// The two-application fixture: m7 (core 0) sends, m4 (core 1) receives. No
/// application declares any cross-binary visibility: the edge is inferred from
/// the receiver's `spawn_by` alone.
const TWO_APPS: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_id = 0

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_id = 1
"#;

/// Three applications: m7 (core 0), m4 (core 1) and m5 (core 2).
const THREE_APPS: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_id = 0

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_id = 1

[[application]]
package = "app-m5"
target = { kind = "bin", name = "m5" }
core_id = 2
"#;

/// The mock fixture's pool bases (see `xbin-mock-distro::fixture_pools`).
const BASE_01: u32 = 0x3004_0000;
const BASE_02: u32 = 0x3004_2000;
const BASE_12: u32 = 0x3004_1000;
/// Default pool budget of the fixture.
const BUDGET: u32 = 4096;

fn config(source: &str) -> ProjectConfig {
    parse_project_str(source).expect("valid project manifest")
}

fn idl(source: &str) -> IpcTypes {
    parse_idl_str(source).expect("valid IDL")
}

fn manifest(package: &str, target: &str) -> AppManifest {
    AppManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        package: package.to_string(),
        target: TargetRef::bin(target),
        interface_hash: Hash64::of(package.as_bytes()),
        types: Vec::new(),
        receivers: Vec::new(),
        capability: None,
    }
}

fn receiver(
    name: &str,
    priority: u16,
    capacity: usize,
    spawn_by: u32,
    input: &str,
) -> ReceiverDecl {
    ReceiverDecl {
        name: name.to_string(),
        priority,
        capacity,
        spawn_by,
        input_type: input.to_string(),
    }
}

/// One distro pool as seen from one endpoint (both views equal, like the mock).
fn pool(id: &str, peer: u32, base: u32, budget: u32) -> PoolDecl {
    PoolDecl {
        id: id.to_string(),
        peer,
        base_local: base,
        base_peer: base,
        budget,
    }
}

/// The capability entry of a single-core binary.
fn capability(physical_core: u32, pools: Vec<PoolDecl>) -> CoreCapability {
    CoreCapability {
        physical_core,
        pools,
    }
}

/// Mutable access to a manifest's capability pools.
fn pools_mut(manifest: &mut AppManifest) -> &mut Vec<PoolDecl> {
    &mut manifest
        .capability
        .as_mut()
        .expect("the fixture binds a capability")
        .pools
}

/// `app-m7`: the producer of global core 0 (physical 0), no declarations.
fn m7_manifest() -> AppManifest {
    m7_with_budget(BUDGET)
}

/// `app-m7` with the `p01` dual carved to `budget` bytes. Both endpoints must
/// report the same budget, so pair this with [`m4_with_budget`].
fn m7_with_budget(budget: u32) -> AppManifest {
    let mut manifest = manifest("app-m7", "m7");
    manifest.capability = Some(capability(
        0,
        vec![
            pool("p01", 1, BASE_01, budget),
            pool("p02", 2, BASE_02, BUDGET),
        ],
    ));
    manifest
}

/// `app-m4`: executes `EncryptTask` for global core 0 (physical 1).
fn m4_manifest() -> AppManifest {
    m4_with_budget(BUDGET)
}

/// `app-m4` with the `p01` dual carved to `budget` bytes.
fn m4_with_budget(budget: u32) -> AppManifest {
    let mut manifest = manifest("app-m4", "m4");
    manifest.types = vec!["ipc_types::EncryptReq".to_string()];
    manifest.receivers = vec![receiver("EncryptTask", 3, 2, 0, "ipc_types::EncryptReq")];
    manifest.capability = Some(capability(
        1,
        vec![
            pool("p01", 0, BASE_01, budget),
            pool("p12", 2, BASE_12, BUDGET),
        ],
    ));
    manifest
}

/// `app-m5`: no declarations of its own (global core 2, physical 2).
fn m5_manifest() -> AppManifest {
    let mut manifest = manifest("app-m5", "m5");
    manifest.capability = Some(capability(
        2,
        vec![
            pool("p02", 0, BASE_02, BUDGET),
            pool("p12", 1, BASE_12, BUDGET),
        ],
    ));
    manifest
}

fn merge(
    project: &str,
    manifests: &[AppManifest],
) -> Result<rticx_xbin_proto::MergedProject, MergeError> {
    merge_project(&config(project), manifests, &idl(IDL))
}

fn error(project: &str, manifests: &[AppManifest]) -> MergeError {
    merge(project, manifests).expect_err("the merge must be rejected")
}

// ---------------------------------------------------------------------------
// Positive: the reference topology
// ---------------------------------------------------------------------------

#[test]
fn merges_the_producer_receiver_fixture() {
    let merged = merge(TWO_APPS, &[m7_manifest(), m4_manifest()]).expect("valid project");

    let packages: Vec<&str> = merged
        .apps()
        .iter()
        .map(|app| app.package.as_str())
        .collect();
    assert_eq!(packages, ["app-m7", "app-m4"]);
    assert_eq!(
        merged
            .apps()
            .iter()
            .map(|app| app.core_id)
            .collect::<Vec<_>>(),
        [0, 1]
    );

    let cores: Vec<(u32, &str)> = merged
        .cores()
        .iter()
        .map(|core| (core.global_id, core.app.as_str()))
        .collect();
    assert_eq!(cores, [(0, "app-m7"), (1, "app-m4")]);

    let types: Vec<&str> = merged.types().iter().map(|ty| ty.name.as_str()).collect();
    assert_eq!(types, ["EncryptReq", "OtherReq", "State"]);
    let encrypt = &merged.types()[0];
    assert_eq!(
        (encrypt.kind, encrypt.size, encrypt.align),
        (TypeKind::Message, 12, 4)
    );
    assert_eq!(
        encrypt
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.ty.as_str(), field.offset))
            .collect::<Vec<_>>(),
        [("addr", "u32", 0), ("len", "u32", 4), ("key", "u32", 8)]
    );
    let state = &merged.types()[2];
    assert_eq!(
        (state.kind, state.size, state.align),
        (TypeKind::Enum, 4, 4)
    );
    assert_eq!(
        state
            .variants
            .iter()
            .map(|variant| (variant.name.as_str(), variant.discriminant))
            .collect::<Vec<_>>(),
        [("Idle", 0), ("Busy", 1)]
    );

    assert_eq!(merged.tasks().len(), 1);
    let task = merged.task("EncryptTask").expect("task exists");
    assert_eq!(task.id(), 1);
    assert_eq!(task.receiver_core(), 1);
    assert_eq!(task.spawner_core(), 0);
    assert_eq!(task.priority(), 3);
    assert_eq!(task.capacity(), 2);
    assert_eq!(task.input_type(), "EncryptReq");
    assert_eq!(task.elem_size(), 12);
    assert_eq!(task.elem_align(), 4);
    assert_eq!(task.fifo_offset(), 0, "the first FIFO starts at offset 0");
    assert_eq!(task.fifo_depth(), 3, "depth = capacity + 1");
    assert_eq!(task.fifo_bytes(), FIFO_HEADER + 3 * 12);
    assert_eq!(task.fifo_bytes(), 44);
    assert_eq!(
        task.fifo(),
        rticx_xbin_proto::FifoEntry {
            source: 0,
            target: 1,
            pool: Some("p01".to_string()),
            offset: 0,
            elem_size: 12,
            depth: 3,
        }
    );

    let pools: Vec<(&str, u32, u32)> = merged
        .pools()
        .iter()
        .map(|pool| (pool.id.as_str(), pool.core_a, pool.core_b))
        .collect();
    assert_eq!(pools, [("p01", 0, 1)]);
    let pool = &merged.pools()[0];
    assert_eq!(pool.base_from_a, BASE_01);
    assert_eq!(pool.base_from_b, BASE_01);
    assert_eq!(pool.budget, BUDGET);
    assert_eq!(pool.used, 44, "one FIFO uses the shared pool budget");
}

#[test]
fn merged_view_is_deterministic_and_tasks_are_numbered_in_name_order() {
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Zeta", 4, 1, 0, "ipc_types::EncryptReq"),
        receiver("Alpha", 5, 1, 0, "ipc_types::EncryptReq"),
    ];
    let manifests = [m7_manifest(), m4];

    let first = merge(TWO_APPS, &manifests).expect("valid project");
    let second = merge(TWO_APPS, &manifests).expect("valid project");
    assert_eq!(first, second, "identical inputs merge identically");

    let tasks: Vec<(&str, u32)> = first
        .tasks()
        .iter()
        .map(|task| (task.name(), task.id()))
        .collect();
    assert_eq!(tasks, [("Alpha", 1), ("Zeta", 2)]);
}

#[test]
fn empty_project_merges_to_an_empty_view() {
    let merged =
        merge_project(&config("schema = 1\n"), &[], &IpcTypes::empty()).expect("empty project");

    assert!(merged.apps().is_empty());
    assert!(merged.cores().is_empty());
    assert!(merged.types().is_empty());
    assert!(merged.tasks().is_empty());
    assert!(merged.pools().is_empty());
}

#[test]
fn a_project_without_a_capability_binding_allocates_unbounded() {
    // A fixture that binds no distribution backend carries no capability
    // binding: the merge still allocates the FIFOs deterministically and emits
    // no pool.
    let mut m7 = manifest("app-m7", "m7");
    m7.capability = None;
    let mut m4 = m4_manifest();
    m4.capability = None;

    let merged = merge(TWO_APPS, &[m7, m4]).expect("no capability binding is not an error");
    assert_eq!(merged.tasks().len(), 1);
    assert_eq!(merged.tasks()[0].fifo_offset(), 0);
    assert!(merged.pools().is_empty());
}

// ---------------------------------------------------------------------------
// Core-id consistency
// ---------------------------------------------------------------------------

#[test]
fn rejects_missing_manifest() {
    let error = error(TWO_APPS, &[m7_manifest()]);
    assert_eq!(
        error.to_string(),
        "application `app-m4` has no manifest; `cargo xbin sync` must collect \
         `m4.xbin.json` for every `[[application]]`"
    );
    assert!(matches!(
        &error,
        MergeError::MissingManifest { package, .. } if package == "app-m4"
    ));
}

#[test]
fn rejects_undeclared_application() {
    let extra = manifest("app-extra", "extra");
    let error = error(TWO_APPS, &[m7_manifest(), m4_manifest(), extra]);
    assert!(
        matches!(
            &error,
            MergeError::UndeclaredApplication { package } if package == "app-extra"
        ),
        "{error}"
    );
}

#[test]
fn rejects_duplicate_manifest() {
    let error = error(TWO_APPS, &[m7_manifest(), m4_manifest(), m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::DuplicateManifest { package } if package == "app-m4"
        ),
        "{error}"
    );
}

#[test]
fn rejects_target_mismatch() {
    let mut m7 = m7_manifest();
    m7.target = TargetRef::bin("other");
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::TargetMismatch { package, declared, found }
                if package == "app-m7" && declared == "m7" && found == "other"
        ),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Receiver topology: producer inference from `spawn_by`
// ---------------------------------------------------------------------------

#[test]
fn infers_the_producer_from_the_receivers_spawn_by() {
    // Neither application declares the other: the `0 -> 1` edge is inferred
    // from the receiver's `spawn_by` alone.
    let merged = merge(TWO_APPS, &[m7_manifest(), m4_manifest()]).expect("valid project");
    let task = merged.task("EncryptTask").expect("task exists");
    assert_eq!(
        (task.spawner_core(), task.receiver_core()),
        (0, 1),
        "the producer is the receiver's `spawn_by`"
    );
}

#[test]
fn rejects_unknown_core_in_spawn_by() {
    let mut m4 = m4_manifest();
    m4.receivers[0].spawn_by = 9;
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::UnknownCore { core: 9, context, .. } if context.contains("spawn_by")
        ),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "`app-m4` references global core id 9 in `spawn_by` of receiver `EncryptTask`, \
         but no application declares it"
    );
}

#[test]
fn rejects_own_core_in_spawn_by() {
    // `app-m4`'s core is 1; a `spawn_by = 1` names its own core, so the task
    // would be core-local and must drop `spawn_by`.
    let mut m4 = m4_manifest();
    m4.receivers[0].spawn_by = 1;
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(&error, MergeError::OwnSpawnerCore { core: 1, .. }),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "receiver `EncryptTask` in `app-m4` declares `spawn_by = 1`, which is its own core id; \
         a core-local task must not set `spawn_by`, so drop it"
    );
}

#[test]
fn rejects_duplicate_receiver() {
    let mut m7 = m7_manifest();
    m7.receivers = vec![receiver("EncryptTask", 3, 2, 1, "ipc_types::EncryptReq")];
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::DuplicateReceiver { task, first, second }
                if task == "EncryptTask" && first == "app-m7" && second == "app-m4"
        ),
        "{error}"
    );
}

#[test]
fn rejects_unknown_receiver_input_type() {
    let mut m4 = m4_manifest();
    m4.receivers[0].input_type = "ipc_types::Missing".to_string();
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::UnknownInputType { package, task, type_name }
                if package == "app-m4" && task == "EncryptTask" && type_name == "Missing"
        ),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Priority disjointness
// ---------------------------------------------------------------------------

#[test]
fn rejects_priority_conflict_between_sources() {
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Alpha", 3, 1, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 2, "ipc_types::EncryptReq"),
    ];

    let error = error(THREE_APPS, &[m7_manifest(), m4, m5_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::PriorityConflict {
                target: 1,
                priority: 3,
                first_task,
                first_source: 0,
                second_task,
                second_source: 2,
            } if first_task == "Alpha" && second_task == "Beta"
        ),
        "{error}"
    );
}

#[test]
fn allows_same_priority_from_the_same_source() {
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Alpha", 3, 1, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 0, "ipc_types::EncryptReq"),
    ];

    let merged =
        merge(TWO_APPS, &[m7_manifest(), m4]).expect("one dispatcher line drains both FIFOs");
    assert_eq!(merged.tasks().len(), 2);
    assert!(merged.tasks().iter().all(|task| task.spawner_core() == 0));
}

#[test]
fn allows_the_same_priority_on_different_targets() {
    let mut m4 = m4_manifest();
    m4.receivers = vec![receiver("Alpha", 3, 1, 0, "ipc_types::EncryptReq")];

    let mut m5 = m5_manifest();
    m5.receivers = vec![receiver("Beta", 3, 1, 0, "ipc_types::EncryptReq")];

    let merged =
        merge(THREE_APPS, &[m7_manifest(), m4, m5]).expect("different targets do not conflict");
    assert_eq!(merged.tasks().len(), 2);
}

// ---------------------------------------------------------------------------
// Pool graph and adjacency
// ---------------------------------------------------------------------------

#[test]
fn rejects_a_one_sided_pool() {
    // `app-m7` reports `p01` towards physical core 1, but `app-m4` does not
    // report the matching pool back.
    let mut m4 = m4_manifest();
    pools_mut(&mut m4).retain(|pool| pool.id != "p01");

    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert_eq!(
        error.to_string(),
        "IPC pool `p01` between cores 0 and 1 is not reported consistently by both endpoints; \
         each must report the same pool id with the opposite physical core, the same budget, \
         and the two base views swapped"
    );
    assert!(matches!(
        &error,
        MergeError::IpcPoolMismatch {
            id,
            core_a: 0,
            core_b: 1
        } if id == "p01"
    ));
}

#[test]
fn rejects_an_inconsistent_budget() {
    // The two endpoints must agree on the shared budget.
    let error = error(TWO_APPS, &[m7_with_budget(2048), m4_with_budget(1024)]);
    assert!(
        matches!(
            &error,
            MergeError::IpcPoolMismatch {
                id,
                core_a: 0,
                core_b: 1
            } if id == "p01"
        ),
        "{error}"
    );
}

#[test]
fn rejects_disagreeing_base_views() {
    // `app-m4`'s view of core 0 must be `app-m7`'s view of physical core 1.
    let mut m4 = m4_manifest();
    pools_mut(&mut m4)[0].base_peer = 0x4000_0000;

    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(&error, MergeError::IpcPoolMismatch { id, .. } if id == "p01"),
        "{error}"
    );
}

#[test]
fn rejects_a_used_direction_without_a_pool_path() {
    // Both applications bind a capability table, but neither reports a pool
    // between cores 0 and 1.
    let mut m7 = m7_manifest();
    pools_mut(&mut m7).retain(|pool| pool.id != "p01");
    let mut m4 = m4_manifest();
    pools_mut(&mut m4).retain(|pool| pool.id != "p01");

    let error = error(TWO_APPS, &[m7, m4]);
    assert_eq!(
        error.to_string(),
        "task `EncryptTask` needs a `0->1` IPC pool, but the distribution provides no pool \
         between those cores"
    );
    assert!(matches!(
        &error,
        MergeError::NoIpcPath {
            producer: 0,
            target: 1,
            task
        } if task == "EncryptTask"
    ));
}

#[test]
fn rejects_overlapping_pool_views() {
    // Core 0 sees two pools at overlapping addresses.
    let mut m7 = manifest("app-m7", "m7");
    m7.capability = Some(capability(
        0,
        vec![
            pool("p01", 1, 0x3000_0000, BUDGET),
            pool("p02", 2, 0x3000_0800, BUDGET),
        ],
    ));
    let mut m4 = manifest("app-m4", "m4");
    m4.capability = Some(capability(1, vec![pool("p01", 0, 0x3000_0000, BUDGET)]));
    let mut m5 = manifest("app-m5", "m5");
    m5.capability = Some(capability(2, vec![pool("p02", 0, 0x3000_0800, BUDGET)]));

    let error = error(THREE_APPS, &[m7, m4, m5]);
    assert_eq!(
        error.to_string(),
        "the `p01` and `p02` pool views overlap on core 0: \
         0x30000000..0x30001000 and 0x30000800..0x30001800; \
         a core's pool views must be disjoint"
    );
    assert!(matches!(
        &error,
        MergeError::PoolViewOverlap {
            core: 0,
            first,
            second,
            ..
        } if first == "p01" && second == "p02"
    ));
}

#[test]
fn allows_adjacent_pool_views() {
    // Adjacent, non-overlapping views of core 0 are fine.
    let mut m7 = manifest("app-m7", "m7");
    m7.capability = Some(capability(
        0,
        vec![
            pool("p01", 1, 0x3000_0000, BUDGET),
            pool("p02", 2, 0x3000_1000, BUDGET),
        ],
    ));
    let mut m4 = manifest("app-m4", "m4");
    m4.capability = Some(capability(1, vec![pool("p01", 0, 0x3000_0000, BUDGET)]));
    let mut m5 = manifest("app-m5", "m5");
    m5.capability = Some(capability(2, vec![pool("p02", 0, 0x3000_1000, BUDGET)]));

    let merged = merge(THREE_APPS, &[m7, m4, m5]).expect("adjacent views do not overlap");
    assert_eq!(merged.pools().len(), 2);
}

#[test]
fn ignores_pools_to_cores_outside_the_project() {
    // `p02` peers physical core 2, which the two-application project does not
    // contain: the pool is not part of the graph.
    let merged = merge(TWO_APPS, &[m7_manifest(), m4_manifest()]).expect("unused pool");
    assert_eq!(
        merged
            .pools()
            .iter()
            .map(|pool| pool.id.as_str())
            .collect::<Vec<_>>(),
        ["p01"]
    );
}

// ---------------------------------------------------------------------------
// Shared pool budget
// ---------------------------------------------------------------------------

#[test]
fn shares_one_budget_between_both_directions() {
    // `BackTask` (1 -> 0, OtherReq, capacity 1) is 12 bytes; `EncryptTask`
    // (0 -> 1, EncryptReq, capacity 2) is 44. Both use the one `p01` budget,
    // allocated in name order.
    let mut m7 = m7_with_budget(200);
    m7.receivers = vec![receiver("BackTask", 2, 1, 1, "ipc_types::OtherReq")];

    let merged = merge(TWO_APPS, &[m7, m4_with_budget(200)]).expect("both fit the shared budget");
    assert_eq!(merged.pools().len(), 1);
    assert_eq!(merged.pools()[0].used, 60);
    assert_eq!(merged.task("BackTask").expect("task").fifo_offset(), 0);
    assert_eq!(
        merged.task("EncryptTask").expect("task").fifo_offset(),
        16,
        "the second direction continues after the first in the shared budget"
    );
}

#[test]
fn rejects_pool_budget_overflow() {
    // Each direction fits alone, but the shared budget is 50, below the 60 the
    // two FIFOs need.
    let mut m7 = m7_with_budget(50);
    m7.receivers = vec![receiver("BackTask", 2, 1, 1, "ipc_types::OtherReq")];

    let error = error(TWO_APPS, &[m7, m4_with_budget(50)]);
    assert_eq!(
        error.to_string(),
        "the FIFOs of the `0<->1` pool `p01` need 60 bytes, but the distribution reserves 50"
    );
    assert!(matches!(
        &error,
        MergeError::PoolBudgetExceeded {
            pool,
            core_a: 0,
            core_b: 1,
            needed: 60,
            budget: 50
        } if pool == "p01"
    ));
}

#[test]
fn allows_an_exact_fit() {
    // `BackTask` (OtherReq, capacity 1) needs 8 + 2 * 2 = 12 bytes and
    // `EncryptTask` (EncryptReq, capacity 2) needs 8 + 3 * 12 = 44; the aligned
    // total (12 rounded up to the next 8-byte boundary, then 44) exactly fills
    // the 60-byte `p01` budget.
    let mut m7 = m7_with_budget(60);
    m7.receivers = vec![receiver("BackTask", 2, 1, 1, "ipc_types::OtherReq")];

    let merged = merge(TWO_APPS, &[m7, m4_with_budget(60)]).expect("exact fit");
    assert_eq!(merged.pools()[0].used, 60);
    assert_eq!(merged.task("BackTask").expect("task").fifo_bytes(), 12);
    assert_eq!(merged.task("EncryptTask").expect("task").fifo_bytes(), 44);
}

#[test]
fn pool_fit_accounts_for_fifo_alignment() {
    let mut m4 = m4_with_budget(91);
    m4.receivers = vec![
        receiver("Alpha", 3, 2, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 4, 2, 0, "ipc_types::EncryptReq"),
    ];

    // 44 bytes each; the second FIFO starts at the next 8-byte boundary (48).
    let error = error(TWO_APPS, &[m7_with_budget(91), m4.clone()]);
    assert_eq!(
        error.to_string(),
        "the FIFOs of the `0<->1` pool `p01` need 92 bytes, but the distribution reserves 91"
    );
    assert!(
        matches!(
            &error,
            MergeError::PoolBudgetExceeded {
                needed: 92,
                budget: 91,
                ..
            }
        ),
        "{error}"
    );

    let mut m4 = m4_with_budget(92);
    m4.receivers = vec![
        receiver("Alpha", 3, 2, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 4, 2, 0, "ipc_types::EncryptReq"),
    ];
    let merged = merge(TWO_APPS, &[m7_with_budget(92), m4]).expect("92 bytes fit exactly");
    assert_eq!(merged.tasks().len(), 2);
    assert_eq!(merged.task("Alpha").expect("task").fifo_offset(), 0);
    assert_eq!(
        merged.task("Beta").expect("task").fifo_offset(),
        48,
        "the second FIFO is aligned to 8 bytes"
    );
}
