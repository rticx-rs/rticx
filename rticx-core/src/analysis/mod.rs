use std::collections::HashSet;

use syn::Ident;
use syn::spanned::Spanned;

use crate::App;
use crate::parser::SubApp;
use crate::parser::ast::{HardwareTask, SharedResources};
#[derive(Debug, Clone)]
pub struct Analysis {
    pub sub_analysis: Vec<SubAnalysis>,
    pub task_traits: HashSet<syn::Ident>,
}

impl Analysis {
    /// - updates resource ceilings
    /// - collects and structure key information about the user application to be used during code generation
    /// - collect the task traits
    pub fn run(parsed_app: &mut App) -> syn::Result<Self> {
        // update resource ceilings
        for app in parsed_app.sub_apps.iter_mut() {
            update_resource_priorities(app.shared.as_mut(), &app.tasks)?;
        }

        // collect and structure key information about the user application to be used during code generation
        let sub_analysis = parsed_app
            .sub_apps
            .iter()
            .map(SubAnalysis::run)
            .collect::<syn::Result<_>>()?;

        let mut task_traits = HashSet::new();
        for subapp in parsed_app.sub_apps.iter() {
            for task in subapp.tasks.iter() {
                task_traits.insert(task.args.task_trait.clone());
            }
            if let Some(idle) = &subapp.idle {
                task_traits.insert(idle.args.task_trait.clone());
            }
        }

        Ok(Self {
            sub_analysis,
            task_traits,
        })
    }
}

#[derive(Debug, Clone)]
pub struct UsedIrq {
    pub name: Ident,
    pub priority: u16,
}

#[derive(Debug, Clone)]
pub struct SubAnalysis {
    // used interrupts and their priorities
    pub used_irqs: Vec<UsedIrq>,
    // tasks the user must initialize through the `TaskInits` struct returned by `#[init]`
    pub late_resource_tasks: Vec<LateResourceTask>,
}

impl SubAnalysis {
    pub fn run(app: &SubApp) -> syn::Result<Self> {
        // hw interrupts bound to hardware tasks
        let used_interrupts = app
            .tasks
            .iter()
            .filter_map(|t| {
                Some(UsedIrq {
                    name: t.args.binds.clone()?,
                    priority: t.args.priority,
                })
            })
            .collect();

        // All user tasks must be initialized explicitly by the user through the
        // `TaskInits` struct returned by `#[init]`. Only framework-generated
        // tasks (marked with `init = generated`) are constructed by the
        // framework itself.
        let user_initializable_tasks = app
            .tasks
            .iter()
            .chain(app.idle.iter()) // idle is also a task and we shouldn't forget it
            .filter_map(|t| {
                if t.args.init_generated {
                    None
                } else {
                    Some(LateResourceTask {
                        task_name: t.task_struct.ident.clone(),
                    })
                }
            })
            .collect();

        Ok(Self {
            used_irqs: used_interrupts,
            late_resource_tasks: user_initializable_tasks,
        })
    }
}

fn update_resource_priorities(
    shared: Option<&mut SharedResources>,
    hw_tasks: &[HardwareTask],
) -> syn::Result<()> {
    let Some(shared) = shared else { return Ok(()) };
    for task in hw_tasks.iter() {
        let task_priority = task.args.priority;
        for resource_ident in task.args.shared.iter() {
            if let Some(shared_element) = shared.get_field_mut(resource_ident) {
                if shared_element.priority < task_priority {
                    shared_element.priority = task_priority
                }
            } else {
                return Err(syn::Error::new(
                    task.task_struct.span(),
                    format!(
                        "The resource `{resource_ident}` was not found in `{}`",
                        shared.strct.ident
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct LateResourceTask {
    pub task_name: Ident,
}
impl LateResourceTask {
    /// By convention, this method is used to generate the name of the static task instance
    pub fn name_uppercase(&self) -> Ident {
        crate::parser::ast::uppercase_ident(&self.task_name)
    }

    pub fn name_snakecase(&self) -> Ident {
        crate::parser::ast::snakecase_ident(&self.task_name)
    }
}
