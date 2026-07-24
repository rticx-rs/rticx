use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rtic_core::parser::{App, ast::AppArgs};

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

            #[task(binds = UART1, priority = 3, shared = [counter], core = 1)]
            struct UartTask1;

            impl RticTask for UartTask1 {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask1 }
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
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };
    let err = App::parse(args, module).expect_err("missing init should fail");
    assert!(err.to_string().contains("init"));
}

#[test]
fn parse_task_args_default_values() {
    use rtic_core::parser::ast::TaskArgs;
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
    use rtic_core::parser::ast::TaskArgs;
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
    use rtic_core::parser::ast::TaskArgs;
    // The default task priority is set by the backend before parsing; this test runs with the
    // mock backend value of 1 because the full builder sets the static.
    let meta: syn::Meta = syn::parse_quote!(task);
    let args = TaskArgs::parse(meta).expect("valid task args");
    assert!(args.binds.is_none());
    assert_eq!(args.core, 0);
    assert_eq!(args.task_trait.to_string(), "RticTask");
    // priority defaults to whatever DEFAULT_TASK_PRIORITY currently is; this test is only
    // checking the shape of the defaults, not the exact value.
    assert_eq!(args.shared.len(), 0);
}
