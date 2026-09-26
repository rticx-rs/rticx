//! `ipc-types` crate generator.
//!
//! Given a validated [`IpcTypes`], this module emits the complete generated
//! crate:
//!
//! - `Cargo.toml`, depending on `rticx-xbin-rt` (path or version);
//! - `src/lib.rs` with one `#[repr(C)]` struct / `#[repr(u32)]` enum per IDL
//!   declaration, the canonical `SIZE_*` / `ALIGN_*` / `OFF_*` constants, one
//!   `unsafe impl CrossCoreMessage` per type, a `LAYOUT_HASH`, and a block of
//!   `const` assertions checked by the target compiler.
//!
//! Output is fully deterministic: items are ordered by name and the canonical
//! text hashed into `LAYOUT_HASH` contains no incidental formatting.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::Path;

use heck::ToShoutySnakeCase;

use crate::error::CodegenError;
use crate::idl::{FieldType, IpcTypes};
use crate::layout::{Layouts, MAX_ALIGN};

/// Package name of the generated crate.
pub const GENERATED_CRATE_NAME: &str = "ipc-types";

/// Default in-tree relative path to `rticx-xbin-rt`.
///
/// This is only a convenience for tests and tooling that run from inside the
/// experimental workspace; the driver computes a project-relative (or later,
/// registry) dependency and passes it via [`CodegenOptions`].
pub const DEFAULT_RT_PATH: &str = "../../crates/rticx-xbin-rt";

/// How the generated crate depends on `rticx-xbin-rt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtDependency {
    /// In-tree development: `rticx-xbin-rt = { path = "…" }`.
    Path(String),
    /// After extraction: `rticx-xbin-rt = "…"`.
    Version(String),
}

impl Default for RtDependency {
    fn default() -> Self {
        Self::Path(DEFAULT_RT_PATH.to_string())
    }
}

/// Options for [`generate_crate`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodegenOptions {
    /// Dependency specification for the runtime crate.
    pub rt_dependency: RtDependency,
}

/// A single generated file, with a `/`-separated path relative to the
/// generated crate root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Relative path, for example `src/lib.rs`.
    pub path: String,
    /// Full file contents.
    pub contents: String,
}

/// The set of files making up the generated crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCrate {
    /// Generated files, in a stable order.
    pub files: Vec<GeneratedFile>,
}

impl GeneratedCrate {
    /// Returns the contents of the file at `path`, if generated.
    pub fn file(&self, path: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.contents.as_str())
    }

    /// Writes every file below `root`, creating parent directories.
    pub fn write_to(&self, root: &Path) -> io::Result<()> {
        for file in &self.files {
            let path = root.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, &file.contents)?;
        }
        Ok(())
    }
}

/// Generates the complete `ipc-types` crate for `idl`.
pub fn generate_crate(
    idl: &IpcTypes,
    options: &CodegenOptions,
) -> Result<GeneratedCrate, CodegenError> {
    let layouts = Layouts::of(idl)?;
    let lib_rs = generate_lib_rs(idl, &layouts)?;
    let cargo_toml = generate_cargo_toml(options);
    Ok(GeneratedCrate {
        files: vec![
            GeneratedFile {
                path: "Cargo.toml".to_string(),
                contents: cargo_toml,
            },
            GeneratedFile {
                path: "src/lib.rs".to_string(),
                contents: lib_rs,
            },
        ],
    })
}

/// Builds the canonical layout description that is hashed into `LAYOUT_HASH`.
///
/// The text contains every size, alignment, offset and discriminator, so two
/// IDL files producing the same hash have byte-identical cross-core layouts.
pub fn canonical_layout_text(idl: &IpcTypes) -> Result<String, CodegenError> {
    let layouts = Layouts::of(idl)?;
    let mut text = String::new();
    let _ = writeln!(text, "ipc-types layout v1");
    let _ = writeln!(text, "schema={}", idl.schema());

    for (name, message) in layouts.messages() {
        let _ = writeln!(
            text,
            "message {name} align={} size={}",
            message.layout.align, message.layout.size
        );
        for field in &message.fields {
            let _ = writeln!(
                text,
                "field {name}.{} type={} offset={} size={} align={}",
                field.name, field.ty, field.offset, field.layout.size, field.layout.align
            );
        }
    }
    for (name, enumeration) in layouts.enums() {
        let _ = writeln!(
            text,
            "enum {name} align={} size={}",
            enumeration.layout.align, enumeration.layout.size
        );
        for variant in &enumeration.variants {
            let _ = writeln!(
                text,
                "variant {name}::{}={}",
                variant.name, variant.discriminant
            );
        }
    }
    Ok(text)
}

