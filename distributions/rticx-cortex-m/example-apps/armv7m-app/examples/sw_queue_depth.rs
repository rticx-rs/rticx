#![no_std]
#![no_main]

//! QEMU-runnable proof that the `capacity = N` argument of `#[sw_task]`
//! sizes the task's input queue: the task can be spawned N times before the
//! queue fills up, and the buffered inputs are processed in FIFO order.
//!
//! A SysTick hardware task (priority 1) spawns `Worker` (priority 2,
//! `capacity = 4`) four times back-to-back and then verifies that a fifth
//! spawn is rejected. The dispatcher runs at priority 2, so it cannot
//! preempt the SysTick handler: all four spawns are buffered before any
//! input is processed.

use panic_halt as _;

#[rticx_cortex_m::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6])]
pub mod my_app {
    use cortex_m::peripheral::{syst::SystClkSource, Peripherals};
    use cortex_m_semihosting::{debug, hprintln};

    /// Number of spawns the example queues back-to-back.
    const SPAWNS: u32 = 4;

    #[shared]
    struct Shared {
        state: State,
    }

    pub struct State {
        /// Whether the SysTick handler has already performed its spawns.
        pub spawned: bool,
        /// Next input value we expect (used to check FIFO ordering).
        pub next_expected: u32,
        /// Number of inputs processed so far.
        pub received: u32,
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

        Shared {
            state: State {
                spawned: false,
                next_expected: 0,
                received: 0,
            },
        }
    }

    /// SysTick exception hardware task: queues `SPAWNS` spawns of `Worker`
    /// once, then verifies the queue rejects one more spawn.
    #[task(binds = SysTick, priority = 1, shared = [state])]
    struct Tick;

    impl RticTask for Tick {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            self.shared().state.lock(|state| {
                if state.spawned {
                    return;
                }
                state.spawned = true;

                for i in 0..SPAWNS {
                    if Worker::spawn(i).is_err() {
                        hprintln!("FAILURE: spawn #{} was rejected", i);
                        debug::exit(debug::EXIT_FAILURE);
                    }
                }
                match Worker::spawn(SPAWNS) {
                    Err(_) => {
                        hprintln!("as expected: spawn #{} rejected (queue full)", SPAWNS)
                    }
                    Ok(()) => {
                        hprintln!("FAILURE: spawn #{} unexpectedly succeeded", SPAWNS);
                        debug::exit(debug::EXIT_FAILURE);
                    }
                }
            });
        }
    }

    /// Software task with an input queue of capacity 4 (ring buffer of 5 slots).
    #[sw_task(priority = 2, capacity = 4, shared = [state])]
    struct Worker;

    impl RticSwTask for Worker {
        type SpawnInput = u32;

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, input: u32) {
            self.shared().state.lock(|state| {
                if input != state.next_expected {
                    hprintln!(
                        "FAILURE: expected input {}, got {}",
                        state.next_expected,
                        input
                    );
                    debug::exit(debug::EXIT_FAILURE);
                }
                state.next_expected += 1;
                state.received += 1;
                hprintln!(
                    "worker: processed input {} ({}/{})",
                    input,
                    state.received,
                    SPAWNS
                );
                if state.received == SPAWNS {
                    hprintln!("SUCCESS: all {} queued spawns processed in FIFO order", SPAWNS);
                    // Terminate QEMU with exit code 0.
                    debug::exit(debug::EXIT_SUCCESS);
                }
            });
        }
    }
}
