//! Merge and validation tests (M1-T5).
//!
//! One positive and one negative test per validation rule: core-id
//! consistency, sender/receiver match, priority disjointness, type existence
//! and region fit. Negative cases assert the exact error variant (and, for
//! the central rules, the deterministic message).

use rticx_xbin_proto::{
    AppManifest, FIFO_HEADER, Hash64, IpcTypes, MergeError, ProjectConfig, ReceiverDecl,
    SenderDecl, TargetRef, TypeKind, merge_project, parse_idl_str, parse_project_str,
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

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"1->0" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
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

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"0->2" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
"2->1" = { base_from_source = 0x30042000, base_from_target = 0x30042000, size = 4096 }
"#;

/// One application (`app-duo`, global cores 0 and 1) plus the receiver binary.
const DUO_APPS: &str = r#"
schema = 1

[[application]]
package = "app-duo"
target = { kind = "bin", name = "duo" }
core_ids = [0, 1]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [2]

[ipc.regions]
"0->2" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"1->2" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
"#;

fn config(source: &str) -> ProjectConfig {
    parse_project_str(source).expect("valid project manifest")
}

fn idl(source: &str) -> IpcTypes {
    parse_idl_str(source).expect("valid IDL")
}

fn manifest(package: &str, target: &str, core_ids: &[u32], external_cores: &[u32]) -> AppManifest {
    AppManifest {
        schema_version: 1,
        package: package.to_string(),
        target: TargetRef::bin(target),
        source_hash: Hash64::of(package.as_bytes()),
        cores: core_ids.len() as u32,
        core_ids: Some(core_ids.to_vec()),
        external_cores: external_cores.to_vec(),
        types: Vec::new(),
        receivers: Vec::new(),
        senders: Vec::new(),
    }
}

fn receiver(
    name: &str,
    priority: u16,
    capacity: usize,
    core: u32,
    spawned_by: Option<&[u32]>,
    input: &str,
) -> ReceiverDecl {
    ReceiverDecl {
        name: name.to_string(),
        priority,
        capacity,
        core,
        spawned_by: spawned_by.map(<[u32]>::to_vec),
        input_type: input.to_string(),
    }
}

fn sender(
    name: &str,
    core: u32,
    priority: u16,
    capacity: usize,
    input: Option<&str>,
) -> SenderDecl {
    SenderDecl {
        name: name.to_string(),
        core,
        priority,
        capacity,
        input_type: input.map(str::to_string),
    }
}

/// `app-m7`: spawns `EncryptTask` on global core 1 with `EncryptReq`.
fn m7_manifest() -> AppManifest {
    let mut manifest = manifest("app-m7", "m7", &[0], &[1]);
    manifest.types = vec!["ipc_types::EncryptReq".to_string()];
    manifest.senders = vec![sender(
        "EncryptTask",
        1,
        3,
        2,
        Some("ipc_types::EncryptReq"),
    )];
    manifest
}

/// `app-m4`: executes `EncryptTask` for global core 0.
fn m4_manifest() -> AppManifest {
    let mut manifest = manifest("app-m4", "m4", &[1], &[0]);
    manifest.types = vec!["ipc_types::EncryptReq".to_string()];
    manifest.receivers = vec![receiver(
        "EncryptTask",
        3,
        2,
        0,
        Some(&[0]),
        "ipc_types::EncryptReq",
    )];
    manifest
}

/// `app-m5`: no cross-binary declarations of its own.
fn m5_manifest() -> AppManifest {
    manifest("app-m5", "m5", &[2], &[])
}

/// `app-m6`: owns global core 5, used by the `spawned_by` tests.
fn m5_manifest_core5() -> AppManifest {
    manifest("app-m6", "m6", &[5], &[])
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
fn merges_the_sender_receiver_fixture() {
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
    assert_eq!(merged.regions()[0].base_from_source, 0x3004_0000);
    assert_eq!(merged.regions()[0].size, 4096);
}

#[test]
fn merged_view_is_deterministic_and_tasks_are_numbered_in_name_order() {
    let mut m7 = m7_manifest();
    m7.senders = vec![
        sender("Zeta", 1, 4, 1, None),
        sender("Alpha", 1, 5, 1, None),
    ];
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Zeta", 4, 1, 0, Some(&[0]), "ipc_types::EncryptReq"),
        receiver("Alpha", 5, 1, 0, Some(&[0]), "ipc_types::EncryptReq"),
    ];
    let manifests = [m7, m4];

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
fn rejects_unknown_core_in_spawned_by() {
    let mut m4 = m4_manifest();
    m4.receivers[0].spawned_by = Some(vec![9]);
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::UnknownCore { core: 9, context, .. } if context.contains("spawned_by")
        ),
        "{error}"
    );
}

