//! `<target>.xbin.json` schema tests (M6.9-T2).
//!
//! Schema 2 adds the distro capability binding ([`CoreCapability`]): one entry
//! per local core with its physical core id and the IPC pools that physical
//! core can reach. These tests pin the JSON shape, the deterministic
//! serialization (empty capabilities are omitted) and the schema-version gate.

use rticx_xbin_proto::{
    AppManifest, CoreCapability, Hash64, MANIFEST_SCHEMA_VERSION, PoolCachePolicy, PoolDecl,
    TargetRef,
};

fn manifest() -> AppManifest {
    AppManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        package: "app-m4".to_string(),
        target: TargetRef::bin("m4"),
        source_hash: Hash64::of(b"app-m4"),
        cores: 2,
        core_ids: Some(vec![7, 9]),
        external_cores: vec![0],
        types: Vec::new(),
        receivers: Vec::new(),
        capabilities: vec![
            CoreCapability {
                local_core: 0,
                physical_core: 7,
                pools: vec![PoolDecl {
                    id: "sram3".to_string(),
                    peer: 9,
                    base_local: 0x3004_0000,
                    base_peer: 0x1004_0000,
                    budget: 4096,
                    policy: PoolCachePolicy::NormalNonCacheableShareable,
                }],
            },
            CoreCapability {
                local_core: 1,
                physical_core: 9,
                pools: Vec::new(),
            },
        ],
    }
}

#[test]
fn capabilities_round_trip() {
    let manifest = manifest();
    let json = manifest.to_json();

    assert_eq!(MANIFEST_SCHEMA_VERSION, 2);
    assert!(json.contains("\"schema_version\": 2"), "{json}");
    assert!(json.contains("\"physical_core\": 7"), "{json}");
    assert!(json.contains("\"peer\": 9"), "{json}");
    assert!(json.contains("\"base_local\": 805568512"), "{json}");
    assert!(
        json.contains("\"policy\": \"normal_non_cacheable_shareable\""),
        "{json}"
    );

    assert_eq!(AppManifest::from_json(&json).expect("round trip"), manifest);
}

#[test]
fn empty_capabilities_are_omitted() {
    let mut manifest = manifest();
    manifest.capabilities.clear();

    let json = manifest.to_json();
    assert!(!json.contains("\"capabilities\""), "{json}");
    assert!(
        AppManifest::from_json(&json)
            .expect("round trip")
            .capabilities
            .is_empty()
    );
}

#[test]
fn schema_version_one_is_rejected() {
    let mut manifest = manifest();
    manifest.schema_version = 1;

    let error = AppManifest::from_json(&manifest.to_json()).expect_err("v1 is not schema 2");
    assert!(error.to_string().contains("schema version 1"), "{error}");
}
