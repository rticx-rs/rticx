mod utils;

use crate::SwPassBackend;
use crate::software_pass::analyze::{Analysis, SubAnalysis};
use crate::software_pass::parse::ast::SoftwareTask;
use crate::software_pass::parse::{App, SWT_TRAIT_TY};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rtic_core::parse_utils::RticAttr;
use syn::{ItemMod, LitInt, Path, parse_quote};

/// Compute the name of the core-local pend function for `core`.
fn local_pend_fn_ident(core: u32, num_cores: usize) -> Ident {
    if num_cores == 1 {
        format_ident!("{SC_PEND_FN_NAME}")
    } else {
        format_ident!("{SC_PEND_FN_NAME}_core{core}")
    }
}

/// Compute the name of the cross-core pend function for `core`.
fn cross_pend_fn_ident(core: u32) -> Ident {
    format_ident!("{MC_PEND_FN_NAME}_core{core}")
}

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

    pub fn run(&mut self) -> ItemMod {
        // For every sub-application, generate the software tasks and their dispatchers and associated queues and types.
        let sub_apps = self.generate_subapps();
        let local_pend_fns = self.get_local_pend_fns();
        let cross_pend_fns = self.get_cross_pend_fns();
        let rest_of_code = &self.app.rest_of_code;
        let software_task_trait = format_ident!("{SWT_TRAIT_TY}");
        let sw_task_trait_def = quote! {
            /// Trait for a software task
            pub trait #software_task_trait {
                type InitArgs: Sized;
                type SpawnInput;
                /// Task local variables initialization routine
                fn init(args: Self::InitArgs) -> Self;
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
            }
        }
    }

    /// Compute the interrupt type path for the dispatcher on a given core.
    ///
    /// Uses the backend's `custom_interrupt_path` if provided, otherwise falls
    /// back to `pac[core]::Interrupt`.
    fn get_interrupt_path(&self, core: u32) -> Path {
        let pac = &self.app.app_params.pacs[core as usize];
        self.backend
            .custom_interrupt_path(core)
            .unwrap_or_else(|| parse_quote!(#pac::Interrupt))
    }

    /// Generate the core-local interrupt-pending functions.
    ///
    /// One function is generated per core.  In single-core apps the function
    /// keeps the historical name `__rtic_local_irq_pend`; for multi-core apps
    /// the core index is appended (`__rtic_local_irq_pend_core{N}`).
    fn get_local_pend_fns(&self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let fns: Vec<TokenStream> = self
            .app
            .sub_apps
            .iter()
            .map(|sub_app| {
                let core = sub_app.core;
                let interrupt_ty = self.get_interrupt_path(core);
                let fn_ident = local_pend_fn_ident(core, num_cores);
                let empty_body_fn = parse_quote! {
                    #[doc(hidden)]
                    #[inline]
                    pub fn #fn_ident(irq_nbr: #interrupt_ty) {
                        // To be implemented by distributor
                        // example:
                        // NVIC::pend( irq );
                    }
                };
                let fn_def = self.backend.generate_local_pend_fn(core, empty_body_fn);
                quote!(#fn_def)
            })
            .collect();
        quote!(#(#fns)*)
    }

    /// Generate the cross-core interrupt-pending functions.
    ///
    /// One function is generated per *target* core that actually has cross-core
    /// tasks.  The function name includes the target core index.
    fn get_cross_pend_fns(&self) -> TokenStream {
        let fns: Vec<TokenStream> = self
            .app
            .sub_apps
            .iter()
            .filter(|sub_app| !sub_app.mc_sw_tasks.is_empty())
            .filter_map(|sub_app| {
                let core = sub_app.core;
                let interrupt_ty = self.get_interrupt_path(core);
                let fn_ident = cross_pend_fn_ident(core);
                let empty_body_fn = parse_quote! {
                    #[doc(hidden)]
                    #[inline]
                    pub fn #fn_ident(irq_nbr: #interrupt_ty) { // TODO: this function should return a result, as pending can fail in multicore !
                        // To be implemented by distributor
                        // How do you pend an interrupt on the other core ?
                    }
                };
                self.backend
                    .generate_cross_pend_fn(core, empty_body_fn)
                    .map(|fn_def| quote!(#fn_def))
            })
            .collect();
        quote!(#(#fns)*)
    }

    fn generate_subapps(&mut self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let queue_path = self.backend.queue_path();
        let apps = self.app.sub_apps.iter_mut();
        let analysis = self.analysis.sub_analysis.iter();

        let sub_apps = apps.zip(analysis).map(|(sub_app, sub_analysis)| {
            let pac = &self.app.app_params.pacs[sub_app.core as usize];
            // first merge the multi-core and core local tasks as the same code will be generated for both
            let tasks_iter = sub_app
                .sw_tasks
                .iter_mut()
                .chain(sub_app.mc_sw_tasks.iter_mut());
            // Re-generate the software tasks definitions and generate the spawn() api for each task
            let sw_tasks = tasks_iter.map(|task| {
                // We will rename the "sw_task" attribute to "task" so that the standard pass recognizes this as a task
                // also, we will add the `task_trait = RticSwTask` argument.

                // first find the index of the sw_task attribute
                let attr_idx = task
                    .task_struct
                    .attrs
                    .iter()
                    .position(|attr| attr.path().is_ident("sw_task"))
                    .expect("A sw task must have a sw_task attribute");

                // Then remove the old attribute as we will reconstruct it
                let attr = task.task_struct.attrs.remove(attr_idx);

                // Now we parse and reconstruct the task attribute
                let mut reconstructed_task_attr = RticAttr::parse_from_attr(&attr).unwrap(); // FIXME: propagate error
                let _ = reconstructed_task_attr.name.insert(format_ident!("task"));
                reconstructed_task_attr
                    .elements
                    .insert("task_trait".into(), syn::parse_str(SWT_TRAIT_TY).unwrap());

                let task_struct = &task.task_struct;
                let task_impl = &task.task_impl;
                // generate the spawn() function for this software task
                let dispatcher = sub_analysis
                    .dispatcher_priority_map
                    .get(&task.params.priority)
                    .unwrap(); // safe to unwrap
                let spawn_impl =
                    task.generate_spawn_api(dispatcher, pac, self.backend, num_cores, &queue_path);

                quote! {
                    #reconstructed_task_attr
                    #task_struct
                    #task_impl
                    #spawn_impl
                }
            });

            // generate dispatchers as hardware tasks
            let dispatcher_tasks = generate_dispatcher_tasks(sub_analysis, &queue_path);
            let core_doc = format!(" Core {}", sub_app.core);
            quote! {
                #[doc = " Software tasks of"]
                #[doc = #core_doc]
                #(#sw_tasks)*

                #[doc = " Dispatchers of"]
                #[doc = #core_doc]
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
        let dispatch_match_branches = tasks.iter().map(|(task_ident, _)| {
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
        let ready_queue_size = tasks.len() + 1; // queue size must always be one more than number of tasks
        let dispatcher_irq_name = dispatchers.get(prio).unwrap(); // safe to unwrap due to guarantees from analysis
        let dispatcher_priority = prio;
        let dispatcher_task_ty = utils::dispatcher_ident(*prio, core);
        let core_nbr = LitInt::new(&core.to_string(), Span::call_site());
        let tasks = tasks.iter().map(|(ident, _span_by)| ident);

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
            #[task( binds = #dispatcher_irq_name , priority = #dispatcher_priority, core = #core_nbr )]
            pub struct #dispatcher_task_ty;

            impl RticTask for #dispatcher_task_ty {
                fn init() -> Self {
                    // here you can generate initialization for task queues or any MaybeUnit thing related to software tasks
                    Self
                }

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

pub const SC_PEND_FN_NAME: &str = "__rtic_local_irq_pend"; // function name for core-local pending
pub const MC_PEND_FN_NAME: &str = "__rtic_cross_irq_pend"; // function name for cross-core pending

impl SoftwareTask {
    /// generate the spawn() function for the task
    fn generate_spawn_api(
        &self,
        dispatcher_irq_name: &Path,
        peripheral_crate: &Path,
        backend: &dyn SwPassBackend,
        num_cores: usize,
        queue_path: &Path,
    ) -> TokenStream {
        let task_name = self.name();
        let task_inputs_queue = utils::sw_task_inputs_ident(task_name);
        let task_trait_name = format_ident!("{}", SWT_TRAIT_TY);
        // get the inputs type. see the RticSwTask trait to understand this and where it comes from.
        let inputs_ty = quote!(<#task_name as #task_trait_name>::SpawnInput);
        let prio_ty = utils::priority_ty_ident(self.params.priority, self.params.core);
        let ready_queue_name = utils::priority_queue_ident(&prio_ty);

        let critical_section_fn = format_ident!("{}", rtic_core::rtic_functions::INTERRUPT_FREE_FN);
        let interrupt_ty = backend
            .custom_interrupt_path(self.params.core)
            .unwrap_or(parse_quote!(#peripheral_crate::Interrupt));

        // spawn for core-local tasks
        if self.params.core == self.params.spawn_by {
            let pend_fn = local_pend_fn_ident(self.params.core, num_cores);
            quote! {
                static mut #task_inputs_queue: #queue_path<#inputs_ty, 2> = #queue_path::new();

                impl #task_name {
                    pub fn spawn(input : #inputs_ty) -> Result<(), #inputs_ty> {
                        let mut inputs_producer = unsafe {#task_inputs_queue.split().0};
                        let mut ready_producer = unsafe {#ready_queue_name.split().0};
                        /// need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                        #critical_section_fn(|| -> Result<(), #inputs_ty>  {
                            // enqueue inputs
                            inputs_producer.enqueue(input)?;
                            // enqueue task to ready queue
                            unsafe {ready_producer.enqueue_unchecked(#prio_ty::#task_name)};
                            // pend dispatcher
                            #pend_fn(#interrupt_ty::#dispatcher_irq_name);
                            Ok(())
                        })
                    }
                }
            }
        }
        // spawn for cross-core tasks
        else {
            let spawner_ty = utils::core_type(self.params.spawn_by);
            let pend_fn = cross_pend_fn_ident(self.params.core);
            quote! {
                static mut #task_inputs_queue: #queue_path<#inputs_ty, 2> = #queue_path::new();

                impl #task_name {
                    pub fn spawn_from(_spawner: #spawner_ty , input : #inputs_ty) -> Result<(), #inputs_ty> {
                        let mut inputs_producer = unsafe {#task_inputs_queue.split().0};
                        let mut ready_producer = unsafe {#ready_queue_name.split().0};
                        /// need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                        #critical_section_fn(|| -> Result<(), #inputs_ty>  {
                            // enqueue inputs
                            inputs_producer.enqueue(input)?;
                            // enqueue task to ready queue
                            unsafe {ready_producer.enqueue_unchecked(#prio_ty::#task_name)};
                            // pend dispatcher
                            #pend_fn(#interrupt_ty::#dispatcher_irq_name);
                            Ok(())
                        })
                    }
                }
            }
        }
    }
}
