//! Shared parsing infrastructure for the software-task compilation passes.
//!
//! Consumed by both `rticx-sw-pass` (the base crate) and `rticx-async-pass`
//! (the extension).  These items are pass-internal plumbing, **not** a stable
//! public API.

use proc_macro2::{Span, TokenStream};
use quote::format_ident;
use rticx_core::{errors::ParseError, parse_utils::RticAttr};
use std::collections::HashMap;
use syn::{Attribute, Expr, Ident, Item, ItemImpl, Path, Type, spanned::Spanned};

#[derive(Clone)]
pub struct AppParameters {
    pub dispatchers: HashMap<u32, Vec<Path>>,
    /// Span of the `dispatchers = [...]` argument, for error reporting.
    pub dispatchers_span: Option<Span>,
    pub pacs: Vec<Path>,
    pub cores: u32,
}

impl AppParameters {
    pub fn parse(args: &TokenStream) -> syn::Result<Self> {
        let args_span = args.span();
        let mut args = RticAttr::parse_from_tokens(args.clone(), format_ident!("app"))?;

        // parse the number of cores
        let cores = args.take_u32("cores")?.unwrap_or(1);
        if cores == 0 {
            return Err(syn::Error::new(
                args_span,
                "The `cores` argument must be at least 1.",
            ));
        }

        // parse the path(s) to PAC(s)
        let device = args
            .elements
            .remove("device")
            .ok_or(ParseError::DeviceArg.to_syn(args_span))?;

        let pacs = match device {
            Expr::Array(array_exp) => {
                if array_exp.elems.len() != cores as usize {
                    return Err(ParseError::DevicesCoresMismatch.to_syn(args_span));
                }

                array_exp
                    .elems
                    .into_iter()
                    .map(|exp| match exp {
                        Expr::Path(p) => Ok(p.path),
                        other => Err(ParseError::DeviceNotPath.to_syn(other.span())),
                    })
                    .collect::<syn::Result<Vec<_>>>()?
            }
            Expr::Path(path_to_pac) => vec![path_to_pac.path; cores as usize],
            other => return Err(ParseError::DeviceNotPath.to_syn(other.span())),
        };

        // dispatchers
        let mut dispatchers: HashMap<u32, Vec<Path>> = HashMap::with_capacity(cores as usize);
        let dispatchers_span = args.elements.get("dispatchers").map(Spanned::span);
        if let Some(dispatcher_expr) = args.elements.get("dispatchers") {
            let Expr::Array(arr) = dispatcher_expr else {
                return Err(syn::Error::new(
                    dispatcher_expr.span(),
                    "The `dispatchers` argument must be an array of interrupt paths, e.g. `dispatchers = [IRQ0, IRQ1]` or `dispatchers = [[IRQ0], [IRQ1]]` for per-core lists.",
                ));
            };
            for (i, element) in arr.elems.iter().enumerate() {
                match element {
                    Expr::Path(path) => dispatchers.entry(0).or_default().push(path.path.clone()),
                    Expr::Array(arr) => {
                        let irqs = arr
                            .elems
                            .iter()
                            .map(|element| match element {
                                Expr::Path(path) => Ok(path.path.clone()),
                                other => Err(syn::Error::new(
                                    other.span(),
                                    "The elements of the `dispatchers` argument must be interrupt paths.",
                                )),
                            })
                            .collect::<syn::Result<Vec<Path>>>()?;
                        dispatchers.insert(i as u32, irqs);
                    }
                    other => {
                        return Err(syn::Error::new(
                            other.span(),
                            "The elements of the `dispatchers` argument must be interrupt paths or arrays of interrupt paths.",
                        ));
                    }
                }
            }
        }

        if !dispatchers.is_empty() && cores as usize != dispatchers.len() {
            return Err(syn::Error::new(
                dispatchers_span.unwrap_or_else(Span::call_site),
                format!(
                    "The number of cores `{cores}` does not match the number of dispatchers `{}`",
                    dispatchers.len()
                ),
            ));
        }

        Ok(Self {
            dispatchers,
            dispatchers_span,
            pacs,
            cores,
        })
    }
}

