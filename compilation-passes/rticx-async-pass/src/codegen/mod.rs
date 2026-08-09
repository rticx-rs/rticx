mod utils;

use crate::analyze::{Analysis, SubAnalysis};
use crate::parse::ast::AsyncTask;
use crate::parse::{App, ASYNC_TASK_TRAIT_TY};
use crate::AsyncPassBackend;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rticx_core::parse_utils::RticAttr;
use std::cell::RefCell;
use syn::{parse_quote, ItemMod, LitInt, Path};

fn local_pend_fn_ident(core: u32, num_cores: usize) -> Ident {
    if num_cores == 1 {
        format_ident!("{SC_PEND_FN_NAME}")
    } else {
        format_ident!("{SC_PEND_FN_NAME}_core{core}")
    }
}

fn cross_pend_fn_ident(core: u32) -> Ident {
    format_ident!("{MC_PEND_FN_NAME}_core{core}")
}

fn wake_pend_fn_ident(core: u32, num_cores: usize) -> Ident {
    if num_cores == 1 {
        format_ident!("{WAKE_PEND_FN_NAME}")
    } else {
        format_ident!("{WAKE_PEND_FN_NAME}_core{core}")
    }
}

pub struct CodeGen<'a> {
    app: App,
    analysis: Analysis,
    backend: &'a dyn AsyncPassBackend,
    slot_init_stmts: &'a RefCell<Vec<TokenStream>>,
}

