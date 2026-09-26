//! `ipc-types.toml` parsing and 32-bit-safe type subset validation.
//!
//! # IDL format (schema 1)
//!
//! ```toml
//! schema = 1
//!
//! [message.EncryptReq]
//! fields = { addr = "u32", len = "u32", key = "u32" }
//!
//! [message.SensorBatch]
//! fields = { tag = "u8", xs = "[i16; 8]", state = "State" }
//!
//! # Explicit discriminants:
//! [enum.State]
//! variants = { Idle = 0, Busy = 1, Failed = 7 }
//!
//! # Or implicit 0..N discriminants (declaration order):
//! [enum.Kind]
//! variants = ["Fast", "Slow"]
//! ```
//!
//! Only *data types* live in the IDL. Task names, priorities and capacities
//! are application syntax and are merged by the driver in phase 1.
//!
//! # Supported subset (v1)
//!
//! - integers `u8 u16 u32 i8 i16 i32`, `f32`;
//! - fixed arrays `[T; N]` with `N >= 1`;
//! - references to other messages or enums declared in the same file;
//! - C-like enums with `repr(u32)` semantics.
//!
//! Everything else (`u64/i64/f64/...`, `usize`, `bool`, `char`, strings,
//! pointers, references, tuples, generics, zero-sized messages, recursive
//! messages) is rejected with a message naming the offending TOML path.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::error::{IdlError, kind_of};

/// Schema version understood by this crate.
pub const SCHEMA_VERSION: u32 = 1;

/// A parsed and validated `ipc-types.toml`.
///
/// Values can only be produced by [`parse_idl_str`] / [`parse_idl_file`], so
/// the invariants checked during validation (known references, acyclic
/// messages, valid identifiers, ...) hold for every instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcTypes {
    schema: u32,
    messages: BTreeMap<String, Message>,
    enums: BTreeMap<String, Enum>,
}

impl IpcTypes {
    /// Returns the IDL schema version (always [`SCHEMA_VERSION`] for parsed
    /// values).
    pub fn schema(&self) -> u32 {
        self.schema
    }

    /// Returns every message, keyed by name (sorted).
    pub fn messages(&self) -> &BTreeMap<String, Message> {
        &self.messages
    }

    /// Returns every enum, keyed by name (sorted).
    pub fn enums(&self) -> &BTreeMap<String, Enum> {
        &self.enums
    }

    /// Returns the message named `name`, if any.
    pub fn message(&self, name: &str) -> Option<&Message> {
        self.messages.get(name)
    }

    /// Returns the enum named `name`, if any.
    pub fn get_enum(&self, name: &str) -> Option<&Enum> {
        self.enums.get(name)
    }

    /// Returns whether `name` is a declared message or enum.
    pub fn has_type(&self, name: &str) -> bool {
        self.messages.contains_key(name) || self.enums.contains_key(name)
    }

    fn validate(&self) -> Result<(), IdlError> {
        for (message_name, message) in &self.messages {
            for field in &message.fields {
                for named in field.ty.named_types() {
                    if !self.has_type(named) {
                        return Err(IdlError::invalid(format!(
                            "`message.{message_name}.fields.{}` references unknown type `{named}`; \
                             declare it as `[message.{named}]` or `[enum.{named}]`",
                            field.name
                        )));
                    }
                }
            }
        }

        let mut state: BTreeMap<&str, Visit> = BTreeMap::new();
        let mut stack: Vec<&str> = Vec::new();
        for name in self.messages.keys() {
            self.visit(name, &mut state, &mut stack)?;
        }
        Ok(())
    }

