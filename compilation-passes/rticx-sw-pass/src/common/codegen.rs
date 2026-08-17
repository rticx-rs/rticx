//! Shared codegen infrastructure for the software-task compilation passes.
//!
//! Consumed by both `rticx-sw-pass` (the base crate) and `rticx-async-pass`
//! (the extension).  These items are pass-internal plumbing, **not** a stable
//! public API.

use crate::SwPassBackend;
use heck::ToSnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Path, parse_quote};

/// Name of the core-local interrupt-pending function.
pub const SC_PEND_FN_NAME: &str = "__rticx_local_irq_pend";
/// Name of the cross-core interrupt-pending function.
pub const MC_PEND_FN_NAME: &str = "__rticx_cross_irq_pend";

/// Compute the name of the core-local pend function for `core`.
///
/// In single-core apps the function keeps the plain name
/// (`__rticx_local_irq_pend`); for multi-core apps the core index is appended
/// (`__rticx_local_irq_pend_core{N}`).
pub fn local_pend_fn_ident(core: u32, num_cores: usize) -> Ident {
    if num_cores == 1 {
        format_ident!("{SC_PEND_FN_NAME}")
    } else {
        format_ident!("{SC_PEND_FN_NAME}_core{core}")
    }
}

/// Compute the name of the cross-core pend function for `core`.
pub fn cross_pend_fn_ident(core: u32) -> Ident {
    format_ident!("{MC_PEND_FN_NAME}_core{core}")
}

/// Used for statics (task instance handles): uppercase snake case of the ident.
pub fn ident_uppercase(ident: &Ident) -> Ident {
    let name = ident.to_string().to_snake_case().to_uppercase();
    Ident::new(&name, Span::call_site())
}

pub fn priority_ty_ident(priority: u16, core: u32) -> Ident {
    format_ident!("Core{core}Prio{priority}Tasks")
}

pub fn dispatcher_ident(priority: u16, core: u32) -> Ident {
    format_ident!("Core{core}Priority{priority}Dispatcher")
}

pub fn priority_queue_ident(prio_ty: &Ident) -> Ident {
    format_ident!("__rticx_internal__{prio_ty}__RQ")
}

pub fn sw_task_inputs_ident(task_ident: &Ident) -> Ident {
    format_ident!("__rticx_internal__{task_ident}__INPUTS")
}

