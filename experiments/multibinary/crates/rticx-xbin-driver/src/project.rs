//! Project root discovery and the `target/rticx-xbin/` output layout.

use std::path::{Path, PathBuf};

/// Manifest file identifying the root of a multi-binary project.
pub const PROJECT_MANIFEST: &str = "rticx.toml";

/// Type-only IDL file at the project root 
pub const IDL_MANIFEST: &str = "ipc-types.toml";

/// Driver-owned output directory, relative to the project root.
///
/// Holds `<app>.xbin.json` per-application metadata, the emitted
/// `system.json` system view and generated artifacts
pub const OUTPUT_DIR: &str = "target/rticx-xbin";

/// Driver-generated system view inside [`OUTPUT_DIR`] (M1-T6).
pub const SYSTEM_FILE: &str = "system.json";

/// Driver-generated `ipc_types` module inside [`OUTPUT_DIR`] (M1-T7).
///
/// The cross-binary pass re-emits this file's tokens into each application's
/// `#[app]` module. The name is owned by `rticx-xbin-proto`, which also emits
/// the file, so the driver and the pass can never disagree.
pub use rticx_xbin_proto::IPC_TYPES_FILE;

/// Self-contained HTML visualization of the system view, written inside
/// [`OUTPUT_DIR`] by `cargo xbin sync --html`.
pub const HTML_FILE: &str = "system.html";

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
