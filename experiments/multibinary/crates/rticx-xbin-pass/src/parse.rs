//! Parsing of the `#[app]` additions and the native cross-binary task
//! declarations.
//!
//! This module owns the syntax that the multi-binary extension adds on top of
//! the core RTIC syntax:
//!
//! - the **front of `#[app(dispatchers = [..])]`**: the shared interrupt pool
//!   also read by `rticx-sw-pass`/`rticx-async-pass`. The xbin pass
//!   steals the first `K` entries — one per distinct `(source, priority)`
//!   cross line of the application, in ascending `(source, priority)` order —
//!   and binds them to the generated cross-binary line dispatchers. The
//!   remaining entries are written back into the `#[app]` arguments for the
//!   software/async passes to assign to the local priority groups. Convention:
//!   **cross-binary lines first, then the local task dispatchers**; duplicates
//!   inside the stolen prefix are rejected. The list is **flat**: an
//!   application owns exactly one core, so the old nested one-array-per-core
//!   shape is a hard error.
//!
//! `rticx.toml` is the single user-facing source of project topology:
//! [`parse_app_args`] takes the resolved application core id (or the standalone
//! [`AppTopology::identity`] fallback for a binary with no governing project)
//! and **injects** `core_ids = [core_id]` into the arguments for
//! `rticx-core`.
//!
//! - native `#[sw_task(..)]` structs with `impl RticSwTask { type SpawnInput
//!   = …; }` blocks. `spawn_by` **always** names a global producer core id, so
//!   a `#[sw_task]` carrying `spawn_by` is a cross-binary receiver; when the
//!   id is this application's own core the task is core-local and the key is a
//!   hard error. A `#[sw_task]` without `spawn_by` stays a plain local
//!   software task. Task `core` is always the local index `0` (the application
//!   owns exactly one core) and a cross receiver may **not** declare `core`.
//!
//! Besides the cross receivers, [`parse_module`] records every local
//! `#[sw_task]`/`#[async_task]` declaration (its `priority`, `core` and origin
//! core) in [`ModuleDecls::local_tasks`]. Phase 2 combines them with the synced
//! view in [`crate::priority::validate_priority_lines`], which enforces one
//! origin core per `(target core, priority)` line; the declarations themselves
//! are left untouched, so the software and async passes keep owning their
//! syntax and errors.
//!
//! Everything is parsed into the plain data types of `rticx-xbin-proto` that
//! the metadata manifest is made of. In codegen mode the receiver structs are
//! additionally rewritten into the core `#[task(..)]` items the generated
//! dispatchers run against (see [`inject_receiver_tasks`]); in metadata mode
//! their parsed attributes are stripped from the emitted module.
//!
//! Confirmed syntax:
//!
//! - a cross receiver declares exactly one global producer core in `spawn_by`
//!   (an array is rejected: a task has exactly one producer core);
//! - defaults: `priority = 1` (cross receivers require `priority >= 1`),
//!   `capacity = 1` (minimum 1); the injected `#[task]` runs on local core 0;
//! - unknown, duplicate and multi-segment keys as well as malformed values
//!   are hard errors;
//! - stripping happens in both modes and for every distribution that binds
//!   the pass. `ipc_dispatchers` no longer exists: an application still
//!   declaring it keeps the key in its arguments and the core pass warns
//!   about it as an unknown `#[app]` argument (`core_ids` is native to
//!   `rticx-core` and is understood with or without the pass).
//!
//! The producer application declares nothing: its `Task::cross_spawn` stubs
//! are generated in codegen mode from the synced system view.
//!
//! TODO(extract): `parse_attr_int` and `type_path_string` duplicate helpers in
//! `rticx-sw-pass`'s internal modules. They may be promoted into `rticx-core`
//! once a second pass needs them (`take_u32_array` already lives on
//! [`RticAttr`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::str::FromStr;

