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
