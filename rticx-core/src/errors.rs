#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("`device = [ path::to:pac ]` argument must be provided.")]
    DeviceArg,

    #[error(
        "The number of elements provided to the `device` argument doesn't match the number of cores."
    )]
    DevicesCoresMismatch,

    #[error("The value passed to the `device` argument must be a path to a PAC crate.")]
    DeviceNotPath,
}
impl ParseError {
    pub fn to_syn(&self, span: proc_macro2::Span) -> syn::Error {
        syn::Error::new(span, self)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Can't publish '{0}' entry to info bus. Entry already exists")]
    EntryOccupied(String),
    #[error("Could not find '{0}' entry in info bus")]
    EntryNotFound(String),
    #[error("Type mismatch for entry '{0}': expected `{1}`, found `{2}`")]
    InvalidTargetType(String, String, String),
}
