use crate::parse::ast::{AppParameters, SoftwareTask, TaskParams, into_task_attr};
use proc_macro2::{Ident, Span, TokenStream};
use rticx_core::parse_utils::RticAttr;
use std::collections::HashMap;
use syn::{Attribute, Item, ItemImpl, ItemMod, Type, Visibility, spanned::Spanned};

pub mod ast;

pub const SWT_TRAIT_TY: &str = "RticSwTask";

/// Type to represent a sub application (application on a single core)
#[derive(Clone)]
pub struct SubApp {
    pub core: u32,
    pub dispatchers: Vec<syn::Path>,
    /// Span of the `dispatchers = [...]` app argument, for error reporting.
    pub dispatchers_span: Option<Span>,
    /// Single core/ Core-local software tasks
    pub sw_tasks: Vec<SoftwareTask>,
    /// Multi core/ software tasks to be spawned on this core from other cores
    pub mc_sw_tasks: Vec<SoftwareTask>,
}

/// Type to represent an RTICX application (within software pass context)
/// The application contains one or more sub-applications (one application per-core)
#[derive(Clone)]
pub struct App {
    pub mod_visibility: Visibility,
    pub mod_ident: Ident,
    pub app_params: AppParameters,
    /// a list of sub-applications, one sub-app per core.
    pub sub_apps: Vec<SubApp>,
    pub rest_of_code: Vec<Item>,
}

impl App {
    pub fn parse(args: &TokenStream, mut app_mod: ItemMod) -> syn::Result<Self> {
        let app_params = AppParameters::parse(args)?;
        let app_mod_items = app_mod.content.take().unwrap_or_default().1;
        let mut sw_task_structs = Vec::new();
        let mut sw_task_impls: Vec<(Ident, ItemImpl)> = Vec::new();
        let mut rest_of_code = Vec::with_capacity(app_mod_items.len());

        for item in app_mod_items {
            if !matches!(item, Item::Struct(_))
                && let Some(attrs) = item_attrs(&item)
                && let Some(attr_idx) = find_sw_task_attr(attrs)
            {
                return Err(syn::Error::new(
                    attrs[attr_idx].span(),
                    "`#[sw_task]` can only be applied to structs.",
                ));
            }
            match item {
                Item::Struct(struct_) => {
                    if let Some(attr_idx) = find_sw_task_attr(&struct_.attrs) {
                        sw_task_structs.push((struct_, attr_idx))
                    } else {
                        rest_of_code.push(Item::Struct(struct_))
                    }
                }
                Item::Impl(impl_) => {
                    if let Some(implementor) = get_sw_task_implementor(&impl_) {
                        sw_task_impls.push((implementor.clone(), impl_));
                    } else {
                        rest_of_code.push(Item::Impl(impl_))
                    }
                }
                item => rest_of_code.push(item),
            }
        }

        let cores = app_params.cores;
        let mut sw_tasks = HashMap::with_capacity(cores as usize);
        let mut mc_sw_tasks = HashMap::with_capacity(cores as usize);
        for (mut task_struct, attr_idx) in sw_task_structs {
            // The `impl RticSwTask for <struct>` is optional at this stage: it
            // may also live outside the `#[app]` module.  The core pass
            // generates static checks that the trait is implemented.
            let task_impl = take_impl_for(&mut sw_task_impls, &task_struct.ident)?;

            let mut attrs = RticAttr::parse_from_attr(&task_struct.attrs[attr_idx])?;
            let params = TaskParams::from_attr(&mut attrs)?;
            let task_attr = into_task_attr(attrs);
            // Consume the original `#[sw_task]` attribute: the reconstructed
            // `#[task(...)]` attribute replaces it in the generated code.
            task_struct.attrs.remove(attr_idx);

            if params.core >= cores {
                return Err(syn::Error::new(
                    task_struct.ident.span(),
                    format!(
                        "Task `{}` has `core = {}`, but the application only has {cores} core(s).",
                        task_struct.ident, params.core
                    ),
                ));
            }
            if params.spawn_by >= cores {
                return Err(syn::Error::new(
                    task_struct.ident.span(),
                    format!(
                        "Task `{}` has `spawn_by = {}`, but the application only has {cores} core(s).",
                        task_struct.ident, params.spawn_by
                    ),
                ));
            }

            let task = SoftwareTask {
                params,
                task_attr,
                task_struct,
                task_impl,
            };

            if task.params.core == task.params.spawn_by {
                sw_tasks
                    .entry(task.params.core)
                    .or_insert(Vec::new())
                    .push(task);
            } else {
                mc_sw_tasks
                    .entry(task.params.core)
                    .or_insert(Vec::new())
                    .push(task);
            }
        }

        // `impl RticSwTask for X` blocks whose `X` is not a software task are
        // re-emitted as-is so they are not silently dropped.
        for (_, impl_) in sw_task_impls {
            rest_of_code.push(Item::Impl(impl_));
        }

        let mut sub_apps = Vec::with_capacity(cores as usize);
        for core in 0..cores {
            let dispatchers = app_params
                .dispatchers
                .get(&core)
                .cloned()
                .unwrap_or_default();
            sub_apps.push(SubApp {
                core,
                dispatchers,
                dispatchers_span: app_params.dispatchers_span,
                sw_tasks: sw_tasks.remove(&core).unwrap_or_default(),
                mc_sw_tasks: mc_sw_tasks.remove(&core).unwrap_or_default(),
            })
        }

        Ok(Self {
            mod_ident: app_mod.ident,
            mod_visibility: app_mod.vis,
            app_params,
            sub_apps,
            rest_of_code,
        })
    }
}

/// Returns the index of the first `sw_task` attribute in `attrs`, if any.
fn find_sw_task_attr(attrs: &[Attribute]) -> Option<usize> {
    attrs
        .iter()
        .position(|attr| attr.path().is_ident("sw_task"))
}

/// Attributes attached to `item`, if the item kind carries attributes.
fn item_attrs(item: &Item) -> Option<&[Attribute]> {
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
/// with `RticSwTask`.  The last path segment is matched so that qualified
/// paths (e.g. `crate::RticSwTask`) are recognized too.
fn get_sw_task_implementor(impl_item: &ItemImpl) -> Option<&Ident> {
    let (_, path, _) = impl_item.trait_.as_ref()?;
    if !path
        .segments
        .last()?
        .ident
        .to_string()
        .ends_with(SWT_TRAIT_TY)
    {
        return None;
    }
    if let Type::Path(struct_type) = impl_item.self_ty.as_ref() {
        return Some(&struct_type.path.segments.last()?.ident);
    }
    None
}

/// Removes the `impl RticSwTask for <ident>` block from `impls`, erroring if
/// the task has more than one such implementation.
fn take_impl_for(
    impls: &mut Vec<(Ident, ItemImpl)>,
    ident: &Ident,
) -> syn::Result<Option<ItemImpl>> {
    let mut found = None;
    let mut i = 0;
    while i < impls.len() {
        if &impls[i].0 == ident {
            let (_, impl_) = impls.remove(i);
            if found.replace(impl_).is_some() {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Multiple `RticSwTask` implementations found for task `{ident}`."),
                ));
            }
        } else {
            i += 1;
        }
    }
    Ok(found)
}
