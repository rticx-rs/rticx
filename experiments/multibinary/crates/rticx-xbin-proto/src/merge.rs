//! Merge and validation of the per-application manifests.
//!
//! Phase 1 (`cargo xbin sync`) collects one [`AppManifest`] per application
//! and hands them, together with `rticx.toml` ([`ProjectConfig`]) and the IDL
//! ([`IpcTypes`]), to [`merge_project`]. The result is a [`MergedProject`]:
//! the validated whole-project view with resolved global core ids, canonical
//! type layouts, tasks with their FIFO allocations and the region list.
//!
//! Validation rules (each with a focused [`MergeError`]):
//!
//! - **core-id consistency**: every `[[application]]` has exactly one
//!   manifest; `cores` and `core_ids` agree with `rticx.toml` (which is
//!   authoritative); every referenced external core exists and is owned by
//!   another binary;
//! - **receiver topology**: task names are unique, each receiver's local
//!   `core` is in range, its single `spawn_by` names an existing producer in
//!   another application, both applications list the peer's core in
//!   `external_cores`, and the input type exists in `ipc-types.toml`;
//! - **priority disjointness**: on one target core, tasks from different
//!   source cores never share a priority line (local tasks are checked by the
//!   core pass in phase 2);
//! - **region fit**: every used `(source -> target)` direction has a region
//!   and the per-task FIFOs fit it, using the canonical [`crate::fifo`] image.
//!
//! Region fit and FIFO allocation are one deterministic pass: within each
//! region, tasks are placed in `(source, target, task name)` order, each FIFO
//! aligned to [`crate::FIFO_ALIGN`], with ring depth `capacity + 1`. The
//! first FIFO that does not fit fails with the task and the region named
//! ([`MergeError::RegionOverflow`]). The `system.json` emit of M1-T6
//! ([`crate::system_view`]) then copies the resolved offsets.
//!
//! v1 supports exactly one producer core per task, so the producer of every
//! task is its receiver's singular `spawn_by` (M5.5); multi-producer tasks
//! are rejected by the pass that parses the task attribute.
//!
//! [`merge_project`] is a pure function of its inputs and emits tasks sorted
//! by name with deterministic ids, so identical inputs always produce an
//! equal [`MergedProject`].

use std::collections::BTreeMap;

use crate::error::MergeError;
use crate::fifo::{align_up, fifo_depth, fifo_size};
use crate::idl::IpcTypes;
use crate::layout::{Layout, Layouts};
use crate::manifest::{AppManifest, ReceiverDecl, simple_type_name};
use crate::project::ProjectConfig;
use crate::system::{
    AppEntry, CoreEntry, FieldEntry, FifoEntry, RegionEntry, TypeEntry, TypeKind, VariantEntry,
};

/// A merged, validated and allocated whole-project view.
///
/// Produced by [`merge_project`] (FIFO offsets included); converted into the
/// driver's `target/rticx-xbin/system.json` by [`crate::system_view`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedProject {
    apps: Vec<AppEntry>,
    cores: Vec<CoreEntry>,
    types: Vec<TypeEntry>,
    tasks: Vec<MergedTask>,
    regions: Vec<RegionEntry>,
}

impl MergedProject {
    /// Returns the applications in `rticx.toml` declaration order.
    pub fn apps(&self) -> &[AppEntry] {
        &self.apps
    }

    /// Returns the global core table in `(application, local index)` order.
    pub fn cores(&self) -> &[CoreEntry] {
        &self.cores
    }

    /// Returns every IDL type with its canonical layout, sorted by name.
    pub fn types(&self) -> &[TypeEntry] {
        &self.types
    }

    /// Returns every cross-binary task, sorted by name (matching the task ids).
    pub fn tasks(&self) -> &[MergedTask] {
        &self.tasks
    }

    /// Returns every declared region, sorted by `(source, target)`.
    pub fn regions(&self) -> &[RegionEntry] {
        &self.regions
    }

    /// Returns the task named `name`, if any.
    pub fn task(&self, name: &str) -> Option<&MergedTask> {
        self.tasks.iter().find(|task| task.name == name)
    }
}

/// One validated cross-binary task with its deterministic FIFO allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedTask {
    id: u32,
    name: String,
    receiver_core: u32,
    spawner_core: u32,
    priority: u16,
    capacity: usize,
    input_type: String,
    elem_size: u32,
    elem_align: u32,
    offset: u32,
}

impl MergedTask {
    /// Returns the deterministic project-wide task id (`1..=N`, in name
    /// order).
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Returns the task name shared by the receiver and its sender stubs.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the global id of the core that executes the task.
    pub fn receiver_core(&self) -> u32 {
        self.receiver_core
    }

    /// Returns the global id of the (single, in v1) core that spawns it.
    pub fn spawner_core(&self) -> u32 {
        self.spawner_core
    }

