use std::cell::OnceCell;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};

use rticx_core::{
    Analysis, App, AppArgs, CorePassBackend, InfoBus, RticMacroBuilder, SubAnalysis, SubApp,
};
#[cfg(feature = "swtasks")]
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
#[cfg(feature = "swtasks")]
use syn::Path;
use syn::{ItemFn, parse_quote};

extern crate proc_macro;

const MIN_TASK_PRIORITY: u16 = 1;

// ============================================================================
// Entry point – dispatches to the selected backend
// ============================================================================

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(BackendImpl::default());
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
#[derive(Default)]
struct BackendImpl {
    info: OnceCell<InfoBus>,
}

impl CorePassBackend for BackendImpl {
    fn subscribe(&mut self, info_bus: InfoBus) {
        let _ = self.info.set(info_bus);
    }
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
    fn generate_global_definitions(
        &self,
        app_args: &AppArgs,
        app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        if cfg!(feature = "slic") {
            // The SLIC requires us to call to the [`riscv_rtic::codegen`] macro to generate
            // the appropriate SLIC structure, interrupt enumerations, etc.
            let mut stmts = vec![];
            let used_irqs = app_analysis.used_irqs.iter().map(|irq| &irq.0);
            let device = &app_args.pacs[0];
            let slic = quote! {rtic::export::riscv_slic};

            if cfg!(feature = "clint-backend") {
                let hart_id = app_info.core;
                stmts.push(quote!(rticx_riscv::export::codegen!(slic = #slic, pac = #device, swi = [#(#used_irqs,)*], backend = [hart_id = #hart_id]);));
            } else if cfg!(feature = "mecall-backend") {
                stmts.push(quote!(rticx_riscv::export::codegen!(slic = #slic, pac = #device, swi = [#(#used_irqs,)*]);));
            }

            // stmts
            Some(quote! {
                // TODO: check if this is needed ?
                use rticx_riscv::export::riscv_slic;
                [#(#stmts,)*]
            })
        } else {
            None
        }
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

    // ---- validation: dispatcher names for ESP targets ------------------------
    //
    // ESP32-C3 and ESP32-C6 only support `FROM_CPU_INTR{0..3}` as software
    // interrupt dispatchers.
    //
    // For the SLIC backend all interrupt names are valid because the SLIC
    // controller can route any interrupt.
    fn pre_codegen_validation(&self, _app: &App, _analysis: &Analysis) -> syn::Result<()> {
        // ESP32-C3/C6: validate dispatcher names against the supported set
        #[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
        {
            let info = self.info.get().expect("info must be set");
            let sw_pas = info
                .get::<rticx_sw_pass::App>(rticx_sw_pass::INFO_APP)
                .expect("sw pass promise violated");
            let allowed_names = [
                "FROM_CPU_INTR0",
                "FROM_CPU_INTR1",
                "FROM_CPU_INTR2",
                "FROM_CPU_INTR3",
            ];

            for irq_name in sw_pas.sub_apps[0].dispatchers.iter() {
                use quote::ToTokens;
                let irq_name = irq_name.segments.to_token_stream();
                if !allowed_names.contains(&irq_name.to_string().trim()) {
                    use syn::spanned::Spanned;

                    return Err(syn::Error::new(
                        irq_name.span(),
                        "Only FROM_CPU_INTR{0..3} are supported as \
                         interrupt sources on ESP32 targets.  Use these \
                         as dispatchers: `#[app(..., dispatchers = \
                         [FROM_CPU_INTR0, ...])]`.",
                    ));
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
