//! `#[app]` macro of the dual-core STM32H7 distribution.
//!
//! It assembles the three pieces a cross-binary Cortex-M target needs:
//!
//! - the **core pass** on [`Stm32H7Rtic`], which provides the BASEPRI locking,
//!   the NVIC/exception configuration and the idle loop, and reserves the HSEM
//!   interrupts for the cross-binary transport;
//! - the **cross-binary pass** ([`XbinPass`]) with [`Stm32H7XbinBackend`], the
//!   compile-time half of the distro contract: the `{M7, M4}` capability pool,
//!   the physical-core vocabulary and the HSEM ring/read doorbell bodies;
//! - the **software-tasks pass** ([`SoftwarePass`]), which emits the
//!   `RticSwTask` trait the cross-binary receivers implement and the
//!   `__rticx_local_irq_pend` the generated line dispatcher router calls.
//!
//! The core is selected at build time by exactly one of the `cm7` / `cm4`
//! features; see the `rticx-stm32h7` crate.

use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};

use rticx_core::{AppArgs, CorePassBackend, InfoBus, RticMacroBuilder, SubAnalysis, SubApp};
use rticx_stm32h7_bindings as bindings;
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
use rticx_xbin_pass::{CachePolicy, IpcPool, PhysicalCore, PoolId, XbinPass, XbinPassBackend};
use syn::{ItemFn, Path, parse_quote};

extern crate proc_macro;

/// Cortex-M exceptions that have a *configurable* priority. They may be bound
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

/// Exceptions whose priority is *not* configurable. They may never be bound to
/// a task (neither a dispatcher nor a user hardware task).
const NON_CONFIGURABLE_EXCEPTIONS: &[&str] = &["NonMaskableInt", "HardFault"];

/// HSEM interrupts reserved by the distribution for cross-binary IPC.
///
/// `HSEM0` is the Cortex-M7's HSEM interrupt (IRQ 125) and `HSEM1` the
/// Cortex-M4's (IRQ 126); the generated per-pair doorbell routers bind them.
/// A user task or dispatcher binding either would fight the transport.
const RESERVED_HSEM_INTERRUPTS: &[&str] = &["HSEM0", "HSEM1"];

/// Prefix of the generated doorbell router tasks, the only tasks allowed to
/// bind a reserved HSEM interrupt.
const XBIN_ROUTER_PREFIX: &str = "__RticxXbinRouter";

fn is_exception(name: &Ident) -> bool {
    let s = name.to_string();
    CONFIGURABLE_EXCEPTIONS.iter().any(|e| s == *e)
}

/// Physical core this binary runs on, selected by the distro feature.
const fn physical_core() -> u32 {
    if cfg!(feature = "cm7") {
        bindings::PHYSICAL_CM7
    } else {
        bindings::PHYSICAL_CM4
    }
}

#[doc = include_str!("../README_lib.md")]
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut builder = RticMacroBuilder::new(Stm32H7Rtic::new());
    // The cross-binary pass runs first and consumes `external_cores` /
    // `ipc_dispatchers` and the native cross-binary `#[sw_task]` receivers; the
    // software pass then compiles the rewritten receivers through `RticSwTask`.
    builder.bind_pre_core_pass(XbinPass::from_env().with_backend(Stm32H7XbinBackend));
    builder.bind_pre_core_pass(SoftwarePass::new(SwPassBackendImpl));
    builder.build_rtic_macro(args, input)
}

// ======================================= Core pass backend =======================================

struct Stm32H7Rtic {
    info_bus: Option<InfoBus>,
}

impl Stm32H7Rtic {
    fn new() -> Self {
        Self { info_bus: None }
    }
}

impl CorePassBackend for Stm32H7Rtic {
    fn subscribe(&mut self, info_bus: InfoBus) {
        self.info_bus = Some(info_bus);
    }

