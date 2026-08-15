//! Interaction with `cargo`: metadata discovery, fingerprint busting, and the
//! `cargo check` invocation that drives the macro expansion.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use serde::Deserialize;

/// The subset of `cargo metadata` output the tool needs.
#[derive(Deserialize)]
pub struct Metadata {
    pub target_directory: PathBuf,
    pub packages: Vec<Package>,
}

#[derive(Deserialize)]
pub struct Package {
    pub name: String,
    pub edition: String,
    pub manifest_path: PathBuf,
    pub targets: Vec<Target>,
}

#[derive(Deserialize, Clone)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    pub src_path: PathBuf,
}

impl Metadata {
    pub fn load(manifest_path: &Path) -> Result<Metadata, String> {
        let output = Command::new(cargo())
            .args([
                "metadata",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
            ])
            .arg(manifest_path)
            .output()
            .map_err(|e| format!("failed to run `cargo metadata`: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "`cargo metadata` failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("failed to parse `cargo metadata` output: {e}"))
    }

    /// The package whose manifest is `manifest_path`.
    pub fn root_package(&self, manifest_path: &Path) -> Result<&Package, String> {
        let canonical =
            std::fs::canonicalize(manifest_path).unwrap_or_else(|_| manifest_path.to_path_buf());
        self.packages
            .iter()
            .find(|p| {
                let p_canon = std::fs::canonicalize(&p.manifest_path)
                    .unwrap_or_else(|_| p.manifest_path.clone());
                p_canon == canonical
            })
            .ok_or_else(|| format!("no package found for manifest {}", manifest_path.display()))
    }
}

/// Resolves which binary/example target to expand.
pub fn resolve_target<'a>(
    package: &'a Package,
    bin: Option<&str>,
    example: Option<&str>,
) -> Result<&'a Target, String> {
    if let Some(name) = bin {
        return find_target(package, "bin", name)
            .ok_or_else(|| format!("no binary target `{name}` in package `{}`", package.name));
    }
    if let Some(name) = example {
        return find_target(package, "example", name)
            .ok_or_else(|| format!("no example target `{name}` in package `{}`", package.name));
    }
    let bins: Vec<_> = package
        .targets
        .iter()
        .filter(|t| t.kind.iter().any(|k| k == "bin"))
        .collect();
    match bins.as_slice() {
        [single] => return Ok(single),
        [] => {}
        many => {
            return Err(format!(
                "package `{}` has {} binary targets ({}); specify one with --bin or --example",
                package.name,
                many.len(),
                many.iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    }
    let examples: Vec<_> = package
        .targets
        .iter()
        .filter(|t| t.kind.iter().any(|k| k == "example"))
        .collect();
    match examples.as_slice() {
        [single] => Ok(single),
        [] => Err(format!(
            "package `{}` has no binary or example targets",
            package.name
        )),
        many => Err(format!(
            "package `{}` has {} example targets ({}); specify one with --example",
            package.name,
            many.len(),
            many.iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )),
    }
}

fn find_target<'a>(package: &'a Package, kind: &str, name: &str) -> Option<&'a Target> {
    package
        .targets
        .iter()
        .find(|t| t.name == name && t.kind.iter().any(|k| k == kind))
}

/// Removes the package artifacts so `cargo check` re-runs the proc macro.
/// Environment variables do not participate in cargo fingerprints, so a
/// cached build would otherwise never re-expand.
pub fn clean_package(manifest_path: &Path, package: &str) -> Result<(), String> {
    let status = Command::new(cargo())
        .args(["clean", "-p", package, "--manifest-path"])
        .arg(manifest_path)
        .status()
        .map_err(|e| format!("failed to run `cargo clean -p {package}`: {e}"))?;
    if !status.success() {
        return Err("`cargo clean` failed".to_string());
    }
    Ok(())
}

/// Builder for the `cargo check` invocation that drives expansion.
pub struct CheckOptions<'a> {
    pub manifest_path: &'a Path,
    pub target: Option<&'a str>,
    pub bin: Option<&'a str>,
    pub example: Option<&'a str>,
    pub features: Option<&'a str>,
    pub all_features: bool,
    pub no_default_features: bool,
    pub extra_args: &'a [String],
    /// Value for `RTICX_EXPAND_PASS_DIR` (unset when `None`).
    pub pass_dir: Option<&'a Path>,
    /// Value for `RTICX_EXPAND_PATH` override (unset when `None`).
    pub output_dir: Option<&'a Path>,
}

pub fn run_check(opts: &CheckOptions<'_>) -> Result<ExitStatus, String> {
    let mut cmd = Command::new(cargo());
    cmd.args(["check", "--manifest-path"])
        .arg(opts.manifest_path);
    if let Some(bin) = opts.bin {
        cmd.args(["--bin", bin]);
    }
    if let Some(example) = opts.example {
        cmd.args(["--example", example]);
    }
    if let Some(target) = opts.target {
        cmd.args(["--target", target]);
    }
    if let Some(features) = opts.features {
        cmd.args(["--features", features]);
    }
    if opts.all_features {
        cmd.arg("--all-features");
    }
    if opts.no_default_features {
        cmd.arg("--no-default-features");
    }
    cmd.args(opts.extra_args);

    // Trigger expansion inside rticx-core (any value works).
    cmd.env("RTICX_EXPAND", "1");
    if let Some(pass_dir) = opts.pass_dir {
        cmd.env("RTICX_EXPAND_PASS_DIR", pass_dir);
    }
    if let Some(dir) = opts.output_dir {
        cmd.env("RTICX_EXPAND_PATH", dir);
    }

    cmd.status()
        .map_err(|e| format!("failed to run `cargo check`: {e}"))
}

/// `cargo` binary: `$CARGO` when set (e.g. by cargo subcommand invocation).
fn cargo() -> &'static str {
    option_env!("CARGO").unwrap_or("cargo")
}

/// The default expansion directory for a crate: `<target>/rticx-expand`.
pub fn default_expansion_dir(metadata: &Metadata) -> PathBuf {
    metadata.target_directory.join("rticx-expand")
}
