use proc_macro2::Ident;
use rticx_core::parse_utils::RticAttr;
use syn::{ItemImpl, ItemStruct};

pub use rticx_sw_pass::common::parse::{
    AppParameters, TaskParams, int_span, into_task_attr, parse_attr_int,
};

#[derive(Debug, Clone)]
pub struct AsyncTask {
    pub params: TaskParams,
    /// `#[task(...)]` attribute reconstructed for the core pass: `async_task`
    /// renamed to `task`, pass-only keys removed, `task_trait` added.
    pub task_attr: RticAttr,
    pub task_struct: ItemStruct,
    /// The `impl RticAsyncTask for <struct>` block, if present inside the
    /// `#[app]` module.  Optional because the implementation may also live in
    /// another module; the core pass generates static checks that the trait
    /// is implemented for every task.
    pub task_impl: Option<ItemImpl>,
}

impl AsyncTask {
    pub fn name(&self) -> &Ident {
        &self.task_struct.ident
    }
}