/// Computes the FNV-1a 64-bit hash of [`canonical_layout_text`].
pub fn layout_hash(idl: &IpcTypes) -> Result<u64, CodegenError> {
    Ok(fnv1a64(canonical_layout_text(idl)?.as_bytes()))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn generate_cargo_toml(options: &CodegenOptions) -> String {
    let dependency = match &options.rt_dependency {
        RtDependency::Path(path) => format!("rticx-xbin-rt = {{ path = \"{path}\" }}"),
        RtDependency::Version(version) => format!("rticx-xbin-rt = \"{version}\""),
    };
    format!(
        "# @generated by `cargo xbin sync` from `ipc-types.toml` -- do not edit\n\
         # manually; run `cargo xbin sync` instead.\n\
         \n\
         [package]\n\
         name = \"{GENERATED_CRATE_NAME}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [dependencies]\n\
         {dependency}\n"
    )
}

fn generate_lib_rs(idl: &IpcTypes, layouts: &Layouts) -> Result<String, CodegenError> {
    let names: Vec<String> = interleaved_names(idl);
    let mut constants = ConstRegistry::default();

    let mut out = String::new();
    out.push_str(HEADER);

    for name in &names {
        if let Some(message) = idl.message(name) {
            let _ = writeln!(out, "#[repr(C)]");
            let _ = writeln!(out, "#[derive(Clone, Copy)]");
            let _ = writeln!(out, "pub struct {} {{", message.name());
            for field in message.fields() {
                let _ = writeln!(
                    out,
                    "    pub {}: {},",
                    field.name(),
                    render_field_type(field.ty())
                );
            }
            let _ = writeln!(out, "}}\n");
        } else if let Some(enumeration) = idl.get_enum(name) {
            let _ = writeln!(out, "#[repr(u32)]");
            let _ = writeln!(out, "#[derive(Clone, Copy)]");
            let _ = writeln!(out, "pub enum {} {{", enumeration.name());
            for variant in enumeration.variants() {
                let _ = writeln!(out, "    {} = {},", variant.name(), variant.discriminant());
            }
            let _ = writeln!(out, "}}\n");
        }
    }

    for name in &names {
        if let Some(message) = layouts.message(name) {
            let prefix = name.to_shouty_snake_case();
            constants.add(format!("SIZE_{prefix}"), &format!("message `{name}`"))?;
            let _ = writeln!(
                out,
                "pub const SIZE_{prefix}: usize = {};",
                message.layout.size
            );
            constants.add(format!("ALIGN_{prefix}"), &format!("message `{name}`"))?;
            let _ = writeln!(
                out,
                "pub const ALIGN_{prefix}: usize = {};",
                message.layout.align
            );
            for field in &message.fields {
                let constant = format!("OFF_{prefix}_{}", field.name.to_shouty_snake_case());
                constants.add(constant.clone(), &format!("field `{name}.{}`", field.name))?;
                let _ = writeln!(out, "pub const {constant}: usize = {};", field.offset);
            }
        } else if let Some(enumeration) = layouts.get_enum(name) {
            let prefix = name.to_shouty_snake_case();
            constants.add(format!("SIZE_{prefix}"), &format!("enum `{name}`"))?;
            let _ = writeln!(
                out,
                "pub const SIZE_{prefix}: usize = {};",
                enumeration.layout.size
            );
            constants.add(format!("ALIGN_{prefix}"), &format!("enum `{name}`"))?;
            let _ = writeln!(
                out,
                "pub const ALIGN_{prefix}: usize = {};",
                enumeration.layout.align
            );
        }
    }

    let hash = fnv1a64(canonical_layout_text(idl)?.as_bytes());
    let _ = writeln!(out, "pub const LAYOUT_HASH: u64 = 0x{hash:016x};\n");

    for name in &names {
        let _ = writeln!(
            out,
            "unsafe impl rticx_xbin_rt::CrossCoreMessage for {name} {{}}\n"
        );
    }

    let _ = writeln!(out, "const _: () = {{");
    for name in &names {
        if let Some(message) = layouts.message(name) {
            let prefix = name.to_shouty_snake_case();
            let _ = writeln!(out, "    // message `{name}`");
            let _ = writeln!(
                out,
                "    assert!(core::mem::size_of::<{name}>() == SIZE_{prefix});"
            );
            let _ = writeln!(
                out,
                "    assert!(core::mem::align_of::<{name}>() == ALIGN_{prefix});"
            );
            for field in &message.fields {
                let constant = format!("OFF_{prefix}_{}", field.name.to_shouty_snake_case());
                let _ = writeln!(
                    out,
                    "    assert!(core::mem::offset_of!({name}, {}) == {constant});",
                    field.name
                );
            }
        } else if let Some(enumeration) = layouts.get_enum(name) {
            let prefix = name.to_shouty_snake_case();
            let _ = writeln!(out, "    // enum `{name}`");
            let _ = writeln!(
                out,
                "    assert!(core::mem::size_of::<{name}>() == SIZE_{prefix});"
            );
            let _ = writeln!(
                out,
                "    assert!(core::mem::align_of::<{name}>() == ALIGN_{prefix});"
            );
            for variant in &enumeration.variants {
                let _ = writeln!(
                    out,
                    "    assert!(({name}::{} as u32) == {});",
                    variant.name, variant.discriminant
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "    // The canonical layout is little-endian. The supported"
    );
    let _ = writeln!(
        out,
        "    // subset is pointer-free, so it is identical on 32-bit and"
    );
    let _ = writeln!(
        out,
        "    // 64-bit targets (alignment is capped at {MAX_ALIGN})."
    );
    let _ = writeln!(out, "    assert!(cfg!(target_endian = \"little\"));");
    let _ = writeln!(out, "}};");

    Ok(out)
}

const HEADER: &str = "\
// @generated by `cargo xbin sync` from `ipc-types.toml` -- do not edit
// manually; run `cargo xbin sync` instead.
//
// Canonical cross-core message types for the RTICX multi-binary extension.
// The `SIZE_*`, `ALIGN_*` and `OFF_*` constants describe the canonical
// 32-bit, little-endian layout; the const assertions at the bottom check that
// the compiler agrees with them on the target core.

#![no_std]
#![allow(non_camel_case_types, non_snake_case)]

";

fn interleaved_names(idl: &IpcTypes) -> Vec<String> {
    let mut names: Vec<String> = idl
        .messages()
        .keys()
        .chain(idl.enums().keys())
        .cloned()
        .collect();
    names.sort();
    names
}

fn render_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::Prim(prim) => prim.name().to_string(),
        FieldType::Named(name) => name.clone(),
        FieldType::Array(element, length) => {
            format!("[{}; {length}]", render_field_type(element))
        }
    }
}

/// Tracks generated constant names so collisions (for example `FooBar` and
/// `Foo_Bar` both mapping to `FOO_BAR`) are hard errors instead of duplicate
/// definitions.
#[derive(Default)]
struct ConstRegistry {
    used: BTreeMap<String, String>,
}

impl ConstRegistry {
    fn add(&mut self, constant: String, declaration: &str) -> Result<(), CodegenError> {
        if let Some(first) = self.used.get(&constant) {
            return Err(CodegenError::ConstCollision {
                first: first.clone(),
                second: declaration.to_string(),
                constant,
            });
        }
        self.used.insert(constant, declaration.to_string());
        Ok(())
    }
}
