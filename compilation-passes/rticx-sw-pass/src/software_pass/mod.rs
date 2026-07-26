pub mod analyze;
mod codegen;
pub mod parse;

use crate::parse::App;
use crate::software_pass::codegen::CodeGen;
use analyze::Analysis;
use proc_macro2::TokenStream;
use rticx_core::{InfoBus, RticPass};
use syn::ItemMod;

pub struct SoftwarePass {
    backend: Box<dyn SwPassBackend>,
    info_bus: Option<InfoBus>,
}

impl SoftwarePass {
    pub fn new<T: SwPassBackend + 'static>(backend: T) -> Self {
        Self {
            backend: Box::new(backend),
            info_bus: None,
        }
    }
}

impl RticPass for SoftwarePass {
    fn subscribe(&mut self, info_bus: InfoBus) {
        let _ = self.info_bus.insert(info_bus);
    }

    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        let parsed = App::parse(&args, app_mod)?;
        let analysis = Analysis::run(&parsed)?;
        let code = CodeGen::new(parsed.clone(), analysis.clone(), self.backend.as_ref()).run();
        // publish info
        self.info_bus.as_ref().inspect(|b| {
            b.publish("rticx_sw_pass::App", parsed)
                .expect("no other crate is allowed to publish `rticx_sw_pass::App`");
            b.publish("rticx_sw_pass::Analysis", analysis)
                .expect("no other crate is allowed to publish `rticx_sw_pass::Analysis`")
        });
        Ok((args, code))
    }

    fn pass_name(&self) -> &str {
        "SoftwareTasks"
    }
}

/// Interface for providing the hardware-specific backend needed by the
/// software-tasks compilation pass.
///
/// Implement this trait in your distribution's proc-macro crate and pass
/// it to [`SoftwarePass::new`] to enable `spawn` and `spawn_from` for
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
    /// The software pass generates an empty function for each core and
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
    /// The software pass generates an empty function for each target core
    /// that has cross-core spawners and passes it to this method.  The
    /// implementation must fill the body with code that signals the target
    /// core to run a software task that was spawned remotely.  The resulting
    /// function is called by `spawn_from()` at runtime.
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
}
