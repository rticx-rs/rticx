// Note: most of the code here is taken from rtic repo
#![allow(clippy::inline_always)]

/// Distribution crate must re-export the `export` module from all the used compilation passes
pub use rtic_sw_pass::export::*;

/// Exports required by core-pass
pub use hippomenes_core::Interrupt as InterruptNumber; // a trait that abstracts an interrupt type

/// re-exports needed from the code generation in internal rtic-macro crate
// use core::cell::Cell;
use hippomenes_core::mintthresh;

pub mod interrupts {
    pub use hippomenes_core::Interrupt0;
    pub use hippomenes_core::Interrupt1;
    pub use hippomenes_core::Interrupt2;
    pub use hippomenes_core::Interrupt3;

}
pub use hippomenes_core::Interrupt;
pub use hippomenes_core::{Peripherals, OutputPin};
pub use riscv::interrupt::machine::disable as interrupt_disable;
pub use riscv::interrupt::machine::enable as interrupt_enable;

// Newtype over `Cell` that forbids mutation through a shared reference
// pub struct Priority {
//     inner: Cell<u8>,
// }

// impl Priority {
//     #[inline(always)]
//     /// # Safety
//     /// We'll do that later, trust me
//     pub unsafe fn new(value: u8) -> Self {
//         Priority {
//             inner: Cell::new(value),
//         }
//     }

//     // these two methods are used by `lock` (see below) but can't be used from the RTIC application
//     #[inline(always)]
//     fn set(&self, value: u8) {
//         self.inner.set(value)
//     }

//     #[inline(always)]
//     fn get(&self) -> u8 {
//         self.inner.get()
//     }
// }
/// Lock implementation using threshold and global Critical Section (CS)
///
/// # Safety
///
/// The system ceiling is raised from current to ceiling
/// by either
/// - raising the threshold to the ceiling value, or
/// - disable all interrupts in case we want to
///   mask interrupts with maximum priority
///
/// Dereferencing a raw pointer inside CS
///
/// The priority.set/priority.get can safely be outside the CS
/// as being a context local cell (not affected by preemptions).
/// It is merely used in order to omit masking in case current
/// priority is current priority >= ceiling.
#[inline(always)]
pub unsafe fn lock<T, R>(ptr: *mut T, priority: u8, ceiling: u8, f: impl FnOnce(&mut T) -> R) -> R {
    if priority < ceiling {
        // priority.set(ceiling);

        mintthresh::Bits::write(ceiling as usize);
        let r = f(&mut *ptr);
        mintthresh::Bits::write(priority as usize);
        // priority.set(current);
        r
    } else {
        f(&mut *ptr)
    }
}


/// Sets the given software interrupt as pending
pub fn pend<T: Interrupt>(_int: T) {
    unsafe { <T as Interrupt>::pend_int() };
}

// Sets the given software interrupt as not pending
// pub fn unpend<T: Interrupt>(_int: T) {
//     unsafe { <T as Interrupt>::clear_int() };
// }

pub fn enable<T: Interrupt>(_int: T, prio: u8) {
    unsafe {
        <T as Interrupt>::set_priority(prio);
        <T as Interrupt>::enable_int();
    }
}
