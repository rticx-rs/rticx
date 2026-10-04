//! Driver error type.

use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use rticx_xbin_proto::{
    CodegenError, IdlError, ManifestError, MergeError, ProjectError, SystemError,
};
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

    /// `--html` was requested but the project produced no system view.
    #[error(
        "cannot render `--html`: no system view was produced for `{}`\n\
         `cargo xbin sync --html` needs a project with an `rticx.toml` manifest",
        root.display()
    )]
    NoSystemView {
        /// Project root that was synced.
        root: PathBuf,
    },

    /// The HTML visualization could not be written.
    #[error("failed to write the HTML visualization `{}`: {source}", path.display())]
    WriteHtml {
        /// Path the driver tried to write.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The generated `ipc_types` module could not be written.
    #[error("failed to write the generated `ipc_types` module at `{}`: {source}", path.display())]
    WriteGenerated {
        /// Generated module file the driver tried to write.
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

    /// `cargo build` succeeded but reported no linked executable to verify.
    #[error(
        "`cargo build --package {package} --bin {target}` reported no linked executable;\n\
         cannot verify a library target"
    )]
    MissingExecutable {
        /// Cargo package that was built.
        package: String,
        /// Cargo binary target that was built.
        target: String,
    },

    /// `verify` was run without the project manifest it needs.
    #[error("cannot verify `{}`: no `rticx.toml` project manifest", root.display())]
    NoProjectManifest {
        /// Resolved project root.
        root: PathBuf,
    },

    /// `verify` was run without a synced system view.
    #[error(
        "cannot verify: failed to read the system view `{}`: {source}\n\
         run `cargo xbin sync` first",
        path.display()
    )]
    MissingVerificationView {
        /// `target/rticx-xbin/system.json` the verifier tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// `verify` read a system view whose stored `topology_hash` no longer
    /// matches its contents.
    #[error(
        "the system view `{}` is stale (`topology_hash` mismatch);\n\
         run `cargo xbin sync` before verifying",
        path.display()
    )]
    StaleVerificationView {
        /// `target/rticx-xbin/system.json` the verifier read.
        path: PathBuf,
    },

    /// An application manifest could not be parsed or has an unknown schema.
    #[error(transparent)]
    Manifest(#[from] ManifestError),

    /// `system.json` could not be parsed or has an unknown schema.
    #[error(transparent)]
    System(#[from] SystemError),

    /// A linked binary failed ELF verification (M6.9-T7). Boxed because
    /// [`VerifyError`] carries several owned names and ranges.
    #[error(transparent)]
    Verify(Box<VerifyError>),
}

impl From<VerifyError> for DriverError {
    fn from(error: VerifyError) -> Self {
        DriverError::Verify(Box::new(error))
    }
}

/// A linked-image (ELF) verification failure (M6.9-T7).
///
/// Produced by `cargo xbin build --verify-elf` and `cargo xbin verify`; every
/// message names the offending binary, the section or symbol and the pool, so
/// a distro that reserved its pool over the image is told exactly what to fix.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// The linked binary could not be read.
    #[error("failed to read the linked binary `{}`: {source}", path.display())]
    Read {
        /// Binary the verifier tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },

    /// The linked binary could not be parsed as an ELF object.
    #[error(
        "`{}` is not a parseable ELF object: {reason}\n\
         verification needs the unstripped link output of `cargo xbin build`",
        path.display()
    )]
    Parse {
        /// Binary the verifier tried to parse.
        path: PathBuf,
        /// Message from the object parser.
        reason: String,
    },

    /// An allocated section of the linked image overlaps a distro pool.
    #[error(
        "`{application}`: section `{section}` at [0x{section_start:08x}, 0x{section_end:08x}) \
         overlaps IPC pool `{pool}` at [0x{pool_start:08x}, 0x{pool_end:08x})\n\
         the distribution must reserve its IPC pools outside the linked image"
    )]
    Overlap {
        /// Application whose binary was checked.
        application: String,
        /// Name of the overlapping `SHF_ALLOC` section.
        section: String,
        /// First byte of the section.
        section_start: u64,
        /// One past the last byte of the section.
        section_end: u64,
        /// Id of the overlapped pool.
        pool: String,
        /// First byte of the pool in this application's address space.
        pool_start: u64,
        /// One past the last byte of the pool.
        pool_end: u64,
    },

    /// The stack bound symbol lies inside a distro pool.
    #[error(
        "`{application}`: stack bound symbol `{symbol}` at 0x{address:08x} lies inside \
         IPC pool `{pool}` at [0x{pool_start:08x}, 0x{pool_end:08x})\n\
         the distribution must reserve its IPC pools outside the stack"
    )]
    StackOverlap {
        /// Application whose binary was checked.
        application: String,
        /// Name of the stack bound symbol.
        symbol: String,
        /// Address the symbol resolves to.
        address: u64,
        /// Id of the overlapped pool.
        pool: String,
        /// First byte of the pool in this application's address space.
        pool_start: u64,
        /// One past the last byte of the pool.
        pool_end: u64,
    },

    /// A distro pool bound symbol disagrees with `system.json`.
    #[error(
        "`{application}`: pool `{pool}` is bounded by `{start_symbol}`/`{end_symbol}` at \
         [0x{found_start:08x}, 0x{found_end:08x}) but `system.json` places it at \
         [0x{expected_start:08x}, 0x{expected_end:08x})"
    )]
    PoolBounds {
        /// Application whose binary was checked.
        application: String,
        /// Id of the mismatched pool.
        pool: String,
        /// Exported start symbol of the pool.
        start_symbol: String,
        /// Exported end symbol of the pool.
        end_symbol: String,
        /// Address the start symbol resolves to.
        found_start: u64,
        /// Address the end symbol resolves to.
        found_end: u64,
        /// Base `system.json` records for the pool view.
        expected_start: u64,
        /// End `system.json` records for the pool view (`base + budget`).
        expected_end: u64,
    },
}
