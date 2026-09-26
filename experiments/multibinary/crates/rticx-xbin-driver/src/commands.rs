//! `sync` and `build` implementations.

use std::path::{Path, PathBuf};

use rticx_xbin_proto::{Application, ProjectConfig, parse_project_file};

use crate::error::DriverError;
use crate::project::{PROJECT_MANIFEST, output_dir};

/// Result of a `cargo xbin sync` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Resolved project root.
    pub project_root: PathBuf,
    /// `rticx.toml` path, when the project declares one.
    pub manifest_path: Option<PathBuf>,
    /// Created output directory (`target/rticx-xbin`).
    pub output_dir: PathBuf,
    /// Parsed project topology, when a manifest exists.
    pub config: Option<ProjectConfig>,
}

impl SyncOutcome {
    /// Returns the applications declared by the manifest (empty without one).
    pub fn applications(&self) -> &[Application] {
        self.config
            .as_ref()
            .map(ProjectConfig::applications)
            .unwrap_or(&[])
    }

    /// Returns the number of declared IPC regions.
    pub fn region_count(&self) -> usize {
        self.config
            .as_ref()
            .map(|config| config.regions().len())
            .unwrap_or(0)
    }
}

/// Phase 1 entry point.
///
/// M0 only validates `rticx.toml` when present and lays out
/// `target/rticx-xbin/`; per-application metadata collection (`cargo check`
/// with `RTICX_XBIN_META_OUT`), merge/validation, address allocation and
/// `system.json` emission land in M1.
pub fn sync(project_root: &Path) -> Result<SyncOutcome, DriverError> {
    let manifest_path = project_root.join(PROJECT_MANIFEST);
    let config = if manifest_path.is_file() {
        Some(parse_project_file(&manifest_path)?)
    } else {
        None
    };

    let output_dir = output_dir(project_root);
    std::fs::create_dir_all(&output_dir).map_err(|source| DriverError::CreateDir {
        path: output_dir.clone(),
        source,
    })?;

    Ok(SyncOutcome {
        project_root: project_root.to_path_buf(),
        manifest_path: config.is_some().then_some(manifest_path),
        output_dir,
        config,
    })
}

/// Result of a `cargo xbin build` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutcome {
    /// The `sync` phase that ran first.
    pub sync: SyncOutcome,
}

/// Phase 2 entry point: `sync`, then build every application.
///
/// M0 only runs the `sync` skeleton; per-application `cargo` invocation lands
/// in M1-T4/M4.
pub fn build(project_root: &Path) -> Result<BuildOutcome, DriverError> {
    // TODO(M1-T4): enumerate the applications, run `cargo check` with
    // `RTICX_XBIN_META_OUT`, merge/filter `system.json`, then build each app.
    Ok(BuildOutcome {
        sync: sync(project_root)?,
    })
}
