//! Parsing of the `#[app]` additions and the native cross-binary task
//! declarations (M5.5).
//!
//! This module owns the syntax that the multi-binary extension adds on top of
//! the core RTIC syntax (see `multibinary-multicore-plan.md` §6.4 and §6.5):
//!
//! - `#[app(core_ids = [..], external_cores = [..])]`: `core_ids` is read
//!   through the core parser (`rticx_core` owns the key since M5) and left in
//!   the arguments for the core pass; only `external_cores` is consumed here,
//!   so the core pass never sees it (it would warn about an unknown
//!   argument);
//! - native `#[sw_task(..)]` structs with `impl RticSwTask { type SpawnInput
//!   = …; }` blocks. In an application that declares `external_cores`, the
//!   task's `spawn_by` is resolved in the **global** namespace:
//!   - `spawn_by ∈ core_ids` → an in-app cross-core task; the value is
//!     rewritten to its local index and the task is left for `rticx-sw-pass`;
//!   - `spawn_by ∈ external_cores` → a cross-binary receiver; it is recorded
//!     in the manifest and `spawn_by` is stripped so the software pass never
//!     sees an external id (in codegen mode the whole attribute is later
//!     replaced by the core `#[task(..)]` shape);
//!   - anything else → a hard error naming the unknown core.
//!
//! Task `core` always stays a **local** index (`0..cores`), like RTIC's task
//! `core`. The xbin-specific task attributes and traits of the M1–M4
//! prototype were deleted outright in M5.5: there is no compatibility shim,
//! no warning and no migration error (the extension has no released users).
//!
//! Everything is parsed into the plain data types of `rticx-xbin-proto` that
//! the metadata manifest is made of. In codegen mode the receiver structs are
//! additionally rewritten into the core `#[task(..)]` items the generated
//! dispatchers run against (see [`inject_receiver_tasks`]); in metadata mode
//! their parsed attributes are stripped from the emitted module.
//!
//! Confirmed syntax (M5.5):
//!
//! - receiver `core` is a **local** core index (`0..cores`);
//! - cross receivers declare exactly one global producer core in `spawn_by`
//!   (an array is rejected: multi-producer tasks land in M6-T1);
//! - defaults: `priority = 1` (cross receivers require `priority >= 1`),
//!   `capacity = 1` (minimum 1), receiver `core = 0`;
//! - unknown, duplicate and multi-segment keys as well as malformed values
//!   are hard errors; `external_cores` is validated against the `core_ids`
//!   mapping the core parser resolves (identity when not declared);
//! - stripping happens in both modes and for every distribution that binds
//!   the pass. Without the pass, the core pass only *warns* about
//!   `external_cores` as an unknown `#[app]` argument (`core_ids` is native
//!   to `rticx-core` since M5 and is understood with or without the pass).
//!
//! The producer application declares nothing: its `Task::cross_spawn` stubs
//! are generated in codegen mode from the synced system view (M5.5).
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
    Attribute, Expr, ExprLit, ImplItem, Item, ItemImpl, ItemMod, Lit, LitInt, Meta, PathArguments,
    Token, Type, parse::Parser, parse_quote, punctuated::Punctuated, spanned::Spanned,
};

/// `#[app(core_ids = [..])]`: local core index -> global core id.
pub(crate) const CORE_IDS_ARG: &str = "core_ids";
/// `#[app(external_cores = [..])]`: global ids of cores in other binaries.
pub(crate) const EXTERNAL_CORES_ARG: &str = "external_cores";
/// Native software-task attribute.
pub(crate) const SW_TASK_ATTR: &str = "sw_task";
/// `#[sw_task(spawn_by = G)]`: the (global, in apps declaring
/// `external_cores`) producer core id.
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
/// Default receiver local core index.
const DEFAULT_CORE: u32 = 0;

/// The `#[app]` arguments owned by this pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppExtensions {
    /// Local core count (`cores`, default 1).
    pub cores: u32,
    /// Resolved `core_ids` mapping (local index -> global id), the identity
    /// `0..cores` when the application does not declare the key.
    ///
    /// Parsed and validated by `rticx-core` (M5), which owns `core_ids`; this
    /// pass only reads the mapping (and leaves the key in the arguments).
    pub core_ids: Vec<u32>,
    /// `external_cores`, empty when not declared.
    pub external_cores: Vec<u32>,
}

/// The cross-binary declarations found inside the `#[app]` module.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ModuleDecls {
    /// Cross-binary receivers, sorted by name.
    pub receivers: Vec<ReceiverDecl>,
}

/// Parses the `#[app]` arguments owned by this pass and returns them together
/// with the argument token stream stripped of the consumed keys.
///
/// `cores`, `device` and `core_ids` are left untouched for the core pass;
/// `core_ids` is parsed through [`AppArgs`] (the core parser owns and
/// validates it since M5) so the pass reads the resolved mapping, and only
/// `external_cores` is consumed.
pub(crate) fn parse_app_args(args: TokenStream) -> syn::Result<(AppExtensions, TokenStream)> {
    let args_span = args.span();

    // `rticx-core` owns `core_ids` (M5): parsing through `AppArgs` resolves
    // the identity default and validates the length and uniqueness with the
    // core pass's own errors. The arguments themselves are not consumed here.
    let app_args = AppArgs::parse(args.clone())?;
    let cores = app_args.cores;
    if cores == 0 {
        return Err(syn::Error::new(
            args_span,
            "The `cores` argument must be at least 1.",
        ));
    }
    let core_ids = app_args.core_ids;

    let mut attr = parse_attr_tokens(args, format_ident!("app"))?;

    let external_cores_span = attr.get_expr(EXTERNAL_CORES_ARG).map(Spanned::span);
    let external_cores = attr.take_u32_array(EXTERNAL_CORES_ARG)?.unwrap_or_default();
    if let Some(duplicate) = first_duplicate(&external_cores) {
        return Err(syn::Error::new(
            external_cores_span.unwrap_or(args_span),
            format!("`{EXTERNAL_CORES_ARG}` lists global core id {duplicate} more than once"),
        ));
    }
    if let Some(owned) = external_cores.iter().find(|id| core_ids.contains(id)) {
        return Err(syn::Error::new(
            external_cores_span.unwrap_or(args_span),
            format!(
                "`{EXTERNAL_CORES_ARG}` lists global core id {owned}, which this application \
                 owns via `{CORE_IDS_ARG}`"
            ),
        ));
    }

    let extensions = AppExtensions {
        cores,
        core_ids,
        external_cores,
    };
    Ok((extensions, attr.args_tokens()))
}

