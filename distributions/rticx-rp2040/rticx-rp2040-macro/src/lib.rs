use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
#[cfg(feature = "async")]
use rticx_async_pass::{AsyncPass, AsyncPassBackend};
#[cfg(feature = "autoassign")]
use rticx_auto_assign::AutoAssignPass;
use rticx_core::{AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
#[cfg(feature = "swtasks")]
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
#[allow(unused)]
use syn::{ItemFn, LitInt, Path, parse_quote};

extern crate proc_macro;

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
#[cfg(any(feature = "async", feature = "swtasks"))]
const FIFO_INTERRUPTS: &[&str] = &["SIO_IRQ_PROC1", "SIO_IRQ_PROC0"];

fn is_exception(name: &Ident) -> bool {
    let s = name.to_string();
    CONFIGURABLE_EXCEPTIONS.iter().any(|e| s == *e)
}

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    // use the standard software pass provided by rticx-sw-pass crate
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwPassBackendImpl);

    #[cfg(feature = "async")]
    let async_pass = AsyncPass::new(AsyncPassBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(Rp2040Rtic);
    #[cfg(feature = "autoassign")]
    builder.bind_pre_core_pass(AutoAssignPass); // run auto-assign pass first
    #[cfg(feature = "swtasks")]
    builder.bind_pre_core_pass(sw_pass); // run software-pass second
    #[cfg(feature = "async")]
    builder.bind_pre_core_pass(async_pass); // run async-pass third
    builder.build_rtic_macro(args, input)
}

struct Rp2040Rtic;

