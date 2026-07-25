use proc_macro2::TokenStream;
use rticx_core::RticPass;
use syn::ItemMod;

pub struct AsyncPass;

impl RticPass for AsyncPass {
    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        // TODO: implement async task transformation
        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        "AsyncPass"
    }
}
