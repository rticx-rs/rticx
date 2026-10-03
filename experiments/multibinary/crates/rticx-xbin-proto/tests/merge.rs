//! Merge and validation tests.
//!
//! One positive and one negative test per validation rule: core-id
//! consistency, receiver topology, priority disjointness, type existence and
//! pool fit. Negative cases assert the exact error variant (and, for the
//! central rules, the deterministic message).
//!
//! Since M5.5 the task topology comes exclusively from the receivers'
//! singular `spawn_by`: there are no sender manifests to match. Since M6.9-T3
//! the per-direction pool views come from the distro capability binding each
//! manifest records, not from `rticx.toml`.

use rticx_xbin_proto::{
    AppManifest, CoreCapability, FIFO_HEADER, Hash64, IpcTypes, MergeError, PoolCachePolicy,
    PoolDecl, ProjectConfig, ReceiverDecl, TargetRef, TypeKind, merge_project, parse_idl_str,
    parse_project_str,
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

/// The two-application fixture of the plan (§6.1/§7.2): m7 sends, m4 receives.
const TWO_APPS: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]
"#;

/// Three applications: m7 (core 0), m4 (core 1) and m5 (core 2).
const THREE_APPS: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[[application]]
package = "app-m5"
target = { kind = "bin", name = "m5" }
core_ids = [2]
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

fn manifest(package: &str, target: &str, core_ids: &[u32], external_cores: &[u32]) -> AppManifest {
    AppManifest {
        schema_version: 2,
        package: package.to_string(),
        target: TargetRef::bin(target),
        source_hash: Hash64::of(package.as_bytes()),
        cores: core_ids.len() as u32,
        core_ids: Some(core_ids.to_vec()),
        external_cores: external_cores.to_vec(),
        types: Vec::new(),
        receivers: Vec::new(),
        capabilities: Vec::new(),
    }
}

fn receiver(
    name: &str,
    priority: u16,
    capacity: usize,
    core: u32,
    spawn_by: u32,
    input: &str,
) -> ReceiverDecl {
    ReceiverDecl {
        name: name.to_string(),
        priority,
        capacity,
        core,
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
        policy: PoolCachePolicy::NormalNonCacheableShareable,
    }
}

/// The capability entry of a single-core application.
fn capability(local_core: u32, physical_core: u32, pools: Vec<PoolDecl>) -> CoreCapability {
    CoreCapability {
        local_core,
        physical_core,
        pools,
    }
}

/// `app-m7`: the producer of global core 0 (physical 0), no declarations.
fn m7_manifest() -> AppManifest {
    m7_manifest_with_budget(BUDGET)
}

/// `app-m7` with the `{0, 1}` pool carved to `budget` bytes.
fn m7_manifest_with_budget(budget: u32) -> AppManifest {
    let mut manifest = manifest("app-m7", "m7", &[0], &[1]);
    manifest.capabilities = vec![capability(
        0,
        0,
        vec![
            pool("p01", 1, BASE_01, budget),
            pool("p02", 2, BASE_02, BUDGET),
        ],
    )];
    manifest
}

/// `app-m4`: executes `EncryptTask` for global core 0 (physical 1).
fn m4_manifest() -> AppManifest {
    let mut manifest = manifest("app-m4", "m4", &[1], &[0]);
    manifest.types = vec!["ipc_types::EncryptReq".to_string()];
    manifest.receivers = vec![receiver("EncryptTask", 3, 2, 0, 0, "ipc_types::EncryptReq")];
    manifest.capabilities = vec![capability(
        0,
        1,
        vec![
            pool("p01", 0, BASE_01, BUDGET),
            pool("p12", 2, BASE_12, BUDGET),
        ],
    )];
    manifest
}

/// `app-m5`: no declarations of its own (global core 2, physical 2).
fn m5_manifest() -> AppManifest {
    let mut manifest = manifest("app-m5", "m5", &[2], &[]);
    manifest.capabilities = vec![capability(
        0,
        2,
        vec![
            pool("p02", 0, BASE_02, BUDGET),
            pool("p12", 1, BASE_12, BUDGET),
        ],
    )];
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

    let cores: Vec<(u32, &str, u32)> = merged
        .cores()
        .iter()
        .map(|core| (core.global_id, core.app.as_str(), core.local_index))
        .collect();
    assert_eq!(cores, [(0, "app-m7", 0), (1, "app-m4", 0)]);

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
    assert_eq!(task.fifo_bytes(), 100);
    assert_eq!(
        task.fifo(),
        rticx_xbin_proto::FifoEntry {
            source: 0,
            target: 1,
            offset: 0,
            elem_size: 12,
            depth: 3,
        }
    );

    let regions: Vec<(u32, u32)> = merged
        .regions()
        .iter()
        .map(|region| (region.source, region.target))
        .collect();
    assert_eq!(regions, [(0, 1), (1, 0)]);
    assert_eq!(merged.regions()[0].base_from_source, BASE_01);
    assert_eq!(merged.regions()[0].size, BUDGET);
}

