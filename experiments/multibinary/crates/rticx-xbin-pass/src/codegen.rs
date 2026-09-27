//! Phase-2 code generation for the cross-binary tasks of this application
//! (M3-T1 sender side, M3-T2 receiver side).
//!
//! In codegen mode the pass has loaded the driver-generated `system.json`
//! (see `multibinary-multicore-plan.md` §8) and emits code from it:
//!
//! - for every `#[cross_bin_spawn(..)]` stub (M3-T1):
//!   - a hidden **FIFO view** helper returning the fixed-address
//!     `rticx_xbin_rt::Fifo` of the task at
//!     `region.base_for(this_core, source, target) + offset`, where the region
//!     comes from the distribution's runtime backend and the offset, depth and
//!     element layout come from the system view (const addresses, no local input
//!     queue, no forwarder);
//!   - `Task::cross_spawn(input)`, matching the error semantics of the
//!     single-binary `cross_spawn`:
//!     - `Ok(())`: the input is enqueued and the target doorbell was rung;
//!     - `Err(None)`: the input is enqueued, but ringing the doorbell failed
//!       (do not retry the enqueue; re-notify the target);
//!     - `Err(Some(input))`: nothing was enqueued (the FIFO is full, the caller
//!       does not run on the expected core, …); retry later or raise `capacity`.
//! - for every `#[cross_bin_task(..)]` receiver (M3-T2):
//!   - a hidden **FIFO view** helper like the sender's, but resolving the
//!     consumer view of the region (`base_from_target`);
//!   - one generated **dispatcher** task per doorbell line: a hardware task
//!     bound to the distribution's doorbell IRQ at the line's priority, whose
//!     `exec` drains the FIFOs of the line directly (single consumer) and
//!     calls the receiver task's `exec(input)`.
//!
//! The receiver structs themselves are turned into framework tasks by
//! [`crate::parse::inject_receiver_tasks`]: `#[task(priority = …,
//! core = …, task_trait = CrossBinTask, init = generated)]`, the same shape
//! the single-binary software-task pass uses. The core pass then generates the
//! task static, runs the user's `impl CrossBinTask`, and checks the trait
//! implementation — no core-pass changes are required.
//!
//! - **Init hooks** (M3-T3): codegen mode also emits three hook functions and
//!   wires them into the generated entry functions through
//!   [`rticx_core::RticPass::main_injection`]:
//!   - `__rticx_xbin_configure_shared_memory` runs on every core at the start
//!     of its entry (`MainInjectionPoint::BeforeInit`, before the user
//!     `init`): MPU/MMU attributes are per-core, so each core maps its own
//!     view of the IPC regions Normal, Non-cacheable, Shareable;
//!   - `__rticx_xbin_init_shared` is generated for the application owning the
//!     project's **owner core** (the lowest global core id, i.e. the boot
//!     core). It runs `init_shared()` and zeroes the ring indices of every
//!     task FIFO, before the owner's `mark_ready` and before any peer may
//!     rely on the shared memory (`MainInjectionPoint::BeforePostInit`);
//!   - `__rticx_xbin_mark_ready_core<N>` runs on every core at the end of its
//!     `post_init` (`MainInjectionPoint::BeforeIdle`): it arms the doorbell
//!     lines targeting that core (`doorbell_setup`) and then calls
//!     `mark_ready(core)`, so a core is never advertised ready before its
//!     doorbells are armed.
//!
//!   Boot sequencing itself stays distribution-owned (for example the H7
//!   release of the M4 core): the owner must run its init before peers rely
//!   on the shared state.
//!
//! The enqueue happens inside `__rticx_interrupt_free`, the v1 sender-side
//! lock: the transport itself is kept independent of the lock mechanism, so a
//! future `Producer` shared-resource API (SRP) can replace it in one place.
//! The dispatcher runs under the doorbell interrupt, so the consumer side
//! needs no lock: the SPSC discipline gives it the only `dequeue` call.
//!
//! The generated code only uses the frozen public surface of `rticx-core`
//! (`__rticx_interrupt_free`, `main_injection`), the generated `ipc-types` and
//! the distribution's re-export of `rticx-xbin-rt`, so no root-workspace API
//! changes are required.
//!
//! [`CrossBinBackend`]: rticx_xbin_rt::backend::CrossBinBackend

use std::collections::BTreeMap;
use std::path::Path;

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rticx_core::parser::ast::uppercase_ident;
use rticx_core::rticx_traits::HWT_TRAIT_TY;
use rticx_xbin_proto::{
    AppEntry, DoorbellEntry, Hash64, ReceiverDecl, SenderDecl, SystemView, TaskEntry,
    simple_type_name,
};
use syn::{Item, LitInt, LitStr, Type};

use crate::parse::AppExtensions;

/// Code-generation configuration of the cross-binary extension.
///
/// A distribution that binds [`crate::XbinPass`] implements this trait to tell
/// the pass how the generated code reaches the distribution's runtime backend
/// and the `rticx-xbin-rt` items it re-exports. It is the code-generation
/// counterpart of `rticx_xbin_rt::CrossBinBackend`.
pub trait XbinPassBackend {
    /// Expression evaluating to the distribution's runtime backend.
    ///
    /// The expression must yield a **value** (not a reference) implementing
    /// `rticx_xbin_rt::CrossBinBackend`; generated code does
    /// `let backend = <expression>;` and then calls its methods. Typical
    /// implementations name a stateless unit struct
    /// (`parse_quote!(rticx_h7::xbin::Backend)`) or call a constructor
    /// (`parse_quote!(rticx_h7::xbin::backend())`). The expression is expanded
    /// **inside the `#[app]` module**, so it may rely on the application's
    /// imports but not on private items of the distribution macro.
    fn backend(&self) -> syn::Expr;

