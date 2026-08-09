pub mod analyze;
mod codegen;
pub mod parse;

pub use crate::parse::App;
use crate::codegen::CodeGen;
pub use analyze::Analysis;
use proc_macro2::TokenStream;
use rticx_core::{InfoBus, RticPass};
use syn::ItemMod;

pub static INFO_APP: &str = "rticx_async_pass::App";
pub static INFO_ANALYSIS: &str = "rticx_async_pass::Analysis";

pub struct AsyncPass {
    backend: Box<dyn AsyncPassBackend>,
    info_bus: Option<InfoBus>,
}

impl AsyncPass {
    pub fn new<T: AsyncPassBackend + 'static>(backend: T) -> Self {
        Self {
            backend: Box::new(backend),
            info_bus: None,
        }
    }
}

impl RticPass for AsyncPass {
    fn subscribe(&mut self, info_bus: InfoBus) {
        let _ = self.info_bus.insert(info_bus.clone());
        self.backend.subscribe(info_bus);
    }

    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        let parsed = App::parse(&args, app_mod)?;
        let analysis = Analysis::run(&parsed)?;
        let code = CodeGen::new(parsed.clone(), analysis.clone(), self.backend.as_ref()).run();
        self.info_bus.as_ref().inspect(|b| {
            b.publish(INFO_APP, parsed)
                .unwrap_or_else(|_| panic!("no other crate is allowed to publish {INFO_APP}"));
            b.publish(INFO_ANALYSIS, analysis)
                .unwrap_or_else(|_| panic!("no other crate is allowed to publish {INFO_ANALYSIS}"))
        });
        Ok((args, code))
    }

    fn pass_name(&self) -> &str {
        "AsyncPass"
    }
}

pub trait AsyncPassBackend {
    fn queue_path(&self) -> syn::Path;

    fn generate_local_pend_fn(&self, core: u32, empty_body_fn: syn::ItemFn) -> syn::ItemFn;

    fn generate_cross_pend_fn(&self, core: u32, empty_body_fn: syn::ItemFn) -> Option<syn::ItemFn>;

    fn generate_wake_pend_fn(&self, core: u32, empty_body_fn: syn::ItemFn) -> syn::ItemFn {
        self.generate_local_pend_fn(core, empty_body_fn)
    }

    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        None
    }

    fn subscribe(&mut self, _info_bus: InfoBus) {}
}
