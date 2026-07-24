use rtic_core::analysis::{Analysis, LateResourceTask};
use rtic_core::codegen::CodeGen;
use rtic_core::mock_backend::MockCoreBackend;
use rtic_core::parser::App;

mod common;

#[test]
fn codegen_generates_shared_resources_def() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let app = App::parse(args, module).expect("valid app");
    let shared = app.sub_apps[0].shared.as_ref().expect("shared exists");
    let tokens = shared.generate_shared_resources_def();
    let s = tokens.to_string();
    assert!(s.contains("static mut SHARED"));
    assert!(s.contains("MaybeUninit"));
    assert!(s.contains("struct Shared"));
}

#[test]
fn codegen_generates_resource_proxies() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let mut app = App::parse(args, module).expect("valid app");
    let _ = Analysis::run(&mut app).expect("analysis succeeds");
    let shared = app.sub_apps[0].shared.as_ref().expect("shared exists");
    let sub_app = &app.sub_apps[0];
    let tokens = shared.generate_resource_proxies(&MockCoreBackend, &app.args, sub_app);
    let s = tokens.to_string();
    assert!(s.contains("__counter_mutex"));
    assert!(s.contains("impl RticMutex"));
    assert!(s.contains("resource_ptr"));
    assert!(s.contains("f ("));
}

#[test]
fn codegen_generates_shared_for_task() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let app = App::parse(args, module).expect("valid app");
    let shared = app.sub_apps[0].shared.as_ref().expect("shared exists");
    let task = &app.sub_apps[0].tasks[0];
    let tokens = shared.generate_shared_for_task(task);
    let s = tokens.to_string();
    assert!(s.contains("__uart_task_shared_resources"));
    assert!(s.contains("pub fn shared"));
    assert!(s.contains("__counter_mutex"));
}

#[test]
fn codegen_generates_task_def() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    let tokens = task.generate_task_def(app.sub_apps[0].shared.as_ref());
    let s = tokens.to_string();
    assert!(s.contains("static mut UART_TASK"));
    assert!(s.contains("struct UartTask"));
    assert!(s.contains("fn priority"));
    assert!(s.contains("fn current_core"));
}

#[test]
fn codegen_generates_task_init_call() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    let tokens = task.task_init_call().expect("init call exists");
    let s = tokens.to_string();
    assert!(s.contains("UART_TASK"));
    assert!(s.contains("write"));
}

#[test]
fn codegen_generates_hw_task_binding() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    let tokens = task.generate_hw_task_to_irq_binding(&MockCoreBackend);
    assert!(tokens.is_some());
    let s = tokens.unwrap().to_string();
    assert!(s.contains("fn UART"));
    assert!(s.contains("UART_TASK"));
}

#[test]
fn codegen_generates_late_init_struct() {
    use rtic_core::codegen::task_init::generate_late_init_tasks_struct;
    let tasks = vec![
        LateResourceTask {
            task_name: quote::format_ident!("UartTask"),
        },
        LateResourceTask {
            task_name: quote::format_ident!("TimerTask"),
        },
    ];
    let item_struct = generate_late_init_tasks_struct(&tasks).expect("struct generated");
    let tokens = quote::quote!(#item_struct);
    let s = tokens.to_string();
    assert!(s.contains("pub struct TaskInits"));
    assert!(s.contains("pub uart_task : UartTask"));
    assert!(s.contains("pub timer_task : TimerTask"));
}

#[test]
fn codegen_late_init_struct_empty() {
    use rtic_core::codegen::task_init::generate_late_init_tasks_struct;
    assert!(generate_late_init_tasks_struct(&[]).is_none());
}

#[test]
fn codegen_late_init_calls() {
    use rtic_core::codegen::task_init::generate_late_tasks_init_calls;
    let tasks = vec![
        LateResourceTask {
            task_name: quote::format_ident!("UartTask"),
        },
    ];
    let initializer = quote::format_ident!("task_inits");
    let tokens = generate_late_tasks_init_calls(&tasks, &initializer);
    let s = tokens.to_string();
    assert!(s.contains("UART_TASK"));
    assert!(s.contains("write"));
    assert!(s.contains("task_inits") && s.contains("uart_task"));
}

#[test]
fn codegen_full_run_contains_expected_sections() {
    let args = common::single_core_app_args();
    let module = common::single_core_app_module();
    let mut app = App::parse(args, module).expect("valid app");
    let analysis = Analysis::run(&mut app).expect("analysis succeeds");
    let tokens = CodeGen::new(&MockCoreBackend, &app, &analysis).run();
    let s = tokens.to_string();
    assert!(s.contains("pub mod app"));
    assert!(s.contains("use mypac as _"));
    assert!(s.contains("pub mod rtic_traits"));
    assert!(s.contains("fn main"));
    assert!(s.contains("__rtic_interrupt_free"));
    assert!(s.contains("UART_TASK"));
    assert!(s.contains("fn UART"));
    assert!(s.contains("SHARED"));
}

#[test]
fn task_adjusts_default_init_args() {
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
        }
    };
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    assert!(!task.user_initializable);
}

#[test]
fn task_marks_custom_init_args_as_user_initializable() {
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
                type InitArgs = u32;
                fn init(_: u32) -> Self { UartTask }
                fn exec(&mut self) {}
            }
        }
    };
    let args: proc_macro2::TokenStream = quote::quote!(device = mypac);
    let app = App::parse(args, module).expect("valid app");
    let task = &app.sub_apps[0].tasks[0];
    assert!(task.user_initializable);
    assert!(task.task_init_call().is_none());
}
