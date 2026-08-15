use std::collections::BTreeMap;

use crate::parse::{App, SubApp};
use proc_macro2::Span;

#[derive(Clone)]
pub struct Analysis {
    /// analysis for every sub-application (per-core analysis)
    pub sub_analysis: Vec<SubAnalysis>,
}

impl Analysis {
    pub fn run(app: &App) -> syn::Result<Self> {
        let sub_analysis = app
            .sub_apps
            .iter()
            .map(SubAnalysis::analyse_subapp)
            .collect::<syn::Result<_>>()?;
        Ok(Self { sub_analysis })
    }
}

/// Per-core/Sub application analysis
///
/// The priority maps are `BTreeMap`s so that iteration (and therefore the
/// generated code and the dispatcher assignment) is deterministic.
#[derive(Debug, Clone)]
pub struct SubAnalysis {
    pub core: u32,
    /// Maps every group of software tasks to some priority level
    /// Tasks are identified by their `Ident` (the name of the task struct)
    /// The `u32` is the core allowed to spawn the task, and the `usize` is
    /// the capacity of the task's input queue.
    pub tasks_priority_map: BTreeMap<u16, Vec<(syn::Ident, u32, usize)>>,
    /// Maps every priority level to a dispatcher
    pub dispatcher_priority_map: BTreeMap<u16, syn::Path>,
    /// Priority-0 tasks that run on the idle executor (no dispatcher needed)
    pub prio_0_tasks: Vec<(syn::Ident, u32, usize)>,
}

impl SubAnalysis {
    fn analyse_subapp(sub_app: &SubApp) -> syn::Result<Self> {
        // group sw tasks based on their associated priorities (skip priority 0)
        let mut sw_tasks_pgroups: BTreeMap<u16, Vec<(syn::Ident, u32, usize)>> = BTreeMap::new();
        let mut prio_0_tasks = Vec::new();
        for task in sub_app.sw_tasks.iter() {
            let task_prio = task.params.priority;
            if task_prio == 0 {
                prio_0_tasks.push((task.name().clone(), sub_app.core, task.params.capacity));
            } else {
                sw_tasks_pgroups.entry(task_prio).or_default().push((
                    task.name().clone(),
                    sub_app.core, /* core local tasks*/
                    task.params.capacity,
                ));
            }
        }

        // group multicore sw tasks based on their associated priorities (skip priority 0)
        let mut mc_tasks_pgroups: BTreeMap<u16, Vec<(syn::Ident, u32, usize)>> = BTreeMap::new();
        for task in sub_app.mc_sw_tasks.iter() {
            let task_prio = task.params.priority;
            if task_prio == 0 {
                return Err(syn::Error::new(
                    task.name().span(),
                    format!(
                        "Async task `{}`: cross-core spawn (spawn_by != core) is not supported for priority-0 tasks. Use priority > 0 instead.",
                        task.name()
                    ),
                ));
            }
            mc_tasks_pgroups.entry(task_prio).or_default().push((
                task.name().clone(),
                task.params.spawn_by,
                task.params.capacity,
            ));
        }

        Self::check_disjoint_priorities(&sw_tasks_pgroups, &mc_tasks_pgroups, sub_app.core)?;
        Self::check_uniform_spawn_by(&mc_tasks_pgroups)?;

        // now we can merge all priority groups together since we know they are disjoint and no overlap exists
        let mut tasks_priority_map = sw_tasks_pgroups;
        tasks_priority_map.extend(mc_tasks_pgroups);

        // check if the number of dispatchers meets the number of sw task priority groups
        let n_dispatchers = sub_app.dispatchers.len();
        let n_priority_groups = tasks_priority_map.len();
        if n_dispatchers < n_priority_groups {
            return Err(syn::Error::new(
                sub_app.dispatchers_span.unwrap_or_else(Span::call_site),
                format!("Expected {n_priority_groups} dispatchers, but found {n_dispatchers}."),
            ));
        }

        // map dispatchers to priorities: one dispatcher per priority group,
        // assigned in declaration order.  BTreeMap iteration yields the
        // priorities in ascending order, so the first dispatcher handles the
        // lowest priority group.
        let mut dispatchers = sub_app.dispatchers.iter();
        let dispatcher_priority_map = tasks_priority_map
            .keys()
            .map(|&priority| {
                let dispatcher = dispatchers
                    .next()
                    .expect("checked above: enough dispatchers for every priority group");
                (priority, dispatcher.clone())
            })
            .collect();

        Ok(Self {
            core: sub_app.core,
            tasks_priority_map,
            dispatcher_priority_map,
            prio_0_tasks,
        })
    }

    /// Ensure that the multi-core tasks do not have overlapping priorities
    /// with core-local software tasks.
    fn check_disjoint_priorities(
        sw_tasks: &BTreeMap<u16, Vec<(syn::Ident, u32, usize)>>,
        mc_tasks: &BTreeMap<u16, Vec<(syn::Ident, u32, usize)>>,
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
    fn check_uniform_spawn_by(
        mc_tasks: &BTreeMap<u16, Vec<(syn::Ident, u32, usize)>>,
    ) -> syn::Result<()> {
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
}
