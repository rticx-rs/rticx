pub mod analyze;
mod codegen;
pub mod parse;

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

/// Interface for providing the hardware-specific backend needed by the
/// software-tasks compilation pass.
///
/// Implement this trait in your distribution's proc-macro crate and pass
/// it to [`SoftwarePass::new`] to enable `spawn` and `spawn_from` for
/// software tasks.
pub trait SwPassBackend {
    /// Body of the core-local interrupt-pending function.
    ///
    /// The software pass generates an empty function and passes it to
    /// this method.  The implementation must fill the body with code
    /// that triggers (pends) the dispatcher interrupt on the local core.
    /// The resulting function is called by `spawn()` at runtime.
    ///
    /// # Contract
    /// * The function takes a single argument `irq_nbr` (the interrupt
    ///   number of the dispatcher).
    /// * Write to the pending bit of the corresponding NVIC (or equivalent)
    ///   register to trigger the interrupt.
    /// * Do NOT change the function signature.
    ///
    /// # Porting
    ///
    /// * **Cortex-M**: write to NVIC ISPR register.
    /// * **RISC-V CLIC**: set the pending bit via `Clic::ip(irq).pend()`.
    /// * **RISC-V mintthresh**: use a software interrupt or ECLIC API.
    ///
    /// Reference: `cortex-m-rtic` uses `rtic::export::NVIC::pend(irq_nbr)`.
    /// `rp2040-rtic` uses the same. `atalanta-rtic` uses
    /// `rtic::export::pend(irq_nbr)`.
    fn generate_local_pend_fn(&self, empty_body_fn: syn::ItemFn) -> syn::ItemFn;

    /// Body of the cross-core interrupt-pending function.
    ///
    /// The software pass generates an empty function and passes it to
    /// this method.  The implementation must fill the body with code
    /// that signals another core to run a software task that was spawned
    /// remotely.  The resulting function is called by `spawn_from()` at
    /// runtime.
    ///
    /// # Contract
    /// * The function takes `irq_nbr` (dispatcher interrupt number) and
    ///   `core` (target core index).
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
    ///
    /// Reference: `rp2040-rtic` writes `irq_nbr` to the SIO FIFO via
    /// `rtic::export::cross_core::pend_irq(irq_nbr.number())`.
    fn generate_cross_pend_fn(&self, empty_body_fn: syn::ItemFn) -> Option<syn::ItemFn>;

    /// Custom path to the `Interrupt` enum type used for dispatchers.
    ///
    /// Override this if your PAC's interrupt enum is not at the default
    /// path `pac[core]::interrupt::Interrupt`.
    ///
    /// Return `None` to use the default path.
    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        None
    }
}
