// Enable the `no_std` attribute if `no_std` is enabled
#![cfg_attr(not(feature = "proc-macro"), no_std)]

#[cfg(feature = "proc-macro")]
pub mod software_pass;

/// To be re-exported by distributor crate
pub mod export;

#[cfg(feature = "proc-macro")]
pub use software_pass::*;
