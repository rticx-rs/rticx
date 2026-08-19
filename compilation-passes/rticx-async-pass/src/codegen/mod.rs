mod utils;

use crate::AsyncPassBackend;
use crate::analyze::{Analysis, SubAnalysis};
use crate::parse::ast::AsyncTask;
use crate::parse::{ASYNC_TASK_TRAIT_TY, App};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rticx_sw_pass::common::codegen::{
    SpawnApiParams, cross_pend_fn_ident, generate_cross_pend_fns, generate_local_pend_fns,
    generate_spawn_api, get_interrupt_path, local_pend_fn_ident,
};
use std::cell::RefCell;
use std::collections::HashMap;
use syn::{ItemMod, LitInt, Path, parse_quote};

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
    slot_init_stmts: &'a RefCell<HashMap<u32, Vec<TokenStream>>>,
}

impl<'a> CodeGen<'a> {
    pub fn new(
        app: App,
        analysis: Analysis,
        backend: &'a dyn AsyncPassBackend,
        slot_init_stmts: &'a RefCell<HashMap<u32, Vec<TokenStream>>>,
    ) -> CodeGen<'a> {
        Self {
            app,
            analysis,
            backend,
            slot_init_stmts,
        }
    }

    pub fn run(&self) -> ItemMod {
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

        let async_prio_limit = self.generate_async_prio_limit();

        parse_quote! {
            #mod_visibility mod #mod_ident {
                #(#rest_of_code)*
                #sub_apps
                #sw_task_trait_def
                #local_pend_fns
                #wake_pend_fns
                #cross_pend_fns
                #async_prio_limit
                /// Flag set to true after system initialization completes
                static mut __rticx_async_system_initialized: bool = false;
            }
        }
    }

    /// Emits the `RTIC_ASYNC_MAX_LOGICAL_PRIO` symbol consumed by
    /// `rtic-monotonics` (and other async HAL drivers) to set their timer
    /// interrupt priority so it strictly preempts async tasks.
    ///
    /// The value is `max(async task priority) + 1`, or `u8::MAX` when there
    /// are no async tasks. `rtic-monotonics` clamps it to the device's max
    /// logical priority (`1 << NVIC_PRIO_BITS`) itself as it is target aware
    fn generate_async_prio_limit(&self) -> TokenStream {
        let max_async_prio = self
            .analysis
            .sub_analysis
            .iter()
            .flat_map(|sa| sa.tasks_priority_map.keys())
            .copied()
            .max();

        // `+1` so the timer strictly preempts async tasks; saturating so it
        // can't wrap. `u8::MAX` is the "no limit" sentinel, clamped by
        // `rtic-monotonics`.
        let timer_prio = max_async_prio
            .map(|p| p.saturating_add(1))
            .unwrap_or(u16::MAX)
            .min(u8::MAX as u16) as u8;

        quote! {
            #[doc(hidden)]
            #[unsafe(no_mangle)]
            // FIXME: rtic-monotonics is not multicore-aware, so this single
            // symbol is shared by all cores (max async priority over all
            // cores). Once it gains per-core support, emit one symbol per
            // core instead.
            static RTIC_ASYNC_MAX_LOGICAL_PRIO: u8 = #timer_prio;
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

                // Each core's entry function allocates the slots of its own
                // tasks (on its own stack); the injection for a core is looked
                // up by `sub_app.core` in `main_injection`.
                stmts.entry(sub_app.core).or_default().push(quote! {
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
        get_interrupt_path(self.backend, &self.app.app_params.pacs, core)
    }

    fn get_local_pend_fns(&self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let cores = self
            .app
            .sub_apps
            .iter()
            .map(|sub_app| (sub_app.core, self.get_interrupt_path(sub_app.core)));
        generate_local_pend_fns(self.backend, cores, num_cores)
    }

    fn get_cross_pend_fns(&self) -> TokenStream {
        let cores = self
            .app
            .sub_apps
            .iter()
            .filter(|sub_app| !sub_app.mc_sw_tasks.is_empty())
            .map(|sub_app| (sub_app.core, self.get_interrupt_path(sub_app.core)));
        generate_cross_pend_fns(self.backend, cores)
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

    fn generate_subapps(&self) -> TokenStream {
        let num_cores = self.app.sub_apps.len();
        let queue_path = self.backend.queue_path();
        let async_runtime_path = self.backend.async_runtime_path();
        let backend = self.backend;
        let app_params = &self.app.app_params;
        let apps = self.app.sub_apps.iter();
        let analysis = self.analysis.sub_analysis.iter();

        let sub_apps = apps.zip(analysis).map(|(sub_app, sub_analysis)| {
            let core = sub_app.core;
            let pac = &app_params.pacs[core as usize];
            let interrupt_ty = backend
                .custom_interrupt_path(core)
                .unwrap_or_else(|| parse_quote!(#pac::Interrupt));
            let tasks_iter = sub_app.sw_tasks.iter().chain(sub_app.mc_sw_tasks.iter());
            let (sw_tasks, wrapper_fns, ptr_statics, wake_fns): (Vec<_>, Vec<_>, Vec<_>, Vec<_>) =
                tasks_iter
                    .map(|task| {
                        // The task attribute was already reconstructed by the
                        // parsing phase: `async_task` renamed to `task`,
                        // pass-only keys removed, and the
                        // `task_trait = RticAsyncTask` argument added.
                        let task_attr = &task.task_attr;
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
                            task.generate_spawn_api_prio_0(&queue_path, self.backend)
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
                                #task_attr
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
            quote! {
                #(#sw_tasks)*
                #(#wrapper_fns)*
                #(#ptr_statics)*
                #(#wake_fns)*

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
                            #[allow(static_mut_refs)]
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
                    #[allow(static_mut_refs)]
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

pub const WAKE_PEND_FN_NAME: &str = "__rticx_wake_irq_pend";

impl AsyncTask {
    fn generate_spawn_api_prio_0(
        &self,
        queue_path: &Path,
        backend: &dyn AsyncPassBackend,
    ) -> TokenStream {
        let task_name = self.name();
        let task_trait_name = format_ident!("{}", ASYNC_TASK_TRAIT_TY);
        let inputs_ty = quote!(<#task_name as #task_trait_name>::SpawnInput);
        let prio_ty = utils::priority_ty_ident(0, self.params.core);
        let ready_queue_name = utils::priority_queue_ident(&prio_ty);

        // ring buffer holds one slot more than the queue capacity
        let queue_buffer_size = self.params.capacity + 1;

        // Optional runtime check that the caller runs on this task's core.
        let core_lit = LitInt::new(&self.params.core.to_string(), Span::call_site());
        let core_check = backend.current_core_id().map(|current_core_id| {
            quote! {
                if #current_core_id != #core_lit {
                    return Err(input);
                }
            }
        });

        let system_initialized_flag = format_ident!("__rticx_async_system_initialized");
        generate_spawn_api(&SpawnApiParams {
            task_name,
            inputs_ty: &inputs_ty,
            prio_ty: &prio_ty,
            ready_queue_name: &ready_queue_name,
            queue_path,
            queue_buffer_size,
            system_initialized_flag: &system_initialized_flag,
            cross: false,
            // The priority-0 idle executor busy-polls its queues, so no
            // interrupt needs to be pended here.
            pend_stmt: None,
            core_check,
        })
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
        let task_trait_name = format_ident!("{}", ASYNC_TASK_TRAIT_TY);
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
            let pend_fn = cross_pend_fn_ident(self.params.core);
            quote!(#pend_fn(#interrupt_ty::#dispatcher_irq_name).map_err(|_| None))
        } else {
            let pend_fn = local_pend_fn_ident(self.params.core, num_cores);
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

        let system_initialized_flag = format_ident!("__rticx_async_system_initialized");
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
