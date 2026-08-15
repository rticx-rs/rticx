use heck::ToSnakeCase;
use proc_macro2::Span;
use quote::format_ident;
use syn::{Expr, Ident, ItemFn, ItemImpl, ItemStruct, Meta, spanned::Spanned};

use crate::{errors::ParseError, parse_utils::RticAttr, rticx_traits::HWT_TRAIT_TY};

/// Default task priority when the `priority = N` attribute is omitted.
///
/// Priorities follow upstream RTIC semantics: `0` is the disabled/idle task
/// priority, and `1` onwards denote increasing urgency.
const DEFAULT_TASK_PRIORITY: u16 = 1;

#[derive(Debug, Clone)]
pub struct InitTask {
    pub args: InitTaskArgs,
    pub ident: Ident,
    pub body: ItemFn,
}

#[derive(Debug, Clone)]
pub struct PostInitTask {
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
        let mut attr = RticAttr::from_meta(&args)?;
        attr.ensure_supported(&["core"])?;
        let core = attr.take_u32("core")?.unwrap_or_default();
        Ok(Self { core })
    }
}

#[derive(Debug, Clone)]
pub struct TaskArgs {
    /// Interrupt handler name
    pub binds: Option<syn::Ident>,
    pub priority: u16,
    /// Shared resources, stored as a list of [identifiers](`proc_macro2::Ident`)
    pub shared: Vec<Ident>,
    pub core: u32,
    // tells whether a task is native to this compilation pass or if another compilation pass handles its trait implementation
    pub task_trait: Ident,
    /// Whether the task is constructed by the framework at boot (internal
    /// `init = generated` marker). Generated tasks are excluded from
    /// `TaskInits` and are written into their static as a unit literal.
    pub init_generated: bool,
}

impl TaskArgs {
    pub fn parse(args: Meta) -> syn::Result<Self> {
        let mut attr = RticAttr::from_meta(&args)?;
        attr.ensure_supported(&["binds", "priority", "shared", "core", "task_trait", "init"])?;

        let binds = attr.take_ident("binds")?;
        let priority = attr.take_u16("priority")?.unwrap_or(DEFAULT_TASK_PRIORITY);
        let shared = attr.take_ident_array("shared")?.unwrap_or_default();
        let core = attr.take_u32("core")?.unwrap_or_default();
        let task_trait = attr
            .take_ident("task_trait")?
            .unwrap_or_else(|| format_ident!("{HWT_TRAIT_TY}"));
        let init_generated = match attr.take_expr("init") {
            None => false,
            Some(Expr::Path(p)) if p.path.is_ident("generated") => true,
            Some(other) => {
                return Err(syn::Error::new(other.span(), "expected `init = generated`"));
            }
        };

        Ok(Self {
            binds,
            priority,
            shared,
            core,
            task_trait,
            init_generated,
        })
    }
}

/// Alias for hardware task
pub type HardwareTask = RticTask;

/// Alias for idle tasks. idle task has `interrupt_handler_name` set to None and priority 0
pub type IdleTask = RticTask;

#[derive(Debug, Clone)]
pub struct RticTask {
    pub args: TaskArgs,
    pub task_struct: ItemStruct,
    pub struct_impl: Option<ItemImpl>,
}

impl RticTask {
    pub fn name(&self) -> &Ident {
        &self.task_struct.ident
    }

    /// By convention, this method is used to generate the name of the static task instance
    pub fn name_uppercase(&self) -> Ident {
        uppercase_ident(&self.task_struct.ident)
    }

    pub fn name_snakecase(&self) -> Ident {
        snakecase_ident(&self.task_struct.ident)
    }
}

/// By convention, used to generate the name of a static task/resource instance.
pub fn uppercase_ident(ident: &Ident) -> Ident {
    let name = ident.to_string().to_snake_case().to_uppercase();
    Ident::new(&name, Span::call_site())
}

/// By convention, used to generate snake-cased names (e.g. struct fields).
pub fn snakecase_ident(ident: &Ident) -> Ident {
    let name = ident.to_string().to_snake_case();
    Ident::new(&name, Span::call_site())
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
        let mut attr = RticAttr::from_meta(&args)?;
        attr.ensure_supported(&["core"])?;
        let core = attr.take_u32("core")?.unwrap_or_default();
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
        uppercase_ident(&self.strct.ident)
    }
}

/// Arguments provided to the #[app(...)] macro attribute, this includes paths to PACs, number of cores, and peripherals option.
#[derive(Debug, Clone)]
pub struct AppArgs {
    // path to peripheral crate
    pub pacs: Vec<syn::Path>,
    pub cores: u32,
    /// Warning items generated for unsupported arguments, to be emitted by codegen.
    pub warnings: Vec<proc_macro2::TokenStream>,
}

impl AppArgs {
    pub fn parse(args: proc_macro2::TokenStream) -> syn::Result<Self> {
        let args_span = args.span();

        let mut attr = RticAttr::parse_from_tokens(args.clone(), format_ident!("app"))?;

        // parse the number of cores
        let cores = attr.take_u32("cores")?.unwrap_or(1);

        // parse the path(s) to PAC(s)
        let device = attr
            .take_expr("device")
            .ok_or(ParseError::DeviceArg.to_syn(args_span))?;

        let pacs = match device {
            Expr::Array(array_exp) => {
                if array_exp.elems.len() != cores as usize {
                    return Err(ParseError::DevicesCoresMismatch.to_syn(args_span));
                }

                array_exp
                    .elems
                    .into_iter()
                    .map(|exp| match exp {
                        Expr::Path(p) if p.qself.is_none() => Ok(p.path),
                        other => Err(syn::Error::new(
                            other.span(),
                            "each element of `device` must be a path to a PAC crate",
                        )),
                    })
                    .collect::<syn::Result<Vec<_>>>()?
            }
            Expr::Path(path_to_pac) if path_to_pac.qself.is_none() => {
                vec![path_to_pac.path; cores as usize]
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "`device` must be a path or an array of paths to PAC crates",
                ));
            }
        };

        // warn about unsupported arguments instead of failing
        let warnings = attr.unsupported_warnings(&["device", "cores"]);

        Ok(Self {
            pacs,
            cores,
            warnings,
        })
    }
}
