//! Golden, round-trip and error tests for `rticx.toml` parsing (M0-T5).
//!
//! Accepted manifests are asserted structurally and round-tripped through the
//! canonical rendering; rejected manifests assert the exact, deterministic
//! error message.

use std::io::Write;

use rticx_xbin_proto::project::{
    PROJECT_SCHEMA_VERSION, RegionKey, TargetKind, parse_project_file, parse_project_str,
};

const FULL_PROJECT: &str = r#"
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4", triple = "thumbv7em-none-eabihf" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"1->0" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
"#;

const CANONICAL: &str = "\
schema = 1

[[application]]
package = \"app-m7\"
target = { kind = \"bin\", name = \"m7\" }
core_ids = [0]

[[application]]
package = \"app-m4\"
target = { kind = \"bin\", name = \"m4\", triple = \"thumbv7em-none-eabihf\" }
core_ids = [1]

[ipc.regions]
\"0->1\" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
\"1->0\" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
";

fn err(source: &str) -> String {
    parse_project_str(source)
        .expect_err("expected the manifest to be rejected")
        .to_string()
}

// ---------------------------------------------------------------------------
// Accepted manifests
// ---------------------------------------------------------------------------

#[test]
fn parses_minimal_project() {
    let config = parse_project_str(
        r#"
        schema = 1

        [[application]]
        package = "app-m7"
        target = { kind = "bin", name = "m7" }
        core_ids = [0]
        "#,
    )
    .expect("valid manifest");

    assert_eq!(config.schema(), PROJECT_SCHEMA_VERSION);
    assert_eq!(config.applications().len(), 1);
    assert!(config.regions().is_empty());

    let application = config.application("app-m7").expect("application exists");
    assert_eq!(application.package(), "app-m7");
    assert_eq!(application.target().kind(), TargetKind::Bin);
    assert_eq!(application.target().name(), "m7");
    assert_eq!(application.target().triple(), None);
    assert_eq!(application.core_ids(), &[0]);
}

#[test]
fn parses_multi_application_project_with_regions() {
    let config = parse_project_str(FULL_PROJECT).expect("valid manifest");

    let packages: Vec<&str> = config
        .applications()
        .iter()
        .map(|application| application.package())
        .collect();
    assert_eq!(packages, vec!["app-m7", "app-m4"]);
    assert_eq!(
        config
            .application("app-m4")
            .expect("application exists")
            .target()
            .triple(),
        Some("thumbv7em-none-eabihf")
    );
    assert_eq!(config.core_ids().collect::<Vec<_>>(), vec![0, 1]);

    let region = config.region(0, 1).expect("region exists");
    assert_eq!(region.base_from_source(), 0x3004_0000);
    assert_eq!(region.base_from_target(), 0x3004_0000);
    assert_eq!(region.size(), 4096);
    assert_eq!(
        RegionKey::new(0, 1).to_string(),
        "0->1",
        "region keys render as source->target"
    );
    assert!(config.region(1, 0).is_some());
    assert!(config.region(1, 1).is_none());
}

#[test]
fn preserves_application_and_core_id_order() {
    let config = parse_project_str(
        r#"
        schema = 1

        [[application]]
        package = "z-app"
        target = { kind = "bin", name = "z" }
        core_ids = [4, 2]

        [[application]]
        package = "a-app"
        target = { kind = "bin", name = "a" }
        core_ids = [1]
        "#,
    )
    .expect("valid manifest");

    let packages: Vec<&str> = config
        .applications()
        .iter()
        .map(|application| application.package())
        .collect();
    assert_eq!(
        packages,
        vec!["z-app", "a-app"],
        "declaration order is kept"
    );
    assert_eq!(
        config
            .application("z-app")
            .expect("application exists")
            .core_ids(),
        &[4, 2],
        "local index order is kept"
    );
}

#[test]
fn regions_are_ordered_regardless_of_declaration_order() {
    let config = parse_project_str(
        r#"
        schema = 1

        [ipc.regions]
        "1->0" = { base_from_source = 50335744, base_from_target = 50335744, size = 4096 }
        "0->1" = { base_from_source = 50331648, base_from_target = 50331648, size = 4096 }
        "#,
    )
    .expect("valid manifest");

    let keys: Vec<String> = config.regions().keys().map(RegionKey::to_string).collect();
    assert_eq!(
        keys,
        vec!["0->1", "1->0"],
        "regions sort by (source, target)"
    );
}

#[test]
fn parse_project_file_reads_and_validates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rticx.toml");
    let mut file = std::fs::File::create(&path).expect("create file");
    file.write_all(b"schema = 1\n").expect("write file");

    let config = parse_project_file(&path).expect("valid manifest");
    assert_eq!(config.schema(), PROJECT_SCHEMA_VERSION);

    let missing = parse_project_file(&dir.path().join("nope.toml")).expect_err("missing file");
    let message = missing.to_string();
    assert!(
        message.starts_with("failed to read `") && message.contains("nope.toml"),
        "unexpected error: {message}"
    );
}

// ---------------------------------------------------------------------------
// Round-trip
// ---------------------------------------------------------------------------

#[test]
fn canonical_toml_matches_golden_fixture() {
    let config = parse_project_str(FULL_PROJECT).expect("valid manifest");
    assert_eq!(config.to_toml_string(), CANONICAL);
    assert_eq!(
        config.to_string(),
        CANONICAL,
        "Display is the canonical form"
    );
}

#[test]
fn canonical_toml_round_trips() {
    let config = parse_project_str(FULL_PROJECT).expect("valid manifest");
    let reparsed = parse_project_str(&config.to_toml_string()).expect("canonical form parses");
    assert_eq!(reparsed, config);
    assert_eq!(reparsed.to_toml_string(), config.to_toml_string());
}

#[test]
fn empty_project_round_trips() {
    let config = parse_project_str("schema = 1\n").expect("valid manifest");
    assert_eq!(config.to_toml_string(), "schema = 1\n");
    assert_eq!(
        parse_project_str(&config.to_toml_string()).expect("parses"),
        config
    );
}

// ---------------------------------------------------------------------------
// Rejected manifests: schema and top level
// ---------------------------------------------------------------------------

#[test]
fn rejects_missing_schema() {
    assert_eq!(
        err(
            "[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n"
        ),
        "missing required top-level key `schema` (expected `schema = 1`)"
    );
}

#[test]
fn rejects_unknown_schema_version() {
    assert_eq!(
        err("schema = 2\n"),
        "unsupported rticx.toml schema version 2; this tool supports schema = 1"
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
        err("schema = 1\napplications = []\n"),
        "unknown top-level key `applications`; expected `schema`, `application` or `ipc`"
    );
}

#[test]
fn rejects_malformed_toml() {
    let message = err("schema = 1\n[[application]\n");
    assert!(
        message.starts_with("failed to parse rticx.toml:"),
        "unexpected error: {message}"
    );
}

// ---------------------------------------------------------------------------
// Rejected manifests: applications
// ---------------------------------------------------------------------------

#[test]
fn rejects_application_section_that_is_not_an_array_of_tables() {
    assert_eq!(
        err("schema = 1\napplication = 3\n"),
        "`application` must be an array of tables (`[[application]]`), found an integer"
    );
    assert_eq!(
        err("schema = 1\n[application]\npackage = \"app\"\n"),
        "`application` must be an array of tables (`[[application]]`), found a table"
    );
}

#[test]
fn rejects_application_entry_that_is_not_a_table() {
    assert_eq!(
        err("schema = 1\napplication = [\"app\"]\n"),
        "`application[0]` must be a table, found a string"
    );
}

#[test]
fn rejects_unknown_application_key() {
    assert_eq!(
        err("schema = 1\n[[application]]\npackages = \"app\"\n"),
        "`application[0]` has unknown key `packages`; expected `package`, `target` or `core_ids`"
    );
}

#[test]
fn rejects_missing_application_keys() {
    assert_eq!(
        err(
            "schema = 1\n[[application]]\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0]` is missing the required `package` key"
    );
    assert_eq!(
        err("schema = 1\n[[application]]\npackage = \"app\"\ncore_ids = [0]\n"),
        "`application[0]` is missing the required `target` key"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\n"
        ),
        "`application[0]` is missing the required `core_ids` key"
    );
}

#[test]
fn rejects_invalid_package_name() {
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = 3\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0].package` must be a string, found an integer"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0].package` must not be empty"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app m7\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0].package` `app m7` is not a valid cargo package name \
         (expected ASCII letters, digits, `-` or `_`)"
    );
}

