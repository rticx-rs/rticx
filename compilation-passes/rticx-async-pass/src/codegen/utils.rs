use heck::ToSnakeCase;
use proc_macro2::Ident;
use quote::format_ident;

pub fn ident_uppercase(ident: &Ident) -> Ident {
    format_ident!("{}", ident.to_string().to_snake_case().to_uppercase())
}

pub fn priority_ty_ident(priority: u16, core: u32) -> Ident {
    format_ident!("Core{core}Prio{priority}Tasks")
}

pub fn dispatcher_ident(priority: u16, core: u32) -> Ident {
    format_ident!("Core{core}Priority{priority}Dispatcher")
}

pub fn priority_queue_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__RQ", prio_ty)
}

pub fn overflow_queue_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__OQ", prio_ty)
}

pub fn install_fn_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__try_install", prio_ty)
}

pub fn sw_task_inputs_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__INPUTS", task_ident)
}

pub fn exec_wake_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__wake", task_ident)
}

pub fn core_type(core: u32) -> Ident {
    format_ident!("__rticx__internal__Core{core}")
}

pub fn async_wrapper_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_async_{}", task_ident)
}

pub fn exec_ptr_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__PTR", task_ident)
}

pub fn idle_executor_ident(core: u32) -> Ident {
    format_ident!("__RticxAsyncPrio0ExecutorCore{core}")
}
