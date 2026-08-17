//! Integration tests for the codegen phase of `rticx-sw-pass`.
//!
//! These run the full `SoftwarePass::run_pass` pipeline (parse + analysis +
//! codegen) against canonical single-core and multi-core app modules and
//! verify that the expanded `ItemMod` contains the expected sections. Each
//! expected section is itself built with `quote!{...}` (a balanced token tree)
//! and its `.to_string()` is searched inside the generated `.to_string()`.

use proc_macro2::TokenStream;
use quote::quote;
use rticx_core::RticPass;
use rticx_sw_pass::SoftwarePass;

mod common;

use common::{MockSwBackend, assert_section_present, mod_to_string};

/// Run the software pass end-to-end and return the generated module string.
fn run_pass(args: TokenStream, app_mod: syn::ItemMod, cross: bool, core_check: bool) -> String {
    let pass = SoftwarePass::new(MockSwBackend { cross, core_check });
    let (_, module) = pass.run_pass(args, app_mod).expect("pass succeeds");
    mod_to_string(&module)
}

// ===========================================================================
// Single-core expansion
// ===========================================================================

#[test]
fn codegen_expands_single_core_sw_app() {
    let generated = run_pass(
        common::single_core_sw_args(),
        common::single_core_sw_app_module(),
        false,
        false,
    );

    // ---- module shell & rest-of-code passthrough ----
    assert_section_present(&generated, quote! { mod app }, "app module declaration");
    assert_section_present(
        &generated,
        quote! { struct Bar ; },
        "rest-of-code passthrough",
    );
    // The original `#[sw_task]` attribute must be consumed by the pass.
    assert!(
        !generated.contains("sw_task"),
        "the original `#[sw_task]` attribute leaked into the generated code:\n{generated}"
    );

    // ---- RticSwTask trait ----
    assert_section_present(
        &generated,
        quote! {
            pub trait RticSwTask {
                type SpawnInput ;
                /// Function to be executing when the scheduled software task is dispatched
                fn exec (& mut self , input : Self :: SpawnInput) ;
            }
        },
        "RticSwTask trait",
    );

    // ---- core-local interrupt pending function ----
    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_local_irq_pend (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "local pend fn",
    );

    // ---- sw_task reconstructed attribute fragment (non-deterministic order) ----
    assert_section_present(
        &generated,
        quote! { task_trait = RticSwTask },
        "reconstructed task_trait element",
    );

    // ---- sw_task struct + impl ----
    assert_section_present(&generated, quote! { struct Foo ; }, "sw_task struct");
    assert_section_present(
        &generated,
        quote! {
            impl RticSwTask for Foo {
                type SpawnInput = u32 ;
                fn exec (& mut self , input : u32) { }
            }
        },
        "sw_task impl",
    );

    // ---- spawn() API (core-local) ----
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Foo__INPUTS : rticx :: export :: Queue < < Foo as RticSwTask > :: SpawnInput , 2usize > = rticx :: export :: Queue :: new () ;
            impl Foo {
                pub fn spawn (input : < Foo as RticSwTask > :: SpawnInput) -> Result < () , < Foo as RticSwTask > :: SpawnInput > {
                    let mut inputs_producer = unsafe { __rticx_internal__Foo__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rticx_internal__Core0Prio2Tasks__RQ . split () . 0 } ;
                    __rticx_interrupt_free (| | -> Result < () , < Foo as RticSwTask > :: SpawnInput > {
                        if unsafe { ! __rticx_sw_system_initialized } { return Err (input) ; }
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio2Tasks :: Foo) } ;
                        __rticx_local_irq_pend (mypac :: Interrupt :: IRQ0) ;
                        Ok (())
                    })
                }
            }
        },
        "spawn() api",
    );

    // ---- runtime core check is not injected when the backend returns None ----
    assert!(
        !generated.contains("mock_current_core_id"),
        "no runtime core check expected when the backend provides none:\n{generated}"
    );

    // ---- dispatcher: priority enum, ready queue, hw task, exec match ----
    assert_section_present(
        &generated,
        quote! {
            #[derive (Clone , Copy)]
            #[doc (hidden)]
            pub enum Core0Prio2Tasks { Foo , }

            #[doc (hidden)]
            #[allow (non_upper_case_globals)]
            static mut __rticx_internal__Core0Prio2Tasks__RQ : rticx :: export :: Queue < Core0Prio2Tasks , 2usize > = rticx :: export :: Queue :: new () ;

            #[doc (hidden)]
            #[task (binds = IRQ0 , priority = 2u16 , core = 0 , init = generated)]
            pub struct Core0Priority2Dispatcher ;

            impl RticTask for Core0Priority2Dispatcher {
                fn exec (& mut self) {
                    unsafe {
                        let mut ready_consumer = __rticx_internal__Core0Prio2Tasks__RQ . split () . 1 ;
                        while let Some (task) = ready_consumer . dequeue () {
                            match task {
                                Core0Prio2Tasks :: Foo => {
                                    let mut input_consumer = __rticx_internal__Foo__INPUTS . split () . 1 ;
                                    let input = input_consumer . dequeue_unchecked () ;
                                    FOO . assume_init_mut () . exec (input) ;
                                }
                            }
                        }
                    }
                }
            }
        },
        "dispatcher block",
    );
}

