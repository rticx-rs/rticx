//! `system.json` emission tests (M1-T6, M5.5).
//!
//! [`system_view`] converts a merged, allocated project into the view the
//! driver writes to `target/rticx-xbin/system.json`: FIFO offsets, doorbell
//! lines, `layout_hash` and the sealed `topology_hash`. The tests below cover
//! the reference topology, deterministic allocation, per-direction offset
//! independence, doorbell line assignment and byte-identical output across
//! runs.

use rticx_xbin_proto::{
    AppManifest, DoorbellEntry, FifoEntry, Hash64, IpcTypes, ProjectConfig, ReceiverDecl,
    SystemView, TargetRef, layout_hash, merge_project, parse_idl_str, parse_project_str,
    system_view,
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
"1->0" = { base_from_source = 0x30043000, base_from_target = 0x30043000, size = 4096 }
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

/// `app-m7`: the producer of global core 0, no declarations of its own.
fn m7_manifest() -> AppManifest {
    manifest("app-m7", "m7", &[0], &[1])
}

/// `app-m4`: executes `EncryptTask` for global core 0.
fn m4_manifest() -> AppManifest {
    let mut manifest = manifest("app-m4", "m4", &[1], &[0]);
    manifest.receivers = vec![receiver("EncryptTask", 3, 2, 0, 0, "ipc_types::EncryptReq")];
    manifest
}

fn view(project: &str, manifests: &[AppManifest]) -> SystemView {
    let idl = idl(IDL);
    let merged = merge_project(&config(project), manifests, &idl).expect("valid project");
    system_view(&merged, &idl, rticx_xbin_proto::RTICX_GENERATION).expect("system view")
}

fn task<'a>(view: &'a SystemView, name: &str) -> &'a rticx_xbin_proto::TaskEntry {
    view.tasks
        .iter()
        .find(|task| task.name == name)
        .expect("task in view")
}

// ---------------------------------------------------------------------------
// Reference topology
// ---------------------------------------------------------------------------

#[test]
fn emits_the_reference_system_view() {
    let idl = idl(IDL);
    let merged = merge_project(&config(TWO_APPS), &[m7_manifest(), m4_manifest()], &idl)
        .expect("valid project");
    let view = system_view(&merged, &idl, rticx_xbin_proto::RTICX_GENERATION)
        .expect("layout hash is computable");

    assert_eq!(view.schema_version, 1);
    assert_eq!(view.rticx_generation, "0.2");
    assert_eq!(
        view.layout_hash,
        Hash64::new(layout_hash(&idl).expect("layout hash"))
    );
    assert!(view.verify_topology_hash(), "the emitted view is sealed");

    assert_eq!(
        view.apps
            .iter()
            .map(|app| app.package.as_str())
            .collect::<Vec<_>>(),
        ["app-m7", "app-m4"]
    );
    assert_eq!(view.cores.len(), 2);
    assert_eq!(
        view.types
            .iter()
            .map(|ty| ty.name.as_str())
            .collect::<Vec<_>>(),
        ["EncryptReq", "OtherReq", "State"]
    );

    assert_eq!(view.tasks.len(), 1);
    let encrypt = task(&view, "EncryptTask");
    assert_eq!(encrypt.id, 1);
    assert_eq!(encrypt.receiver_core, 1);
    assert_eq!(encrypt.spawner_core, 0);
    assert_eq!(encrypt.priority, 3);
    assert_eq!(encrypt.capacity, 2);
    assert_eq!(encrypt.input_type, "EncryptReq");
    assert_eq!(
        encrypt.fifo,
        FifoEntry {
            source: 0,
            target: 1,
            offset: 0,
            elem_size: 12,
            depth: 3,
        }
    );

    assert_eq!(view.regions.len(), 2, "all declared regions are emitted");
    assert_eq!(view.regions[0].source, 0);
    assert_eq!(view.regions[0].target, 1);
    assert_eq!(view.regions[0].base_from_source, 0x3004_0000);

    assert_eq!(
        view.doorbells,
        [DoorbellEntry {
            source: 0,
            target: 1,
            priority: 3,
            line: 0,
        }]
    );

    let json = view.to_json();
    assert_eq!(SystemView::from_json(&json).expect("round-trip"), view);
    assert!(json.contains("\"spawner_core\": 0"), "{json}");
}

#[test]
fn emits_an_empty_view_for_an_empty_project() {
    let idl = IpcTypes::empty();
    let merged = merge_project(&config("schema = 1\n"), &[], &idl).expect("empty project");
    let view = system_view(&merged, &idl, rticx_xbin_proto::RTICX_GENERATION).expect("view");

    assert!(view.apps.is_empty());
    assert!(view.tasks.is_empty());
    assert!(view.doorbells.is_empty());
    assert_eq!(view.layout_hash, Hash64::new(layout_hash(&idl).unwrap()));
    assert!(view.verify_topology_hash());
}

