#![no_std]
#![no_main]

#[rtic::app(device = bsp, dispatchers = [Dma0])]
mod app {
    use bsp::{
        fugit::ExtU32,
        led::{self, Led},
        mmap::apb_timer::TIMER0_ADDR,
        sprintln,
        timer_group::Timer,
        uart::ApbUart,
        CPU_FREQ,
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

    // Tied to timer
    #[task(binds = Timer0Cmp, priority=1, shared=[uart])]
    struct SomeTask;

    impl RticTask for SomeTask {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            Sw1::spawn(1).ok();
            Sw2::spawn(2).ok();

            self.shared().uart.lock(|_uart| {
                sprintln!("Sorry");
            });
        }
    }

    #[sw_task(priority=2, shared=[uart])]
    struct Sw1;

    impl RticSwTask for Sw1 {
        type SpawnInput = u8;

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, p: u8) {
            self.shared().uart.lock(|_uart| {
                sprintln!("Fin {}", p);
            });
        }
    }

    #[sw_task(priority=2, shared=[uart])]
    struct Sw2;

    impl RticSwTask for Sw2 {
        type SpawnInput = u8;
        fn init() -> Self {
            Self
        }

        fn exec(&mut self, p: u8) {
            self.shared().uart.lock(|_uart| {
                sprintln!("Swe {}", p);
            });
            led::led_on(Led::Ld2);
        }
    }
}
