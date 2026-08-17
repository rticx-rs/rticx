#![no_std]
#![no_main]

//! QEMU-runnable proof that the `capacity = N` argument of `#[async_task]`
//! lets a task be spawned N times while a previous instance is still
//! running. Async tasks execute one instance at a time; extra spawns are
//! buffered in the input queue and installed by the dispatcher as soon as
//! the running future completes.
//!
//! A priority-3 `Spawner` task spawns `Worker` (priority 2, `capacity = 4`)
//! four times back-to-back. Because the priority-2 dispatcher cannot preempt
//! the priority-3 dispatcher, all four spawns are buffered before the first
//! `Worker` future is even installed; with a default queue (`capacity = 1`)
//! only the first spawn would succeed. A fifth spawn must be rejected. Each
//! `Worker` instance then runs for 200 ms before the next one is installed,
//! and the buffered inputs must be processed in FIFO order.

use panic_halt as _;
use rtic_monotonics::systick::prelude::*;
systick_monotonic!(Mono, 1000);

#[rticx_cortex_m::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6, TIM3])]
mod app {
    use super::*;
    use cortex_m_semihosting::{debug, hprintln};

    /// Number of spawns the example queues back-to-back.
    const SPAWNS: u32 = 4;

    #[shared]
    struct Shared {
        state: State,
    }

    pub struct State {
        /// Next input value we expect (used to check FIFO ordering).
        pub next_expected: u32,
        /// Number of inputs processed so far.
        pub received: u32,
    }

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        // Setup clocks
        let core = unsafe { cortex_m::Peripherals::steal() };
        Mono::start(core.SYST, 10_000_000); // 10MHz

        (
            Shared {
                state: State {
                    next_expected: 0,
                    received: 0,
                },
            },
            TaskInits {},
        )
    }

    #[post_init]
    fn post_init() {
        let _ = Spawner::spawn(());
    }

    /// Spawns `Worker` `SPAWNS` times in one go and checks the queue boundary.
    #[async_task(priority = 3, init = generated)]
    struct Spawner;
    impl RticAsyncTask for Spawner {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            for i in 0..SPAWNS {
                if Worker::spawn(i).is_err() {
                    hprintln!("FAILURE: spawn #{} was rejected", i);
                    debug::exit(debug::EXIT_FAILURE);
                }
                hprintln!("spawner: queued spawn #{}", i);
            }
            match Worker::spawn(SPAWNS) {
                Err(_) => hprintln!("as expected: spawn #{} rejected (queue full)", SPAWNS),
                Ok(()) => {
                    hprintln!("FAILURE: spawn #{} unexpectedly succeeded", SPAWNS);
                    debug::exit(debug::EXIT_FAILURE);
                }
            }
        }
    }

    /// Async task with an input queue of capacity 4 (ring buffer of 5 slots).
    /// Each instance runs for 200 ms so that the queued spawns have to wait.
    #[async_task(priority = 2, capacity = 4, shared = [state], init = generated)]
    struct Worker;
    impl RticAsyncTask for Worker {
        type SpawnInput = u32;
        async fn exec(&mut self, input: u32) {
            hprintln!("worker: running input {}", input);
            // Keep this instance alive so subsequent spawns are deferred
            // until the dispatcher frees this task's exec slot.
            Mono::delay(200.millis()).await;

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
                    "worker: done with input {} ({}/{})",
                    input,
                    state.received,
                    SPAWNS
                );
                if state.received == SPAWNS {
                    hprintln!(
                        "SUCCESS: all {} queued spawns processed in FIFO order",
                        SPAWNS
                    );
                    // Terminate QEMU with exit code 0.
                    debug::exit(debug::EXIT_SUCCESS);
                }
            });
        }
    }
}
