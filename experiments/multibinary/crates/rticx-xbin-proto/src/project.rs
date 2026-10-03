//! `rticx.toml` parsing and validation.
//!
//! The project manifest is the single source of project topology (see
//! `multibinary-multicore-plan.md` §6.1): which binaries (applications) exist,
//! how each application maps its *local* core indices to *globally unique*
//! core ids, and which shared-memory region carries each
//! `(source -> target)` direction.
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
//!
//! [ipc.regions]
//! # Shorthand when both cores see the same absolute address:
//! "0->1" = { base = 0x30040000, size = 4096 }
//! # Granular form for aliased views (source and target differ):
//! "1->0" = { base_from_source = 0x30041000, base_from_target = 0x10041000, size = 4096 }
//! ```
//!
//! Values can only be produced by [`parse_project_str`] /
//! [`parse_project_file`], so the invariants checked while parsing (unique
//! package names, globally unique core ids, well-formed region keys, distinct
//! region endpoints, non-overlapping per-core region views) hold for every
//! instance. [`ProjectConfig::to_toml_string`]
//! renders the canonical form (always the fully explicit
//! `base_from_source`/`base_from_target` form); parsing it back yields an equal
//! value.
//!
//! Cross-referencing the manifest against the per-application metadata
//! manifests (region endpoints vs. declared core ids, priority disjointness,
//! region capacity) is the driver's merge step (M1-T5), not this parser.

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
    regions: BTreeMap<RegionKey, Region>,
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

    /// Returns every IPC region, keyed and sorted by `(source, target)`.
    pub fn regions(&self) -> &BTreeMap<RegionKey, Region> {
        &self.regions
    }

    /// Returns the region for the `source -> target` direction, if declared.
    pub fn region(&self, source: u32, target: u32) -> Option<&Region> {
        self.regions.get(&RegionKey::new(source, target))
    }

    /// Iterates over every declared global core id, in application order.
    pub fn core_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.applications
            .iter()
            .flat_map(|application| application.core_ids.iter().copied())
    }

    /// Renders the canonical `rticx.toml` form of this configuration.
    ///
    /// The output is deterministic (declaration order for applications,
    /// `(source, target)` order for regions, lowercase hex addresses) and
    /// always parses back to an equal [`ProjectConfig`]. Regions are always
    /// rendered in the explicit `base_from_source`/`base_from_target` form; the
    /// `base` shorthand is accepted on input only.
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

        if !self.regions.is_empty() {
            out.push_str("\n[ipc.regions]\n");
            for (key, region) in &self.regions {
                let _ = writeln!(
                    out,
                    "\"{key}\" = {{ base_from_source = 0x{:08x}, base_from_target = 0x{:08x}, size = {} }}",
                    region.base_from_source, region.base_from_target, region.size
                );
            }
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

        self.validate_region_views()?;

        Ok(())
    }

    /// Rejects regions that map two different address ranges into the same
    /// core's address space.
    ///
    /// Each region contributes one range to each of its two endpoint cores: the
    /// source core sees the region at `base_from_source`, the target core at
    /// `base_from_target`. A core therefore has one range per region it
    /// participates in (as source and/or target), and those ranges must be
    /// pairwise disjoint: otherwise the core would see two different regions at
    /// the same addresses. Overlap is checked on `[base, base + size)`.
    ///
    /// The check is independent of the `source != target` view aliasing: the
    /// source and target views of one region belong to different cores, so a
    /// region whose two views are equal is fine; only ranges assigned to the
    /// *same* core are compared.
    fn validate_region_views(&self) -> Result<(), ProjectError> {
        struct View<'a> {
            region: &'a RegionKey,
            role: &'static str,
            base: u32,
            end: u64,
        }

        let mut by_core: BTreeMap<u32, Vec<View<'_>>> = BTreeMap::new();
        for (key, region) in &self.regions {
            by_core.entry(key.source()).or_default().push(View {
                region: key,
                role: "source view",
                base: region.base_from_source,
                end: u64::from(region.base_from_source) + u64::from(region.size),
            });
            by_core.entry(key.target()).or_default().push(View {
                region: key,
                role: "target view",
                base: region.base_from_target,
                end: u64::from(region.base_from_target) + u64::from(region.size),
            });
        }

        for (core, views) in &mut by_core {
            views.sort_by(|left, right| {
                (left.base, left.end, left.region, left.role).cmp(&(
                    right.base,
                    right.end,
                    right.region,
                    right.role,
                ))
            });
            for pair in views.windows(2) {
                let (first, second) = (&pair[0], &pair[1]);
                if u64::from(second.base) < first.end {
                    return Err(ProjectError::invalid(format!(
                        "`ipc.regions` overlap on core {core}: `{first_region}` \
                         ({first_role} 0x{first_base:08x}..0x{first_end:08x}) and \
                         `{second_region}` ({second_role} 0x{second_base:08x}..0x{second_end:08x}); \
                         a core's source and target views must map to non-overlapping addresses",
                        first_region = first.region,
                        second_region = second.region,
                        first_role = first.role,
                        first_base = first.base,
                        first_end = first.end,
                        second_role = second.role,
                        second_base = second.base,
                        second_end = second.end,
                    )));
                }
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

/// The `source -> target` direction a region carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionKey {
    source: u32,
    target: u32,
}

impl RegionKey {
    /// Creates a region key for the `source -> target` direction.
    pub const fn new(source: u32, target: u32) -> Self {
        Self { source, target }
    }

    /// Returns the source (producer) global core id.
    pub const fn source(self) -> u32 {
        self.source
    }

    /// Returns the target (consumer) global core id.
    pub const fn target(self) -> u32 {
        self.target
    }
}

impl fmt::Display for RegionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}->{}", self.source, self.target)
    }
}

