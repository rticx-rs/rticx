//! Super simple RTIC multicore PingPong example:
//! Core 0 task dispatched by DMA_IRQ_0 -> pings -> Core 1 task dispatched by DMA_IRQ_1
//! and vice-versa !

#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_halt as _;

#[unsafe(link_section = ".boot2")]
#[used]
pub static BOOT2: [u8; 256] = rp2040_boot2::BOOT_LOADER_GENERIC_03H;

#[rticx_rp2040::app(device=rp2040_hal::pac, dispatchers=[[DMA_IRQ_0], [DMA_IRQ_1]], cores = 2, core1_stack = 2048)]
pub mod my_app {

    use cortex_m::asm;
    use defmt::assert_eq;
    use defmt::*;
    use rp2040_hal::pac;

    const PING_PONG_DELAY: u32 = 30000000;

    // ======================================= CORE 0 ==============================================
    #[init(core = 0)]
    fn init_core0() -> TaskInitsCore0 {
        assert_eq!(get_core_id(), 0);
        println!("staring RP2040 core 0 ...");

        let mut device = pac::Peripherals::take().unwrap();

        // Initialization of the system clock.
        let mut watchdog = rp2040_hal::watchdog::Watchdog::new(device.WATCHDOG);

        // Configure the clocks - The default is to generate a 125 MHz system clock
        let _clocks = rp2040_hal::clocks::init_clocks_and_plls(
            // External high-speed crystal on the Raspberry Pi Pico board is 12 MHz
            12_000_000u32,
            device.XOSC,
            device.CLOCKS,
            device.PLL_SYS,
            device.PLL_USB,
            &mut device.RESETS,
            &mut watchdog,
        )
        .ok()
        .unwrap();

        TaskInitsCore0 {
            my_idle_task: MyIdleTask { count: 0 },
        }
    }

    #[idle(core = 0)]
    pub struct MyIdleTask {
        /* local resources */
        count: u32,
    }
    impl RticIdleTask for MyIdleTask {
        fn exec(&mut self) -> ! {
            loop {
                self.count += 1;
                // println!("looping in idle... {}", self.count);
                asm::delay(120000000);
            }
        }
    }

    /// a Core0 task to be spawned by a task on Core1
    #[sw_task(priority = 2, spawn_by = 1, core = 0, init = generated)]
    struct Core0Task;
    impl RticSwTask for Core0Task {
        type SpawnInput = u32;
        fn exec(&mut self, ping: Self::SpawnInput) {
            assert_eq!(get_core_id(), 0); // assert that this is executing on core 0
            asm::delay(PING_PONG_DELAY); // add some delay for visualization
            let pong = ping + 1;
            println!("CORE0: Got ping {}, sending pong {}", ping, pong);
            if let Err(_e) = Core1Task::spawn_from(Self::current_core(), pong) {
                error!("couldn't spawn task on core 1 from core 0")
            }

            // UNCOMMENT NEXT STATEMENT TO SEE THAT IT IS NOT ALLOWED
            // BECAUSE TASK IS MARKED BY `spawn_by = 1`. I.e only core 1 can spawn this task
            // let _ = Core0Task::spawn_from(Self::current_core(), 1);
        }
    }

    // ======================================= CORE 1 ==============================================

    #[init(core = 1)]
    fn init_core1() -> TaskInitsCore1 {
        assert_eq!(get_core_id(), 1);
        println!("staring RP2040 core 1 ...");

        TaskInitsCore1 {
            core1_task: Core1Task,
        }
    }

    /// Spawn the first ping-pong message from core 1. Spawning during `#[init]`
    /// is rejected (tasks are not initialized yet), so this runs in post-init.
    #[post_init(core = 1)]
    fn start_ping_pong() {
        Core0Task::spawn_from(Core1Task::current_core(), 1).expect("Couldn't start task on core 0");
    }

    /// a Core1 task to be spawned by a task on Core0
    #[sw_task(priority = 2, core = 1, spawn_by = 0)]
    struct Core1Task;
    impl RticSwTask for Core1Task {
        type SpawnInput = u32;
        fn exec(&mut self, pong: Self::SpawnInput) {
            assert_eq!(get_core_id(), 1); // assert that this is executing on core 0
            asm::delay(PING_PONG_DELAY); // add some delay for visualization
            let ping = pong + 1;
            println!("CORE1: Got pong {}, sending ping {}", pong, ping);
            if let Err(_e) = Core0Task::spawn_from(Self::current_core(), ping) {
                error!("couldn't spawn task on core 0 from core 1")
            }
        }
    }

    // no idle task is needed for core 1 in this example

    // ======================================== UTILS ==============================================

    /// Reads core number (0 or 1) from the rp2040 CPUID register
    fn get_core_id() -> u32 {
        unsafe { (*pac::SIO::PTR).cpuid().read().bits() }
    }
}
