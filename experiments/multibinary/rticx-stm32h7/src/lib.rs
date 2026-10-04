#![no_std]

//! RTICX distribution for the dual-core STM32H7 (Cortex-M7 + Cortex-M4).
//!
//! One binary per core, each selecting its core with exactly one of the `cm7`
//! / `cm4` features. The distribution binds the cross-binary compilation pass
//! in its `#[app]` macro and provides the H7 hardware support the generated
//! code calls into (core id, MPU configuration of the shared pool and the HSEM
//! doorbell transport) in [`xbin`]; see `README.md`.

#[cfg(all(feature = "cm7", feature = "cm4"))]
compile_error!(
    "rticx-stm32h7: the `cm7` and `cm4` features select different physical cores and are \
     mutually exclusive; enable exactly one"
);

#[cfg(not(any(feature = "cm7", feature = "cm4")))]
compile_error!(
    "rticx-stm32h7: enable exactly one of the `cm7` and `cm4` features to select the physical \
     core this binary runs on"
);

pub mod export;
pub mod xbin;

pub use rticx_stm32h7_macro::app;
