// Enable the `no_std` attribute if `no_std` is enabled
#![cfg_attr(not(feature = "proc-macro"), no_std)]

#[cfg(feature = "proc-macro")]
pub mod deadline_pass;

#[cfg(feature = "proc-macro")]
pub use deadline_pass::*;
