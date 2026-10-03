//! `rticx.toml` parsing and validation.
//!
//! The project manifest is the single source of project topology (see
//! `multibinary-multicore-plan.md` §6.1): which binaries (applications) exist
//! and how each application maps its *local* core indices to *globally unique*
//! core ids.
//!
//! ```toml
//! schema = 1
//!
//! [[application]]
//! package = "app-m7"
//! target = { kind = "bin", name = "m7" }
//! core_ids = [0]
//!
//! [[application]]
//! package = "app-m4"
//! target = { kind = "bin", name = "m4" }
//! core_ids = [1]
//! ```
//!
//! Since M6.9 the manifest carries **no** IPC memory: the distribution owns
//! the IPC pools and reports them through its capability binding (recorded per
//! application in the metadata manifest). A leftover `[ipc.regions]` table is
//! rejected with an error naming the removal.
//!
//! Values can only be produced by [`parse_project_str`] /
//! [`parse_project_file`], so the invariants checked while parsing (unique
//! package names, globally unique core ids) hold for every instance.
//! [`ProjectConfig::to_toml_string`] renders the canonical form; parsing it
//! back yields an equal value.
//!
//! Cross-referencing the manifest against the per-application metadata
//! manifests (capability pools vs. declared core ids, priority disjointness,
//! pool budget) is the driver's merge step, not this parser.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ProjectError, kind_of};

/// Schema version of `rticx.toml` understood by this crate.
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

/// A parsed and validated `rticx.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    schema: u32,
    applications: Vec<Application>,
}

impl ProjectConfig {
    /// Returns the manifest schema version (always
    /// [`PROJECT_SCHEMA_VERSION`] for parsed values).
    pub fn schema(&self) -> u32 {
        self.schema
    }

    /// Returns the applications in declaration order.
    pub fn applications(&self) -> &[Application] {
        &self.applications
    }

    /// Returns the application building `package`, if any.
    pub fn application(&self, package: &str) -> Option<&Application> {
        self.applications
            .iter()
            .find(|application| application.package == package)
    }

    /// Iterates over every declared global core id, in application order.
    pub fn core_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.applications
            .iter()
            .flat_map(|application| application.core_ids.iter().copied())
    }

    /// Renders the canonical `rticx.toml` form of this configuration.
    ///
    /// The output is deterministic (declaration order for applications) and
    /// always parses back to an equal [`ProjectConfig`].
    pub fn to_toml_string(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "schema = {}", self.schema);

        for application in &self.applications {
            out.push_str("\n[[application]]\n");
            let _ = writeln!(out, "package = \"{}\"", application.package);
            let target = &application.target;
            let kind = target.kind.name();
            match &target.triple {
                Some(triple) => {
                    let _ = writeln!(
                        out,
                        "target = {{ kind = \"{kind}\", name = \"{}\", triple = \"{triple}\" }}",
                        target.name
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "target = {{ kind = \"{kind}\", name = \"{}\" }}",
                        target.name
                    );
                }
            }
            let ids: Vec<String> = application.core_ids.iter().map(u32::to_string).collect();
            let _ = writeln!(out, "core_ids = [{}]", ids.join(", "));
        }

        out
    }

    fn validate(&self) -> Result<(), ProjectError> {
        let mut packages: BTreeMap<&str, ()> = BTreeMap::new();
        let mut core_ids: BTreeMap<u32, &str> = BTreeMap::new();

        for application in &self.applications {
            if packages.insert(application.package.as_str(), ()).is_some() {
                return Err(ProjectError::invalid(format!(
                    "package `{}` is declared by more than one `[[application]]`",
                    application.package
                )));
            }
            for &core_id in &application.core_ids {
                if let Some(first) = core_ids.get(&core_id) {
                    return Err(ProjectError::invalid(format!(
                        "global core id {core_id} is declared by both `{first}` and `{}`; \
                         core ids must be unique across applications",
                        application.package
                    )));
                }
                core_ids.insert(core_id, application.package.as_str());
            }
        }

        Ok(())
    }
}

impl fmt::Display for ProjectConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_toml_string())
    }
}

/// One binary of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Application {
    package: String,
    target: Target,
    core_ids: Vec<u32>,
}

impl Application {
    /// Returns the Cargo package name.
    pub fn package(&self) -> &str {
        &self.package
    }

