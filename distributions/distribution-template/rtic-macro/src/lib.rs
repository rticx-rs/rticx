// RTIC Distribution Porting Template -- Proc-macro backend
//
// This crate defines the `#[rtic::app]` attribute macro for your
// distribution.  It wires together:
//
//   1. **CorePassBackend** (`struct Backend`) -- the target-specific
//      code-generation hooks that the RTIC core pass calls during expansion.
//
//   2. **SwPassBackend** (`struct SwBackend`) -- the target-specific hooks
//      for the software-tasks compilation pass (only compiled when the
//      `swtasks` Cargo feature is active).
//
//   3. **Compilation passes** -- auto-assign and software-tasks are
//      registered via `RticMacroBuilder::bind_pre_core_pass`.
//
// Every method below is a **binding point** that you MUST fill when
// porting to real hardware.  The method stubs return no-op values so
// that the crate compiles, but the generated user application will not
// work until each binding is implemented.
//
// See The Wiki for the full porting checklist

use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};

use rtic_core::{Analysis, AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
#[cfg(feature = "swtasks")]
use rtic_sw_pass::{SoftwarePass, SwPassBackend};
use syn::{parse_quote, ItemFn};

extern crate proc_macro;

// ===========================================================================
// Constants
// ===========================================================================

/// Lowest logical priority value that the target hardware supports.
///
/// This is used as the fallback when the user omits a `priority` on a task
/// or when the idle task is created.
///
/// # Porting
///
/// * **Cortex-M** -- `1` (higher number = lower urgency).
/// * **RISC-V CLIC** -- `31` or the maximum level your CLIC exposes.
/// * **RISC-V mintthresh** -- `15` (16 levels, 0 = highest).
///
/// Reference: `cortex-m-rtic/rtic-macro/src/lib.rs`
///            `rtic-hippo/rtic-macro/src/lib.rs`
const MIN_TASK_PRIORITY: u16 = 0xff;

// ===========================================================================
// Distribution backend struct
// ===========================================================================

/// The core backend for this distribution which enables the "Tasks and Resources" model.
///
/// Rename this struct to match your distribution (e.g. `CortexMRtic`,
/// `Rp2040Rtic`, `HippoRtic`) and implement [`CorePassBackend`] for it.
///
/// Every method of [`CorePassBackend`] is documented below with the
/// contract from `rtic-core/src/backend.rs`
struct Backend;

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwBackend);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(Backend);

    // Passes execute in the order they are bound:
    //
    //   1. Auto-assign   -- resolves `core = N` for tasks that share resources (keep for multicore only)
    //   2. Software pass -- expands dispatchers, spawn, spawn_from
    //   3. Core pass     -- RTIC core: Hardware Tasks and Locat/Shared Resources, SRP ceiling analysis
    //
    // For a single-core MCU that does not need auto-assignment you can remove the next `bind_pre_core_pass` call.
    #[cfg(feature = "autoassign")]
    builder.bind_pre_core_pass(rtic_auto_assign::AutoAssignPass);
    #[cfg(feature = "swtasks")]
    builder.bind_pre_core_pass(sw_pass);

    builder.build_rtic_macro(args, input)
}

// ===========================================================================
// CorePassBackend -- each method below is a binding point
// ===========================================================================

impl CorePassBackend for Backend {
    /// Returns the default priority assigned to a task when the user omits
    /// the `priority = N` attribute.
    ///
    /// The returned value is also stored globally so that the idle task
    /// (which runs at the lowest priority) and any priority computation
    /// can reference it.
    ///
    /// # Porting
    ///
    /// Return the **lowest** (numerically largest on most architectures)
    /// priority your hardware supports.
    ///
    /// * Cortex-M BASEPRI: `0xff`
    /// * Cortex-M source-mask: `0b11`
    /// * RISC-V mintthresh: `15`
    /// * RISC-V CLIC: `31`
    ///
    /// Reference: `cortex-m-rtic` uses `1`. `rtic-hippo` uses `15`.
    fn default_task_priority(&self) -> u16 {
        MIN_TASK_PRIORITY
    }

