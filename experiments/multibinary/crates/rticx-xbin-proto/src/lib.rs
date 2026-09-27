//! Protocol support for the RTICX multi-binary extension.
//!
//! Responsibilities (see `multibinary-multicore-plan.md` §6 and §9):
//!
//! - [`idl`]: parse `ipc-types.toml` and validate the 32-bit-safe type subset;
//! - [`project`]: parse `rticx.toml`, the single source of project topology;
//! - [`layout`]: compute the canonical little-endian layout (align cap 4) of
//!   every declared type;
//! - [`manifest`]: the per-application `<target>.xbin.json` metadata written
//!   by the compilation pass in phase 1;
//! - [`merge`]: merge the manifests into a validated whole-project view
//!   (receiver topology, priority disjointness, type existence, region fit,
//!   core-id consistency);
//! - [`system`]: the driver-generated `system.json` system view;
//! - [`alloc`]: the M1-T6 emission of that view (FIFOs, doorbell lines,
//!   layout hash, sealed topology hash);
//! - [`fifo`]: the canonical in-region FIFO image and sizing;
//! - [`hash`]: the deterministic FNV-1a 64-bit hash used for source, layout
//!   and topology hashes;
//! - [`codegen`]: generate the `ipc-types` crate (types, marker impls,
//!   layout constants, compile-time assertions and `LAYOUT_HASH`).
//!
//! The crate is host-only: the driver and the compilation pass link it, while
//! the *generated* crate depends on `rticx-xbin-rt` instead.

#![forbid(unsafe_code)]

pub mod alloc;
pub mod codegen;
pub mod error;
pub mod fifo;
pub mod hash;
pub mod idl;
pub mod layout;
pub mod manifest;
pub mod merge;
pub mod project;
pub mod system;

pub use alloc::system_view;
pub use codegen::{
    CodegenOptions, CrateStatus, FileChange, FileStatus, GENERATED_CRATE_NAME, GeneratedCrate,
    GeneratedFile, RtDependency, canonical_layout_text, generate_crate, layout_hash,
};
pub use error::{
    CodegenError, HashParseError, IdlError, LayoutError, ManifestError, MergeError, ProjectError,
    SystemError,
};
pub use fifo::{FIFO_ALIGN, FIFO_HEADER, FIFO_INDEX_STRIDE, align_up, fifo_depth, fifo_size};
pub use hash::Hash64;
pub use idl::{
    Enum, EnumVariant, Field, FieldType, IpcTypes, Message, PrimTy, SCHEMA_VERSION, parse_idl_file,
    parse_idl_str,
};
pub use layout::{EnumLayout, FieldLayout, Layout, Layouts, MessageLayout, VariantLayout};
pub use manifest::{
    AppManifest, MANIFEST_FILE_SUFFIX, MANIFEST_SCHEMA_VERSION, ReceiverDecl, TargetRef,
    simple_type_name,
};
pub use merge::{MergedProject, MergedTask, merge_project};
pub use project::{
    Application, PROJECT_SCHEMA_VERSION, ProjectConfig, Region, RegionKey, Target, TargetKind,
    parse_project_file, parse_project_str,
};
pub use system::{
    AppEntry, CoreEntry, DoorbellEntry, FieldEntry, FifoEntry, RTICX_GENERATION, RegionEntry,
    SYSTEM_SCHEMA_VERSION, SystemView, TaskEntry, TypeEntry, TypeKind, VariantEntry,
};