    /// Returns the built target.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Returns the global core ids of the application, indexed by local core
    /// index (`core_ids()[i]` is the global id of local core `i`).
    pub fn core_ids(&self) -> &[u32] {
        &self.core_ids
    }
}

/// The Cargo target to build for an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    kind: TargetKind,
    name: String,
    triple: Option<String>,
}

impl Target {
    /// Returns the target kind.
    pub fn kind(&self) -> TargetKind {
        self.kind
    }

    /// Returns the target name (binary name, or example name for `example`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the explicit target triple, if the manifest declares one.
    ///
    /// When absent, the triple comes from `.cargo/config.toml`.
    pub fn triple(&self) -> Option<&str> {
        self.triple.as_deref()
    }
}

/// The kind of Cargo target an application builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// A binary target.
    Bin,
}

impl TargetKind {
    /// Returns the `kind` spelling used in `rticx.toml`.
    pub const fn name(self) -> &'static str {
        match self {
            TargetKind::Bin => "bin",
        }
    }
}

/// Parses and validates a `rticx.toml` document.
pub fn parse_project_str(source: &str) -> Result<ProjectConfig, ProjectError> {
    let value: toml::Value = toml::from_str(source)?;
    let table = value
        .as_table()
        .expect("the TOML root value is always a table");

    let mut schema = None;
    let mut applications = Vec::new();

    for (key, value) in table {
        match key.as_str() {
            "schema" => {
                if schema.is_some() {
                    return Err(ProjectError::invalid("`schema` is defined more than once"));
                }
                schema = Some(parse_schema(value)?);
            }
            "application" => {
                let items = value.as_array().ok_or_else(|| {
                    ProjectError::invalid(format!(
                        "`application` must be an array of tables (`[[application]]`), found {}",
                        kind_of(value)
                    ))
                })?;
                for (index, item) in items.iter().enumerate() {
                    applications.push(parse_application(index, item)?);
                }
            }
            "ipc" => {
                return Err(ProjectError::invalid(
                    "`[ipc.regions]` was removed: IPC memory is owned by the distribution's \
                     capability binding (M6.9), not declared in rticx.toml. \
                     Remove the `[ipc.regions]` table",
                ));
            }
            other => {
                return Err(ProjectError::invalid(format!(
                    "unknown top-level key `{other}`; expected `schema` or `application`"
                )));
            }
        }
    }

    let schema = schema.ok_or_else(|| {
        ProjectError::invalid("missing required top-level key `schema` (expected `schema = 1`)")
    })?;

    let config = ProjectConfig {
        schema,
        applications,
    };
    config.validate()?;
    Ok(config)
}

/// Parses and validates a `rticx.toml` file.
pub fn parse_project_file(path: &Path) -> Result<ProjectConfig, ProjectError> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        ProjectError::invalid(format!("failed to read `{}`: {error}", path.display()))
    })?;
    parse_project_str(&source)
}

fn parse_schema(value: &toml::Value) -> Result<u32, ProjectError> {
    match value.as_integer() {
        Some(found) if found == i64::from(PROJECT_SCHEMA_VERSION) => Ok(PROJECT_SCHEMA_VERSION),
        Some(found) => Err(ProjectError::invalid(format!(
            "unsupported rticx.toml schema version {found}; this tool supports schema = {PROJECT_SCHEMA_VERSION}"
        ))),
        None => Err(ProjectError::invalid(format!(
            "`schema` must be an integer, found {}",
            kind_of(value)
        ))),
    }
}

fn parse_application(index: usize, value: &toml::Value) -> Result<Application, ProjectError> {
    let path = format!("application[{index}]");
    let table = expect_table(value, &path)?;

    let mut package = None;
    let mut target = None;
    let mut core_ids = None;

    for (key, value) in table {
        match key.as_str() {
            "package" => {
                let field_path = format!("{path}.package");
                let name = expect_string(value, &field_path)?;
                validate_name(name, &field_path, "cargo package name")?;
                package = Some(name.to_string());
            }
            "target" => target = Some(parse_target(&format!("{path}.target"), value)?),
            "core_ids" => core_ids = Some(parse_core_ids(&format!("{path}.core_ids"), value)?),
            other => {
                return Err(ProjectError::invalid(format!(
                    "`{path}` has unknown key `{other}`; expected `package`, `target` or `core_ids`"
                )));
            }
        }
    }

    let package = package.ok_or_else(|| {
        ProjectError::invalid(format!("`{path}` is missing the required `package` key"))
    })?;
    let target = target.ok_or_else(|| {
        ProjectError::invalid(format!("`{path}` is missing the required `target` key"))
    })?;
    let core_ids = core_ids.ok_or_else(|| {
        ProjectError::invalid(format!("`{path}` is missing the required `core_ids` key"))
    })?;

    Ok(Application {
        package,
        target,
        core_ids,
    })
}