#[test]
fn rejects_duplicate_package() {
    assert_eq!(
        err("schema = 1\n\
             [[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n\
             [[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"b\" }\ncore_ids = [1]\n"),
        "package `app` is declared by more than one `[[application]]`"
    );
}

#[test]
fn rejects_invalid_target() {
    assert_eq!(
        err("schema = 1\n[[application]]\npackage = \"app\"\ntarget = 3\ncore_ids = [0]\n"),
        "`application[0].target` must be a table, found an integer"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\", tripples = \"x\" }\ncore_ids = [0]\n"
        ),
        "`application[0].target` has unknown key `tripples`; expected `kind`, `name` or `triple`"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0].target` is missing the required `kind` key"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\" }\ncore_ids = [0]\n"
        ),
        "`application[0].target` is missing the required `name` key"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = 3, name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0].target.kind` must be a string, found an integer"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"lib\", name = \"a\" }\ncore_ids = [0]\n"
        ),
        "`application[0].target.kind` must be `\"bin\"`, found `\"lib\"`"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"m 7\" }\ncore_ids = [0]\n"
        ),
        "`application[0].target.name` `m 7` is not a valid cargo target name \
         (expected ASCII letters, digits, `-` or `_`)"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\", triple = 3 }\ncore_ids = [0]\n"
        ),
        "`application[0].target.triple` must be a string, found an integer"
    );
}

