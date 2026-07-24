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
