use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, ToTokens};
use task_init::{generate_late_init_tasks_struct, generate_late_tasks_init_calls};

use crate::analysis::Analysis;
use crate::multibin::multibin_cfg_core;
use crate::parser::ast::{RticTask, SharedResources};
use crate::parser::{ast::IdleTask, App};
use crate::rtic_functions::{
    generate_task_traits_check_functions, get_interrupt_free_fn, INTERRUPT_FREE_FN,
};
use crate::rtic_traits::get_rtic_traits_mod;
use crate::CorePassBackend;

pub mod hw_task;
pub use utils::multibin;
mod shared_resources;
mod task_init;
mod utils;

pub struct CodeGen<'a> {
    app: &'a App,
    analysis: &'a Analysis,
    implementation: &'a dyn CorePassBackend,
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
        }
    }

    pub fn run(&self) -> TokenStream2 {
        let app = self.app;
        let implementation = self.implementation;

        #[cfg(feature = "multibin")]
        let use_multibin_shared = {
            let multibin_shared_path = self.implementation.multibin_shared_macro_path();
            Some(quote!(use #multibin_shared_path as multibin_shared;))
        };
        #[cfg(not(feature = "multibin"))]
        let use_multibin_shared: Option<TokenStream2> = None;

        let app_mod = &app.app_name;
        let peripheral_crate = generate_use_pac_statement(app);
        let user_includes = &app.user_includes;
        let user_code = &app.other_code;
        let interrupt_free_fn = get_interrupt_free_fn(implementation);

        // traits
        let rtic_traits_mod = get_rtic_traits_mod();

        // sub_apps
        let sub_apps = self.generate_sub_apps();

        // task trait checks
        let task_trait_check_functions = generate_task_traits_check_functions(self.analysis);

        quote! {
            pub mod #app_mod {
                /// Include peripheral crate(s) that defines the vector table
                #peripheral_crate

                // if multibin feature is enabled, add the this use statement
                #use_multibin_shared

                // ================================== user includes ====================================
                #(#user_includes)*
                // ==================================== rtic traits ====================================
                #rtic_traits_mod
                // ================================== rtic functions ===================================
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
            let cfg_core = multibin::multibin_cfg_core(app.core);
            let post_init = implementation.post_init(args, app, analysis);

            // init
            let def_init_task = &app.init.body;
            let init_task = &app.init.ident;
            let late_init_struct = generate_late_init_tasks_struct(&analysis.late_resource_tasks);

            // idle
            let def_idle_task = app.idle.as_ref().map(|idle| {
                let idle_task = idle.generate_task_def(app.shared.as_ref());
                Some(idle_task)
            });

            let call_idle_task =
                generate_idle_call(app.idle.as_ref(), implementation.populate_idle_loop());

            // tasks
            let tasks_def = app
                .tasks
                .iter()
                .map(|task| task.generate_task_def(app.shared.as_ref()));
            let task_init_calls = app.tasks.iter().filter_map(RticTask::task_init_call);

            let hw_tasks_binds = app
                .tasks
                .iter()
                .filter_map(|t| t.generate_hw_task_to_irq_binding(implementation));

            // shared resources
            let shared = app.shared.as_ref();
            let def_shared = shared.map(|shared| shared.generate_shared_resources_def());
            let shared_resources_handle = shared.map(SharedResources::name_uppercase);
            let shared_resources_handle = shared_resources_handle.iter();
            let resource_proxies = app
                .shared
                .as_ref()
                .map(|shared| shared.generate_resource_proxies(implementation, args, app));

            // local and shared resources initialization
            let init_system = if let Some(s) = late_init_struct.as_ref() {
                let tasks_initializer = format_ident!("__late_task_inits");
                let user_task_late_inits = generate_late_tasks_init_calls(
                    &analysis.late_resource_tasks,
                    &tasks_initializer,
                );
                let task_inits_ty = &s.ident;
                let shared_resource_ty = shared
                    .map(|s| s.strct.ident.to_token_stream())
                    .unwrap_or(quote!("()"));
                quote! {
                    let (__shared_resources, #tasks_initializer) : (#shared_resource_ty, #task_inits_ty) = #init_task(); // call to init and get shared and local resources inits
                    #(unsafe {#shared_resources_handle.write(__shared_resources);})* // init shared resources
                    #user_task_late_inits
                }
            } else {
                quote! {
                    let shared_resources = #init_task();  // call to init and get shared resources init
                    #(unsafe {#shared_resources_handle.write(shared_resources);})* // init shared resources
                }
            };

            // priority masks
            let priority_masks = implementation.generate_global_definitions(args, app, analysis);
            let entry_attrs = implementation.entry_attrs();
            let entry_name = implementation.entry_name(app.core);

            let interrupt_free = format_ident!("{}", INTERRUPT_FREE_FN);

            let def_core_type = generate_core_type(app.core);

            let doc = format!(" # CORE {}", app.core);
            quote! {
                #[doc = #doc]
                // define static mut shared resources
                #def_shared
                // init task
                #cfg_core
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
                /// Type representing tasks that need explicit user initialization
                #late_init_struct

                #[doc = r" Entry of "]
                #[doc = #doc]
                #cfg_core
                #(#entry_attrs)*
                #[no_mangle]
                fn #entry_name() -> ! {
                    // Disable interrupts during initialization
                    #interrupt_free(||{
                        // user init code
                        #init_system

                        // init tasks
                        unsafe {#(#task_init_calls)*}

                        // post initialization code
                        #post_init
                    });

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
        if !idle.user_initializable {
            quote! {
                unsafe {
                    #idle_instance_name.write(#idle_ty::init(()));
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

/// This will generate the `user path::to::pac` statement. The output varies based on what features the distribution enables:
///
/// 1) If both `multipac` and `multibin` features are enabled, and the user provides a list of paths to PACs (i.e #app(device = [ path1, path2, ..])) the following will be generated
/// ```
/// #[cfg(core = '0')]
/// use path1 as _;
///
/// #[cfg(core = '1')]
/// use path2 as _;
/// ```
///
/// 2) If only `multipac` feature is enabled, and the user provides a list of paths to PACs (i.e #app(device = [ path1, path2, ..])) the following will be generated
/// ```
/// use path1 as _;
/// use path2 as _;
/// ```
///
/// 3) If neither `multipac`, nor `multibin` features are enabled, or if the user provides a single path to PACs (i.e #app(device = path::to::pac ) the following will be generated
/// ```
/// use  path::to::pac as _;
/// ```
fn generate_use_pac_statement(app: &App) -> TokenStream2 {
    if cfg!(feature = "multipac") && app.args.pacs.len() != 1 {
        if cfg!(feature = "multibin") {
            let iter = app.args.pacs.iter().enumerate().map(|(core, pac)| {
                let cfg_core = multibin_cfg_core(core as u32);
                quote! {
                 #cfg_core
                 use #pac as _;
                }
            });
            quote! {
                #(#iter)*
            }
        } else {
            let pacs = &app.args.pacs;
            quote! {
                use #(#pacs)* as _;
            }
        }
    } else {
        let path_to_pac = &app.args.pacs[0];
        quote! {
            use #path_to_pac as _;
        }
    }
}
