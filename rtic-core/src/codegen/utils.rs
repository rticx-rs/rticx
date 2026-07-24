use quote::format_ident;
use syn::Ident;

pub fn core_type(core: u32) -> Ident {
    format_ident!("__rtic__internal__Core{core}")
}

pub fn core_type_inner(core: u32) -> Ident {
    format_ident!("__rtic__internal__Core{core}Inner")
}

pub fn core_type_mod(core: u32) -> Ident {
    format_ident!("core{core}_type_mod")
}

#[allow(unused)]
pub mod multibin {
    use proc_macro2::TokenStream;
    use syn::{Attribute, parse_quote};

    /// If `multibin` feature is enabled, this returns a tokenstream for the attribute `#[cfg(core = "x")]` to partition an application
    /// to multiple binaries. Otherwise `None` is returned
    pub fn multibin_cfg_core(core: u32) -> Option<Attribute> {
        #[cfg(feature = "multibin")]
        {
            let val = core.to_string();
            Some(parse_quote! {
                #[cfg(core = #val)]
            })
        }
        #[cfg(not(feature = "multibin"))]
        None
    }

    /// If `multibin` feature is enabled, this returns a tokenstream for the attribute `#[cfg(not(core = "x"))]`
    /// Otherwise `None` is returned
    pub fn multibin_cfg_not_core(core: u32) -> Option<Attribute> {
        #[cfg(feature = "multibin")]
        {
            let val = core.to_string();
            Some(parse_quote! {
                #[cfg(not(core = #val))]
            })
        }
        #[cfg(not(feature = "multibin"))]
        None
    }

    /// If `multibin` feature is enabled, this returns a tokenstream for the attribute `#[cfg_attr(not(core = "x"), your_attr)]`
    /// Otherwise `None` is returned
    pub fn multibin_cfg_attr_not_core(core: u32, attr: TokenStream) -> Option<Attribute> {
        #[cfg(feature = "multibin")]
        {
            let val = core.to_string();
            Some(parse_quote! {
                #[cfg_attr(not(core = #val), #attr)]
            })
        }
        #[cfg(not(feature = "multibin"))]
        None
    }

    /// If `multibin` feature is enabled, this returns a tokenstream for the attribute `#[multibin_shared]` to make sure the annotated variable
    /// is present at the same address on all cores. Otherwise `None` is returned
    pub fn multibin_shared() -> Option<Attribute> {
        #[cfg(feature = "multibin")]
        {
            Some(parse_quote! {
                #[multibin_shared]
            })
        }
        #[cfg(not(feature = "multibin"))]
        None
    }
}
