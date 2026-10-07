//! Phase 2 freshness acceptance (M3-T4): the cross-binary interface fingerprint
//! ignores task bodies and doc comments, but a plain `cargo build` after an
//! interface edit fails and names `cargo xbin sync`.
//!
//! Both tests run the real proc-macro path over a copy of
//! `crates/mock/fixtures/e2e`; the shared helpers live in `common`.

mod common;

use rticx_xbin_driver::sync;

/// The user's original complaint: editing a receiver's body (or adding a doc
/// comment) after `cargo xbin sync` must still build, because nothing
/// cross-binary changed.
#[test]
fn plain_build_accepts_a_body_only_edit() {
    let dir = common::fixture_copy();
    let root = dir.path().join("e2e");
    sync(&root).expect("the copied fixture syncs");

    let source_path = root.join("app-m4/src/main.rs");
    let source = std::fs::read_to_string(&source_path).expect("app-m4 source");
    // A doc comment on the task method and a new statement in its body: the
    // receiver's cross-binary interface is unchanged.
    let edited = source.replace(
        "        fn exec(&mut self, _input: Self::SpawnInput) {\n",
        concat!(
            "        /// Documentation is not part of the cross-binary interface.\n",
            "        fn exec(&mut self, _input: Self::SpawnInput) {\n",
            "            let _body_only = 1 + 1;\n",
        ),
    );
    assert_ne!(edited, source, "the fixture receiver body must be edited");
    std::fs::write(&source_path, edited).expect("edit app-m4 source");

    let output = common::cargo_build(&root, "app-m4", "m4");
    assert!(
        output.status.success(),
        "a body-only edit must build:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An interface edit needs a re-sync: dropping the receiver declaration removes
/// a cross-binary endpoint, which the recorded fingerprint detects even though
/// no per-field `resolve_receiver` check runs.
#[test]
fn plain_build_rejects_a_source_interface_edit() {
    let dir = common::fixture_copy();
    let root = dir.path().join("e2e");
    sync(&root).expect("the copied fixture syncs");

    let source_path = root.join("app-m4/src/main.rs");
    let source = std::fs::read_to_string(&source_path).expect("app-m4 source");
    let edited = source.replace(
        "    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]\n",
        "",
    );
    assert_ne!(edited, source, "the fixture receiver must be edited");
    std::fs::write(&source_path, edited).expect("edit app-m4 source");

    let output = common::cargo_build(&root, "app-m4", "m4");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an interface edit must fail a plain build:\n{stderr}"
    );
    assert!(
        stderr.contains("interface changed since the last `cargo xbin sync`"),
        "{stderr}"
    );
    assert!(stderr.contains("run `cargo xbin sync`"), "{stderr}");
}
