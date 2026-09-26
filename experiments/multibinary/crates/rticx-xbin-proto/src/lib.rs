//! Protocol support for the RTICX multi-binary extension.
//!
//! Responsibilities (see `multibinary-multicore-plan.md` §6 and §9):
//!
//! - [`idl`]: parse `ipc-types.toml` and validate the 32-bit-safe type subset;
//! - [`project`]: parse `rticx.toml`, the single source of project topology;
//! - [`layout`]: compute the canonical little-endian layout (align cap 4) of
//!   every declared type;
//! - [`codegen`]: generate the `ipc-types` crate (types, marker impls,
//!   layout constants, compile-time assertions and `LAYOUT_HASH`).
//!
//! The crate is host-only: the driver and the compilation pass link it, while
//! the *generated* crate depends on `rticx-xbin-rt` instead.

#![forbid(unsafe_code)]

pub mod codegen;
pub mod error;
pub mod idl;
pub mod layout;
pub mod project;

pub use codegen::{
    CodegenOptions, GeneratedCrate, GeneratedFile, RtDependency, canonical_layout_text,
    generate_crate, layout_hash,
};
pub use error::{CodegenError, IdlError, LayoutError, ProjectError};
pub use idl::{
    Enum, EnumVariant, Field, FieldType, IpcTypes, Message, PrimTy, SCHEMA_VERSION, parse_idl_file,
    parse_idl_str,
};
pub use layout::{EnumLayout, FieldLayout, Layout, Layouts, MessageLayout, VariantLayout};
pub use project::{
    Application, PROJECT_SCHEMA_VERSION, ProjectConfig, Region, RegionKey, Target, TargetKind,
    parse_project_file, parse_project_str,
};
