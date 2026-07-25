// Most of the locking logic here is adapted from the upstream RTIC cortex-m backend
// (`cortex_basepri.rs`, `cortex_source_mask.rs`, `cortex_common.rs`) and from the
// rp2040-rtic distribution in this repository.
#![allow(clippy::inline_always)]

/// Distribution crate must re-export the `export` module from all the used compilation passes
pub use rtic_sw_pass::export::*;

/// Exports required by the core pass and by generated code
pub use cortex_m::interrupt::InterruptNumber; // a trait that abstracts an interrupt type
pub use cortex_m::{
    Peripherals,
    asm::nop,
    asm::wfi,
    interrupt,
    peripheral::{DWT, NVIC, SCB, SYST, scb::SystemHandler},
};

#[inline]
#[must_use]
pub const fn cortex_logical2hw(logical: u8, nvic_prio_bits: u8) -> u8 {
    ((1 << nvic_prio_bits) - logical) << (8 - nvic_prio_bits)
}

/// Sets the given `interrupt` as pending
///
/// Convenience wrapper around [`NVIC::pend`](cortex_m::peripheral::struct.NVIC.html)
pub fn pend<I>(interrupt: I)
where
    I: InterruptNumber,
{
    NVIC::pend(interrupt);
}

// ============================================================================
// BASEPRI locking (armv7-m and above) — default path
// ============================================================================
#[cfg(not(feature = "armv6m"))]
pub use basepri::*;

#[cfg(not(feature = "armv6m"))]
mod basepri {
    use cortex_m::register::{basepri, basepri_max};

    /// Called around every task's `exec`. On armv7-m the BASEPRI register is
    /// raised to the task's priority by hardware on interrupt entry; this restores
    /// BASEPRI to its pre-handler value after the task runs.
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

    /// Lock implementation using BASEPRI and a global critical section (CS).
    ///
    /// # Safety
    ///
    /// The system ceiling is raised from the current to `ceiling` by either
    /// - raising BASEPRI to the ceiling value, or
    /// - disabling all interrupts if we want to mask interrupts with maximum
    ///   priority.
    ///
    /// Dereferencing a raw pointer inside the CS.
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
                basepri_max::write(super::cortex_logical2hw(ceiling, nvic_prio_bits));
                let r = f(&mut *ptr);
                basepri::write(current);
                r
            }
        }
    }
}

// ============================================================================
// Interrupt source masking (armv6-m: Cortex-M0/M0+/M23)
// ============================================================================
#[cfg(feature = "armv6m")]
pub use source_mask::*;

#[cfg(feature = "armv6m")]
mod source_mask {
    /// Mask is used to store interrupt masks on systems without a BASEPRI register
    /// (M0, M0+, M23). It needs to be large enough to cover all the relevant
    /// interrupts in use. For M0/M0+ there are only 32 interrupts so we only need
    /// one u32 value. For M23 there can be as many as 480 interrupts; rather than
    /// allocating space for all of them, we detect the highest interrupt in use at
    /// compile time and allocate enough u32 chunks to cover it.
    #[derive(Copy, Clone)]
    pub struct Mask<const M: usize>([u32; M]);

    impl<const M: usize> core::ops::BitOrAssign for Mask<M> {
        fn bitor_assign(&mut self, rhs: Self) {
            for i in 0..M {
                self.0[i] |= rhs.0[i];
            }
        }
    }

    impl<const M: usize> Mask<M> {
        /// Set a bit inside a Mask.
        const fn set_bit(mut self, bit: u32) -> Self {
            let block = bit / 32;

            if block as usize >= M {
                panic!(
                    "Generating masks for thumbv6/thumbv8m.base failed! Are you compiling for thumbv6 on an thumbv7 MCU or using an unsupported thumbv8m.base MCU?"
                );
            }

            let offset = bit - (block * 32);
            self.0[block as usize] |= 1 << offset;
            self
        }
    }

    pub const fn create_mask<const N: usize, const M: usize>(list_of_shifts: [u32; N]) -> Mask<M> {
        let mut mask = Mask([0; M]);
        let mut i = 0;

        while i < N {
            let shift = list_of_shifts[i];
            i += 1;
            mask = mask.set_bit(shift);
        }

        mask
    }

    /// Compute the number of u32 chunks needed to store the Mask value.
    /// On M0, M0+ this should always end up being 1.
    pub const fn compute_mask_chunks<const L: usize>(ids: [u32; L]) -> usize {
        let mut max: usize = 0;
        let mut i = 0;

        while i < L {
            let id = ids[i] as usize;
            i += 1;

            if id > max {
                max = id;
            }
        }
        (max + 32) / 32
    }

    /// Lock implementation using interrupt masking.
    ///
    /// # Safety
    ///
    /// The system ceiling is raised from current to `ceiling` by computing a
    /// 32 bit `mask` (1 bit per interrupt): 1 where `ceiling >= priority > current`,
    /// 0 elsewhere.
    ///
    /// On CS entry `clear_enable_mask(mask)` disables interrupts; on CS exit
    /// `set_enable_mask(mask)` re-enables them.
    ///
    /// Dereferencing a raw pointer is done safely inside the CS.
    #[inline(always)]
    pub unsafe fn lock<T, R, const M: usize>(
        ptr: *mut T,
        priority: u16,
        ceiling: u16,
        masks: &[Mask<M>; 3],
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        let current = priority;
        if current < ceiling {
            if ceiling >= 4 {
                // Safe to manipulate outside critical section; execute closure under
                // protection of the raised system ceiling via a global critical section.
                cortex_m::interrupt::free(|_| f(unsafe { &mut *ptr }))
            } else {
                let mask = compute_mask(current as u8, ceiling as u8, masks);
                unsafe { clear_enable_mask(mask) };
                // execute closure under protection of raised system ceiling
                let r = f(unsafe { &mut *ptr });
                unsafe { set_enable_mask(mask) };
                r
            }
        } else {
            // execute closure without raising system ceiling
            f(unsafe { &mut *ptr })
        }
    }

    #[inline(always)]
    fn compute_mask<const M: usize>(from_prio: u8, to_prio: u8, masks: &[Mask<M>; 3]) -> Mask<M> {
        let mut res = Mask([0; M]);
        masks[from_prio as usize..to_prio as usize]
            .iter()
            .for_each(|m| res |= *m);
        res
    }

    // enables interrupts
    #[inline(always)]
    unsafe fn set_enable_mask<const M: usize>(mask: Mask<M>) {
        for i in 0..M {
            // This check should involve compile time constants and be optimized out.
            if mask.0[i] != 0 {
                unsafe { (*cortex_m::peripheral::NVIC::PTR).iser[i].write(mask.0[i]) };
            }
        }
    }

    // disables interrupts
    #[inline(always)]
    unsafe fn clear_enable_mask<const M: usize>(mask: Mask<M>) {
        for i in 0..M {
            // This check should involve compile time constants and be optimized out.
            if mask.0[i] != 0 {
                unsafe { (*cortex_m::peripheral::NVIC::PTR).icer[i].write(mask.0[i]) };
            }
        }
    }
}
