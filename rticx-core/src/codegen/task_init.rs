use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemStruct, parse_quote};

use crate::analysis::LateResourceTask;

pub fn generate_late_init_tasks_struct(tasks: &[LateResourceTask]) -> Option<ItemStruct> {
    if tasks.is_empty() {
        return None;
    }
    let struct_fields = tasks.iter().map(|t| {
        let field_name = t.name_snakecase();
        let field_ty = &t.task_name;
        quote! {pub #field_name: #field_ty,}
    });
    Some(parse_quote! {
        pub struct TaskInits {
            #(#struct_fields)*
        }
    })
}

pub fn generate_late_tasks_init_calls(
    tasks: &[LateResourceTask],
    initializer_instance: &syn::Ident,
) -> TokenStream {
    let init_calls = tasks.iter().map(|t| {
        let field_name = t.name_snakecase();
        let instance_name = t.name_uppercase();
        quote! {
            #instance_name.write(#initializer_instance.#field_name);
        }
    });
    quote! {
        unsafe{#(#init_calls)*}
    }
}
