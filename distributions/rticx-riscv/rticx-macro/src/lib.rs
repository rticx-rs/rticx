use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};

use rticx_core::{Analysis, App, AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
#[cfg(feature = "swtasks")]
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
#[cfg(feature = "swtasks")]
use syn::Path;
use syn::{ItemFn, parse_quote};

extern crate proc_macro;

// ---- Compile-time gate: exactly one target feature must be selected ----------

#[cfg(not(any(feature = "slic", feature = "esp32c3", feature = "esp32c6")))]
compile_error!(
    "rticx-riscv-macro: no target feature selected. \
     Enable exactly one of: `slic`, `esp32c3`, `esp32c6`."
);

#[cfg(all(feature = "slic", feature = "esp32c3"))]
compile_error!("rticx-riscv-macro: `slic` and `esp32c3` are mutually exclusive");
#[cfg(all(feature = "slic", feature = "esp32c6"))]
compile_error!("rticx-riscv-macro: `slic` and `esp32c6` are mutually exclusive");
#[cfg(all(feature = "esp32c3", feature = "esp32c6"))]
compile_error!("rticx-riscv-macro: `esp32c3` and `esp32c6` are mutually exclusive");

const MIN_TASK_PRIORITY: u16 = 1;

// ============================================================================
// Entry point – dispatches to the selected backend
// ============================================================================

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(BackendImpl);
    #[cfg(feature = "swtasks")]
    builder.bind_pre_core_pass(sw_pass);
    builder.build_rtic_macro(args, input)
}

// ============================================================================
// Single backend struct that dispatches through cfg_attr / cfg blocks at
// method level.  This avoids duplicating the boilerplate while keeping
// the three backends' logic clearly separated.
// If this grows too much in the future, backend trait implementation should be splitted
// ============================================================================

struct BackendImpl;

impl CorePassBackend for BackendImpl {
    fn default_task_priority(&self) -> u16 {
        MIN_TASK_PRIORITY
    }

