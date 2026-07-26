pub mod codegen;
pub mod parse;
pub use parse::App;

use codegen::CodeGen;
use proc_macro2::TokenStream;
use rticx_core::RticPass;
use rticx_core::parse_utils::RticAttr;
use syn::{ItemMod, parse_quote};

pub struct DeadlineToPriorityPass {
    max_priority: u16,
}

impl DeadlineToPriorityPass {
    #[allow(clippy::new_without_default)]
    pub fn new(max_priority: u16) -> Self {
        Self { max_priority }
    }
}

impl RticPass for DeadlineToPriorityPass {
    fn subscribe(&mut self, _info_bus: rticx_core::InfoBus) {}
    fn run_pass(&self, args: TokenStream, app_mod: ItemMod) -> syn::Result<(TokenStream, ItemMod)> {
        let params = RticAttr::parse_from_tokens(args.clone())?;

        let mut parsed = App::parse(&params, app_mod)?;

        self.analyze(&mut parsed);

        for task in parsed.tasks.iter_mut() {
            if let Some(deadline) = task.deadline {
                task.params.elements.remove("deadline");
                let expr: syn::Expr = parse_quote! { #deadline };
                let _ = task.params.elements.insert("priority".into(), expr);
            } else {
                continue;
            }
        }

        let code = CodeGen::new(parsed).run();
        Ok((args, code))
    }

    fn pass_name(&self) -> &str {
        "deadline_pass"
    }
}

impl DeadlineToPriorityPass {
    pub fn analyze(&self, app: &mut App) {
        let mut deadlines: Vec<_> = app
            .tasks
            .iter()
            .map(|task| task.deadline.unwrap_or(u32::MAX))
            .collect();
        eprintln!("task deadlines {:?}", deadlines);

        deadlines.sort();
        eprintln!("sorted {:?}", deadlines);
        deadlines.dedup();
        eprintln!("sorted dedup {:?}", deadlines);
        // NOTE: Do NOT reverse - shorter deadlines should get higher priority (lower number)
        // deadlines.reverse();
        // eprintln!("sorted dedup reversed {:?}", deadlines);

        if deadlines.len() as u16 > self.max_priority {
            panic!(
                "Exceeded number of priorities for this platform ({}), please coerce deadlines manually.",
                self.max_priority
            );
        }

        for t in app.tasks.iter_mut() {
            match t.deadline {
                None => {}
                Some(v) => {
                    let pos = deadlines.iter().position(|d| *d == v).unwrap();
                    t.deadline = Some(pos as u32 + 1);
                }
            }
        }
    }
}
