use std::collections::BTreeMap;

use crate::common::analyze::{
    assign_dispatchers, check_disjoint_priorities, check_uniform_spawn_by,
};
use crate::parse::ast::SoftwareTask;
use crate::parse::{App, SubApp};

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
    /// The `u32` is the core that is allowed to spawn the task, and the
    /// `usize` is the capacity of the task's input queue.
    pub tasks_priority_map: BTreeMap<u16, Vec<(syn::Ident, u32, usize)>>,
    /// Maps every priority level to a dispatcher
    pub dispatcher_priority_map: BTreeMap<u16, syn::Path>,
}

impl SubAnalysis {
    fn analyse_subapp(sub_app: &SubApp) -> syn::Result<Self> {
        // group sw tasks based on their associated priorities
        let sw_tasks_pgroups = group_by_priority(sub_app.sw_tasks.iter(), |_| sub_app.core);
        // group multicore sw tasks based on their associated priorities
        let mc_tasks_pgroups =
            group_by_priority(sub_app.mc_sw_tasks.iter(), |task| task.params.spawn_by);

        check_disjoint_priorities(&sw_tasks_pgroups, &mc_tasks_pgroups, sub_app.core)?;
        check_uniform_spawn_by(&mc_tasks_pgroups)?;

        // now we can merge all priority groups together since we know they are disjoint and no overlap exists
        let mut tasks_priority_map = sw_tasks_pgroups;
        tasks_priority_map.extend(mc_tasks_pgroups);

        let dispatcher_priority_map = assign_dispatchers(
            &tasks_priority_map,
            &sub_app.dispatchers,
            sub_app.dispatchers_span,
        )?;

        Ok(Self {
            core: sub_app.core,
            tasks_priority_map,
            dispatcher_priority_map,
        })
    }
}

/// Groups tasks by priority.  `spawner_core` selects the core recorded for
/// each task: the task's own core for core-local tasks, or the spawning core
/// for multi-core tasks.
fn group_by_priority<'a>(
    tasks: impl Iterator<Item = &'a SoftwareTask>,
    spawner_core: impl Fn(&SoftwareTask) -> u32,
) -> BTreeMap<u16, Vec<(syn::Ident, u32, usize)>> {
    let mut groups: BTreeMap<u16, Vec<(syn::Ident, u32, usize)>> = BTreeMap::new();
    for task in tasks {
        groups.entry(task.params.priority).or_default().push((
            task.name().clone(),
            spawner_core(task),
            task.params.capacity,
        ));
    }
    groups
}
