//! Phase-2 code generation for the cross-binary tasks of this application
//! (M3-T1 sender side, M3-T2 receiver side).
//!
//! In codegen mode the pass has loaded the driver-generated `system.json`
//! (see `multibinary-multicore-plan.md` §8) and emits code from it:
//!
//! - for every view task whose `spawner_core` belongs to this application
//!   (M5.5: the producer declares nothing; the stubs are generated):
//!   - one **ring function** per `(source -> target)` pair the application
//!     produces into: `__rticx_xbin_ring_{source}_{target}(task_id)` publishes
//!     the task id to the target's doorbell and triggers its router. The pass
//!     generates only the documented signature; the body is the distribution's
//!     transport, filled through [`XbinPassBackend::ring_doorbell_fn`]
//!     (M6.5-T2);
//!   - a hidden **FIFO view** helper returning the fixed-address
//!     `rticx_xbin_rt::Fifo` of the task at
//!     `region.base_for(this_core, source, target) + offset`, where the region
//!     comes from the distribution's runtime backend and the offset, depth and
//!     element layout come from the system view (const addresses, no local input
//!     queue, no forwarder);
//!   - the task struct itself (`pub struct <Task>;`, the generated sender
//!     stub) and `Task::cross_spawn(input)`, matching the error semantics of
//!     the single-binary `cross_spawn`:
//!     - `Ok(())`: the input is enqueued and the target router was notified;
//!     - `Err(None)`: the input is enqueued, but the notification failed
//!       (do not retry the enqueue; re-notify the target);
//!     - `Err(Some(input))`: nothing was enqueued (the FIFO is full, the caller
//!       does not run on the expected core, or the target has not marked itself
//!       ready — including after a peer reset, which the spawner detects
//!       through its cached epoch, M6-T2); retry later or raise `capacity`.
//! - for every native `#[sw_task]` cross-binary receiver of this application
//!   (M3-T2, M5.5, M6.5-T3):
//!   - a hidden **FIFO view** helper like the sender's, but resolving the
//!     consumer view of the region (`base_from_target`);
//!   - a const assertion that the receiver's `<Task as
//!     RticSwTask>::SpawnInput` implements `rticx_xbin_rt::CrossCoreMessage`,
//!     so the guarantee lives in generated code instead of the trait
//!     definition;
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
//! - **Init hooks** (M3-T3, M6-T1): codegen mode also emits the hook functions
//!   and wires them into the generated entry functions through
//!   [`rticx_core::RticPass::main_injection`]:
//!   - `__rticx_xbin_configure_shared_memory` runs on every core at the start
//!     of its entry (`MainInjectionPoint::BeforeInit`, before the user
//!     `init`): MPU/MMU attributes are per-core, so each core maps its own
//!     view of the IPC regions Normal, Non-cacheable, Shareable;
//!   - `__rticx_xbin_init_shared` is generated for the application owning the
//!     project's **owner core** (the lowest global core id, i.e. the boot
//!     core) and runs `init_shared()` at `BeforePostInit`, before the owner's
//!     `mark_ready`;
//!   - `__rticx_xbin_init_fifos_core<N>` is generated for every local core
//!     that produces cross-binary tasks and runs at `BeforePostInit`: it
//!     zeroes the ring indices of the core's own FIFOs, so a topology whose
//!     owner core is not an endpoint of a region initializes correctly
//!     (M6-T1). The producer is always an endpoint of its region;
//!   - `__rticx_xbin_mark_ready_core<N>` runs on every core at the end of its
//!     `post_init` (`MainInjectionPoint::BeforeIdle`): it calls
//!     `mark_ready(core)`. The router IRQs are enabled and prioritized by the
//!     core pass's used-IRQ machinery through the generated router `#[task]`,
//!     so no per-line arming step remains (M6.5-T4).
//!
//!   Boot sequencing itself stays distribution-owned (for example the H7
//!   release of the M4 core): the owner must run its init before peers rely
//!   on the shared state.
//!
//! The enqueue happens inside `__rticx_interrupt_free`, the v1 sender-side
//! lock: the transport itself is kept independent of the lock mechanism, so a
//! future `Producer` shared-resource API (SRP) can replace it in one place.
//! The consumer side needs no lock: each FIFO has one producer core and one
//! line dispatcher, which runs under its line interrupt at a priority no
//! higher than the router that feeds its ready queue (M6.5-T3).
//!
//! The generated code only uses the frozen public surface of `rticx-core`
//! (`__rticx_interrupt_free`, `main_injection`), the generated `ipc-types` and
//! the distribution's re-export of `rticx-xbin-rt`, so no root-workspace API
//! changes are required.
//!
//! [`CrossBinBackend`]: rticx_xbin_rt::backend::CrossBinBackend

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use rticx_core::parser::ast::uppercase_ident;
use rticx_core::rticx_traits::HWT_TRAIT_TY;
use rticx_xbin_proto::{AppEntry, Hash64, ReceiverDecl, SystemView, TaskEntry, simple_type_name};
use syn::{Item, ItemMod, LitInt, LitStr, Type};