#[test]
fn system_json_is_byte_identical_across_runs() {
    let first = view(TWO_APPS, &[m7_manifest(), m4_manifest()]).to_json();
    let second = view(TWO_APPS, &[m7_manifest(), m4_manifest()]).to_json();
    assert_eq!(first, second, "identical inputs produce identical bytes");

    let reparsed = SystemView::from_json(&first).expect("parse");
    assert_eq!(reparsed.to_json(), first, "re-serializing is byte-stable");
}

// ---------------------------------------------------------------------------
// FIFO allocation
// ---------------------------------------------------------------------------

#[test]
fn allocates_offsets_in_name_order_within_a_direction() {
    // Declared Zeta first, but allocation (and ids) follow name order.
    let mut m4 = manifest("app-m4", "m4", &[1], &[0]);
    m4.receivers = vec![
        receiver("Zeta", 4, 2, 0, 0, "ipc_types::EncryptReq"),
        receiver("Alpha", 3, 2, 0, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 5, 2, 0, 0, "ipc_types::EncryptReq"),
    ];

    let view = view(TWO_APPS, &[m7_manifest(), m4]);

    assert_eq!(
        view.tasks
            .iter()
            .map(|task| (task.name.as_str(), task.id))
            .collect::<Vec<_>>(),
        [("Alpha", 1), ("Beta", 2), ("Zeta", 3)]
    );
    // Each FIFO is 64 + 3 * 12 = 100 bytes; the next start is aligned up.
    assert_eq!(task(&view, "Alpha").fifo.offset, 0);
    assert_eq!(task(&view, "Beta").fifo.offset, 104);
    assert_eq!(task(&view, "Zeta").fifo.offset, 208);
}

#[test]
fn offsets_are_independent_per_direction() {
    // m7 -> m4 (`EncryptTask`) and m4 -> m7 (`BackTask`).
    let mut m7 = manifest("app-m7", "m7", &[0], &[1]);
    m7.receivers = vec![receiver("BackTask", 2, 1, 0, 1, "ipc_types::OtherReq")];
    let m4 = m4_manifest();

    let view = view(TWO_APPS, &[m7, m4]);

    let encrypt = task(&view, "EncryptTask");
    assert_eq!(
        (
            encrypt.fifo.source,
            encrypt.fifo.target,
            encrypt.fifo.offset
        ),
        (0, 1, 0)
    );
    let back = task(&view, "BackTask");
    assert_eq!(
        (back.fifo.source, back.fifo.target, back.fifo.offset),
        (1, 0, 0),
        "each direction allocates inside its own region"
    );
}

// ---------------------------------------------------------------------------
// Doorbell lines
// ---------------------------------------------------------------------------

#[test]
fn doorbell_lines_are_unique_per_target_and_shared_within_a_line() {
    let mut m5 = manifest("app-m5", "m5", &[2], &[1]);
    m5.receivers = Vec::new();
    let mut m4 = manifest("app-m4", "m4", &[1], &[0, 2]);
    m4.receivers = vec![
        receiver("Alpha", 3, 1, 0, 0, "ipc_types::EncryptReq"),
        receiver("Beta", 3, 1, 0, 0, "ipc_types::EncryptReq"),
        receiver("Gamma", 5, 1, 0, 0, "ipc_types::EncryptReq"),
        receiver("Delta", 4, 1, 0, 2, "ipc_types::EncryptReq"),
    ];

    let view = view(THREE_APPS, &[m7_manifest(), m4, m5]);

    // Target core 1 has lines (0,3) -> 0, (0,5) -> 1 and (2,4) -> 2; Alpha
    // and Beta share one line.
    assert_eq!(
        view.doorbells,
        [
            DoorbellEntry {
                source: 0,
                target: 1,
                priority: 3,
                line: 0,
            },
            DoorbellEntry {
                source: 0,
                target: 1,
                priority: 5,
                line: 1,
            },
            DoorbellEntry {
                source: 2,
                target: 1,
                priority: 4,
                line: 2,
            },
        ]
    );
}

#[test]
fn doorbell_lines_are_independent_between_targets() {
    let mut m7 = manifest("app-m7", "m7", &[0], &[1]);
    m7.receivers = vec![receiver("BackTask", 2, 1, 0, 1, "ipc_types::OtherReq")];
    let m4 = m4_manifest();

    let view = view(TWO_APPS, &[m7, m4]);

    assert_eq!(
        view.doorbells,
        [
            DoorbellEntry {
                source: 0,
                target: 1,
                priority: 3,
                line: 0,
            },
            DoorbellEntry {
                source: 1,
                target: 0,
                priority: 2,
                line: 0,
            },
        ],
        "each target core numbers its lines from zero"
    );
}
