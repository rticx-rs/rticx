use std::sync::atomic::Ordering;

use heck::ToSnakeCase;
use proc_macro2::Span;
use quote::{format_ident, ToTokens};
use syn::{
    parse::Parser, parse_quote, spanned::Spanned, Expr, ExprArray, ExprLit, Ident, ItemFn,
    ItemImpl, ItemStruct, Lit, LitInt, Meta,
};

use crate::{
    errors::ParseError, parse_utils::RticAttr, rtic_traits::HWT_TRAIT_TY, DEFAULT_TASK_PRIORITY,
};

#[derive(Debug)]
pub struct InitTask {
    pub args: InitTaskArgs,
    pub ident: Ident,
    pub body: ItemFn,
}

#[derive(Debug, Clone, Default)]
pub struct InitTaskArgs {
    pub core: u32,
}

impl InitTaskArgs {
    pub fn parse(args: Meta) -> syn::Result<Self> {
        let mut core: Option<syn::LitInt> = None;
        let Meta::List(args) = args else {
            return Ok(Self::default());
        };

        syn::meta::parser(|meta| {
            if meta.path.is_ident("core") {
                core = Some(meta.value()?.parse()?)
            } else {
                // this is needed to advance the values iterator
                let _ = meta.value()?.parse::<Expr>();
            }
            Ok(())
        })
        .parse2(args.tokens)?;

        let core = core
            .and_then(|core| core.base10_parse().ok())
            .unwrap_or_default();

        Ok(Self { core })
    }
}

#[derive(Debug)]
pub struct TaskArgs {
    /// Interrupt handler name
    pub binds: Option<syn::Ident>,
    pub priority: u16,
    /// Shared resources, stored as a list of [identifiers](`proc_macro2::Ident`)
    pub shared: Vec<Ident>,
    pub core: u32,
    // tells whether a task is native to this compilation pass or if another compilation pass handles its trait implementation
    pub task_trait: Ident,
}

