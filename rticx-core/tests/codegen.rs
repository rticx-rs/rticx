use proc_macro2::TokenStream;
use quote::quote;
use rticx_core::analysis::Analysis;
use rticx_core::codegen::CodeGen;
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::parser::App;

mod common;

// ---------------------------------------------------------------------------
// Expansion tests: run the full `CodeGen::run()` for a whole app and verify
// that the generated TokenStream contains the expected sections. Instead of
// checking raw substrings, each expected section is itself built with
// `quote!{...}` and its `.to_string()` is searched inside the generated
// `.to_string()`.
// ---------------------------------------------------------------------------

/// Asserts that the `expected` tokenstream (rendered to a string) is present
/// as a contiguous substring of the `generated` string. A `label` is used to
/// make failures easier to diagnose.
fn assert_section_present(generated: &str, expected: TokenStream, label: &str) {
    let expected = expected.to_string();
    assert!(
        generated.contains(&expected),
        "missing expected section `{label}` in the generated output\n\
         expected:\n{expected}\n\n\
         generated:\n{generated}"
    );
}

#[test]
fn codegen_writes_generated_task_as_unit_literal() {
    let args = common::single_core_app_args();
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits {})
            }

            // framework-generated task: not in TaskInits, written as a literal
            #[task(binds = UART, priority = 2, init = generated)]
            struct GenTask;

            impl RticTask for GenTask {
                fn exec(&mut self) {}
            }
        }
    };
    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");
    let tokens = CodeGen::new(&MockCoreBackend, &app, &analysis).run();
    let generated = tokens.to_string();

    // empty TaskInits struct (no user tasks on this core)
    assert_section_present(
        &generated,
        quote! { pub struct TaskInits { } },
        "empty TaskInits struct",
    );

    // the generated task is written as a unit literal inside the entry
    assert_section_present(
        &generated,
        quote! {
            unsafe { GEN_TASK . write (GenTask) ; }
        },
        "generated task literal write",
    );
}

