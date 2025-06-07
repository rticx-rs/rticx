#![no_std]
#![no_main]

#[rtic::app(device = bsp, dispatchers = [Dma2, Dma3])]
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
        resource: bool,
    }

    #[init]
    fn init() -> Shared {
        let _uart = ApbUart::init(CPU_FREQ, 115_200);
        let mut timer = Timer::init::<TIMER0_ADDR>().into_periodic();

        sprintln!("init");
        timer.set_period(10_u32.micros());
        timer.start();

        Shared { resource: true }
    }

    #[task(binds = Timer0Cmp, priority=2, shared=[resource])]
    struct TimerTask;

    impl RticTask for TimerTask {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            Sw1::spawn(()).ok();
            Sw2::spawn(()).ok();
            self.shared().resource.lock(|_| {});
        }
    }

    #[sw_task(priority=1, shared=[resource])]
    struct Sw1;

    impl RticSwTask for Sw1 {
        type SpawnInput = ();

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, _: ()) {
            led::led_on(Led::Ld2);
            self.shared().resource.lock(|_| {});
        }
    }
    #[sw_task(priority = 3)]
    struct Sw2;

    impl RticSwTask for Sw2 {
        type SpawnInput = ();

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, _: ()) {}
    }
}
