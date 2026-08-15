use std::collections::HashMap;

use proc_macro2::Span;
use quote::format_ident;
use syn::{Ident, Item, ItemFn, ItemImpl, ItemStruct, ItemUse, Type, spanned::Spanned};

use ast::*;

use crate::common_internal::rticx_traits::IDLE_TRAIT_TY;

pub mod ast;

#[derive(Debug, Clone)]
pub struct SubApp {
    pub core: u32,
    pub shared: Option<SharedResources>,
    pub init: InitTask,
    pub post_init: Option<PostInitTask>,
    pub idle: Option<IdleTask>,
    pub tasks: Vec<HardwareTask>,
}

#[derive(Debug, Clone)]
pub struct App {
    pub app_name: Ident,
    pub args: AppArgs,
    pub sub_apps: Vec<SubApp>,
    pub user_includes: Vec<ItemUse>,
    pub other_code: Vec<Item>,
}

impl App {
    pub fn parse(args: proc_macro2::TokenStream, module: syn::ItemMod) -> syn::Result<Self> {
        let span = module.span();
        let args = AppArgs::parse(args)?;
        let mut shared_resources = Vec::new();
        let mut inits = Vec::with_capacity(1);
        let mut post_inits = Vec::new();
        // idle tasks are a list because the framework may allow more than one idle task in multicore setups,
        // but it is not decided yet how this will be handled
        let mut idles = Vec::new();
        let mut task_structs = Vec::new();
        let mut task_impls: HashMap<String, Vec<ItemImpl>> = HashMap::new();
        let mut user_includes = Vec::new();
        let mut other_code = Vec::new();
        let mut errors = Vec::new();
        let app_mod_items = module
            .content
            .ok_or(syn::Error::new(span, "Empty app module."))?
            .1;

        for item in app_mod_items {
            match item {
                Item::Fn(function) => {
                    if let Some(attr_idx) = Self::is_init(&function) {
                        inits.push((function, attr_idx))
                    } else if let Some(attr_idx) = Self::is_post_init(&function) {
                        post_inits.push((function, attr_idx))
                    } else {
                        other_code.push(function.into())
                    }
                }
                Item::Struct(strct) => {
                    if let Some(attr_idx) = Self::is_struct_with_attr(&strct, "task") {
                        task_structs.push((strct, attr_idx))
                    } else if let Some(attr_idx) = Self::is_struct_with_attr(&strct, "shared") {
                        shared_resources.push((strct, attr_idx));
                    } else if let Some(attr_idx) = Self::is_struct_with_attr(&strct, "idle") {
                        idles.push((strct, attr_idx))
                    } else {
                        other_code.push(strct.into())
                    }
                }
                Item::Impl(impl_item) => {
                    if let Some(implementor) = Self::capture_trait_impl(&impl_item) {
                        let impls = task_impls.entry(implementor.clone()).or_default();
                        if let Some(existing) = impls.iter().find(|existing| {
                            Self::impl_trait_str(existing) == Self::impl_trait_str(&impl_item)
                        }) {
                            let trait_str =
                                Self::impl_trait_str(&impl_item).unwrap_or_else(|| "?".to_string());
                            let mut error = syn::Error::new(
                                impl_item.span(),
                                format!("duplicate `impl {trait_str}` for `{implementor}`"),
                            );
                            error.combine(syn::Error::new(
                                existing.span(),
                                "first impl defined here",
                            ));
                            errors.push(error);
                        }
                        impls.push(impl_item);
                    } else {
                        other_code.push(impl_item.into())
                    }
                }
                Item::Use(use_item) => user_includes.push(use_item),
                _ => other_code.push(item),
            }
        }

        if let Some(error) = Self::combine_errors(errors) {
            return Err(error);
        }

        let mut shared = Self::construct_shared_resources(shared_resources)?;
        let mut inits = Self::construct_inits(inits, span)?;
        let mut post_inits = Self::construct_post_inits(post_inits)?;
        let mut idles = Self::construct_idle_tasks(idles, &task_impls)?;
        let mut tasks = Self::construct_rtic_tasks(task_structs, &task_impls)?;

        // partition into sub_applications
        let mut sub_apps = Vec::with_capacity(args.cores as usize);
        for core in 0..args.cores {
            sub_apps.push(SubApp {
                core,
                shared: shared.remove(&core),
                init: match inits.remove(&core) {
                    Some(init) => init,
                    None => {
                        return Err(syn::Error::new(
                            span,
                            format!("No `#[init]` function was found for core {core}."),
                        ));
                    }
                },
                post_init: post_inits.remove(&core),
                idle: idles.remove(&core),
                tasks: tasks.remove(&core).unwrap_or_default(),
            })
        }

        Ok(Self {
            app_name: module.ident,
            args,
            sub_apps,
            user_includes,
            other_code,
        })
    }