/// Compute the interrupt type path for the dispatcher on a given core.
///
/// Uses the backend's `custom_interrupt_path` if provided, otherwise falls
/// back to `pac[core]::Interrupt`.
pub fn get_interrupt_path<B: SwPassBackend + ?Sized>(
    backend: &B,
    pacs: &[Path],
    core: u32,
) -> Path {
    let pac = &pacs[core as usize];
    backend
        .custom_interrupt_path(core)
        .unwrap_or_else(|| parse_quote!(#pac::Interrupt))
}

/// Generate the core-local interrupt-pending functions.
///
/// One function is generated per core.  `cores` iterates `(core index,
/// interrupt type path)` pairs and `num_cores` is the total core count (the
/// function name includes the core index only in multi-core apps).
pub fn generate_local_pend_fns<B: SwPassBackend + ?Sized>(
    backend: &B,
    cores: impl Iterator<Item = (u32, Path)>,
    num_cores: usize,
) -> TokenStream {
    let fns: Vec<TokenStream> = cores
        .map(|(core, interrupt_ty)| {
            let fn_ident = local_pend_fn_ident(core, num_cores);
            let empty_body_fn = parse_quote! {
                #[doc(hidden)]
                #[inline]
                pub fn #fn_ident(irq_nbr: #interrupt_ty) {
                    // To be implemented by distributor
                    // example:
                    // NVIC::pend( irq );
                }
            };
            let fn_def = backend.generate_local_pend_fn(core, empty_body_fn);
            quote!(#fn_def)
        })
        .collect();
    quote!(#(#fns)*)
}

/// Generate the cross-core interrupt-pending functions.
///
/// One function is generated per *target* core that actually has cross-core
/// tasks.  `cores` iterates `(target core index, interrupt type path)` pairs.
pub fn generate_cross_pend_fns<B: SwPassBackend + ?Sized>(
    backend: &B,
    cores: impl Iterator<Item = (u32, Path)>,
) -> TokenStream {
    let fns: Vec<TokenStream> = cores
        .filter_map(|(core, interrupt_ty)| {
            let fn_ident = cross_pend_fn_ident(core);
            let empty_body_fn = parse_quote! {
                #[doc(hidden)]
                #[inline]
                pub fn #fn_ident(irq_nbr: #interrupt_ty) -> Result<(), ()>{
                    // To be implemented by distributor
                    // How do you pend an interrupt on the other core ?
                }
            };
            backend
                .generate_cross_pend_fn(core, empty_body_fn)
                .map(|fn_def| quote!(#fn_def))
        })
        .collect();
    quote!(#(#fns)*)
}

/// Parameters for [`generate_spawn_api`], the shared template that emits the
/// `spawn` / `cross_spawn` API plus the task's input-queue static.
pub struct SpawnApiParams<'a> {
    /// The task struct ident.
    pub task_name: &'a Ident,
    /// The spawn input type, e.g. `<Task as RticSwTask>::SpawnInput`.
    pub inputs_ty: &'a TokenStream,
    /// The priority-group enum type, e.g. `Core0Prio2Tasks`.
    pub prio_ty: &'a Ident,
    /// The ready-queue static ident.
    pub ready_queue_name: &'a Ident,
    /// Path to the SPSC queue type.
    pub queue_path: &'a Path,
    /// The input queue is a ring buffer of `capacity + 1` slots.
    pub queue_buffer_size: usize,
    /// The "system initialized" flag static ident
    /// (`__rticx_sw_system_initialized` / `__rticx_async_system_initialized`).
    pub system_initialized_flag: &'a Ident,
    /// Generate `cross_spawn` (cross-core) instead of `spawn` (core-local).
    pub cross: bool,
    /// The interrupt-pending statement, e.g.
    /// `#pend_fn(#interrupt_ty::#dispatcher_irq_name);`.  `None` when no
    /// interrupt needs to be pended (async priority-0 idle executor).
    pub pend_stmt: Option<TokenStream>,
    /// Optional runtime check that the caller runs on the expected core.
    pub core_check: Option<TokenStream>,
}

/// Generate the `spawn`/`cross_spawn` API for a single software task,
/// including its input-queue static.
pub fn generate_spawn_api(p: &SpawnApiParams) -> TokenStream {
    let task_name = p.task_name;
    let task_inputs_queue = sw_task_inputs_ident(task_name);
    let inputs_ty = p.inputs_ty;
    let prio_ty = p.prio_ty;
    let ready_queue_name = p.ready_queue_name;
    let queue_path = p.queue_path;
    let queue_buffer_size = p.queue_buffer_size;
    let system_initialized_flag = p.system_initialized_flag;
    let critical_section_fn = format_ident!("{}", rticx_core::rticx_functions::INTERRUPT_FREE_FN);
    let core_check = &p.core_check;
    let pend_stmt = &p.pend_stmt;

    if p.cross {
        quote! {
            static mut #task_inputs_queue: #queue_path<#inputs_ty, #queue_buffer_size> = #queue_path::new();

            impl #task_name {
                /// Cross-core spawn: enqueue `input` to this task, which executes on
                /// `core`, from the core specified using `spawn_by`.
                /// ## Returns:
                /// - Ok(()), the inputs are enqueued successfully and the task's dispatcher interrupt is successfully pended
                /// - Err(None), the inputs are enqueued the inputs are enqueued successfully but and the task's dispatcher interrupt pendeding failed.
                /// Either repend it manually or try at a later time.
                /// - Err(Some(input)), the inputs failed to be enqueued. Consider increasing the channel capacity using `capacity = N`.
                /// `Err(Some(input))` is also returned when the caller is not executing on the `spawn_by` core. (in Multicore)
                pub fn cross_spawn(input : #inputs_ty) -> Result<(), Option<#inputs_ty>> {
                    #core_check
                    #[allow(static_mut_refs)]
                    let mut inputs_producer = unsafe {#task_inputs_queue.split().0};
                    let mut ready_producer = unsafe {#ready_queue_name.split().0};
                    // need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                    #critical_section_fn(|| -> Result<(), Option<#inputs_ty>>  {
                        if unsafe { !#system_initialized_flag } {
                            return Err(Some(input));
                        }
                        // enqueue inputs
                        inputs_producer.enqueue(input).map_err(Option::Some)?;
                        // enqueue task to ready queue
                        unsafe {ready_producer.enqueue_unchecked(#prio_ty::#task_name)};
                        // pend dispatcher
                        #pend_stmt
                    })
                }
            }
        }
    } else {
        quote! {
            static mut #task_inputs_queue: #queue_path<#inputs_ty, #queue_buffer_size> = #queue_path::new();

            impl #task_name {
                pub fn spawn(input : #inputs_ty) -> Result<(), #inputs_ty> {
                    #core_check
                    #[allow(static_mut_refs)]
                    let mut inputs_producer = unsafe {#task_inputs_queue.split().0};
                    let mut ready_producer = unsafe {#ready_queue_name.split().0};
                    // need to protect by a critical section because many producers of different priorities can spawn/enqueue this task
                    #critical_section_fn(|| -> Result<(), #inputs_ty>  {
                        if unsafe { !#system_initialized_flag } {
                            return Err(input);
                        }
                        // enqueue inputs
                        inputs_producer.enqueue(input)?;
                        // enqueue task to ready queue
                        unsafe {ready_producer.enqueue_unchecked(#prio_ty::#task_name)};
                        // pend dispatcher
                        #pend_stmt
                        Ok(())
                    })
                }
            }
        }
    }
}
