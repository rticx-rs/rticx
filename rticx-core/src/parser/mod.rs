use std::collections::HashMap;

use proc_macro2::Span;
use quote::format_ident;
use syn::{Ident, Item, ItemFn, ItemImpl, ItemStruct, ItemUse, Type, spanned::Spanned};

use ast::*;

use crate::common_internal::rticx_traits::{HWT_TRAIT_TY, IDLE_TRAIT_TY, SWT_TRAIT_TY};

pub mod ast;

#[derive(Debug)]
pub struct SubApp {
    pub core: u32,
    pub shared: Option<SharedResources>,
    pub init: InitTask,
    pub idle: Option<IdleTask>,
    pub tasks: Vec<HardwareTask>,
}

#[derive(Debug)]
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
        // idle tasks are a list because the framework may allow more than one idle task in multicore setups,
        // but it is not decided yet how this will be handled
        let mut idles = Vec::new();
        let mut task_structs = Vec::new();
        let mut task_impls: HashMap<String, ItemImpl> = HashMap::new();
        let mut user_includes = Vec::new();
        let mut other_code = Vec::new();
        let app_mod_items = module
            .content
            .ok_or(syn::Error::new(span, "Empty app module."))?
            .1;

        for item in app_mod_items {
            match item {
                Item::Fn(function) => {
                    if let Some(attr_idx) = Self::is_init(&function) {
                        inits.push((function, attr_idx))
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
                    if let Some(implementor) = Self::is_task_impl(&impl_item) {
                        let _ = task_impls.insert(implementor, impl_item);
                    } else {
                        other_code.push(impl_item.into())
                    }
                }
                Item::Use(use_item) => user_includes.push(use_item),
                _ => other_code.push(item),
            }
        }

        let mut shared = Self::construct_shared_resources(shared_resources)?;
        let mut inits = Self::construct_inits(inits, span)?;
        let mut idles = Self::construct_idle_tasks(idles, &task_impls)?;
        let mut tasks = Self::construct_rtic_tasks(task_structs, &task_impls)?;

        // partition into sub_applications
        let mut sub_apps = Vec::with_capacity(args.cores as usize);
        for core in 0..args.cores {
            sub_apps.push(SubApp {
                core,
                shared: shared.remove(&core),
                init: inits
                    .remove(&core)
                    .unwrap_or_else(|| panic!("No init found for core {core}")),
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

    fn is_task_impl(impl_item: &ItemImpl) -> Option<String> {
        if let Some((_, ref path, _)) = impl_item.trait_ {
            if path.segments.is_empty() {
                return None;
            }

            let is_hw_task = path.segments[0].ident.to_string().ends_with(HWT_TRAIT_TY);
            let is_sw_task = path.segments[0].ident.to_string().ends_with(SWT_TRAIT_TY);
            let is_idle = path.segments[0].ident.to_string().ends_with(IDLE_TRAIT_TY);
            let is_task = is_hw_task || is_sw_task || is_idle;

            if is_task && let Type::Path(struct_type) = impl_item.self_ty.as_ref() {
                let implementor_name = struct_type.path.segments[0].ident.to_string();
                return Some(implementor_name);
            }
        }
        None
    }

    fn construct_shared_resources(
        shared_resources: Vec<(ItemStruct, usize)>,
    ) -> syn::Result<HashMap<u32, SharedResources>> {
        shared_resources
            .into_iter()
            .map(|(mut strct, attr_idx)| {
                // remove the #[shared] attribute
                let attr = strct.attrs.remove(attr_idx);
                let args = SharedResourcesArgs::parse(attr.meta)?;
                let parsed_elements = strct
                    .fields
                    .iter()
                    .map(|f| SharedElement {
                        ident: f
                            .ident
                            .clone()
                            .expect("unnamed struct is not supported for shared resources"),
                        ty: f.ty.clone(),
                        priority: 0,
                    })
                    .collect();
                Ok((
                    args.core,
                    SharedResources {
                        args,
                        strct,
                        resources: parsed_elements,
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, syn::Error>>()
    }

    /// links the tasks struct definitions with their implementation part and generates a RticTask struct of it.
    /// The returned tasks are already split between hardware and software tasks
    fn construct_rtic_tasks(
        task_structs: Vec<(ItemStruct, usize)>,
        task_impls: &HashMap<String, ItemImpl>,
    ) -> syn::Result<HashMap<u32, Vec<RticTask>>> {
        let mut out = HashMap::new();
        for (mut task_struct, attr_idx) in task_structs {
            // parse the task attribute args
            let attr = task_struct.attrs.remove(attr_idx);
            let args = TaskArgs::parse(attr.meta)?;

            // find the task struct impl
            let struct_impl = task_impls.get(&task_struct.ident.to_string());

            let tasks = out.entry(args.core).or_insert_with(Vec::new);
            let mut task = RticTask {
                args,
                task_struct,
                struct_impl: struct_impl.cloned(),
                user_initializable: false, //initially this is false.
            };
            task.adjust_task_impl_initialization()?; // adjust the init method and args type of the task trait implementation
            tasks.push(task);
        }
        Ok(out)
    }

    fn construct_idle_tasks(
        idles: Vec<(ItemStruct, usize)>,
        task_impls: &HashMap<String, ItemImpl>,
    ) -> syn::Result<HashMap<u32, IdleTask>> {
        idles
            .into_iter()
            .map(|(mut idle_struct, init_attr_idx)| {
                // find the task struct impl
                let struct_impl = task_impls.get(&idle_struct.ident.to_string());

                // remove the #[idle]
                let attrs = idle_struct.attrs.remove(init_attr_idx);
                let mut args = TaskArgs::parse(attrs.meta)?;
                args.task_trait = format_ident!("{IDLE_TRAIT_TY}"); // correct the trait type for idle
                let core = args.core;
                let mut task = IdleTask {
                    args,
                    task_struct: idle_struct,
                    struct_impl: struct_impl.cloned(),
                    user_initializable: false,
                };
                task.adjust_task_impl_initialization()?; // adjust the init method and args type of the task trait implementation
                Ok((core, task))
            })
            .collect::<Result<HashMap<_, _>, syn::Error>>()
    }

    fn construct_inits(
        inits: Vec<(ItemFn, usize)>,
        module_span: Span,
    ) -> syn::Result<HashMap<u32, InitTask>> {
        if inits.is_empty() {
            Err(syn::Error::new(
                module_span,
                "No function with #[init] attribute was found in this module.",
            ))
        } else {
            inits
                .into_iter()
                .map(|(mut init_fn, init_attr_idx)| {
                    // // check return type
                    // let expected_ret = format!("-> {}", shared_resources.strct.ident);
                    // let found_ret = format!("{}", init_fn.sig.output.to_token_stream());
                    // if found_ret != expected_ret {
                    //     return Err(syn::Error::new(
                    //         init_fn.span(),
                    //         format!(
                    //             "Expected function return type to be {expected_ret}, found {found_ret}."
                    //         ),
                    //     ));
                    // }

                    // remove the [#init]
                    let attr = init_fn.attrs.remove(init_attr_idx);
                    let args = InitTaskArgs::parse(attr.meta)?;
                    Ok((
                        args.core,
                        InitTask {
                            args,
                            ident: init_fn.sig.ident.clone(),
                            body: init_fn,
                        },
                    ))
                })
                .collect::<Result<HashMap<_, _>, syn::Error>>()
        }
    }
}
