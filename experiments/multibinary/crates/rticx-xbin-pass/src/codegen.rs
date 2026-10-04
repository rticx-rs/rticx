//! Phase-2 code generation for the cross-binary tasks of this application
//! (M3-T1 sender side, M3-T2 receiver side).
//!
//! In codegen mode the pass has loaded the driver-generated `system.json`
//! and emits code from it:
//!
//! - for every view task whose `spawner_core` belongs to this application
//!   (M5.5: the producer declares nothing; the stubs are generated):
//!   - one **ring function** per `(source -> target)` pair the application
//!     produces into: `__rticx_xbin_ring_{source}_{target}(task_id)` publishes
//!     the task id to the target's doorbell and triggers its router. The pass
//!     generates only the documented signature; the body is the distribution's
//!     transport, filled through [`XbinPassBackend::ring_doorbell_fn`]
//!     (M6.5-T2);
//!   - a hidden, argument-less **FIFO view** helper returning the
//!     `rticx_xbin_rt::Fifo` of the task at its synced address: the literal
//!     pool base recorded in `system.json` plus the task offset, or the
//!     distribution's [`XbinPassBackend::ipc_base_override`] when it needs a
//!     runtime lookup (host/mock backends whose pools are not at the synced
//!     addresses). The offset, depth and element layout come from the system
//!     view (const addresses, no local input queue, no forwarder);
//!   - the task struct itself (`pub struct <Task>;`, the generated sender
//!     stub) and `Task::cross_spawn(input)`, matching the error semantics of
//!     the single-binary `cross_spawn`:
//!     - `Ok(())`: the input is enqueued and the target router was notified;
//!     - `Err(None)`: the input is enqueued, but the notification failed
//!       (do not retry the enqueue; re-notify the target);
//!     - `Err(Some(input))`: nothing was enqueued (the FIFO is full or the
//!       caller does not run on the expected core); retry later or raise
//!       `capacity`.
//! - for every native `#[sw_task]` cross-binary receiver of this application
//!   (M3-T2, M5.5, M6.5-T3):
//!   - a hidden **FIFO view** helper like the sender's, but resolving the
//!     consumer view of the pool (`base_from_target`);
//!   - a const assertion that the receiver's `<Task as
//!     RticSwTask>::SpawnInput` implements the injected `CrossCoreMessage`
//!     marker trait, so the guarantee lives in generated code instead of the
//!     trait definition;
//!   - one generated **line dispatcher** per `(source, target, priority)`
//!     line: a hardware task bound to the line's `ipc_dispatchers` entry at
//!     the line's priority, whose `exec` drains the line's ready queue and,
//!     per popped task, that task's FIFO until empty (a duplicate
//!     notification finds an empty FIFO and is a no-op), then calls the
//!     receiver task's `exec(input)`;
//!   - one generated **doorbell router** per `(source, target)` pair: a
//!     hardware task bound to the pair's doorbell IRQ at the highest line
//!     priority of the pair, whose `exec` loops reading task ids from the
//!     pair's doorbell, enqueues each task in its line's ready queue and
//!     pends that line's dispatcher through the software pass's
//!     `__rticx_local_irq_pend[_core{N}]`; unknown ids are ignored;
//!   - the target-side read function
//!     `__rticx_xbin_read_{source}_{target}()`, whose body is the
//!     distribution's transport, filled through
//!     [`XbinPassBackend::read_doorbell_msg_fn`].
//!
//! The receiver structs themselves are turned into framework tasks by
//! [`crate::parse::inject_receiver_tasks`]: `#[task(priority = …,
//! core = …, task_trait = RticSwTask, init = generated)]`, the same shape the
//! single-binary software-task pass uses. The core pass then generates the
//! task static, runs the user's `impl RticSwTask`, and checks the trait
//! implementation — no core-pass changes are required.
//!
//! - **Init hooks** (M3-T3, M6-T1): codegen mode wires the shared-memory
//!   configuration and the FIFO initializers into the generated entry
//!   functions through [`rticx_core::RticPass::main_injection`]:
//!   - [`XbinPassBackend::configure_shared_memory`] is inlined on every core at
//!     the start of its entry (`MainInjectionPoint::BeforeInit`, before the
//!     user `init`): MPU/MMU attributes are per-core, so each core maps its own
//!     view of the IPC pools Normal, Non-cacheable, Shareable;
//!   - `__rticx_xbin_init_fifos_core<N>` is generated for every local core
//!     that produces cross-binary tasks and runs at `BeforePostInit`: it
//!     zeroes the ring indices of the core's own FIFOs, so a topology whose
//!     owner core is not an endpoint of a pool initializes correctly
//!     (M6-T1). The producer is always an endpoint of its pool.
//!
//!   Boot sequencing itself is distribution-owned (for example the H7
//!   release of the M4 core), typically through
//!   `CorePassBackend::post_init`: the distribution guarantees its peers are
//!   up before IPC or the router interrupts are used.
//!
//! The enqueue happens inside `__rticx_interrupt_free`, the v1 sender-side
//! lock: the transport itself is kept independent of the lock mechanism, so a
//! future `Producer` shared-resource API (SRP) can replace it in one place.
//! The consumer side needs no lock: each FIFO has one producer core and one
//! line dispatcher, which runs under its line interrupt at a priority no
//! higher than the router that feeds its ready queue (M6.5-T3).
//!
//! The generated code only uses the frozen public surface of `rticx-core`
//! (`__rticx_interrupt_free`, `main_injection`), the injected `ipc_types`
//! module and the distribution's re-export of `rticx-xbin-rt`, so no
//! root-workspace API changes are required.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rticx_core::parser::ast::uppercase_ident;
use rticx_core::rticx_traits::HWT_TRAIT_TY;
use rticx_xbin_proto::{
    AppEntry, Hash64, PoolEntry, ReceiverDecl, SystemView, TaskEntry, fifo_size, simple_type_name,
};
use syn::{Item, ItemMod, LitInt, LitStr, Type};

use crate::binding::{IpcPool, PhysicalCore};
use crate::parse::AppExtensions;

/// Code-generation configuration of the cross-binary extension.
///
/// A distribution that binds [`crate::XbinPass`] implements this trait to
/// describe, at compile time, how the generated code reaches the
/// distribution's runtime: the doorbell transport bodies, the shared-memory
/// configuration, the current core's identity and the addresses of the IPC
/// pools. It is the single code-generation contract: the generated code no
/// longer threads a runtime backend value through the helpers, so everything
/// it reads at runtime is resolved here or baked from the synced
/// `system.json`.
pub trait XbinPassBackend {
    /// Path to the `rticx-xbin-rt` items (`Fifo`, `FIFO_ALIGN`, …) as
    /// re-exported by the distribution.
    ///
    /// The generated code expands to `#rt_path::Fifo`, so distributions
    /// normally re-export the runtime crate (for example
    /// `rticx_h7::export::xbin_rt`).
    fn rt_path(&self) -> syn::Path;

    /// Path to the generated `ipc_types` module as seen by applications.
    ///
    /// `cargo xbin sync` writes the IDL types to `<output dir>/ipc_types.rs`
    /// and the pass re-emits them into the `#[app]` module as
    /// `pub mod <ipc_types>` (M1-T7). The generated producer stubs reach the
    /// IDL input type as `#ipc_types_path::<InputType>` (the producer declares
    /// nothing, M5.5). The pass injects a module, so only a single-segment path
    /// is supported; a distribution that needs a nested path must keep the
    /// default.
    fn ipc_types_path(&self) -> syn::Path {
        syn::parse_quote!(ipc_types)
    }

    /// Expression of the global core id of the core executing the generated
    /// code (D5). Required.
    ///
    /// `cross_spawn` compares it with the task's synced `source` core and
    /// returns `Err(Some(input))` when they differ, so a multi-core
    /// application never enqueues from the wrong core. The expression is
    /// expanded **inside the `#[app]` module** and must evaluate to `u32`.
    ///
    /// Single-core binaries typically return a constant (for example the
    /// STM32H7's `CURRENT_CORE`), so the guard folds away; the mock returns a
    /// runtime lookup (`__rticx_xbin_backend().global_core_id()`).
    fn current_global_core_id(&self) -> syn::Expr;