use proc_macro2::{Ident, Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use rticx_core::parse_utils::RticAttr;
use rticx_core::parser::ast::AppArgs;
use rticx_xbin_proto::ReceiverDecl;
use syn::{
    Attribute, Expr, ExprLit, ImplItem, Item, ItemImpl, ItemMod, Lit, LitInt, Meta, Path,
    PathArguments, Token, Type, parse::Parser, parse_quote, punctuated::Punctuated,
    spanned::Spanned,
};

use crate::LOCAL_CORE;
use crate::topology::AppTopology;

/// `#[app(core_ids = [..])]`: replaced by the scalar `core_id` in `rticx.toml`.
pub(crate) const CORE_IDS_ARG: &str = "core_ids";

/// The shared `#[app(dispatchers = [..])]` argument: the xbin pass steals the
/// front (one entry per cross line) and rewrites the remainder for
/// `rticx-sw-pass`/`rticx-async-pass`.
const DISPATCHERS_ARG: &str = "dispatchers";
/// Native software-task attribute.
pub(crate) const SW_TASK_ATTR: &str = "sw_task";
/// Native async-task attribute, read (never consumed) by this pass for the
/// priority-line validation; the async pass owns its syntax.
pub(crate) const ASYNC_TASK_ATTR: &str = "async_task";
/// `#[sw_task(spawn_by = G)]`: the global producer core id.
pub(crate) const SPAWN_BY_ARG: &str = "spawn_by";
/// `#[sw_task(init = generated)]`: accepted for parity with the native
/// software-task syntax; the pass generates the receiver's init.
pub(crate) const INIT_ARG: &str = "init";
/// The only `init` value a cross receiver may declare.
pub(crate) const INIT_GENERATED: &str = "generated";
/// Software-task trait (last path segment), carrying `type SpawnInput`.
pub(crate) const SW_TASK_TRAIT: &str = "RticSwTask";
/// Associated type of [`SW_TASK_TRAIT`] declaring the task input.
pub(crate) const SPAWN_INPUT_ASSOC: &str = "SpawnInput";

/// Default cross-receiver priority (`1` is the lowest active priority;
/// `rticx-sw-pass` defaults native software tasks to `0`).
const DEFAULT_PRIORITY: u16 = 1;
/// Default FIFO capacity (one pending spawn), matching `rticx-sw-pass`.
const DEFAULT_CAPACITY: usize = 1;

/// The `#[app]` arguments owned by this pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppExtensions {
    /// The application's single, globally unique core id (`rticx.toml`).
    pub core_id: u32,
    /// Resolved `device` PAC path.
    ///
    /// The router pends its line dispatchers through the same interrupt type
    /// the software pass uses: `pac::Interrupt` unless the backend provides a
    /// `custom_interrupt_path`.
    pub pac: syn::Path,
    /// The application's flat `dispatchers` argument as written by the user.
    ///
    /// The xbin pass steals its first `K` entries (one per cross line, in
    /// ascending `(source, priority)` order) into [`Self::cross_dispatchers`];
    /// the remainder is written back into the `#[app]` arguments for the
    /// software/async passes.
    pub dispatchers: Vec<Path>,
    /// The stolen front of [`Self::dispatchers`].
    ///
    /// Holds one interrupt path per distinct `(spawn_by, priority)` cross line
    /// of the application, in ascending `(source, priority)` order; consumed by
    /// `codegen.rs`.
    pub cross_dispatchers: Vec<Path>,
}

/// The cross-binary declarations found inside the `#[app]` module.
#[derive(Debug, Default, Clone)]
pub(crate) struct ModuleDecls {
    /// Cross-binary receivers, sorted by name.
    pub receivers: Vec<ReceiverDecl>,
    /// Local `#[sw_task]`/`#[async_task]` declarations (excluding the cross
    /// receivers), sorted by name, recorded for the build-phase priority-line
    /// validation. Their attributes are left for the software/async
    /// passes.
    pub local_tasks: Vec<LocalTaskDecl>,
    /// Span of every cross-binary receiver declaration, keyed by task name,
    /// so the priority-line validation can point at the receiver.
    pub receiver_spans: BTreeMap<String, Span>,
}

/// Whether a local task declaration is a `#[sw_task]` or an `#[async_task]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalTaskKind {
    /// A native `#[sw_task]`.
    Software,
    /// A native `#[async_task]`.
    Async,
}

impl LocalTaskKind {
    /// Human-readable name used in priority-line errors.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Software => "software task",
            Self::Async => "async task",
        }
    }
}

/// A local task declaration, reduced to what the build-phase priority-line
/// validation needs.
#[derive(Debug, Clone)]
pub(crate) struct LocalTaskDecl {
    /// Task struct name.
    pub name: String,
    /// Whether this is a software or an async task.
    pub kind: LocalTaskKind,
    /// Declared priority (`0` when absent, matching the software/async
    /// passes).
    pub priority: u16,
    /// Global id of the core that executes the task (the application's
    /// `core_id`).
    pub target_core: u32,
    /// Global id of the task's origin core: its `spawn_by` resolved to a global
    /// id, or its own core when `spawn_by` is absent.
    pub origin_core: u32,
    /// Span of the task struct, for error reporting.
    pub span: Span,
}

