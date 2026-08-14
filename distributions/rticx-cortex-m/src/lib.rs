#![no_std]

#[cfg(all(feature = "swtasks", feature = "async"))]
compile_error!(
    "rticx-cortex-m: the `swtasks` and `async` features are mutually exclusive; enable at most one"
);

pub mod export;

pub use rticx_cortex_m_macro::app;
