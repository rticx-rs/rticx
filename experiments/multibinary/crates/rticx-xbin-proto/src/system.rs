//! The driver-generated system view, `target/rticx-xbin/system.json`.
//!
//! `system.json` is the single description of the whole multi-binary project
//! (see `multibinary-multicore-plan.md` §7.2): cores, IDL types with their
//! canonical layout, cross-binary tasks with their FIFO allocations, the
//! shared-memory regions and the doorbell lines. Phase 1 (`cargo xbin sync`)
//! merges the per-application manifests into it; phase 2 (the compilation
//! pass) reads it and filters it by its own cores.
//!
//! Determinism is a hard requirement: identical inputs must produce
//! byte-identical JSON. The schema itself preserves vector order (the builder
//! sorts before emitting) and every field is serialized in declaration order.
//! Addresses are rendered as `0x`-prefixed hex strings; hashes as
//! [`Hash64`] hex strings.
//!
//! The [`SystemView::topology_hash`] covers everything except itself and is
//! recomputed with [`SystemView::seal`] after all other fields are filled in;
//! [`SystemView::verify_topology_hash`] detects post-hoc edits.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::error::SystemError;
use crate::hash::Hash64;
use crate::manifest::TargetRef;

/// Schema version of `system.json` understood by this crate.
pub const SYSTEM_SCHEMA_VERSION: u32 = 1;

/// RTICX generation (major.minor) recorded in every emitted `system.json`.
///
/// It tracks the root-workspace generation (`COMPATIBILITY.md`); the
/// experimental workspace pins the same generation through its `rticx-core`
/// path dependency, so this constant must be bumped together with it.
pub const RTICX_GENERATION: &str = "0.2";

/// The whole-project system view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemView {
    /// Schema version (always [`SYSTEM_SCHEMA_VERSION`] for produced values).
    pub schema_version: u32,
    /// Major.minor generation of the RTICX crates the project was synced with.
    pub rticx_generation: String,
    /// Hash over every other field of this view (the topology); see
    /// [`Self::seal`].
    pub topology_hash: Hash64,
    /// Hash of the IDL canonical layout, matching `ipc_types::LAYOUT_HASH`.
    pub layout_hash: Hash64,
    /// One entry per application (binary) of the project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<AppEntry>,
    /// Global core id -> application and local index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cores: Vec<CoreEntry>,
    /// Every IDL type with its canonical layout, sorted by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<TypeEntry>,
    /// Every cross-binary task, sorted by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskEntry>,
    /// Every declared `(source -> target)` region, sorted by `(source, target)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<RegionEntry>,
    /// Every doorbell line, sorted by `(source, target, priority)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doorbells: Vec<DoorbellEntry>,
}

impl SystemView {
    /// Creates an empty view with the current schema version. Callers fill in
    /// the fields and then call [`Self::seal`].
    pub fn empty(rticx_generation: impl Into<String>) -> Self {
        Self {
            schema_version: SYSTEM_SCHEMA_VERSION,
            rticx_generation: rticx_generation.into(),
            topology_hash: Hash64::ZERO,
            layout_hash: Hash64::ZERO,
            apps: Vec::new(),
            cores: Vec::new(),
            types: Vec::new(),
            tasks: Vec::new(),
            regions: Vec::new(),
            doorbells: Vec::new(),
        }
    }

    /// Renders the view as deterministic, pretty-printed JSON with a trailing
    /// newline.
    pub fn to_json(&self) -> String {
        let mut json =
            serde_json::to_string_pretty(self).expect("a SystemView always serializes to JSON");
        json.push('\n');
        json
    }

    /// Parses a view, rejecting schema versions this tool does not know.
    pub fn from_json(source: &str) -> Result<Self, SystemError> {
        let view: Self = serde_json::from_str(source)?;
        if view.schema_version != SYSTEM_SCHEMA_VERSION {
            return Err(SystemError::Schema {
                found: view.schema_version,
                expected: SYSTEM_SCHEMA_VERSION,
            });
        }
        Ok(view)
    }

    /// Computes the topology hash over every field except `topology_hash`
    /// itself.
    pub fn compute_topology_hash(&self) -> Hash64 {
        Hash64::of(self.canonical_topology_text().as_bytes())
    }

