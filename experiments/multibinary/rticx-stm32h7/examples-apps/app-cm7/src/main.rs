//! Cortex-M7 side of the rticx-stm32h7 demo (M7-T1; transport ready for M7-T2).
//!
//! Global core 0. The rticx-stm32h7 `#[app]` macro runs the core pass, the
//! cross-binary pass and the software-tasks pass; the distribution's runtime
//! backend initializes the shared ready/epoch state in D2 SRAM3, maps it
//! Non-cacheable through the MPU and releases the Cortex-M4 by setting
//! `RCC_GCR.BOOT_C2` (all inside the generated `init_shared` hook). The
//! register access goes through the `stm32h7` PAC; the console is a
//! `stm32-hal2` `Usart`.
//!
//! The application declares a cross-binary receiver (`PongTask`, spawned by the
//! M4 on global core 1). The pass therefore generates the complete M7-T1/M7-T2
//! transport here: the `PingTask` sender stub, the `0 -> 1` doorbell ring
//! function, the `1 -> 0` read function and doorbell router bound to `HSEM0`
//! (IRQ 125), and the USART3 line dispatcher. Triggering the ping from `idle`
//! is left to M7-T2: phase-1 `cargo xbin sync` cannot type-check a generated
//! sender stub.
#![no_std]
#![no_main]

use panic_halt as _;

#[rticx_stm32h7::app(
    device = stm32h7::stm32h747cm7,
    cores = 1,
    core_ids = [0],
    external_cores = [1],
    ipc_dispatchers = [USART3]
)]
pub mod app {
    use embedded_io::Write as _;

    use stm32_hal2::{clocks::Clocks, pac, usart::Usart};

    /// Console shared (under the SRP lock) by `init`, `PongTask` and `Idle`.
    #[shared]
    struct Shared {
        console: Usart<pac::USART1>,
    }

    /// Cross-binary receiver: the M4 (global core 1) spawns a pong here. The
    /// M7 source never declares the `PingTask` sender stub; the cross-binary
    /// pass generates it from the synced system view.
    #[allow(dead_code)]
    #[sw_task(priority = 3, capacity = 2, spawn_by = 1, shared = [console])]
    struct PongTask;

    impl RticSwTask for PongTask {
        type SpawnInput = ipc_types::PingMsg;

        fn exec(&mut self, input: Self::SpawnInput) {
            self.shared().console.lock(|console| {
                let _ = writeln!(
                    console,
                    "[M7] rx pong seq={} value={:#010x}",
                    input.seq, input.value
                );
            });
        }
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let dp = pac::Peripherals::take().unwrap();
        let clocks = Clocks::default();
        let mut console = Usart::new(dp.USART1, 115_200, Default::default(), &clocks).unwrap();
        let _ = writeln!(
            console,
            "[M7] boot; initializing shared IPC region and releasing the M4"
        );
        (Shared { console }, TaskInits { idle: Idle })
    }

    #[idle(shared = [console])]
    struct Idle;

    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            use rticx_stm32h7::export::xbin_rt::CrossBinBackend as _;

            let backend = rticx_stm32h7::xbin::Backend;
            self.shared().console.lock(|console| {
                let _ = writeln!(
                    console,
                    "[M7] shared region ready, RCC_GCR.BOOT_C2 set; waiting for the M4"
                );
            });

            let mut announced = false;
            loop {
                if !announced && backend.is_ready(1) {
                    announced = true;
                    self.shared().console.lock(|console| {
                        let _ = writeln!(
                            console,
                            "[M7] Cortex-M4 marked itself ready; both cores ready (M7-T1)"
                        );
                    });
                }
                core::hint::spin_loop();
            }
        }
    }
}
