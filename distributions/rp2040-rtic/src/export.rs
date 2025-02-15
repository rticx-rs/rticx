// Note: most of the code here is taken from rtic repo
#![allow(clippy::inline_always)]

/// Distribution crate must re-export the `export` module from all the used compilation passes
pub use rtic_sw_pass::export::*;

/// Exports required by core-pass
pub use cortex_m::interrupt::InterruptNumber as AbstractInterrupt; // a trait that abstracts an interrupt type

/// re-exports needed from the code generation in internal rp2040-rtic-macro crate
pub use rp2040_hal::multicore::{Multicore, Stack};
pub use rp2040_hal::sio::Sio;
pub use cortex_m::{
    asm::nop,
    asm::wfi,
    interrupt,
    peripheral::{scb::SystemHandler, DWT, NVIC, SCB, SYST},
    Peripherals,
};

/// Mask is used to store interrupt masks on systems without a BASEPRI register (M0, M0+, M23).
/// It needs to be large enough to cover all the relevant interrupts in use.
/// For M0/M0+ there are only 32 interrupts so we only need one u32 value.
/// For M23 there can be as many as 480 interrupts.
/// Rather than providing space for all possible interrupts, we just detect the highest interrupt in
/// use at compile time and allocate enough u32 chunks to cover them.
#[derive(Copy, Clone)]
pub struct Mask<const M: usize>([u32; M]);

impl<const M: usize> core::ops::BitOrAssign for Mask<M> {
    fn bitor_assign(&mut self, rhs: Self) {
        for i in 0..M {
            self.0[i] |= rhs.0[i];
        }
    }
}

#[cfg(not(have_basepri))]
impl<const M: usize> Mask<M> {
    /// Set a bit inside a Mask.
    const fn set_bit(mut self, bit: u32) -> Self {
        let block = bit / 32;

        if block as usize >= M {
            panic!("Generating masks for thumbv6/thumbv8m.base failed! Are you compiling for thumbv6 on an thumbv7 MCU or using an unsupported thumbv8m.base MCU?");
        }

        let offset = bit - (block * 32);
        self.0[block as usize] |= 1 << offset;
        self
    }
}
#[cfg(not(have_basepri))]
#[inline(always)]
pub fn run<F>(_priority: u8, f: F)
where
    F: FnOnce(),
{
    f();
}

/// Const helper to check architecture
pub const fn have_basepri() -> bool {
    #[cfg(have_basepri)]
    {
        true
    }

    #[cfg(not(have_basepri))]
    {
        false
    }
}

/// Lock implementation using interrupt masking
///
/// # Safety
///
/// The system ceiling is raised from current to ceiling
/// by computing a 32 bit `mask` (1 bit per interrupt)
/// 1: ceiling >= priority > current
/// 0: else
///
/// On CS entry, `clear_enable_mask(mask)` disables interrupts
/// On CS exit,  `set_enable_mask(mask)` re-enables interrupts
///
/// The priority.set/priority.get can safely be outside the CS
/// as being a context local cell (not affected by preemptions).
/// It is merely used in order to omit masking in case
/// current priority >= ceiling.
///
/// Dereferencing a raw pointer is done safely inside the CS
///
/// Lock Efficiency:
/// Early experiments validate (sub)-zero cost for CS implementation
/// (Sub)-zero as:
/// - Either zero OH (lock optimized out), or
/// - Amounting to an optimal assembly implementation
///   - if ceiling == (1 << nvic_prio_bits)
///     - we execute the closure in a global critical section (interrupt free)
///     - CS entry cost, single write to core register
///     - CS exit cost, single write to core register
///   - else
///     - The `mask` value is folded to a constant at compile time
///     - CS entry, single write of the 32 bit `mask` to the `icer` register
///     - CS exit, single write of the 32 bit `mask` to the `iser` register
/// - priority.set/get optimized out (their effect not)
/// - On par or better than any hand written implementation of SRP
///
/// Limitations:
/// Current implementation does not allow for tasks with shared resources
/// to be bound to exception handlers, as these cannot be masked in HW.
///
/// Possible solutions:
/// - Mask exceptions by global critical sections (interrupt::free)
/// - Temporary lower exception priority
///
/// These possible solutions are set goals for future work
#[cfg(not(have_basepri))]
#[inline(always)]
pub unsafe fn lock<T, const M: usize>(
    ptr: *mut T,
    priority: u16,
    ceiling: u16,
    // _nvic_prio_bits: u8,
    masks: &[Mask<M>; 3],
    f: impl FnOnce(&mut T),
) {
    let current = priority;
    if current < ceiling {
        if ceiling >= 4 {
            // execute closure under protection of raised system ceiling
            interrupt::free(|_| f(&mut *ptr));
        } else {
            let mask = compute_mask(current as u8, ceiling as u8, masks);
            clear_enable_mask(mask);
            // execute closure under protection of raised system ceiling
            f(&mut *ptr);
            set_enable_mask(mask);
        }
    } else {
        // execute closure without raising system ceiling
        f(&mut *ptr)
    }
}

