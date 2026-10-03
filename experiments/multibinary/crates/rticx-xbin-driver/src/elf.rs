//! Linked-image (ELF) verification (M6.9-T7).
//!
//! Since M6.9 the distribution owns the IPC pools: it reserves each dual's
//! shared block in its linker script and reports the per-core base views and
//! the shared budget through its capability binding. If a reserved pool
//! overlaps an allocated section of an application's image — or the stack — the
//! two cores silently corrupt each other's FIFOs. `cargo xbin build
//! --verify-elf` and `cargo xbin verify` parse the **linked** binary with the
//! [`object`] crate and turn that silent class into a build error:
//!
//! - every `SHF_ALLOC` section (`[address, address + size)`) of the image must
//!   lie outside every pool view of the application's own cores;
//! - the stack bound symbol (`_stack_start` and friends, when the image defines
//!   one) must lie outside every pool view; and
//! - where the distribution exports the linker symbols
//!   `__rticx_xbin_pool_<id>_start` / `_end`, they must match the pool's
//!   `system.json` base and budget.
//!
//! The check is *detection*, not prevention: it sees the static `SHF_ALLOC`
//! sections only, not runtime stack/heap growth, and the symbol checks need the
//! **unstripped** link output (a stripped binary has no `.symtab`, so the
//! distro bounds and the stack symbol cannot be found; the section-overlap
//! check still works). It is deliberately limited to the pools of this
//! application's cores: an application can only corrupt memory its own cores
//! can reach.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use object::{Object, ObjectSection, ObjectSymbol};
use rticx_xbin_proto::{PoolEntry, SystemView};

use crate::error::{DriverError, VerifyError};

/// Prefix of the linker symbols a distribution may export for a pool's bounds.
///
/// The complete names are `__rticx_xbin_pool_<id>_start` and
/// `__rticx_xbin_pool_<id>_end`, where `<id>` is the pool id with every
/// non-alphanumeric byte replaced by `_` (`mock-0-1` -> `mock_0_1`).
const POOL_SYMBOL_PREFIX: &str = "__rticx_xbin_pool_";

/// Conventional top-of-stack symbols checked against the pools.
///
/// The first one the image defines is checked; `_stack_start` is the
/// `cortex-m-rt`/linker-script symbol. An image without any of them (host
/// binaries) skips the stack check.
const STACK_SYMBOLS: &[&str] = &["_stack_start", "__rticx_xbin_stack_start"];

/// Result of verifying one application's linked binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfVerification {
    /// Cargo package (application) the binary belongs to.
    pub application: String,
    /// Linked binary that was checked.
    pub binary: PathBuf,
    /// Number of `SHF_ALLOC` sections inspected.
    pub sections: usize,
    /// Number of pool views the application's cores can reach.
    pub pool_views: usize,
    /// Number of distro pool bound symbol pairs found and checked.
    pub pool_symbols: usize,
    /// Stack bound symbol that was checked, when the image defines one.
    pub stack_symbol: Option<String>,
}

/// Verifies the linked ELF at `binary` for application `application` against
/// the synced `view`.
///
/// `application` is the Cargo package name; the application's cores are looked
/// up in `view.cores` and its pools are the `view.pools` entries one of those
/// cores is an endpoint of. Returns a report on success and a
/// [`VerifyError`](crate::VerifyError) naming the offending section/symbol and
/// pool on failure.
pub fn verify_binary(
    binary: &Path,
    application: &str,
    view: &SystemView,
) -> Result<ElfVerification, DriverError> {
    let bytes = std::fs::read(binary).map_err(|source| VerifyError::Read {
        path: binary.to_path_buf(),
        source,
    })?;
    let file = object::File::parse(bytes.as_slice()).map_err(|error| VerifyError::Parse {
        path: binary.to_path_buf(),
        reason: error.to_string(),
    })?;

    let views = pool_views(view, application);

    let sections = allocated_sections(&file);
    if let Some((section, pool)) = first_overlap(&sections, &views) {
        return Err(VerifyError::Overlap {
            application: application.to_string(),
            section: section.name.clone(),
            section_start: section.start,
            section_end: section.end,
            pool: pool.pool.clone(),
            pool_start: pool.start,
            pool_end: pool.end,
        }
        .into());
    }

    let symbols = image_symbols(&file);
    let pool_symbols = check_pool_bounds(&symbols, &views, application)?;

    let mut stack_symbol = None;
    for candidate in STACK_SYMBOLS {
        let Some(address) = symbol_address(&symbols, candidate) else {
            continue;
        };
        if let Some(pool) = containing_view(&views, address) {
            return Err(VerifyError::StackOverlap {
                application: application.to_string(),
                symbol: (*candidate).to_string(),
                address,
                pool: pool.pool.clone(),
                pool_start: pool.start,
                pool_end: pool.end,
            }
            .into());
        }
        stack_symbol = Some((*candidate).to_string());
        break;
    }

    Ok(ElfVerification {
        application: application.to_string(),
        binary: binary.to_path_buf(),
        sections: sections.len(),
        pool_views: views.len(),
        pool_symbols,
        stack_symbol,
    })
}

