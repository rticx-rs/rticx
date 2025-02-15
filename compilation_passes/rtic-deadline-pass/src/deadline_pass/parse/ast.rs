use rtic_core::parse_utils::RticAttr;
use syn::{Expr, ItemStruct, Lit};

#[derive(Debug)]
pub struct RticTask {
    pub params: RticAttr,
    pub attr_idx: usize,
    pub task_struct: ItemStruct,
    pub deadline: Option<u32>, // explicit deadline
}

impl RticTask {
    pub fn from_struct((task_struct, attr_idx): (ItemStruct, usize)) -> syn::Result<Self> {
        let params = RticAttr::parse_from_attr(&task_struct.attrs[attr_idx])?;

        let deadline = if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(int), ..
        })) = params.elements.get("deadline")
        {
            // deadline explicitly assigned by the user
            int.base10_parse().ok()
        } else {
            None
        };

        if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(_int),
            ..
        })) = params.elements.get("priority")
        {
            panic!("'priority' found, please use 'deadlines' only or compile with --no-default-features.")
        }

        Ok(Self {
            params,
            attr_idx,
            task_struct,
            deadline,
        })
    }
}