// =========================================== Trait implementations ===================================================
impl CorePassBackend for Rp2040Rtic {
    fn post_init(
        &self,
        app_args: &AppArgs,
        sub_app: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        let pac = &app_args.pacs[sub_app.core as usize];
        let nvic_prio_bits = quote!(#pac::NVIC_PRIO_BITS);

        let mut interrupt_init_stmts = Vec::new();
        interrupt_init_stmts
            .push(quote!(let mut core = unsafe { rticx_rp2040::export::Peripherals::steal() };));

        // Configure priority + enable for every interrupt bound in this application
        // (this covers both user hardware tasks and dispatcher interrupts generated
        // by the software tasks pass, since both end up as `#[task(binds = ..)]`).
        for (irq_name, priority) in &app_analysis.used_irqs {
            let es = format!(
                "Maximum priority used by interrupt vector '{irq_name}' is more than supported by hardware"
            );
            // Compile-time assert that this priority is supported by the device
            interrupt_init_stmts.push(quote!(
                const _: () = if (1usize << #nvic_prio_bits) < #priority as usize {
                    ::core::panic!(#es);
                };
            ));

            if is_exception(irq_name) {
                // Exceptions use the SCB and are never unmasked
                interrupt_init_stmts.push(quote!(
                    core.SCB.set_priority(
                        rticx_rp2040::export::SystemHandler::#irq_name,
                        rticx_rp2040::export::cortex_logical2hw(#priority as u8, #nvic_prio_bits),
                    );
                ));
            } else {
                // External interrupts use the NVIC and must be unmasked after their
                // priority is set (changing the priority of a pended interrupt is
                // implementation-defined).
                interrupt_init_stmts.push(quote!(
                    core.NVIC.set_priority(
                        #pac::Interrupt::#irq_name,
                        rticx_rp2040::export::cortex_logical2hw(#priority as u8, #nvic_prio_bits),
                    );
                    rticx_rp2040::export::NVIC::unmask(#pac::Interrupt::#irq_name);
                ));
            }
        }

        // initialize core 1 from core 0 if the application is for multicore (cores > 1)
        let init_and_spawn_core1 = if sub_app.core == 0 && app_args.cores > 1 {
            Some(quote!(rticx_rp2040::export::cross_core::init_core1(
                move || core1_entry()
            );))
        } else {
            None
        };

        let configure_fifo =
            if app_args.cores > 1 && cfg!(any(feature = "async", feature = "swtasks")) {
                let core = sub_app.core;
                Some(quote!(rticx_rp2040::export::cross_core::configure_fifo(#core as u8)))
            } else {
                None
            };

        Some(quote! {
            unsafe {
                #(#interrupt_init_stmts)*
            }
            // init and spawn core 1 (if app.core == 0 and app_args.cores == 2 )
            #init_and_spawn_core1

            // configure fifo (if app_args.cores == 2 )
            #configure_fifo
        })
    }

    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        Some(quote! {
            unsafe { core::arch::asm!("wfi" ); }
        })
    }

    fn generate_interrupt_free_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        // eprintln!("{}", empty_body_fn.to_token_stream().to_string()); // enable comment to see the function signature
        let fn_body = parse_quote! {
            {
                // RTICX multicore model prevents shared resources between multiple cores. As such
                // We do not need to use multicore aware critical section (cortex_m::interrupt::free() with underlying critical section impl provided by rp2040-hal using spinlocks)
                // so we only need a core-local critical section.
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
        let peripheral_crate = &app_args.pacs[app_info.core as usize];

        let nvic_tasks: Vec<_> = app_info
            .tasks
            .iter()
            .filter(|t| t.args.binds.as_ref().is_some_and(|n| !is_exception(n)))
            .collect();
        let irq_list_as_u32 = nvic_tasks.iter().filter_map(|t| {
            let irq_name = t.args.binds.as_ref()?;
            Some(quote! { #peripheral_crate::Interrupt::#irq_name as u32, })
        });

        // Group NVIC interrupts by priority level (1..=3) to build one mask per level
        let mut irq_prio_map = [Vec::new(), Vec::new(), Vec::new()];
        for hw_task in &nvic_tasks {
            let prio = hw_task.args.priority;
            if (1..=3).contains(&prio) {
                let Some(irq_name) = hw_task.args.binds.as_ref() else {
                    continue;
                };
                irq_prio_map[(prio - 1) as usize].push(quote! {
                    #peripheral_crate::Interrupt::#irq_name as u32,
                })
            }
        }

        let mut masks = Vec::with_capacity(3);
        for priority_level in 1..=3 {
            let irq_as_u32 = &irq_prio_map[priority_level - 1];
            masks.push(quote! {
                rticx_rp2040::export::create_mask([
                    #(#irq_as_u32)*
                ]),
            })
        }

        let core = app_info.core;
        let chunks_ident = format_ident!("__rticx_internal_MASK_CHUNKS_core{core}");
        let masks_ident = format_ident!("__rticx_internal_MASKS_core{core}");
        Some(quote! {
            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            const #chunks_ident: usize = rticx_rp2040::export::compute_mask_chunks([
                #(#irq_list_as_u32)*
            ]);

            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            const #masks_ident: [rticx_rp2040::export::Mask<#chunks_ident>; 3] = [
                #(#masks)*
            ];
        })
    }

    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        let core = app_info.core;
        let masks_ident = format_ident!("__rticx_internal_MASKS_core{core}"); // already computed by `compute_lock_static_args(...)`

        let lock_impl: syn::Block = parse_quote! {
            { unsafe { rticx_rp2040::export::lock(resource_ptr, task_priority, CEILING, &#masks_ident, f) } }
        };

        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, core: u32) -> Ident {
        match core {
            0 => format_ident!("main"),
            1 => format_ident!("core1_entry"),
            _ => format_ident!("core{core}_entry"),
        }
    }

    fn wrap_task_execution(
        &self,
        _task: &rticx_core::RticTask,
        _dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        None
    }

    fn pre_codegen_validation(
        &self,
        app: &rticx_core::App,
        _analysis: &rticx_core::Analysis,
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
                // Software tasks use FIFO for spawning tasks from one core to the other
                #[cfg(any(feature = "async", feature = "swtasks"))]
                if FIFO_INTERRUPTS.iter().any(|e| name == *e) {
                    return Err(syn::Error::new(
                        binds.span(),
                        "FIFO interrupts are reserved by RTICX for cross-core tasks and cannot be for a hardware task",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "swtasks")]
struct SwPassBackendImpl;
#[cfg(feature = "swtasks")]
impl SwPassBackend for SwPassBackendImpl {
    /// Path to the SPSC queue type re-exported by this distribution.
    fn queue_path(&self) -> Path {
        parse_quote!(rticx_rp2040::export::Queue)
    }

    /// Provide the implementation/body of the core local interrupt pending function.
    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        // #[doc(hidden)]
        // #[inline]
        // pub fn __rticx_local_irq_pend_coreN(irq_nbr : rp2040::Interrupt) {
        let body = parse_quote!({
            rticx_rp2040::export::NVIC::pend(irq_nbr);
        });
        // }
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// Provide the implementation/body of the cross-core interrupt pending function.
    fn generate_cross_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> Option<ItemFn> {
        // #[doc(hidden)]
        // #[inline]
        // pub fn __rticx_cross_irq_coreN(irq_nbr : rp2040::Interrupt) {
        let body = parse_quote!({
            use rticx_rp2040::export::InterruptNumber;
            let _ = rticx_rp2040::export::cross_core::pend_irq(irq_nbr.number());
        });
        // }
        empty_body_fn.block = Box::new(body);
        Some(empty_body_fn)
    }
}

#[cfg(feature = "async")]
struct AsyncPassBackendImpl;
#[cfg(feature = "async")]
impl AsyncPassBackend for AsyncPassBackendImpl {
    fn queue_path(&self) -> Path {
        parse_quote!(rticx_rp2040::export::Queue)
    }

    fn async_runtime_path(&self) -> Path {
        parse_quote!(rticx_rp2040::export::async_rt)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            rticx_rp2040::export::NVIC::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    fn generate_cross_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> Option<ItemFn> {
        let body = parse_quote!({
            use rticx_rp2040::export::InterruptNumber;
            let _ = rticx_rp2040::export::cross_core::pend_irq(irq_nbr.number());
        });
        empty_body_fn.block = Box::new(body);
        Some(empty_body_fn)
    }

    fn generate_wake_pend_fn(&self, core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let core_lit = LitInt::new(&core.to_string(), proc_macro2::Span::call_site());
        let body: syn::Block = parse_quote!({
            let current_core = unsafe { (*rp2040_hal::pac::SIO::PTR).cpuid().read().bits() };
            if current_core == #core_lit {
                rticx_rp2040::export::NVIC::pend(irq_nbr);
            } else {
                use rticx_rp2040::export::InterruptNumber;
                let _ = rticx_rp2040::export::cross_core::pend_irq(irq_nbr.number());
            }
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }
}