#[cfg(not(have_basepri))]
#[inline(always)]
fn compute_mask<const M: usize>(from_prio: u8, to_prio: u8, masks: &[Mask<M>; 3]) -> Mask<M> {
    let mut res = Mask([0; M]);
    masks[from_prio as usize..to_prio as usize]
        .iter()
        .for_each(|m| res |= *m);
    res
}

// enables interrupts
#[cfg(not(have_basepri))]
#[inline(always)]
unsafe fn set_enable_mask<const M: usize>(mask: Mask<M>) {
    for i in 0..M {
        // This check should involve compile time constants and be optimized out.
        if mask.0[i] != 0 {
            (*NVIC::PTR).iser[i].write(mask.0[i]);
        }
    }
}

// disables interrupts
#[cfg(not(have_basepri))]
#[inline(always)]
unsafe fn clear_enable_mask<const M: usize>(mask: Mask<M>) {
    for i in 0..M {
        // This check should involve compile time constants and be optimized out.
        if mask.0[i] != 0 {
            (*NVIC::PTR).icer[i].write(mask.0[i]);
        }
    }
}
#[cfg(not(have_basepri))]
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
/// On M23 we will pick a number that allows us to store the highest index used by the code.
/// This means the amount of overhead will vary based on the actually interrupts used by the code.
#[cfg(not(have_basepri))]
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

/// Cross pending interrupts
pub mod cross_core {

    pub struct FullFifoErr;

    #[inline]
    pub fn pend_irq(irq: u16) -> Result<(), FullFifoErr> {
        let sio = unsafe { &(*rp2040_hal::pac::SIO::PTR) };
        cortex_m::interrupt::free(|_| {
            if sio.fifo_st.read().rdy().bit() {
                // TX fifo is not full
                sio.fifo_wr.write(|wr| unsafe { wr.bits(irq as u32) });
                Ok(())
            } else {
                Err(FullFifoErr)
            }
        })
    }

    pub fn get_pended_irq() -> Option<rp2040_hal::pac::Interrupt> {
        let sio = unsafe { &(*rp2040_hal::pac::SIO::PTR) };
        if sio.fifo_st.read().vld().bit() {
            // valid data on fifo
            let irq = sio.fifo_rd.read().bits() as u16;
            // implementation must guarantee that the only messages passed in the fifo are of pac::Interrupt type.
            let irq = unsafe { core::mem::transmute::<u16, rp2040_hal::pac::Interrupt>(irq) };
            Some(irq)
        } else {
            None
        }
    }
}

#[no_mangle]
#[allow(non_snake_case)]
fn SIO_IRQ_PROC0() {
    if let Some(signal) = cross_core::get_pended_irq() {
        // info!("SIO_IRQ_PROC0: forwarding irq {}", signal as u16);
        NVIC::pend(signal);
    }
}

#[no_mangle]
#[allow(non_snake_case)]
fn SIO_IRQ_PROC1() {
    if let Some(signal) = cross_core::get_pended_irq() {
        // info!("SIO_IRQ_PROC1: forwarding irq {}", signal as u16);
        NVIC::pend(signal);
    }
}