    /// Statement(s) configuring local core `local_core`'s view of every IPC
    /// pool before any shared access (D1).
    ///
    /// MPU/MMU attributes are per-core, so each core maps its own view of the
    /// pools Normal, Non-cacheable, Shareable. The pass inlines the returned
    /// tokens at [`rticx_core::MainInjectionPoint::BeforeInit`], before the
    /// user `init`, so the pool is mapped before the first shared write (the
    /// generated `__rticx_xbin_init_fifos_core{N}` runs later, at
    /// `BeforePostInit`).
    ///
    /// **Device and Strongly-ordered memory is forbidden:** exclusive accesses
    /// (`ldrex`/`strex` on Cortex-M) are not valid there, so the pool's
    /// atomics would fault or lose atomicity.
    ///
    /// The default is `None` (nothing), which is correct on hosts and targets
    /// whose data cache does not cover the pool.
    fn configure_shared_memory(&self, _local_core: u32) -> Option<TokenStream> {
        None
    }

    /// Overrides the pool base address of the `(source -> target)` direction
    /// as seen by endpoint `view_core` (D3).
    ///
    /// The default is `None`: the pass emits the literal address recorded in
    /// `system.json`, which is what a real fixed-address distribution (the
    /// STM32H7) needs. The expression must evaluate to `usize` and is expanded
    /// **inside the `#[app]` module**.
    ///
    /// This exists **only for host/mock backends** whose pools are not at the
    /// synced addresses (heap-backed test pools). A real fixed-address
    /// distribution never overrides it.
    fn ipc_base_override(&self, _view_core: u32, _source: u32, _target: u32) -> Option<syn::Expr> {
        None
    }

    /// Emits the producer-side ring function of the `(source -> target)`
    /// doorbell pair (M6.5-T2).
    ///
    /// The pass generates one ring function per pair producing cross-binary
    /// tasks and hands a template with the documented signature
    ///
    /// ```ignore
    /// fn __rticx_xbin_ring_{source}_{target}(task_id: u32) -> Result<(), ()>
    /// ```
    ///
    /// to this method. The implementation fills the body with the
    /// distribution's transport: publish `task_id` to the pair's doorbell (a
    /// per-pair shared atomic word is the portable choice) and trigger the
    /// router interrupt. `Err(())` reports a notification that could not be
    /// delivered; the spawn input is **already enqueued** in the shared pool,
    /// so `cross_spawn` maps it to `Err(None)` (D2).
    ///
    /// The transport body owns any cache maintenance a cacheable pool mapping
    /// needs: v1 requires a Normal, Non-cacheable, Shareable pool, so a
    /// cacheable mapping is unsupported until a distribution appears that
    /// maintains the caches explicitly around every shared access (D2).
    fn ring_doorbell_fn(&self, source: u32, target: u32, template: syn::ItemFn) -> syn::ItemFn;

    /// Identifier of the interrupt handler bound to the `(source -> target)`
    /// doorbell — the target's router ISR (M6.5-T3).
    ///
    /// One router is generated per pair; it is a `#[task(binds = <ident>, …)]`
    /// at the highest priority line of the pair, so the identifier must
    /// resolve inside the `#[app]` module (typically a PAC interrupt name the
    /// distribution re-exports or imports there).
    fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident;

    /// Emits the target-side read function of the `(source -> target)`
    /// doorbell pair (M6.5-T3).
    ///
    /// The pass generates one read function per pair this application
    /// receives into and hands a template with the documented signature
    ///
    /// ```ignore
    /// fn __rticx_xbin_read_{source}_{target}() -> Option<u32>
    /// ```
    ///
    /// to this method. The implementation returns the next pending task id of
    /// the pair's doorbell, or `None` when the doorbell is empty; the router
    /// drains it in a loop and ignores unknown ids.
    fn read_doorbell_msg_fn(&self, target: u32, source: u32, template: syn::ItemFn) -> syn::ItemFn;

    /// Custom path to the interrupt type used for dispatchers on `core`.
    ///
    /// The generated router pends a line dispatcher as
    /// `#pend_fn(#interrupt_type::#ipc_dispatchers_entry)`, so the returned
    /// path must name a **type** whose variants or associated constants match
    /// the `ipc_dispatchers` entries. Return `None` to use the application's
    /// PAC path `pacs[core]::Interrupt`.
    ///
    /// A distribution must return the same path from this method and from
    /// `SwPassBackend::custom_interrupt_path`, so the router and the software
    /// pass's `__rticx_local_irq_pend` agree on the interrupt enum.
    // TODO(extract): mirrors `rticx_sw_pass::SwPassBackend::custom_interrupt_path`.
    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        None
    }

    /// Physical core this application's local core `local_core` runs on
    /// (M6.9-T1).
    ///
    /// The id is a distribution vocabulary, not a project one: two
    /// applications whose cores run on the same physical core report the same
    /// id even when their local indexes differ, and the driver matches the two
    /// endpoints of a dual through it (M6.9-T4).
    ///
    /// The default is the identity mapping, which is correct for a
    /// distribution whose physical-core ids coincide with the local indexes
    /// (the mock or a single-core target).
    fn physical_core(&self, local_core: u32) -> PhysicalCore {
        PhysicalCore(local_core)
    }

    /// Every IPC pool the physical core running local core `local_core` can
    /// reach, from this side of the dual (M6.9-T1).
    ///
    /// The returned pools are the distribution's adjacency. An edge `{A, B}`
    /// exists iff `A` reports a pool whose peer is `B` **and** `B` reports the
    /// matching pool (the same [`PoolId`](crate::PoolId) with the opposite
    /// physical core); the driver builds the global graph from the two
    /// manifests and rejects a used direction with no pool path (M6.9-T4).
    /// Both directions of a dual allocate inside one shared
    /// [`budget`](IpcPool::budget).
    ///
    /// The default is empty: a distribution that has not adopted the
    /// capability binding exposes no reachable peer.
    fn ipc_pools(&self, _local_core: u32) -> Vec<IpcPool> {
        Vec::new()
    }
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
    /// Global core id of each local core index (`core_ids[local]`).
    global_ids: Vec<u32>,
    /// Local cores that produce at least one view task, running
    /// `__rticx_xbin_init_fifos_core<N>` before their `post_init` (M6-T1).
    fifo_locals: Vec<u32>,
}

