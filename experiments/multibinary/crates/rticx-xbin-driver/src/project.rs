//! Project root discovery and the `target/rticx-xbin/` output layout.

use std::path::{Path, PathBuf};

/// Manifest file identifying the root of a multi-binary project.
pub const PROJECT_MANIFEST: &str = "rticx.toml";

/// Driver-owned output directory, relative to the project root.
///
/// Eventually holds `<app>.xbin.json` per-application metadata,
/// `system.json` and generated artifacts (see
/// `multibinary-multicore-plan.md` §7).
pub const OUTPUT_DIR: &str = "target/rticx-xbin";

/// Returns the driver output directory below `project_root`.
pub fn output_dir(project_root: &Path) -> PathBuf {
    project_root.join(OUTPUT_DIR)
}

/// Walks up from `start` and returns the first directory containing
/// `rticx.toml`.
///
/// `cargo xbin` may be invoked from any workspace member, so the project root
/// is discovered like Cargo discovers the workspace root: the current
/// directory first, then each ancestor.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(PROJECT_MANIFEST).is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}