    /// Code emitted **after** `#[init]` and all task `init()` functions,
    /// but **before** the idle loop begins.  Runs inside a critical
    /// section (interrupts disabled).
    ///
    /// This is the place to:
    /// * Set interrupt priorities via the NVIC (or equivalent controller).
    /// * Unmask interrupt lines used by hardware tasks and dispatchers.
    /// * On multicore targets: wake up and initialize secondary cores.
    ///
    /// # Contract
    ///
    /// Return `None` to emit nothing, or `Some(token_stream)` containing
    /// the target-specific initialization code.
    ///
    /// # Porting
    ///
    /// Iterate over `app_analysis.used_irqs` to configure every interrupt
    /// the application depends on.  For each IRQ you typically:
    /// 1. Set its priority (clamped to `MIN_TASK_PRIORITY`).
    /// 2. Unmask it (NVIC::unmask or equivalent).
    ///
    /// Access the PAC path via `app_args.pacs[app_info.core as usize]`.
    ///
    /// On single-core targets the `core` field of `app_info` is always `0`.
    ///
    /// Reference: `cortex-m-rtic` configures exceptions via SCB and external
    /// interrupts via NVIC. `rp2040-rtic` adds core-1 boot and FIFO setup.
    /// `rtic-hippo` uses `mintthresh::write`. `atalanta-rtic` uses CLIC.
    fn post_init(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        None
    }

    /// SRP-based resource locking.
    ///
    /// The core pass generates an incomplete `lock` function for each
    /// shared resource proxy.  This method must append the hardware-
    /// specific locking body to it.
    ///
    /// The incomplete function can be see by locating the `fn get_resource_proxy_lock_fn` in
    /// `rtic-core/src/common_internal/rtic_functions.rs`.
    ///
    /// For more details see the function documentation in the [CorePassBackend] trait definition
    ///
    /// Reference: `cortex-m-rtic` exports lock function and emits `rtic::export::lock(resource_ptr, CEILING as u8, PAC::NVIC_PRIO_BITS, f)`
    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        incomplete_lock_fn
    }

    /// Global definitions emitted into the crate root scope.
    ///
    /// Return a `TokenStream` of items (constants, `use` statements,
    /// helper functions) that the locking implementation needs to
    /// reference at the global scope.
    ///
    /// # Porting
    ///
    /// * **BASEPRI locks** -- typically need nothing here (return `None`).
    /// * **Source-mask locks** -- emit the `Mask<N>` struct, `create_mask`,
    ///   `compute_mask_chunks`, and the `__rtic_internal_MASKS` constant.
    /// * **Threshold locks** -- may emit a `use` for the threshold register.
    ///
    /// Reference: `cortex-m-rtic` (armv6m)
    fn generate_global_definitions(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        None
    }

    /// Wrapping code emitted around every task's `exec()` call.
    ///
    /// The generated interrupt handler calls `exec()` inside whatever
    /// token stream you return here.  This is used to save/restore
    /// state around task execution.
    ///
    /// # Porting
    ///
    /// * **BASEPRI** -- save BASEPRI before, restore after.
    /// * **Source-mask** -- no-op (lock primitive handles it).
    /// * **Threshold** -- save threshold before, restore after.
    ///
    /// Return `None` to skip wrapping entirely.
    ///
    /// Reference: `cortex-m-rtic` implements execution wrapping for non-armv6m targets
    fn wrap_task_execution(
        &self,
        _task_prio: u16,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        None
    }

    /// Name of the entry function for each core.
    ///
    /// # Contract
    /// * **Single-core**: always return `"main"`.
    /// * **Multicore**: return `"main"` for core 0, and a unique identifier (e.g. `core1_entry`) for others.
    ///
    /// References: 
    /// - `cortex-m-rtic` always returns `"main"`.
    /// - `rp2040-rtic` returns `"main"` for core 0 and `"core{N}_entry"` for core N > 0.
    fn entry_name(&self, _core: u32) -> Ident {
        format_ident!("main")
    }

    /// Custom body for the default idle loop.
    ///
    /// When the user does not define an `#[idle]` task, RTIC generates
    /// one with an infinite loop.  This method populates that loop.
    ///
    /// You can return `Some(quote! { wfi(); })` to have the CPU sleep between
    /// interrupts (saves power).  Return `None` for an empty busy loop.
    ///
    /// Reference: `cortex-m-rtic` emits `rtic::export::wfi()`.
    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        None
    }

    /// Body of the global critical-section function.
    ///
    /// RTIC generates a function like:
    ///
    /// ```ignore
    /// pub fn __rtic_critical_section<F, R>(f: F) -> R
    /// where F: FnOnce() -> R { /* YOU FILL THIS */ }
    /// ```
    ///
    /// # Contract
    /// * Do NOT change the function signature of `empty_body_fn`.
    /// * The function must re-enable interrupts when done.
    ///
    /// # Porting
    ///
    /// * **Cortex-M**: `cpsid i` / `cpsie i`, or `cortex_m::interrupt::free`.
    /// * **RISC-V**: disable/enable global interrupt via `mstatus.MIE`.
    ///
    /// Reference: `cortex-m-rtic` uses `core::arch::asm!("cpsid i")` /
    /// `core::arch::asm!("cpsie i")`.
    fn generate_interrupt_free_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let fn_body = parse_quote!({ 
            // TODO(port): disable interrupts here 
            let r = f();
            // TODO(port): re-enable interrupts here 
            r
         });
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    /// Validation hook called after parsing and analysis, but before
    /// code generation.
    ///
    /// Use this to reject user code that is invalid for your target.
    ///
    /// # Porting
    ///
    /// * **Cortex-M**: reject tasks bound to non-configurable exceptions
    ///   (`NonMaskableInt`, `HardFault`).
    /// * **Single-core**: reject `core = N` where N > 0.
    /// * **RISC-V**: reject interrupts that cannot be used as dispatchers.
    ///
    /// Return `Ok(())` to allow codegen, or `Err(syn::Error)` to abort.
    ///
    /// Reference: `cortex-m-rtic` checks against `NON_CONFIGURABLE_EXCEPTIONS`.
    fn pre_codegen_validation(
        &self,
        _app: &rtic_core::App,
        _analysis: &Analysis,
    ) -> syn::Result<()> {
        Ok(())
    }
}

