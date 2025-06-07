use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
#[cfg(feature = "autoassign")]
use rtic_auto_assign::AutoAssignPass;
use rtic_core::{AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
use syn::{parse_quote, ItemFn};

extern crate proc_macro;

struct Rp2040Rtic;

#[cfg(feature = "swtasks")]
use rtic_sw_pass::{SoftwarePass, SwPassBackend};

const MIN_TASK_PRIORITY: u16 = 3;
const MAX_TASK_PRIORITY: u16 = 0;

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    // use the standard software pass provided by rtic-sw-pass crate
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwPassBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(Rp2040Rtic);
    #[cfg(feature = "autoassign")]
    builder.bind_pre_core_pass(AutoAssignPass); // run auto-assign pass first
    #[cfg(feature = "swtasks")]
    builder.bind_pre_core_pass(sw_pass); // run software-pass second
    builder.build_rtic_macro(args, input)
}

// =========================================== Trait implementations ===================================================
impl CorePassBackend for Rp2040Rtic {
    fn default_task_priority(&self) -> u16 {
        MIN_TASK_PRIORITY
    }
    fn post_init(
        &self,
        app_args: &AppArgs,
        sub_app: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        let peripheral_crate = &app_args.pacs[sub_app.core as usize];
        let initialize_dispatcher_interrupts =
            app_analysis.used_irqs.iter().map(|(irq_name, priority)| {
                let priority = priority.min(&MIN_TASK_PRIORITY); // limit piority to minmum
                quote! {
                    //set interrupt priority
                    #peripheral_crate::CorePeripherals::steal()
                        .NVIC
                        .set_priority(#peripheral_crate::Interrupt::#irq_name, #priority as u8);
                    //unmask interrupt
                    #peripheral_crate::NVIC::unmask(#peripheral_crate::Interrupt::#irq_name);
                }
            });

        // initialize core 1 from core 0 if the application is for multicore (cores > 1)
        let init_and_spawn_core1 = if sub_app.core == 0 && app_args.cores > 1 {
            Some(init_core1(peripheral_crate))
        } else {
            None
        };

        let configure_fifo = if app_args.cores > 1 {
            Some(configure_fifo(peripheral_crate, sub_app.core))
        } else {
            None
        };

        Some(quote! {
            unsafe {
                #(#initialize_dispatcher_interrupts)*
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

        // irq names from hadware tasks
        let irq_list_as_u32 = app_info.tasks.iter().filter_map(|t| {
            let irq_name = t.args.binds.as_ref()?;
            Some(quote! { #peripheral_crate::Interrupt::#irq_name as u32, })
        });

        let mut irq_prio_map = [Vec::new(), Vec::new(), Vec::new()];
        for hw_task in app_info.tasks.iter() {
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
                rtic::export::create_mask([
                    #(#irq_as_u32)*
                ]),
            })
        }

        let core = app_info.core;
        let chunks_ident = format_ident!("__rtic_internal_MASK_CHUNKS_core{core}");
        let masks_ident = format_ident!("__rtic_internal_MASKS_core{core}");
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

    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        let core = app_info.core;
        let masks_ident = format_ident!("__rtic_internal_MASKS_core{core}"); // already computed by `compute_lock_static_args(...)`

        let lock_impl: syn::Block = parse_quote! {
            { unsafe { rtic::export::lock(resource_ptr, task_priority, CEILING, &#masks_ident, f); } }
        };

        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, core: u32) -> Ident {
        match core {
            0 => format_ident!("main"),
            _ => format_ident!("core{core}_entry"),
        }
    }

    fn wrap_task_execution(
        &self,
        _task_prio: u16,
        _dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        None
    }

    fn pre_codegen_validation(
        &self,
        _app: &rtic_core::App,
        _analysis: &rtic_core::Analysis,
    ) -> syn::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "swtasks")]
struct SwPassBackendImpl;
#[cfg(feature = "swtasks")]
impl SwPassBackend for SwPassBackendImpl {
    /// Provide the implementation/body of the core local interrupt pending function.
    fn generate_local_pend_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        // #[doc(hidden)]
        // #[inline]
        // pub fn __rtic_local_irq_pend<I: rtic::export::InterruptNumber>(irq_nbr : I) {
        let body = parse_quote!({
            rtic::export::NVIC::pend(irq_nbr);
        });
        // }
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// Provide the implementation/body of the cross-core interrupt pending function.
    fn generate_cross_pend_fn(&self, mut empty_body_fn: ItemFn) -> Option<ItemFn> {
        // #[doc(hidden)]
        // #[inline]
        // pub fn __rtic_cross_irq_pend<I: rtic::export::InterruptNumber>(irq_nbr : I, core: u32) {
        let body = parse_quote!({
            use rtic::export::InterruptNumber;
            let _ = rtic::export::cross_core::pend_irq(irq_nbr.number());
        });
        // }
        empty_body_fn.block = Box::new(body);
        Some(empty_body_fn)
    }
}

fn init_core1(pac: &syn::Path) -> TokenStream2 {
    quote! {
        /// Stack for core 1
        ///
        /// Core 0 gets its stack via the normal route - any memory not used by static values is
        /// reserved for stack and initialised by cortex-m-rt.
        /// To get the same for Core 1, we would need to compile everything seperately and
        /// modify the linker file for both programs, and that's quite annoying.
        /// So instead, core1.spawn takes a [usize] which gets used for the stack.
        /// NOTE: We use the `Stack` struct here to ensure that it has 32-byte alignment, which allows
        /// the stack guard to take up the least amount of usable RAM.
        static mut CORE1_STACK: rtic::export::Stack<4096> = rtic::export::Stack::new();

        let mut pac = unsafe { #pac::Peripherals::steal() };

        // The single-cycle I/O block controls our GPIO pins
        let mut sio = rtic::export::Sio::new(pac.SIO);

        let mut mc = rtic::export::Multicore::new(&mut pac.PSM, &mut pac.PPB, &mut sio.fifo);
        let cores = mc.cores();
        let core1 = &mut cores[1];
        let _ = core1.spawn(unsafe { &mut CORE1_STACK.mem }, move || core1_entry());
    }
}

fn configure_fifo(peripheral_crate: &syn::Path, core: u32) -> TokenStream2 {
    #[allow(non_snake_case)]
    let SIO_IRQ_PROC = format_ident!("SIO_IRQ_PROC{core}");
    quote! {
        unsafe {
            let sio = unsafe { &(*rp2040_hal::pac::SIO::PTR) };
            // drain fifo
            while sio.fifo_st.read().vld().bit() {
                let _ = sio.fifo_rd.read();
            }
            // clear status bits and unpend the FIFO interrupt
            sio.fifo_st.write(|wr| wr.bits(0xff) );
            #peripheral_crate::NVIC::unpend( #peripheral_crate::Interrupt::#SIO_IRQ_PROC);
            // Set FIFO0 interrupts priority to MAX priority
            #peripheral_crate::CorePeripherals::steal()
                .NVIC.set_priority( #peripheral_crate::Interrupt::#SIO_IRQ_PROC, #MAX_TASK_PRIORITY as u8);
            // unmask FIFO irq
            #peripheral_crate::NVIC::unmask( #peripheral_crate::Interrupt::#SIO_IRQ_PROC);
        }
    }
}