    fn is_init(function: &ItemFn) -> Option<usize> {
        for (i, attr) in function.attrs.iter().enumerate() {
            let path = attr.meta.path();
            // we are looking for a path that has a single segment
            if path.segments.len() == 1 && path.segments[0].ident == "init" {
                return Some(i);
            }
        }
        None
    }

    fn is_post_init(function: &ItemFn) -> Option<usize> {
        for (i, attr) in function.attrs.iter().enumerate() {
            let path = attr.meta.path();
            if path.segments.len() == 1 && path.segments[0].ident == "post_init" {
                return Some(i);
            }
        }
        None
    }

    /// returns the index of the `attr_name` attribute if found in the attribute list of some struct
    fn is_struct_with_attr(strct: &ItemStruct, attr_name: &str) -> Option<usize> {
        for (i, attr) in strct.attrs.iter().enumerate() {
            let path = attr.meta.path();
            if path.segments.len() == 1 && path.segments[0].ident == attr_name {
                return Some(i);
            }
        }
        None
    }

    fn capture_trait_impl(impl_item: &ItemImpl) -> Option<String> {
        if let Some((_, ref path, _)) = impl_item.trait_
            && !path.segments.is_empty()
            && let Type::Path(struct_type) = impl_item.self_ty.as_ref()
        {
            return struct_type
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string());
        }
        None
    }

    /// String of the trait an impl block implements, e.g. `RticTask`.
    fn impl_trait_str(impl_item: &ItemImpl) -> Option<String> {
        impl_item
            .trait_
            .as_ref()
            .and_then(|(_, path, _)| path.segments.last())
            .map(|segment| segment.ident.to_string())
    }

    fn combine_errors(mut errors: Vec<syn::Error>) -> Option<syn::Error> {
        let mut combined = errors.pop()?;
        for error in errors {
            combined.combine(error);
        }
        Some(combined)
    }

    fn construct_shared_resources(
        shared_resources: Vec<(ItemStruct, usize)>,
    ) -> syn::Result<HashMap<u32, SharedResources>> {
        let mut out = HashMap::new();
        for (mut strct, attr_idx) in shared_resources {
            // remove the #[shared] attribute
            let attr = strct.attrs.remove(attr_idx);
            let args = SharedResourcesArgs::parse(attr.meta)?;
            if out.contains_key(&args.core) {
                return Err(syn::Error::new(
                    strct.ident.span(),
                    format!("multiple `#[shared]` structs for core {}", args.core),
                ));
            }
            let parsed_elements = strct
                .fields
                .iter()
                .map(|f| {
                    let ident = f.ident.clone().ok_or_else(|| {
                        syn::Error::new(
                            f.span(),
                            "`#[shared]` struct must use named fields; tuple structs are not supported.",
                        )
                    })?;
                    Ok(SharedElement {
                        ident,
                        ty: f.ty.clone(),
                        priority: 0,
                    })
                })
                .collect::<syn::Result<_>>()?;
            out.insert(
                args.core,
                SharedResources {
                    args,
                    strct,
                    resources: parsed_elements,
                },
            );
        }
        Ok(out)
    }

    /// links the tasks struct definitions with their implementation part and generates a RticTask struct of it.
    /// The returned tasks are already split between hardware and software tasks
    fn construct_rtic_tasks(
        task_structs: Vec<(ItemStruct, usize)>,
        task_impls: &HashMap<String, Vec<ItemImpl>>,
    ) -> syn::Result<HashMap<u32, Vec<RticTask>>> {
        let mut out = HashMap::new();
        for (mut task_struct, attr_idx) in task_structs {
            // parse the task attribute args
            let attr = task_struct.attrs.remove(attr_idx);
            let args = TaskArgs::parse(attr.meta)?;

            // find the task struct impl and verify its trait matches the task_trait
            let trait_name = args.task_trait.to_string();
            let struct_impl = task_impls
                .get(&task_struct.ident.to_string())
                .and_then(|impls| {
                    impls.iter().find(|impl_item| {
                        Self::impl_trait_str(impl_item).as_deref() == Some(trait_name.as_str())
                    })
                })
                .cloned();

            let tasks = out.entry(args.core).or_insert_with(Vec::new);
            let task = RticTask {
                args,
                task_struct,
                struct_impl,
            };
            tasks.push(task);
        }
        Ok(out)
    }

    fn construct_idle_tasks(
        idles: Vec<(ItemStruct, usize)>,
        task_impls: &HashMap<String, Vec<ItemImpl>>,
    ) -> syn::Result<HashMap<u32, IdleTask>> {
        let mut out = HashMap::new();
        for (mut idle_struct, init_attr_idx) in idles {
            // find the task struct impl and verify its trait is RticIdleTask
            let struct_impl = task_impls
                .get(&idle_struct.ident.to_string())
                .and_then(|impls| {
                    impls.iter().find(|impl_item| {
                        Self::impl_trait_str(impl_item).as_deref() == Some(IDLE_TRAIT_TY)
                    })
                })
                .cloned();

            // remove the #[idle]
            let attrs = idle_struct.attrs.remove(init_attr_idx);
            let mut args = TaskArgs::parse(attrs.meta)?;
            args.task_trait = format_ident!("{IDLE_TRAIT_TY}"); // correct the trait type for idle
            let core = args.core;
            if out.contains_key(&core) {
                return Err(syn::Error::new(
                    idle_struct.ident.span(),
                    format!("multiple `#[idle]` structs for core {core}"),
                ));
            }
            let task = IdleTask {
                args,
                task_struct: idle_struct,
                struct_impl,
            };
            out.insert(core, task);
        }
        Ok(out)
    }

    fn construct_inits(
        inits: Vec<(ItemFn, usize)>,
        module_span: Span,
    ) -> syn::Result<HashMap<u32, InitTask>> {
        if inits.is_empty() {
            return Err(syn::Error::new(
                module_span,
                "No function with #[init] attribute was found in this module.",
            ));
        }

        let mut out = HashMap::new();
        for (mut init_fn, init_attr_idx) in inits {
            // remove the [#init]
            let attr = init_fn.attrs.remove(init_attr_idx);
            let args = InitTaskArgs::parse(attr.meta)?;
            if out.contains_key(&args.core) {
                return Err(syn::Error::new(
                    init_fn.sig.ident.span(),
                    format!("multiple `#[init]` functions for core {}", args.core),
                ));
            }
            out.insert(
                args.core,
                InitTask {
                    args,
                    ident: init_fn.sig.ident.clone(),
                    body: init_fn,
                },
            );
        }
        Ok(out)
    }

    fn construct_post_inits(
        post_inits: Vec<(ItemFn, usize)>,
    ) -> syn::Result<HashMap<u32, PostInitTask>> {
        let mut out = HashMap::new();
        for (mut post_init_fn, init_attr_idx) in post_inits {
            let attr = post_init_fn.attrs.remove(init_attr_idx);
            let args = InitTaskArgs::parse(attr.meta)?;
            if out.contains_key(&args.core) {
                return Err(syn::Error::new(
                    post_init_fn.sig.ident.span(),
                    format!("multiple `#[post_init]` functions for core {}", args.core),
                ));
            }
            out.insert(
                args.core,
                PostInitTask {
                    args,
                    ident: post_init_fn.sig.ident.clone(),
                    body: post_init_fn,
                },
            );
        }
        Ok(out)
    }
}
