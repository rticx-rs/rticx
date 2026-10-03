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
//! - the native `#[sw_task]` cross-binary receivers declared by this
//!   application (§6.5), including the full `SpawnInput` paths. There are no
//!   sender declarations (M5.5): the driver infers each task's single producer
//!   application from the receivers' `spawn_by`;
//! - the **distro capability binding** (M6.9-T2): per local core, the physical
//!   core it runs on and the IPC pools that physical core can reach
//!   ([`CoreCapability`]). The distro owns the pool addresses and the shared
//!   per-dual budget; the driver matches the two endpoints' entries when it
//!   merges the manifests (M6.9-T4).
//!
//! Like `system.json`, the manifest is JSON with a `schema_version` and is
//! serialized deterministically: declarations are sorted by task name, the
//! type list is sorted and deduplicated, and capability entries are ordered by
//! local core and `(peer, pool id)`.

use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::hash::Hash64;
use crate::project::TargetKind;

/// Schema version of `<target>.xbin.json` understood by this crate.
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;

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
    /// Native `#[sw_task]` receivers (this application executes them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receivers: Vec<ReceiverDecl>,
    /// Distro capability binding, one entry per local core (M6.9-T2): the
    /// physical core it runs on and the IPC pools that physical core can
    /// reach. Recorded only when the distribution bound a code-generation
    /// backend; without one the field is absent, because no distro vocabulary
    /// is available (the driver then has no pool path for any direction).
    ///
    /// Entries are ordered by local core index; within an entry, pools are
    /// ordered by `(peer, id)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CoreCapability>,
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

/// A native `#[sw_task]` cross-binary receiver: a task executed by this
/// application on behalf of a single remote producer core (M5.5).
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
    /// Global id of the single producer core that spawns this task (the
    /// receiver's `spawn_by`, resolved in the global namespace).
    pub spawn_by: u32,
    /// Full path of the task input type as written in the receiver's
    /// `impl RticSwTask { type SpawnInput = … }`.
    pub input_type: String,
}

impl ReceiverDecl {
    /// Returns the IDL name (last path segment) of the input type.
    pub fn input_type_name(&self) -> &str {
        simple_type_name(&self.input_type)
    }
}

/// The distro capability binding of one local core (M6.9-T2).
///
/// The distribution reports the physical core a local core runs on and the
/// IPC pools that physical core can reach. The driver matches the two
/// endpoints of a dual through the physical ids and the pool ids (M6.9-T4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreCapability {
    /// Local core index (`0..cores`).
    pub local_core: u32,
    /// Distro-defined physical core id this local core runs on.
    pub physical_core: u32,
    /// IPC pools this physical core can reach, ordered by `(peer, id)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pools: Vec<PoolDecl>,
}

/// One distro IPC pool as seen from one endpoint of a dual (M6.9-T2).
///
/// The two endpoints of a dual (`A <-> B`) report the same [`PoolDecl::id`]
/// with opposite physical cores, each reporting its own view as
/// [`PoolDecl::base_local`] and the peer's as [`PoolDecl::base_peer`]; both
/// directions allocate inside the shared [`PoolDecl::budget`] (M6.9-T4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolDecl {
    /// Symbolic, distro-defined pool id (the dual's matching key).
    pub id: String,
    /// The other endpoint of the dual, in the distribution's physical-core
    /// vocabulary.
    pub peer: u32,
    /// This core's view (base address) of the pool.
    pub base_local: u32,
    /// The peer's view (base address) of the pool; aliases are allowed.
    pub base_peer: u32,
    /// Bytes the distribution reserves in the pool for IPC, shared by both
    /// directions of the dual.
    pub budget: u32,
    /// Cache/MPU policy of the pool.
    pub policy: PoolCachePolicy,
}

/// Cache/MPU policy of a distro IPC pool, as recorded in the manifest
/// (M6.9-T2). Mirrors the pass-side capability binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolCachePolicy {
    /// Normal, Non-cacheable, Shareable — the supported v1 policy.
    NormalNonCacheableShareable,
    /// Normal, cacheable, Shareable: requires explicit cache maintenance
    /// around every shared access (the runtime backend's cache hooks).
    NormalCacheableShareable,
}

/// Returns the last `::`-separated segment of a type path (for example
/// `EncryptReq` for `ipc_types::EncryptReq`), with surrounding whitespace
/// trimmed.
pub fn simple_type_name(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path).trim()
}