impl HookPlan {
    /// Returns the tokens to inject at `point` for the entry function of the
    /// local `core`, using `backend` for the shared-memory configuration (D1).
    pub(crate) fn injection(
        &self,
        point: &rticx_core::MainInjectionPoint,
        core: u32,
        backend: &dyn XbinPassBackend,
    ) -> Option<TokenStream> {
        match point {
            rticx_core::MainInjectionPoint::BeforeInit => {
                self.global_ids.get(core as usize)?;
                // The distribution's configuration tokens are inlined: there
                // is no generated wrapper function anymore (D1).
                backend.configure_shared_memory(core)
            }
            rticx_core::MainInjectionPoint::BeforePostInit => {
                self.global_ids.get(core as usize)?;
                if self.fifo_locals.contains(&core) {
                    // Each FIFO's indices are zeroed by its producer core,
                    // before its `post_init` can spawn (M6-T1).
                    let init_fn = format_ident!("__rticx_xbin_init_fifos_core{core}");
                    Some(quote! {
                        #init_fn();
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Generates the producer-side items of an application (M5.5, M6.5-T2): for
/// every view task whose `spawner_core` is one of its cores, the generated
/// `pub struct <Task>;` sender stub, its hidden FIFO view and its
/// `cross_spawn` impl — plus one ring function per `(source -> target)` pair
/// the application produces into, which delivers task ids to the target's
/// doorbell router.
///
/// The producer source declares nothing, so the stubs are derived from the
/// synced system view. A user item with the generated stub's name is rejected
/// with a dedicated error. Applications without producer endpoints generate
/// nothing and do not need a backend.
pub(crate) fn generate_sender_items(
    app_mod: &ItemMod,
    view: &SystemView,
    application: &AppEntry,
    backend: Option<&dyn XbinPassBackend>,
) -> syn::Result<Vec<Item>> {
    let stubs: Vec<&TaskEntry> = view
        .tasks
        .iter()
        .filter(|task| application.core_ids.contains(&task.spawner_core))
        .collect();
    if stubs.is_empty() {
        return Ok(Vec::new());
    }
    let Some(backend) = backend else {
        return Err(error(
            "the synced system view spawns cross-binary tasks from this application, but the \
             distribution did not configure a cross-binary code-generation backend; bind \
             `XbinPass::from_env().with_backend(...)` in the distribution macro",
        ));
    };

    let mut items = Vec::with_capacity(stubs.len() * 3 + 1);
    let mut pairs = BTreeSet::new();
    for task in &stubs {
        pairs.insert((task.fifo.source, task.fifo.target));
    }
    for (source, target) in pairs {
        items.push(generate_ring_function(source, target, backend));
    }
    for task in stubs {
        check_stub_collision(app_mod, task)?;
        items.extend(generate_sender(view, task, backend)?);
    }
    Ok(items)
}

/// Generates the producer ring function of one `(source -> target)` pair
/// (M6.5-T2): the pass hands [`XbinPassBackend::ring_doorbell_fn`] a template
/// with the documented signature and appends the filled function to the
/// `#[app]` module.
fn generate_ring_function(source: u32, target: u32, backend: &dyn XbinPassBackend) -> Item {
    let fn_ident = format_ident!("__rticx_xbin_ring_{source}_{target}");
    let doc = format!(
        "Publishes `task_id` to the `({source} -> {target})` doorbell and triggers the target's \
         router interrupt. The distribution fills the body through \
         `XbinPassBackend::ring_doorbell_fn` (M6.5-T2)."
    );
    let template: syn::ItemFn = syn::parse_quote! {
        #[doc = #doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #fn_ident(task_id: u32) -> Result<(), ()> {
            let _ = task_id;
            // The distribution replaces this body through
            // `XbinPassBackend::ring_doorbell_fn`.
            Err(())
        }
    };
    Item::Fn(backend.ring_doorbell_fn(source, target, template))
}

/// Rejects a generated sender stub whose name collides with a user item of
/// the `#[app]` module (M5.5).
///
/// The producer source never declares the stub, so any same-named item is a
/// collision; naming the task in the error makes the generated name easy to
/// fix (rename the user item).
fn check_stub_collision(app_mod: &ItemMod, task: &TaskEntry) -> syn::Result<()> {
    let Some((_, items)) = app_mod.content.as_ref() else {
        return Ok(());
    };
    for item in items {
        let name = match item {
            Item::Struct(item) => Some(&item.ident),
            Item::Enum(item) => Some(&item.ident),
            Item::Union(item) => Some(&item.ident),
            Item::Type(item) => Some(&item.ident),
            Item::Trait(item) => Some(&item.ident),
            Item::Fn(item) => Some(&item.sig.ident),
            Item::Const(item) => Some(&item.ident),
            Item::Static(item) => Some(&item.ident),
            _ => None,
        };
        if name.is_some_and(|name| *name == task.name) {
            return Err(error(format!(
                "the cross-binary pass generates `pub struct {};` for the task spawned by \
                 global core {}, but the `#[app]` module already defines an item with that \
                 name; rename the item",
                task.name, task.spawner_core
            )));
        }
    }
    Ok(())
}

/// Generates the receiver items for every native cross-binary receiver of an
/// application (M3-T2, M5.5, M6.5-T3): the `SpawnInput: CrossCoreMessage`
/// assertion, one hidden FIFO view per task, one **line dispatcher** per
/// `(source -> target, priority)` line bound to its `ipc_dispatchers` pool
/// entry and draining the line's ready queue, and one **doorbell router** per
/// `(source -> target)` pair that routes task ids to the line dispatchers.
///
/// Returns an error naming the offending declaration when a declaration
/// disagrees with the synced view, or the view places a task on this
/// application's cores without a matching declaration — both mean the source
/// changed after the last `cargo xbin sync`.
pub(crate) fn generate_receiver_items(
    view: &SystemView,
    application: &AppEntry,
    receivers: &[ReceiverDecl],
    extensions: &AppExtensions,
    backend: Option<&dyn XbinPassBackend>,
) -> syn::Result<Vec<Item>> {
    if receivers.is_empty() {
        return Ok(Vec::new());
    }
    let Some(backend) = backend else {
        return Err(error(
            "this application declares cross-binary receivers, but the distribution did not \
             configure a cross-binary code-generation backend; bind \
             `XbinPass::from_env().with_backend(...)` in the distribution macro",
        ));
    };
    let package = &application.package;

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
                 declares no cross-binary receiver for it; run `cargo xbin sync`",
                task.name
            )));
        }
    }

    // ~ The pass-owned `ipc_dispatchers` pool assigns one interrupt per line
    // in ascending `(source, priority)` order (`validate_ipc_dispatchers`
    // checked the count in `run_pass`), so the line's index in its core's
    // list names the dispatcher's interrupt line (M6.5-T1/T4).
    let irq_indices = line_irq_indices(receivers);

    // ~ One line dispatcher per `(source, target, priority)`: the key of the
    // tasks' `system.json` doorbell and of the `ipc_dispatchers` entry.
    let mut groups: BTreeMap<(u32, u32, u16), LineGroup<'_>> = BTreeMap::new();
    for receiver in receivers {
        let resolved = resolve_receiver(view, application, receiver)?;
        let task = resolved.task;
        let key = (task.fifo.source, task.fifo.target, task.priority);
        let index = irq_indices[&(resolved.local_core, task.fifo.source, task.priority)];
        let irq = extensions.ipc_dispatchers[resolved.local_core as usize][index].clone();
        groups
            .entry(key)
            .or_insert_with(|| LineGroup {
                source: task.fifo.source,
                target: task.fifo.target,
                priority: task.priority,
                local_core: resolved.local_core,
                irq,
                receivers: Vec::new(),
            })
            .receivers
            .push(resolved);
    }

    // ~ The receiver's `SpawnInput` must be a cross-core message; a generated
    // const assertion pins the guarantee to the injected `CrossCoreMessage`
    // marker trait (M5.5, M1-T7) instead of relying on a bound in its
    // definition.
    let assertions = receivers
        .iter()
        .map(|receiver| {
            let task_ident = ident(&receiver.name)?;
            Ok(quote! {
                __rticx_xbin_assert_cross_core_message::<<#task_ident as RticSwTask>::SpawnInput>();
            })
        })
        .collect::<syn::Result<Vec<TokenStream>>>()?;
    let assert_doc = "Compile-time assertion that every cross-binary receiver's `SpawnInput` \
         implements `CrossCoreMessage` (M5.5).";
    let mut items = Vec::with_capacity(receivers.len() * 2 + groups.len() * 3 + 1);
    items.push(syn::parse_quote! {
        #[doc = #assert_doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        const _: () = {
            fn __rticx_xbin_assert_cross_core_message<T: CrossCoreMessage>() {}
            fn __rticx_xbin_check() {
                #(#assertions)*
            }
        };
    });

    for group in groups.values() {
        items.extend(generate_line_items(group, backend)?);
    }