    /// Returns the priority line on the receiver core.
    pub fn priority(&self) -> u16 {
        self.priority
    }

    /// Returns the number of pending inputs the FIFO holds.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the IDL name of the input type.
    pub fn input_type(&self) -> &str {
        &self.input_type
    }

    /// Returns the canonical size of the input type in bytes.
    pub fn elem_size(&self) -> u32 {
        self.elem_size
    }

    /// Returns the canonical alignment of the input type in bytes.
    pub fn elem_align(&self) -> u32 {
        self.elem_align
    }

    /// Returns the byte offset of the task FIFO inside its
    /// `(source -> target)` region.
    pub fn fifo_offset(&self) -> u32 {
        self.offset
    }

    /// Returns the ring depth emitted into `system.json` (`capacity + 1`).
    pub fn fifo_depth(&self) -> u32 {
        u32::try_from(fifo_depth(self.capacity).expect("validated by merge_project"))
            .expect("validated by merge_project")
    }

    /// Returns the FIFO allocation entry emitted into `system.json`.
    pub fn fifo(&self) -> FifoEntry {
        FifoEntry {
            source: self.spawner_core,
            target: self.receiver_core,
            offset: self.offset,
            elem_size: self.elem_size,
            depth: self.fifo_depth(),
        }
    }

    /// Returns the total FIFO footprint inside its region, in bytes.
    pub fn fifo_bytes(&self) -> u64 {
        fifo_size(self.elem_size, self.capacity).expect("validated by merge_project")
    }
}

