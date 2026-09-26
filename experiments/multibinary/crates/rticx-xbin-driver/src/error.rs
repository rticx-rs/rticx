//! Driver error type.

use std::io;
use std::path::PathBuf;

use rticx_xbin_proto::ProjectError;
use thiserror::Error;

/// A fatal `cargo xbin` failure.
///
/// Messages are deterministic and name the offending path, so they can be
/// asserted on in tests and shown to the user verbatim.
#[derive(Debug, Error)]
pub enum DriverError {
    /// `rticx.toml` could not be read, parsed or validated.
    #[error(transparent)]
    Project(#[from] ProjectError),

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
}
