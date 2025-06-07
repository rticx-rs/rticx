use super::parse::ast::RticTask;
use proc_macro2::Ident;
use rtic_core::parse_utils::RticAttr;
use syn::{Item, ItemMod, ItemStruct, Visibility};

pub mod ast;

/// Type to represent an RTIC application for PCS pass
pub struct App {
    pub mod_visibility: Visibility,
    pub mod_ident: Ident,

    pub tasks: Vec<RticTask>,

    /// Code that remains unchanged by this pass
    pub code: Vec<Item>,
}

impl App {
    pub fn parse(_params: &RticAttr, mut app_mod: ItemMod) -> syn::Result<Self> {
        let app_mod_items = app_mod.content.take().unwrap_or_default().1;
        let mut code = Vec::with_capacity(app_mod_items.len());
        let mut tasks = Vec::new();

        for item in app_mod_items {
            match item {
                Item::Struct(ref struct_) => {
                    if let Some(task_attr_idx) = locate_attr_in_struct("task", &struct_) {
                        tasks.push(RticTask::from_struct((struct_, task_attr_idx))?);
                    } else if let Some(attr_idx) = locate_attr_in_struct("sw_task", &struct_) {
                        tasks.push(RticTask::from_struct((struct_, attr_idx))?);
                    }
                }
                _ => {}
            }
            code.push(item);
        }

        Ok(Self {
            mod_ident: app_mod.ident,
            mod_visibility: app_mod.vis,
            tasks,
            code,
        })
    }
}

/// Returns the index of the `attr_name` attribute if found in the attribute list of some struct
fn locate_attr_in_struct(attr_name: &str, struct_: &ItemStruct) -> Option<usize> {
    struct_.attrs.iter().position(|attr| {
        let path = attr.meta.path();
        path.segments.len() == 1 && path.segments[0].ident == attr_name
    })
}
