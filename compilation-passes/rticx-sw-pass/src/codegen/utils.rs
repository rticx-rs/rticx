use heck::ToSnakeCase;
use proc_macro2::Span;
use quote::format_ident;
use syn::Ident;

/// used for statics
pub fn ident_uppercase(ident: &Ident) -> Ident {
    let name = ident.to_string().to_snake_case().to_uppercase();
    Ident::new(&name, Span::call_site())
}

pub fn priority_ty_ident(priority: u16, core: u32) -> Ident {
    format_ident!("Core{core}Prio{priority}Tasks")
}

pub fn dispatcher_ident(priority: u16, core: u32) -> Ident {
    format_ident!("Core{core}Priority{priority}Dispatcher")
}
pub fn priority_queue_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{prio_ty}__RQ")
}

pub fn sw_task_inputs_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_internal__{task_ident}__INPUTS")
}
