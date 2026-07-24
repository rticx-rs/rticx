use std::collections::{HashMap, HashSet};

use crate::software_pass::parse::{App, SubApp};
use proc_macro2::Span;

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
#[derive(Debug)]
pub struct SubAnalysis {
    pub core: u32,
    /// Maps every group of software tasks to some priority level
    /// Tasks are identified by their `Ident` (the name of the task struct)
    pub tasks_priority_map: HashMap<u16, Vec<(syn::Ident, u32)>>,
    /// Maps every dispatcher to a priority level
    pub dispatcher_priority_map: HashMap<u16, syn::Path>,
}

impl SubAnalysis {
    fn analyse_subapp(sub_app: &SubApp) -> syn::Result<Self> {
        // group sw tasks based on their associated priorities
        let mut sw_tasks_pgroups: HashMap<u16, Vec<_>> =
            HashMap::with_capacity(sub_app.dispatchers.len());
        for task in sub_app.sw_tasks.iter() {
            let task_prio = task.params.priority;
            sw_tasks_pgroups
                .entry(task_prio)
                .or_default()
                .push((task.name().clone(), sub_app.core /* core local tasks*/));
        }

        // group multicore sw tasks based on their associated priorities
        let mut mc_tasks_pgroups: HashMap<u16, Vec<_>> =
            HashMap::with_capacity(sub_app.dispatchers.len());
        for task in sub_app.mc_sw_tasks.iter() {
            let task_prio = task.params.priority;
            mc_tasks_pgroups
                .entry(task_prio)
                .or_default()
                .push((task.name().clone(), task.params.spawn_by));
        }

        // ensure that the multi-core tasks do not have overlapping priorities with core local software tasks
        let sw_tasks_prios = sw_tasks_pgroups.keys().collect::<HashSet<_>>();
        let mc_tasks_prios = mc_tasks_pgroups.keys().collect::<HashSet<_>>();
        let disjoint = sw_tasks_prios.is_disjoint(&mc_tasks_prios);
        if !disjoint {
            return Err(syn::Error::new(
                Span::call_site(),
                format!("The priority of some tasks with `spawn_by` argument in core {} have overlapping priority with other core-local software tasks, which is forbidden.", sub_app.core),
            ));
        }

        // need to further check that multi core tasks in the same priority group must all have the spawn_by index.
        for priority_group in mc_tasks_pgroups.values() {
            if priority_group.len() > 1 {
                let (task_1, spawn_by1) = &priority_group[0];
                for (task_x, spawn_byx) in priority_group.iter() {
                    if spawn_by1 != spawn_byx {
                        return Err(syn::Error::new(
                            Span::call_site(),
                            format!("{task_1} and {task_x} have the same priority but they are spawned by different cores which is forbidden."),
                        ));
                    }
                }
            }
        }

        // now we can merge all priority groups together since we know they are disjoint and no overlap exists
        sw_tasks_pgroups.extend(mc_tasks_pgroups);

        // check if the number of dispatchers meets the number of sw task priority groups
        let n_dispatchers = sub_app.dispatchers.len();
        let n_priority_groups = sw_tasks_pgroups.len();
        if n_dispatchers < n_priority_groups {
            return Err(syn::Error::new(
                Span::call_site(),
                format!("Expected {n_priority_groups} dispatchers, but found {n_dispatchers}."),
            ));
        }

        // map dispatchers to priorities
        let mut dispatchers = sub_app.dispatchers.clone();
        let dispatcher_priorities = sw_tasks_pgroups
            .keys()
            .copied()
            .map(|p| (p, dispatchers.pop().unwrap()))
            .collect();

        Ok(Self {
            core: sub_app.core,
            tasks_priority_map: sw_tasks_pgroups,
            dispatcher_priority_map: dispatcher_priorities,
        })
    }
}
