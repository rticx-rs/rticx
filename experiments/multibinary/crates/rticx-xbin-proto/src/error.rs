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

/// Failure while parsing a hex-encoded hash (`Hash64`).
#[derive(Debug, Error)]
pub enum HashParseError {
    /// The text is not `0x` followed by 1..=16 hexadecimal digits.
    #[error("invalid hash `{0}`; expected `0x` followed by up to 16 hexadecimal digits")]
    Invalid(String),
}

impl HashParseError {
    /// Creates an error for the offending hash text.
    pub(crate) fn invalid(text: impl Into<String>) -> Self {
        Self::Invalid(text.into())
    }
}

/// Failure while parsing a `system.json` system view.
#[derive(Debug, Error)]
pub enum SystemError {
    /// The JSON is malformed or does not match the schema.
    #[error("failed to parse system.json: {0}")]
    Json(#[from] serde_json::Error),

    /// The JSON carries a schema version this tool does not understand.
    #[error(
        "unsupported system.json schema version {found}; this tool supports schema = {expected}"
    )]
    Schema {
        /// Version found in the file.
        found: u32,
        /// Version supported by this tool.
        expected: u32,
    },
}

/// Failure while parsing an application manifest (`<target>.xbin.json`).
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The JSON is malformed or does not match the schema.
    #[error("failed to parse application manifest: {0}")]
    Json(#[from] serde_json::Error),

    /// The JSON carries a schema version this tool does not understand.
    #[error(
        "unsupported application manifest schema version {found}; this tool supports schema = {expected}"
    )]
    Schema {
        /// Version found in the file.
        found: u32,
        /// Version supported by this tool.
        expected: u32,
    },
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