/// Merges the per-application manifests and validates the whole project.
///
/// `config` comes from `rticx.toml`, `manifests` is the list collected by
/// `cargo xbin sync` (one per application) and `idl` is the parsed
/// `ipc-types.toml` (empty for a project without cross-binary tasks). See the
/// [module documentation](self) for the exact rules.
pub fn merge_project(
    config: &ProjectConfig,
    manifests: &[AppManifest],
    idl: &IpcTypes,
) -> Result<MergedProject, MergeError> {
    let applications = config.applications();
    let layouts = Layouts::of(idl)?;

    // -- core-id consistency -------------------------------------------------
    let mut manifest_of: BTreeMap<&str, &AppManifest> = BTreeMap::new();
    for manifest in manifests {
        let application = config.application(&manifest.package).ok_or_else(|| {
            MergeError::UndeclaredApplication {
                package: manifest.package.clone(),
            }
        })?;
        if manifest_of.insert(&manifest.package, manifest).is_some() {
            return Err(MergeError::DuplicateManifest {
                package: manifest.package.clone(),
            });
        }
        if manifest.target.name != application.target().name() {
            return Err(MergeError::TargetMismatch {
                package: manifest.package.clone(),
                declared: application.target().name().to_string(),
                found: manifest.target.name.clone(),
            });
        }
        if manifest.cores as usize != application.core_ids().len() {
            return Err(MergeError::CoreCountMismatch {
                package: manifest.package.clone(),
                declared: manifest.cores,
                mapped: application.core_ids().len(),
            });
        }
        if let Some(declared) = &manifest.core_ids
            && declared.as_slice() != application.core_ids()
        {
            return Err(MergeError::CoreIdsMismatch {
                package: manifest.package.clone(),
                expected: application.core_ids().to_vec(),
                found: declared.clone(),
            });
        }
    }
    for application in applications {
        if !manifest_of.contains_key(application.package()) {
            return Err(MergeError::MissingManifest {
                package: application.package().to_string(),
                target: application.target().name().to_string(),
            });
        }
    }

    // -- global core table ---------------------------------------------------
    let mut cores = Vec::new();
    let mut owner: BTreeMap<u32, (&str, u32)> = BTreeMap::new();
    for application in applications {
        for (local, &global) in application.core_ids().iter().enumerate() {
            cores.push(CoreEntry {
                global_id: global,
                app: application.package().to_string(),
                local_index: local as u32,
            });
            owner.insert(global, (application.package(), local as u32));
        }
    }

    // -- external cores ------------------------------------------------------
    for application in applications {
        let manifest = manifest_of[application.package()];
        for &global in &manifest.external_cores {
            let Some((app, _)) = owner.get(&global) else {
                return Err(MergeError::UnknownCore {
                    package: application.package().to_string(),
                    core: global,
                    context: "`external_cores`".to_string(),
                });
            };
            if *app == application.package() {
                return Err(MergeError::OwnExternalCore {
                    package: application.package().to_string(),
                    core: global,
                });
            }
        }
    }

    // -- receivers -----------------------------------------------------------
    struct Receiver<'a> {
        manifest: &'a AppManifest,
        decl: &'a ReceiverDecl,
        /// Global id of the core that executes the task.
        core: u32,
        /// Global id of the single producer core (`spawn_by`).
        spawner: u32,
    }

    let mut receivers: Vec<Receiver<'_>> = Vec::new();
    let mut receiver_index: BTreeMap<&str, usize> = BTreeMap::new();
    for application in applications {
        let manifest = manifest_of[application.package()];
        for decl in &manifest.receivers {
            if let Some(&first) = receiver_index.get(decl.name.as_str()) {
                return Err(MergeError::DuplicateReceiver {
                    task: decl.name.clone(),
                    first: receivers[first].manifest.package.clone(),
                    second: manifest.package.clone(),
                });
            }
            let core = application
                .core_ids()
                .get(decl.core as usize)
                .copied()
                .ok_or_else(|| MergeError::ReceiverCoreOutOfRange {
                    package: manifest.package.clone(),
                    task: decl.name.clone(),
                    core: decl.core,
                    cores: application.core_ids().len() as u32,
                })?;
            check_input_type(&manifest.package, &decl.name, &decl.input_type, idl)?;

            // The pass resolves `spawn_by` in the global namespace, so the
            // manifest always carries a global core id of another binary.
            let Some((producer_app, _)) = owner.get(&decl.spawn_by) else {
                return Err(MergeError::UnknownCore {
                    package: manifest.package.clone(),
                    core: decl.spawn_by,
                    context: format!("`spawn_by` of receiver `{}`", decl.name),
                });
            };
            if *producer_app == manifest.package {
                return Err(MergeError::OwnSpawnerCore {
                    package: manifest.package.clone(),
                    task: decl.name.clone(),
                    core: decl.spawn_by,
                });
            }
            if !manifest.external_cores.contains(&decl.spawn_by) {
                return Err(MergeError::SpawnerCoreNotVisible {
                    package: manifest.package.clone(),
                    task: decl.name.clone(),
                    core: decl.spawn_by,
                });
            }
            let producer = manifest_of[producer_app];
            if !producer.external_cores.contains(&core) {
                return Err(MergeError::TargetCoreNotVisible {
                    package: producer.package.clone(),
                    task: decl.name.clone(),
                    core,
                });
            }

            receiver_index.insert(decl.name.as_str(), receivers.len());
            receivers.push(Receiver {
                manifest,
                decl,
                core,
                spawner: decl.spawn_by,
            });
        }
    }

    // -- resolved tasks ------------------------------------------------------
    let mut tasks = Vec::with_capacity(receivers.len());
    for receiver in &receivers {
        let input_type = simple_type_name(&receiver.decl.input_type).to_string();
        let layout =
            type_layout(&layouts, &input_type).ok_or_else(|| MergeError::UnknownInputType {
                package: receiver.manifest.package.clone(),
                task: receiver.decl.name.clone(),
                type_name: input_type.clone(),
            })?;
        tasks.push(MergedTask {
            id: 0,
            name: receiver.decl.name.clone(),
            receiver_core: receiver.core,
            spawner_core: receiver.spawner,
            priority: receiver.decl.priority,
            capacity: receiver.decl.capacity,
            input_type,
            elem_size: to_u32(layout.size, &receiver.decl.name)?,
            elem_align: layout.align as u32,
            offset: 0,
        });
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    for (index, task) in tasks.iter_mut().enumerate() {
        task.id = u32::try_from(index + 1).expect("task count fits in u32");
    }

    // -- priority lines ------------------------------------------------------
    let mut lines: BTreeMap<(u32, u16), (&MergedTask, u32)> = BTreeMap::new();
    for task in &tasks {
        let key = (task.receiver_core, task.priority);
        if let Some((first, first_source)) = lines.get(&key) {
            if *first_source != task.spawner_core {
                return Err(MergeError::PriorityConflict {
                    target: task.receiver_core,
                    priority: task.priority,
                    first_task: first.name.clone(),
                    first_source: *first_source,
                    second_task: task.name.clone(),
                    second_source: task.spawner_core,
                });
            }
        } else {
            lines.insert(key, (task, task.spawner_core));
        }
    }

    // -- FIFO allocation and region fit --------------------------------------
    // Deterministic layout: within each region, tasks are placed in
    // `(source, target, task name)` order, each FIFO aligned to
    // `FIFO_ALIGN`. The first FIFO that does not fit names its task and
    // region in the error. The `(source, target, name)` order restricted to
    // one direction is the name order of the (name-sorted) task vector, but
    // the explicit key keeps the allocation independent of the vector order.
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by(|&left, &right| {
        let (left, right) = (&tasks[left], &tasks[right]);
        (left.spawner_core, left.receiver_core, &left.name).cmp(&(
            right.spawner_core,
            right.receiver_core,
            &right.name,
        ))
    });

    let mut offsets = vec![0u32; tasks.len()];
    let mut region_offset: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for &index in &order {
        let task = &tasks[index];
        let direction = (task.spawner_core, task.receiver_core);
        let region =
            config
                .region(direction.0, direction.1)
                .ok_or_else(|| MergeError::MissingRegion {
                    task: task.name.clone(),
                    producer: direction.0,
                    target: direction.1,
                })?;
        let bytes =
            fifo_size(task.elem_size, task.capacity).ok_or_else(|| MergeError::TaskTooLarge {
                task: task.name.clone(),
                capacity: task.capacity,
            })?;
        let start = align_up(*region_offset.get(&direction).unwrap_or(&0)).ok_or_else(|| {
            MergeError::TaskTooLarge {
                task: task.name.clone(),
                capacity: task.capacity,
            }
        })?;
        let end = start
            .checked_add(bytes)
            .ok_or_else(|| MergeError::TaskTooLarge {
                task: task.name.clone(),
                capacity: task.capacity,
            })?;
        if end > u64::from(region.size()) {
            return Err(MergeError::RegionOverflow {
                task: task.name.clone(),
                producer: direction.0,
                target: direction.1,
                needed: end,
                available: region.size(),
            });
        }
        offsets[index] = u32::try_from(start).map_err(|_| MergeError::TaskTooLarge {
            task: task.name.clone(),
            capacity: task.capacity,
        })?;
        region_offset.insert(direction, end);
    }
    for (index, task) in tasks.iter_mut().enumerate() {
        task.offset = offsets[index];
    }

    // -- assembled view ------------------------------------------------------
    let apps = applications
        .iter()
        .map(|application| {
            let manifest = manifest_of[application.package()];
            AppEntry {
                package: application.package().to_string(),
                target: manifest.target.clone(),
                source_hash: manifest.source_hash,
                core_ids: application.core_ids().to_vec(),
                external_cores: manifest.external_cores.clone(),
            }
        })
        .collect();

    let regions = config
        .regions()
        .iter()
        .map(|(key, region)| RegionEntry {
            source: key.source(),
            target: key.target(),
            base_from_source: region.base_from_source(),
            base_from_target: region.base_from_target(),
            size: region.size(),
        })
        .collect();

    Ok(MergedProject {
        apps,
        cores,
        types: type_entries(idl, &layouts)?,
        tasks,
        regions,
    })
}

