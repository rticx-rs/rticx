#![allow(clippy::inline_always)]
//! Re-exports required by the core pass and the software-tasks pass, and by the
//! backend trait bindings generated code.
//! The contents depend on the selected target backend (`slic` / `esp32c3` / `esp32c6`),
//! controlled by the parent crate's feature flags. The bindings below are adapted from the upstream RTIC RISC-V
//! backends under `upstream/exports/` of this repository.
// Distribution crate must re-export the `export` module from all the used
// compilation passes. This brings in `rticx_spsc::Queue` used for ready
// queues and task inputs by the software-tasks pass.
pub use rticx_sw_pass::export::*;

// ============================================================================
// Generic SLIC exports
// ============================================================================
#[cfg(feature = "slic")]
pub use slic_export::*;

#[cfg(feature = "slic")]
mod slic_export;

// ============================================================================
// ESP32-C3 exports
// ============================================================================
#[cfg(feature = "esp32c3")]
pub use esp32c3_export::*;

#[cfg(feature = "esp32c3")]
#[allow(clippy::module_inception)]
mod esp32c3_export;

// ============================================================================
// ESP32-C6 exports
// ============================================================================
#[cfg(feature = "esp32c6")]
pub use esp32c6_export::*;

#[cfg(feature = "esp32c6")]
#[allow(clippy::module_inception)]
mod esp32c6_export;