#[test]
fn merged_view_is_deterministic_and_tasks_are_numbered_in_name_order() {
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Zeta", 4, 1, 0, 0, "ipc_types::EncryptReq"),
        receiver("Alpha", 5, 1, 0, 0, "ipc_types::EncryptReq"),
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
    assert!(merged.regions().is_empty());
}

#[test]
fn a_project_without_a_capability_binding_allocates_unbounded() {
    // The minimal `metadata-macro` fixture binds no distribution backend, so
    // its manifests carry no capability table: the merge still allocates the
    // FIFOs deterministically and emits no region (M6.9-T3; T9 supplies the
    // mock capability table to the fixtures).
    let mut m7 = manifest("app-m7", "m7", &[0], &[1]);
    m7.capabilities = Vec::new();
    let mut m4 = m4_manifest();
    m4.capabilities = Vec::new();

    let merged = merge(TWO_APPS, &[m7, m4]).expect("no capability binding is not an error");
    assert_eq!(merged.tasks().len(), 1);
    assert_eq!(merged.tasks()[0].fifo_offset(), 0);
    assert!(merged.regions().is_empty());
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
    let extra = manifest("app-extra", "extra", &[9], &[]);
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

#[test]
fn rejects_core_count_mismatch() {
    let mut m7 = m7_manifest();
    m7.cores = 2;
    m7.core_ids = None;
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::CoreCountMismatch { package, declared: 2, mapped: 1 } if package == "app-m7"
        ),
        "{error}"
    );
}

#[test]
fn rejects_core_ids_mismatch() {
    let mut m7 = m7_manifest();
    m7.core_ids = Some(vec![7]);
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::CoreIdsMismatch { package, expected, found }
                if package == "app-m7" && expected == &[0] && found == &[7]
        ),
        "{error}"
    );
}

#[test]
fn rejects_unknown_external_core() {
    let mut m7 = m7_manifest();
    m7.external_cores = vec![9];
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::UnknownCore { core: 9, context, .. } if context == "`external_cores`"
        ),
        "{error}"
    );
}

#[test]
fn rejects_own_external_core() {
    let mut m7 = m7_manifest();
    m7.external_cores = vec![0];
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(&error, MergeError::OwnExternalCore { core: 0, .. }),
        "{error}"
    );
}

#[test]
fn rejects_receiver_core_out_of_range() {
    let mut m4 = m4_manifest();
    m4.receivers[0].core = 3;
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::ReceiverCoreOutOfRange { task, core: 3, cores: 1, .. } if task == "EncryptTask"
        ),
        "{error}"
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
}

#[test]
fn rejects_own_core_in_spawn_by() {
    let mut m4 = m4_manifest();
    m4.receivers[0].spawn_by = 1;
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(&error, MergeError::OwnSpawnerCore { core: 1, .. }),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Receiver topology
// ---------------------------------------------------------------------------

#[test]
fn rejects_duplicate_receiver() {
    let mut m7 = m7_manifest();
    m7.receivers = vec![receiver("EncryptTask", 3, 2, 0, 1, "ipc_types::EncryptReq")];
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
fn rejects_spawner_core_not_visible_to_the_receiver() {
    let mut m4 = m4_manifest();
    m4.external_cores = Vec::new();
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::SpawnerCoreNotVisible { task, core: 0, package } if task == "EncryptTask" && package == "app-m4"
        ),
        "{error}"
    );
}