    // ~ One router per `(source, target)` pair: it routes every task id of
    // the pair's doorbell word to its line (M6.5-T3).
    let mut pairs: BTreeMap<(u32, u32), Vec<&LineGroup<'_>>> = BTreeMap::new();
    for group in groups.values() {
        pairs
            .entry((group.source, group.target))
            .or_default()
            .push(group);
    }
    for ((source, target), lines) in &pairs {
        items.extend(generate_router_items(
            *source, *target, lines, extensions, backend,
        )?);
    }
    Ok(items)
}

/// One `(source -> target, priority)` line of an application: the receivers
/// that share the line and the `ipc_dispatchers` entry that wakes its
/// dispatcher.
struct LineGroup<'a> {
    /// Producer (source) global core id.
    source: u32,
    /// Receiver (target) global core id.
    target: u32,
    /// Line priority.
    priority: u16,
    /// Local core index running the line's dispatcher (`0..cores`).
    local_core: u32,
    /// Interrupt line of the line's dispatcher, from the application's
    /// `ipc_dispatchers` pool.
    irq: syn::Path,
    /// The receivers of the line, in declaration (name) order.
    receivers: Vec<ResolvedReceiver<'a>>,
}

impl LineGroup<'_> {
    /// The generated task-enum type naming the line's tasks.
    fn enum_ident(&self) -> Ident {
        format_ident!(
            "__RticxXbinLine{}To{}P{}",
            self.source,
            self.target,
            self.priority
        )
    }

    /// The generated static ready queue of the line.
    fn ready_queue_ident(&self) -> Ident {
        format_ident!(
            "__rticx_xbin_ready_{}_{}_p{}",
            self.source,
            self.target,
            self.priority
        )
    }

    /// The generated dispatcher task struct of the line.
    fn dispatcher_ident(&self) -> Ident {
        format_ident!(
            "__RticxXbinDispatcher{}To{}P{}",
            self.source,
            self.target,
            self.priority
        )
    }

    /// The ready-queue depth: one slot per pending notification plus the ring
    /// buffer's spare slot (M6.5-T3).
    ///
    /// Each spawn enqueues exactly one FIFO element before ringing, so the
    /// line's pending notifications are bounded by the sum of its tasks'
    /// capacities; the dispatcher drains both together.
    fn queue_depth(&self) -> usize {
        self.receivers
            .iter()
            .map(|resolved| resolved.task.capacity)
            .sum::<usize>()
            + 1
    }
}

/// Index of every cross line in its local core's `ipc_dispatchers` list.
///
/// Mirrors [`crate::parse::validate_ipc_dispatchers`]: the pool lists each
/// core's distinct `(source, priority)` lines in ascending order (M6.5-T1),
/// and a task's line is the `(spawn_by, priority)` pair of its declaration.
fn line_irq_indices(receivers: &[ReceiverDecl]) -> BTreeMap<(u32, u32, u16), usize> {
    let mut per_core: BTreeMap<u32, Vec<(u32, u16)>> = BTreeMap::new();
    for receiver in receivers {
        per_core
            .entry(receiver.core)
            .or_default()
            .push((receiver.spawn_by, receiver.priority));
    }
    let mut indices = BTreeMap::new();
    for (core, mut lines) in per_core {
        lines.sort_unstable();
        lines.dedup();
        for (index, (source, priority)) in lines.into_iter().enumerate() {
            indices.insert((core, source, priority), index);
        }
    }
    indices
}

/// Generates the items of one `(source -> target, priority)` line (M6.5-T3):
/// the hidden FIFO views, the task enum and ready queue shared with the
/// router, and the line dispatcher.
///
/// The dispatcher is a hardware task bound to the line's `ipc_dispatchers`
/// entry at the line priority: it drains its ready queue and, per popped task,
/// drains that task's FIFO until empty (a duplicate notification finds an
/// empty FIFO and is a no-op) and calls the receiver's `exec`.
fn generate_line_items(
    line: &LineGroup<'_>,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<Item>> {
    let rt_path = backend.rt_path();
    let task_trait = format_ident!("{HWT_TRAIT_TY}");
    let line_ty = line.enum_ident();
    let ready_queue = line.ready_queue_ident();
    let dispatcher_ty = line.dispatcher_ident();
    let queue_depth = line.queue_depth();
    let irq = &line.irq;
    let priority_lit = LitInt::new(&line.priority.to_string(), Span::call_site());
    let core_lit = LitInt::new(&line.local_core.to_string(), Span::call_site());

    let mut items = Vec::with_capacity(line.receivers.len() * 2 + 3);
    let mut variants = Vec::with_capacity(line.receivers.len());
    let mut arms = Vec::with_capacity(line.receivers.len());
    for resolved in &line.receivers {
        items.push(generate_receiver_fifo_view(resolved, backend)?);

        let task = resolved.task;
        let task_ident = ident(&task.name)?;
        let task_static = uppercase_ident(&task_ident);
        let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
        variants.push(task_ident.clone());
        arms.push(quote! {
            #line_ty::#task_ident => {
                let __rticx_xbin_fifo = #fifo_fn();
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

    let enum_doc = format!(
        "Tasks of the `({} -> {})` priority-{} line: the router enqueues one \
         variant per notification (M6.5-T3).",
        line.source, line.target, line.priority
    );
    items.push(syn::parse_quote! {
        #[doc = #enum_doc]
        #[doc(hidden)]
        #[derive(Clone, Copy)]
        pub enum #line_ty {
            #(#variants,)*
        }
    });

    let queue_doc = format!(
        "Ready queue of the `({} -> {})` priority-{} line: the router produces, \
         the dispatcher consumes. Sized to the sum of the line's task capacities \
         plus the ring buffer's spare slot, so it cannot overflow (M6.5-T3).",
        line.source, line.target, line.priority
    );
    items.push(syn::parse_quote! {
        #[doc = #queue_doc]
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        static mut #ready_queue: #rt_path::Queue<#line_ty, #queue_depth> = #rt_path::Queue::new();
    });

    let dispatcher_doc = format!(
        "Line dispatcher of the `({} -> {})` priority-{} line: drains the \
         line's ready queue and FIFOs and runs the receiver tasks (M6.5-T3).",
        line.source, line.target, line.priority
    );
    items.push(syn::parse_quote! {
        #[doc = #dispatcher_doc]
        #[doc(hidden)]
        #[task(binds = #irq, priority = #priority_lit, core = #core_lit, init = generated)]
        pub struct #dispatcher_ty;
    });
    items.push(syn::parse_quote! {
        impl #task_trait for #dispatcher_ty {
            fn exec(&mut self) {
                // SAFETY: the router (the only producer) runs at a priority
                // at least as high as this dispatcher, so the enqueue and the
                // dequeue never overlap on this core.
                let mut __rticx_xbin_ready = unsafe { #ready_queue.split().1 };
                while let Some(__rticx_xbin_task) = __rticx_xbin_ready.dequeue() {
                    match __rticx_xbin_task {
                        #(#arms)*
                    }
                }
            }
        }
    });
    Ok(items)
}

/// Generates the router of one `(source -> target)` pair (M6.5-T3).
///
/// The router is a hardware task bound to the pair's doorbell at the highest
/// line priority of the pair: it loops reading task ids from the pair's
/// doorbell, enqueues the task in its line's ready queue and pends that line's
/// dispatcher through the software pass's `__rticx_local_irq_pend[_core{N}]`.
/// Unknown ids are ignored.
fn generate_router_items(
    source: u32,
    target: u32,
    lines: &[&LineGroup<'_>],
    extensions: &AppExtensions,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<Item>> {
    let first = lines.first().expect("a router pair is never empty");
    let local_core = first.local_core;
    let max_priority = lines
        .iter()
        .map(|line| line.priority)
        .max()
        .expect("a router pair is never empty");
    let router_ty = format_ident!("__RticxXbinRouter{source}To{target}");
    let router_irq = backend.doorbell_interrupt(target, source);
    let read_fn = format_ident!("__rticx_xbin_read_{source}_{target}");
    let interrupt_ty = interrupt_path(backend, extensions, local_core);
    let pend_fn = local_pend_fn_ident(local_core, extensions.cores);
    let task_trait = format_ident!("{HWT_TRAIT_TY}");
    let max_priority_lit = LitInt::new(&max_priority.to_string(), Span::call_site());
    let core_lit = LitInt::new(&local_core.to_string(), Span::call_site());

    let mut arms = Vec::with_capacity(lines.iter().map(|line| line.receivers.len()).sum());
    for line in lines {
        let line_ty = line.enum_ident();
        let ready_queue = line.ready_queue_ident();
        let line_irq = &line.irq;
        for resolved in &line.receivers {
            let task = resolved.task;
            let task_id = task.id;
            let task_ident = ident(&task.name)?;
            arms.push(quote! {
                #task_id => {
                    // SAFETY: the router is the only producer on this core
                    // and runs at a priority at least as high as the line
                    // dispatcher, so it never races the dequeue.
                    unsafe {
                        #ready_queue.split().0.enqueue_unchecked(#line_ty::#task_ident);
                    }
                    #pend_fn(#interrupt_ty::#line_irq);
                }
            });
        }
    }

    let doc = format!(
        "Doorbell router of the `({source} -> {target})` pair: reads task ids \
         from the pair's doorbell word, enqueues each task in its line's ready \
         queue and pends that line's dispatcher (M6.5-T3). Unknown ids are \
         ignored."
    );
    Ok(vec![
        generate_read_function(source, target, backend),
        syn::parse_quote! {
            #[doc = #doc]
            #[doc(hidden)]
            #[task(binds = #router_irq, priority = #max_priority_lit, core = #core_lit, init = generated)]
            pub struct #router_ty;
        },
        syn::parse_quote! {
            impl #task_trait for #router_ty {
                fn exec(&mut self) {
                    // The router drains the pair's doorbell word; duplicate and
                    // coalesced ids lose no spawns because each line
                    // dispatcher drains its FIFOs until empty (M6.5-T3).
                    while let Some(__rticx_xbin_task_id) = #read_fn() {
                        match __rticx_xbin_task_id {
                            #(#arms)*
                            _ => {}
                        }
                    }
                }
            }
        },
    ])
}

/// Generates the target-side read function of one `(source -> target)` pair
/// (M6.5-T3): the pass hands [`XbinPassBackend::read_doorbell_msg_fn`] a
/// template with the documented signature and appends the filled function.
fn generate_read_function(source: u32, target: u32, backend: &dyn XbinPassBackend) -> Item {
    let fn_ident = format_ident!("__rticx_xbin_read_{source}_{target}");
    let doc = format!(
        "Returns the next pending task id of the `({source} -> {target})` doorbell, or `None` \
         when it is empty. The distribution fills the body through \
         `XbinPassBackend::read_doorbell_msg_fn` (M6.5-T3)."
    );
    let template: syn::ItemFn = syn::parse_quote! {
        #[doc = #doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #fn_ident() -> Option<u32> {
            // The distribution replaces this body through
            // `XbinPassBackend::read_doorbell_msg_fn`.
            None
        }
    };
    Item::Fn(backend.read_doorbell_msg_fn(target, source, template))
}

