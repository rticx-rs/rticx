use proc_macro2::{Span, TokenStream};
use rtic_core::{errors::ParseError, parse_utils::RticAttr};
use std::collections::HashMap;
use syn::{spanned::Spanned, Expr, Ident, ItemImpl, ItemStruct, Lit, Path};

pub struct AppParameters {
    pub dispatchers: HashMap<u32, Vec<Path>>,
    pub pacs: Vec<Path>,
    pub cores: u32,
}

impl AppParameters {
    pub fn parse(args: &TokenStream) -> syn::Result<Self> {
        let args_span = args.span();
        let mut args = RticAttr::parse_from_tokens(args.clone())?;

        // parse the number of cores
        let cores = args.elements.remove("cores");
        let cores = match cores {
            Some(Expr::Lit(syn::ExprLit {
                lit: Lit::Int(lit_int),
                ..
            })) => lit_int.base10_parse()?,
            _ => 1_u32,
        };

        // parse the path(s) to PAC(s)
        let device = args
            .elements
            .remove("device")
            .ok_or(ParseError::DeviceArg.to_syn(args_span))?;

        let pacs = match device {
            Expr::Array(array_exp) => {
                if array_exp.elems.len() != cores as usize {
                    return Err(ParseError::DevicesCoresMismatch.to_syn(args_span));
                }

                let mut devices = Vec::with_capacity(cores as usize);
                for exp in array_exp.elems {
                    if let Expr::Path(p) = exp {
                        devices.push(p.path)
                    } else {
                        return Err(ParseError::DeviceNotPath.to_syn(args_span));
                    }
                }
                devices
            }
            Expr::Path(path_to_pac) => {
                let mut devices = Vec::with_capacity(cores as usize);
                for _ in 0..cores {
                    devices.push(path_to_pac.path.clone())
                }
                devices
            }
            _ => return Err(ParseError::DeviceNotPath.to_syn(args_span)),
        };

        // dispatchers
        let mut dispatchers = HashMap::with_capacity(cores as usize);
        if let Some(Expr::Array(ref arr)) = args.elements.get("dispatchers") {
            for (i, element) in arr.elems.iter().enumerate() {
                if let Expr::Path(ref path) = element {
                    dispatchers
                        .entry(0)
                        .or_insert(Vec::new())
                        .push(path.path.clone())
                } else if let Expr::Array(ref arr) = element {
                    let core = i;
                    let a = arr
                        .elems
                        .iter()
                        .map(|element| {
                            if let Expr::Path(ref path) = element {
                                path.path.clone()
                            } else {
                                panic!("wrong syntax")
                            }
                        })
                        .collect::<Vec<Path>>();
                    dispatchers.insert(core as u32, a);
                }
            }
        }

        if !dispatchers.is_empty() && cores as usize != dispatchers.len() {
            return Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "The number of cores `{cores}` does not match the number of dispatchers `{}`",
                    dispatchers.len()
                ),
            ));
        }

        Ok(Self {
            dispatchers,
            pacs,
            cores,
        })
    }
}

#[derive(Debug)]
pub struct SoftwareTask {
    pub params: TaskParams,
    pub task_struct: ItemStruct,
    pub task_impl: Option<ItemImpl>,
}

impl SoftwareTask {
    pub fn name(&self) -> &Ident {
        &self.task_struct.ident
    }
}

#[derive(Debug)]
pub struct TaskParams {
    pub priority: u16,
    pub core: u32,
    pub spawn_by: u32,
}

impl TaskParams {
    pub fn from_attr(attr: &RticAttr) -> Self {
        let mut priority = 0;
        if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(int), ..
        })) = attr.elements.get("priority")
        {
            priority = int.base10_parse().unwrap_or_default();
        }

        let mut core = 0;
        if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(int), ..
        })) = attr.elements.get("core")
        {
            core = int.base10_parse().unwrap_or_default();
        }

        let mut spawn_by = core; // spawn_by is initially set to be the same core, unless the user chooses otherwize
        if let Some(Expr::Lit(syn::ExprLit {
            lit: Lit::Int(int), ..
        })) = attr.elements.get("spawn_by")
        {
            spawn_by = int.base10_parse().unwrap_or_default();
        }

        Self {
            priority,
            core,
            spawn_by,
        }
    }
}
