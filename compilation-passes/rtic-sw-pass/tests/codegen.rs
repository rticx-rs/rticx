//! Integration tests for the codegen phase of `rtic-sw-pass`.
//!
//! These run the full `SoftwarePass::run_pass` pipeline (parse + analysis +
//! codegen) against canonical single-core and multi-core app modules and
//! verify that the expanded `ItemMod` contains the expected sections. Each
//! expected section is itself built with `quote!{...}` (a balanced token tree)
//! and its `.to_string()` is searched inside the generated `.to_string()`.

use proc_macro2::TokenStream;
use quote::quote;
use rtic_core::RticPass;
use rtic_sw_pass::SoftwarePass;

mod common;

use common::{MockSwBackend, assert_section_present, mod_to_string};

/// Run the software pass end-to-end and return the generated module string.
fn run_pass(args: TokenStream, app_mod: syn::ItemMod, cross: bool) -> String {
    let pass = SoftwarePass::new(MockSwBackend { cross });
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
    );

    // ---- module shell & rest-of-code passthrough ----
    assert_section_present(&generated, quote! { mod app }, "app module declaration");
    assert_section_present(
        &generated,
        quote! { struct Bar ; },
        "rest-of-code passthrough",
    );

    // ---- RticSwTask trait ----
    assert_section_present(
        &generated,
        quote! {
            pub trait RticSwTask {
                type InitArgs : Sized ;
                type SpawnInput ;
                /// Task local variables initialization routine
                fn init (args : Self :: InitArgs) -> Self ;
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
            pub fn __rtic_local_irq_pend (irq_nbr : mypac :: Interrupt) {
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
                type InitArgs = () ;
                type SpawnInput = u32 ;
                fn init (_ : ()) -> Self { Foo }
                fn exec (& mut self , input : u32) { }
            }
        },
        "sw_task impl",
    );

    // ---- spawn() API (core-local) ----
    assert_section_present(
        &generated,
        quote! {
            static mut __rtic_internal__Foo__INPUTS : rtic :: export :: Queue < < Foo as RticSwTask > :: SpawnInput , 2 > = rtic :: export :: Queue :: new () ;
            impl Foo {
                pub fn spawn (input : < Foo as RticSwTask > :: SpawnInput) -> Result < () , < Foo as RticSwTask > :: SpawnInput > {
                    let mut inputs_producer = unsafe { __rtic_internal__Foo__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rtic_internal__Core0Prio2Tasks__RQ . split () . 0 } ;
                    /// need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                    __rtic_interrupt_free (| | -> Result < () , < Foo as RticSwTask > :: SpawnInput > {
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio2Tasks :: Foo) } ;
                        __rtic_local_irq_pend (mypac :: Interrupt :: IRQ0) ;
                        Ok (())
                    })
                }
            }
        },
        "spawn() api",
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
            static mut __rtic_internal__Core0Prio2Tasks__RQ : rtic :: export :: Queue < Core0Prio2Tasks , 2usize > = rtic :: export :: Queue :: new () ;

            #[doc (hidden)]
            #[task (binds = IRQ0 , priority = 2u16 , core = 0)]
            pub struct Core0Priority2Dispatcher ;

            impl RticTask for Core0Priority2Dispatcher {
                fn init () -> Self { Self }
                fn exec (& mut self) {
                    unsafe {
                        let mut ready_consumer = __rtic_internal__Core0Prio2Tasks__RQ . split () . 1 ;
                        while let Some (task) = ready_consumer . dequeue () {
                            match task {
                                Core0Prio2Tasks :: Foo => {
                                    let mut input_consumer = __rtic_internal__Foo__INPUTS . split () . 1 ;
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
// Multi-core expansion
// ===========================================================================

#[test]
fn codegen_expands_multi_core_sw_app() {
    let generated = run_pass(
        common::multi_core_sw_args(),
        common::multi_core_sw_app_module(),
        true,
    );

    // ---- module shell ----
    assert_section_present(&generated, quote! { mod app }, "app module declaration");

    // ---- RticSwTask trait ----
    assert_section_present(
        &generated,
        quote! {
            pub trait RticSwTask {
                type InitArgs : Sized ;
                type SpawnInput ;
                /// Task local variables initialization routine
                fn init (args : Self :: InitArgs) -> Self ;
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
            pub fn __rtic_local_irq_pend_core0 (irq_nbr : mypac :: Interrupt) {
                mock_local_pend (irq_nbr) ;
            }
        },
        "local pend fn",
    );
    assert_section_present(
        &generated,
        quote! {
            pub fn __rtic_cross_irq_pend_core1 (irq_nbr : mypac :: Interrupt) {
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
            static mut __rtic_internal__Task0__INPUTS : rtic :: export :: Queue < < Task0 as RticSwTask > :: SpawnInput , 2 > = rtic :: export :: Queue :: new () ;
            impl Task0 {
                pub fn spawn (input : < Task0 as RticSwTask > :: SpawnInput) -> Result < () , < Task0 as RticSwTask > :: SpawnInput > {
                    let mut inputs_producer = unsafe { __rtic_internal__Task0__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rtic_internal__Core0Prio2Tasks__RQ . split () . 0 } ;
                    /// need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                    __rtic_interrupt_free (| | -> Result < () , < Task0 as RticSwTask > :: SpawnInput > {
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio2Tasks :: Task0) } ;
                        __rtic_local_irq_pend_core0 (mypac :: Interrupt :: IRQ0) ;
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
            static mut __rtic_internal__Core0Prio2Tasks__RQ : rtic :: export :: Queue < Core0Prio2Tasks , 2usize > = rtic :: export :: Queue :: new () ;

            #[doc (hidden)]
            #[task (binds = IRQ0 , priority = 2u16 , core = 0)]
            pub struct Core0Priority2Dispatcher ;
        },
        "core0 dispatcher decl",
    );
    assert_section_present(
        &generated,
        quote! {
            impl RticTask for Core0Priority2Dispatcher {
                fn init () -> Self { Self }
                fn exec (& mut self) {
                    unsafe {
                        let mut ready_consumer = __rtic_internal__Core0Prio2Tasks__RQ . split () . 1 ;
                        while let Some (task) = ready_consumer . dequeue () {
                            match task {
                                Core0Prio2Tasks :: Task0 => {
                                    let mut input_consumer = __rtic_internal__Task0__INPUTS . split () . 1 ;
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
            static mut __rtic_internal__Cross__INPUTS : rtic :: export :: Queue < < Cross as RticSwTask > :: SpawnInput , 2 > = rtic :: export :: Queue :: new () ;
            impl Cross {
                pub fn spawn_from (_spawner : __rtic__internal__Core0 , input : < Cross as RticSwTask > :: SpawnInput) -> Result < () , < Cross as RticSwTask > :: SpawnInput > {
                    let mut inputs_producer = unsafe { __rtic_internal__Cross__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rtic_internal__Core1Prio3Tasks__RQ . split () . 0 } ;
                    /// need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                    __rtic_interrupt_free (| | -> Result < () , < Cross as RticSwTask > :: SpawnInput > {
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core1Prio3Tasks :: Cross) } ;
                        __rtic_cross_irq_pend_core1 (mypac :: Interrupt :: IRQ1) ;
                        Ok (())
                    })
                }
            }
        },
        "core1 spawn_from() api",
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
            static mut __rtic_internal__Core1Prio3Tasks__RQ : rtic :: export :: Queue < Core1Prio3Tasks , 2usize > = rtic :: export :: Queue :: new () ;

            #[doc (hidden)]
            #[task (binds = IRQ1 , priority = 3u16 , core = 1)]
            pub struct Core1Priority3Dispatcher ;
        },
        "core1 dispatcher decl",
    );
    assert_section_present(
        &generated,
        quote! {
            impl RticTask for Core1Priority3Dispatcher {
                fn init () -> Self { Self }
                fn exec (& mut self) {
                    unsafe {
                        let mut ready_consumer = __rtic_internal__Core1Prio3Tasks__RQ . split () . 1 ;
                        while let Some (task) = ready_consumer . dequeue () {
                            match task {
                                Core1Prio3Tasks :: Cross => {
                                    let mut input_consumer = __rtic_internal__Cross__INPUTS . split () . 1 ;
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
