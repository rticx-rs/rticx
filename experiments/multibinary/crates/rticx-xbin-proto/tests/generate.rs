//! Golden and determinism tests for the `ipc_types` module generator
//! (M0-T4).
//!
//! The golden fixture under `tests/fixtures/` is byte-for-byte compared with
//! the generator output; it documents exactly what `cargo xbin sync` writes
//! (the fixture is regenerated with the `dump` helper when the format
//! intentionally changes).

use rticx_xbin_proto::{
    FileChange, canonical_layout_text, generate_module, layout_hash, parse_idl_str,
    write_if_changed,
};

const GOLDEN_IDL: &str = r#"
schema = 1

[message.EncryptReq]
fields = { addr = "u32", len = "u32", key = "u32" }

[message.Point]
fields = { x = "i16", y = "i16", tag = "Kind" }

[enum.Kind]
variants = { Fast = 0, Slow = 1 }
"#;

fn golden_idl() -> rticx_xbin_proto::IpcTypes {
    parse_idl_str(GOLDEN_IDL).expect("valid golden IDL")
}

#[test]
fn generated_module_matches_golden_fixture() {
    let generated = generate_module(&golden_idl()).expect("codegen");
    assert_eq!(generated, include_str!("fixtures/golden_ipc_types.rs"));
}

#[test]
fn generation_is_deterministic() {
    let first = generate_module(&golden_idl()).expect("codegen");
    let second = generate_module(&golden_idl()).expect("codegen");
    assert_eq!(first, second);
}

#[test]
fn module_has_no_inner_attributes_or_rt_dependency() {
    let generated = generate_module(&golden_idl()).expect("codegen");
    assert!(
        !generated.contains("#!["),
        "the module body must not carry inner attributes"
    );
    assert!(
        !generated.contains("rticx_xbin_rt") && !generated.contains("rticx-xbin-rt"),
        "the module body must not reference the runtime crate"
    );
    assert!(
        generated.contains("unsafe impl CrossCoreMessage for EncryptReq {}"),
        "marker impls must use the bare trait name"
    );
}

#[test]
fn layout_hash_is_stable() {
    // Locked so accidental changes to the canonical text are caught.
    assert_eq!(
        layout_hash(&golden_idl()).expect("hash"),
        0x3520_dde9_389d_1891
    );
}

#[test]
fn canonical_layout_text_contains_offsets_and_discriminants() {
    let text = canonical_layout_text(&golden_idl()).expect("canonical text");
    let expected = "\
ipc-types layout v1
schema=1
message EncryptReq align=4 size=12
field EncryptReq.addr type=u32 offset=0 size=4 align=4
field EncryptReq.len type=u32 offset=4 size=4 align=4
field EncryptReq.key type=u32 offset=8 size=4 align=4
message Point align=4 size=8
field Point.x type=i16 offset=0 size=2 align=2
field Point.y type=i16 offset=2 size=2 align=2
field Point.tag type=Kind offset=4 size=4 align=4
enum Kind align=4 size=4
variant Kind::Fast=0
variant Kind::Slow=1
";
    assert_eq!(text, expected);
}

#[test]
fn write_if_changed_creates_then_is_a_noop() {
    let contents = generate_module(&golden_idl()).expect("codegen");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("target/rticx-xbin/ipc_types.rs");

    assert_eq!(
        write_if_changed(&path, &contents).expect("write"),
        FileChange::Created
    );
    assert_eq!(std::fs::read_to_string(&path).expect("read"), contents);

    assert_eq!(
        write_if_changed(&path, &contents).expect("write"),
        FileChange::Unchanged
    );
    assert_eq!(std::fs::read_to_string(&path).expect("read"), contents);
}

#[test]
fn write_if_changed_restores_a_tampered_file() {
    let contents = generate_module(&golden_idl()).expect("codegen");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ipc_types.rs");
    write_if_changed(&path, &contents).expect("write");

    std::fs::write(&path, "// stale\n").expect("tamper");

    assert_eq!(
        write_if_changed(&path, &contents).expect("write"),
        FileChange::Updated
    );
    assert_eq!(std::fs::read_to_string(&path).expect("read"), contents);
}

#[test]
fn constant_name_collisions_are_rejected() {
    let idl = parse_idl_str(
        "schema = 1\n\
         [message.FooBar]\nfields = { x = \"u8\" }\n\
         [message.Foo_Bar]\nfields = { y = \"u8\" }\n",
    )
    .expect("valid IDL");
    let error = generate_module(&idl)
        .expect_err("colliding constant names")
        .to_string();
    assert_eq!(
        error,
        "message `FooBar` and message `Foo_Bar` would both generate the constant \
         `SIZE_FOO_BAR`; rename one of them"
    );

    let idl = parse_idl_str(
        "schema = 1\n\
         [message.Foo]\nfields = { xY = \"u8\", x_y = \"u8\" }\n",
    )
    .expect("valid IDL");
    let error = generate_module(&idl)
        .expect_err("colliding field constant names")
        .to_string();
    assert_eq!(
        error,
        "field `Foo.xY` and field `Foo.x_y` would both generate the constant \
         `OFF_FOO_X_Y`; rename one of them"
    );
}
