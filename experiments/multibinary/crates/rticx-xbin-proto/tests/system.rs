//! Golden, round-trip, determinism and hashing tests for the `system.json`
//! schema (M1-T1).

use rticx_xbin_proto::{
    AppEntry, CoreEntry, DoorbellEntry, FieldEntry, FifoEntry, Hash64, RegionEntry, SystemError,
    SystemView, TargetRef, TaskEntry, TypeEntry, TypeKind, VariantEntry,
};

/// A hand-written system view exercising every part of the schema.
fn sample_view() -> SystemView {
    let mut view = SystemView::empty("0.2");
    view.layout_hash = Hash64::of(b"ipc-types layout v1");
    view.apps = vec![
        AppEntry {
            package: "app-m7".to_string(),
            target: TargetRef::bin("m7"),
            source_hash: Hash64::of(b"app-m7 source"),
            core_ids: vec![0],
            external_cores: vec![1],
        },
        AppEntry {
            package: "app-m4".to_string(),
            target: TargetRef::bin("m4"),
            source_hash: Hash64::of(b"app-m4 source"),
            core_ids: vec![1],
            external_cores: vec![0],
        },
    ];
    view.cores = vec![
        CoreEntry {
            global_id: 0,
            app: "app-m7".to_string(),
            local_index: 0,
        },
        CoreEntry {
            global_id: 1,
            app: "app-m4".to_string(),
            local_index: 0,
        },
    ];
    view.types = vec![
        TypeEntry {
            name: "EncryptReq".to_string(),
            kind: TypeKind::Message,
            size: 12,
            align: 4,
            fields: vec![
                FieldEntry {
                    name: "addr".to_string(),
                    ty: "u32".to_string(),
                    offset: 0,
                },
                FieldEntry {
                    name: "len".to_string(),
                    ty: "u32".to_string(),
                    offset: 4,
                },
                FieldEntry {
                    name: "key".to_string(),
                    ty: "u32".to_string(),
                    offset: 8,
                },
            ],
            variants: Vec::new(),
        },
        TypeEntry {
            name: "State".to_string(),
            kind: TypeKind::Enum,
            size: 4,
            align: 4,
            fields: Vec::new(),
            variants: vec![
                VariantEntry {
                    name: "Idle".to_string(),
                    discriminant: 0,
                },
                VariantEntry {
                    name: "Busy".to_string(),
                    discriminant: 7,
                },
            ],
        },
    ];
    view.tasks = vec![TaskEntry {
        id: 1,
        name: "EncryptTask".to_string(),
        receiver_core: 1,
        spawner_core: 0,
        priority: 3,
        capacity: 2,
        input_type: "EncryptReq".to_string(),
        fifo: FifoEntry {
            source: 0,
            target: 1,
            offset: 0,
            elem_size: 12,
            depth: 3,
        },
    }];
    view.regions = vec![RegionEntry {
        source: 0,
        target: 1,
        base_from_source: 0x3004_0000,
        base_from_target: 0x3004_0000,
        size: 4096,
    }];
    view.doorbells = vec![DoorbellEntry {
        source: 0,
        target: 1,
        priority: 3,
        line: 0,
    }];
    view.seal();
    view
}

#[test]
fn serialization_round_trips() {
    let view = sample_view();
    let json = view.to_json();
    let parsed = SystemView::from_json(&json).expect("round-trip");
    assert_eq!(parsed, view);
    assert_eq!(parsed.to_json(), json);
}

#[test]
fn serialization_is_deterministic() {
    let first = sample_view().to_json();
    let second = sample_view().to_json();
    assert_eq!(
        first, second,
        "identical inputs must produce identical bytes"
    );

    // Re-serializing a parsed view must not change a single byte.
    let reparsed = SystemView::from_json(&first).expect("parse");
    assert_eq!(reparsed.to_json(), first);
}

#[test]
fn empty_view_round_trips() {
    let mut view = SystemView::empty("0.2");
    view.seal();
    let json = view.to_json();
    assert_eq!(SystemView::from_json(&json).expect("parse"), view);
    assert!(view.verify_topology_hash());
}

#[test]
fn topology_hash_covers_every_other_field() {
    let view = sample_view();
    assert!(view.verify_topology_hash());
    assert_eq!(view.topology_hash, view.compute_topology_hash());

    let mut edited = view.clone();
    edited.tasks[0].name = "OtherTask".to_string();
    assert!(!edited.verify_topology_hash());
    assert_ne!(edited.compute_topology_hash(), view.topology_hash);

    // The hash field itself is excluded, so re-sealing is idempotent.
    edited.seal();
    assert!(edited.verify_topology_hash());
    assert_eq!(edited.topology_hash, edited.compute_topology_hash());
}

#[test]
fn canonical_text_is_stable_and_order_sensitive() {
    let view = sample_view();
    assert_eq!(
        view.canonical_topology_text(),
        sample_view().canonical_topology_text()
    );

    let mut edited = view.clone();
    edited.cores.swap(0, 1);
    assert_ne!(
        edited.canonical_topology_text(),
        view.canonical_topology_text()
    );
}

#[test]
fn addresses_and_hashes_are_hex_strings_in_json() {
    let json = sample_view().to_json();
    let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");

    assert_eq!(value["regions"][0]["base_from_source"], "0x30040000");
    assert_eq!(value["regions"][0]["base_from_target"], "0x30040000");
    assert_eq!(value["tasks"][0]["fifo"]["offset"], 0);

    let topology = value["topology_hash"].as_str().expect("hash is a string");
    assert!(topology.starts_with("0x"), "{topology}");
    assert_eq!(topology.len(), 18, "{topology}");
    assert!(
        value["layout_hash"]
            .as_str()
            .expect("hash is a string")
            .starts_with("0x")
    );
}

#[test]
fn unknown_schema_version_is_rejected() {
    let mut value: serde_json::Value =
        serde_json::from_str(&sample_view().to_json()).expect("valid JSON");
    value["schema_version"] = serde_json::Value::from(2u32);

    let error = SystemView::from_json(&value.to_string()).expect_err("schema 2 is rejected");
    assert!(
        matches!(
            error,
            SystemError::Schema {
                found: 2,
                expected: 1
            }
        ),
        "unexpected error: {error}"
    );
    assert_eq!(
        error.to_string(),
        "unsupported system.json schema version 2; this tool supports schema = 1"
    );
}
