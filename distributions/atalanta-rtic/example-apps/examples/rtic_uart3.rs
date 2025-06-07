#![no_std]
#![no_main]

#[rtic::app(device = bsp)]
mod app {
    use bsp::{
        embedded_io::Write, fugit::ExtU32, mmap::apb_timer::TIMER0_ADDR, sprintln,
        timer_group::Timer, uart::ApbUart, Interrupt, CPU_FREQ,
    };

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

    #[task(binds = Dma0, priority=1, shared=[uart])]
    struct Task1;

    impl RticTask for Task1 {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            self.shared().uart.lock(|uart| {
                uart.write_all(&[40, 41]).unwrap();

                rtic::export::pend(Interrupt::Dma1);

                uart.write_all(&[102, 103]).unwrap();
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