/// One pool as seen by one of the application's cores.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PoolView {
    /// Pool id, as recorded in `system.json`.
    pool: String,
    /// Global core id whose view this is.
    core: u32,
    /// First byte of the pool in that core's address space.
    start: u64,
    /// One past the last byte of the pool.
    end: u64,
}

/// The pool views the application can reach.
///
/// For every `pools[]` entry with one of the application's cores as an
/// endpoint, the view is that core's base extended by the shared budget. A
/// single-application dual contributes both views.
fn pool_views(view: &SystemView, application: &str) -> Vec<PoolView> {
    let cores: BTreeSet<u32> = view
        .cores
        .iter()
        .filter(|core| core.app == application)
        .map(|core| core.global_id)
        .collect();

    let mut views = Vec::new();
    for pool in &view.pools {
        if cores.contains(&pool.core_a) {
            views.push(pool_view(pool, pool.core_a, pool.base_from_a));
        }
        if cores.contains(&pool.core_b) {
            views.push(pool_view(pool, pool.core_b, pool.base_from_b));
        }
    }
    views
}

/// Builds the view of `pool` seen by `core`, whose base is `base`.
fn pool_view(pool: &PoolEntry, core: u32, base: u32) -> PoolView {
    let start = u64::from(base);
    PoolView {
        pool: pool.id.clone(),
        core,
        start,
        end: start + u64::from(pool.budget),
    }
}

/// One allocated section of the linked image.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionSpan {
    /// Section name (`.text`, `.data`, …).
    name: String,
    /// First byte of the section.
    start: u64,
    /// One past the last byte of the section.
    end: u64,
}

/// Collects every non-empty `SHF_ALLOC` section of `file`.
fn allocated_sections(file: &object::File<'_>) -> Vec<SectionSpan> {
    let mut sections = Vec::new();
    for section in file.sections() {
        let object::SectionFlags::Elf { sh_flags } = section.flags() else {
            continue;
        };
        if sh_flags & u64::from(object::elf::SHF_ALLOC) == 0 {
            continue;
        }
        let size = section.size();
        if size == 0 {
            continue;
        }
        let name = section.name().unwrap_or("<unnamed>").to_string();
        sections.push(SectionSpan {
            name,
            start: section.address(),
            end: section.address() + size,
        });
    }
    sections
}

/// Returns the first `(section, pool view)` pair whose ranges overlap.
///
/// Ranges are half-open (`[start, end)`) and therefore only overlap when
/// `section.start < pool.end && pool.start < section.end`. Sections are
/// compared in file order and pools in `system.json` order, so the reported
/// offender is deterministic.
fn first_overlap<'a>(
    sections: &'a [SectionSpan],
    views: &'a [PoolView],
) -> Option<(&'a SectionSpan, &'a PoolView)> {
    sections.iter().find_map(|section| {
        views
            .iter()
            .find(|view| section.start < view.end && view.start < section.end)
            .map(|view| (section, view))
    })
}

/// Returns the pool view containing `address`, if any.
fn containing_view(views: &[PoolView], address: u64) -> Option<&PoolView> {
    views
        .iter()
        .find(|view| view.start <= address && address < view.end)
}

/// One defined symbol of the linked image.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageSymbol {
    /// Symbol name.
    name: String,
    /// Address the symbol resolves to.
    address: u64,
}

