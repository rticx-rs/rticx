//! `sync` and `build` implementations.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use rticx_xbin_pass::META_OUT_ENV;
use rticx_xbin_proto::{
    AppManifest, Application, CodegenOptions, CrateStatus, GENERATED_CRATE_NAME, IpcTypes,
    MergedProject, ProjectConfig, RTICX_GENERATION, RtDependency, SystemView, generate_crate,
    merge_project, parse_idl_file, parse_project_file, system_view,
};

use crate::error::DriverError;
use crate::project::{IDL_MANIFEST, PROJECT_MANIFEST, SYSTEM_FILE, output_dir};

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
    /// Parsed IDL; empty when the project has no `ipc-types.toml` (and `None`
    /// without a project manifest).
    pub idl: Option<IpcTypes>,
    /// Merged and validated whole-project view (M1-T5); `None` without a
    /// project manifest.
    pub merged: Option<MergedProject>,
    /// Emitted `target/rticx-xbin/system.json` view (M1-T6); `None` without a
    /// project manifest.
    pub system: Option<SystemView>,
    /// Generated `ipc-types/` crate state (M1-T7); `None` without a project
    /// manifest or without an `ipc-types.toml`.
    pub ipc_types: Option<IpcTypesOutcome>,
    /// Application manifests collected during this run, in `rticx.toml` order
    /// (empty without a project manifest).
    pub manifests: Vec<AppManifest>,
}

/// Result of generating the `ipc-types` crate during a `sync` run (M1-T7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcTypesOutcome {
    /// Generated crate directory (`<project root>/ipc-types`).
    pub path: PathBuf,
    /// Per-file change detection result; untouched files stay untouched, so
    /// an unchanged IDL is a no-op.
    pub status: CrateStatus,
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

    /// Returns the collected manifest of the Cargo target `target`, if any.
    pub fn manifest(&self, target: &str) -> Option<&AppManifest> {
        self.manifests
            .iter()
            .find(|manifest| manifest.target.name == target)
    }
}

/// Phase 1 entry point.
///
/// Parses `rticx.toml` and `ipc-types.toml`, lays out `target/rticx-xbin/`
/// and, for every declared application, runs `cargo clean -p <package>`
/// followed by `cargo check -p <package> --bin <target>` with
/// `RTICX_XBIN_META_OUT` pointing at the output directory. The `#[app]`
/// macro's cross-binary pass then writes `<target>.xbin.json`, which is read
/// back into [`SyncOutcome::manifests`]. The manifests are merged and
/// validated into [`SyncOutcome::merged`] (M1-T5), the per-task FIFO
/// addresses are allocated and the sealed system view is written to
/// `target/rticx-xbin/system.json` (M1-T6). Finally, when the project has an
/// `ipc-types.toml`, the `ipc-types/` crate is generated below the project
/// root and only changed files are rewritten (M1-T7).
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

    let idl_path = project_root.join(IDL_MANIFEST);
    let has_idl = idl_path.is_file();

    let (idl, merged, manifests) = match &config {
        Some(config) => {
            let idl = read_idl(&idl_path)?;
            let manifests = collect_manifests(project_root, &output_dir, config)?;
            let merged = merge_project(config, &manifests, &idl)?;
            (Some(idl), Some(merged), manifests)
        }
        None => (None, None, Vec::new()),
    };

    let mut system = None;
    let mut ipc_types = None;
    if let (Some(merged), Some(idl)) = (&merged, &idl) {
        let view = system_view(merged, idl, RTICX_GENERATION)?;

        // Generate (but do not write yet) the crate so a codegen error leaves
        // the previous run's output untouched.
        let crate_path = project_root.join(GENERATED_CRATE_NAME);
        let generated = if has_idl {
            let options = CodegenOptions {
                rt_dependency: rt_dependency(project_root),
            };
            Some(generate_crate(idl, &options)?)
        } else {
            None
        };

        let path = output_dir.join(SYSTEM_FILE);
        std::fs::write(&path, view.to_json())
            .map_err(|source| DriverError::WriteSystem { path, source })?;
        system = Some(view);

        if let Some(generated) = generated {
            let status = generated.write_if_changed(&crate_path).map_err(|source| {
                DriverError::WriteGenerated {
                    path: crate_path.clone(),
                    source,
                }
            })?;
            ipc_types = Some(IpcTypesOutcome {
                path: crate_path,
                status,
            });
        }
    }

    Ok(SyncOutcome {
        project_root: project_root.to_path_buf(),
        manifest_path: config.is_some().then_some(manifest_path),
        output_dir,
        config,
        idl,
        merged,
        system,
        ipc_types,
        manifests,
    })
}

/// Dependency of the generated crate on `rticx-xbin-rt`.
///
/// The generated crate lives at `<project root>/ipc-types`, so the dependency
/// is expressed as a path relative to that directory. The in-tree runtime
/// crate is located next to the driver's own package; after extraction (M6)
/// this becomes a registry version.
fn rt_dependency(project_root: &Path) -> RtDependency {
    let rt = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rticx-xbin-rt");
    let from = project_root.join(GENERATED_CRATE_NAME);
    match relative_path(&from, &rt) {
        // TOML basic strings treat `\` as an escape, so the dependency path
        // always uses `/` separators.
        Some(relative) => RtDependency::Path(relative.replace('\\', "/")),
        None => RtDependency::Path(rt.to_string_lossy().into_owned()),
    }
}

