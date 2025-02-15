use std::sync::atomic::{AtomicU32, Ordering};

use crate::parse::ast::RticTask;
use proc_macro2::Ident;
use rtic_core::parse_utils::RticAttr;
use syn::{Expr, Item, ItemMod, ItemStruct, Lit, Visibility};

use self::ast::SharedResources;

pub mod ast;

/// Type to represent an RTIC application (within auto core assignment pass context)
pub struct App {
    pub mod_visibility: Visibility,
    pub mod_ident: Ident,
    pub shared_resources: Vec<SharedResources>,
    pub tasks: Vec<RticTask>,
    pub rest_of_code: Vec<Item>,
}

pub static APP_CORES: AtomicU32 = AtomicU32::new(1);

impl App {
    pub fn parse(params: &RticAttr, mut app_mod: ItemMod) -> syn::Result<Self> {
        // parse the number of cores
        let cores = parse_cores_arg(params)?;
        APP_CORES.store(cores, Ordering::SeqCst);

        let app_mod_items = app_mod.content.take().unwrap_or_default().1;
        let mut task_structs = Vec::new();
        let mut shared_structs = Vec::new();
        let mut rest_of_code = Vec::with_capacity(app_mod_items.len());

        for item in app_mod_items {
            match item {
                Item::Struct(strct) => {
                    if let Some(attr_idx) = is_struct_with_attr(&strct, "shared") {
                        shared_structs.push((strct, attr_idx))
                    } else if let Some(attr_idx) = is_struct_with_attr(&strct, "task") {
                        task_structs.push((strct, attr_idx))
                    } else if let Some(attr_idx) = is_struct_with_attr(&strct, "sw_task") {
                        task_structs.push((strct, attr_idx))
                    } else if let Some(attr_idx) = is_struct_with_attr(&strct, "idle") {
                        task_structs.push((strct, attr_idx))
                    } else {
                        rest_of_code.push(Item::Struct(strct))
                    }
                }
                _ => rest_of_code.push(item),
            }
        }
        let tasks = task_structs
            .into_iter()
            .map(RticTask::from_struct)
            .collect::<syn::Result<_>>()?;

        let shared_resources = shared_structs
            .into_iter()
            .map(SharedResources::from_struct)
            .collect::<syn::Result<_>>()?;

        Ok(Self {
            mod_ident: app_mod.ident,
            mod_visibility: app_mod.vis,
            tasks,
            shared_resources,
            rest_of_code,
        })
    }
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

fn parse_cores_arg(params: &RticAttr) -> Result<u32, syn::Error> {
    let cores = if let Some(Expr::Lit(syn::ExprLit {
        lit: Lit::Int(ref cores),
        ..
    })) = params.elements.get("cores")
    {
        cores.base10_parse()?
    } else {
        // return Err(Error::NoCores.into());
        1
    };
    Ok(cores)
}
