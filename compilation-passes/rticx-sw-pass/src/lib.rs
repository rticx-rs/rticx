pub mod analyze;
mod codegen;
/// Shared infrastructure for the software-task passes.  
pub mod common;
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

pub static INFO_APP: &str = "rticx_sw_pass::App";
pub static INFO_ANALYSIS: &str = "rticx_sw_pass::Analysis";

pub struct SoftwarePass {
    backend: Box<dyn SwPassBackend>,
    info_bus: Option<InfoBus>,
    has_tasks: RefCell<bool>,
}

impl SoftwarePass {
    pub fn new<T: SwPassBackend + 'static>(backend: T) -> Self {
        Self {
            backend: Box::new(backend),
            info_bus: None,
            has_tasks: RefCell::new(false),
        }
    }
}

impl RticPass for SoftwarePass {
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
                "this application has cross-core tasks (`spawn_by != core`), but the distribution backend does not implement `SwPassBackend::current_core_id`. Multicore distributions must provide a runtime core-id expression to enforce cross-core spawning.",
            ));
        }
        let analysis = Analysis::run(&parsed)?;
        let has_any = parsed
            .sub_apps
            .iter()
            .any(|s| !s.sw_tasks.is_empty() || !s.mc_sw_tasks.is_empty());
        *self.has_tasks.borrow_mut() = has_any;
        let code = CodeGen::new(parsed.clone(), analysis.clone(), self.backend.as_ref()).run();
        // publish info
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
        "SoftwareTasks"
    }

    fn main_injection(&self, point: &MainInjectionPoint, _core: u32) -> Option<TokenStream> {
        if matches!(point, MainInjectionPoint::BeforePostInit) && *self.has_tasks.borrow() {
            Some(quote::quote! {
                unsafe { __rticx_sw_system_initialized = true; }
            })
        } else {
            None
        }
    }
}

/// Interface for providing the hardware-specific backend needed by the
/// software-tasks compilation pass.
///
/// Implement this trait in your distribution's proc-macro crate and pass
/// it to [`SoftwarePass::new`] to enable `spawn` and `cross_spawn` for
/// software tasks.
pub trait SwPassBackend {
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
    /// The pass generates an empty function for each core and
    /// passes it to this method.  The implementation must fill the body
    /// with code that triggers (pends) the dispatcher interrupt on the
    /// local core.  The resulting function is called by `spawn()` at
    /// runtime.
    ///
    /// # Contract
    /// * The function is generated per core; `core` is the core index.
    /// * The generated function takes a single argument `irq_nbr` whose
    ///   concrete type is the interrupt type for that core (see
    ///   [`custom_interrupt_path`](SwPassBackend::custom_interrupt_path)).
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
    /// The pass generates an empty function for each target core
    /// that has cross-core spawners and passes it to this method.  The
    /// implementation must fill the body with code that signals the target
    /// core to run a software task that was spawned remotely.  The resulting
    /// function is called by `cross_spawn()` at runtime.
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
    ///   communication is needed).  `cross_spawn` will not be available
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

    /// Expression yielding the numeric id (`u32`) of the core this code is
    /// currently executing on.
    ///
    /// The generated `spawn`/`cross_spawn` functions start with a runtime check:
    /// `if <expression> != <expected core> { return Err(input) }`.  The expected
    /// core is the task's own `core` for `spawn`, and its `spawn_by` for
    /// `cross_spawn`.
    ///
    /// The expression must side-effect-free read of the actual hardware state (e.g. the `cpuid` register on the RP2040).
    fn current_core_id(&self) -> Option<syn::Expr> {
        None
    }

    /// Subscribe to info_bus
    /// This method is guaranteed to be called before any other methods in this trait.
    fn subscribe(&mut self, _info_bus: InfoBus) {}
}
