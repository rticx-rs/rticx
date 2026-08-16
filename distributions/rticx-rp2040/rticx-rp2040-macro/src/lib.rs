use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use rticx_async_pass::{AsyncPass, AsyncPassBackend};
use rticx_auto_assign::AutoAssignPass;
use rticx_core::parse_utils::RticAttr;
use rticx_core::{
    AppArgs, CorePassBackend, InfoBus, RticMacroBuilder, RticPass, SubAnalysis, SubApp,
};
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
#[allow(unused)]
use syn::{Expr, ExprLit, ItemFn, ItemMod, Lit, LitInt, Path, parse_quote, spanned::Spanned};

extern crate proc_macro;

#[cfg(all(feature = "swtasks", feature = "async"))]
compile_error!(
    "rticx-rp2040-macro: the `swtasks` and `async` features are mutually exclusive; enable at most one"
);

/// Info bus entry holding the stack size (in 32-bit words) of core 1.
/// Published by [`Core1StackPass`], consumed by the backend when generating the
/// core 1 stack static in `generate_global_definitions`.
static INFO_CORE1_STACK_SIZE: &str = "rticx_rp2040_core1_stack::StackSize";

/// Default core 1 stack size in 32-bit words (4096 * 4 = 16 KiB) used when the
/// user does not specify the `core1_stack` argument.
const DEFAULT_CORE1_STACK_SIZE: usize = 4096;

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
const FIFO_INTERRUPTS: &[&str] = &["SIO_IRQ_PROC1", "SIO_IRQ_PROC0"];

fn is_exception(name: &Ident) -> bool {
    let s = name.to_string();
    CONFIGURABLE_EXCEPTIONS.iter().any(|e| s == *e)
}

#[doc = include_str!("../README_lib.md")]
#[proc_macro_attribute]
pub fn app(args: TokenStream, input: TokenStream) -> TokenStream {
    // use the standard software pass provided by rticx-sw-pass crate
    let sw_pass = SoftwarePass::new(SwPassBackendImpl);

    let async_pass = AsyncPass::new(AsyncPassBackendImpl);

    #[allow(unused_mut)]
    let mut builder = RticMacroBuilder::new(Rp2040Rtic::new());
    // distro-specific pass: consumes the `core1_stack` app argument (pass-through otherwise)
    builder.bind_pre_core_pass(Core1StackPass::new());
    if cfg!(feature = "autoassign") {
        builder.bind_pre_core_pass(AutoAssignPass); // run auto-assign pass first
    }
    if cfg!(feature = "swtasks") {
        builder.bind_pre_core_pass(sw_pass); // run software-pass second
    }
    if cfg!(feature = "async") {
        builder.bind_pre_core_pass(async_pass); // run async-pass third
    }
    builder.build_rtic_macro(args, input)
}

struct Rp2040Rtic {
    info_bus: Option<InfoBus>,
}

impl Rp2040Rtic {
    fn new() -> Self {
        Self { info_bus: None }
    }
}

/// Distro-specific pass that consumes the `core1_stack = N` `#[app]` argument
/// and publishes the core 1 stack size (in 32-bit words) to the info bus.
///
/// The application module syntax is left completely unchanged (pass-through);
/// only the consumed argument is stripped from the `#[app(...)]` arguments so
/// that the core parser never sees it.
struct Core1StackPass {
    info_bus: Option<InfoBus>,
}

impl Core1StackPass {
    fn new() -> Self {
        Self { info_bus: None }
    }
}

/// Parse the `cores = N` app argument without removing it from the attribute
/// (the core parser still needs it). Returns `Ok(None)` if the argument is
/// absent or not an integer literal.
fn parse_cores_arg(expr: Option<&Expr>) -> syn::Result<Option<u32>> {
    match expr {
        Some(Expr::Lit(ExprLit {
            lit: Lit::Int(lit), ..
        })) => lit.base10_digits().parse().map(Some).map_err(|e| {
            syn::Error::new(
                lit.span(),
                format!("`cores` must be an integer literal: {e}"),
            )
        }),
        _ => Ok(None),
    }
}

impl RticPass for Core1StackPass {
    fn subscribe(&mut self, info_bus: InfoBus) {
        let _ = self.info_bus.insert(info_bus);
    }