fn parse_target(path: &str, value: &toml::Value) -> Result<Target, ProjectError> {
    let table = expect_table(value, path)?;

    let mut kind = None;
    let mut name = None;
    let mut triple = None;

    for (key, value) in table {
        match key.as_str() {
            "kind" => {
                let field_path = format!("{path}.kind");
                let text = expect_string(value, &field_path)?;
                kind = Some(match text {
                    "bin" => TargetKind::Bin,
                    other => {
                        return Err(ProjectError::invalid(format!(
                            "`{field_path}` must be `\"bin\"`, found `\"{other}\"`"
                        )));
                    }
                });
            }
            "name" => {
                let field_path = format!("{path}.name");
                let text = expect_string(value, &field_path)?;
                validate_name(text, &field_path, "cargo target name")?;
                name = Some(text.to_string());
            }
            "triple" => {
                let field_path = format!("{path}.triple");
                let text = expect_string(value, &field_path)?;
                validate_name(text, &field_path, "target triple")?;
                triple = Some(text.to_string());
            }
            other => {
                return Err(ProjectError::invalid(format!(
                    "`{path}` has unknown key `{other}`; expected `kind`, `name` or `triple`"
                )));
            }
        }
    }

    let kind = kind.ok_or_else(|| {
        ProjectError::invalid(format!("`{path}` is missing the required `kind` key"))
    })?;
    let name = name.ok_or_else(|| {
        ProjectError::invalid(format!("`{path}` is missing the required `name` key"))
    })?;

    Ok(Target { kind, name, triple })
}

fn parse_core_ids(path: &str, value: &toml::Value) -> Result<Vec<u32>, ProjectError> {
    let items = value.as_array().ok_or_else(|| {
        ProjectError::invalid(format!(
            "`{path}` must be an array of integers, found {}",
            kind_of(value)
        ))
    })?;
    if items.is_empty() {
        return Err(ProjectError::invalid(format!(
            "`{path}` must not be empty; it maps each local core index to a global core id"
        )));
    }

    let mut core_ids = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let item_path = format!("{path}[{index}]");
        let Some(integer) = item.as_integer() else {
            return Err(ProjectError::invalid(format!(
                "`{item_path}` must be an integer, found {}",
                kind_of(item)
            )));
        };
        let core_id = u32::try_from(integer).map_err(|_| {
            ProjectError::invalid(format!(
                "`{item_path}` = {integer} is out of range 0..={}",
                u32::MAX
            ))
        })?;
        if core_ids.contains(&core_id) {
            return Err(ProjectError::invalid(format!(
                "`{path}` lists core id {core_id} more than once"
            )));
        }
        core_ids.push(core_id);
    }
    Ok(core_ids)
}

fn expect_table<'a>(value: &'a toml::Value, path: &str) -> Result<&'a toml::Table, ProjectError> {
    value.as_table().ok_or_else(|| {
        ProjectError::invalid(format!(
            "`{path}` must be a table, found {}",
            kind_of(value)
        ))
    })
}

fn expect_string<'a>(value: &'a toml::Value, path: &str) -> Result<&'a str, ProjectError> {
    value.as_str().ok_or_else(|| {
        ProjectError::invalid(format!(
            "`{path}` must be a string, found {}",
            kind_of(value)
        ))
    })
}

fn validate_name(value: &str, path: &str, what: &str) -> Result<(), ProjectError> {
    if value.is_empty() {
        return Err(ProjectError::invalid(format!("`{path}` must not be empty")));
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(ProjectError::invalid(format!(
            "`{path}` `{value}` is not a valid {what} \
             (expected ASCII letters, digits, `-` or `_`)"
        )))
    }
}
