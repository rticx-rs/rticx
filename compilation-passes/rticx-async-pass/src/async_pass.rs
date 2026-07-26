use proc_macro2::TokenStream;
use rticx_core::RticPass;
use syn::ItemMod;

pub struct AsyncPass;

impl RticPass for AsyncPass {
    fn subscribe(&mut self, _info_bus: rticx_core::InfoBus) {}
    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        // TODO: implement async task transformation
        Ok((args, app_mod))
    }

    fn pass_name(&self) -> &str {
        "AsyncPass"
    }
}