#[test]
fn rejects_invalid_core_ids() {
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = \"0\"\n"
        ),
        "`application[0].core_ids` must be an array of integers, found a string"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = []\n"
        ),
        "`application[0].core_ids` must not be empty; \
         it maps each local core index to a global core id"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0, \"1\"]\n"
        ),
        "`application[0].core_ids[1]` must be an integer, found a string"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [-1]\n"
        ),
        "`application[0].core_ids[0]` = -1 is out of range 0..=4294967295"
    );
    assert_eq!(
        err(
            "schema = 1\n[[application]]\npackage = \"app\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0, 0]\n"
        ),
        "`application[0].core_ids` lists core id 0 more than once"
    );
}

#[test]
fn rejects_duplicate_global_core_id_across_applications() {
    assert_eq!(
        err("schema = 1\n\
             [[application]]\npackage = \"app-a\"\ntarget = { kind = \"bin\", name = \"a\" }\ncore_ids = [0]\n\
             [[application]]\npackage = \"app-b\"\ntarget = { kind = \"bin\", name = \"b\" }\ncore_ids = [1, 0]\n"),
        "global core id 0 is declared by both `app-a` and `app-b`; \
         core ids must be unique across applications"
    );
}

// ---------------------------------------------------------------------------
// Rejected manifests: ipc regions
// ---------------------------------------------------------------------------

#[test]
fn rejects_invalid_ipc_section() {
    assert_eq!(
        err("schema = 1\nipc = 3\n"),
        "`ipc` must be a table, found an integer"
    );
    assert_eq!(
        err("schema = 1\n[ipc]\nregion = {}\n"),
        "`ipc` has unknown key `region`; expected `regions`"
    );
    assert_eq!(
        err("schema = 1\n[ipc]\n"),
        "`ipc` is missing the required `regions` key"
    );
    assert_eq!(
        err("schema = 1\n[ipc]\nregions = 3\n"),
        "`ipc.regions` must be a table, found an integer"
    );
}

#[test]
fn rejects_malformed_region_keys() {
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0-1\" = { base_from_source = 0, base_from_target = 0, size = 1 }\n"
        ),
        "`ipc.regions` key `0-1` is not of the form `<source>-><target>` (for example `\"0->1\"`)"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"a->1\" = { base_from_source = 0, base_from_target = 0, size = 1 }\n"
        ),
        "`ipc.regions` key `a->1` has a non-integer source core id `a`"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0->b\" = { base_from_source = 0, base_from_target = 0, size = 1 }\n"
        ),
        "`ipc.regions` key `0->b` has a non-integer target core id `b`"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"1->1\" = { base_from_source = 0, base_from_target = 0, size = 1 }\n"
        ),
        "`ipc.regions` key `1->1` connects core 1 to itself; \
         a region carries one `source -> target` direction"
    );
}

#[test]
fn rejects_invalid_region_body() {
    assert_eq!(
        err("schema = 1\n[ipc.regions]\n\"0->1\" = 3\n"),
        "`ipc.regions.\"0->1\"` must be a table, found an integer"
    );
    assert_eq!(
        err("schema = 1\n[ipc.regions]\n\"0->1\" = { base = 0 }\n"),
        "`ipc.regions.\"0->1\"` has unknown key `base`; \
         expected `base_from_source`, `base_from_target` or `size`"
    );
    assert_eq!(
        err("schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_target = 0, size = 1 }\n"),
        "`ipc.regions.\"0->1\"` is missing the required `base_from_source` key"
    );
    assert_eq!(
        err("schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_source = 0, size = 1 }\n"),
        "`ipc.regions.\"0->1\"` is missing the required `base_from_target` key"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_source = 0, base_from_target = 0 }\n"
        ),
        "`ipc.regions.\"0->1\"` is missing the required `size` key"
    );
}

#[test]
fn rejects_region_fields_of_the_wrong_type_or_range() {
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_source = \"0x30040000\", base_from_target = 0, size = 1 }\n"
        ),
        "`ipc.regions.\"0->1\".base_from_source` must be an integer, found a string"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_source = -1, base_from_target = 0, size = 1 }\n"
        ),
        "`ipc.regions.\"0->1\".base_from_source` = -1 is out of range 0..=4294967295"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_source = 0, base_from_target = 0, size = 0 }\n"
        ),
        "`ipc.regions.\"0->1\".size` = 0 is out of range 1..=4294967295"
    );
    assert_eq!(
        err(
            "schema = 1\n[ipc.regions]\n\"0->1\" = { base_from_source = 0, base_from_target = 0, size = 4294967296 }\n"
        ),
        "`ipc.regions.\"0->1\".size` = 4294967296 is out of range 1..=4294967295"
    );
}