    fn visit<'a>(
        &'a self,
        name: &'a str,
        state: &mut BTreeMap<&'a str, Visit>,
        stack: &mut Vec<&'a str>,
    ) -> Result<(), IdlError> {
        match state.get(name) {
            Some(Visit::Done) => return Ok(()),
            Some(Visit::Active) => {
                let start = stack.iter().position(|entry| *entry == name).unwrap_or(0);
                let mut cycle: Vec<&str> = stack[start..].to_vec();
                cycle.push(name);
                return Err(IdlError::invalid(format!(
                    "cyclic message reference: {}",
                    cycle.join(" -> ")
                )));
            }
            None => {}
        }

        state.insert(name, Visit::Active);
        stack.push(name);
        for field in &self.messages[name].fields {
            for named in field.ty.named_types() {
                if self.messages.contains_key(named) {
                    self.visit(named, state, stack)?;
                }
            }
        }
        stack.pop();
        state.insert(name, Visit::Done);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    Active,
    Done,
}

/// A `#[repr(C)]` message: a named sequence of fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    name: String,
    fields: Vec<Field>,
}

impl Message {
    /// Returns the message name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the fields in declaration order.
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Returns the field named `name`, if any.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// A single message field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    name: String,
    ty: FieldType,
}

impl Field {
    /// Returns the field name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the field type.
    pub fn ty(&self) -> &FieldType {
        &self.ty
    }
}

/// A C-like enum with `repr(u32)` semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
    name: String,
    variants: Vec<EnumVariant>,
}

impl Enum {
    /// Returns the enum name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the variants, sorted by discriminant (ties broken by name) for
    /// explicit-discriminant tables, or in declaration order for the array
    /// form.
    pub fn variants(&self) -> &[EnumVariant] {
        &self.variants
    }
}

/// An enum variant with its `u32` discriminant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariant {
    name: String,
    discriminant: u32,
}

impl EnumVariant {
    /// Returns the variant name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the discriminant.
    pub fn discriminant(&self) -> u32 {
        self.discriminant
    }
}

/// The type of a message field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    /// A supported scalar.
    Prim(PrimTy),
    /// A fixed-size array `[T; N]`, `N >= 1`.
    Array(Box<FieldType>, usize),
    /// A reference to another message or enum.
    Named(String),
}

impl FieldType {
    /// Returns every named type referenced by this type (recursing into
    /// arrays).
    pub fn named_types(&self) -> Vec<&str> {
        match self {
            FieldType::Prim(_) => Vec::new(),
            FieldType::Named(name) => vec![name.as_str()],
            FieldType::Array(inner, _) => inner.named_types(),
        }
    }
}

impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FieldType::Prim(prim) => f.write_str(prim.name()),
            FieldType::Array(inner, len) => write!(f, "[{inner}; {len}]"),
            FieldType::Named(name) => f.write_str(name),
        }
    }
}

/// The 32-bit-safe scalar types allowed in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimTy {
    /// `u8`
    U8,
    /// `u16`
    U16,
    /// `u32`
    U32,
    /// `i8`
    I8,
    /// `i16`
    I16,
    /// `i32`
    I32,
    /// `f32`
    F32,
}

impl PrimTy {
    /// Parses a primitive type name, returning `None` for non-primitives.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "u8" => PrimTy::U8,
            "u16" => PrimTy::U16,
            "u32" => PrimTy::U32,
            "i8" => PrimTy::I8,
            "i16" => PrimTy::I16,
            "i32" => PrimTy::I32,
            "f32" => PrimTy::F32,
            _ => return None,
        })
    }

    /// Returns the Rust spelling of the type.
    pub const fn name(self) -> &'static str {
        match self {
            PrimTy::U8 => "u8",
            PrimTy::U16 => "u16",
            PrimTy::U32 => "u32",
            PrimTy::I8 => "i8",
            PrimTy::I16 => "i16",
            PrimTy::I32 => "i32",
            PrimTy::F32 => "f32",
        }
    }

    /// Returns the canonical size in bytes.
    pub const fn size(self) -> usize {
        match self {
            PrimTy::U8 | PrimTy::I8 => 1,
            PrimTy::U16 | PrimTy::I16 => 2,
            PrimTy::U32 | PrimTy::I32 | PrimTy::F32 => 4,
        }
    }

    /// Returns the canonical alignment in bytes (never above the align cap).
    pub const fn align(self) -> usize {
        self.size()
    }
}

