use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::format_ident;

use rticx_core::{AppArgs, CorePassBackend, RticMacroBuilder, SubAnalysis, SubApp};
#[cfg(feature = "swtasks")]
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
#[cfg(feature = "swtasks")]
use syn::Path;
use syn::{parse_quote, ItemFn};

extern crate proc_macro;

struct RiscVRtic;

const MIN_TASK_PRIORITY: u16 = 1;

#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    #[cfg(feature = "swtasks")]
    let sw_pass = SoftwarePass::new(SwPassBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(RiscVRtic);
    #[cfg(feature = "swtasks")]
    builder.bind_pre_core_pass(sw_pass);
    builder.build_rtic_macro(args, input)
}

impl CorePassBackend for RiscVRtic {
    fn default_task_priority(&self) -> u16 {
        MIN_TASK_PRIORITY
    }

    fn post_init(
        &self,
        _app_args: &AppArgs,
        _sub_app: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        // TODO: implement target-specific post-init (enable interrupts, etc.)
        None
    }

    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        Some(quote::quote! {
            unsafe { core::arch::asm!("wfi"); }
        })
    }

    fn generate_interrupt_free_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let fn_body = parse_quote!({
            // TODO: implement RISC-V critical section (e.g. using mstatus.MIE)
            let r = f();
            r
        });
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    fn generate_global_definitions(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        _app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        None
    }

    fn generate_resource_proxy_lock_impl(
        &self,
        _app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        // TODO: implement RISC-V resource lock (threshold-based or CSRC-based)
        let lock_impl: syn::Block = parse_quote!({
            {
                let r = f(resource_ptr);
                r
            }
        });
        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, _core: u32) -> syn::Ident {
        format_ident!("main")
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
        _app: &rticx_core::App,
        _analysis: &rticx_core::Analysis,
    ) -> syn::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "swtasks")]
struct SwPassBackendImpl;

#[cfg(feature = "swtasks")]
impl SwPassBackend for SwPassBackendImpl {
    fn queue_path(&self) -> Path {
        parse_quote!(rticx_riscv::export::Queue)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            // TODO: implement RISC-V software interrupt pending
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    fn generate_cross_pend_fn(&self, _core: u32, _empty_body_fn: ItemFn) -> Option<ItemFn> {
        // Single-core by default; override for multicore RISC-V targets
        None
    }
}