    fn run_pass(
        &self,
        args: TokenStream2,
        app_mod: ItemMod,
    ) -> syn::Result<(TokenStream2, ItemMod)> {
        let mut attr = RticAttr::parse_from_tokens(args.clone(), format_ident!("app"))?;

        // capture the span before removing the argument
        let core1_stack_span = attr
            .get_expr("core1_stack")
            .map(|e| e.span())
            .unwrap_or_else(proc_macro2::Span::call_site);

        let stack_size = match attr.take_usize("core1_stack")? {
            Some(n) => {
                // core1_stack is only meaningful when a second core is actually started
                let cores = parse_cores_arg(attr.get_expr("cores"))?.unwrap_or(1);
                if cores != 2 {
                    return Err(syn::Error::new(
                        core1_stack_span,
                        format!(
                            "`core1_stack` is only supported with `cores = 2` (found `cores = {cores}`)"
                        ),
                    ));
                }
                if n < 1024 {
                    return Err(syn::Error::new(
                        core1_stack_span,
                        "`core1_stack` must be at least 1024 words (4KiB)",
                    ));
                }
                n
            }
            None => DEFAULT_CORE1_STACK_SIZE,
        };

        self.info_bus.as_ref().inspect(|b| {
            b.publish(INFO_CORE1_STACK_SIZE, stack_size)
                .unwrap_or_else(|_| {
                    panic!("no other crate is allowed to publish {INFO_CORE1_STACK_SIZE}")
                });
        });

        // hand the app arguments back without the argument we consumed; the module is untouched
        let args = attr.args_tokens();
        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        "Core1Stack"
    }
}

// =========================================== Trait implementations ===================================================
impl CorePassBackend for Rp2040Rtic {
    fn subscribe(&mut self, info_bus: InfoBus) {
        self.info_bus = Some(info_bus);
    }

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
        for irq in &app_analysis.used_irqs {
            let irq_name = &irq.name;
            let priority = irq.priority;
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
        // The stack static itself is emitted by `generate_global_definitions` (core 0 only),
        // sized from the `core1_stack` argument published to the info bus by `Core1StackPass`.
        let init_and_spawn_core1 = if sub_app.core == 0 && app_args.cores > 1 {
            Some(quote!(
                let core1_stack = unsafe { &mut __rticx_internal_CORE1_STACK.mem };
                rticx_rp2040::export::cross_core::init_core1(move || core1_entry(), core1_stack);
            ))
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
                // We do not need to use multicore aware critical section so we only need a core-local critical section.
                rticx_rp2040::export::interrupt::free(|_| f())
            }
        };
        empty_body_fn.block = Box::new(fn_body);
        empty_body_fn
    }

    fn generate_enable_global_interrupts(&self) -> Option<TokenStream2> {
        // Cortex-M enables global interrupts by default (PRIMASK is cleared
        // at reset)
        None
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

        // Stack for core 1, sized from the `core1_stack` argument published to
        // the info bus by `Core1StackPass`. Emitted only by the core 0 sub-app;
        // referenced by `post_init` when spawning core 1.
        let core1_stack_def = if app_info.core == 0 && app_args.cores > 1 {
            let core1_stack_size = self
                .info_bus
                .as_ref()
                .and_then(|b| b.get::<usize>(INFO_CORE1_STACK_SIZE).ok())
                .map(|v| *v)
                .unwrap_or(DEFAULT_CORE1_STACK_SIZE);

            Some(quote! {
                #[doc(hidden)]
                static mut __rticx_internal_CORE1_STACK: rticx_rp2040::export::Stack<#core1_stack_size> =
                    rticx_rp2040::export::Stack::new();
            })
        } else {
            None
        };

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

            #core1_stack_def
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

                if cfg!(any(feature = "async", feature = "swtasks"))
                    && FIFO_INTERRUPTS.iter().any(|e| name == *e)
                {
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

struct SwPassBackendImpl;
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

struct AsyncPassBackendImpl;
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

    fn generate_stack_overflow_check(&self, core: u32) -> Option<TokenStream2> {
        match core {
            0 => Some(quote! {
                {
                    // Check for stack overflow using symbols from `cortex-m-rt`.
                    unsafe extern "C" {
                        static _stack_start: u32;
                        static __ebss: u32;
                    }
                    let stack_start = unsafe { &_stack_start as *const _ as u32 };
                    let ebss = unsafe { &__ebss as *const _ as u32 };
                    if stack_start > ebss {
                        // No flip-link usage, check the MSP for overflow.
                        if rticx_rp2040::export::msp::read() <= ebss {
                            ::core::panic!("Stack overflow after allocating executors (core 0)");
                        }
                    }
                }
            }),
            // Core 1's stack is the `__rticx_internal_CORE1_STACK` static (sized by
            // the `core1_stack` app argument); check the MSP against its base.
            1 => Some(quote! {
                {
                    let stack_base =
                        unsafe { core::ptr::addr_of!(__rticx_internal_CORE1_STACK.mem) } as u32;
                    if rticx_rp2040::export::msp::read() <= stack_base {
                        ::core::panic!("Stack overflow after allocating executors (core 1)");
                    }
                }
            }),
            _ => None,
        }
    }
}
