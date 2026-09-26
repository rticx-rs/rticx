//! Error types shared by the IDL parser, the layout engine and the code
//! generator.
//!
//! All messages are deterministic and self-contained: they name the offending
//! TOML path (for example `message.EncryptReq.fields.addr`) or generated
//! constant, so they can be asserted on in tests and shown verbatim by the
//! driver.

use thiserror::Error;

/// Failure while parsing or validating `ipc-types.toml`.
#[derive(Debug, Error)]
pub enum IdlError {
    /// The file/string is not valid TOML at all.
    #[error("failed to parse ipc-types.toml: {0}")]
    Toml(#[from] toml::de::Error),

    /// The TOML parsed, but violates the IDL schema or the v1 type subset.
    #[error("{0}")]
    Invalid(String),
}

/// Failure while parsing or validating `rticx.toml`.
#[derive(Debug, Error)]
pub enum ProjectError {
    /// The file/string is not valid TOML at all.
    #[error("failed to parse rticx.toml: {0}")]
    Toml(#[from] toml::de::Error),

    /// The TOML parsed, but violates the project-manifest schema (unknown
    /// keys, duplicate core ids, malformed region keys, ...).
    #[error("{0}")]
    Invalid(String),
}

impl ProjectError {
    /// Creates a semantic-validation error with a fully formatted message.
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// Renders the TOML kind of `value` for deterministic error messages.
pub(crate) fn kind_of(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a float",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a datetime",
        toml::Value::Array(_) => "an array",
        toml::Value::Table(_) => "a table",
    }
}

impl IdlError {
    /// Creates a semantic-validation error with a fully formatted message.
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// Failure while computing the canonical layout of an [`crate::IpcTypes`].
#[derive(Debug, Error)]
pub enum LayoutError {
    /// A message (transitively) contains itself, so it has no finite size.
    #[error("cyclic message reference: {cycle}")]
    Cycle {
        /// The dependency chain, for example `A -> B -> A`.
        cycle: String,
    },

    /// Size/offset arithmetic overflowed `usize`.
    #[error("layout of `{name}` overflows the address space (array too large)")]
    Overflow {
        /// Name of the message being laid out.
        name: String,
    },

    /// A field references a type not present in the IDL.
    ///
    /// Unreachable for values produced by [`parse_idl_str`](crate::parse_idl_str);
    /// kept as a defensive error.
    #[error("unknown type `{name}` referenced during layout")]
    UnknownType {
        /// The referenced type name.
        name: String,
    },
}

/// Failure while generating the `ipc-types` crate.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// The canonical layout could not be computed.
    #[error(transparent)]
    Layout(#[from] LayoutError),

    /// Two declarations would generate the same constant (for example
    /// `FooBar` and `Foo_Bar` both mapping to `FOO_BAR`).
    #[error(
        "{first} and {second} would both generate the constant `{constant}`; rename one of them"
    )]
    ConstCollision {
        /// First declaration path.
        first: String,
        /// Second declaration path.
        second: String,
        /// The colliding constant name.
        constant: String,
    },
}