/// Parses and validates an `ipc-types.toml` document.
pub fn parse_idl_str(source: &str) -> Result<IpcTypes, IdlError> {
    let value: toml::Value = toml::from_str(source)?;
    let table = value
        .as_table()
        .expect("the TOML root value is always a table");

    let mut schema = None;
    let mut messages = BTreeMap::new();
    let mut enums = BTreeMap::new();

    for (key, value) in table {
        match key.as_str() {
            "schema" => {
                if schema.is_some() {
                    return Err(IdlError::invalid("`schema` is defined more than once"));
                }
                schema = Some(parse_schema(value)?);
            }
            "message" => {
                for (name, def) in expect_table(value, "message")? {
                    validate_ident(name, "message name")?;
                    if messages.contains_key(name) || enums.contains_key(name) {
                        return Err(duplicate_type(name));
                    }
                    messages.insert(name.clone(), parse_message(name, def)?);
                }
            }
            "enum" => {
                for (name, def) in expect_table(value, "enum")? {
                    validate_ident(name, "enum name")?;
                    if messages.contains_key(name) || enums.contains_key(name) {
                        return Err(duplicate_type(name));
                    }
                    enums.insert(name.clone(), parse_enum(name, def)?);
                }
            }
            other => {
                return Err(IdlError::invalid(format!(
                    "unknown top-level key `{other}`; expected `schema`, `message` or `enum`"
                )));
            }
        }
    }

    let schema = schema.ok_or_else(|| {
        IdlError::invalid("missing required top-level key `schema` (expected `schema = 1`)")
    })?;

    let idl = IpcTypes {
        schema,
        messages,
        enums,
    };
    idl.validate()?;
    Ok(idl)
}

/// Parses and validates an `ipc-types.toml` file.
pub fn parse_idl_file(path: &Path) -> Result<IpcTypes, IdlError> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        IdlError::invalid(format!("failed to read `{}`: {error}", path.display()))
    })?;
    parse_idl_str(&source)
}

fn parse_schema(value: &toml::Value) -> Result<u32, IdlError> {
    match value.as_integer() {
        Some(found) if found == i64::from(SCHEMA_VERSION) => Ok(SCHEMA_VERSION),
        Some(found) => Err(IdlError::invalid(format!(
            "unsupported IDL schema version {found}; this tool supports schema = {SCHEMA_VERSION}"
        ))),
        None => Err(IdlError::invalid(format!(
            "`schema` must be an integer, found {}",
            kind_of(value)
        ))),
    }
}

fn parse_message(name: &str, value: &toml::Value) -> Result<Message, IdlError> {
    let path = format!("message.{name}");
    let table = expect_table(value, &path)?;

    let mut fields = None;
    for (key, value) in table {
        match key.as_str() {
            "fields" => {
                let fields_path = format!("{path}.fields");
                let raw_fields = expect_table(value, &fields_path)?;
                let mut parsed = Vec::with_capacity(raw_fields.len());
                for (field_name, field_value) in raw_fields {
                    let field_path = format!("{fields_path}.{field_name}");
                    validate_ident(field_name, &format!("field name in `{path}`"))?;
                    let text = field_value.as_str().ok_or_else(|| {
                        IdlError::invalid(format!(
                            "`{field_path}` must be a string type expression, found {}",
                            kind_of(field_value)
                        ))
                    })?;
                    parsed.push(Field {
                        name: field_name.clone(),
                        ty: parse_type_expr(text, &field_path)?,
                    });
                }
                fields = Some(parsed);
            }
            other => {
                return Err(IdlError::invalid(format!(
                    "`{path}` has unknown key `{other}`; expected `fields`"
                )));
            }
        }
    }

    let fields = fields.ok_or_else(|| {
        IdlError::invalid(format!("`{path}` is missing the required `fields` key"))
    })?;
    if fields.is_empty() {
        return Err(IdlError::invalid(format!(
            "message `{name}` declares no fields (zero-sized types cannot cross the IPC boundary)"
        )));
    }

    Ok(Message {
        name: name.to_string(),
        fields,
    })
}