// ===========================================================================
// SwPassBackend -- software-tasks backend (behind `swtasks` feature)
// ===========================================================================

#[cfg(feature = "swtasks")]
struct SwBackend;

#[cfg(feature = "swtasks")]
impl SwPassBackend for SwBackend {
    /// Body of the core-local interrupt-pending function.
    ///
    /// This function is called by `spawn()` to trigger the dispatcher
    /// interrupt that will run the software task on the local core.
    ///
    /// # Contract
    /// * The function takes a single argument `irq_nbr` (the interrupt
    ///   number of the dispatcher).
    /// * Write to the pending bit of the corresponding NVIC (or equivalent)
    ///   register to trigger the interrupt.
    ///
    /// # Porting
    ///
    /// * **Cortex-M**: write to NVIC ISPR register.
    /// * **RISC-V CLIC**: set the pending bit via `Clic::ip(irq).pend()`.
    /// * **RISC-V mintthresh**: use a software interrupt or ECLIC API.
    ///
    /// Reference: `cortex-m-rtic` uses `rtic::export::NVIC::pend(irq_nbr)`.
    fn generate_local_pend_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            // TODO(port): pend the dispatcher interrupt on this core.
            // Example for Cortex-M: rtic::export::NVIC::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// Body of the cross-core interrupt-pending function.
    ///
    /// This function is called by `spawn_from()` to signal another core
    /// to run a software task that was spawned remotely.
    ///
    /// # Contract
    /// * The function takes `irq_nbr` (dispatcher interrupt number) and
    ///   `core` (target core index).
    /// * Return `None` if your target is single-core (no cross-core
    ///   communication is needed).
    ///
    /// # Porting
    /// * **Single-core targets**: return `None`.  `spawn_from` will not
    ///   be available to user code.
    /// * **RP2040**: send the IRQ number through the SIO FIFO.
    /// * **Generic multicore**: use an IPI (inter-processor interrupt)
    ///   mechanism (e.g. mailbox, shared-memory + doorbell).
    ///
    /// Reference: `rp2040-rtic` writes `irq_nbr` to the SIO FIFO via
    /// `rtic::export::cross_core::pend_irq(irq_nbr.number())`.
    fn generate_cross_pend_fn(&self, _empty_body_fn: ItemFn) -> Option<ItemFn> {
        None
    }
}
