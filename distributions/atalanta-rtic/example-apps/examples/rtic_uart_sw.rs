#![no_std]
#![no_main]

#[rtic::app(device = bsp, dispatchers = [Dma2])]
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

    #[task(binds = Timer0Cmp, priority=1, shared=[uart])]
    struct SomeTask;

    impl RticTask for SomeTask {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            self.shared().uart.lock(|_uart| {
                sprintln!("T");
                sprintln!("1");
            });

            Sw1::spawn(()).unwrap();

            self.shared().uart.lock(|_uart| {
                sprintln!("T");
                sprintln!("2");
            });
        }
    }

    #[sw_task(priority=2, shared=[uart])]
    struct Sw1;

    impl RticSwTask for Sw1 {
        type SpawnInput = ();

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, _p: ()) {
            self.shared().uart.lock(|_uart| {
                sprintln!("SW");
            });
            led::led_on(Led::Ld2);
        }
    }
}