/// Computes the interrupt type path of the dispatchers on `core`.
///
/// Uses the backend's [`XbinPassBackend::custom_interrupt_path`] if provided,
/// otherwise falls back to the application's PAC path `pacs[core]::Interrupt`,
/// exactly like the software pass.
// TODO(extract): mirrors `rticx_sw_pass`'s `get_interrupt_path`.
fn interrupt_path(
    backend: &dyn XbinPassBackend,
    extensions: &AppExtensions,
    core: u32,
) -> syn::Path {
    backend.custom_interrupt_path(core).unwrap_or_else(|| {
        let pac = &extensions.pacs[core as usize];
        syn::parse_quote!(#pac::Interrupt)
    })
}

/// Name of the software pass's core-local interrupt-pending function.
///
/// Single-core applications keep the plain name; multi-core applications
/// append the local core index (M6.5-T3).
// TODO(extract): mirrors `rticx_sw_pass`'s `local_pend_fn_ident`.
fn local_pend_fn_ident(core: u32, cores: u32) -> Ident {
    if cores == 1 {
        format_ident!("__rticx_local_irq_pend")
    } else {
        format_ident!("__rticx_local_irq_pend_core{core}")
    }
}

/// Generates the init hooks of one application (M3-T3, M6-T1): one
/// argument-less `__rticx_xbin_init_fifos_core<N>` per producing core, whose
/// body bakes the synced pool addresses.
///
/// The shared-memory configuration is not a generated function: the pass
/// inlines [`XbinPassBackend::configure_shared_memory`] at
/// [`rticx_core::MainInjectionPoint::BeforeInit`] (D1). The returned
/// [`HookPlan`] is stashed by the pass and consulted from
/// [`rticx_core::RticPass::main_injection`] to wire both into the generated
/// entry functions. Boot coordination between cores is distribution-owned and
/// is not emitted here.
pub(crate) fn generate_init_hooks(
    view: &SystemView,
    application: &AppEntry,
    backend: &dyn XbinPassBackend,
) -> syn::Result<InitHooks> {
    // Every core zeroes the FIFOs it produces, before its `post_init` can
    // spawn. The producer is always an endpoint of its region, so a topology
    // whose owner core is not an endpoint still initializes correctly
    // (M6-T1).
    let mut items = Vec::with_capacity(application.core_ids.len());
    let mut fifo_locals = Vec::new();
    for (local, &global) in application.core_ids.iter().enumerate() {
        let zero_fifos = generate_fifo_inits(view, global, backend)?;
        if zero_fifos.is_empty() {
            continue;
        }
        let local = local as u32;
        fifo_locals.push(local);
        let init_doc = format!(
            "Zeroes the ring indices of every task FIFO produced by this core (global core \
             {global}) before its `post_init` can spawn (M6-T1)."
        );
        let init_fn = format_ident!("__rticx_xbin_init_fifos_core{local}");
        items.push(syn::parse_quote! {
            #[doc = #init_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #init_fn() {
                #(#zero_fifos)*
            }
        });
    }

    Ok(InitHooks {
        items,
        plan: HookPlan {
            global_ids: application.core_ids.clone(),
            fifo_locals,
        },
    })
}

/// The distro pool a task FIFO was allocated in, or `None` for a project
/// without a capability table (M6.9-T6).
///
/// Fails when the view names a pool that has no `pools[]` entry: the FIFO view
/// would then point into memory the distribution never reserved.
fn pool_of<'a>(view: &'a SystemView, task: &TaskEntry) -> syn::Result<Option<&'a PoolEntry>> {
    let Some(id) = task.fifo.pool.as_deref() else {
        return Ok(None);
    };
    view.pools
        .iter()
        .find(|pool| pool.id == id)
        .map(Some)
        .ok_or_else(|| {
            error(format!(
                "the synced system view places task `{}` in pool `{id}`, which has no `pools[]` \
                 entry; run `cargo xbin sync`",
                task.name
            ))
        })
}

/// Base address of `pool` as seen by endpoint `view_core`, or `None` when
/// `view_core` is not one of the pool's endpoints (read from the synced
/// pool entry's two endpoint views).
///
/// The pool records both endpoint views (`base_from_a`/`base_from_b`) keyed by
/// the ascending global core ids `core_a`/`core_b`, so the view of a core is
/// independent of the `(source -> target)` direction it participates in.
fn pool_endpoint_base(pool: &PoolEntry, view_core: u32) -> Option<u32> {
    if view_core == pool.core_a {
        Some(pool.base_from_a)
    } else if view_core == pool.core_b {
        Some(pool.base_from_b)
    } else {
        None
    }
}

