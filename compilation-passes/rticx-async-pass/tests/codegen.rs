use proc_macro2::TokenStream;
use quote::quote;
use rticx_async_pass::AsyncPass;
use rticx_core::RticPass;

mod common;

use common::{MockAsyncBackend, assert_section_present, mod_to_string};

fn run_pass(args: TokenStream, app_mod: syn::ItemMod, cross: bool) -> String {
    let pass = AsyncPass::new(MockAsyncBackend { cross });
    let (_, module) = pass.run_pass(args, app_mod).expect("pass succeeds");
    mod_to_string(&module)
}

#[test]
fn codegen_expands_single_core_sw_app() {
    let generated = run_pass(
        common::single_core_sw_args(),
        common::single_core_sw_app_module(),
        false,
    );

    assert_section_present(&generated, quote! { mod app }, "app module declaration");
    assert_section_present(
        &generated,
        quote! { struct Bar ; },
        "rest-of-code passthrough",
    );
    // The original `#[async_task]` attribute must be consumed by the pass.
    assert!(
        !generated.contains("async_task"),
        "the original `#[async_task]` attribute leaked into the generated code:\n{generated}"
    );

    assert_section_present(
        &generated,
        quote! {
            pub trait RticAsyncTask {
                type SpawnInput ;
                fn exec (
                    & mut self ,
                    input : Self :: SpawnInput ,
                ) -> impl core :: future :: Future < Output = () > ;
            }
        },
        "RticAsyncTask trait",
    );

    assert_section_present(
        &generated,
        quote! {
            async fn __rticx_async_Foo (task : & mut Foo , input : < Foo as RticAsyncTask > :: SpawnInput)
        },
        "wrapper async fn",
    );

    assert_section_present(
        &generated,
        quote! {
            static __rticx_internal__Foo__PTR : rticx_async :: executor :: ExecSlotPtr =
                rticx_async :: executor :: ExecSlotPtr :: new () ;
        },
        "ExecSlotPtr static",
    );

    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_async_local_irq_pend (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "local pend fn",
    );

    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_async_wake_irq_pend (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "wake pend fn",
    );

    assert_section_present(
        &generated,
        quote! { task_trait = RticAsyncTask },
        "reconstructed task_trait element",
    );

    assert_section_present(&generated, quote! { struct Foo ; }, "async_task struct");

    assert_section_present(
        &generated,
        quote! { impl RticAsyncTask for Foo },
        "async_task impl",
    );

    assert_section_present(
        &generated,
        quote! {
            fn __rticx_internal__Foo__wake () {
                let exec = unsafe {
                    rticx_async :: executor :: recover_slot (
                        __rticx_async_Foo ,
                        & __rticx_internal__Foo__PTR ,
                    )
                } ;
                exec . set_pending () ;
                __rticx_async_wake_irq_pend (mypac :: Interrupt :: IRQ0) ;
            }
        },
        "wake fn with recover_slot",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Foo__INPUTS : rticx :: export :: Queue < < Foo as RticAsyncTask > :: SpawnInput , 2usize > =
                rticx :: export :: Queue :: new () ;
            impl Foo {
                pub fn spawn (input : < Foo as RticAsyncTask > :: SpawnInput) -> Result < () , < Foo as RticAsyncTask > :: SpawnInput > {
                    #[allow(static_mut_refs)]
                    let mut inputs_producer = unsafe { __rticx_internal__Foo__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rticx_internal__Core0Prio2Tasks__RQ . split () . 0 } ;
                    __rticx_interrupt_free (| | -> Result < () , < Foo as RticAsyncTask > :: SpawnInput > {
                        if unsafe { ! __rticx_async_system_initialized } { return Err (input) ; }
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio2Tasks :: Foo) } ;
                        __rticx_async_local_irq_pend (mypac :: Interrupt :: IRQ0) ;
                        Ok (())
                    })
                }
            }
        },
        "spawn() api (queue-buffered, no try_allocate)",
    );

    assert_section_present(
        &generated,
        quote! {
            #[derive (Clone , Copy)]
            #[doc (hidden)]
            pub enum Core0Prio2Tasks { Foo , }
        },
        "priority task enum",
    );

    assert_section_present(
        &generated,
        quote! {
            #[task (binds = IRQ0 , priority = 2u16 , core = 0 , init = generated)]
            pub struct Core0Priority2Dispatcher ;
        },
        "dispatcher task struct",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Core0Prio2Tasks__OQ : rticx :: export :: Queue < Core0Prio2Tasks , 2usize > = rticx :: export :: Queue :: new () ;
        },
        "overflow queue",
    );

    assert_section_present(
        &generated,
        quote! {
            fn __rticx_internal__Core0Prio2Tasks__try_install (task : Core0Prio2Tasks) -> bool {
                match task {
                    Core0Prio2Tasks :: Foo => {
                        let exec = unsafe {
                            rticx_async :: executor :: recover_slot (
                                __rticx_async_Foo ,
                                & __rticx_internal__Foo__PTR ,
                            )
                        } ;
                        if exec . try_allocate () {
                            #[allow(static_mut_refs)]
                            let mut input_consumer = unsafe { __rticx_internal__Foo__INPUTS . split () . 1 } ;
                            let input = unsafe { input_consumer . dequeue_unchecked () } ;
                            let future = __rticx_async_Foo (unsafe { FOO . assume_init_mut () } , input ,) ;
                            unsafe { exec . spawn (future) ; }
                            true
                        } else {
                            false
                        }
                    }
                }
            }
        },
        "install fn (try_allocate at dispatch time)",
    );

    assert_section_present(
        &generated,
        quote! {
            while let Some (task) = ready_consumer . dequeue () {
                if ! __rticx_internal__Core0Prio2Tasks__try_install (task) {
                    unsafe { ovf_producer . enqueue_unchecked (task) } ;
                }
            }
        },
        "dispatcher Phase A (defer busy tasks)",
    );

    assert_section_present(
        &generated,
        quote! {
            let mut deferred : [Option < Core0Prio2Tasks > ; 1usize] = [None ; 1usize] ;
        },
        "dispatcher Phase B deferred buffer",
    );

    assert_section_present(
        &generated,
        quote! {
            if installed {
                __rticx_async_wake_irq_pend (mypac :: Interrupt :: IRQ0) ;
            }
        },
        "dispatcher self-pend after deferred install",
    );

    assert_section_present(
        &generated,
        quote! {
            {
                let exec = unsafe {
                    rticx_async :: executor :: recover_slot (
                        __rticx_async_Foo ,
                        & __rticx_internal__Foo__PTR ,
                    )
                } ;
                exec . poll (__rticx_internal__Foo__wake) ;
            }
        },
        "dispatcher poll via recover_slot",
    );
}

