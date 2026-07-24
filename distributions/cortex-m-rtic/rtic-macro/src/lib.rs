use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};

use rtic_core::{AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
#[cfg(feature = "swtasks")]
use rtic_sw_pass::{SoftwarePass, SwPassBackend};
use syn::{ItemFn, parse_quote};

extern crate proc_macro;

struct CortexMRtic;

/// Cortex-M exceptions that have a *configurable* priority. These may be bound
/// to hardware tasks (their priority is set via `SCB`), but must not be used as
/// dispatcher interrupts.
const CONFIGURABLE_EXCEPTIONS: &[&str] = &[
    "MemoryManagement",
    "BusFault",
    "UsageFault",
    "SecureFault",
    "SVCall",
    "DebugMonitor",
    "PendSV",
    "SysTick",
];

/// Exceptions whose priority is *not* configurable. They may never be bound to a
/// task (neither a dispatcher nor a user hardware task).
const NON_CONFIGURABLE_EXCEPTIONS: &[&str] = &["NonMaskableInt", "HardFault"];

fn is_exception(name: &Ident) -> bool {
    let s = name.to_string();
    CONFIGURABLE_EXCEPTIONS.iter().any(|e| s == *e)
}

/// Lowest logical priority in Cortex-M (numerically larger == lower urgency).
const MIN_TASK_PRIORITY: u16 = 1;

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwPassBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(CortexMRtic);
    #[cfg(feature = "swtasks")]
    builder.bind_pre_core_pass(sw_pass); // run software pass before the core pass
    builder.build_rtic_macro(args, input)
}

// =========================================== CorePassBackend ===================================================
impl CorePassBackend for CortexMRtic {
    fn default_task_priority(&self) -> u16 {
        MIN_TASK_PRIORITY
    }