/// Failure while merging and validating the application manifests (M1-T5).
///
/// Every variant is a hard, deterministic error: the message names the
/// offending package, task, core id or region, so it can be shown to the user
/// verbatim by `cargo xbin sync`.
#[derive(Debug, Error)]
pub enum MergeError {
    /// The canonical layout of the IDL cannot be computed.
    #[error(transparent)]
    Layout(#[from] LayoutError),

    /// A declared type does not fit the 32-bit address space.
    #[error(
        "type `{name}` is {size} bytes, which does not fit the 32-bit IPC address space; \
         split it into smaller messages"
    )]
    TypeTooLarge {
        /// The offending type.
        name: String,
        /// Its canonical size in bytes.
        size: usize,
    },

    /// An `[[application]]` has no manifest (its `#[app]` did not expand).
    #[error(
        "application `{package}` has no manifest; `cargo xbin sync` must collect \
         `{target}.xbin.json` for every `[[application]]`"
    )]
    MissingManifest {
        /// Package declared in `rticx.toml`.
        package: String,
        /// Binary target name declared in `rticx.toml`.
        target: String,
    },

    /// A manifest's package is not declared in `rticx.toml`.
    #[error(
        "application manifest for package `{package}` has no matching `[[application]]` \
         in rticx.toml"
    )]
    UndeclaredApplication {
        /// Package named by the manifest.
        package: String,
    },

    /// Two manifests name the same package.
    #[error("package `{package}` has more than one application manifest")]
    DuplicateManifest {
        /// The duplicated package.
        package: String,
    },

    /// `rticx.toml` and the manifest disagree on the binary name.
    #[error(
        "`[[application]]` for `{package}` builds binary `{declared}`, but its manifest \
         names binary `{found}`"
    )]
    TargetMismatch {
        /// The package.
        package: String,
        /// Binary name declared in `rticx.toml`.
        declared: String,
        /// Binary name recorded in the manifest.
        found: String,
    },

    /// `rticx.toml` and the manifest disagree on the local core count.
    #[error(
        "rticx.toml maps {mapped} core(s) for `{package}`, but its manifest declares \
         `cores = {declared}`"
    )]
    CoreCountMismatch {
        /// The package.
        package: String,
        /// Local core count recorded in the manifest.
        declared: u32,
        /// Number of `core_ids` entries in `rticx.toml`.
        mapped: usize,
    },

    /// The manifest declares a `core_ids` mapping different from `rticx.toml`.
    #[error(
        "`{package}` declares `core_ids = {found:?}` in its `#[app]`, but rticx.toml maps \
         `core_ids = {expected:?}`; rticx.toml is authoritative"
    )]
    CoreIdsMismatch {
        /// The package.
        package: String,
        /// Mapping declared in `rticx.toml`.
        expected: Vec<u32>,
        /// Mapping recorded in the manifest.
        found: Vec<u32>,
    },

    /// A referenced global core id is not declared by any application.
    #[error(
        "`{package}` references global core id {core} in {context}, but no application declares it"
    )]
    UnknownCore {
        /// Package that made the reference.
        package: String,
        /// The unknown global core id.
        core: u32,
        /// Where the id appeared, for example `` `external_cores` ``.
        context: String,
    },

    /// `external_cores` lists an id owned by the same application.
    #[error(
        "`external_cores` of `{package}` lists global core id {core}, which the application \
         owns; external cores must belong to another binary"
    )]
    OwnExternalCore {
        /// The package.
        package: String,
        /// The locally owned core id.
        core: u32,
    },

    /// A receiver's local `core` index is outside `0..cores`.
    #[error(
        "receiver `{task}` in `{package}` declares local `core = {core}`, but the \
         application has only {cores} core(s)"
    )]
    ReceiverCoreOutOfRange {
        /// The package.
        package: String,
        /// The task name.
        task: String,
        /// The local core index.
        core: u32,
        /// The application's local core count.
        cores: u32,
    },

    /// A receiver's `spawn_by` names a core owned by its own application.
    #[error(
        "receiver `{task}` in `{package}` declares `spawn_by = {core}`, but the application \
         owns that core; cross-binary spawners must belong to another binary"
    )]
    OwnSpawnerCore {
        /// The package.
        package: String,
        /// The task name.
        task: String,
        /// The locally owned core id.
        core: u32,
    },

    /// Two applications declare a receiver with the same task name.
    #[error(
        "receiver `{task}` is declared by both `{first}` and `{second}`; task names must \
         be unique across the project"
    )]
    DuplicateReceiver {
        /// The duplicated task name.
        task: String,
        /// Package of the first declaration.
        first: String,
        /// Package of the second declaration.
        second: String,
    },

    /// The producer application does not list the receiver's core in its own
    /// `external_cores`.
    #[error(
        "task `{task}` runs on global core {core}, but its producer application `{package}` \
         does not list that core in `external_cores`"
    )]
    TargetCoreNotVisible {
        /// The producer package.
        package: String,
        /// The task name.
        task: String,
        /// The receiver core id.
        core: u32,
    },

    /// A receiver's application does not list its producer core.
    #[error(
        "receiver `{task}` in `{package}` is spawned by global core {core}, but the \
         application does not list it in `external_cores`"
    )]
    SpawnerCoreNotVisible {
        /// The receiving package.
        package: String,
        /// The task name.
        task: String,
        /// The producer core id.
        core: u32,
    },

    /// A declared input type does not exist in the IDL.
    #[error(
        "task `{task}` in `{package}` uses type `{type_name}`, which is not declared in `ipc-types.toml`"
    )]
    UnknownInputType {
        /// The package of the declaring application.
        package: String,
        /// The task name.
        task: String,
        /// The missing IDL type name.
        type_name: String,
    },

    /// Two tasks from different source cores share a priority line.
    #[error(
        "tasks `{first_task}` (spawned by core {first_source}) and `{second_task}` \
         (spawned by core {second_source}) share priority {priority} on core {target}; \
         priority lines from different source cores must be disjoint"
    )]
    PriorityConflict {
        /// Receiver core the conflict is on.
        target: u32,
        /// The shared priority.
        priority: u16,
        /// First task (in name order).
        first_task: String,
        /// First task's source core.
        first_source: u32,
        /// Second task.
        second_task: String,
        /// Second task's source core.
        second_source: u32,
    },

    /// A used `(source -> target)` direction has no IPC pool path.
    #[error(
        "task `{task}` needs a `{producer}->{target}` IPC pool, but the distribution's \
         capability binding provides no pool between those cores"
    )]
    MissingRegion {
        /// The task that needs the pool.
        task: String,
        /// The producer core id.
        producer: u32,
        /// The consumer core id.
        target: u32,
    },

    /// The per-task FIFOs do not fit their pool budget.
    #[error(
        "task `{task}` does not fit the `{producer}->{target}` pool: \
         {needed} bytes needed, {available} available"
    )]
    RegionOverflow {
        /// The task whose FIFO did not fit.
        task: String,
        /// The producer core id.
        producer: u32,
        /// The consumer core id.
        target: u32,
        /// Bytes required up to and including this FIFO (including alignment).
        needed: u64,
        /// Pool budget reported by the distribution.
        available: u32,
    },

    /// A capacity is too large for the FIFO size arithmetic.
    #[error(
        "task `{task}` declares `capacity = {capacity}`, which is too large for the region arithmetic"
    )]
    TaskTooLarge {
        /// The task name.
        task: String,
        /// The declared capacity.
        capacity: usize,
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
