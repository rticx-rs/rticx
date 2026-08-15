use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote};
use std::collections::HashSet;
use task_init::{generate_task_inits_struct, generate_task_inits_write_calls};

use crate::CorePassBackend;
use crate::MainInjections;
use crate::analysis::Analysis;
use crate::parser::ast::{RticTask, SharedResources};
use crate::parser::{App, ast::IdleTask};
use crate::rticx_functions::{
    INTERRUPT_FREE_FN, generate_task_traits_check_functions, get_interrupt_free_fn,
};
use crate::rticx_traits::get_rticx_traits_mod;

pub mod hw_task;
pub mod shared_resources;
pub mod task_init;
pub mod utils;

pub struct CodeGen<'a> {
    app: &'a App,
    analysis: &'a Analysis,
    implementation: &'a dyn CorePassBackend,
    injections: Option<&'a MainInjections>,
}

impl<'a> CodeGen<'a> {
    pub fn new(
        implementation: &'a dyn CorePassBackend,
        app: &'a App,
        analysis: &'a Analysis,
    ) -> Self {
        Self {
            app,
            analysis,
            implementation,
            injections: None,
        }
    }

    pub fn with_injections(mut self, injections: &'a MainInjections) -> Self {
        self.injections = Some(injections);
        self
    }