#[test]
fn codegen_expands_single_core_app() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");
    let tokens = CodeGen::new(&MockCoreBackend, &app, &analysis).run();
    let generated = tokens.to_string();

    // ---- module shell ----
    assert_section_present(&generated, quote! { pub mod app }, "app module declaration");
    assert_section_present(
        &generated,
        quote! { use mypac as _ ; },
        "PAC import statement",
    );

    // ---- rticx traits module ----
    assert_section_present(
        &generated,
        quote! { pub mod rticx_traits },
        "rticx_traits module",
    );
    assert_section_present(&generated, quote! { pub trait RticTask }, "RticTask trait");
    assert_section_present(
        &generated,
        quote! { pub trait RticIdleTask },
        "RticIdleTask trait",
    );
    assert_section_present(
        &generated,
        quote! { pub trait RticMutex },
        "RticMutex trait",
    );
    assert_section_present(
        &generated,
        quote! { pub use rticx_traits :: * ; },
        "rticx_traits re-export",
    );

    // ---- interrupt-free critical section function ----
    assert_section_present(
        &generated,
        quote! { pub fn __rticx_interrupt_free < F , R > (f : F) -> R where F : FnOnce () -> R },
        "interrupt-free function signature",
    );

    // ---- shared resources definition for core 0 ----
    assert_section_present(
        &generated,
        quote! {
            static mut SHARED : core :: mem :: MaybeUninit < Shared > = core :: mem :: MaybeUninit :: uninit () ;
            struct Shared { pub counter : u32 , }
        },
        "shared resources definition",
    );

    // ---- task definition for `UartTask` ----
    assert_section_present(
        &generated,
        quote! {
            static mut UART_TASK : core :: mem :: MaybeUninit < UartTask > = core :: mem :: MaybeUninit :: uninit () ;
            struct UartTask ;
        },
        "task static + struct",
    );
    assert_section_present(
        &generated,
        quote! { const _ : fn () = || { __rticx_trait_checks :: implements_rtic_task :: < UartTask > () ; } ; },
        "task trait check",
    );
    assert_section_present(
        &generated,
        quote! { impl UartTask { pub const fn priority () -> u16 { 2u16 } } },
        "task priority function",
    );
    assert_section_present(
        &generated,
        quote! { impl UartTask { pub const fn current_core () -> __rticx__internal__Core0 { unsafe { __rticx__internal__Core0 :: new () } } } },
        "task current_core function",
    );

    // ---- shared-resources access struct for `UartTask` ----
    assert_section_present(
        &generated,
        quote! {
            impl UartTask {
                pub fn shared (& self) -> __uart_task_shared_resources {
                    __uart_task_shared_resources :: new ()
                }
            }
        },
        "task shared() accessor",
    );
    assert_section_present(
        &generated,
        quote! {
            pub struct __uart_task_shared_resources {
                pub counter : __counter_mutex < 2u16 > ,
            }
        },
        "task shared resources struct",
    );

    // ---- resource proxy for `counter` ----
    assert_section_present(
        &generated,
        quote! { pub struct __counter_mutex < const TASK_PRIORITY : u16 > ; },
        "resource proxy struct",
    );
    assert_section_present(
        &generated,
        quote! {
            impl < const TASK_PRIORITY : u16 > RticMutex for __counter_mutex < TASK_PRIORITY > {
                type ResourceType = u32 ;
                fn lock < R > (& mut self , f : impl FnOnce (& mut Self :: ResourceType) -> R) -> R {
                    f (unsafe { & mut * resource_ptr })
                }
            }
        },
        "mutex impl for proxy",
    );
    assert_section_present(
        &generated,
        quote! {
            impl < const TASK_PRIORITY : u16 > __counter_mutex < TASK_PRIORITY > {
                # [inline (always)]
                pub fn new () -> Self { Self }
            }
        },
        "resource proxy constructor",
    );

    // ---- hardware-task to interrupt binding ----
    assert_section_present(
        &generated,
        quote! {
            # [allow (non_snake_case)]
            # [unsafe (no_mangle)]
            fn UART () {
                unsafe { UART_TASK . assume_init_mut () . exec () } ;
            }
        },
        "hw task to irq binding",
    );

    // ---- core type ----
    assert_section_present(
        &generated,
        quote! {
            pub use core0_type_mod :: __rticx__internal__Core0 ;
            mod core0_type_mod {
                struct __rticx__internal__Core0Inner ;
                pub struct __rticx__internal__Core0 (__rticx__internal__Core0Inner) ;
                impl __rticx__internal__Core0 {
                    pub const unsafe fn new () -> Self {
                        __rticx__internal__Core0 (__rticx__internal__Core0Inner)
                    }
                }
            }
        },
        "core type definition",
    );

    // ---- TaskInits struct ----
    assert_section_present(
        &generated,
        quote! {
            pub struct TaskInits {
                pub uart_task : UartTask ,
                pub idle : Idle ,
            }
        },
        "TaskInits struct",
    );

    // ---- entry point ----
    assert_section_present(
        &generated,
        quote! {
            # [unsafe (no_mangle)]
            fn main () -> ! {
                __rticx_interrupt_free (|| {
                    let (__shared_resources , __task_inits) : (Shared , TaskInits) = init () ;
                    unsafe { SHARED . write (__shared_resources) ; }
                    unsafe { UART_TASK . write (__task_inits . uart_task) ; IDLE . write (__task_inits . idle) ; }
                }) ;
                unsafe {
                    IDLE . assume_init_mut () . exec () ;
                }
            }
        },
        "single-core entry point",
    );

    // ---- task trait check functions module ----
    assert_section_present(
        &generated,
        quote! { pub fn implements_rtic_task < T : RticTask > () {} },
        "implements_rtic_task check fn",
    );
    assert_section_present(
        &generated,
        quote! { pub fn implements_rtic_idle_task < T : RticIdleTask > () {} },
        "implements_rtic_idle_task check fn",
    );
    assert_section_present(
        &generated,
        quote! { const _ : fn () = || { __rticx_trait_checks :: implements_rtic_idle_task :: < Idle > () ; } ; },
        "idle trait check",
    );
    assert_section_present(
        &generated,
        quote! { static mut IDLE : core :: mem :: MaybeUninit < Idle > = core :: mem :: MaybeUninit :: uninit () ; struct Idle ; },
        "idle task definition",
    );
}