    /// Sets [`Self::topology_hash`] to the hash of the current *other* fields.
    ///
    /// Must be called after every other field is final; editing a field
    /// afterwards makes the hash stale.
    pub fn seal(&mut self) {
        self.topology_hash = self.compute_topology_hash();
    }

    /// Returns whether the stored `topology_hash` still matches the rest of
    /// the view.
    pub fn verify_topology_hash(&self) -> bool {
        self.topology_hash == self.compute_topology_hash()
    }

    /// Returns the canonical text hashed into [`Self::topology_hash`].
    ///
    /// The text is line-oriented and contains every semantic field, so two
    /// views with the same hash describe byte-identical topologies. The
    /// `topology_hash` field itself is excluded.
    pub fn canonical_topology_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "system schema={}", self.schema_version);
        let _ = writeln!(out, "generation={}", self.rticx_generation);
        let _ = writeln!(out, "layout_hash={}", self.layout_hash);

        for app in &self.apps {
            let _ = writeln!(
                out,
                "app package={} target={:?}:{} source_hash={} core_ids={:?} external_cores={:?}",
                app.package,
                app.target.kind,
                app.target.name,
                app.source_hash,
                app.core_ids,
                app.external_cores
            );
        }
        for core in &self.cores {
            let _ = writeln!(
                out,
                "core global_id={} app={} local_index={}",
                core.global_id, core.app, core.local_index
            );
        }
        for ty in &self.types {
            let _ = writeln!(
                out,
                "type name={} kind={:?} size={} align={}",
                ty.name, ty.kind, ty.size, ty.align
            );
            for field in &ty.fields {
                let _ = writeln!(
                    out,
                    "field type={} name={} ty={} offset={}",
                    ty.name, field.name, field.ty, field.offset
                );
            }
            for variant in &ty.variants {
                let _ = writeln!(
                    out,
                    "variant type={} name={} discriminant={}",
                    ty.name, variant.name, variant.discriminant
                );
            }
        }
        for task in &self.tasks {
            let _ = writeln!(
                out,
                "task id={} name={} receiver_core={} spawner_core={} priority={} \
                 capacity={} input_type={} fifo={{source={} target={} offset={} elem_size={} depth={}}}",
                task.id,
                task.name,
                task.receiver_core,
                task.spawner_core,
                task.priority,
                task.capacity,
                task.input_type,
                task.fifo.source,
                task.fifo.target,
                task.fifo.offset,
                task.fifo.elem_size,
                task.fifo.depth
            );
        }
        for region in &self.regions {
            let _ = writeln!(
                out,
                "region source={} target={} base_from_source=0x{:08x} \
                 base_from_target=0x{:08x} size={}",
                region.source,
                region.target,
                region.base_from_source,
                region.base_from_target,
                region.size
            );
        }
        for doorbell in &self.doorbells {
            let _ = writeln!(
                out,
                "doorbell source={} target={} priority={} line={}",
                doorbell.source, doorbell.target, doorbell.priority, doorbell.line
            );
        }
        out
    }
}

/// One application (binary) of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Cargo package name.
    pub package: String,
    /// Cargo target.
    pub target: TargetRef,
    /// Hash of the application source, taken from its manifest.
    pub source_hash: Hash64,
    /// Local core index -> global core id. Empty when the application did not
    /// declare one and the driver used the identity mapping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub core_ids: Vec<u32>,
    /// Global ids of cores in other binaries visible to this application.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_cores: Vec<u32>,
}

/// A global core id resolved to its application and local index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreEntry {
    /// Globally unique core id.
    pub global_id: u32,
    /// Package owning the core.
    pub app: String,
    /// Local core index inside the application (`core_ids[local_index]`).
    pub local_index: u32,
}

/// A message or enum type declared in the IDL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeEntry {
    /// Type name.
    pub name: String,
    /// Whether this is a `#[repr(C)]` message or a `repr(u32)` enum.
    pub kind: TypeKind,
    /// Canonical size in bytes.
    pub size: u32,
    /// Canonical alignment in bytes.
    pub align: u32,
    /// Message fields in declaration order; empty for enums.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldEntry>,
    /// Enum variants; empty for messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<VariantEntry>,
}

/// Whether a type is a message or an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeKind {
    /// A `#[repr(C)]` struct.
    Message,
    /// A `repr(u32)` enum.
    Enum,
}