    /// Path to the `rticx-xbin-rt` items (`Fifo`, `CrossBinBackend`,
    /// `FIFO_ALIGN`, …) as re-exported by the distribution.
    ///
    /// The generated code expands to `#rt_path::Fifo` and
    /// `#rt_path::CrossBinBackend`, so distributions normally re-export the
    /// runtime crate (for example `rticx_h7::export::xbin_rt`).
    fn rt_path(&self) -> syn::Path;

    /// Path to the generated `ipc-types` crate as seen by applications.
    ///
    /// Used for sender stubs that do not mirror the receiver with an
    /// `impl CrossBinSpawn { type Input = …; }` block: the IDL input type is
    /// then reached as `#ipc_types_path::<InputType>`.
    fn ipc_types_path(&self) -> syn::Path {
        syn::parse_quote!(ipc_types)
    }

    /// Identifier of the interrupt handler bound to doorbell `line` of
    /// `target` — the receiver dispatcher's ISR.
    ///
    /// The generated dispatcher is a `#[task(binds = <ident>, …)]`, so the
    /// identifier must resolve inside the `#[app]` module (typically a PAC
    /// interrupt name the distribution re-exports or imports there).
    fn dispatcher_irq(&self, target: u32, line: u32) -> syn::Ident;

    /// Numeric IRQ of the interrupt handler `dispatcher_irq(target, line)`
    /// names.
    ///
    /// Used by the init hook of `target`'s application to arm the line
    /// (`CrossBinBackend::doorbell_setup`) before the core is marked ready
    /// (M3-T3). It must denote the same interrupt as
    /// [`Self::dispatcher_irq`].
    fn doorbell_irq(&self, target: u32, line: u32) -> u16;
}

/// The init hooks generated for one application (M3-T3).
pub(crate) struct InitHooks {
    /// Hook functions appended to the `#[app]` module.
    pub(crate) items: Vec<Item>,
    /// Per-core wiring consumed by [`crate::XbinPass`]'s `main_injection`.
    pub(crate) plan: HookPlan,
}

/// Per-core init plan of one application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookPlan {
    /// Local core index running `__rticx_xbin_init_shared`, when this
    /// application owns the project's owner core.
    owner_local: Option<u32>,
    /// Global core id of each local core index (`core_ids[local]`).
    global_ids: Vec<u32>,
}

