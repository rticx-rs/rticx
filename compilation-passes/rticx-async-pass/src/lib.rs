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
pub use rticx_sw_pass::SwPassBackend;
use std::cell::RefCell;
use std::collections::HashMap;
use syn::ItemMod;

pub static INFO_APP: &str = "rticx_async_pass::App";
pub static INFO_ANALYSIS: &str = "rticx_async_pass::Analysis";

pub struct AsyncPass {
    backend: Box<dyn AsyncPassBackend>,
    info_bus: Option<InfoBus>,
    /// Executor slot init statements, keyed by the core whose entry function
    /// they must be injected into.
    slot_init_stmts: RefCell<HashMap<u32, Vec<TokenStream>>>,
    has_tasks: RefCell<bool>,
}

impl AsyncPass {
    pub fn new<T: AsyncPassBackend + 'static>(backend: T) -> Self {
        Self {
            backend: Box::new(backend),
            info_bus: None,
            slot_init_stmts: RefCell::new(HashMap::new()),
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
        // Cross-core spawning is enforced exclusively by the runtime core check.
        // Refuse to compile when the application has cross-core tasks but the
        // distribution backend does not provide `current_core_id`.
        let has_cross_core_tasks = parsed.sub_apps.iter().any(|s| !s.mc_sw_tasks.is_empty());
        if has_cross_core_tasks && self.backend.current_core_id().is_none() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "this application has cross-core tasks, but the distribution backend does not implement `AsyncPassBackend::current_core_id`. Multicore distributions must provide a runtime core-id expression to enforce cross-core spawning.",
            ));
        }
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

    fn main_injection(&self, point: &MainInjectionPoint, core: u32) -> Option<TokenStream> {
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
            MainInjectionPoint::EntryStart => {
                if !*self.has_tasks.borrow() {
                    return None;
                }
                // Allocate this core's executor slots at the very start of its
                // entry function (before user init), then let the backend emit
                // an optional startup stack-overflow check right after.
                let stmts = self.slot_init_stmts.borrow();
                let core_stmts = stmts.get(&core).map(Vec::as_slice).unwrap_or(&[]);
                let check = self.backend.generate_stack_overflow_check(core);
                Some(quote::quote! {
                    #(#core_stmts)*
                    #check
                })
            }
            _ => None,
        }
    }
}

/// Backend interface for the async-tasks compilation pass.
///
/// Extends [`SwPassBackend`]: everything the software-tasks pass needs from a
/// distribution backend (queue path, pend-function bodies, custom interrupt
/// path, runtime core-id check) is inherited, so a single backend type can
/// serve both passes. This trait only adds the async-specific pieces.
pub trait AsyncPassBackend: SwPassBackend {
    /// Body of the interrupt-pending function used to wake an executor's dispatcher.
    /// For single core targets, keep the default implementation.
    /// For multicore, the`generate_wake_pend_fn` backend should implement a runtime core check to decide if this is a local or cross-core pend
    /// and perform the appropriate interrupt pending action.
    /// - local-core pend when the calling core is the same as `target_core`
    /// - cross-core pend when the calling core is different from `target_core`
    ///
    /// If a multicore distribution does not implemnent a multicore-aware waker pend backend then rticx_async::Channel
    /// will not work correctly across cores (only for core-local use)
    ///
    /// # Reference:
    /// - rticx-rp2040 distro
    fn generate_wake_pend_fn(&self, target_core: u32, empty_body_fn: syn::ItemFn) -> syn::ItemFn {
        self.generate_local_pend_fn(target_core, empty_body_fn)
    }

    /// Path to async runtime. This should be a re-exported path of `rticx_async`, e.g `rticx_cortex_m::export::rticx_async` (unless you want to point to a custom runtime with similar API)
    fn async_runtime_path(&self) -> syn::Path;

    /// Optional startup stack-overflow check emitted at the start of the `core`'s entry
    /// function, immediately after the executor slot allocations.
    ///
    /// At this point in the generated code the entry frame holds all of the
    /// core's future slots, so the current stack pointer can be compared
    /// against the stack bounds to detect (post-hoc) that the startup
    /// allocations have already overflowed the stack. No interrupt is unmasked
    /// yet at this point
    fn generate_stack_overflow_check(&self, _core: u32) -> Option<TokenStream> {
        None
    }
}
