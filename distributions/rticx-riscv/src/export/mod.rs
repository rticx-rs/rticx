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
/// Read the stack pointer.
#[inline(always)]
pub fn read_sp() -> u32 {
    let r;
    unsafe { core::arch::asm!("mv {}, sp", out(reg) r, options(nomem, nostack, preserves_flags)) };
    r
}

/// Startup stack-overflow check.
///
/// Compares the current stack pointer against the end of the `.bss` section
/// (the lowest address the stack may legally grow into) and panics if the
/// stack has already overflowed into `.bss`. The linker symbol naming the end
/// of `.bss` differs between the supported targets (`riscv-rt` for SLIC, the
/// `esp-hal` generated link script for the Espressif targets).
#[inline(never)]
pub fn check_stack_overflow() {
    unsafe extern "C" {
        static _stack_start: u32;
        #[cfg(feature = "slic")]
        static _ebss: u32;
        #[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
        static _bss_end: u32;
    }

    let stack_start = unsafe { &_stack_start as *const _ as u32 };
    #[cfg(feature = "slic")]
    let bss_end = unsafe { &_ebss as *const _ as u32 };
    #[cfg(any(feature = "esp32c3", feature = "esp32c6"))]
    let bss_end = unsafe { &_bss_end as *const _ as u32 };

    if stack_start > bss_end {
        // No flip-link usage, check the SP for overflow.
        if read_sp() <= bss_end {
            ::core::panic!("Stack overflow after allocating executors");
        }
    }
}
