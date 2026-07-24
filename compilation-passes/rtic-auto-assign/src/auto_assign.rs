use std::{collections::HashMap, sync::atomic::Ordering};

use crate::{
    error::Error,
    parse::{App, APP_CORES},
};

pub fn run(parsed_app: &mut App) -> syn::Result<()> {
    // create a mapping for a resource name and the core that it belongs to
    let mut resource_core_map = HashMap::new();
    for shared in parsed_app.shared_resources.iter() {
        let core = shared.core;
        for element in shared.shared_items.iter() {
            if resource_core_map.insert(element, core).is_some() {
                return Err(Error::DuplicatResourceName(element.to_string()).into());
            }
        }
    }

    // shared resource structs are always less than task structs, so it is reasonable to make the outer loop
    // iterate over tasks structs instead of iterating over shared resources structs
    for task in parsed_app.tasks.iter_mut() {
        let task_name = &task.task_struct.ident;
        if task.core.is_some() {
            continue;
        } else if APP_CORES.load(Ordering::Relaxed) == 1 {
            task.assign_core(0);
            continue;
        } else if task.shared_items.is_empty() {
            return Err(Error::ExplicitCoreNeeded(task_name.to_string()).into());
        }

        // at this point we have a task that has at least one shared resource and a None for the `core` value
        let mut shared_iter = task.shared_items.iter();
        let first_element: &syn::Ident = shared_iter.next().unwrap(); // safe to unwrap
        let assumed_core = resource_core_map
            .get(first_element)
            .ok_or(Error::ResourceNotFound(first_element.to_string()))?;

        // check that the other resources also belong to the same core
        if shared_iter.any(|e| resource_core_map.get(e) != Some(assumed_core)) {
            return Err(Error::CoreMimatch(task_name.to_string(), *assumed_core).into());
        }

        // assign a core to task
        task.assign_core(*assumed_core);
    }
    Ok(())
}