impl HookPlan {
    /// Returns the tokens to inject at `point` for the entry function of the
    /// local `core`, using `backend` as the runtime backend expression.
    pub(crate) fn injection(
        &self,
        point: &rticx_core::MainInjectionPoint,
        core: u32,
        backend: &dyn XbinPassBackend,
    ) -> Option<TokenStream> {
        match point {
            rticx_core::MainInjectionPoint::BeforeInit => {
                self.global_ids.get(core as usize)?;
                let backend_expr = backend.backend();
                Some(quote! {
                    __rticx_xbin_configure_shared_memory(&#backend_expr);
                })
            }
            rticx_core::MainInjectionPoint::BeforePostInit if self.owner_local == Some(core) => {
                let backend_expr = backend.backend();
                Some(quote! {
                    __rticx_xbin_init_shared(&#backend_expr);
                })
            }
            rticx_core::MainInjectionPoint::BeforeIdle => {
                self.global_ids.get(core as usize)?;
                let mark_ready_fn = format_ident!("__rticx_xbin_mark_ready_core{core}");
                let backend_expr = backend.backend();
                Some(quote! {
                    #mark_ready_fn(&#backend_expr);
                })
            }
            _ => None,
        }
    }
}

/// Generates the sender items for every `#[cross_bin_spawn]` stub of an
/// application: one hidden FIFO view per task plus its `cross_spawn` impl.
///
/// Returns an error naming the offending declaration when the application is
/// not part of `view`, a sender has no matching task, or the sender
/// declaration disagrees with the synced view — all of which mean the source
/// changed after the last `cargo xbin sync`.
pub(crate) fn generate_sender_items(
    view: &SystemView,
    package: &str,
    target: &str,
    extensions: &AppExtensions,
    senders: &[SenderDecl],
    backend: Option<&dyn XbinPassBackend>,
) -> syn::Result<Vec<Item>> {
    if senders.is_empty() {
        return Ok(Vec::new());
    }
    let Some(backend) = backend else {
        return Err(error(
            "this application declares `#[cross_bin_spawn]` tasks, but the distribution did not \
             configure a cross-binary code-generation backend; bind \
             `XbinPass::from_env().with_backend(...)` in the distribution macro",
        ));
    };

    let application = check_application(view, package, target, extensions)?;

    let mut items = Vec::with_capacity(senders.len() * 2);
    for sender in senders {
        items.extend(generate_sender(view, application, sender, backend)?);
    }
    Ok(items)
}

/// Generates the receiver items for every `#[cross_bin_task]` declaration of
/// an application (M3-T2): one hidden FIFO view per task plus one doorbell
/// dispatcher per `(source -> target, priority)` line.
///
/// Returns an error naming the offending declaration when the application is
/// not part of `view`, a declaration disagrees with the synced view, or the
/// view places a task on this application's cores without a matching
/// declaration — all of which mean the source changed after the last
/// `cargo xbin sync`.
pub(crate) fn generate_receiver_items(
    view: &SystemView,
    package: &str,
    target: &str,
    extensions: &AppExtensions,
    receivers: &[ReceiverDecl],
    backend: Option<&dyn XbinPassBackend>,
) -> syn::Result<Vec<Item>> {
    if receivers.is_empty() {
        return Ok(Vec::new());
    }
    let Some(backend) = backend else {
        return Err(error(
            "this application declares `#[cross_bin_task]` tasks, but the distribution did not \
             configure a cross-binary code-generation backend; bind \
             `XbinPass::from_env().with_backend(...)` in the distribution macro",
        ));
    };

    let application = check_application(view, package, target, extensions)?;

    // A view task owned by this application without a receiver declaration
    // would otherwise be silently undrained after a `sync` removed the task
    // from the source.
    for task in &view.tasks {
        let owned = view
            .cores
            .iter()
            .any(|core| core.global_id == task.receiver_core && core.app == application.package);
        if owned && !receivers.iter().any(|receiver| receiver.name == task.name) {
            return Err(error(format!(
                "the synced system view has task `{}` for application `{package}`, but the source \
                 declares no `#[cross_bin_task]` for it; run `cargo xbin sync`",
                task.name
            )));
        }
    }

    // One dispatcher per doorbell line: the tasks are grouped by
    // `(source, target, priority)`, the key of their `system.json` doorbell.
    let mut lines: BTreeMap<(u32, u32, u16), Vec<ResolvedReceiver<'_>>> = BTreeMap::new();
    for receiver in receivers {
        let resolved = resolve_receiver(view, application, receiver)?;
        let task = resolved.task;
        lines
            .entry((task.fifo.source, task.fifo.target, task.priority))
            .or_default()
            .push(resolved);
    }

    let mut items = Vec::with_capacity(receivers.len() * 2);
    for ((source, target_core, priority), group) in &lines {
        items.extend(generate_dispatcher(
            group,
            *source,
            *target_core,
            *priority,
            backend,
        )?);
    }
    Ok(items)
}

/// Generates the init hooks of one application (M3-T3): the per-core
/// `__rticx_xbin_configure_shared_memory`, the owner's
/// `__rticx_xbin_init_shared` and the per-core
/// `__rticx_xbin_mark_ready_core<N>`.
///
/// The returned [`HookPlan`] is stashed by the pass and consulted from
/// [`rticx_core::RticPass::main_injection`] to wire the hooks into the
/// generated entry functions. All hooks go through the distribution backend;
/// no target-specific code is generated here.
pub(crate) fn generate_init_hooks(
    view: &SystemView,
    application: &AppEntry,
    backend: &dyn XbinPassBackend,
) -> syn::Result<InitHooks> {
    let rt_path = backend.rt_path();
    let owner = owner_core(view);
    let owner_local = owner.and_then(|owner| {
        application
            .core_ids
            .iter()
            .position(|&global| global == owner)
            .map(|local| local as u32)
    });

    // MPU/MMU attributes are per-core: every core configures its own view of
    // the regions before any shared access (before the user `init` runs).
    let configure_doc = "Configures this core's view of the IPC regions as Normal, Non-cacheable, \
         Shareable before any shared access (M3-T3).";
    let mut items = Vec::with_capacity(application.core_ids.len() + 2);
    items.push(syn::parse_quote! {
        #[doc = #configure_doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn __rticx_xbin_configure_shared_memory(
            __rticx_xbin_backend: &impl #rt_path::CrossBinBackend,
        ) {
            __rticx_xbin_backend.configure_shared_memory();
        }
    });

    for (local, &global) in application.core_ids.iter().enumerate() {
        let setups: Vec<TokenStream> = view
            .doorbells
            .iter()
            .filter(|doorbell| doorbell.target == global)
            .map(|doorbell| {
                let line = doorbell.line;
                let irq = LitInt::new(
                    &format!("{}u16", backend.doorbell_irq(global, line)),
                    Span::call_site(),
                );
                let setup_error = format!(
                    "failed to arm doorbell line {line} of global core {global}; the distribution \
                     must implement `CrossBinBackend::doorbell_setup` for it"
                );
                quote! {
                    __rticx_xbin_backend
                        .doorbell_setup(#global, #line, #irq)
                        .expect(#setup_error);
                }
            })
            .collect();

        let mark_ready_doc = format!(
            "Marks this core (global core {global}) ready at the end of its `post_init`: arms its \
             doorbell lines and publishes its ready bit (M3-T3)."
        );
        let mark_ready_fn = format_ident!("__rticx_xbin_mark_ready_core{local}");
        items.push(syn::parse_quote! {
            #[doc = #mark_ready_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #mark_ready_fn(__rticx_xbin_backend: &impl #rt_path::CrossBinBackend) {
                #(#setups)*
                __rticx_xbin_backend.mark_ready(#global);
            }
        });
    }

    if let Some(owner) = owner
        && owner_local.is_some()
    {
        let zero_fifos = generate_fifo_inits(view, owner, backend)?;
        let init_doc = format!(
            "Initializes the shared ready/epoch state and zeroes the ring indices of every task \
             FIFO from the owner core (global core {owner}); runs before the owner is marked ready \
             (M3-T3)."
        );
        items.push(syn::parse_quote! {
            #[doc = #init_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn __rticx_xbin_init_shared(
                __rticx_xbin_backend: &impl #rt_path::CrossBinBackend,
            ) {
                __rticx_xbin_backend.init_shared();
                #(#zero_fifos)*
            }
        });
    }

    Ok(InitHooks {
        items,
        plan: HookPlan {
            owner_local,
            global_ids: application.core_ids.clone(),
        },
    })
}

/// Generates the FIFO-zeroing blocks of the owner's `init_shared`.
///
/// Only regions the owner is an endpoint of can be addressed through
/// [`rticx_xbin_rt::backend::IpcRegion::base_for`]. v1 (one pair of binaries)
/// has a region per direction and the owner is an endpoint of every one of
/// them; a wider topology needs a per-region initializer and is left to
/// M5-T1.
fn generate_fifo_inits(
    view: &SystemView,
    owner: u32,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<TokenStream>> {
    let rt_path = backend.rt_path();
    let ipc_types = backend.ipc_types_path();

    let mut blocks = Vec::with_capacity(view.tasks.len());
    for task in &view.tasks {
        let source = task.fifo.source;
        let target = task.fifo.target;
        if source != owner && target != owner {
            // TODO(M5-T1): multi-pair topologies need the region's initializer
            // to come from one of its endpoints.
            continue;
        }

        let type_ident = ident(&task.input_type)?;
        let offset = task.fifo.offset as usize;
        let depth = task.fifo.depth as usize;
        let elem_size = task.fifo.elem_size as usize;
        let elem_align = input_align(view, task)? as usize;

        let missing_region = format!(
            "`cargo xbin sync` allocated the `({source} -> {target})` IPC region; re-run it \
             after changing `rticx.toml`"
        );
        let wrong_core =
            format!("the owner core is not an endpoint of the `({source} -> {target})` IPC region");
        blocks.push(quote! {
            {
                const __RTICX_XBIN_SOURCE_CORE: u32 = #source;
                const __RTICX_XBIN_TARGET_CORE: u32 = #target;
                const __RTICX_XBIN_FIFO_OFFSET: usize = #offset;

                const _: () = assert!(
                    __RTICX_XBIN_FIFO_OFFSET % #rt_path::FIFO_ALIGN == 0,
                    "`cargo xbin sync` allocated a misaligned FIFO offset",
                );
                const _: () = assert!(
                    core::mem::size_of::<#ipc_types::#type_ident>() == #elem_size,
                    "the IDL layout of the spawn input changed; run `cargo xbin sync`",
                );
                const _: () = assert!(
                    core::mem::align_of::<#ipc_types::#type_ident>() == #elem_align,
                    "the IDL layout of the spawn input changed; run `cargo xbin sync`",
                );

                let __rticx_xbin_region = __rticx_xbin_backend
                    .ipc_region(__RTICX_XBIN_SOURCE_CORE, __RTICX_XBIN_TARGET_CORE)
                    .expect(#missing_region);
                let __rticx_xbin_base = __rticx_xbin_region
                    .base_for(
                        __rticx_xbin_backend.current_global_core_id(),
                        __RTICX_XBIN_SOURCE_CORE,
                        __RTICX_XBIN_TARGET_CORE,
                    )
                    .expect(#wrong_core);

                // SAFETY: `system.json` places the FIFO at `offset`, aligned,
                // inside the region; no producer or consumer runs before the
                // owner marks itself ready, and `Fifo::init` documents why it
                // must not race one.
                unsafe {
                    (*#rt_path::Fifo::<#ipc_types::#type_ident, #depth>::view_at(
                        __rticx_xbin_base + __RTICX_XBIN_FIFO_OFFSET,
                    ))
                    .init();
                }
            }
        });
    }
    Ok(blocks)
}

/// Returns the project's owner core: the lowest global core id, i.e. the boot
/// core (CM7 on the H7 acceptance target).
///
/// The owner runs `init_shared` before the other cores rely on the shared
/// memory. The distribution owns the boot sequencing that guarantees it.
// TODO(M5): let a distribution or `rticx.toml` designate a different owner.
fn owner_core(view: &SystemView) -> Option<u32> {
    view.cores.iter().map(|core| core.global_id).min()
}

/// One receiver declaration resolved against the system view.
struct ResolvedReceiver<'a> {
    /// The synced task the receiver executes.
    task: &'a TaskEntry,
    /// The doorbell line of the task's `(source, target, priority)` key.
    doorbell: &'a DoorbellEntry,
    /// Local core index the task runs on (`0..cores`).
    local_core: u32,
    /// Rust type of the spawn input, as declared by the user.
    input: Type,
    /// Canonical alignment of the input type (`system.json`).
    elem_align: u32,
}

/// Resolves `receiver` against the view, checking every field the synced
/// topology pins down.
fn resolve_receiver<'a>(
    view: &'a SystemView,
    application: &AppEntry,
    receiver: &ReceiverDecl,
) -> syn::Result<ResolvedReceiver<'a>> {
    let task = view
        .tasks
        .iter()
        .find(|task| task.name == receiver.name)
        .ok_or_else(|| {
            error(format!(
                "the synced system view has no task named `{}`; run `cargo xbin sync`",
                receiver.name
            ))
        })?;

    let owned = view
        .cores
        .iter()
        .any(|core| core.global_id == task.receiver_core && core.app == application.package);
    if !owned {
        return Err(error(format!(
            "task `{}` runs on global core {}, which is not part of application `{}`; \
             run `cargo xbin sync`",
            task.name, task.receiver_core, application.package
        )));
    }

    match application.core_ids.get(receiver.core as usize) {
        Some(&global) if global == task.receiver_core => {}
        Some(&global) => {
            return Err(error(format!(
                "receiver `{}` declares local core {}, which maps to global core {global}, but the \
                 synced system view runs it on global core {}; run `cargo xbin sync`",
                receiver.name, receiver.core, task.receiver_core
            )));
        }
        None => {
            return Err(error(format!(
                "receiver `{}` declares local core {}, but the synced system view maps {} core(s) \
                 for application `{}`; run `cargo xbin sync`",
                receiver.name,
                receiver.core,
                application.core_ids.len(),
                application.package
            )));
        }
    }

    if receiver.priority != task.priority {
        return Err(error(format!(
            "receiver `{}` declares priority {}, but the synced system view has {}; \
             run `cargo xbin sync`",
            receiver.name, receiver.priority, task.priority
        )));
    }
    if receiver.capacity != task.capacity {
        return Err(error(format!(
            "receiver `{}` declares capacity {}, but the synced system view has {}; \
             run `cargo xbin sync`",
            receiver.name, receiver.capacity, task.capacity
        )));
    }
    if simple_type_name(&receiver.input_type) != task.input_type {
        return Err(error(format!(
            "receiver `{}` declares input type `{}`, but the synced system view has `{}`; \
             run `cargo xbin sync`",
            receiver.name, receiver.input_type, task.input_type
        )));
    }
    if let Some(spawned_by) = &receiver.spawned_by {
        let mut declared = spawned_by.clone();
        declared.sort_unstable();
        if declared != task.spawner_cores {
            return Err(error(format!(
                "receiver `{}` declares `spawned_by = {spawned_by:?}`, but the synced system view \
                 has {:?}; run `cargo xbin sync`",
                receiver.name, task.spawner_cores
            )));
        }
    }

    let doorbell = view
        .doorbells
        .iter()
        .find(|doorbell| {
            doorbell.source == task.fifo.source
                && doorbell.target == task.fifo.target
                && doorbell.priority == task.priority
        })
        .ok_or_else(|| {
            error(format!(
                "the synced system view has no doorbell for `{}` ({} -> {} at priority {}); \
                 run `cargo xbin sync`",
                task.name, task.fifo.source, task.fifo.target, task.priority
            ))
        })?;

    let input = syn::parse_str(&receiver.input_type).map_err(|parse_error| {
        error(format!(
            "cannot parse the input type `{}` of receiver `{}`: {parse_error}",
            receiver.input_type, receiver.name
        ))
    })?;

    Ok(ResolvedReceiver {
        task,
        doorbell,
        local_core: receiver.core,
        input,
        elem_align: input_align(view, task)?,
    })
}

/// Generates the dispatcher of one doorbell line: a hardware task bound to the
/// distribution's doorbell IRQ at the line's priority, plus the hidden FIFO
/// view of every task it drains.
fn generate_dispatcher(
    group: &[ResolvedReceiver<'_>],
    source: u32,
    target_core: u32,
    priority: u16,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<Item>> {
    let first = group.first().expect("a dispatcher group is never empty");
    let line = first.doorbell.line;
    let local_core = first.local_core;
    let irq = backend.dispatcher_irq(target_core, line);
    let backend_expr = backend.backend();
    let dispatcher_ty = format_ident!(
        "__RticxXbinDispatcher{}To{}P{}",
        source,
        target_core,
        priority
    );
    let priority_lit = LitInt::new(&priority.to_string(), Span::call_site());
    let core_lit = LitInt::new(&local_core.to_string(), Span::call_site());
    let task_trait = format_ident!("{HWT_TRAIT_TY}");

    let mut items = Vec::with_capacity(group.len() + 1);
    let mut drains = Vec::with_capacity(group.len());
    for resolved in group {
        items.push(generate_receiver_fifo_view(resolved, backend)?);

        let task = resolved.task;
        let task_ident = ident(&task.name)?;
        let task_static = uppercase_ident(&task_ident);
        let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
        drains.push(quote! {
            {
                let __rticx_xbin_fifo = #fifo_fn(&__rticx_xbin_backend);
                // SAFETY: this dispatcher is the single consumer of the FIFO
                // (one producer core, one consumer dispatcher) and the FIFO
                // lives at its synced, aligned address inside the region.
                unsafe {
                    while let Some(input) = (*__rticx_xbin_fifo).dequeue() {
                        // SAFETY: the core pass initialized the task static
                        // during `init` (`init = generated`).
                        #task_static.assume_init_mut().exec(input);
                    }
                }
            }
        });
    }

    let doc = format!(
        "Doorbell dispatcher for the `({source} -> {target_core})` priority-{priority} line: \
         drains its per-task FIFOs and runs the receiver tasks."
    );
    items.push(syn::parse_quote! {
        #[doc = #doc]
        #[doc(hidden)]
        #[task(binds = #irq, priority = #priority_lit, core = #core_lit, init = generated)]
        pub struct #dispatcher_ty;
    });
    items.push(syn::parse_quote! {
        impl #task_trait for #dispatcher_ty {
            fn exec(&mut self) {
                let __rticx_xbin_backend = #backend_expr;
                #(#drains)*
            }
        }
    });
    Ok(items)
}

/// Generates the hidden FIFO view of one resolved receiver, mirroring the
/// sender-side helper but resolving the `base_from_target` view of the region.
fn generate_receiver_fifo_view(
    resolved: &ResolvedReceiver<'_>,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Item> {
    let task = resolved.task;
    let input_ty = &resolved.input;
    let rt_path = backend.rt_path();

    let source = task.fifo.source;
    let target_core = task.fifo.target;
    let offset = task.fifo.offset as usize;
    let depth = task.fifo.depth as usize;
    let elem_size = task.fifo.elem_size as usize;
    let elem_align = resolved.elem_align as usize;

    let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
    let fifo_doc = format!(
        "Returns this application's consumer view of the `{}` FIFO, placed at its `({source} -> \
         {target_core})` region offset from `system.json`.",
        task.name
    );
    let missing_region = format!(
        "`cargo xbin sync` allocated the `({source} -> {target_core})` IPC region; re-run it \
         after changing `rticx.toml`"
    );
    let wrong_core = format!(
        "the current core is not an endpoint of the `({source} -> {target_core})` IPC region"
    );

    Ok(syn::parse_quote! {
        #[doc = #fifo_doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #fifo_fn(
            __rticx_xbin_backend: &impl #rt_path::CrossBinBackend,
        ) -> *mut #rt_path::Fifo<#input_ty, #depth> {
            const __RTICX_XBIN_SOURCE_CORE: u32 = #source;
            const __RTICX_XBIN_TARGET_CORE: u32 = #target_core;
            const __RTICX_XBIN_FIFO_OFFSET: usize = #offset;

            const _: () = assert!(
                __RTICX_XBIN_FIFO_OFFSET % #rt_path::FIFO_ALIGN == 0,
                "`cargo xbin sync` allocated a misaligned FIFO offset",
            );
            const _: () = assert!(
                core::mem::size_of::<#input_ty>() == #elem_size,
                "the IDL layout of the spawn input changed; run `cargo xbin sync`",
            );
            const _: () = assert!(
                core::mem::align_of::<#input_ty>() == #elem_align,
                "the IDL layout of the spawn input changed; run `cargo xbin sync`",
            );

            let __rticx_xbin_region = __rticx_xbin_backend
                .ipc_region(__RTICX_XBIN_SOURCE_CORE, __RTICX_XBIN_TARGET_CORE)
                .expect(#missing_region);
            let __rticx_xbin_base = __rticx_xbin_region
                .base_for(
                    __rticx_xbin_backend.current_global_core_id(),
                    __RTICX_XBIN_SOURCE_CORE,
                    __RTICX_XBIN_TARGET_CORE,
                )
                .expect(#wrong_core);

            // SAFETY: `system.json` places the FIFO at `offset`, 8-byte
            // aligned, inside the region; `view_at` documents the remaining
            // requirements (one producer, one consumer, initialized memory),
            // which the doorbell/dispatcher protocol upholds.
            unsafe {
                #rt_path::Fifo::<#input_ty, #depth>::view_at(
                    __rticx_xbin_base + __RTICX_XBIN_FIFO_OFFSET,
                )
            }
        }
    })
}

/// Finds the application in `view` and checks that its core mapping still
/// matches the source being compiled.
pub(crate) fn check_application<'a>(
    view: &'a SystemView,
    package: &str,
    target: &str,
    extensions: &AppExtensions,
) -> syn::Result<&'a AppEntry> {
    let application = view
        .apps
        .iter()
        .find(|app| app.package == package && app.target.name == target)
        .ok_or_else(|| {
            error(format!(
                "the synced system view has no application `{package}` (target `{target}`); \
                 run `cargo xbin sync`"
            ))
        })?;

    if application.core_ids.len() != extensions.cores as usize {
        return Err(error(format!(
            "application `{package}` declares `cores = {}`, but the synced system view has {}; \
             run `cargo xbin sync`",
            extensions.cores,
            application.core_ids.len()
        )));
    }
    if let Some(core_ids) = &extensions.core_ids
        && core_ids != &application.core_ids
    {
        return Err(error(format!(
            "application `{package}` maps its local cores to {core_ids:?}, but the synced system \
             view has {:?}; run `cargo xbin sync`",
            application.core_ids
        )));
    }
    Ok(application)
}

