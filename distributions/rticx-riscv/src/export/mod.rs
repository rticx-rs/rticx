#![allow(clippy::inline_always)]
//! Re-exports required by the core pass and the software-tasks pass, and by the
//! backend trait bindings generated code.
//! The contents depend on the selected target backend (`slic` / `esp32c3` / `esp32c6`),
//! controlled by the parent crate's feature flags. The bindings below are adapted from the upstream RTIC RISC-V backends.

/// Re-export RTICX Single Producer Single Consumer queue to be used by sw and async passes
pub use rticx_spsc::Queue;

// Async runtime re-export (for async/await software tasks)
#[cfg(feature = "async")]
pub use rticx_async as async_rt;

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

/// Nesting-safe critical section.
///
/// Only the global interrupt enable (`mstatus.MIE`) is toggled and
/// restored to its previous value: the SLIC controller itself is left
/// untouched so that entering a critical section from an interrupt
/// handler (where interrupts are already disabled) does not corrupt the
/// controller state and does not spuriously re-enable interrupts on exit.
#[inline]
pub fn interrupt_free<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    riscv::interrupt::free(f)
}