#[test]
fn codegen_sizes_queues_from_capacity() {
    let generated = run_pass(
        common::single_core_sw_args(),
        common::app_mod(quote! {
                    #[async_task(priority = 2, capacity = 3)]
                    struct Big;

                    impl RticAsyncTask for Big {
        type SpawnInput = u32;
                        fn exec(&mut self, input: u32) {}
                    }

                    #[async_task(priority = 2)]
                    struct Small;

                    impl RticAsyncTask for Small {
        type SpawnInput = u32;
                        fn exec(&mut self, input: u32) {}
                    }
                }),
        false,
    );

    // Input queue of `Big`: ring buffer of capacity + 1 = 4 slots.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Big__INPUTS : rticx :: export :: Queue < < Big as RticAsyncTask > :: SpawnInput , 4usize > =
                rticx :: export :: Queue :: new () ;
        },
        "capacity-3 input queue",
    );

    // Input queue of `Small`: default capacity 1 -> 2 slots.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Small__INPUTS : rticx :: export :: Queue < < Small as RticAsyncTask > :: SpawnInput , 2usize > =
                rticx :: export :: Queue :: new () ;
        },
        "default-capacity input queue",
    );

    // Ready queue: sum of capacities (3 + 1) + 1 = 5 slots.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Core0Prio2Tasks__RQ : rticx :: export :: Queue < Core0Prio2Tasks , 5usize > = rticx :: export :: Queue :: new () ;
        },
        "ready queue sized by capacity sum",
    );

    // Overflow queue has the same size as the ready queue.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Core0Prio2Tasks__OQ : rticx :: export :: Queue < Core0Prio2Tasks , 5usize > = rticx :: export :: Queue :: new () ;
        },
        "overflow queue sized by capacity sum",
    );

    // The deferred buffer of the dispatcher holds all pending spawns.
    assert_section_present(
        &generated,
        quote! {
            let mut deferred : [Option < Core0Prio2Tasks > ; 4usize] = [None ; 4usize] ;
        },
        "deferred buffer sized by capacity sum",
    );
}

