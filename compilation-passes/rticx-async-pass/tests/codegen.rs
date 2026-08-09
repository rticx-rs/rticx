use proc_macro2::TokenStream;
use quote::quote;
use rticx_core::RticPass;
use rticx_async_pass::AsyncPass;

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
    assert_section_present(&generated, quote! { struct Bar ; }, "rest-of-code passthrough");

    assert_section_present(
        &generated,
        quote! {
            pub trait RticAsyncTask {
                type InitArgs : Sized ;
                type SpawnInput ;
                fn init (args : Self :: InitArgs) -> Self ;
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
            static mut __rticx_internal__Foo__EXEC : rticx_async :: executor :: ExecSlot =
                rticx_async :: executor :: ExecSlot :: new () ;
        },
        "EXEC slot static",
    );

    assert_section_present(
        &generated,
        quote! {
            fn __rticx_internal__Foo__wake () {
                let exec = unsafe { & * core :: ptr :: addr_of ! (__rticx_internal__Foo__EXEC) } ;
                exec . set_pending () ;
                __rticx_async_wake_irq_pend (mypac :: Interrupt :: IRQ0) ;
            }
        },
        "wake fn",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Foo__INPUTS : rticx :: export :: Queue < < Foo as RticAsyncTask > :: SpawnInput , 2 > =
                rticx :: export :: Queue :: new () ;
            impl Foo {
                pub fn spawn (input : < Foo as RticAsyncTask > :: SpawnInput) -> Result < () , < Foo as RticAsyncTask > :: SpawnInput > {
                    let mut inputs_producer = unsafe { __rticx_internal__Foo__INPUTS . split () . 0 } ;
                    let mut ready_producer = unsafe { __rticx_internal__Core0Prio2Tasks__RQ . split () . 0 } ;
                    __rticx_interrupt_free (| | -> Result < () , < Foo as RticAsyncTask > :: SpawnInput > {
                        let exec = unsafe { & * core :: ptr :: addr_of ! (__rticx_internal__Foo__EXEC) } ;
                        if ! exec . try_allocate () { return Err (input) ; }
                        inputs_producer . enqueue (input) ? ;
                        unsafe { ready_producer . enqueue_unchecked (Core0Prio2Tasks :: Foo) } ;
                        __rticx_async_local_irq_pend (mypac :: Interrupt :: IRQ0) ;
                        Ok (())
                    })
                }
            }
        },
        "spawn() api with try_allocate",
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
            #[task (binds = IRQ0 , priority = 2u16 , core = 0)]
            pub struct Core0Priority2Dispatcher ;
        },
        "dispatcher task struct",
    );

    assert_section_present(
        &generated,
        quote! {
            let future = RticAsyncTask :: exec (FOO . assume_init_mut () , input ,) ;
            let exec = unsafe { & * core :: ptr :: addr_of ! (__rticx_internal__Foo__EXEC) } ;
            unsafe { exec . install (future) ; }
        },
        "dispatcher future install",
    );

    assert_section_present(
        &generated,
        quote! {
            let mut any_running = false ;
            {
                let exec = unsafe { & * core :: ptr :: addr_of ! (__rticx_internal__Foo__EXEC) } ;
                let still_running = exec . poll (__rticx_internal__Foo__wake) ;
                any_running = any_running || still_running ;
            }
            if any_running { __rticx_async_local_irq_pend (mypac :: Interrupt :: IRQ0) ; }
        },
        "dispatcher poll + self-repend",
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
                type InitArgs : Sized ;
                type SpawnInput ;
                fn init (args : Self :: InitArgs) -> Self ;
                fn exec (
                    & mut self ,
                    input : Self :: SpawnInput ,
                ) -> impl core :: future :: Future < Output = () > ;
            }
        },
        "RticAsyncTask trait",
    );

    assert_section_present(&generated, quote! { task_trait = RticAsyncTask }, "task_trait element");
    assert_section_present(&generated, quote! { struct Task0 ; }, "core0 async_task struct");
    assert_section_present(&generated, quote! { impl RticAsyncTask for Task0 }, "core0 impl");

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Task0__EXEC : rticx_async :: executor :: ExecSlot =
                rticx_async :: executor :: ExecSlot :: new () ;
        },
        "core0 EXEC static",
    );

    assert_section_present(
        &generated,
        quote! {
            static mut __rticx_internal__Task0__INPUTS : rticx :: export :: Queue
        },
        "core0 inputs queue",
    );

    assert_section_present(
        &generated,
        quote! { impl Task0 },
        "core0 spawn impl",
    );
    assert_section_present(
        &generated,
        quote! { pub fn spawn },
        "core0 spawn fn",
    );

    assert_section_present(
        &generated,
        quote! {
            let exec = unsafe { & * core :: ptr :: addr_of ! (__rticx_internal__Task0__EXEC) } ;
            if ! exec . try_allocate () { return Err (input) ; }
        },
        "core0 try_allocate",
    );

    assert_section_present(
        &generated,
        quote! {
            #[task (binds = IRQ0 , priority = 2u16 , core = 0)]
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
            static mut __rticx_internal__Cross__EXEC : rticx_async :: executor :: ExecSlot =
                rticx_async :: executor :: ExecSlot :: new () ;
        },
        "core1 EXEC static",
    );

    assert_section_present(
        &generated,
        quote! { impl Cross },
        "core1 spawn_from impl",
    );
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
            #[task (binds = IRQ1 , priority = 3u16 , core = 1)]
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
