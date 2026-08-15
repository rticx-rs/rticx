use rticx_core::analysis::Analysis;
use rticx_core::parser::App;

mod common;

#[test]
fn analysis_updates_resource_priority() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");

    let sub = &app.sub_apps[0];
    let shared = sub.shared.as_ref().expect("shared resources exist");
    let counter = shared
        .get_field(&quote::format_ident!("counter"))
        .expect("counter resource");
    assert_eq!(counter.priority, 2);

    let sub_analysis = &analysis.sub_analysis[0];
    assert_eq!(sub_analysis.used_irqs.len(), 1);
    assert_eq!(sub_analysis.used_irqs[0].name.to_string(), "UART");
    assert_eq!(sub_analysis.used_irqs[0].priority, 2);
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
            fn init() -> (Shared, TaskInits) {
                (
                    Shared { counter: 0 },
                    TaskInits { uart_task: UartTask, timer_task: TimerTask },
                )
            }

            #[task(binds = UART, priority = 2, shared = [counter])]
            struct UartTask;

            impl RticTask for UartTask {
                fn exec(&mut self) {}
            }

            #[task(binds = TIMER, priority = 5, shared = [counter])]
            struct TimerTask;

            impl RticTask for TimerTask {
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
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits { uart_task: UartTask })
            }

            #[task(binds = UART, priority = 2, shared = [missing])]
            struct UartTask;

            impl RticTask for UartTask {
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
            fn init() -> (Shared, TaskInits) {
                (
                    Shared { counter: 0 },
                    TaskInits { uart_task: UartTask, timer_task: TimerTask },
                )
            }

            #[task(binds = UART, priority = 2)]
            struct UartTask;

            impl RticTask for UartTask {
                fn exec(&mut self) {}
            }

            #[task(binds = TIMER, priority = 3)]
            struct TimerTask;

            impl RticTask for TimerTask {
                fn exec(&mut self) {}
            }

            // framework-generated task: must NOT appear in TaskInits
            #[task(binds = DMA, priority = 4, init = generated)]
            struct GenTask;

            impl RticTask for GenTask {
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");

    let late = &analysis.sub_analysis[0].late_resource_tasks;
    // both user tasks are collected; the generated task is excluded
    assert_eq!(late.len(), 2);
    let names: Vec<String> = late
        .iter()
        .map(|t| t.name_snakecase().to_string())
        .collect();
    assert!(names.contains(&"uart_task".to_string()));
    assert!(names.contains(&"timer_task".to_string()));
    assert!(!names.contains(&"gen_task".to_string()));
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
            fn init() -> (Shared, TaskInits) {
                (Shared { counter: 0 }, TaskInits { uart_task: UartTask })
            }

            #[task(binds = UART, priority = 2, task_trait = CustomTrait)]
            struct UartTask;

            impl CustomTrait for UartTask {
                fn exec(&mut self) {}
            }
        }
    };

    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");
    assert!(analysis.task_traits.iter().any(|t| t == "CustomTrait"));
}
