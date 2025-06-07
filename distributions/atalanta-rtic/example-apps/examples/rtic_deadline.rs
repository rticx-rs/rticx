#![no_std]
#![no_main]

#[rtic::app(device = bsp)]
mod app {
    use bsp::{
        fugit::ExtU32, mmap::apb_timer::TIMER0_ADDR, sprintln, timer_group::Timer, uart::ApbUart,
        CPU_FREQ,
    };
    #[shared]
    struct Shared {
        dummy: bool,
    }

    #[init]
    fn init() -> Shared {
        Shared { dummy: true }
    }

    #[task(binds = Timer0Cmp, deadline=1, shared=[dummy])]
    struct SomeTask {}

    impl RticTask for SomeTask {
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
            //self.uart.write_byte(0); // force sentinel, notice NOT end of packet
            sprintln!("1");
            sprintln!("B");
        }
    }
}
