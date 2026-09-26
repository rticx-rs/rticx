//! Golden tests for `ipc-types.toml` parsing and 32-bit-safe subset
//! validation (M0-T2).
//!
//! Accepted schemas are asserted structurally; rejected schemas assert the
//! exact, deterministic error message.

use std::io::Write;

use rticx_xbin_proto::idl::{FieldType, PrimTy, SCHEMA_VERSION, parse_idl_file, parse_idl_str};

fn err(source: &str) -> String {
    parse_idl_str(source)
        .expect_err("expected the IDL to be rejected")
        .to_string()
}

// ---------------------------------------------------------------------------
// Accepted schemas
// ---------------------------------------------------------------------------

#[test]
fn parses_minimal_message() {
    let idl = parse_idl_str(
        r#"
        schema = 1

        [message.EncryptReq]
        fields = { addr = "u32", len = "u32", key = "u32" }
        "#,
    )
    .expect("valid IDL");

    assert_eq!(idl.schema(), SCHEMA_VERSION);
    assert_eq!(idl.messages().len(), 1);
    assert!(idl.enums().is_empty());
    assert!(idl.has_type("EncryptReq"));

    let message = idl.message("EncryptReq").expect("message exists");
    assert_eq!(message.name(), "EncryptReq");
    let fields: Vec<(&str, &FieldType)> = message
        .fields()
        .iter()
        .map(|field| (field.name(), field.ty()))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("addr", &FieldType::Prim(PrimTy::U32)),
            ("len", &FieldType::Prim(PrimTy::U32)),
            ("key", &FieldType::Prim(PrimTy::U32)),
        ]
    );
}

#[test]
fn preserves_field_declaration_order() {
    // Field order is semantic: the canonical layout follows declaration
    // order, not alphabetical order.
    let idl = parse_idl_str(
        r#"
        schema = 1

        [message.Mixed]
        fields = { tag = "u8", value = "u32", flag = "u8" }
        "#,
    )
    .expect("valid IDL");

    let names: Vec<&str> = idl
        .message("Mixed")
        .expect("message exists")
        .fields()
        .iter()
        .map(|field| field.name())
        .collect();
    assert_eq!(names, vec!["tag", "value", "flag"]);
}

#[test]
fn parses_nested_messages_arrays_and_enums() {
    let idl = parse_idl_str(
        r#"
        schema = 1

        [message.Header]
        fields = { magic = "u32", version = "u8" }

        [message.SensorBatch]
        fields = { tag = "u8", kind = "Kind", xs = "[i16; 8]", headers = "[[Header; 2]; 3]" }

        [enum.Kind]
        variants = ["Fast", "Slow"]

        [enum.State]
        variants = { Idle = 0, Busy = 1, Failed = 7 }
        "#,
    )
    .expect("valid IDL");

    let batch = idl.message("SensorBatch").expect("message exists");
    assert_eq!(
        batch.field("kind").expect("field exists").ty(),
        &FieldType::Named("Kind".to_string())
    );
    assert_eq!(
        batch.field("headers").expect("field exists").ty(),
        &FieldType::Array(
            Box::new(FieldType::Array(
                Box::new(FieldType::Named("Header".to_string())),
                2,
            )),
            3,
        )
    );

    let kind = idl.get_enum("Kind").expect("enum exists");
    let kind_variants: Vec<(&str, u32)> = kind
        .variants()
        .iter()
        .map(|variant| (variant.name(), variant.discriminant()))
        .collect();
    assert_eq!(kind_variants, vec![("Fast", 0), ("Slow", 1)]);

    let state = idl.get_enum("State").expect("enum exists");
    let state_variants: Vec<(&str, u32)> = state
        .variants()
        .iter()
        .map(|variant| (variant.name(), variant.discriminant()))
        .collect();
    // Explicit-discriminant tables are sorted by discriminant, so the
    // generated enum is deterministic regardless of table iteration order.
    assert_eq!(
        state_variants,
        vec![("Idle", 0), ("Busy", 1), ("Failed", 7)]
    );
}

#[test]
fn accepts_all_supported_primitives_and_array_nesting() {
    let idl = parse_idl_str(
        r#"
        schema = 1

        [message.All]
        fields = { a = "u8", b = "u16", c = "u32", d = "i8", e = "i16", f = "i32", g = "f32", nested = "[[u8; 2]; 3]" }
        "#,
    )
    .expect("valid IDL");

    let all = idl.message("All").expect("message exists");
    let prims: Vec<&str> = all.fields()[..7]
        .iter()
        .map(|field| match field.ty() {
            FieldType::Prim(prim) => prim.name(),
            other => panic!("expected primitive, got {other}"),
        })
        .collect();
    assert_eq!(prims, vec!["u8", "u16", "u32", "i8", "i16", "i32", "f32"]);
    assert_eq!(
        all.field("nested").expect("field exists").ty(),
        &FieldType::Array(
            Box::new(FieldType::Array(Box::new(FieldType::Prim(PrimTy::U8)), 2)),
            3,
        )
    );
}

