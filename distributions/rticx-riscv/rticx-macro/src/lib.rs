use std::cell::OnceCell;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};

use rticx_async_pass::{AsyncPass, AsyncPassBackend};
use rticx_core::{
    Analysis, App, AppArgs, CorePassBackend, InfoBus, RticMacroBuilder, SubAnalysis, SubApp,
};

use rticx_sw_pass::{SoftwarePass, SwPassBackend};
use syn::{ItemFn, Path, parse_quote};

extern crate proc_macro;

#[cfg(all(feature = "swtasks", feature = "async"))]
compile_error!(
    "rticx-riscv-macro: the `swtasks` and `async` features are mutually exclusive; enable at most one"
);

// ============================================================================
// Entry point – dispatches to the selected backend
// ============================================================================

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let sw_pass = SoftwarePass::new(SwBackendImpl);
    let async_pass = AsyncPass::new(AsyncPassBackendImpl);

    let mut builder = RticMacroBuilder::new(BackendImpl::default());
    if cfg!(feature = "swtasks") {
        builder.bind_pre_core_pass(sw_pass);
    }
    if cfg!(feature = "async") {
        builder.bind_pre_core_pass(async_pass);
    }
    builder.build_rtic_macro(args, input)
}

#[derive(Default)]
struct BackendImpl {
    info: OnceCell<InfoBus>,
    #[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
    ext_intr_map: OnceCell<std::collections::HashMap<syn::Ident, u8>>,
}

impl CorePassBackend for BackendImpl {
    fn subscribe(&mut self, info_bus: InfoBus) {
        let _ = self.info.set(info_bus);
    }

