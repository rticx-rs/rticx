use proc_macro2::TokenStream;
use quote::quote;
use rtic_core::RticMacroBuilder;
use rtic_core::mock_backend::MockCoreBackend;

mod common;

#[test]
fn full_pipeline_single_core_expands() {
    let args: TokenStream = common::single_core_app_args();
    let input: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> Shared {
                Shared { counter: 0 }
            }

            #[task(binds = UART, priority = 2, shared = [counter])]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    UartTask
                }
                fn exec(&mut self) {
                    // task body
                }
            }

            #[idle]
            struct Idle;

            impl RticIdleTask for Idle {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    Idle
                }
                fn exec(&mut self) -> ! {
                    loop {}
                }
            }
        }
    };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let expanded = builder.build_rtic_macro2(args, input);
    let expanded = expanded.to_string();

    assert!(expanded.contains("pub mod app"));
    assert!(expanded.contains("use mypac as _"));
    assert!(expanded.contains("fn main"));
    assert!(expanded.contains("__rtic_interrupt_free"));
    assert!(expanded.contains("static mut SHARED"));
    assert!(expanded.contains("static mut UART_TASK"));
    assert!(expanded.contains("fn UART"));
    assert!(expanded.contains("pub mod rtic_traits"));
    assert!(expanded.contains("__rtic_trait_checks"));
}

#[test]
fn full_pipeline_pre_core_pass_transforms_input() {
    struct AppendInitPass;

    impl rtic_core::RticPass for AppendInitPass {
        fn run_pass(
            &self,
            args: TokenStream,
            mut app_mod: syn::ItemMod,
        ) -> syn::Result<(TokenStream, syn::ItemMod)> {
            if let Some((_, items)) = &mut app_mod.content {
                let init_fn: syn::ItemFn = syn::parse_quote! {
                    #[init]
                    fn __extra_init() -> () {}
                };
                items.push(syn::Item::Fn(init_fn));
            }
            Ok((args, app_mod))
        }

        fn pass_name(&self) -> &str {
            "AppendInitPass"
        }
    }

    let args: TokenStream = quote!(device = mypac);
    let input: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[task(binds = UART, priority = 1)]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(AppendInitPass);
    let expanded = builder.build_rtic_macro2(args, input);
    assert!(expanded.to_string().contains("__extra_init"));
}

#[test]
fn full_pipeline_post_core_pass_appends_to_output() {
    struct AppendCommentPass;

    impl rtic_core::RticPass for AppendCommentPass {
        fn run_pass(
            &self,
            args: TokenStream,
            mut app_mod: syn::ItemMod,
        ) -> syn::Result<(TokenStream, syn::ItemMod)> {
            if let Some((_, items)) = &mut app_mod.content {
                let comment: syn::Item = syn::parse_quote! {
                    const __POST_CORE_PASS_MARKER: () = ();
                };
                items.push(comment);
            }
            Ok((args, app_mod))
        }

        fn pass_name(&self) -> &str {
            "AppendCommentPass"
        }
    }

    let args: TokenStream = quote!(device = mypac);
    let input: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[init]
            fn init() -> () {}

            #[task(binds = UART, priority = 1)]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };

    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_post_core_pass(AppendCommentPass);
    let expanded = builder.build_rtic_macro2(args, input);
    assert!(expanded.to_string().contains("__POST_CORE_PASS_MARKER"));
}

#[test]
fn full_pipeline_rejects_invalid_app() {
    let args: TokenStream = quote!(device = mypac);
    let input: syn::ItemMod = syn::parse_quote! {
        mod app {
            // missing init
            #[task(binds = UART, priority = 1)]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let expanded = builder.build_rtic_macro2(args, input);
    let expanded = expanded.to_string();
    assert!(expanded.contains("compile_error"));
    assert!(expanded.contains("init"));
}
