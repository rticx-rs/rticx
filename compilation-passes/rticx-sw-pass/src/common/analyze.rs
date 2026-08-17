//! Shared analysis infrastructure for the software-task compilation passes.
//!
//! Consumed by both `rticx-sw-pass` (the base crate) and `rticx-async-pass`
//! (the extension).  These items are pass-internal plumbing, **not** a stable
//! public API.

use std::collections::BTreeMap;

use proc_macro2::Span;
use syn::{Ident, Path};

/// A priority group entry: (task ident, core allowed to spawn it, input queue capacity).
pub type PriorityGroup = BTreeMap<u16, Vec<(Ident, u32, usize)>>;

/// Ensure that the multi-core tasks do not have overlapping priorities
/// with core-local software tasks.
pub fn check_disjoint_priorities(
    sw_tasks: &PriorityGroup,
    mc_tasks: &PriorityGroup,
    core: u32,
) -> syn::Result<()> {
    for priority in mc_tasks.keys() {
        if sw_tasks.contains_key(priority) {
            let task = &mc_tasks[priority][0].0;
            return Err(syn::Error::new(
                task.span(),
                format!(
                    "The priority of some tasks with `spawn_by` argument in core {core} have overlapping priority with other core-local software tasks, which is forbidden."
                ),
            ));
        }
    }
    Ok(())
}

/// Ensure that multi-core tasks in the same priority group are all
/// spawned by the same core.
pub fn check_uniform_spawn_by(mc_tasks: &PriorityGroup) -> syn::Result<()> {
    for priority_group in mc_tasks.values() {
        let Some((task_1, spawn_by1, _)) = priority_group.first() else {
            continue;
        };
        for (task_x, spawn_byx, _) in priority_group {
            if spawn_by1 != spawn_byx {
                return Err(syn::Error::new(
                    task_x.span(),
                    format!(
                        "{task_1} and {task_x} have the same priority but they are spawned by different cores which is forbidden."
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Assign one dispatcher per priority group, in declaration order.
///
/// `BTreeMap` iteration yields priorities in ascending order, so the first
/// declared dispatcher handles the lowest priority group.  Errors if fewer
/// dispatchers than priority groups are provided.
pub fn assign_dispatchers(
    tasks_priority_map: &PriorityGroup,
    dispatchers: &[Path],
    dispatchers_span: Option<Span>,
) -> syn::Result<BTreeMap<u16, Path>> {
    let n_dispatchers = dispatchers.len();
    let n_priority_groups = tasks_priority_map.len();
    if n_dispatchers < n_priority_groups {
        return Err(syn::Error::new(
            dispatchers_span.unwrap_or_else(Span::call_site),
            format!("Expected {n_priority_groups} dispatchers, but found {n_dispatchers}."),
        ));
    }

    let mut dispatchers = dispatchers.iter();
    Ok(tasks_priority_map
        .keys()
        .map(|&priority| {
            let dispatcher = dispatchers
                .next()
                .expect("checked above: enough dispatchers for every priority group");
            (priority, dispatcher.clone())
        })
        .collect())
}