    /// post_init SLIC backend: enable & prioritize every interrupt used by the app
    #[cfg(feature = "slic")]
    fn post_init(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        // SLIC: set the priority of every interrupt in use.
        // Dispatchers are also SLIC interrupts (`SoftwareInterruptN`) and get their
        // priority set here. Hardware task interrupts are PAC interrupts
        // routed through the SLIC.
        let set_prio = app_analysis.used_irqs.iter().map(|irq| {
            let irq_name = &irq.name;
            let priority = irq.priority;
            quote! {
                unsafe {
                    rticx_riscv::export::set_priority(
                        slic::SoftwareInterrupt::#irq_name,
                        #priority as u8,
                    )
                };
            }
        });

        Some(quote! { #(#set_prio)* })
    }

    /// post_init ESP32 backend: enable & prioritize and unmask every interrupt used by the app
    ///
    /// ## CPU Interrupt Reservations (`esp-hal` Compatibility)
    ///
    /// When targeting ESP32 RISC-V devices, lower-numbered CPU interrupt slots are
    /// strictly reserved for `esp-hal`'s internal priority-vectored dispatching and
    /// `mtvec` trap management:
    ///
    /// - **ESP32-C3:** CPU interrupts `1..=15` are reserved.
    /// - **ESP32-C6:** CPU interrupts `1..=19` are reserved.
    ///
    /// ### Implications for RTICX Vector Binding:
    /// `esp-hal` manages these reserved slots via its own internal Interrupt Matrix
    /// routing and dispatch table. Any direct CPU interrupt binding (such as raw
    /// RTICX hardware/software task vectors) **must strictly target unreserved CPU slots**:
    ///
    /// - **ESP32-C3:** CPU interrupts `16..=31`
    /// - **ESP32-C6:** CPU interrupts `20..=31`
    ///
    /// Attempting to assign RTICX handlers directly to reserved CPU interrupt slots
    /// will bypass `esp-hal`'s dispatch table, leading to vector table collisions
    /// and unhandled interrupt panics.
    #[cfg(any(feature = "esp32c6", feature = "esp32c3"))]
    fn post_init(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        let mut stmts: Vec<TokenStream2> = Vec::new();

        let max_prio: usize = 15;
        let min_prio: usize = 1;
        let interrupt_start = if cfg!(feature = "esp32c6") { 20u8 } else { 16 };
        let mut external_interrupts_map = std::collections::HashMap::new();

        for (irq, cpu_interrupt_id) in app_analysis.used_irqs.iter().zip(interrupt_start..) {
            let irq_name = &irq.name;
            let priority = irq.priority;
            let es_max = format!(
                "Maximum priority used by interrupt vector '{irq_name}' is more than supported by hardware"
            );
            let es_min = format!(
                "Priority {priority} used by interrupt vector '{irq_name}' is less than supported by hardware"
            );
            let enable_statements = quote! {
                const _: () = if (#max_prio) <= #priority as usize {
                    ::core::panic!(#es_max);
                };
                const _: () = if (#min_prio) > #priority as usize {
                    ::core::panic!(#es_min);
                };
                rticx_riscv::export::enable(
                    rticx_riscv::export::Interrupt::#irq_name,
                    #priority as u8,
                    #cpu_interrupt_id as u8,
                );
            };
            stmts.push(enable_statements);
            external_interrupts_map.insert(irq_name.clone(), cpu_interrupt_id);
        }
        self.ext_intr_map
            .set(external_interrupts_map)
            .expect("only post init initializes this");
        Some(quote! { #(#stmts)* })
    }

    /// No target selected: nothing to configure.
    #[cfg(not(any(feature = "slic", feature = "esp32c3", feature = "esp32c6")))]
    fn post_init(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        None
    }

    #[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
    fn task_attrs(&self, interrupt: syn::Ident) -> Vec<syn::Attribute> {
        if let Some(map) = self.ext_intr_map.get()
            && let Some(cpu_interrupt_id) = map.get(&interrupt)
        {
            let interrupt_name = format!("interrupt{cpu_interrupt_id}");
            vec![parse_quote!(#[unsafe(export_name = #interrupt_name)])]
        } else {
            vec![]
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
                rticx_riscv::export::interrupt::disable();
            }
            let r = f();
            unsafe {
                rticx_riscv::export::interrupt::enable();
            }
            r
        });
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    /// Target specific global definitions
    fn generate_global_definitions(
        &self,
        app_args: &AppArgs,
        app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        if cfg!(feature = "slic") {
            // The SLIC requires us to call to the [`rticx_riscv::export::codegen`] macro to generate
            // the appropriate SLIC structure, interrupt enumerations, etc.
            let mut stmts = vec![];
            let used_irqs = app_analysis.used_irqs.iter().map(|irq| &irq.name);
            let device = &app_args.pacs[0];
            let slic = quote! {rticx_riscv::export::riscv_slic};

            if cfg!(feature = "clint-backend") {
                let hart_id = syn::Ident::new(&format!("H{}", app_info.core), Span::call_site());
                stmts.push(quote!(rticx_riscv::export::codegen!(slic = #slic, pac = #device, swi = [#(#used_irqs,)*], backend = [hart_id = #hart_id]);));
            } else if cfg!(feature = "mecall-backend") {
                stmts.push(quote!(rticx_riscv::export::codegen!(slic = #slic, pac = #device, swi = [#(#used_irqs,)*]);));
            }

            // stmts
            Some(quote! {
                #(#stmts)*
            })
        } else {
            None
        }
    }

    /// SRP resource locking
    //
    /// All three backends use threshold-based locking.  The export module
    /// exposes a target-specific `lock(ptr, ceiling, f)` function that raises
    /// the interrupt priority ceiling, calls `f`, and restores the old ceiling.
    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        let lock_impl: syn::Block = parse_quote!({
            //fn lock<R>(&mut self, f: impl FnOnce(&mut Self::ResourceType) -> R) -> R {
            //  const CEILING: u16 = ...;
            //  let task_priority = ...;
            //  let resource_ptr = ...;
            { unsafe { rticx_riscv::export::lock(resource_ptr, CEILING as u8, f) } }
            //}
        });
        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, _core: u32) -> syn::Ident {
        format_ident!("main")
    }

    /// task execution wrapping: threshold save/restore
    ///
    /// For the ESP32 backends the `run(prio, f)` function saves the current
    /// `cpu_int_thresh`/`mxint_thresh` value, calls `f`, then restores it.
    /// For the SLIC backend `riscv_slic::run(prio, f)` does the same.
    fn wrap_task_execution(
        &self,
        task: &rticx_core::RticTask,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        let task_prio = task.args.priority;
        // Unlike cortex-m RISC V interrupt handling requires manual unpending of an interrupt
        // binds is None in the case of software tasks, but the dispatcher for the tasks is a hardware task so we unpend once at the root of the dispatcher
        #[cfg(not(feature = "slic"))]
        let unpend_interrupt = task.args.binds.as_ref().map(|interrupt| quote!(rticx_riscv::export::unpend(rticx_riscv::export::Interrupt::#interrupt);));
        #[cfg(feature = "slic")]
        let unpend_interrupt = quote!();
        Some(quote! {
            #unpend_interrupt
            rticx_riscv::export::run(#task_prio as u8, || { #dispatch_task_call });
        })
    }

    /// Validation: dispatcher names for ESP targets
    ///
    /// ESP32-C3 and ESP32-C6 only support `FROM_CPU_INTR{0..3}` as software
    /// interrupt dispatchers.
    ///
    /// For the SLIC backend all interrupt names are valid because the SLIC
    /// controller can route any interrupt.
    fn pre_codegen_validation(&self, _app: &App, _analysis: &Analysis) -> syn::Result<()> {
        // ESP32-C3/C6: validate dispatcher names against the supported set
        #[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
        {
            let info = self.info.get().expect("info must be set");
            if let Ok(sw_pas) = info.get::<rticx_sw_pass::App>(rticx_sw_pass::INFO_APP) {
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
        }

        Ok(())
    }
}

// ============================================================================
// Software-tasks pass backend
// ============================================================================

struct SwBackendImpl;

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
    #[cfg(feature = "slic")]
    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        Some(parse_quote!(slic::SoftwareInterrupt))
    }
}

// ============================================================================
// Async-tasks pass backend
// ============================================================================

struct AsyncPassBackendImpl;

impl AsyncPassBackend for AsyncPassBackendImpl {
    fn queue_path(&self) -> Path {
        parse_quote!(rticx_riscv::export::Queue)
    }

    fn async_runtime_path(&self) -> Path {
        parse_quote!(rticx_riscv::export::async_rt)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            rticx_riscv::export::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    fn generate_cross_pend_fn(&self, _core: u32, _empty_body_fn: ItemFn) -> Option<ItemFn> {
        None
    }
}
