//! Parsing of the `#[app]` additions and the cross-binary task declarations.
//!
//! This module owns the syntax that the multi-binary extension adds on top of
//! the core RTIC syntax (see `multibinary-multicore-plan.md` §6.4 and §6.5):
//!
//! - `#[app(core_ids = [..], external_cores = [..])]`: `core_ids` is read
//!   through the core parser (`rticx_core` owns the key since M5) and left in
//!   the arguments for the core pass; only `external_cores` is consumed here,
//!   so the core pass never sees it (it would warn about an unknown
//!   argument);
//! - `#[cross_bin_task(..)]` receiver structs, whose input type is declared by
//!   `impl CrossBinTask for <Name> { type Input = …; }`;
//! - `#[cross_bin_spawn(..)]` sender stubs, optionally mirrored by an
//!   `impl CrossBinSpawn for <Name> { type Input = …; }`.
//!
//! Everything is parsed into the plain data types of `rticx-xbin-proto` that
//! the metadata manifest is made of. The attributes themselves are *stripped*
//! from the module: once this pass has consumed them they are meaningless to
//! the core pass (and to other distributions). In codegen mode the receiver
//! structs are additionally rewritten into the core `#[task(..)]` items the
//! generated dispatchers run against (see [`inject_receiver_tasks`]).
//!
//! Confirmed syntax (M1-T3):
//!
//! - receiver `core` is a **local** core index (`0..cores`), like RTIC's task
//!   `core`; sender `core` is the **global** id of the target core and
//!   receiver `spawned_by` lists **global** source core ids;
//! - defaults: `priority = 1`, `capacity = 1`, receiver `core = 0`;
//! - unknown, duplicate and multi-segment keys as well as malformed values
//!   are hard errors; `external_cores` is validated against the `core_ids`
//!   mapping the core parser resolves (identity when not declared);
//! - stripping happens in both modes and for every distribution that binds
//!   the pass. Without the pass, the core pass only *warns* about
//!   `external_cores` as an unknown `#[app]` argument (`core_ids` is native
//!   to `rticx-core` since M5 and is understood with or without the pass).
//!
//! TODO(extract): `parse_attr_int` and `item_attrs` duplicate helpers in
//! `rticx-sw-pass`'s internal `common::parse` module. They may be promoted
//! into `rticx-core` once a second pass needs them (`take_u32_array` already
//! lives on [`RticAttr`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::str::FromStr;

use proc_macro2::{Ident, Span, TokenStream};
use quote::{ToTokens, format_ident};
use rticx_core::parse_utils::RticAttr;
use rticx_core::parser::ast::AppArgs;
use rticx_xbin_proto::{ReceiverDecl, SenderDecl};
use syn::{
    Attribute, Expr, ExprLit, ImplItem, Item, ItemImpl, ItemMod, Lit, LitInt, Meta, PathArguments,
    Token, Type, parse::Parser, punctuated::Punctuated, spanned::Spanned,
};

/// `#[app(core_ids = [..])]`: local core index -> global core id.
pub(crate) const CORE_IDS_ARG: &str = "core_ids";
/// `#[app(external_cores = [..])]`: global ids of cores in other binaries.
pub(crate) const EXTERNAL_CORES_ARG: &str = "external_cores";
/// Receiver attribute: the task executes in this binary.
pub(crate) const CROSS_BIN_TASK_ATTR: &str = "cross_bin_task";
/// Sender attribute: a stub that spawns a task in another binary.
pub(crate) const CROSS_BIN_SPAWN_ATTR: &str = "cross_bin_spawn";
/// Receiver trait name (last path segment) carrying `type Input`.
pub(crate) const CROSS_BIN_TASK_TRAIT: &str = "CrossBinTask";
/// Sender trait name (last path segment) optionally carrying `type Input`.
pub(crate) const CROSS_BIN_SPAWN_TRAIT: &str = "CrossBinSpawn";

/// Default task priority, matching the core pass (`1` is the lowest active
/// priority).
const DEFAULT_PRIORITY: u16 = 1;
/// Default FIFO capacity (one pending spawn), matching `rticx-sw-pass`.
const DEFAULT_CAPACITY: usize = 1;

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
    /// `#[cross_bin_task]` receivers, sorted by name.
    pub receivers: Vec<ReceiverDecl>,
    /// `#[cross_bin_spawn]` senders, sorted by name.
    pub senders: Vec<SenderDecl>,
}

/// Which cross-binary attribute an item carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeclKind {
    Receiver,
    Sender,
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