impl<'a> CodeGen<'a> {
    pub fn new(
        app: App,
        analysis: Analysis,
        backend: &'a dyn AsyncPassBackend,
        slot_init_stmts: &'a RefCell<Vec<TokenStream>>,
    ) -> CodeGen<'a> {
        Self {
            app,
            analysis,
            backend,
            slot_init_stmts,
        }
    }

    pub fn run(&mut self) -> ItemMod {
        let sub_apps = self.generate_subapps();
        let local_pend_fns = self.get_local_pend_fns();
        let cross_pend_fns = self.get_cross_pend_fns();
        let wake_pend_fns = self.get_wake_pend_fns();
        let rest_of_code = &self.app.rest_of_code;
        let software_task_trait = format_ident!("{ASYNC_TASK_TRAIT_TY}");
        let sw_task_trait_def = quote! {
            pub trait #software_task_trait {
                type InitArgs: Sized;
                type SpawnInput;
                fn init(args: Self::InitArgs) -> Self;
                fn exec(
                    &mut self,
                    input: Self::SpawnInput,
                ) -> impl core::future::Future<Output = ()>;
            }
        };
        let mod_visibility = &self.app.mod_visibility;
        let mod_ident = &self.app.mod_ident;

        // Push slot init statements for main_injection
        self.push_slot_inits();

        parse_quote! {
            #mod_visibility mod #mod_ident {
                #(#rest_of_code)*
                #sub_apps
                #sw_task_trait_def
                #local_pend_fns
                #wake_pend_fns
                #cross_pend_fns
            }
        }
    }

    fn push_slot_inits(&self) {
        let async_runtime_path = self.backend.async_runtime_path();
        let mut stmts = self.slot_init_stmts.borrow_mut();

        for (sub_app, _) in self.app.sub_apps.iter().zip(self.analysis.sub_analysis.iter()) {
            let all_tasks = sub_app
                .sw_tasks
                .iter()
                .chain(sub_app.mc_sw_tasks.iter());

            for task in all_tasks {
                let wrapper_fn_ident = utils::async_wrapper_ident(task.name());
                let ptr_ident = utils::exec_ptr_ident(task.name());

                stmts.push(quote! {
                    {
                        let __s = core::mem::ManuallyDrop::new(
                            #async_runtime_path::executor::ExecSlot::new_from_witness(#wrapper_fn_ident)
                        );
                        #ptr_ident.store(&*__s as *const _ as *const ());
                    }
                });
            }
        }
    }

    fn get_interrupt_path(&self, core: u32) -> Path {
        let pac = &self.app.app_params.pacs[core as usize];
        self.backend
            .custom_interrupt_path(core)
            .unwrap_or_else(|| parse_quote!(#pac::Interrupt))
    }

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
                    pub fn #fn_ident(irq_nbr: #interrupt_ty) {}
                };
                let fn_def = self.backend.generate_local_pend_fn(core, empty_body_fn);
                quote!(#fn_def)
            })
            .collect();
        quote!(#(#fns)*)
    }

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
                    pub fn #fn_ident(irq_nbr: #interrupt_ty) {}
                };
                self.backend
                    .generate_cross_pend_fn(core, empty_body_fn)
                    .map(|fn_def| quote!(#fn_def))
            })
            .collect();
        quote!(#(#fns)*)
    }

    fn get_wake_pend_fns(&self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let fns: Vec<TokenStream> = self
            .app
            .sub_apps
            .iter()
            .map(|sub_app| {
                let core = sub_app.core;
                let interrupt_ty = self.get_interrupt_path(core);
                let fn_ident = wake_pend_fn_ident(core, num_cores);
                let empty_body_fn = parse_quote! {
                    #[doc(hidden)]
                    #[inline]
                    pub fn #fn_ident(irq_nbr: #interrupt_ty) {}
                };
                let fn_def = self.backend.generate_wake_pend_fn(core, empty_body_fn);
                quote!(#fn_def)
            })
            .collect();
        quote!(#(#fns)*)
    }

    fn generate_subapps(&mut self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let queue_path = self.backend.queue_path();
        let async_runtime_path = self.backend.async_runtime_path();
        let backend = &*self.backend;
        let app_params = &self.app.app_params;
        let apps = self.app.sub_apps.iter_mut();
        let analysis = self.analysis.sub_analysis.iter();

        let sub_apps = apps.zip(analysis).map(|(sub_app, sub_analysis)| {
            let core = sub_app.core;
            let pac = &app_params.pacs[core as usize];
            let interrupt_ty = backend
                .custom_interrupt_path(core)
                .unwrap_or_else(|| parse_quote!(#pac::Interrupt));
            let tasks_iter = sub_app
                .sw_tasks
                .iter_mut()
                .chain(sub_app.mc_sw_tasks.iter_mut());
            let (sw_tasks, wrapper_fns, ptr_statics, wake_fns): (
                Vec<_>,
                Vec<_>,
                Vec<_>,
                Vec<_>,
            ) = tasks_iter
                .map(|task| {
                    let attr_idx = task
                        .task_struct
                        .attrs
                        .iter()
                        .position(|attr| attr.path().is_ident("async_task"))
                        .expect("An async task must have an async_task attribute");

                    let attr = task.task_struct.attrs.remove(attr_idx);

                    let mut reconstructed_task_attr =
                        RticAttr::parse_from_attr(&attr).unwrap();
                    let _ = reconstructed_task_attr.name.insert(format_ident!("task"));
                    reconstructed_task_attr.elements.insert(
                        "task_trait".into(),
                        syn::parse_str(ASYNC_TASK_TRAIT_TY).unwrap(),
                    );

                    let task_struct = &task.task_struct;
                    let task_impl = &task.task_impl;

                    let (wrapper_fn, ptr_static, wake_fn) = generate_exec_static_and_wake(
                        task,
                        sub_analysis,
                        num_cores,
                        &async_runtime_path,
                        &interrupt_ty,
                    );
                    let dispatcher_irq = sub_analysis
                        .dispatcher_priority_map
                        .get(&task.params.priority)
                        .unwrap();
                    let spawn_impl = task.generate_spawn_api(
                        dispatcher_irq,
                        pac,
                        self.backend,
                        num_cores,
                        &queue_path,
                        &async_runtime_path,
                    );

                    (
                        quote! {
                            #reconstructed_task_attr
                            #task_struct
                            #task_impl
                            #spawn_impl
                        },
                        wrapper_fn,
                        ptr_static,
                        wake_fn,
                    )
                })
                .fold(
                    (vec![], vec![], vec![], vec![]),
                    |(mut tasks, mut wrappers, mut ptrs, mut wakes),
                     (task, wrapper, ptr, wake)| {
                        tasks.push(task);
                        wrappers.push(wrapper);
                        ptrs.push(ptr);
                        wakes.push(wake);
                        (tasks, wrappers, ptrs, wakes)
                    },
                );

            let dispatcher_tasks = generate_dispatcher_tasks(
                sub_analysis,
                &queue_path,
                &async_runtime_path,
            );
            let core_doc = format!(" Core {}", sub_app.core);
            quote! {
                #[doc = " Async tasks of"]
                #[doc = #core_doc]
                #(#sw_tasks)*
                #(#wrapper_fns)*
                #(#ptr_statics)*
                #(#wake_fns)*

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

fn generate_exec_static_and_wake(
    task: &AsyncTask,
    sub_analysis: &SubAnalysis,
    num_cores: usize,
    async_runtime_path: &Path,
    interrupt_ty: &Path,
) -> (TokenStream, TokenStream, TokenStream) {
    let task_name = task.name();
    let task_trait = format_ident!("{ASYNC_TASK_TRAIT_TY}");
    let wrapper_fn_ident = utils::async_wrapper_ident(task_name);
    let ptr_ident = utils::exec_ptr_ident(task_name);
    let wake_fn_ident = utils::exec_wake_ident(task_name);
    let wake_pend_fn = wake_pend_fn_ident(task.params.core, num_cores);
    let inputs_ty = quote!(<#task_name as #task_trait>::SpawnInput);
    let dispatcher_irq = sub_analysis
        .dispatcher_priority_map
        .get(&task.params.priority)
        .unwrap();

    let wrapper_fn = quote! {
        #[doc(hidden)]
        async fn #wrapper_fn_ident(task: &mut #task_name, input: #inputs_ty) {
            <#task_name as #task_trait>::exec(task, input).await;
        }
    };

    let ptr_static = quote! {
        #[doc(hidden)]
        static #ptr_ident: #async_runtime_path::executor::ExecSlotPtr =
            #async_runtime_path::executor::ExecSlotPtr::new();
    };

    let wake_fn = quote! {
        #[doc(hidden)]
        fn #wake_fn_ident() {
            let exec = unsafe {
                #async_runtime_path::executor::recover_slot(
                    #wrapper_fn_ident,
                    &#ptr_ident,
                )
            };
            exec.set_pending();
            #wake_pend_fn(#interrupt_ty::#dispatcher_irq);
        }
    };

    (wrapper_fn, ptr_static, wake_fn)
}

fn generate_dispatcher_tasks(
    sub_analysis: &SubAnalysis,
    queue_path: &Path,
    async_runtime_path: &Path,
) -> TokenStream {
    let core = sub_analysis.core;
    let dispatchers = &sub_analysis.dispatcher_priority_map;
    let dispatcher_tasks = sub_analysis
        .tasks_priority_map
        .iter()
        .map(|(prio, tasks)| {
            let prio_ty = utils::priority_ty_ident(*prio, core);

            let install_branches = tasks.iter().map(|(task_ident, _)| {
                let task_static_handle = utils::ident_uppercase(task_ident);
                let task_inputs_queue = utils::sw_task_inputs_ident(task_ident);
                let wrapper_fn = utils::async_wrapper_ident(task_ident);
                let ptr_ident = utils::exec_ptr_ident(task_ident);
                quote! {
                    #prio_ty::#task_ident => {
                        let mut input_consumer = #task_inputs_queue.split().1;
                        let input = input_consumer.dequeue_unchecked();
                        let future = #wrapper_fn(
                            #task_static_handle.assume_init_mut(),
                            input,
                        );
                        let exec = unsafe {
                            #async_runtime_path::executor::recover_slot(
                                #wrapper_fn,
                                &#ptr_ident,
                            )
                        };
                        unsafe { exec.spawn(future); }
                    }
                }
            });

            let poll_stmts = tasks.iter().map(|(task_ident, _)| {
                let wrapper_fn = utils::async_wrapper_ident(task_ident);
                let ptr_ident = utils::exec_ptr_ident(task_ident);
                let wake_fn = utils::exec_wake_ident(task_ident);
                quote! {
                    {
                        let exec = unsafe {
                            #async_runtime_path::executor::recover_slot(
                                #wrapper_fn,
                                &#ptr_ident,
                            )
                        };
                        exec.poll(#wake_fn);
                    }
                }
            });

            let ready_queue_name = utils::priority_queue_ident(&prio_ty);
            let ready_queue_size = tasks.len() + 1;
            let dispatcher_irq_name = dispatchers.get(prio).unwrap();
            let dispatcher_priority = prio;
            let dispatcher_task_ty = utils::dispatcher_ident(*prio, core);
            let core_nbr = LitInt::new(&core.to_string(), Span::call_site());
            let tasks = tasks.iter().map(|(ident, _)| ident);

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
                        Self
                    }

                fn exec(&mut self) {
                    unsafe {
                        let mut ready_consumer = #ready_queue_name.split().1;
                        while let Some(task) = ready_consumer.dequeue() {
                            match task {
                                #(#install_branches)*
                            }
                        }
                    }

                    #(#poll_stmts)*
                }
                }
            }
        });

    quote! {
        #(#dispatcher_tasks)*
    }
}

