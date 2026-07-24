#![no_std]
#![no_main]

use panic_halt as _;

#[rtic::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6])]
pub mod my_app {
    /// Shared resources guarded by RTIC's SRP locking.
    #[shared]
    struct Shared {
        counter: u32,
    }

    #[init]
    fn system_init() -> Shared {
        // TODO: TIM3 initialization is missing
        Shared { counter: 0 }
    }

    /// Hardware task: bumped by the TIM3 interrupt. It bumps the shared counter
    /// (exercising the SRP `lock`) and then spawns a software task.
    #[task(binds = TIM3, priority = 2, shared = [counter])]
    struct Bumper;

    impl RticTask for Bumper {
        fn init() -> Self {
            Self
        }

        fn exec(&mut self) {
            self.shared().counter.lock(|c| {
                *c = c.wrapping_add(1);
            });
            let _ = Worker::spawn(0u8);
        }
    }

    /// Software task: dispatched by the TIM2 dispatcher. Reads/updates the shared
    /// counter through a resource proxy lock.
    #[sw_task(priority = 2, shared = [counter])]
    struct Worker;

    impl RticSwTask for Worker {
        type SpawnInput = u8;

        fn init() -> Self {
            Self
        }

        fn exec(&mut self, n: u8) {
            self.shared().counter.lock(|c| {
                *c = c.wrapping_add(n as u32);
            });
        }
    }
}