#[test]
fn parse_idl_file_reads_and_validates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ipc-types.toml");
    let mut file = std::fs::File::create(&path).expect("create file");
    file.write_all(b"schema = 1\n[message.Ping]\nfields = { x = \"u8\" }\n")
        .expect("write file");

    let idl = parse_idl_file(&path).expect("valid IDL");
    assert!(idl.has_type("Ping"));

    let missing = parse_idl_file(&dir.path().join("nope.toml")).expect_err("missing file");
    let message = missing.to_string();
    assert!(
        message.starts_with("failed to read `") && message.contains("nope.toml"),
        "unexpected error: {message}"
    );
}

// ---------------------------------------------------------------------------
// Rejected schemas: schema / structure
// ---------------------------------------------------------------------------

#[test]
fn rejects_missing_schema() {
    assert_eq!(
        err("[message.Ping]\nfields = { x = \"u8\" }\n"),
        "missing required top-level key `schema` (expected `schema = 1`)"
    );
}

#[test]
fn rejects_unknown_schema_version() {
    assert_eq!(
        err("schema = 2\n"),
        "unsupported IDL schema version 2; this tool supports schema = 1"
    );
}

#[test]
fn rejects_non_integer_schema() {
    assert_eq!(
        err("schema = \"1\"\n"),
        "`schema` must be an integer, found a string"
    );
}

#[test]
fn rejects_unknown_top_level_key() {
    assert_eq!(
        err("schema = 1\ntypes = {}\n"),
        "unknown top-level key `types`; expected `schema`, `message` or `enum`"
    );
}

#[test]
fn rejects_malformed_toml() {
    let message = err("schema = 1\n[message.Ping\n");
    assert!(
        message.starts_with("failed to parse ipc-types.toml:"),
        "unexpected error: {message}"
    );
}

#[test]
fn rejects_message_without_fields_key() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\n"),
        "`message.Ping` is missing the required `fields` key"
    );
}

#[test]
fn rejects_message_with_no_fields() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = {}\n"),
        "message `Ping` declares no fields (zero-sized types cannot cross the IPC boundary)"
    );
}

#[test]
fn rejects_unknown_message_key() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfieldz = { x = \"u8\" }\n"),
        "`message.Ping` has unknown key `fieldz`; expected `fields`"
    );
}

#[test]
fn rejects_message_section_that_is_not_a_table() {
    assert_eq!(
        err("schema = 1\nmessage = 3\n"),
        "`message` must be a table, found an integer"
    );
}

#[test]
fn rejects_non_string_field_type() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = 3 }\n"),
        "`message.Ping.fields.x` must be a string type expression, found an integer"
    );
}

// ---------------------------------------------------------------------------
// Rejected schemas: names and references
// ---------------------------------------------------------------------------

#[test]
fn rejects_invalid_message_name() {
    assert_eq!(
        err("schema = 1\n[message.\"1Ping\"]\nfields = { x = \"u8\" }\n"),
        "message name `1Ping` is not a valid Rust identifier"
    );
}

#[test]
fn rejects_invalid_field_name() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { \"foo-bar\" = \"u8\" }\n"),
        "field name in `message.Ping` `foo-bar` is not a valid Rust identifier"
    );
}

#[test]
fn rejects_keyword_field_name() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { type = \"u8\" }\n"),
        "field name in `message.Ping` `type` is not a valid Rust identifier"
    );
}

#[test]
fn rejects_unknown_type_reference() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"Nope\" }\n"),
        "`message.Ping.fields.x` references unknown type `Nope`; \
         declare it as `[message.Nope]` or `[enum.Nope]`"
    );
}

#[test]
fn rejects_type_declared_as_message_and_enum() {
    assert_eq!(
        err(
            "schema = 1\n[message.Both]\nfields = { x = \"u8\" }\n[enum.Both]\nvariants = [\"A\"]\n"
        ),
        "type `Both` is declared both as a message and as an enum; \
         each name may only be declared once"
    );
}

#[test]
fn rejects_self_referential_message() {
    assert_eq!(
        err("schema = 1\n[message.Node]\nfields = { next = \"Node\" }\n"),
        "cyclic message reference: Node -> Node"
    );
}

#[test]
fn rejects_mutually_recursive_messages() {
    assert_eq!(
        err(
            "schema = 1\n[message.A]\nfields = { b = \"B\" }\n[message.B]\nfields = { a = \"[A; 2]\" }\n"
        ),
        "cyclic message reference: A -> B -> A"
    );
}

// ---------------------------------------------------------------------------
// Rejected schemas: unsupported types
// ---------------------------------------------------------------------------

