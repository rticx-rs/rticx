//! Canonical layout engine for IDL types.
//!
//! The canonical layout is the ABI contract between the two binaries (and
//! between any two cores): `#[repr(C)]`-style field ordering, little-endian
//! scalars, natural alignment capped at [`MAX_ALIGN`] (4) and trailing
//! padding. It is deliberately computed by *this* crate rather than by the
//! host compiler, because the host is not necessarily the target: the same
//! canonical values are embedded in the generated `ipc_types` module and
//! checked there with `core::mem::{size_of, align_of, offset_of}`.
//!
//! Because the supported subset contains no pointers, the canonical sizes and
//! offsets are identical on 32-bit and 64-bit targets.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::LayoutError;
use crate::idl::{FieldType, IpcTypes, Message};

/// Maximum alignment of any canonical type, in bytes.
///
/// Every supported scalar is naturally aligned to at most 4 bytes (the
/// widest is `u32`/`i32`/`f32`), so this cap only documents and enforces the
/// invariant.
pub const MAX_ALIGN: usize = 4;

/// Canonical size and alignment of a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Size in bytes, including padding.
    pub size: usize,
    /// Alignment in bytes, never above [`MAX_ALIGN`].
    pub align: usize,
}

/// Canonical layout of a single message field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    /// Field name.
    pub name: String,
    /// Field type.
    pub ty: FieldType,
    /// Byte offset from the start of the containing message.
    pub offset: usize,
    /// Size and alignment of the field itself.
    pub layout: Layout,
}

/// Canonical layout of a message (`#[repr(C)]` struct).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageLayout {
    /// Message name.
    pub name: String,
    /// Size and alignment of the whole message.
    pub layout: Layout,
    /// Field layouts in declaration order.
    pub fields: Vec<FieldLayout>,
}

/// Canonical layout of a `repr(u32)` enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumLayout {
    /// Enum name.
    pub name: String,
    /// Size and alignment of the enum (always 4).
    pub layout: Layout,
    /// Variants with their discriminants.
    pub variants: Vec<VariantLayout>,
}

/// A single enum variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantLayout {
    /// Variant name.
    pub name: String,
    /// `u32` discriminant.
    pub discriminant: u32,
}

/// Canonical layouts of every type of an [`IpcTypes`] document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layouts {
    messages: BTreeMap<String, MessageLayout>,
    enums: BTreeMap<String, EnumLayout>,
}

impl Layouts {
    /// Computes the canonical layout of every message and enum in `idl`.
    pub fn of(idl: &IpcTypes) -> Result<Self, LayoutError> {
        let mut computer = Computer {
            idl,
            cache: BTreeMap::new(),
            visiting: BTreeSet::new(),
            stack: Vec::new(),
        };

        let mut messages = BTreeMap::new();
        for (name, message) in idl.messages() {
            messages.insert(name.clone(), computer.message_layout(name, message)?);
        }

        let mut enums = BTreeMap::new();
        for (name, enumeration) in idl.enums() {
            let variants = enumeration
                .variants()
                .iter()
                .map(|variant| VariantLayout {
                    name: variant.name().to_string(),
                    discriminant: variant.discriminant(),
                })
                .collect();
            enums.insert(
                name.clone(),
                EnumLayout {
                    name: name.clone(),
                    layout: enum_layout(),
                    variants,
                },
            );
        }

        Ok(Self { messages, enums })
    }

    /// Returns every message layout, keyed by name.
    pub fn messages(&self) -> &BTreeMap<String, MessageLayout> {
        &self.messages
    }

    /// Returns every enum layout, keyed by name.
    pub fn enums(&self) -> &BTreeMap<String, EnumLayout> {
        &self.enums
    }

    /// Returns the layout of the message named `name`, if any.
    pub fn message(&self, name: &str) -> Option<&MessageLayout> {
        self.messages.get(name)
    }

    /// Returns the layout of the enum named `name`, if any.
    pub fn get_enum(&self, name: &str) -> Option<&EnumLayout> {
        self.enums.get(name)
    }
}

/// Layout of every `repr(u32)` enum.
const fn enum_layout() -> Layout {
    Layout { size: 4, align: 4 }
}

struct Computer<'a> {
    idl: &'a IpcTypes,
    cache: BTreeMap<String, Layout>,
    visiting: BTreeSet<String>,
    stack: Vec<String>,
}

impl Computer<'_> {
    fn message_layout(
        &mut self,
        name: &str,
        message: &Message,
    ) -> Result<MessageLayout, LayoutError> {
        let mut offset = 0usize;
        let mut struct_align = 1usize;
        let mut fields = Vec::with_capacity(message.fields().len());

        for field in message.fields() {
            let layout = self.type_layout(field.ty(), name)?;
            offset = align_up(offset, layout.align).ok_or_else(|| LayoutError::Overflow {
                name: name.to_string(),
            })?;
            fields.push(FieldLayout {
                name: field.name().to_string(),
                ty: field.ty().clone(),
                offset,
                layout,
            });
            offset = offset
                .checked_add(layout.size)
                .ok_or_else(|| LayoutError::Overflow {
                    name: name.to_string(),
                })?;
            struct_align = struct_align.max(layout.align).min(MAX_ALIGN);
        }

        let size = align_up(offset, struct_align).ok_or_else(|| LayoutError::Overflow {
            name: name.to_string(),
        })?;

        Ok(MessageLayout {
            name: name.to_string(),
            layout: Layout {
                size,
                align: struct_align,
            },
            fields,
        })
    }

    fn type_layout(&mut self, ty: &FieldType, context: &str) -> Result<Layout, LayoutError> {
        match ty {
            FieldType::Prim(prim) => Ok(Layout {
                size: prim.size(),
                align: prim.align().min(MAX_ALIGN),
            }),
            FieldType::Named(name) => self.named_layout(name),
            FieldType::Array(element, length) => {
                let element_layout = self.type_layout(element, context)?;
                let size = element_layout.size.checked_mul(*length).ok_or_else(|| {
                    LayoutError::Overflow {
                        name: context.to_string(),
                    }
                })?;
                Ok(Layout {
                    size,
                    align: element_layout.align.min(MAX_ALIGN),
                })
            }
        }
    }

    fn named_layout(&mut self, name: &str) -> Result<Layout, LayoutError> {
        if let Some(layout) = self.cache.get(name) {
            return Ok(*layout);
        }
        if self.idl.get_enum(name).is_some() {
            let layout = enum_layout();
            self.cache.insert(name.to_string(), layout);
            return Ok(layout);
        }
        let Some(message) = self.idl.message(name) else {
            return Err(LayoutError::UnknownType {
                name: name.to_string(),
            });
        };
        if self.visiting.contains(name) {
            let start = self
                .stack
                .iter()
                .position(|entry| entry == name)
                .unwrap_or(0);
            let mut cycle = self.stack[start..].to_vec();
            cycle.push(name.to_string());
            return Err(LayoutError::Cycle {
                cycle: cycle.join(" -> "),
            });
        }

        self.visiting.insert(name.to_string());
        self.stack.push(name.to_string());
        let layout = self.message_layout(name, message)?.layout;
        self.stack.pop();
        self.visiting.remove(name);
        self.cache.insert(name.to_string(), layout);
        Ok(layout)
    }
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}
