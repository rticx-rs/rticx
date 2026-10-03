//! Whole-project system view emission (M1-T6).
//!
//! [`system_view`] converts the merged and allocated [`MergedProject`] into
//! the driver's `target/rticx-xbin/system.json` view: it copies the
//! applications, cores and types, copies the tasks with their FIFO
//! allocations and their pool ids, copies the matched distro pools, assigns
//! the doorbell lines, sets the IDL `layout_hash` and seals the topology hash.
//!
//! Determinism is a hard requirement (see `multibinary-multicore-plan.md`
//! §7.1): the merged project is deterministic, the doorbell assignment below
//! is sorted, and [`SystemView`] serializes canonically, so identical inputs
//! produce byte-identical `system.json`.
//!
//! Doorbells: one line per distinct `(source, target, priority)` task line
//! (tasks of one source sharing a priority share a line). Line indices are
//! unique per target core and assigned in `(source, priority)` order starting
//! at 0, so the target binary can map every line to exactly one doorbell ISR
//! (`multibinary-multicore-plan.md` §8/§10). The emitted vector is sorted by
//! `(source, target, priority)`.

use std::collections::{BTreeMap, BTreeSet};

use crate::codegen::layout_hash;
use crate::error::CodegenError;
use crate::hash::Hash64;
use crate::idl::IpcTypes;
use crate::merge::MergedProject;
use crate::system::{DoorbellEntry, SystemView, TaskEntry};

/// Builds the `system.json` system view of `project`.
///
/// `generation` is the RTICX generation recorded in the view (normally
/// [`crate::RTICX_GENERATION`]); the `layout_hash` is recomputed from `idl`,
/// matching the `LAYOUT_HASH` of the generated `ipc-types` crate.
pub fn system_view(
    project: &MergedProject,
    idl: &IpcTypes,
    generation: &str,
) -> Result<SystemView, CodegenError> {
    let mut view = SystemView::empty(generation);
    view.layout_hash = Hash64::new(layout_hash(idl)?);
    view.apps = project.apps().to_vec();
    view.cores = project.cores().to_vec();
    view.types = project.types().to_vec();
    view.tasks = project
        .tasks()
        .iter()
        .map(|task| TaskEntry {
            id: task.id(),
            name: task.name().to_string(),
            receiver_core: task.receiver_core(),
            spawner_core: task.spawner_core(),
            priority: task.priority(),
            capacity: task.capacity(),
            input_type: task.input_type().to_string(),
            fifo: task.fifo(),
        })
        .collect();
    // M6.9-T5: `system.json` schema 2 carries the distro pools directly; each
    // task FIFO records the pool id it was allocated in.
    view.pools = project.pools().to_vec();
    view.doorbells = doorbells(project);
    view.seal();
    Ok(view)
}

/// Computes one doorbell per distinct `(source, target, priority)` task line.
fn doorbells(project: &MergedProject) -> Vec<DoorbellEntry> {
    // `(target, source, priority)` order assigns each target core a
    // consecutive, independent block of line indices.
    let mut lines: BTreeSet<(u32, u32, u16)> = BTreeSet::new();
    for task in project.tasks() {
        lines.insert((task.receiver_core(), task.spawner_core(), task.priority()));
    }

    let mut next_line: BTreeMap<u32, u32> = BTreeMap::new();
    let mut doorbells: Vec<DoorbellEntry> = lines
        .into_iter()
        .map(|(target, source, priority)| {
            let line = next_line.entry(target).or_insert(0);
            let entry = DoorbellEntry {
                source,
                target,
                priority,
                line: *line,
            };
            *line += 1;
            entry
        })
        .collect();
    doorbells.sort_by_key(|doorbell| (doorbell.source, doorbell.target, doorbell.priority));
    doorbells
}