// ===========================================================================
// Queue capacity expansion
// ===========================================================================

/// App module with two tasks at the same priority: `Big` has `capacity = 3`
/// while `Small` uses the default capacity of 1.
fn capacity_app_module() -> syn::ItemMod {
    common::app_mod(quote! {
        #[sw_task(priority = 2, capacity = 3)]
        struct Big;

        impl RticSwTask for Big {
                        type SpawnInput = u32;
            fn exec(&mut self, input: u32) {}
        }

        #[sw_task(priority = 2)]
        struct Small;

        impl RticSwTask for Small {
                        type SpawnInput = u32;
            fn exec(&mut self, input: u32) {}
        }
    })
}

#[test]
fn codegen_sizes_input_and_ready_queues_from_capacity() {
    let generated = run_pass(
        common::single_core_sw_args(),
        capacity_app_module(),
        false,
        false,
    );

    // Input queue of `Big`: ring buffer of capacity + 1 = 4 slots.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Big__INPUTS : rticx :: export :: Queue < < Big as RticSwTask > :: SpawnInput , 4usize > = rticx :: export :: Queue :: new () ;
        },
        "capacity-3 input queue",
    );

    // Input queue of `Small`: default capacity 1 -> 2 slots.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Small__INPUTS : rticx :: export :: Queue < < Small as RticSwTask > :: SpawnInput , 2usize > = rticx :: export :: Queue :: new () ;
        },
        "default-capacity input queue",
    );

    // Ready queue of the priority group: sum of capacities (3 + 1) + 1 = 5.
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Core0Prio2Tasks__RQ : rticx :: export :: Queue < Core0Prio2Tasks , 5usize > = rticx :: export :: Queue :: new () ;
        },
        "ready queue sized by capacity sum",
    );
}

// ===========================================================================
// Multi-core expansion
// ===========================================================================