    fn post_init(
        &self,
        app_args: &AppArgs,
        app_info: &SubApp,
        app_analysis: &SubAnalysis,
    ) -> Option<TokenStream2> {
        // Each binary is single-core, so the PAC is always `pacs[0]`.
        let pac = &app_args.pacs[app_info.core as usize];
        let nvic_prio_bits = quote!(#pac::NVIC_PRIO_BITS);

        let mut stmts = Vec::new();

        // Configure priority + enable for every interrupt bound in this
        // application: user hardware tasks, software dispatchers and the
        // generated cross-binary line dispatchers / doorbell routers (the HSEM
        // interrupt is enabled here too; the HSEM per-semaphore bank is enabled
        // earlier by `configure_shared_memory`).
        for irq in &app_analysis.used_irqs {
            let irq_name = &irq.name;
            let priority = irq.priority;
            let es = format!(
                "Maximum priority used by interrupt vector '{irq_name}' is more than supported by hardware"
            );
            stmts.push(quote!(
                const _: () = if (1usize << #nvic_prio_bits) < #priority as usize {
                    ::core::panic!(#es);
                };
            ));

            if is_exception(irq_name) {
                stmts.push(quote!(
                    core.SCB.set_priority(
                        rticx_stm32h7::export::SystemHandler::#irq_name,
                        rticx_stm32h7::export::cortex_logical2hw(#priority as u8, #nvic_prio_bits),
                    );
                ));
            } else {
                stmts.push(quote!(
                    core.NVIC.set_priority(
                        #pac::Interrupt::#irq_name,
                        rticx_stm32h7::export::cortex_logical2hw(#priority as u8, #nvic_prio_bits),
                    );
                    rticx_stm32h7::export::NVIC::unmask(#pac::Interrupt::#irq_name);
                ));
            }
        }

        Some(quote! {
            let mut core = unsafe { rticx_stm32h7::export::Peripherals::steal() };
            unsafe {
                #(#stmts)*
            }
        })
    }

    fn populate_idle_loop(&self) -> Option<TokenStream2> {
        Some(quote! {
            rticx_stm32h7::export::wfi();
        })
    }

    fn generate_interrupt_free_fn(&self, mut empty_body_fn: ItemFn) -> ItemFn {
        let fn_body = parse_quote! {
            {
                rticx_stm32h7::export::interrupt::free(|_| f())
            }
        };
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    fn generate_enable_global_interrupts(&self) -> Option<TokenStream2> {
        // Cortex-M enables global interrupts by default (PRIMASK is cleared at
        // reset).
        None
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
        app_args: &AppArgs,
        _app_info: &SubApp,
        incomplete_lock_fn: syn::ImplItemFn,
    ) -> syn::ImplItemFn {
        let pac = &app_args.pacs[0];
        let lock_impl: syn::Block = parse_quote! {
            {
                unsafe {
                    rticx_stm32h7::export::lock(resource_ptr, CEILING as u8, #pac::NVIC_PRIO_BITS, f)
                }
            }
        };
        let mut completed_lock_fn = incomplete_lock_fn;
        completed_lock_fn.block.stmts.extend(lock_impl.stmts);
        completed_lock_fn
    }

    fn entry_name(&self, _core: u32) -> Ident {
        format_ident!("main")
    }

    fn wrap_task_execution(
        &self,
        task: &rticx_core::RticTask,
        dispatch_task_call: TokenStream2,
    ) -> Option<TokenStream2> {
        let task_prio = task.args.priority;
        Some(quote! {
            rticx_stm32h7::export::run(#task_prio as u8, || { #dispatch_task_call });
        })
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

                // The HSEM interrupts carry the cross-binary doorbells: the
                // distribution binds them to the generated routers and rejects
                // every user task or dispatcher that tries to use them.
                let generated_router = task
                    .task_struct
                    .ident
                    .to_string()
                    .starts_with(XBIN_ROUTER_PREFIX);
                if !generated_router && RESERVED_HSEM_INTERRUPTS.iter().any(|e| name == *e) {
                    return Err(syn::Error::new(
                        binds.span(),
                        "the HSEM interrupts (HSEM0 on the Cortex-M7, HSEM1 on the Cortex-M4) are \
                         reserved by rticx-stm32h7 for cross-binary IPC notifications and cannot \
                         be bound to a task or dispatcher",
                    ));
                }
            }
        }
        Ok(())
    }
}

// ======================================= Cross-binary backend =======================================

/// Compile-time half of the distro contract (M6.9-T1, M6.5).
struct Stm32H7XbinBackend;

impl XbinPassBackend for Stm32H7XbinBackend {
    fn backend(&self) -> syn::Expr {
        parse_quote!(rticx_stm32h7::xbin::Backend)
    }

    fn rt_path(&self) -> syn::Path {
        parse_quote!(rticx_stm32h7::export::xbin_rt)
    }

    fn ring_doorbell_fn(&self, source: u32, target: u32, mut template: syn::ItemFn) -> syn::ItemFn {
        template.block = parse_quote!({
            rticx_stm32h7::xbin::doorbell_send(#source, #target, task_id);
            Ok(())
        });
        template
    }

    fn doorbell_interrupt(&self, target: u32, _source: u32) -> syn::Ident {
        // `HSEM0` (IRQ 125) is the Cortex-M7 HSEM interrupt and `HSEM1`
        // (IRQ 126) the Cortex-M4's; the router runs on the receiver, so the
        // target physical core selects the local PAC variant.
        if target == bindings::PHYSICAL_CM7 {
            format_ident!("HSEM0")
        } else {
            format_ident!("HSEM1")
        }
    }

    fn read_doorbell_msg_fn(
        &self,
        target: u32,
        source: u32,
        mut template: syn::ItemFn,
    ) -> syn::ItemFn {
        template.block = parse_quote!({
            rticx_stm32h7::xbin::doorbell_take(#source, #target)
        });
        template
    }

    fn physical_core(&self, _local_core: u32) -> PhysicalCore {
        PhysicalCore(physical_core())
    }

    fn ipc_pools(&self, _local_core: u32) -> Vec<IpcPool> {
        let geometry = bindings::pool_for(physical_core());
        vec![IpcPool {
            id: PoolId::new(geometry.id),
            peer: PhysicalCore(geometry.peer),
            base_local: geometry.base_local,
            base_peer: geometry.base_peer,
            budget: geometry.budget,
            policy: CachePolicy::NormalNonCacheableShareable,
        }]
    }
}

// ======================================= Software pass backend =======================================

struct SwPassBackendImpl;

impl SwPassBackend for SwPassBackendImpl {
    fn queue_path(&self) -> Path {
        parse_quote!(rticx_stm32h7::export::Queue)
    }

    /// Core-local interrupt pending: used by `spawn` for software tasks and by
    /// the generated cross-binary router to wake a line dispatcher.
    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            rticx_stm32h7::export::NVIC::pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    /// Each binary is single-core; cross-core spawns between the two binaries
    /// go through the cross-binary transport, not this hook.
    fn generate_cross_pend_fn(&self, _core: u32, _empty_body_fn: ItemFn) -> Option<ItemFn> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capability binding reports exactly the `h7-sram3` dual from this
    /// core's side, with the same budget the runtime uses.
    #[test]
    fn capability_binding_reports_the_h7_sram3_dual() {
        let pools = Stm32H7XbinBackend.ipc_pools(0);
        assert_eq!(pools.len(), 1);
        let pool = &pools[0];
        let geometry = bindings::pool_for(physical_core());

        assert_eq!(pool.id, PoolId::new(bindings::POOL_ID));
        assert_eq!(pool.peer, PhysicalCore(geometry.peer));
        assert_eq!(pool.base_local, geometry.base_local);
        assert_eq!(pool.base_peer, geometry.base_peer);
        assert_eq!(pool.budget, bindings::POOL_BUDGET);
        assert_eq!(pool.policy, CachePolicy::NormalNonCacheableShareable);
    }

    /// The two endpoints' bindings are mirror images: a project building the
    /// M7 and M4 binaries sees the same pool id/budget with swapped views.
    #[test]
    fn binding_is_symmetric_across_the_two_physical_cores() {
        let cm7 = bindings::pool_for(bindings::PHYSICAL_CM7);
        let cm4 = bindings::pool_for(bindings::PHYSICAL_CM4);
        assert_eq!(cm7.id, cm4.id);
        assert_eq!(cm7.base_local, cm4.base_peer);
        assert_eq!(cm7.base_peer, cm4.base_local);
        assert_eq!(cm7.peer, bindings::PHYSICAL_CM4);
        assert_eq!(cm4.peer, bindings::PHYSICAL_CM7);
    }

    /// The router of a physical core binds that core's HSEM interrupt:
    /// `HSEM0` (IRQ 125) on the M7, `HSEM1` (IRQ 126) on the M4.
    #[test]
    fn doorbell_interrupt_selects_the_target_cores_hsem() {
        let backend = Stm32H7XbinBackend;
        assert_eq!(
            backend
                .doorbell_interrupt(bindings::PHYSICAL_CM7, bindings::PHYSICAL_CM4)
                .to_string(),
            "HSEM0"
        );
        assert_eq!(
            backend
                .doorbell_interrupt(bindings::PHYSICAL_CM4, bindings::PHYSICAL_CM7)
                .to_string(),
            "HSEM1"
        );
    }
}