pub const SC_PEND_FN_NAME: &str = "__rticx_async_local_irq_pend";
pub const MC_PEND_FN_NAME: &str = "__rticx_async_cross_irq_pend";
pub const WAKE_PEND_FN_NAME: &str = "__rticx_async_wake_irq_pend";

impl AsyncTask {
    fn generate_spawn_api(
        &self,
        dispatcher_irq_name: &Path,
        peripheral_crate: &Path,
        backend: &dyn AsyncPassBackend,
        num_cores: usize,
        queue_path: &Path,
        async_runtime_path: &Path,
    ) -> TokenStream {
        let task_name = self.name();
        let task_inputs_queue = utils::sw_task_inputs_ident(task_name);
        let ptr_ident = utils::exec_ptr_ident(task_name);
        let wrapper_fn = utils::async_wrapper_ident(task_name);
        let task_trait_name = format_ident!("{}", ASYNC_TASK_TRAIT_TY);
        let inputs_ty = quote!(<#task_name as #task_trait_name>::SpawnInput);
        let prio_ty = utils::priority_ty_ident(self.params.priority, self.params.core);
        let ready_queue_name = utils::priority_queue_ident(&prio_ty);

        let critical_section_fn =
            format_ident!("{}", rticx_core::rticx_functions::INTERRUPT_FREE_FN);
        let interrupt_ty = backend
            .custom_interrupt_path(self.params.core)
            .unwrap_or(parse_quote!(#peripheral_crate::Interrupt));

        if self.params.core == self.params.spawn_by {
            let pend_fn = local_pend_fn_ident(self.params.core, num_cores);
            quote! {
                static mut #task_inputs_queue: #queue_path<#inputs_ty, 2> = #queue_path::new();

                impl #task_name {
                    pub fn spawn(input: #inputs_ty) -> Result<(), #inputs_ty> {
                        let mut inputs_producer = unsafe { #task_inputs_queue.split().0 };
                        let mut ready_producer = unsafe { #ready_queue_name.split().0 };
                        #critical_section_fn(|| -> Result<(), #inputs_ty> {
                            let exec = unsafe {
                                #async_runtime_path::executor::recover_slot(
                                    #wrapper_fn,
                                    &#ptr_ident,
                                )
                            };
                            if !exec.try_allocate() {
                                return Err(input);
                            }
                            inputs_producer.enqueue(input)?;
                            unsafe { ready_producer.enqueue_unchecked(#prio_ty::#task_name) };
                            #pend_fn(#interrupt_ty::#dispatcher_irq_name);
                            Ok(())
                        })
                    }
                }
            }
        } else {
            let spawner_ty = utils::core_type(self.params.spawn_by);
            let pend_fn = cross_pend_fn_ident(self.params.core);
            quote! {
                static mut #task_inputs_queue: #queue_path<#inputs_ty, 2> = #queue_path::new();

                impl #task_name {
                    pub fn spawn_from(
                        _spawner: #spawner_ty,
                        input: #inputs_ty,
                    ) -> Result<(), #inputs_ty> {
                        let mut inputs_producer = unsafe { #task_inputs_queue.split().0 };
                        let mut ready_producer = unsafe { #ready_queue_name.split().0 };
                        #critical_section_fn(|| -> Result<(), #inputs_ty> {
                            let exec = unsafe {
                                #async_runtime_path::executor::recover_slot(
                                    #wrapper_fn,
                                    &#ptr_ident,
                                )
                            };
                            if !exec.try_allocate() {
                                return Err(input);
                            }
                            inputs_producer.enqueue(input)?;
                            unsafe { ready_producer.enqueue_unchecked(#prio_ty::#task_name) };
                            #pend_fn(#interrupt_ty::#dispatcher_irq_name);
                            Ok(())
                        })
                    }
                }
            }
        }
    }
}
