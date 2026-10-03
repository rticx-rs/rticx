//! Merge and validation of the per-application manifests.
//!
//! Phase 1 (`cargo xbin sync`) collects one [`AppManifest`] per application
//! and hands them, together with `rticx.toml` ([`ProjectConfig`]) and the IDL
//! ([`IpcTypes`]), to [`merge_project`]. The result is a [`MergedProject`]:
//! the validated whole-project view with resolved global core ids, canonical
//! type layouts, tasks with their FIFO allocations and the matched IPC pool
//! list.
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
//! - **pool graph and adjacency** (M6.9-T4): the distribution's capability
//!   binding reports, per physical core, which IPC pools it can reach. The
//!   driver matches the two endpoints of every dual (same pool id, opposite
//!   physical cores, same budget and policy, swapped base views); a one-sided
//!   or inconsistent entry is [`MergeError::IpcPoolMismatch`]. A used
//!   `(source -> target)` direction with no matched pool is
//!   [`MergeError::NoIpcPath`];
//! - **pool fit**: both directions of a dual allocate inside the pool's shared
//!   budget, each FIFO aligned to [`crate::FIFO_ALIGN`] with ring depth
//!   `capacity + 1`, using the canonical [`crate::fifo`] image. The first FIFO
//!   that does not fit the shared budget is [`MergeError::PoolBudgetExceeded`],
//!   naming the dual, the pool, the bytes needed and the budget;
//! - **pool view disjointness**: a core may take part in several pools; the
//!   views it has of them must be pairwise disjoint, otherwise
//!   [`MergeError::PoolViewOverlap`].
//!
//! Pool matching and FIFO allocation are deterministic: pools are keyed by
//! `(core_a, core_b)` in ascending order and, within one pool, tasks are placed
//! in the global (name) task order, so identical inputs always produce equal
//! offsets. The `system.json` emit ([`crate::system_view`]) then expands each
//! pool into its two directions until M6.9-T5 switches the schema to `pools[]`.
//!
//! A project whose manifests carry no capability table at all (the minimal
//! `metadata-macro` fixture, until M6.9-T9 supplies the mock table) has no pool
//! graph: its FIFOs are allocated sequentially per direction and no pool is
//! emitted. Real projects always bind a distribution, so this path only
//! affects that stand-in.
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
use crate::manifest::{AppManifest, PoolDecl, ReceiverDecl, simple_type_name};
use crate::project::{Application, ProjectConfig};
use crate::system::{
    AppEntry, CoreEntry, FieldEntry, FifoEntry, PoolEntry, TypeEntry, TypeKind, VariantEntry,
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
    pools: Vec<PoolEntry>,
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

    /// Returns every matched distro IPC pool, ordered by `(core_a, core_b)`.
    pub fn pools(&self) -> &[PoolEntry] {
        &self.pools
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

    /// Returns the byte offset of the task FIFO inside its pool.
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

    /// Returns the total FIFO footprint inside its pool, in bytes.
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

    // -- distro pool graph ---------------------------------------------------
    // Since M6.9 the distribution owns IPC memory: each application manifest
    // reports, per local core, its physical core and the pools that physical
    // core can reach. Matching the two endpoints of a dual yields one shared
    // pool per core pair (M6.9-T4).
    let mut graph = match_pools(applications, &manifest_of)?;

    // -- FIFO allocation and pool fit ----------------------------------------
    // Each pool is an independent cursor; within a pool the tasks are placed in
    // the global (name) task order, each FIFO aligned to `FIFO_ALIGN`. Both
    // directions of a dual share the pool budget, so the first FIFO that does
    // not fit names its pool and the bytes needed.
    let mut unbounded: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for task in &mut tasks {
        let spawner = task.spawner_core;
        let receiver = task.receiver_core;
        let name = task.name.clone();
        let capacity = task.capacity;
        let pair = ordered_core_pair(spawner, receiver);
        let bytes =
            fifo_size(task.elem_size, capacity).ok_or_else(|| MergeError::TaskTooLarge {
                task: name.clone(),
                capacity,
            })?;

        if let Some(pool_index) = graph.by_pair.get(&pair).copied() {
            let (start, end, pool, core_a, core_b, budget) = {
                let pool = &graph.pools[pool_index];
                let start =
                    align_up(u64::from(pool.used)).ok_or_else(|| MergeError::TaskTooLarge {
                        task: name.clone(),
                        capacity,
                    })?;
                let end = start
                    .checked_add(bytes)
                    .ok_or_else(|| MergeError::TaskTooLarge {
                        task: name.clone(),
                        capacity,
                    })?;
                (
                    start,
                    end,
                    pool.id.clone(),
                    pool.core_a,
                    pool.core_b,
                    pool.budget,
                )
            };
            if end > u64::from(budget) {
                return Err(MergeError::PoolBudgetExceeded {
                    pool,
                    core_a,
                    core_b,
                    needed: end,
                    budget,
                });
            }
            task.offset = u32::try_from(start).map_err(|_| MergeError::TaskTooLarge {
                task: name.clone(),
                capacity,
            })?;
            graph.pools[pool_index].used =
                u32::try_from(end).map_err(|_| MergeError::TaskTooLarge {
                    task: name.clone(),
                    capacity,
                })?;
        } else if graph.present {
            return Err(MergeError::NoIpcPath {
                task: name,
                producer: spawner,
                target: receiver,
            });
        } else {
            // Transitional no-capability path (M6.9-T9 supplies the mock
            // capability table to the fixtures): allocate sequentially per
            // direction, without a budget.
            let cursor = unbounded.entry(pair).or_insert(0);
            let start = align_up(*cursor).ok_or_else(|| MergeError::TaskTooLarge {
                task: name.clone(),
                capacity,
            })?;
            let end = start
                .checked_add(bytes)
                .ok_or_else(|| MergeError::TaskTooLarge {
                    task: name.clone(),
                    capacity,
                })?;
            task.offset = u32::try_from(start).map_err(|_| MergeError::TaskTooLarge {
                task: name.clone(),
                capacity,
            })?;
            *cursor = end;
        }
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

    Ok(MergedProject {
        apps,
        cores,
        types: type_entries(idl, &layouts)?,
        tasks,
        pools: graph.pools,
    })
}

/// A distro pool matched from both endpoints' capability entries, oriented so
/// `core_a < core_b`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchedPool {
    id: String,
    core_a: u32,
    core_b: u32,
    base_from_a: u32,
    base_from_b: u32,
    budget: u32,
}

/// The matched distro pool graph of a project (M6.9-T4).
struct PoolGraph {
    /// One entry per matched dual, ordered by `(core_a, core_b)`; `used` is
    /// filled in by FIFO allocation.
    pools: Vec<PoolEntry>,
    /// `(core_a, core_b)` -> index into `pools`.
    by_pair: BTreeMap<(u32, u32), usize>,
    /// Whether any application bound a distribution capability table.
    present: bool,
}

/// Orders two global core ids ascending.
fn ordered_core_pair(left: u32, right: u32) -> (u32, u32) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

/// Builds the project's pool graph from the manifests' capability bindings
/// (M6.9-T4).
///
/// The distribution reports, per physical core, the IPC pools it can reach.
/// Two cores `A` and `B` form a dual exactly when `A` reports a pool whose
/// `peer` is `B`'s physical core **and** `B` reports the matching pool: the
/// same id, the opposite physical core, the same budget and policy, and the
/// two base views swapped. A one-sided or inconsistent pair is
/// [`MergeError::IpcPoolMismatch`].
///
/// The matched pools are oriented so `core_a < core_b` and checked for
/// per-core view disjointness: a core may take part in several pools and the
/// views it has of them must not overlap ([`MergeError::PoolViewOverlap`]).
/// Pools whose peer is not a core of the project are ignored.
///
/// The [`PoolGraph::present`] flag reports whether any application bound a
/// distribution capability table at all. Without one (the minimal
/// `metadata-macro` fixture, until M6.9-T9 supplies the mock capability table)
/// the graph is empty and the caller allocates without a budget.
fn match_pools(
    applications: &[Application],
    manifest_of: &BTreeMap<&str, &AppManifest>,
) -> Result<PoolGraph, MergeError> {
    // Global core id -> distro physical core id, and the pools it reports.
    let mut physical_of: BTreeMap<u32, u32> = BTreeMap::new();
    let mut pools_of: BTreeMap<u32, &[PoolDecl]> = BTreeMap::new();
    let mut present = false;
    for application in applications {
        let manifest = manifest_of[application.package()];
        if manifest.capabilities.is_empty() {
            continue;
        }
        present = true;
        for capability in &manifest.capabilities {
            let Some(&global) = application.core_ids().get(capability.local_core as usize) else {
                continue;
            };
            physical_of.insert(global, capability.physical_core);
            pools_of.insert(global, capability.pools.as_slice());
        }
    }

    // Physical core id -> global core id, so a pool's `peer` resolves to the
    // application that runs on that physical core. The lowest global id wins
    // when several cores report the same physical id, keeping the mapping
    // deterministic.
    let mut global_of_physical: BTreeMap<u32, u32> = BTreeMap::new();
    for (global, physical) in &physical_of {
        global_of_physical.entry(*physical).or_insert(*global);
    }

    let mut matched: BTreeMap<(u32, u32), MatchedPool> = BTreeMap::new();
    for application in applications {
        let manifest = manifest_of[application.package()];
        for capability in &manifest.capabilities {
            let Some(&source) = application.core_ids().get(capability.local_core as usize) else {
                continue;
            };
            let Some(&source_physical) = physical_of.get(&source) else {
                continue;
            };
            for pool in &capability.pools {
                let Some(&target) = global_of_physical.get(&pool.peer) else {
                    continue; // a pool to a core outside the project
                };
                if target == source {
                    continue;
                }
                let (core_a, core_b) = ordered_core_pair(source, target);
                let mismatch = || MergeError::IpcPoolMismatch {
                    id: pool.id.clone(),
                    core_a,
                    core_b,
                };

                // The peer must report the same pool id back to this core.
                let reverse = pools_of.get(&target).and_then(|pools| {
                    pools.iter().find(|candidate| {
                        candidate.id == pool.id && candidate.peer == source_physical
                    })
                });
                let Some(reverse) = reverse else {
                    return Err(mismatch());
                };
                if reverse.budget != pool.budget
                    || reverse.policy != pool.policy
                    || reverse.base_local != pool.base_peer
                    || reverse.base_peer != pool.base_local
                {
                    return Err(mismatch());
                }

                let (base_from_a, base_from_b) = if source == core_a {
                    (pool.base_local, pool.base_peer)
                } else {
                    (pool.base_peer, pool.base_local)
                };
                let candidate = MatchedPool {
                    id: pool.id.clone(),
                    core_a,
                    core_b,
                    base_from_a,
                    base_from_b,
                    budget: pool.budget,
                };
                match matched.get(&(core_a, core_b)) {
                    Some(existing) if existing != &candidate => return Err(mismatch()),
                    Some(_) => {}
                    None => {
                        matched.insert((core_a, core_b), candidate);
                    }
                }
            }
        }
    }

    // Per-core view disjointness: each pool contributes its `core_a` view to
    // `core_a` and its `core_b` view to `core_b`; a core's ranges must be
    // pairwise disjoint on `[base, base + budget)`.
    let mut by_core: BTreeMap<u32, Vec<(u32, u64, &str)>> = BTreeMap::new();
    for pool in matched.values() {
        by_core.entry(pool.core_a).or_default().push((
            pool.base_from_a,
            u64::from(pool.base_from_a) + u64::from(pool.budget),
            pool.id.as_str(),
        ));
        by_core.entry(pool.core_b).or_default().push((
            pool.base_from_b,
            u64::from(pool.base_from_b) + u64::from(pool.budget),
            pool.id.as_str(),
        ));
    }
    for (core, views) in &mut by_core {
        views.sort_unstable();
        for pair in views.windows(2) {
            let (first_base, first_end, first) = pair[0];
            let (second_base, second_end, second) = pair[1];
            if u64::from(second_base) < first_end {
                return Err(MergeError::PoolViewOverlap {
                    core: *core,
                    first: first.to_string(),
                    second: second.to_string(),
                    first_base,
                    first_end,
                    second_base,
                    second_end,
                });
            }
        }
    }

    let mut by_pair = BTreeMap::new();
    let mut pools = Vec::with_capacity(matched.len());
    for (key, pool) in matched {
        by_pair.insert(key, pools.len());
        pools.push(PoolEntry {
            id: pool.id,
            core_a: pool.core_a,
            core_b: pool.core_b,
            base_from_a: pool.base_from_a,
            base_from_b: pool.base_from_b,
            budget: pool.budget,
            used: 0,
        });
    }

    Ok(PoolGraph {
        pools,
        by_pair,
        present,
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
