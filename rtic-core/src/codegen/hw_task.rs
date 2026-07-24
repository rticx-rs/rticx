use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ImplItem, ImplItemFn, parse_quote};

use crate::rtic_functions;
use crate::{
    CorePassBackend,
    codegen::utils,
    parser::ast::{HardwareTask, RticTask, SharedResources},
};

impl RticTask {
    /// Generates task definition, Context struct, resource proxies and binds task to appropriate interrupt
    pub fn generate_task_def(&self, shared_resources: Option<&SharedResources>) -> TokenStream2 {
        let task_ty = &self.task_struct.ident;
        let task_static_handle = &self.name_uppercase();
        let task_struct = &self.task_struct;
        let task_impl = &self.struct_impl;
        let task_trait_check = rtic_functions::trait_check_call_for(self);

        let task_prio_impl = self.generate_priority_func();
        let shared_mod = shared_resources.map(|shared| shared.generate_shared_for_task(self));
        let current_current_fn = self.generate_current_core_fn();
        quote! {
            static mut #task_static_handle: core::mem::MaybeUninit<#task_ty> = core::mem::MaybeUninit::uninit();
            #task_struct
            #task_trait_check

            #task_impl

            #task_prio_impl
            #shared_mod
            #current_current_fn
        }
    }

    pub fn task_init_call(&self) -> Option<TokenStream2> {
        if self.user_initializable {
            // it is user responsibility to initialize task, and this is enforced at compiler time
            return None;
        }
        let task_ty = &self.name();
        let task_static_handle = &self.name_uppercase();
        Some(quote! { #task_static_handle.write(#task_ty::init(())); })
    }

    fn generate_priority_func(&self) -> TokenStream2 {
        let task_ty = self.name();
        let task_prio = self.args.priority;
        quote! {
            impl #task_ty {
                pub const fn priority() -> u16 {
                    #task_prio
                }
            }
        }
    }

    fn generate_current_core_fn(&self) -> TokenStream2 {
        let task_name = self.name();
        let core_type = utils::core_type(self.args.core);
        quote! {
            impl #task_name {
                pub const fn current_core() -> #core_type {
                    unsafe {#core_type::new()}
                }
            }
        }
    }
}

impl HardwareTask {
    /// Generates task definition, Context struct, resource proxies and binds task to appropriate interrupt
    pub fn generate_hw_task_to_irq_binding(
        &self,
        implementation: &dyn CorePassBackend,
    ) -> Option<TokenStream2> {
        let task_attrs = implementation.task_attrs();
        let task_static_handle = &self.name_uppercase();
        let task_irq_handler = &self.args.binds.clone()?;

        let default_task_dispatch_call = quote! {
            unsafe {#task_static_handle.assume_init_mut().exec()};
        };

        let task_dispatch_call = implementation
            .wrap_task_execution(self.args.priority, default_task_dispatch_call.clone())
            .unwrap_or(default_task_dispatch_call);

        Some(quote! {
            #[allow(non_snake_case)]
            #[unsafe(no_mangle)]
            #(#task_attrs)*
            fn #task_irq_handler() {
                #task_dispatch_call
            }
        })
    }

    /// If the type InitArgs is not implement it generate a default implementation
    /// If the type InitArgs is implemented, generate a custom initialization function for the task
    pub fn adjust_task_impl_initialization(&mut self) -> syn::Result<()> {
        let Some(task_impl) = &mut self.struct_impl else {
            // if the trait implementation of the task is not found on the parsed module (external task implementation)
            // the ask the user to provide explicit initialization.
            self.user_initializable = true;
            return Ok(());
        };

        let default_init_type_def: syn::ImplItemType = parse_quote!(
            type InitArgs = ();
        );
        let init_args_type = task_impl.items.iter().find_map(|item| {
            let ImplItem::Type(t) = item else { return None };
            if t == &default_init_type_def {
                Some((t, true))
            } else if t.ident == "InitArgs" {
                Some((t, false))
            } else {
                None
            }
        });

        match init_args_type {
            Some((_, false)) => {
                // user implements custom type
                self.user_initializable = true;

                return Ok(());
            }
            None => {
                // user ask rtic to implicitly generate default implementation
                task_impl.items.push(ImplItem::Type(default_init_type_def))
            }
            Some((_, true)) => { // user implements unit type
            }
        }

        // find the init function and correct its signature
        task_impl.items.iter_mut().for_each(|item| {
            if let ImplItem::Fn(f) = item
                && f.sig.ident == "init"
            {
                let default_init: ImplItemFn = parse_quote!(
                    fn init(_: ()) -> Self {}
                );
                f.sig = default_init.sig; // correct the signature
            }
        });

        Ok(())
    }
}
