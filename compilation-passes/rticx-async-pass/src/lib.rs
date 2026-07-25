#![cfg_attr(not(feature = "proc-macro"), no_std)]

#[cfg(feature = "proc-macro")]
pub mod async_pass;

/// To be re-exported by distributor crate
pub mod export;

#[cfg(feature = "proc-macro")]
pub use async_pass::*;
