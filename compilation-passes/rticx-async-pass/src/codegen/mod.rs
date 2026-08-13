mod utils;

use crate::AsyncPassBackend;
use crate::analyze::{Analysis, SubAnalysis};
use crate::parse::ast::AsyncTask;
use crate::parse::{ASYNC_TASK_TRAIT_TY, App};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rticx_core::parse_utils::RticAttr;
use std::cell::RefCell;
use syn::{ItemMod, LitInt, Path, parse_quote};

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
                type SpawnInput;
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
                /// Flag set to true after system initialization completes
                static mut __rticx_async_system_initialized: bool = false;
            }
        }
    }

    fn push_slot_inits(&self) {
        let async_runtime_path = self.backend.async_runtime_path();
        let mut stmts = self.slot_init_stmts.borrow_mut();

        for (sub_app, _) in self
            .app
            .sub_apps
            .iter()
            .zip(self.analysis.sub_analysis.iter())
        {
            let all_tasks = sub_app.sw_tasks.iter().chain(sub_app.mc_sw_tasks.iter());

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
        let backend = self.backend;
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
            let (sw_tasks, wrapper_fns, ptr_statics, wake_fns): (Vec<_>, Vec<_>, Vec<_>, Vec<_>) =
                tasks_iter
                    .map(|task| {
                        let attr_idx = task
                            .task_struct
                            .attrs
                            .iter()
                            .position(|attr| attr.path().is_ident("async_task"))
                            .expect("An async task must have an async_task attribute");

                        let attr = task.task_struct.attrs.remove(attr_idx);

                        let mut reconstructed_task_attr = RticAttr::parse_from_attr(&attr).unwrap();
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

                        let is_prio_0 = task.params.priority == 0;

                        let spawn_impl = if is_prio_0 {
                            task.generate_spawn_api_prio_0(&queue_path)
                        } else {
                            let dispatcher_irq = sub_analysis
                                .dispatcher_priority_map
                                .get(&task.params.priority)
                                .unwrap();
                            task.generate_spawn_api(
                                dispatcher_irq,
                                pac,
                                self.backend,
                                num_cores,
                                &queue_path,
                            )
                        };

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
                num_cores,
                &interrupt_ty,
            );
            let idle_executor = generate_idle_executor(
                &sub_analysis.prio_0_tasks,
                &queue_path,
                &async_runtime_path,
                core,
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
                #idle_executor
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
    let inputs_ty = quote!(<#task_name as #task_trait>::SpawnInput);

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

    let is_prio_0 = task.params.priority == 0;

    let wake_fn = if is_prio_0 {
        quote! {
            #[doc(hidden)]
            fn #wake_fn_ident() {
                let exec = unsafe {
                    #async_runtime_path::executor::recover_slot(
                        #wrapper_fn_ident,
                        &#ptr_ident,
                    )
                };
                exec.set_pending();
            }
        }
    } else {
        let wake_pend_fn = wake_pend_fn_ident(task.params.core, num_cores);
        let dispatcher_irq = sub_analysis
            .dispatcher_priority_map
            .get(&task.params.priority)
            .unwrap();
        quote! {
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
        }
    };

    (wrapper_fn, ptr_static, wake_fn)
}

fn generate_dispatcher_tasks(
    sub_analysis: &SubAnalysis,
    queue_path: &Path,
    async_runtime_path: &Path,
    num_cores: usize,
    interrupt_ty: &Path,
) -> TokenStream {
    let core = sub_analysis.core;
    let dispatchers = &sub_analysis.dispatcher_priority_map;
    let dispatcher_tasks = sub_analysis
        .tasks_priority_map
        .iter()
        .map(|(prio, tasks)| {
            let prio_ty = utils::priority_ty_ident(*prio, core);

            // Tries to install the next buffered spawn of a task into its exec
            // slot. Returns true if a future was installed (the slot was free),
            // false if the task is still running and the spawn must be deferred.
            let install_arms = tasks.iter().map(|(task_ident, _, _)| {
                let task_static_handle = utils::ident_uppercase(task_ident);
                let task_inputs_queue = utils::sw_task_inputs_ident(task_ident);
                let wrapper_fn = utils::async_wrapper_ident(task_ident);
                let ptr_ident = utils::exec_ptr_ident(task_ident);
                quote! {
                    #prio_ty::#task_ident => {
                        let exec = unsafe {
                            #async_runtime_path::executor::recover_slot(
                                #wrapper_fn,
                                &#ptr_ident,
                            )
                        };
                        if exec.try_allocate() {
                            let mut input_consumer = unsafe { #task_inputs_queue.split().1 };
                            let input = unsafe { input_consumer.dequeue_unchecked() };
                            let future = #wrapper_fn(
                                unsafe { #task_static_handle.assume_init_mut() },
                                input,
                            );
                            unsafe { exec.spawn(future); }
                            true
                        } else {
                            false
                        }
                    }
        }
            });

            let poll_stmts = tasks.iter().map(|(task_ident, _, _)| {
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
            let overflow_queue_name = utils::overflow_queue_ident(&prio_ty);
            let install_fn = utils::install_fn_ident(&prio_ty);
            // Each pending spawn occupies one ready-queue slot, and the number
            // of pending spawns is bounded by the sum of the input-queue
            // capacities of the group. The ring buffer needs one extra slot.
            let ready_queue_size = tasks.iter().map(|(_, _, capacity)| capacity).sum::<usize>() + 1;
            let sum_capacity = ready_queue_size - 1;
            let dispatcher_irq_name = dispatchers.get(prio).unwrap();
            let wake_pend_fn = wake_pend_fn_ident(core, num_cores);
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
                #[allow(non_upper_case_globals)]
                static mut #overflow_queue_name: #queue_path<#prio_ty, #ready_queue_size> = #queue_path::new();

                #[doc(hidden)]
                fn #install_fn(task: #prio_ty) -> bool {
                    match task {
                        #(#install_arms)*
                    }
                }

                #[doc(hidden)]
                #[task( binds = #dispatcher_irq_name , priority = #dispatcher_priority, core = #core_nbr, init = generated)]
                pub struct #dispatcher_task_ty;

                impl RticTask for #dispatcher_task_ty {
                    fn exec(&mut self) {
                        unsafe {
                            let (mut ovf_producer, mut ovf_consumer) = #overflow_queue_name.split();
                            let mut ready_consumer = #ready_queue_name.split().1;

                            // Phase A: drain the ready queue. Spawns whose task
                            // is still running are deferred to the overflow queue.
                            while let Some(task) = ready_consumer.dequeue() {
                                if !#install_fn(task) {
                                    unsafe { ovf_producer.enqueue_unchecked(task) };
                                }
                            }

                            // Poll the running futures.
                            #(#poll_stmts)*

                            // Phase B: futures may have completed during polling.
                            // Install deferred spawns into the freed slots.
                            let mut deferred: [Option<#prio_ty>; #sum_capacity] =
                                [None; #sum_capacity];
                            let mut deferred_len = 0usize;
                            while let Some(task) = ovf_consumer.dequeue() {
                                deferred[deferred_len] = Some(task);
                                deferred_len += 1;
                            }
                            let mut installed = false;
                            for i in 0..deferred_len {
                                if let Some(task) = deferred[i] {
                                    if #install_fn(task) {
                                        installed = true;
                                    } else {
                                        unsafe { ovf_producer.enqueue_unchecked(task) };
                                    }
                                }
                            }
                            // Newly installed futures must be polled: run again.
                            if installed {
                                #wake_pend_fn(#interrupt_ty::#dispatcher_irq_name);
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

fn generate_idle_executor(
    prio_0_tasks: &[(Ident, u32, usize)],
    queue_path: &Path,
    async_runtime_path: &Path,
    core: u32,
) -> TokenStream {
    if prio_0_tasks.is_empty() {
        return quote! {};
    }

    let prio_ty = utils::priority_ty_ident(0, core);
    let idle_ident = utils::idle_executor_ident(core);
    let core_nbr = LitInt::new(&core.to_string(), Span::call_site());

    let install_arms = prio_0_tasks.iter().map(|(task_ident, _, _)| {
        let task_static_handle = utils::ident_uppercase(task_ident);
        let task_inputs_queue = utils::sw_task_inputs_ident(task_ident);
        let wrapper_fn = utils::async_wrapper_ident(task_ident);
        let ptr_ident = utils::exec_ptr_ident(task_ident);
        quote! {
            #prio_ty::#task_ident => {
                let exec = unsafe {
                    #async_runtime_path::executor::recover_slot(
                        #wrapper_fn,
                        &#ptr_ident,
                    )
                };
                if exec.try_allocate() {
                    let mut input_consumer = unsafe { #task_inputs_queue.split().1 };
                    let input = unsafe { input_consumer.dequeue_unchecked() };
                    let future = #wrapper_fn(
                        unsafe { #task_static_handle.assume_init_mut() },
                        input,
                    );
                    unsafe { exec.spawn(future); }
                    true
                } else {
                    false
                }
            }
        }
    });

    let poll_stmts = prio_0_tasks.iter().map(|(task_ident, _, _)| {
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
    let overflow_queue_name = utils::overflow_queue_ident(&prio_ty);
    let install_fn = utils::install_fn_ident(&prio_ty);
    let ready_queue_size = prio_0_tasks
        .iter()
        .map(|(_, _, capacity)| capacity)
        .sum::<usize>()
        + 1;
    let sum_capacity = ready_queue_size - 1;
    let tasks = prio_0_tasks.iter().map(|(ident, _, _)| ident);

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
        #[allow(non_upper_case_globals)]
        static mut #overflow_queue_name: #queue_path<#prio_ty, #ready_queue_size> = #queue_path::new();

        #[doc(hidden)]
        fn #install_fn(task: #prio_ty) -> bool {
            match task {
                #(#install_arms)*
            }
        }

        #[idle(core = #core_nbr, init = generated)]
        struct #idle_ident;

        impl RticIdleTask for #idle_ident {
            fn exec(&mut self) -> ! {
                loop {
                    unsafe {
                        let (mut ovf_producer, mut ovf_consumer) = #overflow_queue_name.split();
                        let mut ready_consumer = #ready_queue_name.split().1;

                        // Phase A: drain the ready queue. Spawns whose task is
                        // still running are deferred to the overflow queue.
                        while let Some(task) = ready_consumer.dequeue() {
                            if !#install_fn(task) {
                                unsafe { ovf_producer.enqueue_unchecked(task) };
                            }
                        }
                    }

                    // Poll the running futures.
                    #(#poll_stmts)*

                    unsafe {
                        // Phase B: futures may have completed during polling.
                        // Install deferred spawns into the freed slots.
                        let (mut ovf_producer, mut ovf_consumer) = #overflow_queue_name.split();
                        let mut deferred: [Option<#prio_ty>; #sum_capacity] =
                            [None; #sum_capacity];
                        let mut deferred_len = 0usize;
                        while let Some(task) = ovf_consumer.dequeue() {
                            deferred[deferred_len] = Some(task);
                            deferred_len += 1;
                        }
                        for i in 0..deferred_len {
                            if let Some(task) = deferred[i] {
                                if !#install_fn(task) {
                                    unsafe { ovf_producer.enqueue_unchecked(task) };
                                }
                            }
                        }
                    }
                    // The idle loop repeats forever: newly installed futures
                    // are polled on the next iteration.
                }
            }
        }
    }
}

pub const SC_PEND_FN_NAME: &str = "__rticx_async_local_irq_pend";
pub const MC_PEND_FN_NAME: &str = "__rticx_async_cross_irq_pend";
pub const WAKE_PEND_FN_NAME: &str = "__rticx_async_wake_irq_pend";

impl AsyncTask {
    fn generate_spawn_api_prio_0(&self, queue_path: &Path) -> TokenStream {
        let task_name = self.name();
        let task_inputs_queue = utils::sw_task_inputs_ident(task_name);
        let task_trait_name = format_ident!("{}", ASYNC_TASK_TRAIT_TY);
        let inputs_ty = quote!(<#task_name as #task_trait_name>::SpawnInput);
        let prio_ty = utils::priority_ty_ident(0, self.params.core);
        let ready_queue_name = utils::priority_queue_ident(&prio_ty);

        let critical_section_fn =
            format_ident!("{}", rticx_core::rticx_functions::INTERRUPT_FREE_FN);
        // ring buffer holds one slot more than the queue capacity
        let queue_buffer_size = self.params.capacity + 1;

        quote! {
            static mut #task_inputs_queue: #queue_path<#inputs_ty, #queue_buffer_size> = #queue_path::new();

            impl #task_name {
                pub fn spawn(input: #inputs_ty) -> Result<(), #inputs_ty> {
                    let mut inputs_producer = unsafe { #task_inputs_queue.split().0 };
                    let mut ready_producer = unsafe { #ready_queue_name.split().0 };
                    #critical_section_fn(|| -> Result<(), #inputs_ty> {
                        if unsafe { !__rticx_async_system_initialized } {
                            return Err(input);
                        }
                        inputs_producer.enqueue(input)?;
                        unsafe { ready_producer.enqueue_unchecked(#prio_ty::#task_name) };
                        // The priority-0 idle executor busy-polls its queues,
                        // so no interrupt needs to be pended here.
                        Ok(())
                    })
                }
            }
        }
    }

    fn generate_spawn_api(
        &self,
        dispatcher_irq_name: &Path,
        peripheral_crate: &Path,
        backend: &dyn AsyncPassBackend,
        num_cores: usize,
        queue_path: &Path,
    ) -> TokenStream {
        let task_name = self.name();
        let task_inputs_queue = utils::sw_task_inputs_ident(task_name);
        let task_trait_name = format_ident!("{}", ASYNC_TASK_TRAIT_TY);
        let inputs_ty = quote!(<#task_name as #task_trait_name>::SpawnInput);
        let prio_ty = utils::priority_ty_ident(self.params.priority, self.params.core);
        let ready_queue_name = utils::priority_queue_ident(&prio_ty);

        let critical_section_fn =
            format_ident!("{}", rticx_core::rticx_functions::INTERRUPT_FREE_FN);
        let interrupt_ty = backend
            .custom_interrupt_path(self.params.core)
            .unwrap_or(parse_quote!(#peripheral_crate::Interrupt));
        // ring buffer holds one slot more than the queue capacity
        let queue_buffer_size = self.params.capacity + 1;

        if self.params.core == self.params.spawn_by {
            let pend_fn = local_pend_fn_ident(self.params.core, num_cores);
            quote! {
                static mut #task_inputs_queue: #queue_path<#inputs_ty, #queue_buffer_size> = #queue_path::new();

                impl #task_name {
                    pub fn spawn(input: #inputs_ty) -> Result<(), #inputs_ty> {
                        let mut inputs_producer = unsafe { #task_inputs_queue.split().0 };
                        let mut ready_producer = unsafe { #ready_queue_name.split().0 };
                        #critical_section_fn(|| -> Result<(), #inputs_ty> {
                            if unsafe { !__rticx_async_system_initialized } {
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
                static mut #task_inputs_queue: #queue_path<#inputs_ty, #queue_buffer_size> = #queue_path::new();

                impl #task_name {
                    pub fn spawn_from(
                        _spawner: #spawner_ty,
                        input: #inputs_ty,
                    ) -> Result<(), #inputs_ty> {
                        let mut inputs_producer = unsafe { #task_inputs_queue.split().0 };
                        let mut ready_producer = unsafe { #ready_queue_name.split().0 };
                        #critical_section_fn(|| -> Result<(), #inputs_ty> {
                            if unsafe { !__rticx_async_system_initialized } {
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