/// Parses the `#[app]` arguments owned by this pass and returns them together
/// with the parsed attribute, so [`steal_dispatchers`] can rewrite the shared
/// `dispatchers` argument once the receivers are known.
///
/// `topology` is `None` for an application with no governing project: the pass
/// then falls back to [`AppTopology::identity`] (`core_id = 0`), keeping a
/// standalone xbin distribution usable.
pub(crate) fn parse_app_args(
    args: TokenStream,
    topology: Option<AppTopology>,
) -> syn::Result<(AppExtensions, RticAttr)> {
    let args_span = args.span();

    let mut attr = parse_attr_tokens(args, format_ident!("app"))?;

    let topology = topology.unwrap_or_else(AppTopology::identity);
    let core_id = topology.core_id();

    // Inject the resolved scalar `core_ids` for the core pass: it owns and
    // validates the key, so the pass hands it the value `rticx.toml` resolved
    // (the core pass never sees the removed `#[app]` keys). `cores` stays
    // absent and defaults to `1`.
    let core_id_lit = LitInt::new(&core_id.to_string(), Span::call_site());
    attr.elements
        .insert(CORE_IDS_ARG.to_string(), syn::parse_quote!([#core_id_lit]));

    // `rticx-core` owns `core_ids`: parsing through `AppArgs` validates the
    // injected `core_ids` (length and uniqueness) with the core pass's own
    // errors and resolves the `device` PAC path.
    let app_args = AppArgs::parse(attr.args_tokens())?;
    let pac =
        app_args.pacs.into_iter().next().ok_or_else(|| {
            syn::Error::new(args_span, "`device` must name a path to a PAC crate")
        })?;

    // The shared `dispatchers` argument is parsed here with the same shape
    // rules as `rticx-sw-pass`. The key is left in `attr` (not consumed):
    // [`steal_dispatchers`] takes the cross-binary front once the receivers
    // are known and rewrites the remainder for the software pass.
    let dispatchers = match attr.get_expr(DISPATCHERS_ARG) {
        Some(expr) => parse_dispatchers(expr)?,
        None => Vec::new(),
    };

    let extensions = AppExtensions {
        core_id,
        pac,
        dispatchers,
        cross_dispatchers: Vec::new(),
    };
    Ok((extensions, attr))
}

/// Parses the shared `#[app(dispatchers = [..])]` argument into a flat list of
/// interrupt paths.
///
/// A cross-binary application owns exactly one core, so the argument is a
/// **flat** array of interrupt paths; the old nested one-array-per-core shape
/// is rejected. An empty array declares no dispatcher.
fn parse_dispatchers(expr: &Expr) -> syn::Result<Vec<Path>> {
    let Expr::Array(array) = expr else {
        return Err(syn::Error::new(
            expr.span(),
            format!(
                "The `{DISPATCHERS_ARG}` argument must be an array of interrupt paths, e.g. \
                 `{DISPATCHERS_ARG} = [IRQ0, IRQ1]`"
            ),
        ));
    };

    // An empty array declares no dispatcher.
    if array.elems.is_empty() {
        return Ok(Vec::new());
    }

    // The nested one-array-per-core shape is gone with the multi-core model.
    if array
        .elems
        .iter()
        .all(|element| matches!(element, Expr::Array(_)))
    {
        return Err(syn::Error::new(
            array.span(),
            format!(
                "The `{DISPATCHERS_ARG}` argument must be a flat list of interrupt paths; an \
                 application owns exactly one core, so the nested one-array-per-core shape was \
                 removed. Write `{DISPATCHERS_ARG} = [IRQ0, IRQ1]`"
            ),
        ));
    }

    array.elems.iter().map(dispatcher_path).collect()
}

/// Reads one element of a `dispatchers` list as an interrupt path.
fn dispatcher_path(element: &Expr) -> syn::Result<Path> {
    match element {
        Expr::Path(path) if path.qself.is_none() => Ok(path.path.clone()),
        other => Err(syn::Error::new(
            other.span(),
            format!("The elements of the `{DISPATCHERS_ARG}` argument must be interrupt paths."),
        )),
    }
}

/// Steals the cross-binary front of the shared `dispatchers` list and rewrites
/// the argument to the remainder.
///
/// The pass computes its distinct cross lines `(spawn_by, priority)`
/// (ascending) and takes that many leading entries of `dispatchers`: the `i`-th
/// entry is bound to the `i`-th line. The leftovers are written back into
/// `attr`'s `dispatchers` for `rticx-sw-pass`/`rticx-async-pass`, which assign
/// them to the local priority groups exactly as before. An absent key stays
/// absent.
///
/// Errors when the list is too short (the missing cross line is named) or when
/// the stolen prefix repeats an interrupt (two lines cannot share one). Surplus
/// entries are not an error: they are the local task dispatchers.
pub(crate) fn steal_dispatchers(
    extensions: &mut AppExtensions,
    decls: &ModuleDecls,
    attr: &mut RticAttr,
) -> syn::Result<()> {
    let mut lines: Vec<(u32, u16)> = decls
        .receivers
        .iter()
        .map(|receiver| (receiver.spawn_by, receiver.priority))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let needed = lines.len();

    let entries = &extensions.dispatchers;
    if entries.len() < needed {
        let (source, priority) = lines[entries.len()];
        return Err(syn::Error::new(
            entries.first().map_or_else(Span::call_site, Spanned::span),
            format!(
                "this application has the cross line `(source {source}, priority {priority})`, \
                 but `{DISPATCHERS_ARG}` declares only {} entr{}; declare one interrupt per cross \
                 line at the front of `{DISPATCHERS_ARG}`",
                entries.len(),
                if entries.len() == 1 { "y" } else { "ies" }
            ),
        ));
    }

    let stolen = entries[..needed].to_vec();
    let mut seen = BTreeSet::new();
    for entry in &stolen {
        let rendered = path_key(entry);
        if !seen.insert(rendered.clone()) {
            return Err(syn::Error::new(
                entry.span(),
                format!(
                    "`{DISPATCHERS_ARG}` lists `{rendered}` more than once; each cross line needs \
                     its own interrupt line"
                ),
            ));
        }
    }

    // Rewrite the argument to the leftovers. A key the user did not write stays
    // absent.
    if attr.get_expr(DISPATCHERS_ARG).is_some() {
        let remainder = &extensions.dispatchers[needed..];
        attr.elements.insert(
            DISPATCHERS_ARG.to_string(),
            syn::parse_quote!([#(#remainder),*]),
        );
    }

    extensions.cross_dispatchers = stolen;
    Ok(())
}

/// Canonical rendering of an interrupt path for duplicate/overlap checks.
fn path_key(path: &Path) -> String {
    path.to_token_stream().to_string()
}

/// Parses (and rewrites) the native cross-binary receivers of the `#[app]`
/// module.
///
/// `spawn_by` always names a **global** producer core id, so any `#[sw_task]`
/// carrying a `spawn_by` that differs from the application's own `core_id` is a
/// cross-binary receiver; it is recorded in the manifest with its `spawn_by`
/// stripped, so the software pass never sees a global id it would compare
/// against a local index. A `spawn_by` naming the application's own core is a
/// core-local task and a hard error.
///
/// Returns every cross-binary receiver sorted by name.
pub(crate) fn parse_module(
    app_mod: &mut ItemMod,
    extensions: &AppExtensions,
) -> syn::Result<ModuleDecls> {
    let Some((_, items)) = app_mod.content.as_mut() else {
        return Ok(ModuleDecls::default());
    };

    let inputs = collect_spawn_input_types(items)?;
    let mut decls = ModuleDecls::default();

    for item in items.iter_mut() {
        let Item::Struct(item_struct) = item else {
            continue;
        };

        // Native async tasks are only *read* here for the build-phase
        // priority-line validation; the async pass owns their syntax,
        // so malformed arguments are left for it to report.
        if let Some(index) = find_attr(&item_struct.attrs, ASYNC_TASK_ATTR)
            && let Ok(attr) = RticAttr::from_meta(&item_struct.attrs[index].meta)
        {
            record_local_task(
                &mut decls,
                &item_struct.ident,
                &attr,
                extensions,
                LocalTaskKind::Async,
            );
            continue;
        }

        let Some(index) = find_attr(&item_struct.attrs, SW_TASK_ATTR) else {
            continue;
        };
        let name = item_struct.ident.clone();
        let mut attr = parse_item_attr(&item_struct.attrs[index])?;
        attr.ensure_supported(&[
            "priority",
            "capacity",
            "core",
            SPAWN_BY_ARG,
            INIT_ARG,
            "shared",
        ])?;

        // `spawn_by` absent: a plain local software task, untouched; recorded
        // with its own core as the origin for the priority-line validation.
        let Some(span) = attr.get_expr(SPAWN_BY_ARG).map(Spanned::span) else {
            record_local_task(
                &mut decls,
                &name,
                &attr,
                extensions,
                LocalTaskKind::Software,
            );
            continue;
        };

        // A cross-binary application owns exactly one core: a `spawn_by`
        // naming it is a core-local task, not a receiver.
        let producer = take_spawn_by(&mut attr, span)?;
        if producer == extensions.core_id {
            return Err(syn::Error::new(
                span,
                format!(
                    "`{name}` declares `{SPAWN_BY_ARG} = {producer}`, which is this application's \
                     own core id; it is a core-local task, so drop `{SPAWN_BY_ARG}`"
                ),
            ));
        }

        // Cross-binary receiver: recorded here, invisible to sw-pass.
        decls.receiver_spans.insert(name.to_string(), name.span());
        decls.receivers.push(parse_receiver(
            name, &attr, &inputs, extensions, producer, span,
        )?);

        // Re-render the rewritten attribute (minus `spawn_by`) in place for the
        // software pass.
        item_struct.attrs[index] = render_attribute(&attr);
    }

    decls.receivers.sort_by(|a, b| a.name.cmp(&b.name));
    decls.local_tasks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(decls)
}

/// Records a local (non-cross) task for the build-phase priority-line
/// validation.
///
/// Reads `priority`, `core` and `spawn_by` without consuming them, so the
/// software and async passes still own their syntax and errors. A declaration
/// whose values do not resolve (out of range, non-integer) is skipped: the
/// owning pass reports it with its own precise error.
fn record_local_task(
    decls: &mut ModuleDecls,
    name: &Ident,
    attr: &RticAttr,
    extensions: &AppExtensions,
    kind: LocalTaskKind,
) {
    let Ok(priority) = parse_attr_int(attr, "priority", 0u16) else {
        return;
    };
    let Ok(core) = parse_attr_int(attr, "core", 0u32) else {
        return;
    };
    // A cross-binary application owns exactly one local core.
    if core != LOCAL_CORE {
        return;
    }
    let target_core = extensions.core_id;
    let origin_core = match attr.get_expr(SPAWN_BY_ARG) {
        None => target_core,
        Some(_) => {
            // Local tasks keep RTIC's local-index `spawn_by`; with one core
            // only `spawn_by = 0` is meaningful and maps to the global id.
            let Ok(spawn_by) = parse_attr_int(attr, SPAWN_BY_ARG, LOCAL_CORE) else {
                return;
            };
            if spawn_by != LOCAL_CORE {
                return;
            }
            target_core
        }
    };

    decls.local_tasks.push(LocalTaskDecl {
        name: name.to_string(),
        kind,
        priority,
        target_core,
        origin_core,
        span: name.span(),
    });
}

/// Reads the singular `spawn_by` producer core id.
///
/// Arrays are rejected outright: v1 has one SPSC FIFO and one priority line
/// per task, so a task has exactly one producer core.
fn take_spawn_by(attr: &mut RticAttr, span: Span) -> syn::Result<u32> {
    if let Some(Expr::Array(array)) = attr.get_expr(SPAWN_BY_ARG) {
        return Err(syn::Error::new(
            array.span(),
            format!(
                "`{SPAWN_BY_ARG}` must be a single core id, not an array; a cross-binary task \
                 has exactly one producer core"
            ),
        ));
    }
    attr.take_u32(SPAWN_BY_ARG)?.ok_or_else(|| {
        syn::Error::new(span, format!("`{SPAWN_BY_ARG}` must be an integer literal"))
    })
}

/// Rejects an `init` argument other than `init = generated`.
///
/// Cross-binary receivers always run through the pass-generated init (their
/// inputs arrive through the FIFO, so there is no user init path); the key is
/// accepted for parity with the native `#[sw_task]` syntax.
fn check_init_generated(attr: &RticAttr, name: &Ident) -> syn::Result<()> {
    let Some(expr) = attr.get_expr(INIT_ARG) else {
        return Ok(());
    };
    if matches!(expr, Expr::Path(path) if path.path.is_ident(INIT_GENERATED)) {
        return Ok(());
    }
    Err(syn::Error::new(
        expr.span(),
        format!(
            "cross-binary receiver `{name}` must use `{INIT_ARG} = {INIT_GENERATED}`; its init \
             is generated by the cross-binary pass"
        ),
    ))
}

/// Builds the cross-binary receiver declaration, validating the remaining
/// attribute keys.
fn parse_receiver(
    name: Ident,
    attr: &RticAttr,
    inputs: &BTreeMap<String, String>,
    extensions: &AppExtensions,
    spawn_by: u32,
    spawn_by_span: Span,
) -> syn::Result<ReceiverDecl> {
    check_init_generated(attr, &name)?;

    // A cross receiver always runs on the application's single core; the pass
    // injects `core = 0` on the generated `#[task]`, so the key is not a
    // user-facing knob.
    if let Some(expr) = attr.get_expr("core") {
        return Err(syn::Error::new(
            expr.span(),
            format!(
                "cross-binary receiver `{name}` must not declare `core`: a cross-binary receiver \
                 always runs on this application's single core (global core {}); remove the `core` \
                 key",
                extensions.core_id
            ),
        ));
    }

    let priority = parse_attr_int(attr, "priority", DEFAULT_PRIORITY)?;
    if priority == 0 {
        return Err(syn::Error::new(
            int_span(attr, "priority").unwrap_or(spawn_by_span),
            format!(
                "cross-binary receiver `{name}` requires `priority >= 1`; `priority = 0` is \
                 reserved for the idle task"
            ),
        ));
    }
    let capacity = parse_attr_int(attr, "capacity", DEFAULT_CAPACITY)?;
    if capacity == 0 {
        return Err(syn::Error::new(
            int_span(attr, "capacity").unwrap_or_else(|| name.span()),
            "The `capacity` argument must be at least 1.",
        ));
    }

    let input_type = inputs.get(&name.to_string()).cloned().ok_or_else(|| {
        syn::Error::new(
            name.span(),
            format!(
                "cross-binary receiver `{name}` must have an `impl {SW_TASK_TRAIT} for {name}` \
                 block declaring `type {SPAWN_INPUT_ASSOC} = …;` inside the `#[app]` module"
            ),
        )
    })?;

    Ok(ReceiverDecl {
        name: name.to_string(),
        priority,
        capacity,
        spawn_by,
        input_type,
    })
}

/// Rewrites every cross-binary receiver struct into the `#[task(..)]` item
/// the core pass consumes.
///
/// Only codegen mode calls this: the injected attribute is what makes the
/// core pass generate the task static and enforce the user's `impl
/// RticSwTask` (`task_trait = RticSwTask`), so the generated dispatcher can
/// call `exec(input)` on the initialized instance. The attribute carries only
/// keys the core pass understands, so no core changes are required; `shared`
/// is passed through untouched (the core pass computes the SRP ceilings).
pub(crate) fn inject_receiver_tasks(app_mod: &mut ItemMod, receivers: &[ReceiverDecl]) {
    let Some((_, items)) = app_mod.content.as_mut() else {
        return;
    };

    for item in items.iter_mut() {
        let Item::Struct(item_struct) = item else {
            continue;
        };
        let Some(receiver) = receivers
            .iter()
            .find(|receiver| item_struct.ident == receiver.name)
        else {
            continue;
        };
        let Some(index) = find_attr(&item_struct.attrs, SW_TASK_ATTR) else {
            continue;
        };

        let Ok(mut attr) = RticAttr::parse_from_attr(&item_struct.attrs[index]) else {
            continue;
        };
        // `shared` is passed through untouched (the core pass computes the
        // SRP ceilings); `spawn_by` and `capacity` are consumed by this pass.
        let shared = attr.elements.remove("shared");
        let shared = shared.map(|expr| quote!(, shared = #expr));
        let priority = LitInt::new(&receiver.priority.to_string(), item_struct.ident.span());
        // A cross-binary receiver always runs on the application's single core.
        let core = LitInt::new(&LOCAL_CORE.to_string(), item_struct.ident.span());
        item_struct.attrs[index] = parse_quote! {
            #[task(priority = #priority, core = #core, task_trait = RticSwTask,
                   init = generated #shared)]
        };
    }
}

/// Removes the `#[sw_task(..)]` attribute of every cross-binary receiver.
///
/// Without it the software pass of a metadata `cargo check` would treat the
/// cross receiver as a local software task and miss the global `spawn_by`;
/// the declaration is already recorded in the manifest, so the attribute is
/// removed from the emitted module and only the struct and its `impl
/// RticSwTask` block remain.
pub(crate) fn strip_receiver_tasks(app_mod: &mut ItemMod, receivers: &[ReceiverDecl]) {
    let Some((_, items)) = app_mod.content.as_mut() else {
        return;
    };

    for item in items.iter_mut() {
        let Item::Struct(item_struct) = item else {
            continue;
        };
        if !receivers
            .iter()
            .any(|receiver| item_struct.ident == receiver.name)
        {
            continue;
        }
        if let Some(index) = find_attr(&item_struct.attrs, SW_TASK_ATTR) {
            item_struct.attrs.remove(index);
        }
    }
}

/// Returns the index of the first `#[<name>]` attribute in `attrs`, if any.
fn find_attr(attrs: &[Attribute], name: &str) -> Option<usize> {
    attrs.iter().position(|attr| attr.path().is_ident(name))
}

/// `type SpawnInput = …;` declarations found in `impl RticSwTask` blocks,
/// keyed by the implementing type name.
fn collect_spawn_input_types(items: &[Item]) -> syn::Result<BTreeMap<String, String>> {
    let mut inputs = BTreeMap::new();

    for item in items {
        let Item::Impl(impl_item) = item else {
            continue;
        };
        if !implements_sw_task(impl_item) {
            continue;
        }
        let Some(self_ty) = impl_self_ident(impl_item) else {
            continue;
        };
        let Some(input) = find_spawn_input_type(impl_item) else {
            continue;
        };
        let name = self_ty.to_string();
        if inputs.contains_key(&name) {
            return Err(syn::Error::new(
                self_ty.span(),
                format!(
                    "duplicate `impl {SW_TASK_TRAIT}` for `{name}` with `type \
                     {SPAWN_INPUT_ASSOC}`"
                ),
            ));
        }
        inputs.insert(name, type_path_string(&input));
    }

    Ok(inputs)
}

/// Whether the `impl` block is an `impl RticSwTask for …` (last path segment
/// matched, so qualified paths like `crate::RticSwTask` are recognized).
fn implements_sw_task(impl_item: &ItemImpl) -> bool {
    impl_item
        .trait_
        .as_ref()
        .and_then(|(_, path, _)| path.segments.last())
        .is_some_and(|last| last.ident == SW_TASK_TRAIT)
}

fn impl_self_ident(impl_item: &ItemImpl) -> Option<&Ident> {
    match impl_item.self_ty.as_ref() {
        Type::Path(path) if path.qself.is_none() => {
            path.path.segments.last().map(|segment| &segment.ident)
        }
        _ => None,
    }
}

fn find_spawn_input_type(impl_item: &ItemImpl) -> Option<Type> {
    impl_item.items.iter().find_map(|item| match item {
        ImplItem::Type(assoc) if assoc.ident == SPAWN_INPUT_ASSOC => Some(assoc.ty.clone()),
        _ => None,
    })
}

/// Renders a type path in the canonical `a::b::C` form (no whitespace), so
/// manifests are deterministic. Types outside the supported subset fall back
/// to their token rendering.
fn type_path_string(ty: &Type) -> String {
    if let Type::Path(type_path) = ty
        && type_path.qself.is_none()
        && type_path
            .path
            .segments
            .iter()
            .all(|segment| matches!(segment.arguments, PathArguments::None))
    {
        let mut out = String::new();
        if type_path.path.leading_colon.is_some() {
            out.push_str("::");
        }
        let segments: Vec<String> = type_path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        out.push_str(&segments.join("::"));
        return out;
    }
    ty.to_token_stream().to_string()
}

/// Renders `attr` back into a `#[name(..)]` item attribute.
///
/// [`RticAttr`]'s [`ToTokens`] emits the complete attribute; `syn` has no
/// `Parse` impl for [`Attribute`], so it is parsed with
/// [`Attribute::parse_outer`].
fn render_attribute(attr: &RticAttr) -> Attribute {
    let mut attributes = Attribute::parse_outer
        .parse2(attr.to_token_stream())
        .expect("an RticAttr renders to a valid attribute");
    attributes
        .pop()
        .expect("an RticAttr renders exactly one attribute")
}

/// Parses `key = value, …` arguments after rejecting malformed keys that
/// [`RticAttr`] would otherwise silently drop or collapse.
fn parse_attr_tokens(tokens: TokenStream, name: Ident) -> syn::Result<RticAttr> {
    check_attr_args(tokens.clone(), &name)?;
    RticAttr::parse_from_tokens(tokens, name)
}

/// [`RticAttr`] for an item attribute, with the same key checks as
/// [`parse_attr_tokens`].
fn parse_item_attr(attr: &Attribute) -> syn::Result<RticAttr> {
    if let Meta::List(list) = &attr.meta {
        let name = attr.path().get_ident().cloned().ok_or_else(|| {
            syn::Error::new(attr.span(), "expected a single-segment attribute name")
        })?;
        check_attr_args(list.tokens.clone(), &name)?;
    }
    RticAttr::from_meta(&attr.meta)
}

/// Rejects argument keys that are not single identifiers and keys that occur
/// more than once.
///
/// [`RticAttr`] stores arguments in a map: multi-segment keys are skipped and
/// repeated keys overwrite each other, which would turn malformed syntax into
/// silently accepted declarations.
///
/// TODO(extract): a candidate for `rticx-core`'s `RticAttr` once a second pass
/// needs the same checks.
fn check_attr_args(tokens: TokenStream, name: &Ident) -> syn::Result<()> {
    let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(tokens)?;
    let mut seen = BTreeSet::new();
    for meta in metas {
        let Some(ident) = meta.path().get_ident() else {
            return Err(syn::Error::new(
                meta.path().span(),
                format!("`{name}` arguments must use single-segment keys"),
            ));
        };
        if !seen.insert(ident.to_string()) {
            return Err(syn::Error::new(
                meta.span(),
                format!("duplicate argument `{ident}` in `{name}`"),
            ));
        }
    }
    Ok(())
}

/// Reads `key` as an integer literal without removing it from the attribute,
/// falling back to `default` when absent.
///
/// TODO(extract): duplicates `rticx-sw-pass`'s internal `parse_attr_int`.
fn parse_attr_int<T>(attr: &RticAttr, key: &str, default: T) -> syn::Result<T>
where
    T: FromStr,
    <T as FromStr>::Err: Display,
{
    let Some(expr) = attr.get_expr(key) else {
        return Ok(default);
    };
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Int(int), ..
        }) => int.base10_parse().map_err(|error| {
            syn::Error::new(
                int.span(),
                format!("`{key}` must be an integer literal: {error}"),
            )
        }),
        other => Err(syn::Error::new(
            other.span(),
            format!("`{key}` must be an integer literal"),
        )),
    }
}

/// Span of an integer literal argument, for error reporting.
fn int_span(attr: &RticAttr, key: &str) -> Option<Span> {
    match attr.get_expr(key) {
        Some(Expr::Lit(ExprLit {
            lit: Lit::Int(int), ..
        })) => Some(int.span()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rticx_xbin_proto::ReceiverDecl;

    fn receiver(name: &str, spawn_by: u32, priority: u16) -> ReceiverDecl {
        ReceiverDecl {
            name: name.to_string(),
            priority,
            capacity: 1,
            spawn_by,
            input_type: "Msg".to_string(),
        }
    }

    fn app_extensions(core_id: u32, dispatchers: Vec<Path>) -> AppExtensions {
        AppExtensions {
            core_id,
            pac: syn::parse_quote!(pac),
            dispatchers,
            cross_dispatchers: Vec::new(),
        }
    }

    fn module_decls(receivers: Vec<ReceiverDecl>) -> ModuleDecls {
        ModuleDecls {
            receivers,
            ..Default::default()
        }
    }

    #[test]
    fn parse_dispatchers_accepts_a_flat_list() {
        let flat: Expr = syn::parse_quote!([IRQ0, IRQ1]);
        let parsed = parse_dispatchers(&flat).expect("flat list");
        assert_eq!(parsed.len(), 2);

        // An empty array declares no dispatcher.
        let empty: Expr = syn::parse_quote!([]);
        assert_eq!(parse_dispatchers(&empty).expect("empty"), Vec::new());
    }

    #[test]
    fn parse_dispatchers_rejects_the_nested_and_malformed_shapes() {
        let nested: Expr = syn::parse_quote!([[IRQ0], [IRQ1]]);
        let error = parse_dispatchers(&nested)
            .expect_err("the per-core nested shape was removed")
            .to_string();
        assert!(
            error.contains("must be a flat list of interrupt paths"),
            "{error}"
        );
        assert!(
            error.contains("one-array-per-core shape was removed"),
            "{error}"
        );

        let mixed: Expr = syn::parse_quote!([[IRQ0], 3]);
        let error = parse_dispatchers(&mixed)
            .expect_err("mixed shapes are rejected")
            .to_string();
        assert!(error.contains("must be interrupt paths"), "{error}");

        let not_array: Expr = syn::parse_quote!(IRQ0);
        let error = parse_dispatchers(&not_array)
            .expect_err("an array is required")
            .to_string();
        assert!(
            error.contains("must be an array of interrupt paths"),
            "{error}"
        );

        let bad_element: Expr = syn::parse_quote!([IRQ0, 3]);
        let error = parse_dispatchers(&bad_element)
            .expect_err("entries must be paths")
            .to_string();
        assert!(error.contains("must be interrupt paths"), "{error}");
    }

    /// The `dispatchers` expression of an attribute, rendered as canonical
    /// (whitespace-free) tokens.
    fn rendered_dispatchers(attr: &RticAttr) -> String {
        attr.get_expr(DISPATCHERS_ARG)
            .expect("the key is present")
            .to_token_stream()
            .to_string()
            .replace(' ', "")
    }

    #[test]
    fn steal_dispatchers_takes_the_prefix_and_rewrites_the_remainder() {
        let decls = module_decls(vec![receiver("Alpha", 7, 3)]);
        let mut extensions = app_extensions(
            1,
            vec![syn::parse_quote!(IRQ_XBIN), syn::parse_quote!(IRQ_SW)],
        );
        let mut attr = parse_attr_tokens(
            quote!(dispatchers = [IRQ_XBIN, IRQ_SW]),
            format_ident!("app"),
        )
        .expect("parses");

        steal_dispatchers(&mut extensions, &decls, &mut attr).expect("one entry per line");

        assert_eq!(
            extensions.cross_dispatchers,
            vec![syn::parse_quote!(IRQ_XBIN)]
        );
        assert_eq!(rendered_dispatchers(&attr), "[IRQ_SW]");
    }

    #[test]
    fn steal_dispatchers_rejects_a_short_list() {
        // Declarations arrive sorted by name; lines are distinct
        // `(source, priority)` pairs sorted ascending: (7, 3) before (9, 1).
        let decls = module_decls(vec![receiver("Alpha", 9, 1), receiver("Zeta", 7, 3)]);
        let mut extensions = app_extensions(1, vec![syn::parse_quote!(IRQ_A)]);
        let mut attr =
            parse_attr_tokens(quote!(dispatchers = [IRQ_A]), format_ident!("app")).expect("parses");

        let error = steal_dispatchers(&mut extensions, &decls, &mut attr)
            .expect_err("the second line needs an entry")
            .to_string();
        assert!(
            error.contains("has the cross line `(source 9, priority 1)`"),
            "{error}"
        );
        assert!(
            error.contains("declare one interrupt per cross line at the front of `dispatchers`"),
            "{error}"
        );

        let mut extensions = app_extensions(1, vec![]);
        let mut attr =
            parse_attr_tokens(quote!(dispatchers = []), format_ident!("app")).expect("parses");
        let error = steal_dispatchers(&mut extensions, &decls, &mut attr)
            .expect_err("the first line is missing")
            .to_string();
        assert!(
            error.contains("has the cross line `(source 7, priority 3)`"),
            "{error}"
        );
    }

    #[test]
    fn steal_dispatchers_rejects_duplicates_in_the_prefix() {
        let decls = module_decls(vec![receiver("Alpha", 9, 1), receiver("Zeta", 7, 3)]);
        let mut extensions =
            app_extensions(1, vec![syn::parse_quote!(IRQ_A), syn::parse_quote!(IRQ_A)]);
        let mut attr =
            parse_attr_tokens(quote!(dispatchers = [IRQ_A, IRQ_A]), format_ident!("app"))
                .expect("parses");

        let error = steal_dispatchers(&mut extensions, &decls, &mut attr)
            .expect_err("two lines cannot share one interrupt")
            .to_string();
        assert!(error.contains("lists `IRQ_A` more than once"), "{error}");
        assert!(
            error.contains("each cross line needs its own interrupt line"),
            "{error}"
        );
    }

    #[test]
    fn steal_dispatchers_keeps_surplus_entries_as_local_dispatchers() {
        let decls = module_decls(vec![receiver("Alpha", 7, 3)]);
        let mut extensions = app_extensions(
            1,
            vec![
                syn::parse_quote!(IRQ_A),
                syn::parse_quote!(IRQ_B),
                syn::parse_quote!(IRQ_C),
            ],
        );
        let mut attr = parse_attr_tokens(
            quote!(dispatchers = [IRQ_A, IRQ_B, IRQ_C]),
            format_ident!("app"),
        )
        .expect("parses");

        steal_dispatchers(&mut extensions, &decls, &mut attr)
            .expect("surplus entries are local dispatchers");

        assert_eq!(extensions.cross_dispatchers, vec![syn::parse_quote!(IRQ_A)]);
        assert_eq!(rendered_dispatchers(&attr), "[IRQ_B,IRQ_C]");
    }

    #[test]
    fn steal_dispatchers_shares_one_entry_per_line() {
        // Receivers of the same source and priority share one line and one
        // pool entry.
        let decls = module_decls(vec![receiver("Alpha", 7, 3), receiver("Zeta", 7, 3)]);
        let mut extensions = app_extensions(1, vec![syn::parse_quote!(IRQ_A)]);
        let mut attr =
            parse_attr_tokens(quote!(dispatchers = [IRQ_A]), format_ident!("app")).expect("parses");

        steal_dispatchers(&mut extensions, &decls, &mut attr)
            .expect("same source and priority share a line");

        assert_eq!(extensions.cross_dispatchers, vec![syn::parse_quote!(IRQ_A)]);
        assert_eq!(rendered_dispatchers(&attr), "[]");
    }

    #[test]
    fn steal_dispatchers_leaves_an_absent_key_absent() {
        let decls = module_decls(vec![]);
        let mut extensions = app_extensions(1, vec![]);
        let mut attr =
            parse_attr_tokens(quote!(device = mypac), format_ident!("app")).expect("parses");

        steal_dispatchers(&mut extensions, &decls, &mut attr).expect("no cross lines");

        assert!(
            attr.get_expr(DISPATCHERS_ARG).is_none(),
            "an absent key stays absent"
        );
    }
}
