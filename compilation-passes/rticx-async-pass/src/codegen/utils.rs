use proc_macro2::Ident;
use quote::format_ident;

pub use rticx_sw_pass::common::codegen::{
    dispatcher_ident, ident_uppercase, priority_queue_ident, priority_ty_ident,
    sw_task_inputs_ident,
};

pub fn overflow_queue_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__OQ", prio_ty)
}

pub fn install_fn_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__try_install", prio_ty)
}

pub fn exec_wake_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_internal__{}__wake", task_ident)
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