#[test]
fn codegen_expands_multi_core_sw_app() {
    let generated = run_pass(
        common::multi_core_sw_args(),
        common::multi_core_sw_app_module(),
        true,
        true,
    );

    // ---- module shell ----
    assert_section_present(&generated, quote! { mod app }, "app module declaration");

    // ---- RticSwTask trait ----
    assert_section_present(
        &generated,
        quote! {
            pub trait RticSwTask {
                type SpawnInput ;
                /// Function to be executing when the scheduled software task is dispatched
                fn exec (& mut self , input : Self :: SpawnInput) ;
            }
        },
        "RticSwTask trait",
    );

    // ---- core-local & cross-core pend functions ----
    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_local_irq_pend_core0 (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "local pend fn",
    );
    assert_section_present(
        &generated,
        quote! {
            pub fn __rticx_cross_irq_pend_core1 (irq_nbr : mypac :: Interrupt) -> Result<(),()>{
                mock_cross_pend (irq_nbr) ;
            }
        },
        "cross pend fn",
    );

    // ---- core 0: local task Task0 ----
    assert_section_present(
        &generated,
        quote! { task_trait = RticSwTask },
        "core0 reconstructed task_trait element",
    );
    assert_section_present(
        &generated,
        quote! { struct Task0 ; },
        "core0 sw_task struct",
    );
    assert_section_present(
        &generated,
        quote! { impl RticSwTask for Task0 },
        "core0 sw_task impl",
    );
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Task0__INPUTS : rticx :: export :: Queue < < Task0 as RticSwTask > :: SpawnInput , 2usize > = rticx :: export :: Queue :: new () ;
            impl Task0 {
                pub fn spawn (input : < Task0 as RticSwTask > :: SpawnInput) -> Result < () , < Task0 as RticSwTask > :: SpawnInput > {
                    if mock_current_core_id () != 0 { return Err (input) ; }
                    let mut inputs_producer = unsafe { __rticx_internal__Task0__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rticx_internal__Core0Prio2Tasks__RQ . split () . 0 } ;
                    __rticx_interrupt_free (| | -> Result < () , < Task0 as RticSwTask > :: SpawnInput > {
                        if unsafe { ! __rticx_sw_system_initialized } { return Err (input) ; }
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio2Tasks :: Task0) } ;
                        __rticx_local_irq_pend_core0 (mypac :: Interrupt :: IRQ0) ;
                        Ok (())
                    })
                }
            }
        },
        "core0 spawn() api",
    );
    // core 0 dispatcher
    assert_section_present(
        &generated,
        quote! {
            #[derive (Clone , Copy)]
            #[doc (hidden)]
            pub enum Core0Prio2Tasks { Task0 , }

            #[doc (hidden)]
            #[allow (non_upper_case_globals)]
            static mut __rticx_internal__Core0Prio2Tasks__RQ : rticx :: export :: Queue < Core0Prio2Tasks , 2usize > = rticx :: export :: Queue :: new () ;

            #[doc (hidden)]
            #[task (binds = IRQ0 , priority = 2u16 , core = 0 , init = generated)]
            pub struct Core0Priority2Dispatcher ;
        },
        "core0 dispatcher decl",
    );
    assert_section_present(
        &generated,
        quote! {
            impl RticTask for Core0Priority2Dispatcher {
                fn exec (& mut self) {
                    unsafe {
                        let mut ready_consumer = __rticx_internal__Core0Prio2Tasks__RQ . split () . 1 ;
                        while let Some (task) = ready_consumer . dequeue () {
                            match task {
                                Core0Prio2Tasks :: Task0 => {
                                    let mut input_consumer = __rticx_internal__Task0__INPUTS . split () . 1 ;
                                    let input = input_consumer . dequeue_unchecked () ;
                                    TASK0 . assume_init_mut () . exec (input) ;
                                }
                            }
                        }
                    }
                }
            }
        },
        "core0 dispatcher exec",
    );

    // ---- core 1: cross-core task Cross (spawned by core 0) ----
    assert_section_present(
        &generated,
        quote! { struct Cross ; },
        "core1 sw_task struct",
    );
    assert_section_present(
        &generated,
        quote! { impl RticSwTask for Cross },
        "core1 sw_task impl",
    );
    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Cross__INPUTS : rticx :: export :: Queue < < Cross as RticSwTask > :: SpawnInput , 2usize > = rticx :: export :: Queue :: new () ;
            impl Cross {
                /// Cross-core spawn: enqueue `input` to this task, which executes on
                /// `core`, from the core specified using `spawn_by`.
                /// ## Returns:
                /// - Ok(()), the inputs are enqueued successfully and the task's dispatcher interrupt is successfully pended
                /// - Err(None), the inputs are enqueued the inputs are enqueued successfully but and the task's dispatcher interrupt pendeding failed.
                /// Either repend it manually or try at a later time.
                /// - Err(Some(input)), the inputs failed to be enqueued. Consider increasing the channel capacity using `capacity = N`.
                /// `Err(Some(input))` is also returned when the caller is not executing on the `spawn_by` core. (in Multicore)
                pub fn cross_spawn (input : < Cross as RticSwTask > :: SpawnInput) -> Result < () , Option< < Cross as RticSwTask > :: SpawnInput > > {
                    if mock_current_core_id () != 0 { return Err (Some (input)) ; }
                    let mut inputs_producer = unsafe { __rticx_internal__Cross__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rticx_internal__Core1Prio3Tasks__RQ . split () . 0 } ;
                    __rticx_interrupt_free (| | -> Result < () ,Option< < Cross as RticSwTask > :: SpawnInput> > {
                        if unsafe { ! __rticx_sw_system_initialized } { return Err (Some(input)) ; }
                        inputs_producer . enqueue (input).map_err(Option::Some) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core1Prio3Tasks :: Cross) } ;
                        __rticx_cross_irq_pend_core1 (mypac :: Interrupt :: IRQ1).map_err(|_| None)
                    })
                }
            }
        },
        "core1 cross_spawn() api",
    );
    // core 1 dispatcher
    assert_section_present(
        &generated,
        quote! {
            #[derive (Clone , Copy)]
            #[doc (hidden)]
            pub enum Core1Prio3Tasks { Cross , }

            #[doc (hidden)]
            #[allow (non_upper_case_globals)]
            static mut __rticx_internal__Core1Prio3Tasks__RQ : rticx :: export :: Queue < Core1Prio3Tasks , 2usize > = rticx :: export :: Queue :: new () ;

            #[doc (hidden)]
            #[task (binds = IRQ1 , priority = 3u16 , core = 1 , init = generated)]
            pub struct Core1Priority3Dispatcher ;
        },
        "core1 dispatcher decl",
    );
    assert_section_present(
        &generated,
        quote! {
            impl RticTask for Core1Priority3Dispatcher {
                fn exec (& mut self) {
                    unsafe {
                        let mut ready_consumer = __rticx_internal__Core1Prio3Tasks__RQ . split () . 1 ;
                        while let Some (task) = ready_consumer . dequeue () {
                            match task {
                                Core1Prio3Tasks :: Cross => {
                                    let mut input_consumer = __rticx_internal__Cross__INPUTS . split () . 1 ;
                                    let input = input_consumer . dequeue_unchecked () ;
                                    CROSS . assume_init_mut () . exec (input) ;
                                }
                            }
                        }
                    }
                }
            }
        },
        "core1 dispatcher exec",
    );
}

#[test]
fn codegen_cross_core_tasks_require_current_core_id() {
    let pass = SoftwarePass::new(MockSwBackend {
        cross: true,
        core_check: false,
    });
    let result = pass.run_pass(
        common::multi_core_sw_args(),
        common::multi_core_sw_app_module(),
    );
    let Err(err) = result else {
        panic!("expected an error for cross-core tasks without a runtime core check");
    };
    assert!(
        err.to_string().contains("current_core_id"),
        "expected an error mentioning `current_core_id`, got: {err}"
    );
}