#[test]
fn rejects_unsupported_scalars() {
    let cases = [
        (
            "u64",
            "64-bit and wider scalar types are not part of the 32-bit-safe IPC subset (v1)",
        ),
        (
            "f64",
            "64-bit and wider scalar types are not part of the 32-bit-safe IPC subset (v1)",
        ),
        (
            "usize",
            "pointer-sized integers are not portable across cores and are not part of the IPC subset (v1)",
        ),
        (
            "bool",
            "`bool` is not part of the 32-bit-safe IPC subset (v1); use `u8` instead",
        ),
        (
            "char",
            "`char` is not part of the 32-bit-safe IPC subset (v1); use `u32` instead",
        ),
        (
            "str",
            "string types cannot cross the IPC boundary; use a fixed byte array (`[u8; N]`) instead",
        ),
    ];
    for (ty, reason) in cases {
        let source = format!("schema = 1\n[message.Ping]\nfields = {{ x = \"{ty}\" }}\n");
        assert_eq!(
            err(&source),
            format!("`message.Ping.fields.x`: type `{ty}` is not supported: {reason}")
        );
    }
}

#[test]
fn rejects_pointers_references_tuples_and_generics() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"*const u8\" }\n"),
        "`message.Ping.fields.x`: pointers and references cannot cross the IPC boundary (`*const u8`)"
    );
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"&u8\" }\n"),
        "`message.Ping.fields.x`: pointers and references cannot cross the IPC boundary (`&u8`)"
    );
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"(u8, u8)\" }\n"),
        "`message.Ping.fields.x`: tuples are not supported in the v1 IPC subset (`(u8, u8)`); \
         declare a `[message.…]` instead"
    );
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"Option<u8>\" }\n"),
        "`message.Ping.fields.x`: generic types are not supported in the v1 IPC subset (`Option<u8>`)"
    );
}

#[test]
fn rejects_arrays() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"[u8; 0]\" }\n"),
        "`message.Ping.fields.x`: array length must be at least 1 in `[u8; 0]`"
    );
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"[u8]\" }\n"),
        "`message.Ping.fields.x`: malformed array type `[u8]`; expected `[T; N]` with `N >= 1`"
    );
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"[u8; nope]\" }\n"),
        "`message.Ping.fields.x`: array length `nope` in `[u8; nope]` is not a non-negative integer"
    );
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"[u8; 1; 2]\" }\n"),
        "`message.Ping.fields.x`: malformed array type `[u8; 1; 2]`; expected `[T; N]` with `N >= 1`"
    );
}

#[test]
fn rejects_empty_type_expression() {
    assert_eq!(
        err("schema = 1\n[message.Ping]\nfields = { x = \"\" }\n"),
        "`message.Ping.fields.x` has an empty type expression"
    );
}

// ---------------------------------------------------------------------------
// Rejected schemas: enums
// ---------------------------------------------------------------------------

#[test]
fn rejects_enum_without_variants_key() {
    assert_eq!(
        err("schema = 1\n[enum.State]\n"),
        "`enum.State` is missing the required `variants` key"
    );
}

#[test]
fn rejects_enum_with_no_variants() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = []\n"),
        "enum `State` declares no variants"
    );
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = {}\n"),
        "enum `State` declares no variants"
    );
}

#[test]
fn rejects_unknown_enum_key() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvalues = [\"A\"]\n"),
        "`enum.State` has unknown key `values`; expected `variants`"
    );
}

#[test]
fn rejects_invalid_enum_name_and_variant_name() {
    assert_eq!(
        err("schema = 1\n[enum.\"1State\"]\nvariants = [\"A\"]\n"),
        "enum name `1State` is not a valid Rust identifier"
    );
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = [\"A-B\"]\n"),
        "variant name in `enum.State` `A-B` is not a valid Rust identifier"
    );
}

#[test]
fn rejects_non_string_variant_in_array_form() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = [1]\n"),
        "`enum.State.variants[0]` must be a string, found an integer"
    );
}

#[test]
fn rejects_duplicate_variant_in_array_form() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = [\"A\", \"A\"]\n"),
        "`enum.State` declares variant `A` more than once"
    );
}

#[test]
fn rejects_negative_or_non_integer_discriminant() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = { A = -1 }\n"),
        "`enum.State.variants.A` discriminant -1 is out of range 0..=4294967295"
    );
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = { A = true }\n"),
        "`enum.State.variants.A` must be an integer discriminant, found a boolean"
    );
}

#[test]
fn rejects_duplicate_discriminant() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = { A = 1, B = 1 }\n"),
        "`enum.State` reuses discriminant 1 for variants `A` and `B`"
    );
}

#[test]
fn rejects_variants_of_wrong_shape() {
    assert_eq!(
        err("schema = 1\n[enum.State]\nvariants = 1\n"),
        "`enum.State.variants` must be an array of variant names or a table mapping names \
         to `u32` discriminants, found an integer"
    );
}
