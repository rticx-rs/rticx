use rtic_core::analysis::Analysis;
use rtic_core::parser::App;

mod common;

#[test]
fn analysis_updates_resource_priority() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");

    let sub = &app.sub_apps[0];
    let shared = sub.shared.as_ref().expect("shared resources exist");
    let counter = shared.get_field(&quote::format_ident!("counter")).expect("counter resource");
    assert_eq!(counter.priority, 2);

    let sub_analysis = &analysis.sub_analysis[0];
    assert_eq!(sub_analysis.used_irqs.len(), 1);
    assert_eq!(sub_analysis.used_irqs[0].0.to_string(), "UART");
    assert_eq!(sub_analysis.used_irqs[0].1, 2);
}

#[test]
fn analysis_computes_max_resource_priority() {
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let module: syn::ItemMod = syn::parse_quote! {
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
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }

            #[task(binds = TIMER, priority = 5, shared = [counter])]
            struct TimerTask;

            impl RticTask for TimerTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { TimerTask }
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let _ = Analysis::run(&mut app).expect("analysis succeeds");

    let shared = app.sub_apps[0].shared.as_ref().unwrap();
    let counter = shared.get_field(&quote::format_ident!("counter")).unwrap();
    assert_eq!(counter.priority, 5);
}

#[test]
fn analysis_detects_missing_resource() {
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> Shared {
                Shared { counter: 0 }
            }

            #[task(binds = UART, priority = 2, shared = [missing])]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let err = Analysis::run(&mut app).expect_err("missing resource should fail");
    assert!(err.to_string().contains("missing"));
}

#[test]
fn analysis_collects_late_resource_tasks() {
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> Shared {
                Shared { counter: 0 }
            }

            #[task(binds = UART, priority = 2)]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }

            #[task(binds = TIMER, priority = 3)]
            struct TimerTask;

            impl RticTask for TimerTask {
                type InitArgs = u32;
                fn init(_: u32) -> Self { TimerTask }
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");

    let late = &analysis.sub_analysis[0].late_resource_tasks;
    assert_eq!(late.len(), 1);
    assert_eq!(late[0].name_snakecase().to_string(), "timer_task");
    assert_eq!(late[0].name_uppercase().to_string(), "TIMER_TASK");
}

#[test]
fn analysis_collects_task_traits() {
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let module: syn::ItemMod = syn::parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> Shared {
                Shared { counter: 0 }
            }

            #[task(binds = UART, priority = 2, task_trait = CustomTrait)]
            struct UartTask;

            impl CustomTrait for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");
    assert!(analysis.task_traits.iter().any(|t| t == "CustomTrait"));
}