impl TaskArgs {
    pub fn parse(args: Meta) -> syn::Result<Self> {
        let Meta::List(args) = args else {
            return Ok(TaskArgs {
                binds: None,
                priority: DEFAULT_TASK_PRIORITY.load(Ordering::Relaxed),
                shared: Default::default(),
                core: 0,
                task_trait: format_ident!("{HWT_TRAIT_TY}"),
            });
        };

        let mut binds: Option<syn::Path> = None;
        let mut task_trait: Option<Ident> = None;
        let mut priority: Option<LitInt> = None;
        let mut shared_res: Option<ExprArray> = None;
        let mut core: Option<LitInt> = None;

        syn::meta::parser(|meta| {
            if meta.path.is_ident("binds") {
                binds = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("priority") {
                priority = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("shared") {
                shared_res = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("core") {
                core = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("task_trait") {
                task_trait = Some(meta.value()?.parse()?);
            } else {
                // this is needed to advance the values iterator
                let _: syn::Result<Expr> = meta
                    .value()
                    // Try parsing the assignment operator. On failure, set value = ().
                    .map(|v| v.parse())
                    .unwrap_or_else(|_| Ok(parse_quote!(())));
            }
            Ok(())
        })
        .parse2(args.tokens.clone())
        .inspect_err(|_e| {
            eprintln!(
                "An error occurred while parsing: {:?}",
                args.tokens.to_string()
            );
        })?;

        let binds = binds.map(|i| Ident::new(&i.to_token_stream().to_string(), Span::call_site()));

        let priority = priority
            .and_then(|p| p.base10_parse().ok())
            .unwrap_or(DEFAULT_TASK_PRIORITY.load(Ordering::Relaxed));

        let core = core
            .and_then(|core| core.base10_parse().ok())
            .unwrap_or_default();
        let task_trait = task_trait.unwrap_or(format_ident!("{HWT_TRAIT_TY}"));

        let shared_res = shared_res
            .map(|expr| {
                expr.elems
                    .into_iter()
                    .map(|elem| Ident::new(&elem.to_token_stream().to_string(), Span::call_site()))
                    .collect()
            })
            .unwrap_or_else(|| Vec::new());

        Ok(Self {
            binds,
            priority,
            shared: shared_res,
            core,
            task_trait,
        })
    }
}

/// Alias for hardware task
pub type HardwareTask = RticTask;

/// Alias for idle tasks. idle task has `interrupt_handler_name` set to None and priority 0
pub type IdleTask = RticTask;

#[derive(Debug)]
pub struct RticTask {
    pub args: TaskArgs,
    pub task_struct: ItemStruct,
    pub struct_impl: Option<ItemImpl>,
    pub user_initializable: bool, // whether user should manually initialize this task during init
}

impl RticTask {
    pub fn name(&self) -> &Ident {
        &self.task_struct.ident
    }

    /// By convention, this method is used to generate the name of the static task instance
    pub fn name_uppercase(&self) -> Ident {
        let name = self
            .task_struct
            .ident
            .to_string()
            .to_snake_case()
            .to_uppercase();
        Ident::new(&name, Span::call_site())
    }

    pub fn name_snakecase(&self) -> Ident {
        let name = self.task_struct.ident.to_string().to_snake_case();
        Ident::new(&name, Span::call_site())
    }
}

#[derive(Debug, Clone)]
pub struct SharedElement {
    pub ident: Ident,
    pub ty: syn::Type,
    pub priority: u16,
}

#[derive(Debug, Clone, Default)]
pub struct SharedResourcesArgs {
    pub core: u32,
}

impl SharedResourcesArgs {
    pub fn parse(args: Meta) -> syn::Result<Self> {
        let mut core: Option<syn::LitInt> = None;
        let Meta::List(args) = args else {
            return Ok(Self::default());
        };

        syn::meta::parser(|meta| {
            if meta.path.is_ident("core") {
                core = Some(meta.value()?.parse()?)
            } else {
                // this is needed to advance the values iterator
                let _ = meta.value()?.parse::<Expr>();
            }
            Ok(())
        })
        .parse2(args.tokens)?;

        let core = core
            .and_then(|core| core.base10_parse().ok())
            .unwrap_or_default();

        Ok(Self { core })
    }
}

#[derive(Debug, Clone)]
pub struct SharedResources {
    pub args: SharedResourcesArgs,
    pub strct: ItemStruct,
    pub resources: Vec<SharedElement>,
}

impl SharedResources {
    pub fn get_field_mut(&mut self, field_name: &Ident) -> Option<&mut SharedElement> {
        self.resources
            .iter_mut()
            .find(|field| &field.ident == field_name)
    }

    pub fn get_field(&self, field_name: &Ident) -> Option<&SharedElement> {
        self.resources
            .iter()
            .find(|field| &field.ident == field_name)
    }
    pub fn name_uppercase(&self) -> Ident {
        let name = self.strct.ident.to_string().to_snake_case().to_uppercase();
        Ident::new(&name, Span::call_site())
    }
}

/// Arguments provided to the #[app(...)] macro attribute, this includes paths to PACs, number of cores, and peripherals option.
#[derive(Debug)]
pub struct AppArgs {
    // path to peripheral crate
    pub pacs: Vec<syn::Path>,
    pub peripherals: bool,
    pub cores: u32,
}

impl AppArgs {
    pub fn parse(args: proc_macro2::TokenStream) -> syn::Result<Self> {
        let args_span = args.span();

        let mut args = RticAttr::parse_from_tokens(args.clone())?;

        // parse the number of cores
        let cores = args.elements.remove("cores");
        let cores = match cores {
            Some(Expr::Lit(ExprLit {
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

        Ok(Self {
            pacs,
            peripherals: false, // TODO: not supported yet
            cores,
        })
    }
}