use crate::binding::{IpcPool, PhysicalCore};
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
    /// The generated producer stubs reach the IDL input type as
    /// `#ipc_types_path::<InputType>` (the producer declares nothing, M5.5).
    fn ipc_types_path(&self) -> syn::Path {
        syn::parse_quote!(ipc_types)
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
    /// delivered; the spawn input is already enqueued, so `cross_spawn` maps
    /// it to `Err(None)`.
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
    /// Local core index running `__rticx_xbin_init_shared`, when this
    /// application owns the project's owner core.
    owner_local: Option<u32>,
    /// Global core id of each local core index (`core_ids[local]`).
    global_ids: Vec<u32>,
    /// Local cores that produce at least one view task, running
    /// `__rticx_xbin_init_fifos_core<N>` before their `post_init` (M6-T1).
    fifo_locals: Vec<u32>,
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
            rticx_core::MainInjectionPoint::BeforePostInit => {
                self.global_ids.get(core as usize)?;
                let backend_expr = backend.backend();
                let mut statements = TokenStream::new();
                if self.owner_local == Some(core) {
                    statements.extend(quote! {
                        __rticx_xbin_init_shared(&#backend_expr);
                    });
                }
                if self.fifo_locals.contains(&core) {
                    // Each FIFO's indices are zeroed by its producer core,
                    // before its `post_init` can spawn (M6-T1).
                    let init_fn = format_ident!("__rticx_xbin_init_fifos_core{core}");
                    statements.extend(quote! {
                        #init_fn(&#backend_expr);
                    });
                }
                (!statements.is_empty()).then_some(statements)
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
    // const assertion pins the guarantee to the native `RticSwTask` trait
    // (M5.5) instead of relying on a bound in its definition.
    let rt_path = backend.rt_path();
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
            fn __rticx_xbin_assert_cross_core_message<T: #rt_path::CrossCoreMessage>() {}
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
    let backend_expr = backend.backend();
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
                let __rticx_xbin_backend = #backend_expr;
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

/// Generates the init hooks of one application (M3-T3, M6-T1): the per-core
/// `__rticx_xbin_configure_shared_memory`, the owner's
/// `__rticx_xbin_init_shared`, one `__rticx_xbin_init_fifos_core<N>` per
/// producing core and the per-core `__rticx_xbin_mark_ready_core<N>`.
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
        let mark_ready_doc = format!(
            "Marks this core (global core {global}) ready at the end of its `post_init` (M3-T3). \
             The router IRQs are enabled and prioritized by the core pass's used-IRQ machinery, so \
             no per-line doorbell arming step remains (M6.5-T4)."
        );
        let mark_ready_fn = format_ident!("__rticx_xbin_mark_ready_core{local}");
        items.push(syn::parse_quote! {
            #[doc = #mark_ready_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #mark_ready_fn(__rticx_xbin_backend: &impl #rt_path::CrossBinBackend) {
                __rticx_xbin_backend.mark_ready(#global);
            }
        });
    }

    if let Some(owner) = owner
        && owner_local.is_some()
    {
        let init_doc = format!(
            "Initializes the shared ready/epoch state on the owner core (global core {owner}); runs \
             before the owner is marked ready (M3-T3). The per-task FIFO indices are zeroed by \
             each FIFO's producer core, in its own `__rticx_xbin_init_fifos_core<N>` (M6-T1)."
        );
        items.push(syn::parse_quote! {
            #[doc = #init_doc]
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn __rticx_xbin_init_shared(
                __rticx_xbin_backend: &impl #rt_path::CrossBinBackend,
            ) {
                __rticx_xbin_backend.init_shared();
            }
        });
    }

    // Every core zeroes the FIFOs it produces, before its `post_init` can
    // spawn. The producer is always an endpoint of its region, so a topology
    // whose owner core is not an endpoint still initializes correctly
    // (M6-T1).
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
            fn #init_fn(__rticx_xbin_backend: &impl #rt_path::CrossBinBackend) {
                #(#zero_fifos)*
            }
        });
    }

    Ok(InitHooks {
        items,
        plan: HookPlan {
            owner_local,
            global_ids: application.core_ids.clone(),
            fifo_locals,
        },
    })
}

