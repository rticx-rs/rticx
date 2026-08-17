mod utils;

use crate::SwPassBackend;
use crate::analyze::{Analysis, SubAnalysis};
use crate::common::codegen::{
    SpawnApiParams, generate_cross_pend_fns, generate_local_pend_fns, generate_spawn_api,
    get_interrupt_path,
};
use crate::parse::ast::SoftwareTask;
use crate::parse::{App, SWT_TRAIT_TY};
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{ItemMod, LitInt, Path, parse_quote};

pub struct CodeGen<'a> {
    app: App,
    analysis: Analysis,
    backend: &'a dyn SwPassBackend,
}

impl<'a> CodeGen<'a> {
    pub fn new(app: App, analysis: Analysis, backend: &'a dyn SwPassBackend) -> CodeGen<'a> {
        Self {
            app,
            analysis,
            backend,
        }
    }

    pub fn run(&self) -> ItemMod {
        // For every sub-application, generate the software tasks and their dispatchers and associated queues and types.
        let sub_apps = self.generate_subapps();
        let local_pend_fns = self.get_local_pend_fns();
        let cross_pend_fns = self.get_cross_pend_fns();
        let rest_of_code = &self.app.rest_of_code;
        let software_task_trait = format_ident!("{SWT_TRAIT_TY}");
        let sw_task_trait_def = quote! {
            /// Trait for a software task
            pub trait #software_task_trait {
                type SpawnInput;
                /// Function to be executing when the scheduled software task is dispatched
                fn exec(&mut self, input: Self::SpawnInput);
            }
        };
        let mod_visibility = &self.app.mod_visibility;
        let mod_ident = &self.app.mod_ident;

        parse_quote! {
            #mod_visibility mod #mod_ident {
                #(#rest_of_code)*
                #sub_apps
                /// RTIC Software task trait
                #sw_task_trait_def
                /// Core local interrupt pending
                #local_pend_fns
                // (optional) Cross Core interrupt pending
                #cross_pend_fns
                /// Flag set to true after system initialization completes
                static mut __rticx_sw_system_initialized: bool = false;
            }
        }
    }

    /// Compute the interrupt type path for the dispatcher on a given core.
    ///
    /// Uses the backend's `custom_interrupt_path` if provided, otherwise falls
    /// back to `pac[core]::Interrupt`.
    fn get_interrupt_path(&self, core: u32) -> Path {
        get_interrupt_path(self.backend, &self.app.app_params.pacs, core)
    }

    /// Generate the core-local interrupt-pending functions.
    ///
    /// One function is generated per core.  In single-core apps the function
    /// keeps the historical name `__rticx_local_irq_pend`; for multi-core apps
    /// the core index is appended (`__rticx_local_irq_pend_core{N}`).
    fn get_local_pend_fns(&self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let cores = self
            .app
            .sub_apps
            .iter()
            .map(|sub_app| (sub_app.core, self.get_interrupt_path(sub_app.core)));
        generate_local_pend_fns(self.backend, cores, num_cores)
    }

    /// Generate the cross-core interrupt-pending functions.
    ///
    /// One function is generated per *target* core that actually has cross-core
    /// tasks.  The function name includes the target core index.
    fn get_cross_pend_fns(&self) -> TokenStream {
        let cores = self
            .app
            .sub_apps
            .iter()
            .filter(|sub_app| !sub_app.mc_sw_tasks.is_empty())
            .map(|sub_app| (sub_app.core, self.get_interrupt_path(sub_app.core)));
        generate_cross_pend_fns(self.backend, cores)
    }

    fn generate_subapps(&self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let queue_path = self.backend.queue_path();
        let apps = self.app.sub_apps.iter();
        let analysis = self.analysis.sub_analysis.iter();

        let sub_apps = apps.zip(analysis).map(|(sub_app, sub_analysis)| {
            let pac = &self.app.app_params.pacs[sub_app.core as usize];
            // first merge the multi-core and core local tasks as the same code will be generated for both
            let tasks_iter = sub_app.sw_tasks.iter().chain(sub_app.mc_sw_tasks.iter());
            // Re-generate the software tasks definitions and generate the spawn() api for each task
            let sw_tasks = tasks_iter.map(|task| {
                // The task attribute was already reconstructed by the parsing
                // phase: `sw_task` renamed to `task`, pass-only keys removed,
                // and the `task_trait = RticSwTask` argument added.
                let task_attr = &task.task_attr;
                let task_struct = &task.task_struct;
                let task_impl = &task.task_impl;
                // generate the spawn() function for this software task
                let dispatcher = sub_analysis
                    .dispatcher_priority_map
                    .get(&task.params.priority)
                    .expect("analysis assigns a dispatcher to every priority group"); // safe to unwrap
                let spawn_impl =
                    task.generate_spawn_api(dispatcher, pac, self.backend, num_cores, &queue_path);

                quote! {
                    #task_attr
                    #task_struct
                    #task_impl
                    #spawn_impl
                }
            });

            // generate dispatchers as hardware tasks
            let dispatcher_tasks = generate_dispatcher_tasks(sub_analysis, &queue_path);
            quote! {
                #(#sw_tasks)*
                #dispatcher_tasks
            }
        });

        quote! {
            #(#sub_apps)*
        }
    }
}

