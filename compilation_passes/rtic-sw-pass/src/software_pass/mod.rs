mod analyze;
mod codegen;
pub(crate) mod parse;

use crate::parse::App;
use crate::software_pass::codegen::CodeGen;
use analyze::Analysis;
use proc_macro2::TokenStream;
use rtic_core::RticPass;
use syn::ItemMod;

pub struct SoftwarePass {
    backend: Box<dyn SwPassBackend>,
}

impl SoftwarePass {
    pub fn new<T: SwPassBackend + 'static>(backend: T) -> Self {
        Self {
            backend: Box::new(backend),
        }
    }
}

impl RticPass for SoftwarePass {
    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        let parsed = App::parse(&args, app_mod)?;
        let analysis = Analysis::run(&parsed)?;
        let code = CodeGen::new(parsed, analysis, self.backend.as_ref()).run();
        Ok((args, code))
    }

    fn pass_name(&self) -> &str {
        "SoftwareTasks"
    }
}

/// Interface for providing the hardware specific details (i.e backend) needed by the software pass
pub trait SwPassBackend {
    /// Implementation of this trait method must populate the body of `empty_body_fn' with the low-level implementation
    /// to generate the core-local interrupt pending function.
    /// The resulting interrupt pending function will be used for implementing the `spawn` method of core-local software tasks
    /// You can use [eprintln()] to see the `empty_body_fn` function signature
    fn generate_local_pend_fn(&self, empty_body_fn: syn::ItemFn) -> syn::ItemFn;

    /// Implementation of this trait method must populate the body of `empty_body_fn' with the low-level implementation
    /// to generate the cross-core interrupt pending function.
    /// The resulting interrupt pending function will be used for implementing the `spawn` method of cross-core software tasks (software
    /// tasks assigned to run on a specific core, but are "spawned by" another core)
    /// You can use [eprintln()] to see the `empty_body_fn` function signature
    fn generate_cross_pend_fn(&self, empty_body_fn: syn::ItemFn) -> Option<syn::ItemFn>;

    /// Use this method to provide a custom path to the Interrupt type. This type must list all the interrupt names usable as dispatchers
    /// If this method is not implemented, pac[core]::interrupt::Interrupt type will be used by default
    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        None
    }
}