    pub fn run(&self) -> TokenStream2 {
        let app = self.app;
        let implementation = self.implementation;

        let app_mod = &app.app_name;
        let peripheral_crate = generate_use_pac_statement(app);
        let user_includes = &app.user_includes;
        let user_code = &app.other_code;
        let warnings = &app.args.warnings;
        let interrupt_free_fn = get_interrupt_free_fn(implementation);

        // traits
        let rticx_traits_mod = get_rticx_traits_mod();

        // sub_apps
        let sub_apps = self.generate_sub_apps();

        // task trait checks
        let task_trait_check_functions = generate_task_traits_check_functions(self.analysis);

        quote! {
            pub mod #app_mod {
                #![allow(non_upper_case_globals)]
                #![allow(non_snake_case)]
                /// Include peripheral crate(s) that defines the vector table
                #peripheral_crate

                // ================================== user includes ====================================
                #(#user_includes)*
                // ================================== app args warnings =================================
                #(#warnings)*
                // ==================================== rticx traits ====================================
                #rticx_traits_mod
                // ================================== rticx functions ===================================
                /// critical section function
                #interrupt_free_fn
                // ==================================== User code ======================================
                #(#user_code)*

                // sub applications
                #sub_apps

                /// Utility functions used to enforce implementing appropriate task traits
                #task_trait_check_functions
            }
        }
    }

    fn generate_sub_apps(&self) -> TokenStream2 {
        let implementation = self.implementation;
        let iter = self
            .app
            .sub_apps
            .iter()
            .zip(self.analysis.sub_analysis.iter());
        let args = &self.app.args;
        let apps = iter.map(|(app, analysis)| {
            let post_init = implementation.post_init(args, app, analysis);

            // init
            let def_init_task = &app.init.body;
            let init_task = &app.init.ident;
            let task_inits_ident = if self.app.sub_apps.len() == 1 {
                format_ident!("TaskInits")
            } else {
                format_ident!("TaskInitsCore{}", app.core)
            };
            let task_inits_struct =
                generate_task_inits_struct(&task_inits_ident, &analysis.late_resource_tasks);

            // idle
            let def_idle_task = app.idle.as_ref().map(|idle| {
                let idle_task = idle.generate_task_def(app.shared.as_ref());
                Some(idle_task)
            });

            // post_init
            let post_init_call = app.post_init.as_ref().map(|pi| {
                let ident = &pi.ident;
                let body = &pi.body;
                quote! {
                    #[doc(hidden)]
                    #body

                    #ident();
                }
            });

            let call_idle_task =
                generate_idle_call(app.idle.as_ref(), implementation.populate_idle_loop());

            // tasks
            let tasks_def = app
                .tasks
                .iter()
                .map(|task| task.generate_task_def(app.shared.as_ref()));
            let task_init_calls: Vec<_> = app
                .tasks
                .iter()
                .filter_map(RticTask::task_init_call)
                .collect();
            let generated_task_inits = if task_init_calls.is_empty() {
                quote! {}
            } else {
                quote! { unsafe {#(#task_init_calls)*} }
            };

            let hw_tasks_binds = app
                .tasks
                .iter()
                .filter_map(|t| t.generate_hw_task_to_irq_binding(implementation));

            // shared resources
            let shared = app.shared.as_ref();
            let def_shared = shared.map(|shared| shared.generate_shared_resources_def());
            let shared_resources_handle = shared.map(SharedResources::name_uppercase);

            let resource_proxies = app
                .shared
                .as_ref()
                .map(|shared| shared.generate_resource_proxies(implementation, args, app));

            // local and shared resources initialization
            let tasks_initializer = format_ident!("__task_inits");
            let user_task_inits_writes = if analysis.late_resource_tasks.is_empty() {
                quote! {}
            } else {
                generate_task_inits_write_calls(&analysis.late_resource_tasks, &tasks_initializer)
            };
            let shared_resource_ty = shared.map(|s| &s.strct.ident);

            let init_system = if let Some(shared_resource_ty) = shared_resource_ty {
                quote! {
                    let (__shared_resources, #tasks_initializer) : (#shared_resource_ty, #task_inits_ident) = #init_task(); // call to init and get shared and local resources inits
                    unsafe {#shared_resources_handle.write(__shared_resources);} // init shared resources
                    #user_task_inits_writes
                }
            } else {
                quote! {
                    let #tasks_initializer : #task_inits_ident = #init_task(); // call to init and get shared and local resources inits
                    #user_task_inits_writes
                }
            };

            // priority masks
            let priority_masks = implementation.generate_global_definitions(args, app, analysis);
            let entry_attrs = implementation.entry_attrs();
            let entry_name = implementation.entry_name(app.core);

            let interrupt_free = format_ident!("{}", INTERRUPT_FREE_FN);

            let def_core_type = generate_core_type(app.core);

            let empty = Vec::new();
            let before_init = self
                .injections
                .map(|inj| &inj.before_init)
                .unwrap_or(&empty);
            let before_post_init = self
                .injections
                .map(|inj| &inj.before_post_init)
                .unwrap_or(&empty);
            let before_idle = self
                .injections
                .map(|inj| &inj.before_idle)
                .unwrap_or(&empty);

            let doc = format!(" # CORE {}", app.core);
            let entry_of = format!(" # Entry of CORE {}", app.core);
            quote! {
                #[doc = #doc]
                // define static mut shared resources
                #def_shared
                // init task
                #def_init_task
                // idle task
                #def_idle_task
                // define tasks
                #(#tasks_def)*
                // bind hw tasks to interrupts
                #(#hw_tasks_binds)*
                // proxies for accessing the shared resources
                #resource_proxies
                // unique type for the specific sub-app/core
                #def_core_type
                // Computed priority Masks
                #priority_masks
                /// Type holding one value per user task, returned from `#[init]`.
                /// Tasks are constructed inline or via user-defined helpers.
                #task_inits_struct

                #[doc = #entry_of]
                #(#entry_attrs)*
                #[unsafe(no_mangle)]
                fn #entry_name() -> ! {
                    // Disable interrupts during initialization
                    #interrupt_free(||{
                        #(#before_init)*

                        // user init code
                        #init_system

                        // init framework-generated tasks
                        #generated_task_inits

                        // injections before post_init
                        #(#before_post_init)*

                        // post initialization code
                        #post_init
                    });

                    // injections before idle
                    #(#before_idle)*

                    // user post_init function
                    #post_init_call

                    #call_idle_task
                }

            }
        });

        quote!( #(#apps)* )
    }
}

fn generate_idle_call(idle: Option<&IdleTask>, wfi: Option<TokenStream2>) -> TokenStream2 {
    if let Some(idle) = idle {
        let idle_ty = &idle.name();
        let idle_instance_name = &idle.name_uppercase();
        if idle.init_generated {
            quote! {
                unsafe {
                    #idle_instance_name.write(#idle_ty);
                    #idle_instance_name.assume_init_mut().exec();
                }

            }
        } else {
            let idle_instance_name = &idle.name_uppercase();
            quote! {
                unsafe {
                    #idle_instance_name.assume_init_mut().exec();
                }
            }
        }
    } else {
        quote! {
            loop {
                #wfi
            }
        }
    }
}

/// Generates a unique type for some core that is unsafe to create by the uer.
/// I.e, it will be used for internal purposes so the the user shouldn't attemp to create it
fn generate_core_type(core: u32) -> TokenStream2 {
    let core_ty = utils::core_type(core);
    let innter_core_ty = utils::core_type_inner(core);
    let mod_core_ty = utils::core_type_mod(core);
    let doc = format!("Unique type for core {core}");

    quote! {
        #[doc = #doc]
        pub use #mod_core_ty::#core_ty;
        mod #mod_core_ty {
            struct #innter_core_ty;
            pub struct #core_ty(#innter_core_ty);
            impl #core_ty {
                pub const unsafe fn new() -> Self {
                    #core_ty(#innter_core_ty)
                }
            }
        }
    }
}

/// This will generate the `use path::to::pac as _;` statements.
/// This is usually needed as the PAC needs to be imported as it defines the vector table.
/// Each core may use a different PAC, so one import is emitted per unique PAC path.
fn generate_use_pac_statement(app: &App) -> TokenStream2 {
    let mut seen = HashSet::new();
    let pacs = app
        .args
        .pacs
        .iter()
        .filter(|path| seen.insert(path.to_token_stream().to_string()))
        .collect::<Vec<_>>();
    quote! {
        #(use #pacs as _;)*
    }
}
