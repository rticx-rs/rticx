use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};

use crate::parser::ast::{RticTask, SharedResources};
use crate::rticx_functions::get_resource_proxy_lock_fn;
use crate::rticx_traits::MUTEX_TY;
use crate::{AppArgs, CorePassBackend, SubApp};

impl SharedResources {
    pub fn generate_shared_resources_def(&self) -> TokenStream2 {
        let shared_struct = &self.strct;
        let resources_ty = &shared_struct.ident;
        let static_instance_name = &self.name_uppercase();

        quote! {
            static mut #static_instance_name: core::mem::MaybeUninit<#resources_ty> = core::mem::MaybeUninit::uninit();
            #shared_struct
        }
    }

    pub fn generate_resource_proxies(
        &self,
        implementor: &dyn CorePassBackend,
        app_params: &AppArgs,
        app_info: &SubApp,
    ) -> TokenStream2 {
        let static_mut_shared_resources = self.name_uppercase();
        let proxies = self.resources.iter().map(|element| {
            let element_name = &element.ident;
            let element_ty = &element.ty;
            let proxy_name = utils::get_proxy_name(element_name);
            let mutex_ty = format_ident!("{}", MUTEX_TY);

            // generate the implementation of lock function, using external implementation
            let impl_lock_fn = get_resource_proxy_lock_fn(
                implementor,
                app_params,
                app_info,
                element,
                &static_mut_shared_resources,
            );

            quote! {
                // Resource proxy for `#element_name`
                pub struct #proxy_name<const TASK_PRIORITY: u16>;

                impl<const TASK_PRIORITY: u16> #proxy_name<TASK_PRIORITY> {
                    #[inline(always)]
                    pub fn new() -> Self {
                        Self
                    }
                }

                impl<const TASK_PRIORITY: u16> #mutex_ty for #proxy_name<TASK_PRIORITY> {
                    type ResourceType = #element_ty;
                    #impl_lock_fn
                }
            }
        });
        quote! {
            #(#proxies)*
        }
    }

    pub fn generate_shared_for_task(&self, task: &RticTask) -> TokenStream2 {
        let task_resources_idents = &task.args.shared;
        if task_resources_idents.is_empty() {
            return quote!();
        }

        let task_ty = task.name();
        let task_prio = task.args.priority;
        let task_shared_resources_struct =
            format_ident!("__{}_shared_resources", task.name_snakecase());

        // generate `field_name : proxy_type<priority>` to populate the struct body
        let field_and_proxytype = task_resources_idents.iter().filter_map(|resource_ident| {
            self.get_field(resource_ident).map(|resource| {
                let ident = &resource.ident;
                let proxy_type = utils::get_proxy_name(ident);
                quote! {#ident: #proxy_type<#task_prio>}
            })
        });
        let proxy_inits = task_resources_idents.iter().filter_map(|resource_ident| {
            self.get_field(resource_ident).map(|resource| {
                let ident = &resource.ident;
                let proxy_type = utils::get_proxy_name(ident);
                quote! {#ident: #proxy_type::new()}
            })
        });

        quote! {
            // Shared resources access through shared() API for `#task_ty`
            impl #task_ty {
                pub fn shared(&self) -> #task_shared_resources_struct {
                    #task_shared_resources_struct::new()
                }
            }

            // internal struct for `#task_ty` resource proxies
            pub struct #task_shared_resources_struct {
                #(pub #field_and_proxytype ,)*
            }

            impl #task_shared_resources_struct {
                #[inline(always)]
                pub fn new() -> Self {
                    Self {
                        #(#proxy_inits ,)*
                    }
                }
            }

        }
    }
}

pub mod utils {
    use quote::format_ident;

    #[inline(always)]
    pub fn get_proxy_name(ident: &syn::Ident) -> syn::Ident {
        format_ident!("__{ident}_mutex")
    }
}