/// One message field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldEntry {
    /// Field name.
    pub name: String,
    /// Canonical type expression (`u32`, `[i16; 8]`, `EncryptReq`, ...).
    pub ty: String,
    /// Byte offset from the start of the message.
    pub offset: u32,
}

/// One enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantEntry {
    /// Variant name.
    pub name: String,
    /// `u32` discriminant.
    pub discriminant: u32,
}

/// A cross-binary task: one per-task FIFO plus its priority line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEntry {
    /// Deterministic task id, unique across the project.
    pub id: u32,
    /// Task name (shared by the receiver and its generated producer stub).
    pub name: String,
    /// Global id of the core that executes the task.
    pub receiver_core: u32,
    /// Global id of the single core that spawns the task (v1: exactly one
    /// producer per task).
    pub spawner_core: u32,
    /// Priority line on the receiver core.
    pub priority: u16,
    /// Number of pending inputs the FIFO can hold.
    pub capacity: usize,
    /// IDL name of the input type.
    pub input_type: String,
    /// FIFO allocation inside the `(source -> target)` region.
    pub fifo: FifoEntry,
}

/// The shared-memory FIFO allocation of one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FifoEntry {
    /// Producer (spawner) global core id.
    pub source: u32,
    /// Consumer (receiver) global core id.
    pub target: u32,
    /// Byte offset inside the `(source, target)` region.
    pub offset: u32,
    /// Size of one element in bytes (canonical input-type size).
    pub elem_size: u32,
    /// Ring depth; `capacity + 1` (one slot is always left empty).
    pub depth: u32,
}

/// A distro-declared shared-memory region for one direction.
///
/// Since M6.9-T4 the merge model is pool-based ([`PoolEntry`]); this
/// per-direction shape is the schema-1 `system.json` rendering emitted until
/// M6.9-T5 replaces `regions[]` with `pools[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionEntry {
    /// Producer global core id.
    pub source: u32,
    /// Consumer global core id.
    pub target: u32,
    /// Region base address as seen by the source core (hex in JSON).
    #[serde(with = "hex_u32")]
    pub base_from_source: u32,
    /// Region base address as seen by the target core (hex in JSON).
    #[serde(with = "hex_u32")]
    pub base_from_target: u32,
    /// Region size in bytes.
    pub size: u32,
}

/// A distro IPC pool shared by the two directions of a dual (M6.9-T4).
///
/// The two directions `A -> B` and `B -> A` allocate their FIFOs inside the
/// same physical block, so the pool carries one `budget` shared by both and
/// the `used` byte count after allocation. `core_a`/`core_b` are the two
/// global core ids in ascending order; `base_from_a`/`base_from_b` are each
/// core's view of the same memory (aliases are allowed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolEntry {
    /// Symbolic, distro-defined pool id.
    pub id: String,
    /// Lower global core id of the dual.
    pub core_a: u32,
    /// Higher global core id of the dual.
    pub core_b: u32,
    /// Pool base address as seen by `core_a` (hex in JSON).
    #[serde(with = "hex_u32")]
    pub base_from_a: u32,
    /// Pool base address as seen by `core_b` (hex in JSON).
    #[serde(with = "hex_u32")]
    pub base_from_b: u32,
    /// Bytes the distribution reserves for both directions of the dual.
    pub budget: u32,
    /// Bytes used by the FIFOs of both directions.
    pub used: u32,
}

/// A doorbell line pended when a `(source, target)` priority line has work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoorbellEntry {
    /// Producer global core id.
    pub source: u32,
    /// Consumer global core id.
    pub target: u32,
    /// Priority line on the target core.
    pub priority: u16,
    /// Distro-specific doorbell line index.
    pub line: u32,
}

/// Serializes `u32` addresses as `0x`-prefixed, 8-digit lowercase hex strings.
mod hex_u32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{value:08x}"))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u32, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        parse(&text).map_err(serde::de::Error::custom)
    }

    fn parse(text: &str) -> Result<u32, String> {
        let digits = text
            .strip_prefix("0x")
            .or_else(|| text.strip_prefix("0X"))
            .unwrap_or(text);
        if digits.is_empty() || digits.len() > 8 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "invalid address `{text}`; expected `0x` followed by up to 8 hexadecimal digits"
            ));
        }
        u32::from_str_radix(digits, 16)
            .map_err(|_| format!("invalid address `{text}`; value does not fit in 32 bits"))
    }
}