/// Generates the FIFO-zeroing blocks of the `producer` core's initializer.
///
/// The producer of a FIFO is an endpoint of its `(source -> target)` region by
/// construction, so [`rticx_xbin_rt::backend::IpcRegion::base_for`] always
/// resolves; a topology whose owner core is not an endpoint of the region
/// initializes correctly (M6-T1).
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

        let missing_region = format!(
            "`cargo xbin sync` allocated the `({source} -> {target})` IPC region; re-run it \
             after changing `rticx.toml`"
        );
        let wrong_core = format!(
            "the producer core is not an endpoint of the `({source} -> {target})` IPC region"
        );
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
                // inside the region; the producer core runs this before its
                // `post_init` can spawn and no consumer acts before a
                // notification, and `Fifo::init` documents why it must not
                // race one.
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
// TODO(M6): let a distribution or `rticx.toml` designate a different owner.
fn owner_core(view: &SystemView) -> Option<u32> {
    view.cores.iter().map(|core| core.global_id).min()
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
    })
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

/// Generates the sender stub of one view task: the `pub struct <Task>;` the
/// producer's source does not declare (M5.5), its FIFO view and its
/// `cross_spawn` impl, including the cached-epoch target-ready gate
/// (M6-T2).
fn generate_sender(
    view: &SystemView,
    task: &TaskEntry,
    backend: &dyn XbinPassBackend,
) -> syn::Result<Vec<Item>> {
    let task_ident = ident(&task.name)?;
    let input_ty = input_type(task, backend)?;
    let rt_path = backend.rt_path();
    let backend_expr = backend.backend();

    let source = task.fifo.source;
    let target_core = task.fifo.target;
    let task_id = task.id;
    let offset = task.fifo.offset as usize;
    let depth = task.fifo.depth as usize;
    let elem_size = task.fifo.elem_size as usize;
    let elem_align = input_align(view, task)? as usize;

    let fifo_fn = format_ident!("__rticx_xbin_fifo_{}", task.name);
    let ring_fn = format_ident!("__rticx_xbin_ring_{source}_{target_core}");
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
        "Cross-binary spawn: enqueue `input` into the `{}` FIFO and notify the router of global \
         core {target_core}.\n\n\
         # Returns\n\n\
         - `Ok(())`: the input is enqueued and the target router was notified;\n\
         - `Err(None)`: the input is enqueued, but the notification failed. Do **not** \
         retry the spawn; re-notify the target instead;\n\
         - `Err(Some(input))`: nothing was enqueued (the FIFO is full, the caller does not \
         run on global core {source}, or the target core {target_core} has not marked itself \
         ready). Retry later or raise `capacity`.",
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
                    const __RTICX_XBIN_TASK_ID: u32 = #task_id;

                    // Last shared epoch this spawner observed with the target
                    // ready. A peer reset or a reinitialization moves the epoch
                    // on, so the cache refreshes once and the spawn is rejected
                    // until the target re-marks itself ready in the new epoch
                    // (M6-T2).
                    static __RTICX_XBIN_READY: #rt_path::ReadyCache = #rt_path::ReadyCache::new();

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
                    if !__RTICX_XBIN_READY.is_ready(
                        __rticx_xbin_backend.shared_state(),
                        __RTICX_XBIN_TARGET_CORE,
                    ) {
                        // The target has not marked itself ready yet, or it
                        // reset without re-marking: return the input without
                        // enqueueing (M6-T2).
                        return Err(Some(input));
                    }

                    __rticx_interrupt_free(|| -> Result<(), Option<#input_ty>> {
                        let __rticx_xbin_fifo = #fifo_fn(&__rticx_xbin_backend);
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
    /// physical core and reports no reachable pool (M6.9-T1): the defaults are
    /// the "no distro capability information" fallback.
    #[test]
    fn capability_binding_defaults_to_identity_and_no_pools() {
        struct BareBackend;

        impl XbinPassBackend for BareBackend {
            fn backend(&self) -> syn::Expr {
                syn::parse_quote!(())
            }

            fn rt_path(&self) -> syn::Path {
                syn::parse_quote!(rticx_xbin_rt)
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
    }
}
