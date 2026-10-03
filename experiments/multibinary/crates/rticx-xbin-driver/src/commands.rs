//! `sync` and `build` implementations.

use std::ffi::{OsStr, OsString};
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use rticx_xbin_pass::{META_OUT_ENV, SYSTEM_ENV};
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

    /// Returns the collected manifest of the Cargo target `target`, if any.
    pub fn manifest(&self, target: &str) -> Option<&AppManifest> {
        self.manifests
            .iter()
            .find(|manifest| manifest.target.name == target)
    }
}

/// How [`run_cargo`] treats the output of the `cargo` subprocesses it spawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CargoOutput {
    /// Capture the child's output and stay quiet on success; failures surface
    /// the captured standard error in [`DriverError::CargoFailed`]. This is the
    /// default for library callers (and the tests), which must not spew Cargo
    /// progress.
    #[default]
    Captured,
    /// Forward the child's output to the driver's own standard output/error as
    /// Cargo produces it (and still capture it for failures). Used by the CLI
    /// so `cargo xbin build` shows the per-application cargo steps.
    Streamed,
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
///
/// Cargo output is captured, keeping library callers quiet; the CLI uses
/// [`sync_with_output`] to stream it.
pub fn sync(project_root: &Path) -> Result<SyncOutcome, DriverError> {
    sync_with_output(project_root, CargoOutput::default())
}

/// [`sync`] with an explicit cargo-output mode.
pub(crate) fn sync_with_output(
    project_root: &Path,
    output: CargoOutput,
) -> Result<SyncOutcome, DriverError> {
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
            let manifests = collect_manifests(project_root, &output_dir, config, output)?;
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
/// crate is located next to the driver's own package; after extraction (M7)
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
    output: CargoOutput,
) -> Result<Vec<AppManifest>, DriverError> {
    config
        .applications()
        .iter()
        .map(|application| collect_manifest(project_root, output_dir, application, output))
        .collect()
}

/// Collects the manifest of a single application.
fn collect_manifest(
    project_root: &Path,
    output_dir: &Path,
    application: &Application,
    output: CargoOutput,
) -> Result<AppManifest, DriverError> {
    let package = application.package();
    let target = application.target().name();

    // `RTICX_XBIN_META_OUT` is invisible to Cargo's fingerprints, so cached
    // artifacts from an earlier run (with a different output directory) must
    // not be reused.
    run_cargo(
        project_root,
        &argv(&["clean", "--package", package]),
        &[],
        &[],
        output,
    )?;

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
    run_cargo(
        project_root,
        &check,
        &[(META_OUT_ENV, output_dir.as_os_str())],
        &[SYSTEM_ENV],
        output,
    )?;

    let source =
        std::fs::read_to_string(&manifest_path).map_err(|source| DriverError::MissingManifest {
            package: package.to_string(),
            path: manifest_path.clone(),
            source,
        })?;
    Ok(AppManifest::from_json(&source)?)
}

/// Runs `cargo <args>` in `project_root` with the given environment variables
/// set (`envs`) and removed (`unset`) for the compiler, and therefore for the
/// `#[app]` proc macro.
///
/// The driver uses this to select the pass mode: `RTICX_XBIN_META_OUT` for the
/// metadata `cargo check`s of [`sync`], `RTICX_XBIN_SYSTEM` for the
/// application builds of [`build`]. The other mode's variable is removed so an
/// inherited value (for example `RTICX_XBIN_META_OUT` exported in the user's
/// shell) can never select the wrong mode.
///
/// In [`CargoOutput::Captured`] mode the output is captured so library users
/// and tests stay quiet; on failure the captured standard error is part of
/// [`DriverError::CargoFailed`]. In [`CargoOutput::Streamed`] mode the
/// subprocess is prefixed with a `[cargo-xbin]` banner and its output is
/// forwarded to the driver's own streams as it is produced (while still being
/// captured for failure reporting), so `cargo xbin build` shows the cargo
/// steps of every application.
fn run_cargo(
    project_root: &Path,
    args: &[String],
    envs: &[(&str, &OsStr)],
    unset: &[&str],
    output: CargoOutput,
) -> Result<(), DriverError> {
    let command_line = format!("cargo {}", args.join(" "));
    let mut command = Command::new(cargo_binary());
    command.args(args).current_dir(project_root);
    for name in unset {
        command.env_remove(name);
    }
    for (name, value) in envs {
        command.env(name, value);
    }

    match output {
        CargoOutput::Captured => {
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
        CargoOutput::Streamed => {
            // Cargo decides whether to color its output from its own stderr
            // being a terminal, but we pipe that stream to forward and capture
            // it. When the driver's stderr is a terminal, ask Cargo to keep its
            // colors (`always`) so `cargo xbin build` looks like a plain
            // `cargo build`. An explicit `CARGO_TERM_COLOR` or `NO_COLOR` from
            // the user wins.
            if io::stderr().is_terminal()
                && std::env::var_os("CARGO_TERM_COLOR").is_none()
                && std::env::var_os("NO_COLOR").is_none()
            {
                command.env("CARGO_TERM_COLOR", "always");
            }
            eprintln!("[cargo-xbin] running `{command_line}`");
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            let mut child = command.spawn().map_err(|source| DriverError::CargoSpawn {
                command: command_line.clone(),
                dir: project_root.to_path_buf(),
                source,
            })?;

            // Drain both pipes on separate threads so the child can never block
            // on a full pipe, forwarding each chunk as Cargo writes it while
            // accumulating standard error for [`DriverError::CargoFailed`].
            let child_stdout = child.stdout.take().expect("piped standard output");
            let child_stderr = child.stderr.take().expect("piped standard error");
            let stdout = std::thread::spawn(move || forward(child_stdout, io::stdout()));
            let stderr = std::thread::spawn(move || forward(child_stderr, io::stderr()));

            let status = child.wait().map_err(|source| DriverError::CargoSpawn {
                command: command_line.clone(),
                dir: project_root.to_path_buf(),
                source,
            })?;
            let _ = stdout.join();
            let stderr = stderr.join().unwrap_or_default();

            if !status.success() {
                return Err(DriverError::CargoFailed {
                    command: command_line,
                    dir: project_root.to_path_buf(),
                    status,
                    stderr: stderr.trim_end().to_string(),
                });
            }
            Ok(())
        }
    }
}

/// Forwards everything `reader` produces to `sink` as it arrives and returns
/// what it read (lossily decoded) for failure reporting.
fn forward<R: Read, W: Write>(reader: R, mut sink: W) -> String {
    let mut reader = BufReader::new(reader);
    let mut captured = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            // The child closed the pipe (or the reader errored): stop.
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let chunk = &buffer[..read];
                let _ = sink.write_all(chunk);
                let _ = sink.flush();
                captured.extend_from_slice(chunk);
            }
        }
    }
    String::from_utf8_lossy(&captured).into_owned()
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