/// Rejects an application whose source changed after the last
/// `cargo xbin sync` (M3-T4).
///
/// `system.json` records the [`app_source_hash`](crate::app_source_hash) of
/// every application at sync time. Phase 2 recomputes the hash of the source
/// being compiled and compares it with the recorded one, so a plain `cargo
/// build` after an edit fails with the documented `cargo xbin sync` hint
/// instead of generating code from a stale view.
///
/// The pass runs this check after the declaration checks: a source change that
/// also breaks a task declaration is reported by the more specific mismatch
/// first.
pub(crate) fn check_source_hash(application: &AppEntry, source_hash: Hash64) -> syn::Result<()> {
    if application.source_hash != source_hash {
        return Err(error(format!(
            "application `{}` changed since the last `cargo xbin sync` (the system view records \
             source hash {}, the source hashes to {}); run `cargo xbin sync`",
            application.package, application.source_hash, source_hash
        )));
    }
    Ok(())
}

/// Generates the freshness anchors of one application (M3-T4).
///
/// - `__RTICX_XBIN_TOPOLOGY_HASH` is the `TOPOLOGY_HASH` anchor of plan §7.3:
///   the `topology_hash` of the view the generated code was produced from;
/// - `__RTICX_XBIN_SYSTEM_JSON` is the system view itself, embedded with
///   `include_str!` so rustc records it in the crate's dep-info and rebuilds
///   the application whenever the file changes.
///
/// The anchors complement the hard errors of [`crate::load_system`] (stale
/// `topology_hash`) and [`check_source_hash`] (source changed after `sync`).
pub(crate) fn generate_freshness_items(view: &SystemView, path: &Path) -> syn::Result<Vec<Item>> {
    // `include_str!` resolves a relative path against the file containing the
    // macro invocation, not the compilation working directory, so the path
    // must be absolute. `load_system` already read the file, so canonicalizing
    // normally succeeds; `absolute` only normalizes lexically as a fallback.
    let absolute = path
        .canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let path_lit = absolute
        .to_str()
        .map(|text| LitStr::new(text, Span::call_site()))
        .ok_or_else(|| {
            error(format!(
                "the system view path `{}` is not valid UTF-8; set `{}` to a UTF-8 path",
                path.display(),
                crate::SYSTEM_ENV
            ))
        })?;
    let hash = LitInt::new(
        &format!("0x{:016x}u64", view.topology_hash.get()),
        Span::call_site(),
    );

    let hash_doc = "Topology hash of the `system.json` this application was generated from; a \
         stale view or a source change is a hard error until `cargo xbin sync` runs again (M3-T4).";
    let json_doc = "The synced system view, embedded so rustc records it in dep-info and rebuilds \
         this application when it changes (M3-T4).";
    Ok(vec![
        syn::parse_quote! {
            #[doc = #hash_doc]
            #[doc(hidden)]
            pub const __RTICX_XBIN_TOPOLOGY_HASH: u64 = #hash;
        },
        syn::parse_quote! {
            #[doc = #json_doc]
            #[doc(hidden)]
            #[allow(dead_code)]
            const __RTICX_XBIN_SYSTEM_JSON: &str = include_str!(#path_lit);
        },
    ])
}

/// Generates the FIFO view and the `cross_spawn` impl of one sender stub.
///
/// `items` are pushed into the `#[app]` module, so the generated
/// `impl <Task>` block extends the user-declared unit struct.
fn generate_sender(
    view: &SystemView,
    application: &AppEntry,
    sender: &SenderDecl,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<Item>> {
    let task = find_task(view, application, sender)?;
    let doorbell = view
        .doorbells
        .iter()
        .find(|doorbell| {
            doorbell.source == task.fifo.source
                && doorbell.target == task.fifo.target
                && doorbell.priority == task.priority
        })
        .ok_or_else(|| {
            error(format!(
                "the synced system view has no doorbell for `{}` ({} -> {} at priority {}); \
                 run `cargo xbin sync`",
                task.name, task.fifo.source, task.fifo.target, task.priority
            ))
        })?;

    let task_ident = ident(&task.name)?;
    let input_ty = input_type(task, sender, backend)?;
    let rt_path = backend.rt_path();
    let backend_expr = backend.backend();

    let source = task.fifo.source;
    let target_core = task.fifo.target;
    let line = doorbell.line;
    let offset = task.fifo.offset as usize;
    let depth = task.fifo.depth as usize;
    let elem_size = task.fifo.elem_size as usize;
    let elem_align = input_align(view, task)? as usize;

    let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
    let fifo_doc = format!(
        "Returns this application's view of the `{}` FIFO, placed at its `({source} -> \
         {target_core})` region offset from `system.json`.",
        task.name
    );
    let missing_region = format!(
        "`cargo xbin sync` allocated the `({source} -> {target_core})` IPC region; re-run it \
         after changing `rticx.toml`"
    );
    let wrong_core = format!(
        "the current core is not an endpoint of the `({source} -> {target_core})` IPC region"
    );

    let spawn_doc = format!(
        "Cross-binary spawn: enqueue `input` into the `{}` FIFO and ring the doorbell of global \
         core {target_core}.\n\n\
         # Returns\n\n\
         - `Ok(())`: the input is enqueued and the target dispatcher was notified;\n\
         - `Err(None)`: the input is enqueued, but the doorbell could not be rung. Do **not** \
         retry the spawn; re-notify the target instead;\n\
         - `Err(Some(input))`: nothing was enqueued (the FIFO is full, or the caller does not \
         run on global core {source}). Retry later or raise `capacity`.",
        task.name
    );

    Ok(vec![
        syn::parse_quote! {
            #[doc = #fifo_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #fifo_fn(
                __rticx_xbin_backend: &impl #rt_path::CrossBinBackend,
            ) -> *mut #rt_path::Fifo<#input_ty, #depth> {
                const __RTICX_XBIN_SOURCE_CORE: u32 = #source;
                const __RTICX_XBIN_TARGET_CORE: u32 = #target_core;
                const __RTICX_XBIN_FIFO_OFFSET: usize = #offset;

                const _: () = assert!(
                    __RTICX_XBIN_FIFO_OFFSET % #rt_path::FIFO_ALIGN == 0,
                    "`cargo xbin sync` allocated a misaligned FIFO offset",
                );

                let __rticx_xbin_region = __rticx_xbin_backend
                    .ipc_region(__RTICX_XBIN_SOURCE_CORE, __RTICX_XBIN_TARGET_CORE)
                    .expect(#missing_region);
                let __rticx_xbin_base = __rticx_xbin_region
                    .base_for(
                        __rticx_xbin_backend.current_global_core_id(),
                        __RTICX_XBIN_SOURCE_CORE,
                        __RTICX_XBIN_TARGET_CORE,
                    )
                    .expect(#wrong_core);

                // SAFETY: `system.json` places the FIFO at `offset`, 8-byte
                // aligned, inside the region; `view_at` documents the remaining
                // requirements (one producer, one consumer, initialized memory),
                // which the doorbell/dispatcher protocol upholds.
                unsafe {
                    #rt_path::Fifo::<#input_ty, #depth>::view_at(
                        __rticx_xbin_base + __RTICX_XBIN_FIFO_OFFSET,
                    )
                }
            }
        },
        syn::parse_quote! {
            impl #task_ident {
                #[doc = #spawn_doc]
                pub fn cross_spawn(
                    input: #input_ty,
                ) -> Result<(), Option<#input_ty>> {
                    use #rt_path::CrossBinBackend as _;

                    const __RTICX_XBIN_SOURCE_CORE: u32 = #source;
                    const __RTICX_XBIN_TARGET_CORE: u32 = #target_core;
                    const __RTICX_XBIN_DOORBELL_LINE: u32 = #line;

                    const _: () = assert!(
                        core::mem::size_of::<#input_ty>() == #elem_size,
                        "the IDL layout of the spawn input changed; run `cargo xbin sync`",
                    );
                    const _: () = assert!(
                        core::mem::align_of::<#input_ty>() == #elem_align,
                        "the IDL layout of the spawn input changed; run `cargo xbin sync`",
                    );

                    let __rticx_xbin_backend = #backend_expr;
                    if __rticx_xbin_backend.current_global_core_id() != __RTICX_XBIN_SOURCE_CORE {
                        return Err(Some(input));
                    }
                    // TODO(M5-T2): return `Err(Some(input))` while the target
                    // core is not ready (`CrossBinBackend::is_ready` + epoch).

                    __rticx_interrupt_free(|| -> Result<(), Option<#input_ty>> {
                        let __rticx_xbin_fifo = #fifo_fn(&__rticx_xbin_backend);
                        // SAFETY: this is the single producer core (checked
                        // above) and `__rticx_interrupt_free` serializes other
                        // local spawners.
                        if let Err(input) = unsafe { (*__rticx_xbin_fifo).enqueue(input) } {
                            return Err(Some(input));
                        }
                        match __rticx_xbin_backend.doorbell_ring(
                            __RTICX_XBIN_TARGET_CORE,
                            __RTICX_XBIN_DOORBELL_LINE,
                        ) {
                            Ok(()) => Ok(()),
                            Err(_) => Err(None),
                        }
                    })
                }
            }
        },
    ])
}

