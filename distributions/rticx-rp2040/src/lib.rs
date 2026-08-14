#![no_std]

#[cfg(all(feature = "swtasks", feature = "async"))]
compile_error!(
    "rticx-rp2040: the `swtasks` and `async` features are mutually exclusive; enable at most one"
);

pub mod export;

pub use rticx_rp2040_macro::app;
