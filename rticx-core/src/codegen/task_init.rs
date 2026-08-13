use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, ItemStruct, parse_quote};

use crate::analysis::LateResourceTask;

/// Generates the `TaskInits` struct that the user must return (along with the
/// shared resources) from `#[init]`. It contains one field per user task; the
/// user constructs the tasks inline or through their own helper functions.
pub fn generate_task_inits_struct(struct_name: &Ident, tasks: &[LateResourceTask]) -> ItemStruct {
    let struct_fields = tasks.iter().map(|t| {
        let field_name = t.name_snakecase();
        let field_ty = &t.task_name;
        quote! {pub #field_name: #field_ty,}
    });
    parse_quote! {
        pub struct #struct_name {
            #(#struct_fields)*
        }
    }
}

pub fn generate_task_inits_write_calls(
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