/// Parses (and strips) every cross-binary declaration from the `#[app]`
/// module.
///
/// `cores` is the application's local core count, used to check receiver core
/// indices (which are local, unlike the global sender target and
/// `spawned_by` ids).
pub(crate) fn parse_module(app_mod: &mut ItemMod, cores: u32) -> syn::Result<ModuleDecls> {
    let Some((_, items)) = app_mod.content.as_mut() else {
        return Ok(ModuleDecls::default());
    };

    let inputs = collect_input_types(items)?;
    let mut decls = ModuleDecls::default();

    for item in items.iter_mut() {
        if !matches!(item, Item::Struct(_)) {
            if let Some((kind, attr)) = find_cross_attr(item) {
                return Err(syn::Error::new(
                    attr.span(),
                    format!(
                        "`{}` must be applied to a struct declaration (task name)",
                        attr_name(kind)
                    ),
                ));
            }
            continue;
        }

        let Item::Struct(item_struct) = item else {
            unreachable!("checked above");
        };
        let Some((kind, attr)) = take_cross_attr(&mut item_struct.attrs)? else {
            continue;
        };
        let name = item_struct.ident.clone();
        match kind {
            DeclKind::Receiver => {
                let input = inputs.receiver.get(&name.to_string()).cloned();
                decls
                    .receivers
                    .push(parse_receiver(name, &attr, input, cores)?);
            }
            DeclKind::Sender => {
                let input = inputs.sender.get(&name.to_string()).cloned();
                decls.senders.push(parse_sender(name, &attr, input)?);
            }
        }
    }

    decls.receivers.sort_by(|a, b| a.name.cmp(&b.name));
    decls.senders.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(decls)
}

/// Rewrites every receiver struct into the `#[task(..)]` item the core pass
/// consumes (M3-T2).
///
/// Only codegen mode calls this: the injected attribute is what makes the
/// core pass generate the task static and enforce the user's `impl
/// CrossBinTask` (`task_trait = CrossBinTask`), so the generated dispatcher
/// can call `exec(input)` on the initialized instance. The attribute carries
/// only keys the core pass understands, so no core changes are required.
pub(crate) fn inject_receiver_tasks(app_mod: &mut ItemMod, receivers: &[ReceiverDecl]) {
    let Some((_, items)) = app_mod.content.as_mut() else {
        return;
    };
    let task_trait = format_ident!("{CROSS_BIN_TASK_TRAIT}");

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

        let priority = LitInt::new(&receiver.priority.to_string(), item_struct.ident.span());
        let core = LitInt::new(&receiver.core.to_string(), item_struct.ident.span());
        item_struct.attrs.push(syn::parse_quote! {
            #[task(priority = #priority, core = #core, task_trait = #task_trait, init = generated)]
        });
    }
}

/// `type Input = …;` declarations found in `impl CrossBinTask`/`CrossBinSpawn`
/// blocks, keyed by the implementing type name.
#[derive(Debug, Default)]
struct InputTypes {
    receiver: BTreeMap<String, String>,
    sender: BTreeMap<String, String>,
}

fn collect_input_types(items: &[Item]) -> syn::Result<InputTypes> {
    let mut inputs = InputTypes::default();

    for item in items {
        let Item::Impl(impl_item) = item else {
            continue;
        };
        let Some(kind) = impl_kind(impl_item) else {
            continue;
        };
        let Some(self_ty) = impl_self_ident(impl_item) else {
            continue;
        };
        let Some(input) = find_input_type(impl_item) else {
            continue;
        };
        let name = self_ty.to_string();
        let slot = match kind {
            DeclKind::Receiver => &mut inputs.receiver,
            DeclKind::Sender => &mut inputs.sender,
        };
        if slot.contains_key(&name) {
            return Err(syn::Error::new(
                self_ty.span(),
                format!(
                    "duplicate `impl {}` for `{name}` with `type Input`",
                    trait_name(kind)
                ),
            ));
        }
        slot.insert(name, type_path_string(&input));
    }

    Ok(inputs)
}

fn impl_kind(impl_item: &ItemImpl) -> Option<DeclKind> {
    let (_, path, _) = impl_item.trait_.as_ref()?;
    let last = path.segments.last()?;
    if last.ident == CROSS_BIN_TASK_TRAIT {
        Some(DeclKind::Receiver)
    } else if last.ident == CROSS_BIN_SPAWN_TRAIT {
        Some(DeclKind::Sender)
    } else {
        None
    }
}

fn impl_self_ident(impl_item: &ItemImpl) -> Option<&Ident> {
    match impl_item.self_ty.as_ref() {
        Type::Path(path) if path.qself.is_none() => {
            path.path.segments.last().map(|segment| &segment.ident)
        }
        _ => None,
    }
}