/// One application built by [`build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBuild {
    /// Cargo package that was built.
    pub package: String,
    /// Cargo binary target that was built.
    pub target: String,
}

/// Result of a `cargo xbin build` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutcome {
    /// The `sync` phase that ran first.
    pub sync: SyncOutcome,
    /// The applications built after `sync`, in `rticx.toml` order (empty
    /// without a project manifest).
    pub builds: Vec<AppBuild>,
}

/// Phase 2 entry point: `sync`, then build every application of the project
/// against the `system.json` it emitted (M4-T1).
///
/// Each application is built with `cargo build --package <package> --bin
/// <target>` (plus `--target <triple>` when declared), with
/// `RTICX_XBIN_SYSTEM` pointing at the just-written system view. The pass then
/// runs in codegen mode and the freshness checks (stale `topology_hash`,
/// source hash) guarantee the build matches the synced view. Without a project
/// manifest there is nothing to build, so `build` only lays out
/// `target/rticx-xbin/`.
///
/// Cargo output is captured, keeping library callers quiet; the CLI uses
/// [`build_with_output`] to stream it.
pub fn build(project_root: &Path) -> Result<BuildOutcome, DriverError> {
    build_with_output(project_root, CargoOutput::default())
}

/// [`build`] with an explicit cargo-output mode.
pub(crate) fn build_with_output(
    project_root: &Path,
    output: CargoOutput,
) -> Result<BuildOutcome, DriverError> {
    let sync = sync_with_output(project_root, output)?;

    // The pass can discover the view from the project root, but the driver
    // knows the exact path it just wrote; passing it explicitly keeps the
    // build independent of the application's directory layout.
    let system = std::path::absolute(sync.output_dir.join(SYSTEM_FILE))
        .unwrap_or_else(|_| sync.output_dir.join(SYSTEM_FILE));

    let mut builds = Vec::with_capacity(sync.applications().len());
    for application in sync.applications() {
        let package = application.package();
        let target = application.target().name();
        let mut args = argv(&["build", "--package", package, "--bin", target]);
        if let Some(triple) = application.target().triple() {
            args.extend(argv(&["--target", triple]));
        }
        run_cargo(
            project_root,
            &args,
            &[(SYSTEM_ENV, system.as_os_str())],
            &[META_OUT_ENV],
            output,
        )?;
        builds.push(AppBuild {
            package: package.to_string(),
            target: target.to_string(),
        });
    }

    Ok(BuildOutcome { sync, builds })
}

#[cfg(test)]
mod tests {
    use super::forward;

    #[test]
    fn forward_streams_every_chunk_and_captures_it() {
        let input: &[u8] = b"Compiling app-m7\nFinished `dev` profile\n";
        let mut sink = Vec::new();

        let captured = forward(input, &mut sink);

        assert_eq!(
            sink.as_slice(),
            input,
            "the sink sees the child's bytes verbatim"
        );
        assert_eq!(
            captured, "Compiling app-m7\nFinished `dev` profile\n",
            "the captured text is returned for failure reporting"
        );
    }

    #[test]
    fn forward_tolerates_non_utf8_output() {
        let input: &[u8] = &[b'a', 0xff, b'\n'];
        let mut sink = Vec::new();

        let captured = forward(input, &mut sink);

        assert_eq!(sink.as_slice(), input);
        assert_eq!(captured, "a\u{fffd}\n");
    }
}
