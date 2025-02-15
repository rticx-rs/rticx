use std::sync::atomic::Ordering;

use quote::{format_ident, ToTokens};
use rtic_core::parse_utils::RticAttr;
use syn::{parse_quote, Expr, ItemStruct, Lit};

use crate::error::Error;

use super::APP_CORES;

#[derive(Debug)]
pub struct RticTask {
    pub params: RticAttr,
    pub attr_idx: usize,
    pub shared_items: Vec<syn::Ident>,
    pub task_struct: ItemStruct,
    pub core: Option<u32>, // core to be assinged (if not already by user) during automatic core assinged part.
                           // Note that this may still be None after the auto assignment part
                           // if the task doesn't use any shared resources (explicit assingment required)
}

impl RticTask {
    pub fn from_struct((task_struct, attr_idx): (ItemStruct, usize)) -> syn::Result<Self> {
        let params = RticAttr::parse_from_attr(&task_struct.attrs[attr_idx])?;

        let core = if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(int), ..
        })) = params.elements.get("core")
        {
            // core explicitly assigned by the user
            int.base10_parse().ok()
        } else {
            None
        };

        let shared_items = if let Some(Expr::Array(arr)) = params.elements.get("shared") {
            arr.elems
                .iter()
                .map(|item| format_ident!("{}", item.to_token_stream().to_string()))
                .collect()
        } else {
            Vec::new()
        };

        Ok(Self {
            core,
            params,
            attr_idx,
            shared_items,
            task_struct,
        })
    }

    pub fn assign_core(&mut self, core: u32) {
        let _ = self.core.insert(core);
        let expr: syn::Expr = parse_quote!(#core);
        self.params.elements.insert(String::from("core"), expr);
    }
}

#[derive(Debug)]
pub struct SharedResources {
    pub core: u32,
    pub shared_items: Vec<syn::Ident>,
    pub shared_struct: ItemStruct,
}

impl SharedResources {
    pub fn from_struct((shared_struct, attr_idx): (ItemStruct, usize)) -> syn::Result<Self> {
        let params = RticAttr::parse_from_attr(&shared_struct.attrs[attr_idx])?;
        let shared_items = shared_struct
            .fields
            .iter()
            .map(|f| {
                f.ident
                    .clone()
                    .expect("Tuple like structs are not supported for shared resources.")
            })
            .collect();

        let core = if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(int), ..
        })) = params.elements.get("core")
        {
            int.base10_parse().unwrap_or_default()
        } else if APP_CORES.load(Ordering::Relaxed) == 1 {
            0
        } else {
            return Err(Error::NoCoreArgShared(shared_struct.ident.to_string()).into());
        };

        Ok(Self {
            shared_items,
            shared_struct,
            core,
        })
    }
}
