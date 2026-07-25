#![no_std]
#![no_main]

//! QEMU-runnable RTIC playground for single-core Cortex-M.

use panic_halt as _;

#[cortex_m_rtic::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6])]
pub mod my_app {
    use cortex_m::peripheral::{Peripherals, syst::SystClkSource};
    use cortex_m_semihosting::{debug, hprintln};

    /// Number of SysTick-driven spawns after which the example declares success
    /// and asks QEMU to terminate with exit code 0.
    const TARGET: u32 = 4;

    /// Shared resource guarded by RTIC's SRP locking. Accessing this from the
    /// software task is what exercises the `lock` primitive whose
    /// implementation differs between the BASEPRI and source-masking paths.
    #[shared]
    struct Shared {
        counter: u32,
    }

    #[init]
    fn system_init() -> Shared {
        let mut cp = unsafe { Peripherals::steal() };
        cp.SYST.set_clock_source(SystClkSource::Core);
        // Short reload so ticks arrive quickly enough for CI.
        cp.SYST.set_reload(0x1_000);
        cp.SYST.clear_current();
        cp.SYST.enable_interrupt();
        cp.SYST.enable_counter();

        Shared { counter: 0 }
    }

    /// SysTick exception hardware task
    #[task(binds = SysTick, priority = 1)]
    struct Tick;

    impl RticTask for Tick {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            let _ = Worker::spawn(());
        }
    }

    /// Software task dispatched by the `TIM6` NVIC interrupt
    #[sw_task(priority = 2, shared = [counter])]
    struct Worker;

    impl RticSwTask for Worker {
        type SpawnInput = ();

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, _input: ()) {
            self.shared().counter.lock(|c| {
                *c = c.wrapping_add(1);
                hprintln!("tick {}: counter = {}", *c, *c);
                if *c >= TARGET {
                    hprintln!("SUCCESS: {} spawns completed under RTIC locking", *c);
                    // Terminate QEMU with exit code 0.
                    debug::exit(debug::EXIT_SUCCESS);
                }
            });
        }
    }
}