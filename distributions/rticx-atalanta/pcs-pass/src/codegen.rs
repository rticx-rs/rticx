use super::parse::App;
use quote::quote;
use syn::parse_quote;

pub struct Codegen {
    app: App,
}

impl Codegen {
    pub fn new(app: App) -> Codegen {
        Self { app }
    }

    pub fn run(&mut self) -> syn::ItemMod {
        // Generate an entry point for each task
        let trap_entries = self.app.tasks.iter_mut().map(|task| {
            let interrupt_name = task.binds.clone();
            if task.fast {
                quote!(bsp::generate_pcs_trap_entry!(#interrupt_name);)
            } else {
                quote!(bsp::generate_nested_trap_entry!(#interrupt_name);)
            }
        });

        let mod_visibility = &self.app.mod_visibility;
        let mod_ident = &self.app.mod_ident;
        let other_code = &self.app.code;

        parse_quote! {
            #mod_visibility mod #mod_ident {
                #(#other_code)*

                #(#trap_entries)*
            }
        }
    }
}