/// Returns `to` relative to the directory `from`, both canonicalized.
///
/// `from` may not exist yet (the generated crate is written after this call);
/// in that case its parent is canonicalized instead.
fn relative_path(from: &Path, to: &Path) -> Option<String> {
    let from = canonicalize_allow_missing(from)?;
    let to = to.canonicalize().ok()?;

    let mut from_components = from.components().peekable();
    let mut to_components = to.components().peekable();
    while from_components.peek().is_some() && from_components.peek() == to_components.peek() {
        from_components.next();
        to_components.next();
    }

    let mut relative = PathBuf::new();
    for component in from_components {
        if !matches!(component, Component::Normal(_)) {
            return None;
        }
        relative.push("..");
    }
    for component in to_components {
        relative.push(component.as_os_str());
    }
    Some(relative.to_string_lossy().into_owned())
}

/// Canonicalizes `path`, or its parent joined with its final component when
/// `path` does not exist yet.
fn canonicalize_allow_missing(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }
    let parent = path.parent()?.canonicalize().ok()?;
    let name = path.file_name()?;
    Some(parent.join(name))
}

/// Reads `ipc-types.toml`, or returns an empty IDL when the file is absent.
fn read_idl(path: &Path) -> Result<IpcTypes, DriverError> {
    if path.is_file() {
        Ok(parse_idl_file(path)?)
    } else {
        Ok(IpcTypes::empty())
    }
}

/// Collects one manifest per declared application.
fn collect_manifests(
    project_root: &Path,
    output_dir: &Path,
    config: &ProjectConfig,
) -> Result<Vec<AppManifest>, DriverError> {
    config
        .applications()
        .iter()
        .map(|application| collect_manifest(project_root, output_dir, application))
        .collect()
}

/// Collects the manifest of a single application.
fn collect_manifest(
    project_root: &Path,
    output_dir: &Path,
    application: &Application,
) -> Result<AppManifest, DriverError> {
    let package = application.package();
    let target = application.target().name();

    // `RTICX_XBIN_META_OUT` is invisible to Cargo's fingerprints, so cached
    // artifacts from an earlier run (with a different output directory) must
    // not be reused.
    run_cargo(project_root, &argv(&["clean", "--package", package]), None)?;

    // Never read a manifest left over from a previous run: the pass must
    // write it for *this* check.
    let manifest_path = output_dir.join(AppManifest::file_name(target));
    if manifest_path.exists() {
        std::fs::remove_file(&manifest_path).map_err(|source| DriverError::RemoveManifest {
            path: manifest_path.clone(),
            source,
        })?;
    }

    let mut check = argv(&["check", "--package", package, "--bin", target]);
    if let Some(triple) = application.target().triple() {
        check.extend(argv(&["--target", triple]));
    }
    run_cargo(project_root, &check, Some(output_dir))?;

    let source =
        std::fs::read_to_string(&manifest_path).map_err(|source| DriverError::MissingManifest {
            package: package.to_string(),
            path: manifest_path.clone(),
            source,
        })?;
    Ok(AppManifest::from_json(&source)?)
}

/// Runs `cargo <args>` in `project_root`, optionally with
/// `RTICX_XBIN_META_OUT=<output_dir>` set for the compiler (and therefore for
/// the `#[app]` proc macro).
///
/// Output is captured so library users and tests stay quiet; on failure the
/// captured standard error is part of [`DriverError::CargoFailed`].
fn run_cargo(
    project_root: &Path,
    args: &[String],
    meta_out: Option<&Path>,
) -> Result<(), DriverError> {
    let command_line = format!("cargo {}", args.join(" "));
    let mut command = Command::new(cargo_binary());
    command.args(args).current_dir(project_root);
    if let Some(out_dir) = meta_out {
        command.env(META_OUT_ENV, out_dir);
    }

    let output = command.output().map_err(|source| DriverError::CargoSpawn {
        command: command_line.clone(),
        dir: project_root.to_path_buf(),
        source,
    })?;
    if !output.status.success() {
        return Err(DriverError::CargoFailed {
            command: command_line,
            dir: project_root.to_path_buf(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_string(),
        });
    }
    Ok(())
}

/// Owns a static argument list for [`run_cargo`].
fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// The Cargo executable to drive: `$CARGO` when invoked as a Cargo subcommand,
/// `cargo` otherwise.
fn cargo_binary() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

/// Result of a `cargo xbin build` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutcome {
    /// The `sync` phase that ran first.
    pub sync: SyncOutcome,
}

/// Phase 2 entry point: `sync`, then build every application.
///
/// Until M4 this only runs the `sync` phase; building the applications
/// against the emitted `system.json` lands in M4.
pub fn build(project_root: &Path) -> Result<BuildOutcome, DriverError> {
    // TODO(M4): after sync, build every application with the generated
    // `system.json`.
    Ok(BuildOutcome {
        sync: sync(project_root)?,
    })
}
