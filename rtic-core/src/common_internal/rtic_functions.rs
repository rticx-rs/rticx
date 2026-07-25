use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ImplItemFn, ItemFn, parse_quote};

use crate::{
    Analysis, AppArgs, CorePassBackend, SubApp,
    parser::ast::{RticTask, SharedElement},
};

pub const INTERRUPT_FREE_FN: &str = "__rtic_interrupt_free";

pub(crate) fn get_interrupt_free_fn(implementor: &dyn CorePassBackend) -> ItemFn {
    let fn_ident = format_ident!("{INTERRUPT_FREE_FN}");
    let critical_section_fn = parse_quote! {
        #[inline]
        pub fn #fn_ident<F, R>(f: F) -> R
        where F: FnOnce() -> R,
        {
           // IMPLEMENTOR RESPONSIBILITY: implement a traditional interrupt critical section
        }
    };
    implementor.generate_interrupt_free_fn(critical_section_fn)
    // TODO: we should validate if the implementor has kept the correct function signature by comparing it to the initial signature
}

pub(crate) fn get_resource_proxy_lock_fn(
    implementor: &dyn CorePassBackend,
    app_params: &AppArgs,
    app_info: &SubApp,
    resource: &SharedElement,
    static_mut_shared_resources: &syn::Ident,
) -> ImplItemFn {
    let ceiling = resource.priority;
    let resource_ident = &resource.ident;
    let lock_fn = parse_quote! {
        fn lock<R>(&mut self, f: impl FnOnce(&mut Self::ResourceType) -> R) -> R {
            // `self` refers to the resource proxy struct

            const CEILING: u16 = #ceiling; // resource priority ceiling
            let task_priority = self.task_priority; // running task priority
            let resource_ptr = unsafe { // get a mut pointer to the resource
                &mut #static_mut_shared_resources.assume_init_mut().#resource_ident
            } as *mut _;
            // IMPLEMENTOR RESPONSIBILITY: continue lock implementation here
            // call for example <distro>::export::lock(resource_ptr, task_priority, ...., f)
        }
    };
    implementor.generate_resource_proxy_lock_impl(app_params, app_info, lock_fn)
    // TODO: we should validate if the implementor has kept the correct function signature by comparing it to the initial signature
}

pub(crate) fn task_trait_check_fn_name(trait_ident: &syn::Ident) -> syn::Ident {
    let trait_lower = trait_ident.to_string().to_snake_case();
    format_ident!("implements_{trait_lower}")
}
pub(crate) fn trait_check_call_for(task: &RticTask) -> TokenStream {
    let task_trait = &task.args.task_trait;
    let task_ty = &task.task_struct.ident;
    let check_fn_name = task_trait_check_fn_name(task_trait);

    quote! {
        const _: fn() = || {
            __rtic_trait_checks::#check_fn_name::<#task_ty>();
        };
    }
}

fn generate_trait_check_fn(task_trait: &syn::Ident) -> TokenStream {
    let check_fn_name = task_trait_check_fn_name(task_trait);
    quote! {
        pub fn #check_fn_name<T: #task_trait>(){}
    }
}

pub(crate) fn generate_task_traits_check_functions(analysis: &Analysis) -> TokenStream {
    let function_definitions = analysis.task_traits.iter().map(generate_trait_check_fn);
    quote! {
        mod __rtic_trait_checks {
            use super::*;
            #(#function_definitions)*
        }
    }
}
