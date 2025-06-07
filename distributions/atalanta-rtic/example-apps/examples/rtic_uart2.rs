#![no_std]
#![no_main]

#[rtic::app(device = bsp)]
mod app {
    use bsp::{
        fugit::ExtU32, mmap::apb_timer::TIMER0_ADDR, sprintln, timer_group::Timer, uart::ApbUart,
        Interrupt, CPU_FREQ,
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

    #[task(binds = Timer0Cmp, priority=2)]
    struct Task1 {}

    impl RticTask for Task1 {
        fn init() -> Self {
            let _uart = ApbUart::init(CPU_FREQ, 115_200);
            let mut timer = Timer::init::<TIMER0_ADDR>().into_periodic();

            sprintln!("init");
            timer.set_period(10_u32.micros());
            timer.start();
            Self {}
        }

        fn exec(&mut self) {
            sprintln!("A");

            rtic::export::pend(Interrupt::Dma1);

            sprintln!("B");
        }
    }

    #[task(binds = Dma1, priority=3)]
    struct Task2;

    impl RticTask for Task2 {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {}
    }
}
