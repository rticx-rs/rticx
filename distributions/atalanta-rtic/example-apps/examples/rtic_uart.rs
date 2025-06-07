#![no_std]
#![no_main]

#[rtic::app(device = bsp)]
mod app {
    use bsp::{
        fugit::ExtU32, mmap::apb_timer::TIMER0_ADDR, sprintln, timer_group::Timer, uart::ApbUart,
        ufmt, CPU_FREQ,
    };
    #[shared]
    struct Shared {
        dummy: bool,
    }

    #[init]
    fn init() -> Shared {
        Shared { dummy: true }
    }

    #[task(binds = Timer0Cmp, priority=1, shared=[dummy])]
    struct SomeTask {}

    impl RticTask for SomeTask {
        fn init() -> Self {
            let _uart = ApbUart::init(CPU_FREQ, 115_200);
            sprintln!("init");

            let mut timer = Timer::init::<TIMER0_ADDR>().into_periodic();
            timer.set_period(10.micros());
            timer.start();

            Self {}
        }

        fn exec(&mut self) {
            sprintln!("A");
            sprintln!("1");
            sprintln!("B");
        }
    }
}