/// Checks that `input` names a type declared in the IDL.
fn check_input_type(
    package: &str,
    task: &str,
    input: &str,
    idl: &IpcTypes,
) -> Result<(), MergeError> {
    let type_name = simple_type_name(input);
    if idl.has_type(type_name) {
        Ok(())
    } else {
        Err(MergeError::UnknownInputType {
            package: package.to_string(),
            task: task.to_string(),
            type_name: type_name.to_string(),
        })
    }
}

/// Returns the canonical layout of the message or enum named `name`.
fn type_layout(layouts: &Layouts, name: &str) -> Option<Layout> {
    layouts
        .message(name)
        .map(|message| message.layout)
        .or_else(|| layouts.get_enum(name).map(|enumeration| enumeration.layout))
}

/// Builds the `system.json` type entries for every IDL type, sorted by name.
fn type_entries(idl: &IpcTypes, layouts: &Layouts) -> Result<Vec<TypeEntry>, MergeError> {
    let mut names: Vec<&str> = idl
        .messages()
        .keys()
        .map(String::as_str)
        .chain(idl.enums().keys().map(String::as_str))
        .collect();
    names.sort_unstable();

    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        if let Some(message) = layouts.message(name) {
            entries.push(TypeEntry {
                name: name.to_string(),
                kind: TypeKind::Message,
                size: to_u32(message.layout.size, name)?,
                align: message.layout.align as u32,
                fields: message
                    .fields
                    .iter()
                    .map(|field| {
                        Ok(FieldEntry {
                            name: field.name.clone(),
                            ty: field.ty.to_string(),
                            offset: to_u32(field.offset, name)?,
                        })
                    })
                    .collect::<Result<_, MergeError>>()?,
                variants: Vec::new(),
            });
        } else if let Some(enumeration) = layouts.get_enum(name) {
            entries.push(TypeEntry {
                name: name.to_string(),
                kind: TypeKind::Enum,
                size: to_u32(enumeration.layout.size, name)?,
                align: enumeration.layout.align as u32,
                fields: Vec::new(),
                variants: enumeration
                    .variants
                    .iter()
                    .map(|variant| VariantEntry {
                        name: variant.name.clone(),
                        discriminant: variant.discriminant,
                    })
                    .collect(),
            });
        }
    }
    Ok(entries)
}

/// Narrows a canonical layout value to the `u32` of the `system.json` schema.
fn to_u32(value: usize, name: &str) -> Result<u32, MergeError> {
    u32::try_from(value).map_err(|_| MergeError::TypeTooLarge {
        name: name.to_string(),
        size: value,
    })
}
