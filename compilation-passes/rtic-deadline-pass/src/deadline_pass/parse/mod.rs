use super::parse::ast::RticTask;
use proc_macro2::Ident;
use rtic_core::parse_utils::RticAttr;
use syn::{Item, ItemMod, ItemStruct, Visibility};

pub mod ast;

/// Type to represent an RTIC application for deadline to priority conversion
pub struct App {
    pub mod_visibility: Visibility,
    pub mod_ident: Ident,

    pub tasks: Vec<RticTask>,
    pub rest_of_code: Vec<Item>,
}

impl App {
    pub fn parse(_params: &RticAttr, mut app_mod: ItemMod) -> syn::Result<Self> {
        let app_mod_items = app_mod.content.take().unwrap_or_default().1;
        let mut task_structs = Vec::new();
        let mut rest_of_code = Vec::with_capacity(app_mod_items.len());

        for item in app_mod_items {
            match item {
                Item::Struct(strct) => {
                    if let Some(attr_idx) = is_struct_with_attr(&strct, "task") {
                        task_structs.push((strct, attr_idx))
                    } else if let Some(attr_idx) = is_struct_with_attr(&strct, "sw_task") {
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

        Ok(Self {
            mod_ident: app_mod.ident,
            mod_visibility: app_mod.vis,
            tasks,
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