/// Parses (and rewrites) the native cross-binary receivers of the `#[app]`
/// module.
///
/// The function performs the `spawn_by` namespace resolution described in the
/// module documentation and returns every cross-binary receiver sorted by
/// name. In-app global `spawn_by` values are rewritten to local indices and
/// cross receivers lose their `spawn_by` key, so `rticx-sw-pass` never sees a
/// global id it would compare against a local index.
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
        let Some(index) = find_sw_task_attr(&item_struct.attrs) else {
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

        // `spawn_by` absent: a plain software task, untouched.
        let Some(span) = attr.get_expr(SPAWN_BY_ARG).map(Spanned::span) else {
            continue;
        };
        // Without `external_cores` the native local-index reading applies and
        // the software pass validates the value (`spawn_by < cores`).
        if extensions.external_cores.is_empty() {
            continue;
        }

        let producer = take_spawn_by(&mut attr, span)?;
        if let Some(local) = extensions.core_ids.iter().position(|&id| id == producer) {
            // In-app cross-core task: map the global id back to the local
            // index the software pass compares against.
            let local = u32::try_from(local).expect("core count fits in u32");
            attr.elements
                .insert(SPAWN_BY_ARG.to_string(), syn::parse_quote!(#local));
        } else if extensions.external_cores.contains(&producer) {
            // Cross-binary receiver: recorded here, invisible to sw-pass.
            attr.elements.remove(SPAWN_BY_ARG);
            decls.receivers.push(parse_receiver(
                name, &attr, &inputs, extensions, producer, span,
            )?);
        } else {
            return Err(unknown_spawn_by(&name, producer, span, extensions));
        }

        // Re-render the rewritten attribute in place for the software pass.
        item_struct.attrs[index] = render_attribute(&attr);
    }

    decls.receivers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(decls)
}

/// Reads the singular `spawn_by` producer core id.
///
/// Arrays are rejected outright: v1 has one SPSC FIFO and one priority line
/// per task, so a task has exactly one producer core (M6-T1 lifts this).
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
/// attribute keys against the application's local cores.
fn parse_receiver(
    name: Ident,
    attr: &RticAttr,
    inputs: &BTreeMap<String, String>,
    extensions: &AppExtensions,
    spawn_by: u32,
    spawn_by_span: Span,
) -> syn::Result<ReceiverDecl> {
    check_init_generated(attr, &name)?;
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
    let core = parse_attr_int(attr, "core", DEFAULT_CORE)?;
    if core >= extensions.cores {
        return Err(syn::Error::new(
            int_span(attr, "core").unwrap_or_else(|| name.span()),
            format!(
                "receiver `{name}` declares local `core = {core}`, but the application has only \
                 {} local core(s); `core` is a local index (map it to a global id through \
                 `{}`)",
                extensions.cores, CORE_IDS_ARG
            ),
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
        core,
        spawn_by,
        input_type,
    })
}

/// Rewrites every cross-binary receiver struct into the `#[task(..)]` item
/// the core pass consumes (M5.5).
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
        let Some(index) = find_sw_task_attr(&item_struct.attrs) else {
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
        let core = LitInt::new(&receiver.core.to_string(), item_struct.ident.span());
        item_struct.attrs[index] = parse_quote! {
            #[task(priority = #priority, core = #core, task_trait = RticSwTask,
                   init = generated #shared)]
        };
    }
}

/// Removes the `#[sw_task(..)]` attribute of every cross-binary receiver.
///
/// Without it the software pass of a metadata `cargo check` would treat the
/// cross receiver as a local software task and miss the external `spawn_by`;
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
        if let Some(index) = find_sw_task_attr(&item_struct.attrs) {
            item_struct.attrs.remove(index);
        }
    }
}

/// Returns the index of the first `#[sw_task]` attribute in `attrs`, if any.
fn find_sw_task_attr(attrs: &[Attribute]) -> Option<usize> {
    attrs
        .iter()
        .position(|attr| attr.path().is_ident(SW_TASK_ATTR))
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

/// Error for a `spawn_by` that names neither a local nor an external core.
fn unknown_spawn_by(
    name: &Ident,
    producer: u32,
    span: Span,
    extensions: &AppExtensions,
) -> syn::Error {
    syn::Error::new(
        span,
        format!(
            "receiver `{name}` declares `{SPAWN_BY_ARG} = {producer}`, which is not a known \
             core: it is neither one of this application's `{CORE_IDS_ARG}` ({:?}) nor listed \
             in `{EXTERNAL_CORES_ARG}` ({:?})",
            extensions.core_ids, extensions.external_cores
        ),
    )
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

/// Returns the first value occurring more than once in `values`.
fn first_duplicate(values: &[u32]) -> Option<u32> {
    let mut seen = std::collections::BTreeSet::new();
    values.iter().copied().find(|value| !seen.insert(*value))
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
