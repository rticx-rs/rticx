// Enable the `no_std` attribute if `no_std` is enabled
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
pub mod deadline_pass;

#[cfg(feature = "std")]
pub use deadline_pass::*;