    fn post_init(
        &self,
        app_args: &AppArgs,
        app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        // single core: the PAC is always pacs[0]
        let pac = &app_args.pacs[app_info.core as usize];
        let nvic_prio_bits = quote!(#pac::NVIC_PRIO_BITS);

        let mut stmts = Vec::new();

        // Configure priority + enable for every interrupt bound in this application
        // (this covers both user hardware tasks and dispatcher interrupts generated
        // by the software tasks pass, since both end up as `#[task(binds = ..)]`).
        for (irq_name, priority) in &app_analysis.used_irqs {
            let es = format!(
                "Maximum priority used by interrupt vector '{irq_name}' is more than supported by hardware"
            );
            // Compile-time assert that this priority is supported by the device
            stmts.push(quote!(
                const _: () = if (1usize << #nvic_prio_bits) < #priority as usize {
                    ::core::panic!(#es);
                };
            ));

            if is_exception(irq_name) {
                // Exceptions use the SCB and are never unmasked
                stmts.push(quote!(
                    core.SCB.set_priority(
                        rtic::export::SystemHandler::#irq_name,
                        rtic::export::cortex_logical2hw(#priority as u8, #nvic_prio_bits),
                    );
                ));
            } else {
                // External interrupts use the NVIC and must be unmasked after their
                // priority is set (changing the priority of a pended interrupt is
                // implementation-defined).
                stmts.push(quote!(
                    core.NVIC.set_priority(
                        #pac::Interrupt::#irq_name,
                        rtic::export::cortex_logical2hw(#priority as u8, #nvic_prio_bits),
                    );
                    rtic::export::NVIC::unmask(#pac::Interrupt::#irq_name);
                ));
            }
        }

        // `core::peripheral::Peripherals` handle for SCB/NVIC access at runtime.
        // `post_init` already runs inside a critical section, so stealing is safe.
        Some(quote! {
            let mut core = unsafe { rtic::export::Peripherals::steal() };
            unsafe {
                #(#stmts)*
            }
        })
    }

    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        Some(quote! {
            rtic::export::wfi();
        })
    }

    fn generate_interrupt_free_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let fn_body = parse_quote! {
            {
                unsafe { core::arch::asm!("cpsid i"); } // critical section begin
                let r = f();
                unsafe { core::arch::asm!("cpsie i"); } // critical section end
                r
            }
        };
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    fn generate_global_definitions(
        &self,
        app_args: &AppArgs,
        app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        // BASEPRI locking needs no precomputed global state.
        #[cfg(not(feature = "armv6m"))]
        {
            let _ = (app_args, app_info);
            None
        }
        // Interrupt source-masking needs a per-priority mask table computed at compile time.
        #[cfg(feature = "armv6m")]
        generate_source_mask_globals(app_args, app_info)
    }

    fn generate_resource_proxy_lock_impl(
        &self,
        app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        // ---- Interrupt source masking (armv6-m: M0/M0+/M23) ----
        if cfg!(feature = "armv6m") {
            let _ = app_args;
            let lock_impl: syn::Block = parse_quote! {
                {
                    unsafe {
                        rtic::export::lock(
                            resource_ptr,
                            task_priority,
                            CEILING,
                            &__rtic_internal_MASKS,
                            f,
                        )
                    }
                }
            };
            let mut completed_lock_fn = incomplete_lock_fn;
            completed_lock_fn.block.stmts.extend(lock_impl.stmts);
            completed_lock_fn
        }
        // ---- BASEPRI locking (armv7-m and above) ----
        else {
            let pac = &app_args.pacs[0];
            let lock_impl: syn::Block = parse_quote! {
                {
                    unsafe {
                        rtic::export::lock(resource_ptr, CEILING as u8, #pac::NVIC_PRIO_BITS, f)
                    }
                }
            };

            {
                let mut completed_lock_fn = incomplete_lock_fn;
                completed_lock_fn.block.stmts.extend(lock_impl.stmts);
                completed_lock_fn
            }
        }
    }

    fn entry_name(&self, _core: u32) -> Ident {
        format_ident!("main")
    }

    /// Save/restore the priority ceiling around task execution. On armv7-m this
    /// saves/restores BASEPRI; on armv6-m `run` is a no-op (masking happens via
    /// the lock primitives only), so the same call works for both paths.
    fn wrap_task_execution(
        &self,
        task_prio: u16,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        Some(quote! {
            rtic::export::run(#task_prio as u8, || { #dispatch_task_call });
        })
    }

    fn pre_codegen_validation(
        &self,
        app: &rtic_core::App,
        _analysis: &rtic_core::Analysis,
    ) -> syn::Result<()> {
        for sub_app in &app.sub_apps {
            for task in &sub_app.tasks {
                let Some(binds) = &task.args.binds else {
                    continue;
                };
                let name = binds.to_string();
                if NON_CONFIGURABLE_EXCEPTIONS.iter().any(|e| name == *e) {
                    return Err(syn::Error::new(
                        binds.span(),
                        "only exceptions with configurable priority can be used as hardware tasks",
                    ));
                }
            }
        }
        Ok(())
    }
}

// =========================================== Source-masking globals ===========================================
/// Emits the compile-time priority mask table required by the interrupt source
/// masking lock implementation (armv6-m).
#[cfg(feature = "armv6m")]
fn generate_source_mask_globals(app_args: &AppArgs, app_info: &SubApp) -> Option<TokenStream2> {
    let pac = &app_args.pacs[app_info.core as usize];

    // All interrupt numbers in use (as u32), needed to size the Mask chunks
    let irq_list_as_u32 = app_info.tasks.iter().filter_map(|t| {
        let irq_name = t.args.binds.as_ref()?;
        Some(quote! { #pac::Interrupt::#irq_name as u32, })
    });

    // Group interrupts by priority level (1..=3) to build one mask per level
    let mut irq_prio_map = [Vec::new(), Vec::new(), Vec::new()];
    for task in app_info.tasks.iter() {
        let prio = task.args.priority;
        if (1..=3).contains(&prio) {
            let Some(irq_name) = task.args.binds.as_ref() else {
                continue;
            };
            irq_prio_map[(prio - 1) as usize].push(quote! {
                #pac::Interrupt::#irq_name as u32,
            });
        }
    }

    let mut masks = Vec::with_capacity(3);
    for priority_level in 1..=3 {
        let irq_as_u32 = &irq_prio_map[priority_level - 1];
        masks.push(quote! {
            rtic::export::create_mask([
                #(#irq_as_u32)*
            ]),
        });
    }

    let chunks_ident = format_ident!("__rtic_internal_MASK_CHUNKS");
    let masks_ident = format_ident!("__rtic_internal_MASKS");

    Some(quote! {
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #chunks_ident: usize = rtic::export::compute_mask_chunks([
            #(#irq_list_as_u32)*
        ]);

        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #masks_ident: [rtic::export::Mask<#chunks_ident>; 3] = [
            #(#masks)*
        ];
    })
}

// =========================================== Software pass backend ===========================================
#[cfg(feature = "swtasks")]
struct SwPassBackendImpl;

#[cfg(feature = "swtasks")]
impl SwPassBackend for SwPassBackendImpl {
    /// Core-local interrupt pending: used by `spawn` for software tasks running
    /// on this core.
    fn generate_local_pend_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            rtic::export::NVIC::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// No secondary core: cross-core pending is unavailable on this single-core
    /// distribution.
    fn generate_cross_pend_fn(&self, _empty_body_fn: ItemFn) -> Option<ItemFn> {
        None
    }
}
