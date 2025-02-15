#![allow(unused)]

#[doc = r"Enumeration of additional interrupts."]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
#[repr(u16)]
pub enum InterruptExt {
    #[doc = "59 - Mailbox data available interrupt"]
    MAILBOX_INTERRUPT = 59, // We are stealing the interrupt line of `DMA2_CHANNEL4_5` and replacing that with mailbox interrupt
}

unsafe impl cortex_m::interrupt::InterruptNumber for InterruptExt {
    #[inline(always)]
    fn number(self) -> u16 {
        self as u16
    }
}

pub use implementation::*;
use stm32f1xx_hal::pac::NVIC;
mod implementation {
    use core::{marker::PhantomData, ops::Deref};

    use volatile_register::{RO, RW, WO};

    const FLAG_QUEUE_FULL: u32 = 1u32 << 0; // F flag
    const FLAG_QUEUE_EMPTY: u32 = 1u32 << 1; // E flag
    const FLAG_DATA_AVAILABLE: u32 = 1u32 << 2; // A flag

    /// Mailbox/FIFO peripheral
    pub struct Mailbox;

    unsafe impl Send for Mailbox {}

    impl Mailbox {
        /// Returns a pointer to the register block
        pub fn ptr() -> *const RegisterBlock {
            0x4003_0000 as *const _
        }
    }

    impl Deref for Mailbox {
        type Target = self::RegisterBlock;

        fn deref(&self) -> &Self::Target {
            unsafe { &*Self::ptr() }
        }
    }

    /// Mailbox Register block
    #[repr(C)]
    pub struct RegisterBlock {
        // mailbox read register
        pub rd: RO<u32>,
        // mailbox write register
        pub wr: WO<u32>,
        // mailbox status register
        pub status: RO<u32>,
    }

    impl RegisterBlock {
        pub fn status_full(&self) -> bool {
            (self.status.read() & FLAG_QUEUE_FULL) != 0
        }

        pub fn status_ready(&self) -> bool {
            (self.status.read() & FLAG_DATA_AVAILABLE) != 0
        }

        pub fn status_empty(&self) -> bool {
            (self.status.read() & FLAG_QUEUE_EMPTY) != 0
        }

        pub fn drain(&self) {
            while self.status_ready() {
                let _ = self.rd.read();
            }
        }
    }
}

/// Cross pending interrupts
pub mod cross_core {
    use super::Mailbox;

    #[inline]
    pub fn pend_irq(irq: u16) {
        cortex_m::interrupt::free(|_| unsafe {
            // WRITE IRQ NBR TO FIFO
            let fifo = &mut Mailbox;
            fifo.wr.write(irq as u32)
        });
    }

    pub fn get_pended_irq() -> Option<stm32f1xx_hal::pac::Interrupt> {
        // READ IRQ NBR FROM FIFO
        let fifo = &Mailbox;
        if fifo.status_ready() {
            let irq = fifo.rd.read() as u16;
            // implementation must guarantee that the only messages passed in the fifo are of pac::Interrupt type.
            let irq = unsafe { core::mem::transmute::<u16, stm32f1xx_hal::pac::Interrupt>(irq) };
            Some(irq)
        } else {
            None
        }
    }
}

#[no_mangle]
#[allow(non_snake_case)]
#[export_name = "DMA2_CHANNEL4_5"] // DMA2 channel 4_5 interrupt is repurposed for mailbox usage
fn MAILBOX_INTERRUPT() {
    if let Some(signal) = cross_core::get_pended_irq() {
        NVIC::pend(signal);
    }
}
