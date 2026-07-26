#![no_std]

pub mod export;

pub use rticx_riscv_macro::app;

// Enforce that exactly one of the target selector features is enabled.
#[cfg(not(any(feature = "slic", feature = "esp32c3", feature = "esp32c6")))]
compile_error!(
    "rticx-riscv: no target feature selected. \
     Enable exactly one of: `slic`, `esp32c3`, `esp32c6`. \
     Example: `rticx-riscv = { version = \"0.1\", default-features = false, features = [\"esp32c3\"] }`"
);

#[cfg(all(feature = "slic", feature = "esp32c3"))]
compile_error!("rticx-riscv: the `slic` and `esp32c3` features are mutually exclusive");
#[cfg(all(feature = "slic", feature = "esp32c6"))]
compile_error!("rticx-riscv: the `slic` and `esp32c6` features are mutually exclusive");
#[cfg(all(feature = "esp32c3", feature = "esp32c6"))]
compile_error!("rticx-riscv: the `esp32c3` and `esp32c6` features are mutually exclusive");

#[cfg(all(
    feature = "slic",
    not(any(feature = "mecall-backend", feature = "clint-backend"))
))]
compile_error!(
    "rticx-riscv: either `mecall-backend` or `clint-backend` must be enabled when `slic` is enabled"
);

#[cfg(all(feature = "mecall-backend", feature = "clint-backend"))]
compile_error!(
    "rticx-riscv: the `mecall-backend` and `clint-backend` features are mutually exclusive"
);