#[test]
fn codegen_expands_multi_core_sw_app() {
    let generated = run_pass(
        common::multi_core_sw_args(),
        common::multi_core_sw_app_module(),
        true,
    );

    assert_section_present(&generated, quote! { mod app }, "app module declaration");

    assert_section_present(
        &generated,
        quote! {
            pub trait RticAsyncTask {
                type SpawnInput ;
                fn exec (
                    & mut self ,
                    input : Self :: SpawnInput ,
                ) -> impl core :: future :: Future < Output = () > ;
            }
        },
        "RticAsyncTask trait",
    );

    assert_section_present(
        &generated,
        quote! { task_trait = RticAsyncTask },
        "task_trait element",
    );
    assert_section_present(
        &generated,
        quote! { struct Task0 ; },
        "core0 async_task struct",
    );
    assert_section_present(
        &generated,
        quote! { impl RticAsyncTask for Task0 },
        "core0 impl",
    );

    assert_section_present(
        &generated,
        quote! {
            static __rticx_internal__Task0__PTR : rticx_async :: executor :: ExecSlotPtr =
                rticx_async :: executor :: ExecSlotPtr :: new () ;
        },
        "core0 ExecSlotPtr static",
    );

    assert_section_present(
        &generated,
        quote! {
            async fn __rticx_async_Task0 (task : & mut Task0 , input : < Task0 as RticAsyncTask > :: SpawnInput)
        },
        "core0 wrapper async fn",
    );

    assert_section_present(
        &generated,
        quote! {
            executor :: recover_slot (
                __rticx_async_Task0 ,
                & __rticx_internal__Task0__PTR ,
            )
        },
        "core0 recover_slot",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Task0__INPUTS : rticx :: export :: Queue
        },
        "core0 inputs queue",
    );

    assert_section_present(&generated, quote! { impl Task0 }, "core0 spawn impl");
    assert_section_present(&generated, quote! { pub fn spawn }, "core0 spawn fn");

    assert_section_present(
        &generated,
        quote! {
            recover_slot (
                __rticx_async_Task0 ,
                & __rticx_internal__Task0__PTR ,
            )
        },
        "core0 recover in install fn",
    );

    assert_section_present(
        &generated,
        quote! {
            if exec . try_allocate ()
        },
        "core0 try_allocate at dispatch time",
    );

    assert_section_present(
        &generated,
        quote! {
            #[task (binds = IRQ0 , priority = 2u16 , core = 0 , init = generated)]
            pub struct Core0Priority2Dispatcher ;
        },
        "core0 dispatcher",
    );

    assert_section_present(
        &generated,
        quote! { struct Cross ; },
        "core1 async_task struct",
    );

    assert_section_present(
        &generated,
        quote! {
            static __rticx_internal__Cross__PTR : rticx_async :: executor :: ExecSlotPtr =
        },
        "core1 ExecSlotPtr static",
    );

    assert_section_present(
        &generated,
        quote! {
            async fn __rticx_async_Cross (task : & mut Cross , input : < Cross as RticAsyncTask > :: SpawnInput)
        },
        "core1 wrapper async fn",
    );

    assert_section_present(&generated, quote! { impl Cross }, "core1 spawn_from impl");
    assert_section_present(
        &generated,
        quote! { pub fn spawn_from },
        "core1 spawn_from fn",
    );

    assert_section_present(
        &generated,
        quote! { __rticx_async_cross_irq_pend_core1 (mypac :: Interrupt :: IRQ1) },
        "core1 cross pend call",
    );

    assert_section_present(
        &generated,
        quote! {
            #[task (binds = IRQ1 , priority = 3u16 , core = 1 , init = generated)]
            pub struct Core1Priority3Dispatcher ;
        },
        "core1 dispatcher",
    );

    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_async_wake_irq_pend_core0 (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "wake pend core0",
    );

    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_async_wake_irq_pend_core1 (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "wake pend core1",
    );

    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_async_cross_irq_pend_core1 (irq_nbr : mypac :: Interrupt) {
                mock_cross_pend (irq_nbr) ;
            }
        },
        "cross pend core1",
    );
}

