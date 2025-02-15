mod auto_assign;
mod codegen;
mod error;
mod parse;

use crate::codegen::CodeGen;
use crate::parse::App;
use proc_macro2::TokenStream;
use rtic_core::parse_utils::RticAttr;
use rtic_core::RticPass;
use syn::ItemMod;

pub struct AutoAssignPass;

impl RticPass for AutoAssignPass {
    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        let params = RticAttr::parse_from_tokens(&args)?;
        let mut parsed = App::parse(&params, app_mod)?;
        auto_assign::run(&mut parsed)?;
        let code = CodeGen::new(parsed).run();
        Ok((args, code))
    }

    fn pass_name(&self) -> &str {
        "AutoAssign"
    }
}
