#![no_std]

#[cfg(feature = "async")]
extern crate alloc;

pub mod export;

pub use rticx_cortex_m_macro::app;
