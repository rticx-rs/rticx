#![no_std]
#![no_main]

use hifive1::hal::e310x;
use riscv_rt as _;

#[rticx_riscv::app(device = e310x, dispatchers = [SW_INT0, SW_INT1])]
pub mod app {
    use super::e310x;
    use semihosting::{println, process::exit};

    const TARGET: u32 = 4;

    #[shared]
    struct Shared {
        counter: u32,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        println!("[Init]: Started");
        (Shared { counter: 0 }, TaskInits {})
    }

    #[idle(init = generated)]
    struct IdleTask;

    impl RticIdleTask for IdleTask {
        fn exec(&mut self) -> ! {
            loop {
                println!("[Idle]: loop start");
                Worker::spawn(()).unwrap();
                println!("[Idle]: loop end");
            }
        }
    }

    #[sw_task(priority = 2, shared = [counter], init = generated)]
    struct Worker;

    impl RticSwTask for Worker {
        type SpawnInput = ();

        fn exec(&mut self, _input: ()) {
            self.shared().counter.lock(|c| {
                *c = c.wrapping_add(1);
                println!("    [Worker]: counter = {}", *c);

                if *c >= TARGET {
                    println!("SUCCESS: {} spawns completed", *c);
                    exit(0);
                }
            });
        }
    }
}
