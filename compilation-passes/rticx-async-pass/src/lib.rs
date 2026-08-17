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

pub trait AsyncPassBackend {
    /// Path to the SPSC queue type used for ready queues and task inputs.
    ///
    /// The generated code uses this path as `#queue_path<T, N>` (type
    /// position) and `#queue_path::new()` (expression position).  The
    /// concrete type must support the same API as `rticx_spsc::Queue`:
    /// a const `new()` constructor, `split()` into producer/consumer halves,
    /// `enqueue` / `dequeue`, and `_unchecked` variants.
    ///
    /// Typical implementation for a distribution:
    /// ```ignore
    /// fn queue_path(&self) -> syn::Path {
    ///     parse_quote!(rticx_rp2040::export::Queue)
    /// }
    /// ```
    fn queue_path(&self) -> syn::Path;

    /// Body of the core-local interrupt-pending function.
    ///
    /// The async pass generates an empty function for each core and
    /// passes it to this method.  The implementation must fill the body
    /// with code that triggers (pends) the dispatcher interrupt on the
    /// local core. The resulting function is called by `spawn()` at
    /// runtime.
    ///
    /// # Contract
    /// * The function is generated per core; `core` is the core index.
    /// * The generated function takes a single argument `irq_nbr` whose
    ///   concrete type is the interrupt type for that core (see
    ///   [`custom_interrupt_path`](Self::custom_interrupt_path)).
    /// * Write to the pending bit of the corresponding NVIC (or equivalent)
    ///   register to trigger the interrupt.
    /// * Do NOT change the function signature.
    ///
    /// # Porting
    ///
    /// * **Cortex-M**: write to NVIC ISPR register.
    /// * **RISC-V CLIC**: set the pending bit via `Clic::ip(irq).pend()`.
    /// * **RISC-V mintthresh**: use a software interrupt or ECLIC API.
    fn generate_local_pend_fn(&self, core: u32, empty_body_fn: syn::ItemFn) -> syn::ItemFn;

    /// Body of the cross-core interrupt-pending function.
    ///
    /// The software pass generates an empty function for each target core
    /// that has cross-core spawners and passes it to this method.  The
    /// implementation must fill the body with code that signals the target
    /// core to run an async software task that was spawned remotely.  The resulting
    /// function is called by `spawn_from()` at runtime.
    ///
    /// The cross-pend function returns a Result<(),()>, Ok(()) if cross-core interrupt
    /// was successfully called, or Err(()) if pending failed for any reason (E.g FIFO full)
    /// ```ignore
    /// pub fn __rticx_internal_cross_pend(irq_nbr: #interrupt_type_path) -> Result<(), ()> { /* you code here */}
    /// ```
    ///
    /// # Contract
    /// * `core` is the *target* core index (the core that owns the task).
    /// * The generated function takes a single argument `irq_nbr` whose
    ///   concrete type is the interrupt type for the target core.
    /// * Return `None` if your target is single-core (no cross-core
    ///   communication is needed).  `spawn_from` will not be available
    ///   to user code.
    /// * Do NOT change the function signature.
    ///
    /// # Porting
    ///
    /// * **Single-core targets**: return `None`.
    /// * **RP2040**: send the IRQ number through the SIO FIFO.
    /// * **Generic multicore**: use an IPI (inter-processor interrupt)
    ///   mechanism (e.g. mailbox, shared-memory + doorbell).
    fn generate_cross_pend_fn(&self, core: u32, empty_body_fn: syn::ItemFn) -> Option<syn::ItemFn>;

    /// Custom path to the interrupt type used for dispatchers on `core`.
    ///
    /// The returned path must name a **type** whose enum variants or
    /// associated constants match the dispatcher names listed in
    /// `dispatchers = [...]`.  Generated code uses it both for the pend
    /// function signature (`fn(irq_nbr: #ty)`) and at spawn call sites
    /// (`#ty::IRQ0`).
    ///
    /// Return `None` to use the default path `pac[core]::Interrupt`.
    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        None
    }

    /// Subscribe to info_bus
    /// This method is guaranteed to be called before any other methods in this trait.
    fn subscribe(&mut self, _info_bus: InfoBus) {}

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
