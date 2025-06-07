use crate::PCS_ATTR_IDENT;
use quote::{format_ident, ToTokens};
use rtic_core::parse_utils::RticAttr;
use syn::{Ident, ItemStruct};

#[derive(Debug)]
pub struct RticTask {
    pub name: String,
    pub binds: Ident,
    /// User has requested parallel context stacking (PCS) for this line
    pub fast: bool,
}

impl RticTask {
    pub fn from_struct((task_struct, attr_idx): (&ItemStruct, usize)) -> syn::Result<Self> {
        let name = task_struct.ident.to_string();
        let params = RticAttr::parse_from_attr(&task_struct.attrs[attr_idx]).inspect_err(|_e| {
            eprintln!(
                "An error occurred while parsing: {:?}",
                task_struct.attrs[attr_idx].to_token_stream().to_string()
            )
        })?;
        let binds_expr = params
            .elements
            .get("binds")
            .expect("Internal error: any task should always have a bound interrupt");
        let binds = format_ident!("{}", binds_expr.to_token_stream().to_string());
        let fast = params.elements.contains_key(PCS_ATTR_IDENT);

        Ok(Self { name, binds, fast })
    }
}