/// Renames a parsed task attribute (`sw_task`/`async_task`) into the
/// `#[task(...)]` attribute expected by the core pass.  The `spawn_by` and
/// `capacity` keys are consumed by the pass, so they are removed; `task_trait`
/// is added to point the core pass at the pass-specific task trait.
pub fn into_task_attr(mut attr: RticAttr, trait_ident: Ident) -> RticAttr {
    attr.name = format_ident!("task");
    attr.elements.remove("spawn_by");
    attr.elements.remove("capacity");
    let trait_ty: Expr = syn::parse_quote!(#trait_ident);
    attr.elements.insert("task_trait".into(), trait_ty);
    attr
}

#[derive(Debug, Clone)]
pub struct TaskParams {
    pub priority: u16,
    pub core: u32,
    pub spawn_by: u32,
    /// Number of pending spawns the task's input queue can hold.
    /// Internally the queue is a ring buffer of `capacity + 1` slots.
    pub capacity: usize,
}

impl TaskParams {
    pub fn from_attr(attr: &mut RticAttr) -> syn::Result<Self> {
        // `priority` and `core` are validated here but must remain in the
        // attribute: the reconstructed `#[task(...)]` attribute carries them
        // over to the core pass.  Only `spawn_by` and `capacity` are consumed
        // (by `into_task_attr`).
        let priority = parse_attr_int(attr, "priority", 0)?;
        let core = parse_attr_int(attr, "core", 0)?;
        // spawn_by defaults to the task's own core, unless the user chooses otherwise
        let spawn_by = parse_attr_int(attr, "spawn_by", core)?;
        let capacity = parse_attr_int(attr, "capacity", 1)?;
        if capacity == 0 {
            return Err(syn::Error::new(
                int_span(attr, "capacity").unwrap_or_else(Span::call_site),
                "The `capacity` argument must be at least 1.",
            ));
        }

        Ok(Self {
            priority,
            core,
            spawn_by,
            capacity,
        })
    }
}

/// Reads `key` as an integer literal without removing it from the attribute,
/// falling back to `default` when absent.  Errors point at the offending
/// token (including overflow errors).
pub fn parse_attr_int<T>(attr: &RticAttr, key: &str, default: T) -> syn::Result<T>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    let Some(expr) = attr.get_expr(key) else {
        return Ok(default);
    };
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(int),
            ..
        }) => int.base10_parse().map_err(|e| {
            syn::Error::new(
                int.span(),
                format!("`{key}` must be an integer literal: {e}"),
            )
        }),
        other => Err(syn::Error::new(
            other.span(),
            format!("`{key}` must be an integer literal"),
        )),
    }
}

/// Span of the `capacity` literal if present, for error reporting.
pub fn int_span(attr: &RticAttr, key: &str) -> Option<Span> {
    match attr.elements.get(key) {
        Some(Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(int),
            ..
        })) => Some(int.span()),
        _ => None,
    }
}

/// Attributes attached to `item`, if the item kind carries attributes.
pub fn item_attrs(item: &Item) -> Option<&[Attribute]> {
    Some(match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
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

/// The implementor of an `impl <trait> for <type>` block whose trait name ends
/// with `trait_ty` (e.g. `RticSwTask`).  The last path segment is matched so
/// that qualified paths (e.g. `crate::RticSwTask`) are recognized too.
pub fn get_task_implementor<'a>(impl_item: &'a ItemImpl, trait_ty: &str) -> Option<&'a Ident> {
    let (_, path, _) = impl_item.trait_.as_ref()?;
    if !path.segments.last()?.ident.to_string().ends_with(trait_ty) {
        return None;
    }
    if let Type::Path(struct_type) = impl_item.self_ty.as_ref() {
        return Some(&struct_type.path.segments.last()?.ident);
    }
    None
}

/// Removes the `impl <trait> for <ident>` block from `impls`, erroring if
/// the task has more than one such implementation.  `trait_ty` names the
/// trait (used only in the error message).
pub fn take_impl_for(
    impls: &mut Vec<(Ident, ItemImpl)>,
    ident: &Ident,
    trait_ty: &str,
) -> syn::Result<Option<ItemImpl>> {
    let mut found = None;
    let mut i = 0;
    while i < impls.len() {
        if &impls[i].0 == ident {
            let (_, impl_) = impls.remove(i);
            if found.replace(impl_).is_some() {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Multiple `{trait_ty}` implementations found for task `{ident}`."),
                ));
            }
        } else {
            i += 1;
        }
    }
    Ok(found)
}
