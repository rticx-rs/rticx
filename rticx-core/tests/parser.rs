use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_core::parser::{App, ast::AppArgs};

mod common;

#[test]
fn parse_single_core_app_args() {
    let args: TokenStream = quote!(device = mypac);
    let parsed = AppArgs::parse(args).expect("valid app args");
    assert_eq!(parsed.cores, 1);
    assert_eq!(parsed.pacs.len(), 1);
    assert_eq!(parsed.pacs[0].to_token_stream().to_string(), "mypac");
}

#[test]
fn parse_app_args_with_cores() {
    let args: TokenStream = quote!(device = mypac, cores = 2);
    let parsed = AppArgs::parse(args).expect("valid app args");
    assert_eq!(parsed.cores, 2);
    assert_eq!(parsed.pacs.len(), 2);
    assert!(
        parsed
            .pacs
            .iter()
            .all(|p| p.to_token_stream().to_string() == "mypac")
    );
}

#[test]
fn parse_app_args_missing_device_fails() {
    let args: TokenStream = quote!(cores = 2);
    let err = AppArgs::parse(args).expect_err("missing device should fail");
    assert!(err.to_string().contains("device"));
}

#[test]
fn parse_single_core_app() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let app = App::parse(args, module).expect("valid single-core app");
    assert_eq!(app.app_name.to_string(), "app");
    assert_eq!(app.sub_apps.len(), 1);
    let sub = &app.sub_apps[0];
    assert_eq!(sub.core, 0);
    assert!(sub.shared.is_some());
    assert_eq!(sub.tasks.len(), 1);
    assert_eq!(sub.tasks[0].name().to_string(), "UartTask");
    assert!(sub.idle.is_some());
    assert_eq!(sub.idle.as_ref().unwrap().name().to_string(), "Idle");
}

#[test]
fn parse_multi_core_app() {
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
            fn init0() -> (Shared0, TaskInitsCore0) {
                (Shared0 { counter: 0 }, TaskInitsCore0 { uart_task0: UartTask0 })
            }

            #[init(core = 1)]
            fn init1() -> (Shared1, TaskInitsCore1) {
                (Shared1 { counter: 0 }, TaskInitsCore1 { uart_task1: UartTask1 })
            }

            #[task(binds = UART0, priority = 2, shared = [counter], core = 0)]
            struct UartTask0;

            impl RticTask for UartTask0 {
                fn exec(&mut self) {}
            }

            #[task(binds = UART1, priority = 3, shared = [counter], core = 1)]
            struct UartTask1;

            impl RticTask for UartTask1 {
                fn exec(&mut self) {}
            }
        }
    };
    let app = App::parse(args, module).expect("valid multi-core app");
    assert_eq!(app.sub_apps.len(), 2);
    assert_eq!(app.sub_apps[0].core, 0);
    assert_eq!(app.sub_apps[1].core, 1);
    assert_eq!(app.sub_apps[0].tasks.len(), 1);
    assert_eq!(app.sub_apps[1].tasks.len(), 1);
}

#[test]
fn parse_app_without_init_fails() {
    let args: TokenStream = quote!(device = mypac);
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[task(binds = UART, priority = 1)]
            struct UartTask;

            impl RticTask for UartTask {
                fn exec(&mut self) {}
            }
        }
    };
    let err = App::parse(args, module).expect_err("missing init should fail");
    assert!(err.to_string().contains("init"));
}

#[test]
fn parse_task_args_default_values() {
    use rticx_core::parser::ast::TaskArgs;
    let meta: syn::Meta = syn::parse_quote!(task(binds = UART, priority = 2, shared = [counter]));
    let args = TaskArgs::parse(meta).expect("valid task args");
    assert_eq!(
        args.binds.as_ref().map(|i| i.to_string()),
        Some("UART".to_string())
    );
    assert_eq!(args.priority, 2);
    assert_eq!(args.shared.len(), 1);
    assert_eq!(args.shared[0].to_string(), "counter");
    assert_eq!(args.core, 0);
    assert_eq!(args.task_trait.to_string(), "RticTask");
}

#[test]
fn parse_task_args_with_core_and_trait() {
    use rticx_core::parser::ast::TaskArgs;
    let meta: syn::Meta = syn::parse_quote!(task(
        binds = UART,
        priority = 3,
        core = 1,
        task_trait = CustomTrait
    ));
    let args = TaskArgs::parse(meta).expect("valid task args");
    assert_eq!(args.core, 1);
    assert_eq!(args.task_trait.to_string(), "CustomTrait");
}

#[test]
fn parse_task_args_defaults() {
    use rticx_core::parser::ast::TaskArgs;
    let meta: syn::Meta = syn::parse_quote!(task);
    let args = TaskArgs::parse(meta).expect("valid task args");
    assert!(args.binds.is_none());
    assert_eq!(args.core, 0);
    assert_eq!(args.priority, 1);
    assert_eq!(args.task_trait.to_string(), "RticTask");
    assert_eq!(args.shared.len(), 0);
    assert!(!args.init_generated);
}

#[test]
fn parse_task_args_init_generated() {
    use rticx_core::parser::ast::TaskArgs;
    let meta: syn::Meta = syn::parse_quote!(task(binds = UART, init = generated));
    let args = TaskArgs::parse(meta).expect("valid task args");
    assert!(args.init_generated);
}

#[test]
fn parse_task_args_init_bad_value_errors() {
    use rticx_core::parser::ast::TaskArgs;
    let meta: syn::Meta = syn::parse_quote!(task(binds = UART, init = user));
    let result = TaskArgs::parse(meta);
    assert!(result.is_err());
}

#[test]
fn task_is_user_initializable_by_default() {
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits { uart_task: UartTask })
            }

            #[task(binds = UART, priority = 2)]
            struct UartTask;

            impl RticTask for UartTask {
                fn exec(&mut self) {}
            }
        }
    };
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    assert!(!task.init_generated);
    assert!(task.task_init_call().is_none());
}

#[test]
fn task_marks_generated_as_framework_initialized() {
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits { uart_task: UartTask })
            }

            #[task(binds = UART, priority = 2, init = generated)]
            struct UartTask;

            impl RticTask for UartTask {
                fn exec(&mut self) {}
            }
        }
    };
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    assert!(task.init_generated);
    assert!(task.task_init_call().is_some());
}

#[test]
fn task_custom_trait_impl_captured() {
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits { uart_task: UartTask })
            }

            #[task(binds = UART, priority = 2, task_trait = RticAsyncTask)]
            struct Foo;

            impl RticAsyncTask for Foo {
                fn exec(&mut self) {}
            }
        }
    };
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    assert!(task.struct_impl.is_some());
    assert_eq!(task.args.task_trait.to_string(), "RticAsyncTask");
}

#[test]
fn task_mismatched_trait_impl_not_captured() {
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {}

            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared {}, TaskInits { uart_task: UartTask })
            }

            #[task(binds = UART, priority = 2, task_trait = RticAsyncTask)]
            struct Foo;

            impl RticTask for Foo {
                fn exec(&mut self) {}
            }
        }
    };
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    // The impl RticTask does NOT match task_trait = RticAsyncTask, so it should NOT be captured
    assert!(task.struct_impl.is_none());
}
