// Note: most of the code here is taken from rtic repo
#![allow(clippy::inline_always)]

/// Distribution crate must re-export the `export` module from all the used compilation passes
pub use rtic_sw_pass::export::*;

/// Exports required by core-pass
pub use cortex_m::interrupt::InterruptNumber as AbstractInterrupt; // a trait that abstracts an interrupt type

/// re-exports needed from the code generation in internal stm32-rtic-macro crate
use crate::mailbox;
use cortex_m::register::{basepri, basepri_max};
pub use cortex_m::{
    asm::nop,
    asm::wfi,
    interrupt,
    peripheral::{scb::SystemHandler, DWT, NVIC, SCB, SYST},
    Peripherals,
};
pub use mailbox::cross_core;
pub use microamp;


#[inline]
#[must_use]
pub const fn cortex_logical2hw(logical: u8, nvic_prio_bits: u8) -> u8 {
    ((1 << nvic_prio_bits) - logical) << (8 - nvic_prio_bits)
}

// TODO: need to think how to abstract this
#[inline(always)]
pub fn run<F>(priority: u8, f: F)
where
    F: FnOnce(),
{
    if priority == 1 {
        // If the priority of this interrupt is `1` then BASEPRI can only be `0`
        f();
        unsafe { basepri::write(0) }
    } else {
        let initial = basepri::read();
        f();
        unsafe { basepri::write(initial) }
    }
}

/// Lock implementation using BASEPRI and global Critical Section (CS)
///
/// # Safety
///
/// The system ceiling is raised from current to ceiling
/// by either
/// - raising the BASEPRI to the ceiling value, or
/// - disable all interrupts in case we want to
///   mask interrupts with maximum priority
///
/// Dereferencing a raw pointer inside CS
///
/// The priority.set/priority.get can safely be outside the CS
/// as being a context local cell (not affected by preemptions).
/// It is merely used in order to omit masking in case current
/// priority is current priority >= ceiling.
///
/// Lock Efficiency:
/// Experiments validate (sub)-zero cost for CS implementation
/// (Sub)-zero as:
/// - Either zero OH (lock optimized out), or
/// - Amounting to an optimal assembly implementation
///   - The BASEPRI value is folded to a constant at compile time
///   - CS entry, single assembly instruction to write BASEPRI
///   - CS exit, single assembly instruction to write BASEPRI
///   - priority.set/get optimized out (their effect not)
/// - On par or better than any handwritten implementation of SRP
///
/// Limitations:
/// The current implementation reads/writes BASEPRI once
/// even in some edge cases where this may be omitted.
/// Total OH of per task is max 2 clock cycles, negligible in practice
/// but can in theory be fixed.
///
///
#[inline(always)]
pub unsafe fn lock<T, R>(
    ptr: *mut T,
    ceiling: u8,
    nvic_prio_bits: u8,
    f: impl FnOnce(&mut T) -> R,
) -> R {
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
