extern crate proc_macro;

use proc_macro::TokenStream;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;

use proc_macro2::{Ident, TokenStream as TokenStream2};
use syn::{parse_macro_input, ItemMod};

pub use common_internal::rtic_functions;
pub use common_internal::rtic_traits;

pub use analysis::{Analysis, SubAnalysis};
pub use backend::CorePassBackend;
pub use codegen::multibin;
use codegen::CodeGen;
pub use parser::ast::AppArgs;
pub use parser::{App, SubApp};

pub mod analysis;
mod backend;
pub mod codegen;
mod common_internal;
pub mod errors;
pub mod mock_backend;
pub mod parse_utils;

pub mod parser;

static DEFAULT_TASK_PRIORITY: AtomicU16 = AtomicU16::new(0);

/// A trait that allows defining a **Compilation Pass**.
///
/// A **Compilation Pass** can be thought of as a (partial) proc-macro that expands parts of the user RTIC application
/// once all the compilation passes provided using [RticMacroBuilder::bind_pre_core_pass] and
/// [RticMacroBuilder::bind_post_core_pass] are run. The resulting code should be comprised only of *init*, *idle* ,
/// *shared resources* and *tasks* (that may be bound to interrupts) that share those resources. The **Core Pass**
/// then will take over from there to generate all the necessary logic and expand the user application further to an
/// application understandable by the Rust compiler.
pub trait RticPass {
    /// Runs the (partial) proc-macro logic that allows extending the basic RTIC syntax
    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)>;

    /// Returns a human readable name/alias used to identify the pass. This identifier will show np in errors for example
    /// to help knowing exactly which compilation pass has failed in that case.
    fn pass_name(&self) -> &str;
}

/// This should be used to compose an **RTIC distribution**. In other words, it allows building the RTIC **app** macro
/// By providing the necessary low-level hardware bindings and binding additional **Compilation Passes**
/// in the case syntax extensions are desired.
pub struct RticMacroBuilder {
    core: Box<dyn CorePassBackend>,
    pre_std_passes: Vec<Box<dyn RticPass>>,
    post_std_passes: Vec<Box<dyn RticPass>>,
}

impl RticMacroBuilder {
    pub fn new<T: CorePassBackend + 'static>(core_impl: T) -> Self {
        Self {
            core: Box::new(core_impl),
            pre_std_passes: Vec::new(),
            post_std_passes: Vec::new(),
        }
    }

    /// Binds a **Compilation Pass** that will run before the **Core Pass**
    pub fn bind_pre_core_pass<P: RticPass + 'static>(&mut self, pass: P) -> &mut Self {
        self.pre_std_passes.push(Box::new(pass));
        self
    }

    /// Binds a **Compilation Pass** that will run after the **Core Pass**
    pub fn bind_post_core_pass<P: RticPass + 'static>(&mut self, pass: P) -> &mut Self {
        self.post_std_passes.push(Box::new(pass));
        self
    }

    /// Once the **CorePass** low level hardware bindings are provided, and a selection of
    /// **Compilation Passes** are bound too, use this method to run the **app** proc macro logic.
    ///
    /// Returns a `proc_macro::TokenStream` of the expanded user application. This is the entry
    /// point used by distribution proc-macros.
    pub fn build_rtic_macro(self, args: TokenStream, input: TokenStream) -> TokenStream {
        let args = TokenStream2::from(args);
        let app_mod = parse_macro_input!(input as ItemMod);
        self.build_rtic_macro2(args, app_mod).into()
    }

    /// Same as [build_rtic_macro] but operates on `proc_macro2` types.
    ///
    /// This method is exposed so that tests and downstream tooling can drive the core pipeline
    /// without needing a proc-macro context.
    pub fn build_rtic_macro2(self, args: TokenStream2, app_mod: ItemMod) -> TokenStream2 {
        // init statics
        DEFAULT_TASK_PRIORITY.store(self.core.default_task_priority(), Ordering::Relaxed);

        let mut args = args;
        let mut app_mod = app_mod;

        // First, run pre-core passes (in the order of their insertion)
        for pass in self.pre_std_passes {
            let (out_args, out_mod) = match pass.run_pass(args, app_mod) {
                Ok(out) => out,
                Err(e) => {
                    eprintln!(
                        "An error occurred during the `{}` compilation pass",
                        pass.pass_name()
                    );
                    return e.to_compile_error();
                }
            };
            app_mod = out_mod;
            args = out_args;
        }

        // Run post-core passes (in the order of their insertion) before the core pass takes over.
        for pass in self.post_std_passes {
            let (out_args, out_mod) = match pass.run_pass(args, app_mod) {
                Ok(out) => out,
                Err(e) => {
                    eprintln!(
                        "An error occurred during the `{}` compilation pass",
                        pass.pass_name()
                    );
                    return e.to_compile_error();
                }
            };
            app_mod = out_mod;
            args = out_args;
        }

        // parse user application comprised of init, idle, and other tasks and resources
        let mut parsed_app = match App::parse(args, app_mod) {
            Ok(parsed) => parsed,
            Err(e) => {
                eprintln!("An error occurred during the `core` compilation pass during the user code `parsing` phase.");
                return e.to_compile_error();
            }
        };

        // update resource ceilings and gather more information about the application
        let analysis = match Analysis::run(&mut parsed_app) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("An error occurred during the `core` compilation pass  during the user code `analysis` phase.");
                return e.to_compile_error();
            }
        };

        // Before starting code generation, ask distribution for further checks
        if let Err(e) = self.core.pre_codegen_validation(&parsed_app, &analysis) {
            return e.to_compile_error();
        }

        let code = CodeGen::new(self.core.as_ref(), &parsed_app, &analysis).run();

        #[cfg(feature = "debug_expand")]
        if let Ok(binary_name) = std::env::var("CARGO_BIN_NAME") {
            if let Ok(out) = project_root::get_project_root() {
                let _ = std::fs::create_dir_all(out.join("examples"));
                let _ = std::fs::write(
                    out.join(format!("examples/{binary_name}_expanded.rs")),
                    code.to_string().as_bytes(),
                );
            }
        }

        code
    }
}
