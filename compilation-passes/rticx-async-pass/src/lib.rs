pub mod analyze;
mod codegen;
pub mod parse;

use crate::codegen::CodeGen;
pub use crate::parse::App;
pub use analyze::Analysis;
use proc_macro2::TokenStream;
use quote::format_ident;
use rticx_core::parse_utils::RticAttr;
use rticx_core::{InfoBus, MainInjectionPoint, RticPass};
use std::cell::RefCell;
use syn::ItemMod;

pub static INFO_APP: &str = "rticx_async_pass::App";
pub static INFO_ANALYSIS: &str = "rticx_async_pass::Analysis";

pub struct AsyncPass {
    backend: Box<dyn AsyncPassBackend>,
    info_bus: Option<InfoBus>,
    slot_init_stmts: RefCell<Vec<TokenStream>>,
    has_tasks: RefCell<bool>,
}

impl AsyncPass {
    pub fn new<T: AsyncPassBackend + 'static>(backend: T) -> Self {
        Self {
            backend: Box::new(backend),
            info_bus: None,
            slot_init_stmts: RefCell::new(Vec::new()),
            has_tasks: RefCell::new(false),
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
        let has_any = parsed
            .sub_apps
            .iter()
            .any(|s| !s.sw_tasks.is_empty() || !s.mc_sw_tasks.is_empty());
        *self.has_tasks.borrow_mut() = has_any;
        let code = CodeGen::new(
            parsed.clone(),
            analysis.clone(),
            self.backend.as_ref(),
            &self.slot_init_stmts,
        )
        .run();
        self.info_bus.as_ref().inspect(|b| {
            b.publish(INFO_APP, parsed)
                .unwrap_or_else(|_| panic!("no other crate is allowed to publish {INFO_APP}"));
            b.publish(INFO_ANALYSIS, analysis)
                .unwrap_or_else(|_| panic!("no other crate is allowed to publish {INFO_ANALYSIS}"))
        });
        // hand the app arguments back to the next pass without the ones we consumed
        let mut attr = RticAttr::parse_from_tokens(args.clone(), format_ident!("app"))?;
        attr.elements.remove("dispatchers");
        let args = attr.args_tokens();
        Ok((args, code))
    }

    fn pass_name(&self) -> &str {
        "AsyncPass"
    }

    fn main_injection(&self, point: &MainInjectionPoint) -> Option<TokenStream> {
        match point {
            MainInjectionPoint::BeforePostInit => {
                if *self.has_tasks.borrow() {
                    Some(quote::quote! {
                        unsafe { __rticx_async_system_initialized = true; }
                    })
                } else {
                    None
                }
            }
            MainInjectionPoint::BeforeIdle => {
                let stmts = self.slot_init_stmts.borrow();
                if stmts.is_empty() {
                    None
                } else {
                    let stmts = &*stmts;
                    Some(quote::quote! { #(#stmts)* })
                }
            }
            _ => None,
        }
    }
}

pub trait AsyncPassBackend {
    fn queue_path(&self) -> syn::Path;

    fn async_runtime_path(&self) -> syn::Path;

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
