use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use rtic_auto_assign::AutoAssignPass;
use rtic_core::{AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
use syn::{parse_quote, ItemFn};

extern crate proc_macro;

struct RenodeRtic;

use rtic_sw_pass::{SoftwarePass, SwPassBackend};

const MIN_TASK_PRIORITY: u16 = 15; // cortex m3 has 16 programmable priority levels (0 -> 15) with level 15 being the lowest
const MAX_TASK_PRIORITY: u16 = 0;
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    // use the standard software pass provided by rtic-sw-pass crate
    let sw_pass = SoftwarePass::new(SwPassBackendImpl);

    let mut builder = RticMacroBuilder::new(RenodeRtic);
    builder.bind_pre_core_pass(AutoAssignPass); // run auto-assign first
    builder.bind_pre_core_pass(sw_pass); // run software pass second
    builder.build_rtic_macro(args, input)
}

// =========================================== Trait implementations ===================================================
impl CorePassBackend for RenodeRtic {
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

        let configure_fifo = if app_args.cores > 1 {
            Some(configure_fifo(peripheral_crate, sub_app.core))
        } else {
            None
        };

        Some(quote! {
            unsafe {
                #(#initialize_dispatcher_interrupts)*
            }
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

        // define only once
        if app_info.core == 0 {
            Some(quote! {
                use #peripheral_crate::NVIC_PRIO_BITS;
            })
        } else {
            None
        }
    }

    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        let lock_impl: syn::Block = parse_quote! {
            {
                unsafe { rtic::export::lock(resource_ptr, CEILING as u8, NVIC_PRIO_BITS, f); }
            }
        };

        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, _core: u32) -> Ident {
        // same entry name for both cores.
        // two main() functions will be generated but both will be guarded by #[cfg(core = "X")]
        // each generated binary will have have one entry
        format_ident!("main")
    }

    /// Customize how the task is dispatched when its bound interrupt is triggered (save baspri before and restore after executing the task)
    fn wrap_task_execution(
        &self,
        task_prio: u16,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        Some(quote! {
            rtic::export::run(#task_prio as u8, || {#dispatch_task_call});
        })
    }

    fn multibin_shared_macro_path(&self) -> syn::Path {
        parse_quote!(rtic::export::microamp::shared)
    }

    fn pre_codegen_validation(
        &self,
        _app: &rtic_core::App,
        _analysis: &rtic_core::Analysis,
    ) -> syn::Result<()> {
        Ok(())
    }
}

struct SwPassBackendImpl;
impl SwPassBackend for SwPassBackendImpl {
    /// Provide the implementation/body of the core local interrupt pending function.
    fn generate_local_pend_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            rtic::export::NVIC::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// Provide the implementation/body of the cross-core interrupt pending function.
    fn generate_cross_pend_fn(&self, mut empty_body_fn: ItemFn) -> Option<ItemFn> {
        let body = parse_quote!({
            use rtic::export::InterruptNumber;
            rtic::export::cross_core::pend_irq(irq_nbr.number());
        });
        empty_body_fn.block = Box::new(body);
        Some(empty_body_fn)
    }
}

fn configure_fifo(peripheral_crate: &syn::Path, _core: u32) -> TokenStream2 {
    quote! {
        unsafe {
            let fifo = &mut rtic::mailbox::Mailbox;
            // drain fifo
            fifo.drain();
            // unpend the FIFO interrupt
            #peripheral_crate::NVIC::unpend(rtic::mailbox::InterruptExt::MAILBOX_INTERRUPT);
            // Set FIFO0 interrupts priority to MAX priority
            #peripheral_crate::CorePeripherals::steal()
                .NVIC.set_priority( rtic::mailbox::InterruptExt::MAILBOX_INTERRUPT, #MAX_TASK_PRIORITY as u8);
            // unmask FIFO irq
            #peripheral_crate::NVIC::unmask( rtic::mailbox::InterruptExt::MAILBOX_INTERRUPT);
        }
    }
}