/// generates:
/// - an enum type for each group of tasks of the same priority
/// - a ready queue for each group of tasks of the same priority
/// - A dispatcher hw task for each priority level
fn generate_dispatcher_tasks(sub_analysis: &SubAnalysis, queue_path: &Path) -> TokenStream {
    let core = sub_analysis.core;
    let dispatchers = &sub_analysis.dispatcher_priority_map;
    let dispatcher_tasks = sub_analysis.tasks_priority_map.iter().map(|(prio, tasks)| {
        let prio_ty = utils::priority_ty_ident(*prio, core);

        // generate the branches of the match statement for the dispatcher task
        let dispatch_match_branches = tasks.iter().map(|(task_ident, _, _)| {
            let task_static_handle = utils::ident_uppercase(task_ident);
            let task_inputs_queue = utils::sw_task_inputs_ident(task_ident);
            let prio_ty = &prio_ty;
            quote! {
                #prio_ty::#task_ident => {
                    let mut input_consumer = #task_inputs_queue.split().1;
                    let input = input_consumer.dequeue_unchecked();
                    #task_static_handle.assume_init_mut().exec(input);
                }
            }
        });

        let ready_queue_name = utils::priority_queue_ident(&prio_ty);
        // Each pending spawn of a task occupies one ready-queue slot, and the
        // number of pending spawns is bounded by the task's input-queue
        // capacity. The ring buffer needs one extra unused slot.
        let ready_queue_size = tasks.iter().map(|(_, _, capacity)| capacity).sum::<usize>() + 1;
        let dispatcher_irq_name = dispatchers.get(prio).unwrap(); // safe to unwrap due to guarantees from analysis
        let dispatcher_priority = prio;
        let dispatcher_task_ty = utils::dispatcher_ident(*prio, core);
        let core_nbr = LitInt::new(&core.to_string(), Span::call_site());
        let tasks = tasks.iter().map(|(ident, _, _)| ident);

        quote! {
            #[derive(Clone, Copy)]
            #[doc(hidden)]
            pub enum #prio_ty {
                #(#tasks,)*
            }

            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            static mut #ready_queue_name: #queue_path<#prio_ty, #ready_queue_size> = #queue_path::new();

            #[doc(hidden)]
            #[task( binds = #dispatcher_irq_name , priority = #dispatcher_priority, core = #core_nbr, init = generated)]
            pub struct #dispatcher_task_ty;

            impl RticTask for #dispatcher_task_ty {
                fn exec(&mut self) {
                    unsafe {
                        let mut ready_consumer = #ready_queue_name.split().1;
                        while let Some(task) = ready_consumer.dequeue() {
                            match task {
                                #(#dispatch_match_branches)*
                            }
                        }
                    }
                }
            }
        }
    });

    quote! {
        #(#dispatcher_tasks)*
    }
}

impl SoftwareTask {
    /// generate the spawn()/cross_spawn() function for the task
    fn generate_spawn_api(
        &self,
        dispatcher_irq_name: &Path,
        peripheral_crate: &Path,
        backend: &dyn SwPassBackend,
        num_cores: usize,
        queue_path: &Path,
    ) -> TokenStream {
        let task_name = self.name();
        let task_trait_name = format_ident!("{}", SWT_TRAIT_TY);
        // get the inputs type. see the RticSwTask trait to understand this and where it comes from.
        let inputs_ty = quote!(<#task_name as #task_trait_name>::SpawnInput);
        let prio_ty = utils::priority_ty_ident(self.params.priority, self.params.core);
        let ready_queue_name = utils::priority_queue_ident(&prio_ty);

        let interrupt_ty = backend
            .custom_interrupt_path(self.params.core)
            .unwrap_or(parse_quote!(#peripheral_crate::Interrupt));
        // ring buffer holds one slot more than the queue capacity
        let queue_buffer_size = self.params.capacity + 1;

        let cross = self.params.core != self.params.spawn_by;
        let pend_stmt = if cross {
            let pend_fn = utils::cross_pend_fn_ident(self.params.core);
            quote!(#pend_fn(#interrupt_ty::#dispatcher_irq_name).map_err(|_| None))
        } else {
            let pend_fn = utils::local_pend_fn_ident(self.params.core, num_cores);
            quote!(#pend_fn(#interrupt_ty::#dispatcher_irq_name);)
        };
        let pend_stmt = Some(pend_stmt);

        let core_check = if cross {
            // Multicore-only Runtime check that the caller runs on this task's `spawn_by` core.
            let spawn_by_lit = LitInt::new(&self.params.spawn_by.to_string(), Span::call_site());
            backend.current_core_id().map(|current_core_id| {
                quote! {
                    if #current_core_id != #spawn_by_lit {
                        return Err(Some(input));
                    }
                }
            })
        } else {
            // Optional runtime check that the caller runs on this task's core.
            let core_lit = LitInt::new(&self.params.core.to_string(), Span::call_site());
            backend.current_core_id().map(|current_core_id| {
                quote! {
                    if #current_core_id != #core_lit {
                        return Err(input);
                    }
                }
            })
        };

        let system_initialized_flag = format_ident!("__rticx_sw_system_initialized");
        generate_spawn_api(&SpawnApiParams {
            task_name,
            inputs_ty: &inputs_ty,
            prio_ty: &prio_ty,
            ready_queue_name: &ready_queue_name,
            queue_path,
            queue_buffer_size,
            system_initialized_flag: &system_initialized_flag,
            cross,
            pend_stmt,
            core_check,
        })
    }
}
