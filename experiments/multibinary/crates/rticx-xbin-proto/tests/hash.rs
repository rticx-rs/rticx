//! Tests for the deterministic [`Hash64`] value type.

use std::str::FromStr;

use rticx_xbin_proto::Hash64;

#[test]
fn fnv1a_known_vectors() {
    // FNV-1a 64-bit reference vectors.
    assert_eq!(Hash64::of(b"").to_hex(), "0xcbf29ce484222325");
    assert_eq!(Hash64::of(b"a").to_hex(), "0xaf63dc4c8601ec8c");
    assert_eq!(Hash64::of(b"foobar").to_hex(), "0x85944171f73967e8");
}

#[test]
fn hex_round_trip() {
    let hash = Hash64::of(b"canonical layout");
    let text = hash.to_hex();
    assert_eq!(Hash64::from_hex(&text).expect("canonical form"), hash);
    assert_eq!(Hash64::from_str(&text).expect("FromStr"), hash);
    // Case-insensitive hex digits and an optional prefix are accepted.
    let bare = text.trim_start_matches("0x").to_uppercase();
    assert_eq!(Hash64::from_hex(&bare).expect("bare digits"), hash);
}

#[test]
fn malformed_hashes_are_rejected() {
    for text in ["", "0x", "zz", "0x10000000000000000", "0x1234567890abcdef0"] {
        let error = Hash64::from_hex(text).expect_err("must be rejected");
        assert!(
            error.to_string().contains(text),
            "unexpected message: {error}"
        );
    }
}

#[test]
fn serde_uses_the_canonical_hex_string() {
    let hash = Hash64::of(b"layout");
    let json = serde_json::to_string(&hash).expect("serialize");
    assert_eq!(json, format!("\"{}\"", hash.to_hex()));
    let parsed: Hash64 = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, hash);

    let error = serde_json::from_str::<Hash64>("\"not-hex\"")
        .expect_err("malformed hash must fail to deserialize");
    assert!(error.to_string().contains("invalid hash"), "{error}");
}
