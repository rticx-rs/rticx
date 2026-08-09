#![no_std]

#[cfg(feature = "asynctasks")]
extern crate alloc;

pub mod export;

pub use rticx_cortex_m_macro::app;
