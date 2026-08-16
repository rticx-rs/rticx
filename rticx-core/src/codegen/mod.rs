use proc_macro2::TokenStream as TokenStream2;
use quote::{ToTokens, format_ident, quote};
use std::collections::HashSet;
use task_init::{generate_task_inits_struct, generate_task_inits_write_calls};

use crate::CorePassBackend;
use crate::MainInjections;
use crate::analysis::{Analysis, SubAnalysis};
use crate::parser::ast::{RticTask, SharedResources};
use crate::parser::{App, SubApp, ast::IdleTask};
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
        let apps = iter.map(|(app, analysis)| self.generate_sub_app(implementation, app, analysis));

        quote!( #(#apps)* )
    }

    fn generate_sub_app(
        &self,
        implementation: &dyn CorePassBackend,
        app: &SubApp,
        analysis: &SubAnalysis,
    ) -> TokenStream2 {
        let args = &self.app.args;

        let backend_post_init = implementation.post_init(args, app, analysis);

        // init
        let init_fn = &app.init.body;
        let init_fn_ident = &app.init.ident;
        let task_inits_ident = if self.app.sub_apps.len() == 1 {
            format_ident!("TaskInits")
        } else {
            format_ident!("TaskInitsCore{}", app.core)
        };
        let task_inits_struct =
            generate_task_inits_struct(&task_inits_ident, &analysis.late_resource_tasks);

        // idle
        let idle_task_def = app
            .idle
            .as_ref()
            .map(|idle| idle.generate_task_def(app.shared.as_ref()));

        // user post_init function, called after the critical section
        let user_post_init_call = app.post_init.as_ref().map(|pi| {
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
        let task_defs = app
            .tasks
            .iter()
            .map(|task| task.generate_task_def(app.shared.as_ref()));
        let generated_task_init_calls: Vec<_> = app
            .tasks
            .iter()
            .filter_map(RticTask::task_init_call)
            .collect();
        let generated_task_inits_block = if generated_task_init_calls.is_empty() {
            quote! {}
        } else {
            quote! { unsafe {#(#generated_task_init_calls)*} }
        };

        let hw_task_bindings = app
            .tasks
            .iter()
            .filter_map(|t| t.generate_hw_task_to_irq_binding(implementation));

        // shared resources
        let shared = app.shared.as_ref();
        let shared_resources_def = shared.map(|shared| shared.generate_shared_resources_def());
        let shared_resources_handle = shared.map(SharedResources::name_uppercase);

        let resource_proxies = app
            .shared
            .as_ref()
            .map(|shared| shared.generate_resource_proxies(implementation, args, app));

        // local and shared resources initialization
        let user_task_inits_var = format_ident!("__task_inits");
        let user_task_inits_writes = if analysis.late_resource_tasks.is_empty() {
            quote! {}
        } else {
            generate_task_inits_write_calls(&analysis.late_resource_tasks, &user_task_inits_var)
        };
        let shared_resource_ty = shared.map(|s| &s.strct.ident);

        let init_system = if let Some(shared_resource_ty) = shared_resource_ty {
            quote! {
                let (__shared_resources, #user_task_inits_var) : (#shared_resource_ty, #task_inits_ident) = #init_fn_ident(); // call to init and get shared and local resources inits
                unsafe {#shared_resources_handle.write(__shared_resources);} // init shared resources
                #user_task_inits_writes
            }
        } else {
            quote! {
                let #user_task_inits_var : #task_inits_ident = #init_fn_ident(); // call to init and get shared and local resources inits
                #user_task_inits_writes
            }
        };

        // priority masks
        let priority_masks = implementation.generate_global_definitions(args, app, analysis);
        let entry_attrs = implementation.entry_attrs();
        let entry_name = implementation.entry_name(app.core);
        let enable_global_interrupts = implementation.generate_enable_global_interrupts();

        let interrupt_free = format_ident!("{}", INTERRUPT_FREE_FN);

        let core_type_def = generate_core_type(app.core);

        let core = app.core;

        let entry_start = self
            .injections
            .and_then(|inj| inj.entry_start.get(&core))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let before_init = self
            .injections
            .and_then(|inj| inj.before_init.get(&core))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let before_post_init = self
            .injections
            .and_then(|inj| inj.before_post_init.get(&core))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let before_idle = self
            .injections
            .and_then(|inj| inj.before_idle.get(&core))
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        let entry_of = format!(" # Entry of CORE {}", app.core);
        quote! {
            // define static mut shared resources
            #shared_resources_def
            // init task
            #init_fn
            // idle task
            #idle_task_def
            // define tasks
            #(#task_defs)*
            // bind hw tasks to interrupts
            #(#hw_task_bindings)*
            // proxies for accessing the shared resources
            #resource_proxies
            // unique type for the specific sub-app/core
            #core_type_def
            // Computed priority Masks
            #priority_masks
            /// Type holding one value per user task, returned from `#[init]`.
            /// Tasks are constructed inline or via user-defined helpers.
            #task_inits_struct

            #[doc = #entry_of]
            #(#entry_attrs)*
            #[unsafe(no_mangle)]
            fn #entry_name() -> ! {
                // injections at entry start (before the interrupt-free init block)
                #(#entry_start)*

                // Disable interrupts during initialization
                #interrupt_free(||{
                    #(#before_init)*

                    // user init code
                    #init_system

                    // init framework-generated tasks
                    #generated_task_inits_block

                    // injections before post_init
                    #(#before_post_init)*

                    // post initialization code
                    #backend_post_init
                });

                // injections before idle
                #(#before_idle)*

                // enable global interrupts (target specific)
                #enable_global_interrupts

                // user post_init function
                #user_post_init_call

                #call_idle_task
            }

        }
    }
}

fn generate_idle_call(idle: Option<&IdleTask>, wfi: Option<TokenStream2>) -> TokenStream2 {
    let Some(idle) = idle else {
        return quote! {
            loop {
                #wfi
            }
        };
    };

    let idle_ty = idle.name();
    let idle_instance_name = idle.name_uppercase();
    let write = if idle.args.init_generated {
        quote! { #idle_instance_name.write(#idle_ty); }
    } else {
        quote! {}
    };
    quote! {
        unsafe {
            #write
            #idle_instance_name.assume_init_mut().exec();
        }
    }
}

/// Generates a unique type for some core that is unsafe to create by the user.
/// I.e, it is used for internal purposes only, so the user shouldn't attempt to create it.
fn generate_core_type(core: u32) -> TokenStream2 {
    let core_ty = utils::core_type(core);
    let inner_core_ty = utils::core_type_inner(core);
    let mod_core_ty = utils::core_type_mod(core);
    let doc = format!("Unique type for core {core}");

    quote! {
        #[doc = #doc]
        pub use #mod_core_ty::#core_ty;
        mod #mod_core_ty {
            struct #inner_core_ty;
            pub struct #core_ty(#inner_core_ty);
            impl #core_ty {
                #[inline(always)]
                pub const unsafe fn new() -> Self {
                    #core_ty(#inner_core_ty)
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