fn parse_enum(name: &str, value: &toml::Value) -> Result<Enum, IdlError> {
    let path = format!("enum.{name}");
    let table = expect_table(value, &path)?;

    let mut variants_value = None;
    for (key, value) in table {
        match key.as_str() {
            "variants" => variants_value = Some(value),
            other => {
                return Err(IdlError::invalid(format!(
                    "`{path}` has unknown key `{other}`; expected `variants`"
                )));
            }
        }
    }
    let variants_value = variants_value.ok_or_else(|| {
        IdlError::invalid(format!("`{path}` is missing the required `variants` key"))
    })?;

    let variants = match variants_value {
        toml::Value::Array(items) => parse_enum_variant_names(&path, items)?,
        toml::Value::Table(map) => parse_enum_variant_table(&path, map)?,
        other => {
            return Err(IdlError::invalid(format!(
                "`{path}.variants` must be an array of variant names or a table mapping names \
                 to `u32` discriminants, found {}",
                kind_of(other)
            )));
        }
    };

    if variants.is_empty() {
        return Err(IdlError::invalid(format!(
            "enum `{name}` declares no variants"
        )));
    }

    Ok(Enum {
        name: name.to_string(),
        variants,
    })
}

fn parse_enum_variant_names(
    path: &str,
    items: &[toml::Value],
) -> Result<Vec<EnumVariant>, IdlError> {
    let mut variants = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let variant_name = item.as_str().ok_or_else(|| {
            IdlError::invalid(format!(
                "`{path}.variants[{index}]` must be a string, found {}",
                kind_of(item)
            ))
        })?;
        validate_ident(variant_name, &format!("variant name in `{path}`"))?;
        if variants
            .iter()
            .any(|existing: &EnumVariant| existing.name == variant_name)
        {
            return Err(IdlError::invalid(format!(
                "`{path}` declares variant `{variant_name}` more than once"
            )));
        }
        let discriminant = u32::try_from(index).map_err(|_| {
            IdlError::invalid(format!("`{path}` declares more than {} variants", u32::MAX))
        })?;
        variants.push(EnumVariant {
            name: variant_name.to_string(),
            discriminant,
        });
    }
    Ok(variants)
}

fn parse_enum_variant_table(path: &str, map: &toml::Table) -> Result<Vec<EnumVariant>, IdlError> {
    let mut variants = Vec::with_capacity(map.len());
    for (variant_name, value) in map {
        validate_ident(variant_name, &format!("variant name in `{path}`"))?;
        let discriminant = value.as_integer().ok_or_else(|| {
            IdlError::invalid(format!(
                "`{path}.variants.{variant_name}` must be an integer discriminant, found {}",
                kind_of(value)
            ))
        })?;
        let discriminant = u32::try_from(discriminant).map_err(|_| {
            IdlError::invalid(format!(
                "`{path}.variants.{variant_name}` discriminant {discriminant} is out of range 0..={}",
                u32::MAX
            ))
        })?;
        variants.push(EnumVariant {
            name: variant_name.clone(),
            discriminant,
        });
    }
    variants.sort_by(|a, b| {
        a.discriminant
            .cmp(&b.discriminant)
            .then_with(|| a.name.cmp(&b.name))
    });
    for pair in variants.windows(2) {
        if pair[0].discriminant == pair[1].discriminant {
            return Err(IdlError::invalid(format!(
                "`{path}` reuses discriminant {} for variants `{}` and `{}`",
                pair[0].discriminant, pair[0].name, pair[1].name
            )));
        }
    }
    Ok(variants)
}

