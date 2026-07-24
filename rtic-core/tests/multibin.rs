#![cfg(feature = "multibin")]

use proc_macro2::TokenStream;
use quote::quote;
use rtic_core::RticMacroBuilder;
use rtic_core::mock_backend::MockCoreBackend;
use rtic_core::parser::App;

mod common;

#[test]
fn multibin_generates_core_cfg_guards() {
    let args: TokenStream = quote!(device = mypac, cores = 2);
    let input: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared(core = 0)]
            struct Shared0 {
                pub counter: u32,
            }

            #[shared(core = 1)]
            struct Shared1 {
                pub counter: u32,
            }

            #[init(core = 0)]
            fn init0() -> Shared0 {
                Shared0 { counter: 0 }
            }

            #[init(core = 1)]
            fn init1() -> Shared1 {
                Shared1 { counter: 0 }
            }

            #[task(binds = UART0, priority = 2, shared = [counter], core = 0)]
            struct UartTask0;

            impl RticTask for UartTask0 {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask0 }
                fn exec(&mut self) {}
            }

            #[task(binds = UART1, priority = 2, shared = [counter], core = 1)]
            struct UartTask1;

            impl RticTask for UartTask1 {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask1 }
                fn exec(&mut self) {}
            }
        }
    };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let expanded = builder.build_rtic_macro2(args, input);
    let expanded = expanded.to_string();

    assert!(expanded.contains("cfg (core = \"0\")"));
    assert!(expanded.contains("cfg (core = \"1\")"));
    assert!(expanded.contains("use crate :: mock_shared as multibin_shared"));
}

#[test]
fn multibin_task_impl_emptied_for_other_cores() {
    let args: TokenStream = quote!(device = mypac, cores = 2);
    let input: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared(core = 0)]
            struct Shared0 {
                pub counter: u32,
            }

            #[shared(core = 1)]
            struct Shared1 {
                pub counter: u32,
            }

            #[init(core = 0)]
            fn init0() -> Shared0 {
                Shared0 { counter: 0 }
            }

            #[init(core = 1)]
            fn init1() -> Shared1 {
                Shared1 { counter: 0 }
            }

            #[task(binds = UART0, priority = 2, shared = [counter], core = 0)]
            struct UartTask0;

            impl RticTask for UartTask0 {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask0 }
                fn exec(&mut self) {}
            }
        }
    };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let expanded = builder.build_rtic_macro2(args, input);
    let expanded = expanded.to_string();

    // The task impl for UartTask0 should appear on core 0 and an emptied version
    // with unreachable bodies should appear under the not(core = "0") guard.
    assert!(expanded.contains("cfg (core = \"0\")"));
    assert!(expanded.contains("cfg (not (core = \"0\"))"));
    assert!(expanded.contains("unreachable !"));
}

#[test]
fn multibin_app_args_parse_two_cores() {
    use rtic_core::parser::ast::AppArgs;
    let args: TokenStream = quote!(device = mypac, cores = 2);
    let parsed = AppArgs::parse(args).expect("valid args");
    assert_eq!(parsed.cores, 2);
    assert_eq!(parsed.pacs.len(), 2);
}

#[test]
fn multibin_analysis_partitions_per_core() {
    let args: TokenStream = quote!(device = mypac, cores = 2);
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared(core = 0)]
            struct Shared0 {
                pub counter: u32,
            }

            #[shared(core = 1)]
            struct Shared1 {
                pub counter: u32,
            }

            #[init(core = 0)]
            fn init0() -> Shared0 {
                Shared0 { counter: 0 }
            }

            #[init(core = 1)]
            fn init1() -> Shared1 {
                Shared1 { counter: 0 }
            }

            #[task(binds = UART0, priority = 2, shared = [counter], core = 0)]
            struct UartTask0;

            impl RticTask for UartTask0 {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask0 }
                fn exec(&mut self) {}
            }

            #[task(binds = UART1, priority = 4, shared = [counter], core = 1)]
            struct UartTask1;

            impl RticTask for UartTask1 {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask1 }
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let analysis = rtic_core::Analysis::run(&mut app).expect("analysis succeeds");
    assert_eq!(analysis.sub_analysis.len(), 2);
    assert_eq!(analysis.sub_analysis[0].used_irqs.len(), 1);
    assert_eq!(analysis.sub_analysis[1].used_irqs.len(), 1);
    assert_eq!(analysis.sub_analysis[0].used_irqs[0].0.to_string(), "UART0");
    assert_eq!(analysis.sub_analysis[1].used_irqs[0].0.to_string(), "UART1");
}
