//! `<target>.xbin.json` schema tests.
//!
//! The schema keeps the distro capability binding ([`CoreCapability`]): the
//! single physical core the binary runs on and the IPC pools that physical core
//! can reach. These tests pin the JSON shape, the deterministic serialization
//! (an absent capability is omitted) and the schema-version gate.

use rticx_xbin_proto::{
    AppManifest, CoreCapability, Hash64, MANIFEST_SCHEMA_VERSION, PoolDecl, ReceiverDecl, TargetRef,
};

fn manifest() -> AppManifest {
    AppManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        package: "app-m4".to_string(),
        target: TargetRef::bin("m4"),
        interface_hash: Hash64::of(b"app-m4"),
        types: Vec::new(),
        receivers: Vec::new(),
        capability: Some(CoreCapability {
            physical_core: 7,
            pools: vec![PoolDecl {
                id: "sram3".to_string(),
                peer: 9,
                base_local: 0x3004_0000,
                base_peer: 0x1004_0000,
                budget: 4096,
            }],
        }),
    }
}

#[test]
fn capability_round_trips() {
    let manifest = manifest();
    let json = manifest.to_json();

    assert!(json.contains("\"physical_core\": 7"), "{json}");
    assert!(
        !json.contains("\"local_core\""),
        "one core per binary needs no local index: {json}"
    );
    assert!(json.contains("\"peer\": 9"), "{json}");
    assert!(json.contains("\"base_local\": 805568512"), "{json}");
    assert!(
        !json.contains("\"policy\""),
        "schema 5 dropped the redundant per-pool policy: {json}"
    );

    assert_eq!(AppManifest::from_json(&json).expect("round trip"), manifest);
}

#[test]
fn schema_has_one_capability_per_binary() {
    assert_eq!(MANIFEST_SCHEMA_VERSION, 1);

    let json = manifest().to_json();
    assert!(json.contains("\"schema_version\": 1"), "{json}");
    assert!(json.contains("\"interface_hash\": \"0x"), "{json}");
    assert!(
        json.contains("\"capability\""),
        "the capability is singular: {json}"
    );
}

#[test]
fn receivers_have_no_core_field() {
    let mut manifest = manifest();
    manifest.receivers = vec![ReceiverDecl {
        name: "EncryptTask".to_string(),
        priority: 3,
        capacity: 2,
        spawn_by: 0,
        input_type: "ipc_types::EncryptReq".to_string(),
    }];

    let json = manifest.to_json();
    assert!(
        !json.contains("\"core\""),
        "a receiver is always core-local: {json}"
    );
    assert_eq!(AppManifest::from_json(&json).expect("round trip"), manifest);
}

#[test]
fn absent_capability_is_omitted() {
    let mut manifest = manifest();
    manifest.capability = None;

    let json = manifest.to_json();
    assert!(!json.contains("\"capability\""), "{json}");
    assert!(
        AppManifest::from_json(&json)
            .expect("round trip")
            .capability
            .is_none()
    );
}
