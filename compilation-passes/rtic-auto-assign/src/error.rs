use proc_macro2::Span;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    // #[error("No `cores=N` argument was found in the rtic application attribute arguments.")]
    // NoCores,
    #[error("`core=M` has to be explicitly assinged in the struct {0} with #[shared] attribute.")]
    NoCoreArgShared(String),
    #[error("A core needs to be explicitly assigned to {0} task since it uses no shared resources that allow automatic core assingment.")]
    ExplicitCoreNeeded(String),
    #[error("The resource name `{0}` was found on multiple structs with #[shared] attribute, but resource names must be unique.")]
    DuplicatResourceName(String),
    #[error("The resource `{0}` was not found in any of the structs with #[shared] attribute.")]
    ResourceNotFound(String),
    #[error("The task `{0}` is only allowed to use resources from core {1}.")]
    CoreMimatch(String, u32),
}

impl From<Error> for syn::Error {
    fn from(value: Error) -> Self {
        syn::Error::new(Span::call_site(), value)
    }
}
