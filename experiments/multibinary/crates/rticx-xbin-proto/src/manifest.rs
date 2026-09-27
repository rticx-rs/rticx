//! Per-application metadata manifest, `<target>.xbin.json`.
//!
//! The compilation pass writes one manifest per application during phase 1
//! (`cargo xbin sync`, see `multibinary-multicore-plan.md` §7.1). The driver
//! then merges all manifests, validates the cross-application topology and
//! emits the single `system.json` system view.
//!
//! A manifest records:
//!
//! - package and Cargo target (`<target>.xbin.json` is named after the target);
//! - a [`Hash64`] of the application source as the pass saw it, used by the
//!   phase-2 freshness check (§7.3);
//! - the local core count plus the local -> global `core_ids` mapping (the
//!   identity `0..cores` when the application does not declare the key) and
//!   the `external_cores` visible to this application (§6.4);
//! - the `#[cross_bin_task]` receivers and `#[cross_bin_spawn]` senders
//!   declared by this application (§6.5), including the full input-type paths.
//!
//! Like `system.json`, the manifest is JSON with a `schema_version` and is
//! serialized deterministically: declarations are sorted by task name and the
//! type list is sorted and deduplicated.

use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::hash::Hash64;
use crate::project::TargetKind;

/// Schema version of `<target>.xbin.json` understood by this crate.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// File-name suffix of an application manifest.
pub const MANIFEST_FILE_SUFFIX: &str = ".xbin.json";

/// The metadata an application contributes to phase 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppManifest {
    /// Manifest schema version (always [`MANIFEST_SCHEMA_VERSION`] when
    /// produced by the pass).
    pub schema_version: u32,
    /// Cargo package name (`CARGO_PKG_NAME`).
    pub package: String,
    /// Cargo target the `#[app]` module lives in.
    pub target: TargetRef,
    /// Hash of the application source as the pass saw it (arguments plus the
    /// annotated module, *before* pass-owned attributes are stripped).
    pub source_hash: Hash64,
    /// Number of local cores (`#[app(cores = N)]`, default 1).
    pub cores: u32,
    /// Local core index -> global core id mapping as resolved from
    /// `#[app(core_ids = [..])]`; the identity `0..cores` when the
    /// application does not declare the key (M5).
    ///
    /// The pass always writes the resolved mapping; the field stays optional
    /// for hand-written manifests and views, which skip the `rticx.toml`
    /// consistency check when it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_ids: Option<Vec<u32>>,
    /// `#[app(external_cores = [..])]`: global ids of cores in other binaries
    /// visible to this application.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_cores: Vec<u32>,
    /// Full paths of every IDL type referenced by the declarations below,
    /// sorted and deduplicated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    /// `#[cross_bin_task]` receivers (this application executes them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receivers: Vec<ReceiverDecl>,
    /// `#[cross_bin_spawn]` senders (this application spawns them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub senders: Vec<SenderDecl>,
}

impl AppManifest {
    /// Returns the manifest file name for the Cargo target `target`.
    pub fn file_name(target: &str) -> String {
        format!("{target}{MANIFEST_FILE_SUFFIX}")
    }

    /// Renders the manifest as deterministic, pretty-printed JSON with a
    /// trailing newline.
    pub fn to_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("an AppManifest always serializes to JSON");
        json.push('\n');
        json
    }

    /// Parses a manifest, rejecting schema versions this tool does not know.
    pub fn from_json(source: &str) -> Result<Self, ManifestError> {
        let manifest: Self = serde_json::from_str(source)?;
        if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::Schema {
                found: manifest.schema_version,
                expected: MANIFEST_SCHEMA_VERSION,
            });
        }
        Ok(manifest)
    }
}

/// A Cargo target reference (`{ kind, name }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRef {
    /// Target kind.
    pub kind: TargetKind,
    /// Target name (`CARGO_BIN_NAME`), i.e. the binary name.
    pub name: String,
}

impl TargetRef {
    /// Creates a `bin` target reference.
    pub fn bin(name: impl Into<String>) -> Self {
        Self {
            kind: TargetKind::Bin,
            name: name.into(),
        }
    }
}

/// A `#[cross_bin_task]` declaration: a task executed by this application on
/// behalf of remote cores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverDecl {
    /// Task name (the declared struct identifier).
    pub name: String,
    /// Priority line on the receiver core.
    pub priority: u16,
    /// Number of pending inputs the task FIFO can hold.
    pub capacity: usize,
    /// Local core index the task runs on.
    pub core: u32,
    /// Global ids of the cores allowed to spawn this task, when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<Vec<u32>>,
    /// Full path of the task input type as written in `impl … { type Input = … }`.
    pub input_type: String,
}

impl ReceiverDecl {
    /// Returns the IDL name (last path segment) of the input type.
    pub fn input_type_name(&self) -> &str {
        simple_type_name(&self.input_type)
    }
}

/// A `#[cross_bin_spawn]` declaration: a stub with which this application
/// spawns a task running in another binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderDecl {
    /// Task name; must match the receiver declaration in the target binary.
    pub name: String,
    /// Global id of the target core that executes the task.
    pub core: u32,
    /// Priority line on the target core; must match the receiver.
    pub priority: u16,
    /// Number of pending inputs; must match the receiver.
    pub capacity: usize,
    /// Full input-type path, when the sender mirrors the receiver's
    /// `impl CrossBinSpawn { type Input = … }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_type: Option<String>,
}

impl SenderDecl {
    /// Returns the IDL name (last path segment) of the input type, if declared.
    pub fn input_type_name(&self) -> Option<&str> {
        self.input_type.as_deref().map(simple_type_name)
    }
}

/// Returns the last `::`-separated segment of a type path (for example
/// `EncryptReq` for `ipc_types::EncryptReq`), with surrounding whitespace
/// trimmed.
pub fn simple_type_name(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path).trim()
}