/// A shared-memory region for one `(source -> target)` direction.
///
/// FIFOs of every cross-binary task spawned by `source` and executed by
/// `target` are allocated inside this region. The two base addresses are the
/// addresses through which each core sees the same physical memory (aliases
/// are allowed; they are often but not necessarily equal). When both cores see
/// the region at the same absolute address, `rticx.toml` may use the `base`
/// shorthand instead of the two explicit keys; it is expanded to equal
/// `base_from_source`/`base_from_target` values here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    base_from_source: u32,
    base_from_target: u32,
    size: u32,
}

impl Region {
    /// Returns the region base address as seen by the source core.
    pub fn base_from_source(&self) -> u32 {
        self.base_from_source
    }

    /// Returns the region base address as seen by the target core.
    pub fn base_from_target(&self) -> u32 {
        self.base_from_target
    }

    /// Returns the region size in bytes.
    pub fn size(&self) -> u32 {
        self.size
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
    let mut regions = BTreeMap::new();

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
            "ipc" => regions = parse_ipc(value)?,
            other => {
                return Err(ProjectError::invalid(format!(
                    "unknown top-level key `{other}`; expected `schema`, `application` or `ipc`"
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
        regions,
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

fn parse_ipc(value: &toml::Value) -> Result<BTreeMap<RegionKey, Region>, ProjectError> {
    let table = expect_table(value, "ipc")?;

    let mut regions = None;
    for (key, value) in table {
        match key.as_str() {
            "regions" => regions = Some(parse_regions(value)?),
            other => {
                return Err(ProjectError::invalid(format!(
                    "`ipc` has unknown key `{other}`; expected `regions`"
                )));
            }
        }
    }

    regions.ok_or_else(|| ProjectError::invalid("`ipc` is missing the required `regions` key"))
}

fn parse_regions(value: &toml::Value) -> Result<BTreeMap<RegionKey, Region>, ProjectError> {
    let table = expect_table(value, "ipc.regions")?;

    let mut regions = BTreeMap::new();
    for (key, value) in table {
        let region_key = parse_region_key(key)?;
        let region = parse_region(&format!("ipc.regions.\"{key}\""), value)?;
        regions.insert(region_key, region);
    }
    Ok(regions)
}

fn parse_region_key(key: &str) -> Result<RegionKey, ProjectError> {
    let Some((source, target)) = key.split_once("->") else {
        return Err(ProjectError::invalid(format!(
            "`ipc.regions` key `{key}` is not of the form `<source>-><target>` \
             (for example `\"0->1\"`)"
        )));
    };
    let source = parse_key_core_id(source.trim(), key, "source")?;
    let target = parse_key_core_id(target.trim(), key, "target")?;
    if source == target {
        return Err(ProjectError::invalid(format!(
            "`ipc.regions` key `{key}` connects core {source} to itself; \
             a region carries one `source -> target` direction"
        )));
    }
    Ok(RegionKey::new(source, target))
}

fn parse_key_core_id(text: &str, key: &str, side: &str) -> Result<u32, ProjectError> {
    text.parse::<u32>().map_err(|_| {
        ProjectError::invalid(format!(
            "`ipc.regions` key `{key}` has a non-integer {side} core id `{text}`"
        ))
    })
}

fn parse_region(path: &str, value: &toml::Value) -> Result<Region, ProjectError> {
    let table = expect_table(value, path)?;

    let mut base = None;
    let mut base_from_source = None;
    let mut base_from_target = None;
    let mut size = None;

    for (key, value) in table {
        match key.as_str() {
            "base" => base = Some(expect_u32(value, &format!("{path}.base"), 0)?),
            "base_from_source" => {
                base_from_source = Some(expect_u32(value, &format!("{path}.base_from_source"), 0)?);
            }
            "base_from_target" => {
                base_from_target = Some(expect_u32(value, &format!("{path}.base_from_target"), 0)?);
            }
            "size" => size = Some(expect_u32(value, &format!("{path}.size"), 1)?),
            other => {
                return Err(ProjectError::invalid(format!(
                    "`{path}` has unknown key `{other}`; \
                     expected `base`, `base_from_source`, `base_from_target` or `size`"
                )));
            }
        }
    }

    // `base` is shorthand for a region both cores see at the same absolute
    // address; combining it with an explicit view would be ambiguous.
    if base.is_some() && base_from_source.is_some() {
        return Err(ProjectError::invalid(format!(
            "`{path}` combines the `base` shorthand with `base_from_source`; \
             set `base` for equal source/target addresses, or the two explicit keys"
        )));
    }
    if base.is_some() && base_from_target.is_some() {
        return Err(ProjectError::invalid(format!(
            "`{path}` combines the `base` shorthand with `base_from_target`; \
             set `base` for equal source/target addresses, or the two explicit keys"
        )));
    }

    let base_from_source = base_from_source.or(base).ok_or_else(|| {
        ProjectError::invalid(format!(
            "`{path}` is missing the required `base_from_source` key \
             (or the `base` shorthand for equal source/target addresses)"
        ))
    })?;
    let base_from_target = base_from_target.or(base).ok_or_else(|| {
        ProjectError::invalid(format!(
            "`{path}` is missing the required `base_from_target` key \
             (or the `base` shorthand for equal source/target addresses)"
        ))
    })?;
    let size = size.ok_or_else(|| {
        ProjectError::invalid(format!("`{path}` is missing the required `size` key"))
    })?;

    Ok(Region {
        base_from_source,
        base_from_target,
        size,
    })
}

fn expect_u32(value: &toml::Value, path: &str, min: u32) -> Result<u32, ProjectError> {
    let Some(integer) = value.as_integer() else {
        return Err(ProjectError::invalid(format!(
            "`{path}` must be an integer, found {}",
            kind_of(value)
        )));
    };
    match u32::try_from(integer) {
        Ok(parsed) if parsed >= min => Ok(parsed),
        _ => Err(ProjectError::invalid(format!(
            "`{path}` = {integer} is out of range {min}..={}",
            u32::MAX
        ))),
    }
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