#[test]
fn rejects_own_core_in_spawned_by() {
    let mut m4 = m4_manifest();
    m4.receivers[0].spawned_by = Some(vec![1]);
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(&error, MergeError::OwnSpawnerCore { core: 1, .. }),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// Sender/receiver match
// ---------------------------------------------------------------------------

#[test]
fn rejects_duplicate_receiver() {
    let mut m7 = m7_manifest();
    m7.receivers = vec![receiver(
        "EncryptTask",
        3,
        2,
        0,
        None,
        "ipc_types::EncryptReq",
    )];
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
fn rejects_sender_without_receiver() {
    let mut m4 = m4_manifest();
    m4.receivers = Vec::new();
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::SenderWithoutReceiver { package, task }
                if package == "app-m7" && task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn rejects_receiver_without_sender() {
    let mut m7 = m7_manifest();
    m7.senders = Vec::new();
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::ReceiverWithoutSender { package, task }
                if package == "app-m4" && task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn rejects_target_core_mismatch() {
    let mut m7 = m7_manifest();
    m7.external_cores = vec![1, 2];
    m7.senders[0].core = 2;
    let error = error(THREE_APPS, &[m7, m4_manifest(), m5_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::TargetCoreMismatch {
                expected: 1,
                found: 2,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn rejects_target_core_not_visible() {
    let mut m7 = m7_manifest();
    m7.external_cores = Vec::new();
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::TargetCoreNotVisible { task, core: 1, .. } if task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn rejects_spawner_core_not_visible() {
    let mut m4 = m4_manifest();
    m4.external_cores = Vec::new();
    let error = error(TWO_APPS, &[m7_manifest(), m4]);
    assert!(
        matches!(
            &error,
            MergeError::SpawnerCoreNotVisible { task, core: 0, .. } if task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn rejects_self_target() {
    let mut duo = manifest("app-duo", "duo", &[0, 1], &[]);
    duo.senders = vec![sender("EncryptTask", 1, 3, 2, None)];
    let error = error(DUO_APPS, &[duo, m4_manifest_with_core(2)]);
    assert!(
        matches!(
            &error,
            MergeError::SelfTarget { package, task, core: 1 }
                if package == "app-duo" && task == "EncryptTask"
        ),
        "{error}"
    );
}

/// `app-m4` variant owning global core 2 (for the DUO_APPS topology).
fn m4_manifest_with_core(core: u32) -> AppManifest {
    let mut manifest = manifest("app-m4", "m4", &[core], &[]);
    manifest.receivers = vec![receiver(
        "EncryptTask",
        3,
        2,
        0,
        None,
        "ipc_types::EncryptReq",
    )];
    manifest
}

#[test]
fn rejects_priority_mismatch() {
    let mut m7 = m7_manifest();
    m7.senders[0].priority = 2;
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::PriorityMismatch { task, receiver: 3, sender: 2, .. } if task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn rejects_capacity_mismatch() {
    let mut m7 = m7_manifest();
    m7.senders[0].capacity = 1;
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::CapacityMismatch { task, receiver: 2, sender: 1, .. } if task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn rejects_input_type_mismatch() {
    let mut m7 = m7_manifest();
    m7.senders[0].input_type = Some("ipc_types::OtherReq".to_string());
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::InputTypeMismatch { task, receiver, sender, .. }
                if task == "EncryptTask"
                    && receiver == "ipc_types::EncryptReq"
                    && sender == "ipc_types::OtherReq"
        ),
        "{error}"
    );
}

#[test]
fn matches_input_types_by_their_idl_name() {
    let mut m7 = m7_manifest();
    m7.senders[0].input_type = Some("EncryptReq".to_string());
    let merged = merge(TWO_APPS, &[m7, m4_manifest()]).expect("same IDL name");
    assert_eq!(merged.tasks()[0].input_type(), "EncryptReq");
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

#[test]
fn rejects_unknown_sender_input_type() {
    let mut m7 = m7_manifest();
    m7.senders[0].input_type = Some("ipc_types::Missing".to_string());
    let error = error(TWO_APPS, &[m7, m4_manifest()]);
    assert!(
        matches!(
            &error,
            MergeError::UnknownInputType { package, type_name, .. }
                if package == "app-m7" && type_name == "Missing"
        ),
        "{error}"
    );
}

#[test]
fn rejects_multiple_senders_from_different_applications() {
    let mut m7 = m7_manifest();
    m7.external_cores = vec![1];
    let mut m5 = manifest("app-m5", "m5", &[2], &[1]);
    m5.senders = vec![sender("EncryptTask", 1, 3, 2, None)];
    let mut m4 = m4_manifest();
    m4.external_cores = vec![0, 2];
    m4.receivers[0].spawned_by = None;

    let error = error(THREE_APPS, &[m7, m4, m5]);
    assert!(
        matches!(
            &error,
            MergeError::MultipleSpawners { task, spawners } if task == "EncryptTask" && spawners == &[0, 2]
        ),
        "{error}"
    );
}

#[test]
fn rejects_multi_core_sender_application() {
    let mut duo = manifest("app-duo", "duo", &[0, 1], &[2]);
    duo.senders = vec![sender("EncryptTask", 2, 3, 2, None)];
    let error = error(DUO_APPS, &[duo, m4_manifest_with_core(2)]);
    assert!(
        matches!(
            &error,
            MergeError::MultipleSpawners { task, spawners } if task == "EncryptTask" && spawners == &[0, 1]
        ),
        "{error}"
    );
}

#[test]
fn rejects_spawner_not_allowed_by_receiver() {
    let mut m4 = m4_manifest();
    m4.external_cores = vec![0, 5];
    m4.receivers[0].spawned_by = Some(vec![5]);

    let error = error(SPAWNED_BY_APPS, &[m7_manifest(), m4, m5_manifest_core5()]);
    assert!(
        matches!(
            &error,
            MergeError::SpawnerNotAllowed { task, producer: 0, allowed } if task == "EncryptTask" && allowed == &[5]
        ),
        "{error}"
    );
}

/// m7 (core 0), m4 (core 1) and m6 (core 5), used for `spawned_by` tests.
const SPAWNED_BY_APPS: &str = r#"
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
package = "app-m6"
target = { kind = "bin", name = "m6" }
core_ids = [5]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"5->1" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
"#;

#[test]
fn rejects_phantom_spawner() {
    let mut m4 = m4_manifest();
    m4.external_cores = vec![0, 5];
    m4.receivers[0].spawned_by = Some(vec![0, 5]);

    let error = error(SPAWNED_BY_APPS, &[m7_manifest(), m4, m5_manifest_core5()]);
    assert!(
        matches!(
            &error,
            MergeError::PhantomSpawner { task, core: 5 } if task == "EncryptTask"
        ),
        "{error}"
    );
}

#[test]
fn allows_omitting_spawned_by() {
    let mut m4 = m4_manifest();
    m4.receivers[0].spawned_by = None;
    let merged = merge(TWO_APPS, &[m7_manifest(), m4]).expect("spawned_by is optional");
    assert_eq!(merged.tasks()[0].spawner_core(), 0);
}

#[test]
fn allows_sender_without_input_type() {
    let mut m7 = m7_manifest();
    m7.senders[0].input_type = None;
    let merged = merge(TWO_APPS, &[m7, m4_manifest()]).expect("sender input is optional");
    assert_eq!(merged.tasks().len(), 1);
}

// ---------------------------------------------------------------------------
// Priority disjointness
// ---------------------------------------------------------------------------

#[test]
fn rejects_priority_conflict_between_sources() {
    let mut m7 = m7_manifest();
    m7.senders = vec![sender("Alpha", 1, 3, 1, None)];
    let mut m5 = manifest("app-m5", "m5", &[2], &[1]);
    m5.senders = vec![sender("Beta", 1, 3, 1, None)];
    let mut m4 = m4_manifest();
    m4.external_cores = vec![0, 2];
    m4.receivers = vec![
        receiver("Alpha", 3, 1, 0, Some(&[0]), "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 0, Some(&[2]), "ipc_types::EncryptReq"),
    ];

    let error = error(THREE_APPS, &[m7, m4, m5]);
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
    let mut m7 = m7_manifest();
    m7.senders = vec![
        sender("Alpha", 1, 3, 1, None),
        sender("Beta", 1, 3, 1, None),
    ];
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Alpha", 3, 1, 0, Some(&[0]), "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 0, Some(&[0]), "ipc_types::EncryptReq"),
    ];

    let merged = merge(TWO_APPS, &[m7, m4]).expect("one dispatcher line drains both FIFOs");
    assert_eq!(merged.tasks().len(), 2);
    assert!(merged.tasks().iter().all(|task| task.spawner_core() == 0));
}

#[test]
fn allows_the_same_priority_on_different_targets() {
    let mut m7 = m7_manifest();
    m7.external_cores = vec![1, 2];
    m7.senders = vec![
        sender("Alpha", 1, 3, 1, None),
        sender("Beta", 2, 3, 1, None),
    ];
    let mut m4 = m4_manifest();
    m4.receivers = vec![receiver(
        "Alpha",
        3,
        1,
        0,
        Some(&[0]),
        "ipc_types::EncryptReq",
    )];
    let mut m5 = m5_manifest();
    m5.external_cores = vec![0];
    m5.receivers = vec![receiver(
        "Beta",
        3,
        1,
        0,
        Some(&[0]),
        "ipc_types::EncryptReq",
    )];

    let merged = merge(THREE_APPS, &[m7, m4, m5]).expect("different targets do not conflict");
    assert_eq!(merged.tasks().len(), 2);
}

// ---------------------------------------------------------------------------
// Region fit
// ---------------------------------------------------------------------------

#[test]
fn rejects_a_missing_region() {
    let error = error(NO_REGIONS, &[m7_manifest(), m4_manifest()]);
    assert_eq!(
        error.to_string(),
        "task `EncryptTask` needs a `0->1` region, but rticx.toml declares none under \
         `[ipc.regions]`"
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

/// Two applications without any region declarations.
const NO_REGIONS: &str = r#"
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

#[test]
fn rejects_region_overflow() {
    let error = error(TINY_REGION, &[m7_manifest(), m4_manifest()]);
    assert_eq!(
        error.to_string(),
        "task `EncryptTask` does not fit the `0->1` region: 100 bytes needed, 99 available"
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

/// `EncryptReq` (12 bytes, capacity 2) needs 64 + 3 * 12 = 100 bytes.
const TINY_REGION: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 99 }
"#;

#[test]
fn allows_an_exact_fit() {
    let merged = merge(EXACT_REGION, &[m7_manifest(), m4_manifest()]).expect("exact fit");
    assert_eq!(merged.tasks()[0].fifo_bytes(), 100);
}

const EXACT_REGION: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 100 }
"#;

#[test]
fn region_fit_accounts_for_fifo_alignment() {
    let mut m7 = m7_manifest();
    m7.senders = vec![
        sender("Alpha", 1, 3, 2, None),
        sender("Beta", 1, 4, 2, None),
    ];
    let mut m4 = m4_manifest();
    m4.receivers = vec![
        receiver("Alpha", 3, 2, 0, Some(&[0]), "ipc_types::EncryptReq"),
        receiver("Beta", 4, 2, 0, Some(&[0]), "ipc_types::EncryptReq"),
    ];
    let manifests = [m7, m4];

    // 100 bytes each; the second FIFO starts at the next 8-byte boundary (104).
    let error = error(ODD_REGION, &manifests);
    assert_eq!(
        error.to_string(),
        "task `Beta` does not fit the `0->1` region: 204 bytes needed, 203 available"
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

    let merged = merge(ALIGNED_REGION, &manifests).expect("204 bytes fit exactly");
    assert_eq!(merged.tasks().len(), 2);
    assert_eq!(merged.task("Alpha").expect("task").fifo_offset(), 0);
    assert_eq!(
        merged.task("Beta").expect("task").fifo_offset(),
        104,
        "the second FIFO is aligned to 8 bytes"
    );
}

const ODD_REGION: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 203 }
"#;

const ALIGNED_REGION: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 204 }
"#;

#[test]
fn allows_regions_without_tasks() {
    let merged = merge(TWO_APPS, &[m7_manifest(), m4_manifest()]).expect("extra region");
    assert_eq!(
        merged.regions().len(),
        2,
        "unused regions are still emitted"
    );
}
