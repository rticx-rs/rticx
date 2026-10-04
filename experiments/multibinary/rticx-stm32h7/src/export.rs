//! Items the core pass, the software-tasks pass and the generated cross-binary
//! code refer to by path.
//!
//! The locking logic is adapted from the upstream RTIC Cortex-M backend and
//! from the in-repo `rticx-cortex-m` distribution: the Cortex-M7 and the
//! Cortex-M4 are both ARMv7-M, so the BASEPRI path applies to either.
#![allow(clippy::inline_always)]

/// Re-export the RTICX single-producer/single-consumer queue used by the
/// software-tasks pass for core-local spawn queues.
pub use rticx_spsc::Queue;

/// Re-export the cross-binary runtime crate: the generated code reaches
/// `Fifo`, `Queue` and the FIFO layout constants through
/// `rticx_stm32h7::export::xbin_rt`.
pub use rticx_xbin_rt as xbin_rt;

/// Trait abstracting over interrupt numbers.
pub use cortex_m::interrupt::InterruptNumber;
pub use cortex_m::{
    Peripherals,
    asm::nop,
    asm::wfi,
    interrupt,
    peripheral::{DWT, NVIC, SCB, SYST, scb::SystemHandler},
    register::msp,
};

/// Converts a *logical* priority (RTIC's, `0` = highest) to the hardware
/// BASEPRI/NVIC encoding.
#[inline]
#[must_use]
pub const fn cortex_logical2hw(logical: u8, nvic_prio_bits: u8) -> u8 {
    ((1 << nvic_prio_bits) - logical) << (8 - nvic_prio_bits)
}

/// Sets the given `interrupt` as pending.
pub fn pend<I>(interrupt: I)
where
    I: InterruptNumber,
{
    NVIC::pend(interrupt);
}

use cortex_m::register::{basepri, basepri_max};

/// Restores BASEPRI around a task's `exec`.
///
/// On ARMv7-M the BASEPRI register is raised to the task's priority by
/// hardware on interrupt entry; this restores it to its pre-handler value after
/// the task runs.
#[inline(always)]
pub fn run<F>(priority: u8, f: F)
where
    F: FnOnce(),
{
    if priority == 1 {
        // If the priority of this interrupt is `1` then BASEPRI can only be `0`.
        f();
        unsafe { basepri::write(0) }
    } else {
        let initial = basepri::read();
        f();
        unsafe { basepri::write(initial) }
    }
}

/// Raises the system ceiling to `ceiling` around `f` (BASEPRI SRP lock).
///
/// # Safety
///
/// The caller must pass a `ptr` valid for the duration of `f` and a `ceiling`
/// no higher than the priority of the current task.
#[inline(always)]
pub unsafe fn lock<T, R>(
    ptr: *mut T,
    ceiling: u8,
    nvic_prio_bits: u8,
    f: impl FnOnce(&mut T) -> R,
) -> R {
    unsafe {
        if ceiling == (1 << nvic_prio_bits) {
            cortex_m::interrupt::free(|_| f(&mut *ptr))
        } else {
            let current = basepri::read();
            basepri_max::write(cortex_logical2hw(ceiling, nvic_prio_bits));
            let r = f(&mut *ptr);
            basepri::write(current);
            r
        }
    }
}
