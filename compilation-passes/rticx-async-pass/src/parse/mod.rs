use crate::parse::ast::{AppParameters, AsyncTask, TaskParams, into_task_attr};
use proc_macro2::{Ident, Span, TokenStream};
use rticx_core::parse_utils::RticAttr;
use rticx_sw_pass::common::parse::{get_task_implementor, item_attrs, take_impl_for};
use std::collections::HashMap;
use syn::{Attribute, Item, ItemImpl, ItemMod, Visibility, spanned::Spanned};

pub mod ast;

pub const ASYNC_TASK_TRAIT_TY: &str = "RticAsyncTask";

/// Type to represent a sub application (application on a single core)
#[derive(Clone)]
pub struct SubApp {
    pub core: u32,
    pub dispatchers: Vec<syn::Path>,
    /// Span of the `dispatchers = [...]` app argument, for error reporting.
    pub dispatchers_span: Option<Span>,
    /// Single core/ Core-local software tasks
    pub sw_tasks: Vec<AsyncTask>,
    /// Multi core/ software tasks to be spawned on this core from other cores
    pub mc_sw_tasks: Vec<AsyncTask>,
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
        let mut has_user_idle = false;
        let mut rest_of_code = Vec::with_capacity(app_mod_items.len());

        for item in app_mod_items {
            if !matches!(item, Item::Struct(_))
                && let Some(attrs) = item_attrs(&item)
                && let Some(attr_idx) = find_async_task_attr(attrs)
            {
                return Err(syn::Error::new(
                    attrs[attr_idx].span(),
                    "`#[async_task]` can only be applied to structs.",
                ));
            }
            match item {
                Item::Struct(struct_) => {
                    if let Some(attr_idx) = find_async_task_attr(&struct_.attrs) {
                        sw_task_structs.push((struct_, attr_idx))
                    } else {
                        if struct_.attrs.iter().any(|a| a.path().is_ident("idle")) {
                            has_user_idle = true;
                        }
                        rest_of_code.push(Item::Struct(struct_))
                    }
                }
                Item::Impl(impl_) => {
                    if let Some(implementor) = get_async_task_implementor(&impl_) {
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
            // The `impl RticAsyncTask for <struct>` is optional at this stage:
            // it may also live outside the `#[app]` module.  The core pass
            // generates static checks that the trait is implemented.
            let task_impl =
                take_impl_for(&mut sw_task_impls, &task_struct.ident, ASYNC_TASK_TRAIT_TY)?;

            let mut attrs = RticAttr::parse_from_attr(&task_struct.attrs[attr_idx])?;
            let params = TaskParams::from_attr(&mut attrs)?;
            let task_attr =
                into_task_attr(attrs, Ident::new(ASYNC_TASK_TRAIT_TY, Span::call_site()));
            // Consume the original `#[async_task]` attribute: the
            // reconstructed `#[task(...)]` attribute replaces it in the
            // generated code.
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

            let task = AsyncTask {
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

        // `impl RticAsyncTask for X` blocks whose `X` is not an async task are
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
            let sw = sw_tasks.remove(&core).unwrap_or_default();
            let mc = mc_sw_tasks.remove(&core).unwrap_or_default();
            sub_apps.push(SubApp {
                core,
                dispatchers,
                dispatchers_span: app_params.dispatchers_span,
                sw_tasks: sw,
                mc_sw_tasks: mc,
            })
        }

        let has_prio_0 = sub_apps.iter().any(|sub| {
            sub.sw_tasks
                .iter()
                .chain(sub.mc_sw_tasks.iter())
                .any(|t| t.params.priority == 0)
        });
        if has_prio_0 && has_user_idle {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "Cannot define a custom `#[idle]` task when priority-0 async tasks are present. Use `#[post_init]` to spawn initial tasks instead.",
            ));
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

/// Returns the index of the first `async_task` attribute in `attrs`, if any.
fn find_async_task_attr(attrs: &[Attribute]) -> Option<usize> {
    attrs
        .iter()
        .position(|attr| attr.path().is_ident("async_task"))
}

/// The implementor of an `impl <trait> for <type>` block whose trait name ends
/// with `RticAsyncTask`.  The last path segment is matched so that qualified
/// paths (e.g. `crate::RticAsyncTask`) are recognized too.
fn get_async_task_implementor(impl_item: &ItemImpl) -> Option<&Ident> {
    get_task_implementor(impl_item, ASYNC_TASK_TRAIT_TY)
}
