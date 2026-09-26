//! Golden and determinism tests for the `ipc-types` crate generator
//! (M0-T4).
//!
//! The golden fixture pair under `tests/fixtures/` is byte-for-byte compared
//! with the generator output; it documents exactly what `cargo xbin sync`
//! writes (the fixture is regenerated with the `dump` helper when the format
//! intentionally changes).

use rticx_xbin_proto::{
    CodegenOptions, FileChange, RtDependency, canonical_layout_text, generate_crate, layout_hash,
    parse_idl_str,
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
fn generated_crate_matches_golden_fixture() {
    let generated = generate_crate(&golden_idl(), &CodegenOptions::default()).expect("codegen");
    assert_eq!(
        generated.file("src/lib.rs"),
        Some(include_str!("fixtures/golden_lib.rs"))
    );
    assert_eq!(
        generated.file("Cargo.toml"),
        Some(include_str!("fixtures/golden_cargo.toml"))
    );

    let paths: Vec<&str> = generated
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, vec!["Cargo.toml", "src/lib.rs"]);
}

#[test]
fn generation_is_deterministic() {
    let first = generate_crate(&golden_idl(), &CodegenOptions::default()).expect("codegen");
    let second = generate_crate(&golden_idl(), &CodegenOptions::default()).expect("codegen");
    assert_eq!(first, second);
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
fn rt_dependency_path_and_version_forms() {
    let path_options = CodegenOptions {
        rt_dependency: RtDependency::Path("vendor/rticx-xbin-rt".to_string()),
    };
    let generated = generate_crate(&golden_idl(), &path_options).expect("codegen");
    assert!(
        generated
            .file("Cargo.toml")
            .expect("manifest")
            .contains("rticx-xbin-rt = { path = \"vendor/rticx-xbin-rt\" }")
    );

    let version_options = CodegenOptions {
        rt_dependency: RtDependency::Version("0.1".to_string()),
    };
    let generated = generate_crate(&golden_idl(), &version_options).expect("codegen");
    assert!(
        generated
            .file("Cargo.toml")
            .expect("manifest")
            .contains("rticx-xbin-rt = \"0.1\"")
    );
}

#[test]
fn write_to_creates_the_crate_tree() {
    let generated = generate_crate(&golden_idl(), &CodegenOptions::default()).expect("codegen");
    let dir = tempfile::tempdir().expect("tempdir");
    generated.write_to(dir.path()).expect("write");
    assert!(dir.path().join("Cargo.toml").is_file());
    assert!(dir.path().join("src/lib.rs").is_file());
}

#[test]
fn write_if_changed_creates_then_is_a_noop() {
    let generated = generate_crate(&golden_idl(), &CodegenOptions::default()).expect("codegen");
    let dir = tempfile::tempdir().expect("tempdir");

    let first = generated.write_if_changed(dir.path()).expect("write");
    assert!(first.changed());
    assert!(!first.is_noop());
    assert_eq!(first.files.len(), 2);
    assert!(
        first
            .files
            .iter()
            .all(|file| file.change == FileChange::Created),
        "{first:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).expect("lib.rs"),
        generated.file("src/lib.rs").expect("generated lib.rs")
    );

    let second = generated.write_if_changed(dir.path()).expect("write");
    assert!(second.is_noop(), "{second:?}");
    assert!(
        second
            .files
            .iter()
            .all(|file| file.change == FileChange::Unchanged),
        "{second:?}"
    );
}

#[test]
fn write_if_changed_restores_tampered_files_only() {
    let generated = generate_crate(&golden_idl(), &CodegenOptions::default()).expect("codegen");
    let dir = tempfile::tempdir().expect("tempdir");
    generated.write_if_changed(dir.path()).expect("write");

    std::fs::write(dir.path().join("src/lib.rs"), "// stale\n").expect("tamper");

    let status = generated.write_if_changed(dir.path()).expect("write");
    assert!(status.changed());
    assert_eq!(
        status
            .changed_files()
            .map(|file| (file.path.as_str(), file.change))
            .collect::<Vec<_>>(),
        [("src/lib.rs", FileChange::Updated)]
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).expect("lib.rs"),
        generated.file("src/lib.rs").expect("generated lib.rs")
    );

    let cargo_toml = status
        .files
        .iter()
        .find(|file| file.path == "Cargo.toml")
        .expect("Cargo.toml status");
    assert_eq!(cargo_toml.change, FileChange::Unchanged);
}

#[test]
fn constant_name_collisions_are_rejected() {
    let idl = parse_idl_str(
        "schema = 1\n\
         [message.FooBar]\nfields = { x = \"u8\" }\n\
         [message.Foo_Bar]\nfields = { y = \"u8\" }\n",
    )
    .expect("valid IDL");
    let error = generate_crate(&idl, &CodegenOptions::default())
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
    let error = generate_crate(&idl, &CodegenOptions::default())
        .expect_err("colliding field constant names")
        .to_string();
    assert_eq!(
        error,
        "field `Foo.xY` and field `Foo.x_y` would both generate the constant \
         `OFF_FOO_X_Y`; rename one of them"
    );
}