#[test]
fn codegen_expands_prio_0_executor() {
    let generated = run_pass(
        common::single_core_sw_args(),
        common::single_core_prio_0_app_module(),
        false,
    );

    assert_section_present(&generated, quote! { mod app }, "app module declaration");

    assert_section_present(
        &generated,
        quote! {
            pub trait RticAsyncTask {
                type SpawnInput ;
                fn exec (
                    & mut self ,
                    input : Self :: SpawnInput ,
                ) -> impl core :: future :: Future < Output = () > ;
            }
        },
        "RticAsyncTask trait",
    );

    assert_section_present(
        &generated,
        quote! { task_trait = RticAsyncTask },
        "task_trait element",
    );

    assert_section_present(&generated, quote! { struct Foo ; }, "async_task struct");

    assert_section_present(
        &generated,
        quote! { impl RticAsyncTask for Foo },
        "async_task impl",
    );

    assert_section_present(
        &generated,
        quote! {
            async fn __rticx_async_Foo (task : & mut Foo , input : < Foo as RticAsyncTask > :: SpawnInput)
        },
        "wrapper async fn",
    );

    assert_section_present(
        &generated,
        quote! {
            static __rticx_internal__Foo__PTR : rticx_async :: executor :: ExecSlotPtr =
                rticx_async :: executor :: ExecSlotPtr :: new () ;
        },
        "ExecSlotPtr static",
    );

    assert_section_present(
        &generated,
        quote! {
            fn __rticx_internal__Foo__wake () {
                let exec = unsafe {
                    rticx_async :: executor :: recover_slot (
                        __rticx_async_Foo ,
                        & __rticx_internal__Foo__PTR ,
                    )
                } ;
                exec . set_pending () ;
            }
        },
        "prio-0 wake fn (sets pending, no IRQ pend)",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Foo__INPUTS : rticx :: export :: Queue < < Foo as RticAsyncTask > :: SpawnInput , 2usize > =
                rticx :: export :: Queue :: new () ;
            impl Foo {
                pub fn spawn (input : < Foo as RticAsyncTask > :: SpawnInput) -> Result < () , < Foo as RticAsyncTask > :: SpawnInput > {
                    #[allow(static_mut_refs)]
                    let mut inputs_producer = unsafe { __rticx_internal__Foo__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rticx_internal__Core0Prio0Tasks__RQ . split () . 0 } ;
                    __rticx_interrupt_free (| | -> Result < () , < Foo as RticAsyncTask > :: SpawnInput > {
                        if unsafe { ! __rticx_async_system_initialized } { return Err (input) ; }
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio0Tasks :: Foo) } ;
                        Ok (())
                    })
                }
            }
        },
        "prio-0 spawn (queue-buffered, no pend)",
    );

    assert_section_present(
        &generated,
        quote! {
            pub enum Core0Prio0Tasks { Foo , }
        },
        "prio-0 task enum",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Core0Prio0Tasks__OQ : rticx :: export :: Queue < Core0Prio0Tasks , 2usize > = rticx :: export :: Queue :: new () ;
        },
        "prio-0 overflow queue",
    );

    assert_section_present(
        &generated,
        quote! {
            fn __rticx_internal__Core0Prio0Tasks__try_install (task : Core0Prio0Tasks) -> bool
        },
        "prio-0 install fn",
    );

    assert_section_present(
        &generated,
        quote! {
            let mut ready_consumer = __rticx_internal__Core0Prio0Tasks__RQ . split () . 1 ;
            while let Some (task) = ready_consumer . dequeue () {
                if ! __rticx_internal__Core0Prio0Tasks__try_install (task) {
                    unsafe { ovf_producer . enqueue_unchecked (task) } ;
                }
            }
        },
        "idle executor Phase A",
    );

    assert_section_present(
        &generated,
        quote! {
            let mut deferred : [Option < Core0Prio0Tasks > ; 1usize] = [None ; 1usize] ;
        },
        "idle executor Phase B deferred buffer",
    );

    assert_section_present(
        &generated,
        quote! {
            {
                let exec = unsafe {
                    rticx_async :: executor :: recover_slot (
                        __rticx_async_Foo ,
                        & __rticx_internal__Foo__PTR ,
                    )
                } ;
                exec . poll (__rticx_internal__Foo__wake) ;
            }
        },
        "idle executor poll",
    );

    assert_section_present(
        &generated,
        quote! { #[idle (core = 0 , init = generated)] },
        "idle executor struct attribute",
    );
}
