use crate::parse::App;

use quote::quote;
use syn::{parse_quote, ItemMod};

pub struct CodeGen {
    app: App,
}

impl CodeGen {
    pub fn new(app: App) -> CodeGen {
        Self { app }
    }

    pub fn run(&mut self) -> ItemMod {
        let tasks = self.app.tasks.iter_mut().map(|task| {
            let task_attribute = &task.params;
            let task_struct = &mut task.task_struct;
            task_struct.attrs.remove(task.attr_idx); // remove the older task attribute and replace with the updated one which includes an automatically assinged core
            quote! {
                #task_attribute
                #task_struct
            }
        });

        let mod_visibility = &self.app.mod_visibility;
        let mod_ident = &self.app.mod_ident;
        let other_code = &self.app.rest_of_code;
        let shared_resources = self.app.shared_resources.iter().map(|s| &s.shared_struct);

        parse_quote! {
            #mod_visibility mod #mod_ident {
                #(#other_code)*
                #(#shared_resources)*
                #(#tasks)*
            }
        }
    }
}
