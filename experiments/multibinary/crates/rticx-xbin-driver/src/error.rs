//! Driver error type.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use rticx_xbin_proto::{CodegenError, IdlError, ManifestError, MergeError, ProjectError};
use thiserror::Error;

/// A fatal `cargo xbin` failure.
///
/// Messages are deterministic and name the offending path or command, so they
/// can be asserted on in tests and shown to the user verbatim.
#[derive(Debug, Error)]
pub enum DriverError {
    /// `rticx.toml` could not be read, parsed or validated.
    #[error(transparent)]
    Project(#[from] ProjectError),

    /// `ipc-types.toml` could not be read, parsed or validated.
    #[error(transparent)]
    Idl(#[from] IdlError),

    /// The collected manifests are inconsistent with each other or with
    /// `rticx.toml`.
    #[error(transparent)]
    Merge(#[from] MergeError),

    /// The IDL layout hash could not be computed while emitting
    /// `system.json`.
    #[error(transparent)]
    Codegen(#[from] CodegenError),

    /// The current working directory could not be determined.
    #[error("failed to determine the current directory: {source}")]
    CurrentDir {
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A driver-owned directory could not be created.
    #[error("failed to create `{}`: {source}", path.display())]
    CreateDir {
        /// Directory the driver tried to create.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A `cargo` subprocess could not be spawned.
    #[error("failed to run `{command}` in `{}`: {source}", dir.display())]
    CargoSpawn {
        /// Rendered command line, e.g. `cargo check --package app-m4`.
        command: String,
        /// Directory the command ran in (the project root).
        dir: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A `cargo` subprocess exited with a failure status.
    #[error("`{command}` failed in `{}` with {status}:\n{stderr}", dir.display())]
    CargoFailed {
        /// Rendered command line, e.g. `cargo check --package app-m4`.
        command: String,
        /// Directory the command ran in (the project root).
        dir: PathBuf,
        /// Exit status reported by `cargo`.
        status: ExitStatus,
        /// Captured standard error, trimmed.
        stderr: String,
    },

    /// `system.json` could not be written.
    #[error("failed to write `{}`: {source}", path.display())]
    WriteSystem {
        /// Path the driver tried to write.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The generated `ipc-types` crate could not be written.
    #[error("failed to write the generated `ipc-types` crate at `{}`: {source}", path.display())]
    WriteGenerated {
        /// Generated crate directory the driver tried to write below.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// A stale manifest could not be removed before metadata collection.
    #[error("failed to remove stale manifest `{}`: {source}", path.display())]
    RemoveManifest {
        /// Manifest the driver tried to remove.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// `cargo check` succeeded but the pass did not write the manifest.
    #[error(
        "`cargo check` for package `{package}` did not write `{}`: {source}\n\
         is `{package}` an RTICX application whose `#[app]` macro binds the cross-binary pass?",
        path.display()
    )]
    MissingManifest {
        /// Cargo package that was checked.
        package: String,
        /// Manifest the pass was expected to write.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// An application manifest could not be parsed or has an unknown schema.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}