/// Collects every defined symbol of `file` with a non-zero address, from both
/// the static and the dynamic symbol table.
fn image_symbols(file: &object::File<'_>) -> Vec<ImageSymbol> {
    let mut symbols = Vec::new();
    for symbol in file.symbols().chain(file.dynamic_symbols()) {
        if !symbol.is_definition() {
            continue;
        }
        let address = symbol.address();
        if address == 0 {
            continue;
        }
        let Ok(name) = symbol.name() else {
            continue;
        };
        symbols.push(ImageSymbol {
            name: name.to_string(),
            address,
        });
    }
    symbols
}

/// Returns the address the first symbol named `name` resolves to.
fn symbol_address(symbols: &[ImageSymbol], name: &str) -> Option<u64> {
    symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .map(|symbol| symbol.address)
}

/// Checks the distro's exported pool bound symbols against `system.json`.
///
/// Returns how many pool bound pairs were found and checked. A pool whose
/// distribution exports neither or only one of the two symbols is skipped (the
/// distro does not describe its bounds that way); a pair whose range differs
/// from the `system.json` view is a hard error.
fn check_pool_bounds(
    symbols: &[ImageSymbol],
    views: &[PoolView],
    application: &str,
) -> Result<usize, DriverError> {
    let mut checked = 0;
    let mut seen: BTreeSet<&str> = BTreeSet::new();

    for view in views {
        if !seen.insert(view.pool.as_str()) {
            continue;
        }
        let id = sanitize_pool_id(&view.pool);
        let start_symbol = format!("{POOL_SYMBOL_PREFIX}{id}_start");
        let end_symbol = format!("{POOL_SYMBOL_PREFIX}{id}_end");
        let (Some(found_start), Some(found_end)) = (
            symbol_address(symbols, &start_symbol),
            symbol_address(symbols, &end_symbol),
        ) else {
            continue;
        };

        // The application may own both endpoints of the dual; the exported
        // symbol then has to match the view of one of them.
        let budget = view.end - view.start;
        let matches = views
            .iter()
            .filter(|candidate| candidate.pool == view.pool)
            .any(|candidate| {
                found_start == candidate.start
                    && found_end.saturating_sub(found_start) == candidate.end - candidate.start
            });
        if !matches {
            return Err(VerifyError::PoolBounds {
                application: application.to_string(),
                pool: view.pool.clone(),
                start_symbol,
                end_symbol,
                found_start,
                found_end,
                expected_start: view.start,
                expected_end: view.start + budget,
            }
            .into());
        }
        checked += 1;
    }

    Ok(checked)
}