fn find_input_type(impl_item: &ItemImpl) -> Option<Type> {
    impl_item.items.iter().find_map(|item| match item {
        ImplItem::Type(assoc) if assoc.ident == "Input" => Some(assoc.ty.clone()),
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

fn parse_receiver(
    name: Ident,
    attr: &Attribute,
    input: Option<String>,
    cores: u32,
) -> syn::Result<ReceiverDecl> {
    let mut args = parse_item_attr(attr)?;
    args.ensure_supported(&["priority", "capacity", "spawned_by", "core"])?;

    let priority = parse_attr_int(&args, "priority", DEFAULT_PRIORITY)?;
    let capacity = parse_attr_int(&args, "capacity", DEFAULT_CAPACITY)?;
    if capacity == 0 {
        return Err(syn::Error::new(
            int_span(&args, "capacity").unwrap_or_else(|| name.span()),
            "The `capacity` argument must be at least 1.",
        ));
    }
    let core = parse_attr_int(&args, "core", 0u32)?;
    if core >= cores {
        return Err(syn::Error::new(
            int_span(&args, "core").unwrap_or_else(|| name.span()),
            format!(
                "receiver `{name}` declares local `core = {core}`, but the application has only \
                 {cores} local core(s); `core` is a local index (map it to a global id through \
                 `{CORE_IDS_ARG}`)"
            ),
        ));
    }
    let spawned_by = args.take_u32_array("spawned_by")?;

    let input_type = input.ok_or_else(|| {
        syn::Error::new(
            name.span(),
            format!(
                "receiver `{name}` must have an `impl {CROSS_BIN_TASK_TRAIT} for {name}` block \
                 declaring `type Input = …;`"
            ),
        )
    })?;

    Ok(ReceiverDecl {
        name: name.to_string(),
        priority,
        capacity,
        core,
        spawned_by,
        input_type,
    })
}

fn parse_sender(name: Ident, attr: &Attribute, input: Option<String>) -> syn::Result<SenderDecl> {
    let mut args = parse_item_attr(attr)?;
    args.ensure_supported(&["core", "priority", "capacity"])?;

    let core = args.take_u32("core")?.ok_or_else(|| {
        syn::Error::new(
            name.span(),
            "sender stubs must declare the global target core id, e.g. `#[cross_bin_spawn(core = 1, …)]`",
        )
    })?;
    let priority = parse_attr_int(&args, "priority", DEFAULT_PRIORITY)?;
    let capacity = parse_attr_int(&args, "capacity", DEFAULT_CAPACITY)?;
    if capacity == 0 {
        return Err(syn::Error::new(
            int_span(&args, "capacity").unwrap_or_else(|| name.span()),
            "The `capacity` argument must be at least 1.",
        ));
    }

    Ok(SenderDecl {
        name: name.to_string(),
        core,
        priority,
        capacity,
        input_type: input,
    })
}

/// Returns the cross-binary attribute on `item`, if any, without removing it.
fn find_cross_attr(item: &Item) -> Option<(DeclKind, &Attribute)> {
    item_attrs(item)?
        .iter()
        .find_map(|attr| attr_kind(attr).map(|kind| (kind, attr)))
}

fn attr_kind(attr: &Attribute) -> Option<DeclKind> {
    let last = attr.path().segments.last()?;
    if last.ident == CROSS_BIN_TASK_ATTR {
        Some(DeclKind::Receiver)
    } else if last.ident == CROSS_BIN_SPAWN_ATTR {
        Some(DeclKind::Sender)
    } else {
        None
    }
}

fn attr_name(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Receiver => CROSS_BIN_TASK_ATTR,
        DeclKind::Sender => CROSS_BIN_SPAWN_ATTR,
    }
}

fn trait_name(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Receiver => CROSS_BIN_TASK_TRAIT,
        DeclKind::Sender => CROSS_BIN_SPAWN_TRAIT,
    }
}

/// Removes and returns the cross-binary attribute of an item, erroring when
/// both kinds are present.
fn take_cross_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<(DeclKind, Attribute)>> {
    let mut found: Option<(DeclKind, Attribute)> = None;
    let mut index = 0;
    while index < attrs.len() {
        let Some(kind) = attr_kind(&attrs[index]) else {
            index += 1;
            continue;
        };
        if found.is_some() {
            return Err(syn::Error::new(
                attrs[index].span(),
                "an item may carry at most one of `#[cross_bin_task]` and `#[cross_bin_spawn]`",
            ));
        }
        let attr = attrs.remove(index);
        found = Some((kind, attr));
    }
    Ok(found)
}

/// Attributes attached to `item`, if the item kind carries attributes.
///
/// TODO(extract): duplicates `rticx-sw-pass`'s internal helper of the same
/// name.
fn item_attrs(item: &Item) -> Option<&[Attribute]> {
    Some(match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => return None,
    })
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
