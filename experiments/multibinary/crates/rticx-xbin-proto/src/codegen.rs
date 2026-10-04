//! `ipc_types` module generator.
//!
//! Given a validated [`IpcTypes`], this module emits the body of the generated
//! `ipc_types.rs` file, which the cross-binary pass re-emits into each
//! application's `#[app]` module as `pub mod ipc_types { … }`:
//!
//! - one `#[repr(C)]` struct / `#[repr(u32)]` enum per IDL declaration;
//! - the canonical `SIZE_*` / `ALIGN_*` / `OFF_*` constants;
//! - one `unsafe impl CrossCoreMessage` per type (the trait is injected next to
//!   the module by the pass);
//! - a `LAYOUT_HASH`;
//! - a block of `const` assertions checked by the target compiler.
//!
//! The generated text carries **no inner attributes** and **no
//! `rticx-xbin-rt` dependency**: the pass wraps it in a `pub mod ipc_types`
//! with the `CrossCoreMessage` trait and a `use super::CrossCoreMessage;`.
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

/// File name of the generated `ipc_types` module, next to `system.json` under
/// `<project root>/target/rticx-xbin/`.
pub const IPC_TYPES_FILE: &str = "ipc_types.rs";

/// What happened to the generated file during [`write_if_changed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// The file did not exist and was written.
    Created,
    /// The file existed with different contents and was overwritten.
    Updated,
    /// The file already had exactly the generated contents and was left
    /// untouched.
    Unchanged,
}

/// Generates the complete `ipc_types.rs` module file for `idl`.
pub fn generate_module(idl: &IpcTypes) -> Result<String, CodegenError> {
    let layouts = Layouts::of(idl)?;
    generate_module_body(idl, &layouts)
}

/// Writes `contents` to `path`, creating parent directories, but leaves the
/// file untouched when it already has exactly those contents.
///
/// This keeps an unchanged IDL from bumping modification times, and therefore
/// from triggering rebuilds of dependents. The outcome is returned for
/// reporting.
pub fn write_if_changed(path: &Path, contents: &str) -> io::Result<FileChange> {
    match std::fs::read_to_string(path) {
        Ok(existing) if existing == contents => Ok(FileChange::Unchanged),
        Ok(_) => {
            std::fs::write(path, contents)?;
            Ok(FileChange::Updated)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, contents)?;
            Ok(FileChange::Created)
        }
        Err(error) => Err(error),
    }
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

fn generate_module_body(idl: &IpcTypes, layouts: &Layouts) -> Result<String, CodegenError> {
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
        let _ = writeln!(out, "unsafe impl CrossCoreMessage for {name} {{}}\n");
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