fn parse_type_expr(text: &str, path: &str) -> Result<FieldType, IdlError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(IdlError::invalid(format!(
            "`{path}` has an empty type expression"
        )));
    }

    if let Some(inner) = text.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or_else(|| malformed_array(path, text))?;
        let separator = inner
            .rfind(';')
            .ok_or_else(|| malformed_array(path, text))?;
        let element_text = inner[..separator].trim();
        let length_text = inner[separator + 1..].trim();
        if element_text.is_empty()
            || length_text.is_empty()
            || has_top_level_semicolon(element_text)
        {
            return Err(malformed_array(path, text));
        }
        let length: usize = length_text.parse().map_err(|_| {
            IdlError::invalid(format!(
                "`{path}`: array length `{length_text}` in `{text}` is not a non-negative integer"
            ))
        })?;
        if length == 0 {
            return Err(IdlError::invalid(format!(
                "`{path}`: array length must be at least 1 in `{text}`"
            )));
        }
        let element = parse_type_expr(element_text, path)?;
        return Ok(FieldType::Array(Box::new(element), length));
    }

    if text.contains(';') {
        return Err(malformed_array(path, text));
    }
    if text.contains('&') || text.contains('*') {
        return Err(IdlError::invalid(format!(
            "`{path}`: pointers and references cannot cross the IPC boundary (`{text}`)"
        )));
    }
    if text.contains('<') || text.contains('>') {
        return Err(IdlError::invalid(format!(
            "`{path}`: generic types are not supported in the v1 IPC subset (`{text}`)"
        )));
    }
    if text.contains('(') || text.contains(')') {
        return Err(IdlError::invalid(format!(
            "`{path}`: tuples are not supported in the v1 IPC subset (`{text}`); \
             declare a `[message.…]` instead"
        )));
    }
    if text.contains('!') {
        return Err(IdlError::invalid(format!(
            "`{path}`: type `{text}` cannot cross the IPC boundary"
        )));
    }
    if text.contains(char::is_whitespace) {
        return Err(IdlError::invalid(format!(
            "`{path}`: `{text}` is not a single type name or a fixed array `[T; N]`"
        )));
    }

    if let Some(prim) = PrimTy::from_name(text) {
        return Ok(FieldType::Prim(prim));
    }
    if let Some(reason) = unsupported_type(text) {
        return Err(IdlError::invalid(format!(
            "`{path}`: type `{text}` is not supported: {reason}"
        )));
    }
    Ok(FieldType::Named(text.to_string()))
}

fn has_top_level_semicolon(text: &str) -> bool {
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

fn malformed_array(path: &str, text: &str) -> IdlError {
    IdlError::invalid(format!(
        "`{path}`: malformed array type `{text}`; expected `[T; N]` with `N >= 1`"
    ))
}

fn unsupported_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "u64" | "i64" | "u128" | "i128" | "f64" | "f16" | "f128" => {
            "64-bit and wider scalar types are not part of the 32-bit-safe IPC subset (v1)"
        }
        "usize" | "isize" => {
            "pointer-sized integers are not portable across cores and are not part of the IPC subset (v1)"
        }
        "bool" => "`bool` is not part of the 32-bit-safe IPC subset (v1); use `u8` instead",
        "char" => "`char` is not part of the 32-bit-safe IPC subset (v1); use `u32` instead",
        "str" | "String" => {
            "string types cannot cross the IPC boundary; use a fixed byte array (`[u8; N]`) instead"
        }
        _ => return None,
    })
}

fn expect_table<'a>(value: &'a toml::Value, path: &str) -> Result<&'a toml::Table, IdlError> {
    value.as_table().ok_or_else(|| {
        IdlError::invalid(format!(
            "`{path}` must be a table, found {}",
            kind_of(value)
        ))
    })
}

fn duplicate_type(name: &str) -> IdlError {
    IdlError::invalid(format!(
        "type `{name}` is declared both as a message and as an enum; \
         each name may only be declared once"
    ))
}

fn validate_ident(name: &str, context: &str) -> Result<(), IdlError> {
    if is_valid_ident(name) {
        Ok(())
    } else {
        Err(IdlError::invalid(format!(
            "{context} `{name}` is not a valid Rust identifier"
        )))
    }
}

fn is_valid_ident(name: &str) -> bool {
    if name.is_empty() || name == "_" || is_keyword(name) {
        return false;
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty checked above");
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    )
}