/// Renders `base` as a `usize` literal expression for the generated code.
fn base_literal(base: u32) -> syn::Expr {
    let lit = LitInt::new(&format!("0x{base:08x}usize"), Span::call_site());
    syn::parse_quote!(#lit)
}

/// Resolves the base-address expression of the `(source -> target)` pool as
/// seen by endpoint `view_core`.
///
/// A distribution override wins (D3: only host/mock backends need one, because
/// their pools are not at the synced addresses); otherwise the pass emits the
/// literal address recorded in `system.json`. A view with neither a pool nor an
/// override cannot be generated and is a hard `cargo xbin sync` error.
fn pool_base_expr(
    backend: &dyn XbinPassBackend,
    view_core: u32,
    source: u32,
    target: u32,
    pool: Option<&PoolEntry>,
) -> syn::Result<syn::Expr> {
    if let Some(expr) = backend.ipc_base_override(view_core, source, target) {
        return Ok(expr);
    }
    let pool = pool.ok_or_else(|| {
        error(format!(
            "the synced system view has no IPC pool for the `({source} -> {target})` direction \
             and the distribution provides no base override; run `cargo xbin sync`"
        ))
    })?;
    let base = pool_endpoint_base(pool, view_core).ok_or_else(|| {
        error(format!(
            "global core {view_core} is not an endpoint of the `({source} -> {target})` IPC \
             pool `{}`; run `cargo xbin sync`",
            pool.id
        ))
    })?;
    Ok(base_literal(base))
}

/// Pins a task FIFO to its distro pool (M6.9-T6).
///
/// Emits the pool id and shared budget beside the FIFO offset and asserts, at
/// compile time, that the FIFO still fits the pool. A `system.json` whose pools
/// moved or shrank since the source was compiled then fails the build instead
/// of letting the FIFO overlap its neighbour. A project without a capability
/// table emits nothing.
fn pool_consts(pool: Option<&PoolEntry>, task: &TaskEntry) -> syn::Result<TokenStream> {
    let Some(pool) = pool else {
        return Ok(TokenStream::new());
    };
    let id = &pool.id;
    let budget = pool.budget as usize;
    let fifo_bytes = usize::try_from(fifo_size(task.fifo.elem_size, task.capacity).ok_or_else(
        || {
            error(format!(
                "the FIFO of task `{}` does not fit the address space; run `cargo xbin sync`",
                task.name
            ))
        },
    )?)
    .map_err(|_| {
        error(format!(
            "the FIFO of task `{}` is too large; run `cargo xbin sync`",
            task.name
        ))
    })?;
    Ok(quote! {
        // M6.9-T6: pin the distro pool this FIFO was allocated in.
        #[allow(dead_code)]
        const __RTICX_XBIN_POOL_ID: &str = #id;
        #[allow(dead_code)]
        const __RTICX_XBIN_POOL_BUDGET: usize = #budget;
        const _: () = assert!(
            __RTICX_XBIN_FIFO_OFFSET + #fifo_bytes <= __RTICX_XBIN_POOL_BUDGET,
            "`cargo xbin sync` placed the FIFO outside its pool budget",
        );
    })
}

/// Generates the FIFO-zeroing blocks of the `producer` core's initializer.
///
/// The producer of a FIFO is an endpoint of its `(source -> target)` pool by
/// construction, so the synced pool address always resolves; a topology whose
/// owner core is not an endpoint of the pool initializes correctly (M6-T1).
/// The base is baked (literal, or the distribution override for host pools),
/// so the generated initializer is argument-less.
fn generate_fifo_inits(
    view: &SystemView,
    producer: u32,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<TokenStream>> {
    let rt_path = backend.rt_path();
    let ipc_types = backend.ipc_types_path();

    let mut blocks = Vec::new();
    for task in &view.tasks {
        let source = task.fifo.source;
        let target = task.fifo.target;
        if source != producer {
            continue;
        }

        let type_ident = ident(&task.input_type)?;
        let offset = task.fifo.offset as usize;
        let depth = task.fifo.depth as usize;
        let elem_size = task.fifo.elem_size as usize;
        let elem_align = input_align(view, task)? as usize;
        let pool = pool_of(view, task)?;
        let pool_consts = pool_consts(pool, task)?;
        let base = pool_base_expr(backend, producer, source, target, pool)?;

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
                #pool_consts

                // SAFETY: `system.json` places the FIFO at `offset`, aligned,
                // inside the pool; the producer core runs this before its
                // `post_init` can spawn and no consumer acts before a
                // notification, and `Fifo::init` documents why it must not
                // race one. The base is the literal synced address, or the
                // distribution's override for host/mock pools (D3).
                unsafe {
                    (*#rt_path::Fifo::<#ipc_types::#type_ident, #depth>::view_at(
                        #base + __RTICX_XBIN_FIFO_OFFSET,
                    ))
                    .init();
                }
            }
        });
    }
    Ok(blocks)
}

/// One receiver declaration resolved against the system view.
struct ResolvedReceiver<'a> {
    /// The synced task the receiver executes.
    task: &'a TaskEntry,
    /// Local core index the task runs on (`0..cores`).
    local_core: u32,
    /// Rust type of the spawn input, as declared by the user.
    input: Type,
    /// Canonical alignment of the input type (`system.json`).
    elem_align: u32,
    /// Distro pool the task FIFO lives in (`system.json`; `None` without a
    /// capability table).
    pool: Option<&'a PoolEntry>,
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
    if receiver.spawn_by != task.spawner_core {
        return Err(error(format!(
            "receiver `{}` declares `spawn_by = {}`, but the synced system view has {}; \
             run `cargo xbin sync`",
            receiver.name, receiver.spawn_by, task.spawner_core
        )));
    }

    // The pair router is generated from the view's doorbell table, so a task
    // without its `(source, target, priority)` doorbell entry is a stale view.
    view.doorbells
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
        local_core: receiver.core,
        input,
        elem_align: input_align(view, task)?,
        pool: pool_of(view, task)?,
    })
}

/// Generates the hidden, argument-less FIFO view of one resolved receiver,
/// mirroring the sender-side helper but resolving the consumer endpoint's view
/// of the pool.
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
    let pool_consts = pool_consts(resolved.pool, task)?;
    let base = pool_base_expr(backend, target_core, source, target_core, resolved.pool)?;

    let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
    let fifo_doc = format!(
        "Returns this application's consumer view of the `{}` FIFO, placed at its `({source} -> \
         {target_core})` pool offset from `system.json`.",
        task.name
    );

    Ok(syn::parse_quote! {
        #[doc = #fifo_doc]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #fifo_fn() -> *mut #rt_path::Fifo<#input_ty, #depth> {
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
            #pool_consts

            // SAFETY: `system.json` places the FIFO at `offset`, 8-byte
            // aligned, inside the pool; `view_at` documents the remaining
            // requirements (one producer, one consumer, initialized memory),
            // which the doorbell/dispatcher protocol upholds. The base is the
            // literal synced address, or the distribution's override for
            // host/mock pools (D3).
            unsafe {
                #rt_path::Fifo::<#input_ty, #depth>::view_at(
                    #base + __RTICX_XBIN_FIFO_OFFSET,
                )
            }
        }
    })
}