/// Turns a pool id into the identifier part of its linker symbols.
fn sanitize_pool_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rticx_xbin_proto::{CoreEntry, PoolEntry};

    /// A view with one core per package and one pool.
    fn view(pools: Vec<PoolEntry>, cores: &[(&str, u32)]) -> SystemView {
        let mut view = SystemView::empty("0.2");
        view.cores = cores
            .iter()
            .enumerate()
            .map(|(index, (app, global_id))| CoreEntry {
                global_id: *global_id,
                physical_core: *global_id,
                app: (*app).to_string(),
                local_index: index as u32,
            })
            .collect();
        view.pools = pools;
        view.seal();
        view
    }

    fn pool(
        id: &str,
        core_a: u32,
        core_b: u32,
        base_a: u32,
        base_b: u32,
        budget: u32,
    ) -> PoolEntry {
        PoolEntry {
            id: id.to_string(),
            core_a,
            core_b,
            base_from_a: base_a,
            base_from_b: base_b,
            budget,
            used: 0,
        }
    }

    fn section(name: &str, start: u64, end: u64) -> SectionSpan {
        SectionSpan {
            name: name.to_string(),
            start,
            end,
        }
    }

    fn symbol(name: &str, address: u64) -> ImageSymbol {
        ImageSymbol {
            name: name.to_string(),
            address,
        }
    }

    #[test]
    fn pool_views_cover_only_the_applications_cores() {
        let view = view(
            vec![
                pool("a-b", 0, 1, 0x1000, 0x2000, 0x100),
                pool("b-c", 1, 2, 0x3000, 0x4000, 0x100),
            ],
            &[("app-a", 0), ("app-b", 1)],
        );

        let views = pool_views(&view, "app-a");
        assert_eq!(views.len(), 1, "app-a only reaches its own dual");
        assert_eq!(views[0].pool, "a-b");
        assert_eq!((views[0].start, views[0].end), (0x1000, 0x1100));

        let views = pool_views(&view, "app-b");
        assert_eq!(views.len(), 2, "app-b is an endpoint of both duals");
        assert_eq!(
            views
                .iter()
                .map(|view| (view.pool.as_str(), view.start, view.end))
                .collect::<Vec<_>>(),
            [("a-b", 0x2000, 0x2100), ("b-c", 0x3000, 0x3100)]
        );
    }

    #[test]
    fn first_overlap_names_the_offending_pool() {
        let views = vec![
            PoolView {
                pool: "mock-0-1".to_string(),
                core: 0,
                start: 0x1800,
                end: 0x2800,
            },
            PoolView {
                pool: "mock-0-2".to_string(),
                core: 0,
                start: 0x5000,
                end: 0x6000,
            },
        ];
        let sections = vec![
            section(".text", 0x1000, 0x2000),
            section(".data", 0x5000, 0x5100),
        ];

        let (section, pool) = first_overlap(&sections, &views).expect("an overlap");
        assert_eq!(section.name, ".text");
        assert_eq!(pool.pool, "mock-0-1");
    }

    #[test]
    fn disjoint_sections_do_not_overlap() {
        let views = vec![PoolView {
            pool: "mock-0-1".to_string(),
            core: 0,
            start: 0x3000,
            end: 0x4000,
        }];
        // Touching ranges (`end == start`) are half-open and do not overlap.
        let sections = vec![
            section(".text", 0x1000, 0x3000),
            section(".data", 0x4000, 0x5000),
        ];
        assert_eq!(first_overlap(&sections, &views), None);
    }

    #[test]
    fn containing_view_treats_the_pool_as_half_open() {
        let views = vec![PoolView {
            pool: "mock-0-1".to_string(),
            core: 0,
            start: 0x2000,
            end: 0x2100,
        }];
        assert!(
            containing_view(&views, 0x2000).is_some(),
            "the base is inside"
        );
        assert!(
            containing_view(&views, 0x20ff).is_some(),
            "the last byte is inside"
        );
        assert!(
            containing_view(&views, 0x2100).is_none(),
            "one past the end is outside"
        );
        assert!(
            containing_view(&views, 0x1fff).is_none(),
            "below the base is outside"
        );
    }

    #[test]
    fn matching_pool_bounds_symbols_are_accepted() {
        let views = vec![PoolView {
            pool: "mock-0-1".to_string(),
            core: 0,
            start: 0x3000_0000,
            end: 0x3000_1000,
        }];
        let symbols = vec![
            symbol("__rticx_xbin_pool_mock_0_1_start", 0x3000_0000),
            symbol("__rticx_xbin_pool_mock_0_1_end", 0x3000_1000),
        ];

        assert_eq!(
            check_pool_bounds(&symbols, &views, "app-a").expect("matching bounds"),
            1
        );
    }

    #[test]
    fn mismatched_pool_bounds_symbols_are_rejected() {
        let views = vec![PoolView {
            pool: "mock-0-1".to_string(),
            core: 0,
            start: 0x3000_0000,
            end: 0x3000_1000,
        }];
        let symbols = vec![
            symbol("__rticx_xbin_pool_mock_0_1_start", 0x3000_0100),
            symbol("__rticx_xbin_pool_mock_0_1_end", 0x3000_1100),
        ];

        let error = check_pool_bounds(&symbols, &views, "app-a").expect_err("mismatched bounds");
        let message = error.to_string();
        assert!(message.contains("`mock-0-1`"), "{message}");
        assert!(message.contains("0x30000000"), "{message}");
        assert!(message.contains("0x30000100"), "{message}");
    }

    #[test]
    fn sanitizes_pool_ids_into_symbol_names() {
        assert_eq!(sanitize_pool_id("mock-0-1"), "mock_0_1");
        assert_eq!(sanitize_pool_id("h7.sram.ipc"), "h7_sram_ipc");
    }
}