/// Finds the task named by `sender` and checks it against the sender stub.
fn find_task<'a>(
    view: &'a SystemView,
    application: &AppEntry,
    sender: &SenderDecl,
) -> syn::Result<&'a TaskEntry> {
    let task = view
        .tasks
        .iter()
        .find(|task| task.name == sender.name)
        .ok_or_else(|| {
            error(format!(
                "the synced system view has no task named `{}`; run `cargo xbin sync`",
                sender.name
            ))
        })?;

    let owned = view
        .cores
        .iter()
        .any(|core| core.global_id == task.fifo.source && core.app == application.package);
    if !owned || !task.spawner_cores.contains(&task.fifo.source) {
        return Err(error(format!(
            "task `{}` is spawned by global core {}, which is not part of application `{}`; \
             run `cargo xbin sync`",
            task.name, task.fifo.source, application.package
        )));
    }

    if sender.core != task.fifo.target {
        return Err(error(format!(
            "sender `{}` targets global core {}, but the synced system view has {}; \
             run `cargo xbin sync`",
            sender.name, sender.core, task.fifo.target
        )));
    }
    if sender.priority != task.priority {
        return Err(error(format!(
            "sender `{}` declares priority {}, but the synced system view has {}; \
             run `cargo xbin sync`",
            sender.name, sender.priority, task.priority
        )));
    }
    if sender.capacity + 1 != task.fifo.depth as usize {
        return Err(error(format!(
            "sender `{}` declares capacity {}, but the synced system view has {}; \
             run `cargo xbin sync`",
            sender.name, sender.capacity, task.capacity
        )));
    }
    if let Some(input) = &sender.input_type
        && simple_type_name(input) != task.input_type
    {
        return Err(error(format!(
            "sender `{}` declares input type `{}`, but the synced system view has `{}`; \
             run `cargo xbin sync`",
            sender.name, input, task.input_type
        )));
    }

    Ok(task)
}

