#![no_std]
#![no_main]

#[rtic::app(device = bsp)]
mod app {
    use bsp::embedded_io::Write;
    use bsp::fugit::ExtU32;
    use bsp::mmap::apb_timer::TIMER0_ADDR;
    use bsp::timer_group::Timer;
    use bsp::uart::ApbUart;
    use bsp::Interrupt::{self};
    use bsp::{sprintln, CPU_FREQ};
    use core::arch::asm;

    #[shared]
    struct Shared {
        uart: ApbUart,
    }

    #[init]
    fn init() -> Shared {
        let uart = ApbUart::init(CPU_FREQ, 115_200);
        let mut timer = Timer::init::<TIMER0_ADDR>().into_periodic();

        sprintln!("init");
        timer.set_period(10_u32.micros());
        timer.start();

        Shared { uart }
    }

    #[task(binds = Timer0Cmp, priority=1, shared=[uart])]
    struct Task1;

    impl RticTask for Task1 {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            let mut r: u32;
            unsafe {
                asm!("csrrs {0}, 0xB40, x0", out(reg) r);
            }
            // csrr    t3, 0xB40               # read captured timestamp
            self.shared().uart.lock(|_uart| {
                sprintln!("time {}", r);
                // uart.write_byte(r);
                // uart.write_byte(41);

                // rtic::export::pend(hippomenes_core::Interrupt1);

                // uart.write_byte(102);
                // uart.write_byte(103);
            });
        }
    }

    #[task(binds = Dma1, priority=3, shared=[uart])]
    struct Task2;

    impl RticTask for Task2 {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            self.shared().uart.lock(|uart| {
                uart.write_all(&[10, 0]).unwrap();
                rtic::export::pend(Interrupt::Dma2);
                uart.write_all(&[11]).unwrap();
            });
        }
    }

    #[task(binds = Dma2, priority=2, shared=[uart])]
    struct Task3 {
        data: u8,
    }

    impl RticTask for Task3 {
        fn init() -> Self {
            Self { data: 0 }
        }

        fn exec(&mut self) {
            self.shared().uart.lock(|uart| {
                uart.write_all(&[90, self.data, 92]).unwrap();
            });
            self.data = (self.data + 1) % 10;
        }
    }
}
