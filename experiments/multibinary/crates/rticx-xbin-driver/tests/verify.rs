//! M6.9-T7 acceptance: ELF verification of the linked fixture binaries.
//!
//! `crates/mock/fixtures/e2e` is built with `cargo xbin build --verify-elf` (through
//! [`build_and_verify`]) and, separately, with the plain-`cargo build`
//! workflow checked by [`verify`]. The mock distribution reserves its pools at
//! `0x3000_0000`+ — far above the static sections of a host PIE — so the
//! clean fixture passes. The overlapping case is injected by moving a pool's
//! `system.json` base over a real section of the linked binary, which must fail
//! naming both the section and the pool.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use object::{Object, ObjectSection};
use rticx_xbin_driver::{build, build_and_verify, verify, verify_binary};

/// Root of the checked-in end-to-end fixture.
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/fixtures/e2e")
        .canonicalize()
        .expect("the end-to-end fixture ships with the driver crate")
}

/// The `build --verify-elf` outcome, shared by the tests so the fixture is
/// built (and verified) only once.
///
/// The fixture is a standalone Cargo workspace below `crates/mock/fixtures/e2e`; building
/// it is expensive, and Cargo serializes concurrent builds on its lock anyway.
fn verified_build() -> &'static rticx_xbin_driver::BuildOutcome {
    static OUTCOME: OnceLock<rticx_xbin_driver::BuildOutcome> = OnceLock::new();
    OUTCOME.get_or_init(|| {
        build_and_verify(&fixture_root()).expect("`cargo xbin build --verify-elf` succeeds")
    })
}

/// Returns the first non-empty `SHF_ALLOC` section of `binary` as
/// `(name, address)`.
fn first_allocated_section(binary: &Path) -> (String, u64) {
    let bytes = std::fs::read(binary).expect("read the linked binary");
    let file = object::File::parse(bytes.as_slice()).expect("the fixture links an ELF");
    for section in file.sections() {
        let object::SectionFlags::Elf { sh_flags } = section.flags() else {
            continue;
        };
        if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0 && section.size() > 0 {
            return (
                section.name().expect("named section").to_string(),
                section.address(),
            );
        }
    }
    panic!("the fixture binary has no allocated section");
}

#[test]
fn build_with_verify_elf_passes_for_the_fixture() {
    let outcome = verified_build();

    assert_eq!(outcome.builds.len(), 2, "both applications build");
    assert_eq!(
        outcome
            .verifications
            .iter()
            .map(|verification| verification.application.as_str())
            .collect::<Vec<_>>(),
        ["app-m7", "app-m4"],
        "verifications follow `rticx.toml` order"
    );

    for verification in &outcome.verifications {
        assert!(
            verification.sections > 0,
            "`{}` reported no allocated sections; the ELF was not parsed",
            verification.application
        );
        assert!(
            verification.pool_views > 0,
            "`{}` is an endpoint of no pool; `system.json` lost its cores",
            verification.application
        );
        assert_eq!(
            verification.pool_symbols, 0,
            "the mock distribution exports no pool bound symbols"
        );
        assert_eq!(
            verification.stack_symbol, None,
            "a host binary defines no stack bound symbol"
        );
    }
}

#[test]
fn an_overlapping_pool_view_is_rejected_with_the_pool_named() {
    let outcome = verified_build();
    let build = outcome
        .builds
        .iter()
        .find(|build| build.target == "m7")
        .expect("app-m7 was built");

    // Move the first pool's view over a real allocated section of the binary
    // and re-seal: the check must catch exactly that.
    let (section_name, address) = first_allocated_section(&build.binary);
    let mut view = outcome
        .sync
        .system
        .clone()
        .expect("sync emitted the system view");
    let base = u32::try_from(address).expect("a host PIE section fits 32 bits");
    view.pools[0].base_from_a = base;
    view.pools[0].base_from_b = base;
    view.seal();

    let pool = view.pools[0].id.clone();
    let error = verify_binary(&build.binary, "app-m7", &view)
        .expect_err("an overlapping pool view must be rejected")
        .to_string();

    assert!(
        error.contains(&format!("`{pool}`")),
        "names the pool: {error}"
    );
    assert!(error.contains(&section_name), "names the section: {error}");
    assert!(error.contains("overlaps IPC pool"), "{error}");
    assert!(
        error.contains(&format!("0x{address:08x}")),
        "names the range: {error}"
    );
}

#[test]
fn verify_checks_the_plain_cargo_build_output() {
    // Ensure the project was synced and built first (the plain workflow).
    let outcome = verified_build();

    let verified = verify(&fixture_root()).expect("`cargo xbin verify` succeeds");
    assert_eq!(
        verified.verifications.len(),
        outcome.builds.len(),
        "one verification per application"
    );
    for verification in &verified.verifications {
        assert!(verification.sections > 0);
        assert!(verification.pool_views > 0);
    }
}

#[test]
fn building_without_verification_reports_no_verifications() {
    // `build` (without `--verify-elf`) must stay cheap: no verifications.
    let root = fixture_root();
    let outcome = build(&root).expect("`cargo xbin build` succeeds");
    assert!(
        outcome.verifications.is_empty(),
        "`build` without `--verify-elf` verifies nothing"
    );
    assert_eq!(outcome.builds.len(), 2);
}