/// Resolves the Rust type of the spawn input.
///
/// A mirrored `impl CrossBinSpawn { type Input = …; }` keeps the user's exact
/// path; without it the IDL type is reached through the backend's
/// `ipc_types_path` (default `ipc_types`).
fn input_type(
    task: &TaskEntry,
    sender: &SenderDecl,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Type> {
    if let Some(path) = &sender.input_type {
        return syn::parse_str(path).map_err(|parse_error| {
            error(format!(
                "cannot parse the input type `{path}` of sender `{}`: {parse_error}",
                sender.name
            ))
        });
    }

    let ipc_types = backend.ipc_types_path();
    let type_ident = ident(&task.input_type)?;
    Ok(syn::parse_quote!(#ipc_types::#type_ident))
}

/// Looks up the canonical alignment of the task input type in the system view.
fn input_align(view: &SystemView, task: &TaskEntry) -> syn::Result<u32> {
    view.types
        .iter()
        .find(|entry| entry.name == task.input_type)
        .map(|entry| entry.align)
        .ok_or_else(|| {
            error(format!(
                "the synced system view has no type `{}` for task `{}`; run `cargo xbin sync`",
                task.input_type, task.name
            ))
        })
}

/// Parses an identifier coming from the system view, with a precise error.
fn ident(name: &str) -> syn::Result<Ident> {
    syn::parse_str(name).map_err(|_| {
        error(format!(
            "`{name}` in the synced system view is not a valid Rust identifier; \
             run `cargo xbin sync`"
        ))
    })
}

/// Builds a code-generation error at the macro call site.
fn error(message: impl Into<String>) -> syn::Error {
    syn::Error::new(Span::call_site(), message.into())
}