#[test]
fn codegen_expands_multi_core_app() {
    let args = common::multi_core_app_args();
    let module = common::multi_core_app_module();
    let mut app = App::parse(args, module).expect("valid multi-core app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");
    let tokens = CodeGen::new(&MockCoreBackend, &app, &analysis).run();
    let generated = tokens.to_string();

    // ---- module shell ----
    assert_section_present(&generated, quote! { pub mod app }, "app module declaration");
    // Two cores share a single PAC path: `use mypac as _ ;` is emitted once at the top.
    assert_section_present(
        &generated,
        quote! { use mypac as _ ; },
        "PAC import statement",
    );

    // ---- rticx traits module ----
    assert_section_present(
        &generated,
        quote! { pub mod rticx_traits },
        "rticx_traits module",
    );
    assert_section_present(&generated, quote! { pub trait RticTask }, "RticTask trait");
    assert_section_present(
        &generated,
        quote! { pub trait RticIdleTask },
        "RticIdleTask trait",
    );
    assert_section_present(
        &generated,
        quote! { pub trait RticMutex },
        "RticMutex trait",
    );

    // ---- core 0 sections ----
    assert_section_present(
        &generated,
        quote! {
            static mut SHARED0 : core :: mem :: MaybeUninit < Shared0 > = core :: mem :: MaybeUninit :: uninit () ;
            struct Shared0 { pub counter : u32 , }
        },
        "core0 shared resources definition",
    );
    assert_section_present(
        &generated,
        quote! {
            static mut UART_TASK0 : core :: mem :: MaybeUninit < UartTask0 > = core :: mem :: MaybeUninit :: uninit () ;
            struct UartTask0 ;
        },
        "core0 task static + struct",
    );
    assert_section_present(
        &generated,
        quote! { const _ : fn () = || { __rticx_trait_checks :: implements_rtic_task :: < UartTask0 > () ; } ; },
        "core0 task trait check",
    );
    assert_section_present(
        &generated,
        quote! { impl UartTask0 { pub const fn priority () -> u16 { 2u16 } } },
        "core0 task priority function",
    );
    assert_section_present(
        &generated,
        quote! { impl UartTask0 { pub const fn current_core () -> __rticx__internal__Core0 { unsafe { __rticx__internal__Core0 :: new () } } } },
        "core0 task current_core function",
    );
    assert_section_present(
        &generated,
        quote! {
            # [allow (non_snake_case)]
            # [unsafe (no_mangle)]
            fn UART0 () {
                unsafe { UART_TASK0 . assume_init_mut () . exec () } ;
            }
        },
        "core0 hw task to irq binding",
    );
    assert_section_present(
        &generated,
        quote! { pub struct __counter_mutex < const TASK_PRIORITY : u16 > ; },
        "core0 resource proxy struct",
    );
    assert_section_present(
        &generated,
        quote! { pub use core0_type_mod :: __rticx__internal__Core0 ; },
        "core0 type re-export",
    );
    assert_section_present(
        &generated,
        quote! {
            pub struct TaskInitsCore0 {
                pub uart_task0 : UartTask0 ,
                pub idle0 : Idle0 ,
            }
        },
        "core0 TaskInits struct",
    );
    assert_section_present(
        &generated,
        quote! {
            # [unsafe (no_mangle)]
            fn main () -> ! {
                __rticx_interrupt_free (|| {
                    let (__shared_resources , __task_inits) : (Shared0 , TaskInitsCore0) = init0 () ;
                    unsafe { SHARED0 . write (__shared_resources) ; }
                    unsafe { UART_TASK0 . write (__task_inits . uart_task0) ; IDLE0 . write (__task_inits . idle0) ; }
                }) ;
                unsafe {
                    IDLE0 . assume_init_mut () . exec () ;
                }
            }
        },
        "core0 entry point",
    );

    // ---- core 1 sections ----
    assert_section_present(
        &generated,
        quote! {
            static mut SHARED1 : core :: mem :: MaybeUninit < Shared1 > = core :: mem :: MaybeUninit :: uninit () ;
            struct Shared1 { pub counter : u32 , }
        },
        "core1 shared resources definition",
    );
    assert_section_present(
        &generated,
        quote! {
            static mut UART_TASK1 : core :: mem :: MaybeUninit < UartTask1 > = core :: mem :: MaybeUninit :: uninit () ;
            struct UartTask1 ;
        },
        "core1 task static + struct",
    );
    assert_section_present(
        &generated,
        quote! { const _ : fn () = || { __rticx_trait_checks :: implements_rtic_task :: < UartTask1 > () ; } ; },
        "core1 task trait check",
    );
    assert_section_present(
        &generated,
        quote! { impl UartTask1 { pub const fn priority () -> u16 { 3u16 } } },
        "core1 task priority function",
    );
    assert_section_present(
        &generated,
        quote! { impl UartTask1 { pub const fn current_core () -> __rticx__internal__Core1 { unsafe { __rticx__internal__Core1 :: new () } } } },
        "core1 task current_core function",
    );
    assert_section_present(
        &generated,
        quote! {
            # [allow (non_snake_case)]
            # [unsafe (no_mangle)]
            fn UART1 () {
                unsafe { UART_TASK1 . assume_init_mut () . exec () } ;
            }
        },
        "core1 hw task to irq binding",
    );
    assert_section_present(
        &generated,
        quote! { pub use core1_type_mod :: __rticx__internal__Core1 ; },
        "core1 type re-export",
    );
    // The second entry point uses the `main_1` suffix produced by `MockCoreBackend::entry_name`.
    assert_section_present(
        &generated,
        quote! {
            pub struct TaskInitsCore1 {
                pub uart_task1 : UartTask1 ,
                pub idle1 : Idle1 ,
            }
        },
        "core1 TaskInits struct",
    );
    assert_section_present(
        &generated,
        quote! {
            # [unsafe (no_mangle)]
            fn main_1 () -> ! {
                __rticx_interrupt_free (|| {
                    let (__shared_resources , __task_inits) : (Shared1 , TaskInitsCore1) = init1 () ;
                    unsafe { SHARED1 . write (__shared_resources) ; }
                    unsafe { UART_TASK1 . write (__task_inits . uart_task1) ; IDLE1 . write (__task_inits . idle1) ; }
                }) ;
                unsafe {
                    IDLE1 . assume_init_mut () . exec () ;
                }
            }
        },
        "core1 entry point",
    );

    // Both cores' hardware tasks share the RticMutex trait check function and the
    // RticTask trait, but each appears in the trait-checks module exactly once.
    assert_section_present(
        &generated,
        quote! { pub fn implements_rtic_task < T : RticTask > () {} },
        "implements_rtic_task check fn",
    );
    assert_section_present(
        &generated,
        quote! { pub fn implements_rtic_idle_task < T : RticIdleTask > () {} },
        "implements_rtic_idle_task check fn",
    );
}