#[test]
fn rejects_receiver_core_not_visible_to_the_producer() {
    let mut m7 = m7_manifest();
    m7.external_cores = Vec::new();
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::TargetCoreNotVisible { task, core: 1, package }
                if task == "EncryptTask" && package == "app-m7"
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
    m4.external_cores = vec![0, 2];
    m4.receivers = vec![
        receiver("Alpha", 3, 1, 0, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 0, 2, "ipc_types::EncryptReq"),
    ];
    let mut m5 = m5_manifest();
    m5.external_cores = vec![1];

    let error = error(THREE_APPS, &[m7_manifest(), m4, m5]);
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
        receiver("Alpha", 3, 1, 0, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 0, 0, "ipc_types::EncryptReq"),
    ];

    let merged =
        merge(TWO_APPS, &[m7_manifest(), m4]).expect("one dispatcher line drains both FIFOs");
    assert_eq!(merged.tasks().len(), 2);
    assert!(merged.tasks().iter().all(|task| task.spawner_core() == 0));
}

#[test]
fn allows_the_same_priority_on_different_targets() {
    let mut m7 = m7_manifest();
    m7.external_cores = vec![1, 2];

    let mut m4 = m4_manifest();
    m4.receivers = vec![receiver("Alpha", 3, 1, 0, 0, "ipc_types::EncryptReq")];

    let mut m5 = m5_manifest();
    m5.external_cores = vec![0];
    m5.receivers = vec![receiver("Beta", 3, 1, 0, 0, "ipc_types::EncryptReq")];

    let merged = merge(THREE_APPS, &[m7, m4, m5]).expect("different targets do not conflict");
    assert_eq!(merged.tasks().len(), 2);
}

// ---------------------------------------------------------------------------
// Pool fit
// ---------------------------------------------------------------------------

#[test]
fn rejects_a_used_direction_without_a_pool_path() {
    // The producer binds a capability table but reports no pool to physical
    // core 1, so the `0->1` direction has no IPC path.
    let mut m7 = m7_manifest();
    m7.capabilities[0].pools.retain(|pool| pool.peer != 1);

    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert_eq!(
        error.to_string(),
        "task `EncryptTask` needs a `0->1` IPC pool, but the distribution's capability binding \
         provides no pool between those cores"
    );
    assert!(matches!(
        &error,
        MergeError::MissingRegion {
            producer: 0,
            target: 1,
            ..
        }
    ));
}

#[test]
fn rejects_region_overflow() {
    // `EncryptReq` (12 bytes, capacity 2) needs 64 + 3 * 12 = 100 bytes.
    let error = error(TWO_APPS, &[m7_manifest_with_budget(99), m4_manifest()]);
    assert_eq!(
        error.to_string(),
        "task `EncryptTask` does not fit the `0->1` pool: 100 bytes needed, 99 available"
    );
    assert!(matches!(
        &error,
        MergeError::RegionOverflow {
            task,
            producer: 0,
            target: 1,
            needed: 100,
            available: 99
        } if task == "EncryptTask"
    ));
}

#[test]
fn allows_an_exact_fit() {
    let merged =
        merge(TWO_APPS, &[m7_manifest_with_budget(100), m4_manifest()]).expect("exact fit");
    assert_eq!(merged.tasks()[0].fifo_bytes(), 100);
}

#[test]
fn region_fit_accounts_for_fifo_alignment() {
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Alpha", 3, 2, 0, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 4, 2, 0, 0, "ipc_types::EncryptReq"),
    ];

    // 100 bytes each; the second FIFO starts at the next 8-byte boundary (104).
    let error = error(TWO_APPS, &[m7_manifest_with_budget(203), m4.clone()]);
    assert_eq!(
        error.to_string(),
        "task `Beta` does not fit the `0->1` pool: 204 bytes needed, 203 available"
    );
    assert!(
        matches!(
            &error,
            MergeError::RegionOverflow {
                task,
                needed: 204,
                available: 203,
                ..
            } if task == "Beta"
        ),
        "{error}"
    );

    let merged =
        merge(TWO_APPS, &[m7_manifest_with_budget(204), m4]).expect("204 bytes fit exactly");
    assert_eq!(merged.tasks().len(), 2);
    assert_eq!(merged.task("Alpha").expect("task").fifo_offset(), 0);
    assert_eq!(
        merged.task("Beta").expect("task").fifo_offset(),
        104,
        "the second FIFO is aligned to 8 bytes"
    );
}

#[test]
fn allows_pools_without_tasks() {
    let merged = merge(TWO_APPS, &[m7_manifest(), m4_manifest()]).expect("extra pool");
    assert_eq!(
        merged.regions().len(),
        2,
        "unused pool directions are still emitted"
    );
}