    // ---- post_init: enable & prioritise every interrupt used by the app ------
    //
    // For the SLIC backend this calls `riscv_slic::set_priority()` for each
    // dispatcher and hardware-task interrupt (the SLIC manages the underlying
    // interrupt controller).
    //
    // For the ESP32 backends this calls the target-specific `enable()` helper
    // which maps the peripheral interrupt to a CPU interrupt, sets its
    // priority and unmasks it.
    fn post_init(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        let mut stmts: Vec<TokenStream2> = Vec::new();

        #[cfg(feature = "slic")]
        {
            // SLIC: set the priority of every interrupt in use.  Dispatchers
            // are also SLIC interrupts (`SoftwareInterruptN`) and get their
            // priority set here.  Hardware task interrupts are PAC interrupts
            // routed through the SLIC.
            let set_prio = app_analysis.used_irqs.iter().map(|(irq_name, priority)| {
                quote! {
                    rticx_riscv::export::set_priority(
                        slic::SoftwareInterrupt::#irq_name,
                        #priority as u8,
                    );
                }
            });
            stmts.extend(set_prio);
        }

        // ESP32-C3: map each interrupt to a CPU interrupt and enable it.
        // CPU interrupt IDs 0..15 are reserved on ESP32-C3, so the first
        // available ID is 16.  We assign IDs sequentially to each interrupt
        // in the order returned by `used_irqs`.

        // ESP32-C6: same logic as C3 but uses the PLIC_MX interrupt matrix.
        // esp-hal reserves CPU interrupt IDs 1..19, so external interrupts
        // start at 20.  We assign from that base sequentially.
        #[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
        {
            let cpu_int_start: u8 = if cfg!(feature = "esp32c6") { 20 } else { 16 };
            let max_prio: usize = 15;
            let min_prio: usize = 1;

            let enable = app_analysis
                .used_irqs
                .iter()
                .enumerate()
                .map(|(idx, (irq_name, priority))| {
                    let cpu_int_id = cpu_int_start + idx as u8;
                    let es_max = format!(
                        "Maximum priority used by interrupt vector '{irq_name}' is more than supported by hardware"
                    );
                    let es_min = format!(
                        "Priority {priority} used by interrupt vector '{irq_name}' is less than supported by hardware"
                    );
                    quote! {
                        const _: () = if (#max_prio) <= #priority as usize {
                            ::core::panic!(#es_max);
                        };
                        const _: () = if (#min_prio) > #priority as usize {
                            ::core::panic!(#es_min);
                        };
                        rticx_riscv::export::enable(
                            rticx_riscv::export::Interrupt::#irq_name,
                            #priority as u8,
                            #cpu_int_id,
                        );
                    }
                });
            stmts.extend(enable);
        }

        if stmts.is_empty() {
            None
        } else {
            Some(quote! { #(#stmts)* })
        }
    }

    // ---- idle loop: wfi on all RISC-V targets -------------------------------
    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        Some(quote! { unsafe { core::arch::asm!("wfi"); } })
    }

    // ---- global critical section (interrupt disable/enable) ------------------
    //
    // SLIC and ESP32 targets all use standard RISC-V `mstatus.MIE` to
    // disable/enable interrupts.  The upstream ESP32 exports re-export
    // `riscv::interrupt` for this purpose.
    fn generate_interrupt_free_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let fn_body = parse_quote!({
            unsafe {
                riscv::interrupt::disable();
            }
            let r = f();
            unsafe {
                riscv::interrupt::enable();
            }
            r
        });
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    // ---- global definitions emitted at crate root ----------------------------
    //
    // For the SLIC backend we emit a `SoftwareInterrupt` enum whose variants
    // match the dispatcher interrupt names.  This is required because the
    // sw-pass generates `#[task(binds = SoftwareInterruptN)]` entries that
    // reference this type.
    //
    // The user must still call `riscv_slic::codegen!` in their crate to
    // generate the actual SLIC interrupt vector and software interrupt
    // handlers.  The enum emitted here is a *compile-time placeholder* that
    // satisfies the type system; the `codegen!` macro generates the real
    // definitions at link time.
    fn generate_global_definitions(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        #[cfg(feature = "slic")]
        {
            // The dispatcher interrupt names are available from the sw-pass
            // via the `dispatchers` attribute on `#[app]`.  However they are
            // not directly accessible here through `SubAnalysis`.  We emit a
            // placeholder type; the real enum is produced by `codegen!`.
            Some(quote! {
                /// SLIC software interrupt enum placeholder.
                ///
                /// The actual enum and interrupt vector are generated by
                /// `riscv_slic::codegen!` invoked in the user's crate.
                /// This type is emitted only to satisfy the sw-pass generated
                /// dispatcher task signatures.
                pub mod slic_dispatchers {
                    /// Placeholder – replaced at link time by `codegen!`.
                    pub enum SoftwareInterrupt {}
                }
            })
        }

        // FIXME: implement proper codegen! emision when cross-pass info sharing is supported
        // TODO: update readme when this is fixed.
        //
        // The SLIC requires us to call to the [`riscv_rtic::codegen`] macro to generate
        // the appropriate SLIC structure, interrupt enumerations, etc.
        // let mut stmts = vec![];

        // let hw_slice: Vec<_> = app
        //     .hardware_tasks
        //     .values()
        //     .map(|task| &task.args.binds)
        //     .collect();
        // let sw_slice: Vec<_> = app.args.dispatchers.keys().collect();

        // let swi_slice: Vec<_> = hw_slice.iter().chain(sw_slice.iter()).collect();

        // let device = &app.args.device;

        // stmts.push(quote!(
        //     use rtic::export::riscv_slic;
        // ));
        // let slic = quote! {rtic::export::riscv_slic};

        // match () {
        //     #[cfg(feature = "riscv-clint")]
        //     () => {
        //         let hart_id = &app.args.backend.as_ref().unwrap().hart_id;
        //         stmts.push(quote!(rtic::export::codegen!(slic = #slic, pac = #device, swi = [#(#swi_slice,)*], backend = [hart_id = #hart_id]);));
        //     }
        //     #[cfg(feature = "riscv-mecall")]
        //     () => {
        //         stmts.push(quote!(rtic::export::codegen!(slic = #slic, pac = #device, swi = [#(#swi_slice,)*]);));
        //     }
        // }

        // stmts

        #[cfg(not(feature = "slic"))]
        None
    }

    // ---- SRP resource locking ------------------------------------------------
    //
    // All three backends use threshold-based locking.  The export module
    // exposes a target-specific `lock(ptr, ceiling, f)` function that raises
    // the interrupt priority ceiling, calls `f`, and restores the old ceiling.
    //
    // NOTE: the incomplete_lock_fn body already contains:
    //   `const CEILING: u16 = N; let task_priority = self.task_priority;`
    // and the backend must fill in the rest.
    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        let lock_impl: syn::Block = parse_quote!({
            { unsafe { rticx_riscv::export::lock(resource_ptr, CEILING as u8, f) } }
        });
        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, _core: u32) -> syn::Ident {
        format_ident!("main")
    }

    // ---- task execution wrapping: threshold save/restore ---------------------
    //
    // For the ESP32 backends the `run(prio, f)` function saves the current
    // `cpu_int_thresh`/`mxint_thresh` value, calls `f`, then restores it.
    // For the SLIC backend `riscv_slic::run(prio, f)` does the same.
    fn wrap_task_execution(
        &self,
        task_prio: u16,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        Some(quote! {
            rticx_riscv::export::run(#task_prio as u8, || { #dispatch_task_call });
        })
    }

    // FIXME: improve this implementation when cross-pass info sharing is supported
    // ---- validation: dispatcher names for ESP targets ------------------------
    //
    // ESP32-C3 and ESP32-C6 only support `FROM_CPU_INTR{0..3}` as software
    // interrupt dispatchers.  The upstream RTIC binding enforces this in
    // `architecture_specific_analysis`.
    //
    // For the SLIC backend all interrupt names are valid because the SLIC
    // controller can route any interrupt.
    fn pre_codegen_validation(&self, _app: &App, _analysis: &Analysis) -> syn::Result<()> {
        // ESP32-C3/C6: validate dispatcher names against the supported set
        #[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
        {
            // The dispatcher names come from the sw-pass parsed `dispatchers`
            // attribute. In the new rticx-core architecture they are not
            // directly accessible from `App`.  Validation is performed via
            // the `used_irqs` list which includes both dispatcher and
            // hardware-task interrupts.  We check that every interrupt
            // in `used_irqs` is a valid dispatcher name.
            for (irq_name, _priority) in &_analysis.sub_analysis[0].used_irqs {
                let name = irq_name.to_string();
                match &*name {
                    "FROM_CPU_INTR0" | "FROM_CPU_INTR1" | "FROM_CPU_INTR2" | "FROM_CPU_INTR3" => {}
                    _ => {
                        return Err(syn::Error::new(
                            irq_name.span(),
                            "Only FROM_CPU_INTR{0..3} are supported as \
                             interrupt sources on ESP32 targets.  Use these \
                             as dispatchers: `#[app(dispatchers = \
                             [FROM_CPU_INTR0, ...])]`.",
                        ));
                    }
                }
            }
        }

        Ok(())
    }
}

// ============================================================================
// Software-tasks pass backend
// ============================================================================

#[cfg(feature = "swtasks")]
struct SwBackendImpl;

#[cfg(feature = "swtasks")]
impl SwPassBackend for SwBackendImpl {
    /// Path to the SPSC queue type re-exported by this distribution.
    fn queue_path(&self) -> Path {
        parse_quote!(rticx_riscv::export::Queue)
    }

    /// Core-local interrupt pending: pends a dispatcher interrupt on the
    /// local core.
    ///
    /// For all three backends the `pend` function is re-exported by the
    /// `export` module.
    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            rticx_riscv::export::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// Single-core targets: no cross-core pending is available.
    fn generate_cross_pend_fn(&self, _core: u32, _empty_body_fn: ItemFn) -> Option<ItemFn> {
        None
    }

    /// Custom interrupt type path used for dispatcher interrupt enums.
    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        None
    }
}