/// Finds the application in `view` and checks that its core mapping still
/// matches the source being compiled.
///
/// `target` is the Cargo target name (`CARGO_BIN_NAME`) when the compiler
/// provides one. Cargo always does; rust-analyzer's proc-macro server
/// deliberately does not, so a `None` target resolves the application by
/// package alone. `rticx.toml` rejects two applications sharing a package, so
/// the package is an unambiguous key and IDE macro expansion keeps working.
pub(crate) fn check_application<'a>(
    view: &'a SystemView,
    package: &str,
    target: Option<&str>,
    extensions: &AppExtensions,
) -> syn::Result<&'a AppEntry> {
    let application = view
        .apps
        .iter()
        .find(|app| app.package == package && target.is_none_or(|target| app.target.name == target))
        .ok_or_else(|| {
            let target = match target {
                Some(target) => format!(" (target `{target}`)"),
                None => String::new(),
            };
            error(format!(
                "the synced system view has no application `{package}`{target}; \
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
    // The mapping the core pass resolves (identity when not declared) is what
    // the generated runtime core checks compare against, so it must match the
    // synced view (M5-T3).
    if extensions.core_ids != application.core_ids {
        return Err(error(format!(
            "application `{package}` maps its local cores to {:?}, but the synced system view \
             has {:?}; run `cargo xbin sync`",
            extensions.core_ids, application.core_ids
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
    // must be absolute (`load_system` already read the file).
    let path_lit = include_str_path_lit(path)?;
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

/// Returns `path` canonicalized (or lexically absolute as a fallback) as a
/// UTF-8 string literal for `include_str!`.
///
/// `include_str!` resolves a relative path against the file containing the
/// macro invocation, not the compilation working directory, so the path must be
/// absolute.
fn include_str_path_lit(path: &Path) -> syn::Result<LitStr> {
    let absolute = path
        .canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    absolute
        .to_str()
        .map(|text| LitStr::new(text, Span::call_site()))
        .ok_or_else(|| {
            error(format!(
                "the generated file path `{}` is not valid UTF-8; use a UTF-8 path",
                path.display()
            ))
        })
}

/// Generates the injected `CrossCoreMessage` marker trait and the `ipc_types`
/// module of this application (M1-T7).
///
/// Since M1-T7 the IDL types are no longer a separate crate: `cargo xbin sync`
/// writes `<output dir>/ipc_types.rs` next to `system.json`, and this pass
/// re-emits the file's tokens into the `#[app]` module as
/// `pub mod <ipc_types> { … }`. Re-emitting (rather than `include!` or
/// `#[path]`) keeps the module visible to `cargo rticx-expand`.
///
/// The trait is injected alongside the module (the same pattern as
/// `RticSwTask` and `RticIdleTask`), so the re-emitted
/// `unsafe impl CrossCoreMessage for X {}` resolves through
/// `use super::CrossCoreMessage;` without a runtime or distribution
/// dependency.
///
/// Returns no items for a project without an `ipc-types.toml` (the view has no
/// types). A view with types but a missing file is the documented
/// "run `cargo xbin sync`" hard error. The `include_str!` anchor pins rustc's
/// dep-info to the file, because the pass re-emits the tokens instead of
/// including the file.
pub(crate) fn generate_ipc_types_items(
    app_mod: &ItemMod,
    system_path: &Path,
    view: &SystemView,
    backend: Option<&dyn XbinPassBackend>,
) -> syn::Result<Vec<Item>> {
    if view.types.is_empty() {
        return Ok(Vec::new());
    }

    let module_ident = injected_ipc_types_ident(backend)?;
    check_ipc_types_collision(app_mod, &module_ident)?;
    let file = system_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(rticx_xbin_proto::IPC_TYPES_FILE);
    let source = std::fs::read_to_string(&file).map_err(|source| {
        error(format!(
            "failed to read the generated `{}`: {source}; run `cargo xbin sync`",
            file.display()
        ))
    })?;
    let parsed = syn::parse_file(&source).map_err(|source| {
        error(format!(
            "failed to parse the generated `{}`: {source}; run `cargo xbin sync`",
            file.display()
        ))
    })?;
    // The generator emits no inner attributes; drop any defensively so a
    // hand-edited file cannot fail inside the injected module.
    let generated_items = parsed.items;

    let trait_doc = "Marker trait for a type that may cross a cross-binary IPC boundary. Every \
         type of `ipc-types.toml` implements it; the injected declaration is what lets the \
         generated `ipc_types` module re-emit the `unsafe impl`s without depending on the runtime \
         crate (M5.5, M1-T7).";
    let trait_item: Item = syn::parse_quote! {
        #[doc = #trait_doc]
        pub unsafe trait CrossCoreMessage: Copy + 'static {}
    };

    let module_doc = format!(
        "IDL types of `ipc-types.toml`, generated by `cargo xbin sync` into `{}` and re-emitted \
         into the `#[app]` module (M1-T7).",
        file.display()
    );
    let module_item: Item = syn::parse_quote! {
        #[doc = #module_doc]
        #[allow(non_camel_case_types, non_snake_case, unused_imports)]
        pub mod #module_ident {
            use super::CrossCoreMessage;
            #(#generated_items)*
        }
    };

    let path_lit = include_str_path_lit(&file)?;
    let anchor_doc = "The generated `ipc_types.rs`, embedded so rustc records it in dep-info and \
         rebuilds this application when it changes (M1-T7).";
    let anchor: Item = syn::parse_quote! {
        #[doc = #anchor_doc]
        #[doc(hidden)]
        #[allow(dead_code)]
        const __RTICX_XBIN_IPC_TYPES_RS: &str = include_str!(#path_lit);
    };

    Ok(vec![trait_item, module_item, anchor])
}

/// Rejects a user item that collides with the injected `CrossCoreMessage`
/// marker trait or the injected `ipc_types` module (M1-T7).
///
/// The pass appends both items to the `#[app]` module; a same-named user item
/// would otherwise fail with rustc's generic "the name is defined multiple
/// times" error. The check is limited to the namespaces the injected items
/// occupy.
fn check_ipc_types_collision(app_mod: &ItemMod, module_ident: &Ident) -> syn::Result<()> {
    let Some((_, items)) = app_mod.content.as_ref() else {
        return Ok(());
    };
    for item in items {
        let (name, is_type_namespace) = match item {
            Item::Mod(item) => (&item.ident, true),
            Item::Trait(item) => (&item.ident, true),
            Item::Struct(item) => (&item.ident, true),
            Item::Enum(item) => (&item.ident, true),
            Item::Union(item) => (&item.ident, true),
            Item::Type(item) => (&item.ident, true),
            Item::Fn(item) => (&item.sig.ident, false),
            Item::Const(item) => (&item.ident, false),
            Item::Static(item) => (&item.ident, false),
            _ => continue,
        };
        if name == module_ident {
            return Err(error(format!(
                "the cross-binary pass injects a `{name}` module into the `#[app]` module; rename \
                 the user item"
            )));
        }
        if is_type_namespace && name == "CrossCoreMessage" {
            return Err(error(
                "the cross-binary pass injects a `CrossCoreMessage` trait into the `#[app]` \
                 module; rename the user item",
            ));
        }
    }
    Ok(())
}

/// Resolves the injected module identifier from the distribution backend's
/// [`XbinPassBackend::ipc_types_path`] (default `ipc_types`).
///
/// The pass injects the module into the `#[app]` module, so only a
/// single-segment path is representable; a multi-segment path is rejected
/// instead of silently dropping the extra segments. Without a backend the
/// default `ipc_types` is used.
fn injected_ipc_types_ident(backend: Option<&dyn XbinPassBackend>) -> syn::Result<Ident> {
    let Some(backend) = backend else {
        return Ok(format_ident!("ipc_types"));
    };
    let path = backend.ipc_types_path();
    let mut segments = path.segments.iter();
    let Some(segment) = segments.next() else {
        return Err(error(
            "`XbinPassBackend::ipc_types_path` returned an empty path",
        ));
    };
    if segments.next().is_some() || path.leading_colon.is_some() {
        return Err(error(
            "the cross-binary pass injects the `ipc_types` module into the `#[app]` module, so \
             `XbinPassBackend::ipc_types_path` must name a single module",
        ));
    }
    Ok(segment.ident.clone())
}

/// Generates the sender stub of one view task: the `pub struct <Task>;` the
/// producer's source does not declare (M5.5), its FIFO view and its
/// `cross_spawn` impl.
fn generate_sender(
    view: &SystemView,
    task: &TaskEntry,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<Item>> {
    let task_ident = ident(&task.name)?;
    let input_ty = input_type(task, backend)?;
    let rt_path = backend.rt_path();
    let current_core = backend.current_global_core_id();

    let source = task.fifo.source;
    let target_core = task.fifo.target;
    let task_id = task.id;
    let offset = task.fifo.offset as usize;
    let depth = task.fifo.depth as usize;
    let elem_size = task.fifo.elem_size as usize;
    let elem_align = input_align(view, task)? as usize;
    let pool = pool_of(view, task)?;
    let pool_consts = pool_consts(pool, task)?;
    let base = pool_base_expr(backend, source, source, target_core, pool)?;

    let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
    let ring_fn = format_ident!("__rticx_xbin_ring_{source}_{target_core}");
    let fifo_doc = format!(
        "Returns this application's view of the `{}` FIFO, placed at its `({source} -> \
         {target_core})` pool offset from `system.json`.",
        task.name
    );

    let spawn_doc = format!(
        "Cross-binary spawn: enqueue `input` into the `{}` FIFO and notify the router of global \
         core {target_core}.\n\n\
         # Returns\n\n\
         - `Ok(())`: the input is enqueued and the target router was notified;\n\
         - `Err(None)`: the input is enqueued, but the notification failed. Do **not** \
         retry the spawn; re-notify the target instead;\n\
         - `Err(Some(input))`: nothing was enqueued (the FIFO is full or the caller does not \
         run on global core {source}). Retry later or raise `capacity`.",
        task.name
    );
    let stub_doc = format!(
        "Generated sender stub of the cross-binary task `{}` (M5.5). The producer application \
         declares nothing; `cargo xbin sync` derived this stub from the synced system view.",
        task.name
    );

    Ok(vec![
        syn::parse_quote! {
            #[doc = #stub_doc]
            pub struct #task_ident;
        },
        syn::parse_quote! {
            #[doc = #fifo_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #fifo_fn() -> *mut #rt_path::Fifo<#input_ty, #depth> {
                const __RTICX_XBIN_SOURCE_CORE: u32 = #source;
                const __RTICX_XBIN_TARGET_CORE: u32 = #target_core;
                const __RTICX_XBIN_FIFO_OFFSET: usize = #offset;

                const _: () = assert!(
                    __RTICX_XBIN_FIFO_OFFSET % #rt_path::FIFO_ALIGN == 0,
                    "`cargo xbin sync` allocated a misaligned FIFO offset",
                );
                #pool_consts

                // SAFETY: `system.json` places the FIFO at `offset`, 8-byte
                // aligned, inside the pool; `view_at` documents the remaining
                // requirements (one producer, one consumer, initialized memory),
                // which the doorbell/dispatcher protocol upholds. The base is
                // the literal synced address, or the distribution's override
                // for host/mock pools (D3).
                unsafe {
                    #rt_path::Fifo::<#input_ty, #depth>::view_at(
                        #base + __RTICX_XBIN_FIFO_OFFSET,
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
                    const __RTICX_XBIN_SOURCE_CORE: u32 = #source;
                    const __RTICX_XBIN_TASK_ID: u32 = #task_id;

                    const _: () = assert!(
                        core::mem::size_of::<#input_ty>() == #elem_size,
                        "the IDL layout of the spawn input changed; run `cargo xbin sync`",
                    );
                    const _: () = assert!(
                        core::mem::align_of::<#input_ty>() == #elem_align,
                        "the IDL layout of the spawn input changed; run `cargo xbin sync`",
                    );

                    // The caller must run on the task's synced producer core;
                    // a single-core binary folds this guard away (D5).
                    if #current_core != __RTICX_XBIN_SOURCE_CORE {
                        return Err(Some(input));
                    }

                    __rticx_interrupt_free(|| -> Result<(), Option<#input_ty>> {
                        let __rticx_xbin_fifo = #fifo_fn();
                        // SAFETY: this is the single producer core (checked
                        // above) and `__rticx_interrupt_free` serializes other
                        // local spawners.
                        if let Err(input) = unsafe { (*__rticx_xbin_fifo).enqueue(input) } {
                            return Err(Some(input));
                        }
                        // The ring function publishes the task id to the
                        // target's doorbell and triggers its router; a failed
                        // notification leaves the input enqueued (M6.5-T2).
                        match #ring_fn(__RTICX_XBIN_TASK_ID) {
                            Ok(()) => Ok(()),
                            Err(()) => Err(None),
                        }
                    })
                }
            }
        },
    ])
}

/// Resolves the Rust type of the sender stub's spawn input.
///
/// The producer declares nothing, so the IDL type is reached through the
/// backend's `ipc_types_path` (default `ipc_types`).
fn input_type(task: &TaskEntry, backend: &dyn XbinPassBackend) -> syn::Result<Type> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `rticx-sw-pass`'s `local_pend_fn_ident`: single-core
    /// applications keep the plain name, multi-core ones append the core.
    #[test]
    fn local_pend_fn_name_mirrors_the_software_pass() {
        assert_eq!(
            local_pend_fn_ident(0, 1).to_string(),
            "__rticx_local_irq_pend"
        );
        assert_eq!(
            local_pend_fn_ident(1, 2).to_string(),
            "__rticx_local_irq_pend_core1"
        );
    }

    /// The pool lists each core's lines in ascending `(source, priority)`
    /// order (M6.5-T1), independent of the declaration order.
    #[test]
    fn line_irq_indices_follow_the_pool_order() {
        let receiver = |name: &str, core: u32, spawn_by: u32, priority: u16| ReceiverDecl {
            name: name.to_string(),
            priority,
            capacity: 1,
            core,
            spawn_by,
            input_type: "Msg".to_string(),
        };
        let receivers = vec![
            receiver("B", 0, 0, 4),
            receiver("A", 0, 0, 3),
            receiver("C", 1, 2, 1),
        ];
        let indices = line_irq_indices(&receivers);
        assert_eq!(indices[&(0, 0, 3)], 0);
        assert_eq!(indices[&(0, 0, 4)], 1);
        assert_eq!(indices[&(1, 2, 1)], 0);
    }

    /// A backend that does not adopt the capability binding keeps the identity
    /// physical core and reports no reachable pool (M6.9-T1), and the hardware
    /// codegen hooks default to "nothing": no shared-memory configuration and
    /// no base override. Only `current_global_core_id` is required (D5).
    #[test]
    fn backend_defaults_are_identity_no_pools_and_no_hooks() {
        struct BareBackend;

        impl XbinPassBackend for BareBackend {
            fn rt_path(&self) -> syn::Path {
                syn::parse_quote!(rticx_xbin_rt)
            }

            fn current_global_core_id(&self) -> syn::Expr {
                syn::parse_quote!(0u32)
            }

            fn ring_doorbell_fn(
                &self,
                _source: u32,
                _target: u32,
                template: syn::ItemFn,
            ) -> syn::ItemFn {
                template
            }

            fn doorbell_interrupt(&self, _target: u32, _source: u32) -> Ident {
                format_ident!("IRQ")
            }

            fn read_doorbell_msg_fn(
                &self,
                _target: u32,
                _source: u32,
                template: syn::ItemFn,
            ) -> syn::ItemFn {
                template
            }
        }

        let backend = BareBackend;
        assert_eq!(backend.physical_core(4), PhysicalCore(4));
        assert!(backend.ipc_pools(0).is_empty());
        assert!(backend.configure_shared_memory(0).is_none());
        assert!(backend.ipc_base_override(0, 0, 1).is_none());
    }

    /// The endpoint base resolves each pool endpoint's own view, independent
    /// of the direction the pool is used in.
    #[test]
    fn pool_endpoint_base_resolves_each_endpoints_view() {
        let pool = PoolEntry {
            id: "p01".to_string(),
            core_a: 0,
            core_b: 1,
            base_from_a: 0x3004_0000,
            base_from_b: 0x1004_0000,
            budget: 4096,
            used: 100,
        };
        assert_eq!(pool_endpoint_base(&pool, 0), Some(0x3004_0000));
        assert_eq!(pool_endpoint_base(&pool, 1), Some(0x1004_0000));
        assert_eq!(pool_endpoint_base(&pool, 2), None);
    }
}
