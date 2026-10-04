//! Cortex-M4 side of the rticx-stm32h7 demo (M7-T1; transport ready for M7-T2).
//!
//! Global core 1. The Cortex-M4 is held in hold-boot until the Cortex-M7 writes
//! `RCC_GCR.BOOT_C2`; when it boots it maps the shared SRAM3 through the MPU,
//! enables its HSEM receive interrupt (`HSEM1`, IRQ 126) and marks itself ready
//! in the shared state (the generated `mark_ready` hook). It never releases a
//! peer. The register access goes through the `stm32h7` PAC; the console is a
//! `stm32-hal2` `Usart`.
//!
//! The application declares a cross-binary receiver (`PingTask`, spawned by the
//! M7 on global core 0). The pass generates the other half of the transport
//! here: the `PongTask` sender stub, the `1 -> 0` doorbell ring function, the
//! `0 -> 1` read function, the doorbell router bound to `HSEM1`, and the
//! USART3 line dispatcher that drains the task FIFO into `PingTask::exec`.
#![no_std]
#![no_main]

use panic_halt as _;

#[rticx_stm32h7::app(
    device = stm32h7::stm32h747cm4,
    cores = 1,
    core_ids = [1],
    external_cores = [0],
    ipc_dispatchers = [USART3]
)]
pub mod app {
    use embedded_io::Write as _;

    use stm32_hal2::{clocks::Clocks, pac, usart::Usart};

    /// Console shared (under the SRP lock) by `init`, `PingTask` and `Idle`.
    #[shared]
    struct Shared {
        console: Usart<pac::USART2>,
    }

    /// Cross-binary receiver: the M7 (global core 0) spawns a ping here.
    #[allow(dead_code)]
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0, shared = [console])]
    struct PingTask;

    impl RticSwTask for PingTask {
        type SpawnInput = ipc_types::PingMsg;

        fn exec(&mut self, input: Self::SpawnInput) {
            self.shared().console.lock(|console| {
                let _ = writeln!(
                    console,
                    "[M4] rx ping seq={} value={:#010x}",
                    input.seq, input.value
                );
            });
        }
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let dp = pac::Peripherals::take().unwrap();
        let clocks = Clocks::default();
        let mut console = Usart::new(dp.USART2, 115_200, Default::default(), &clocks).unwrap();
        let _ = writeln!(
            console,
            "[M4] released by RCC_GCR.BOOT_C2; mapping shared region and marking ready"
        );
        (Shared { console }, TaskInits { idle: Idle })
    }

    #[idle(shared = [console])]
    struct Idle;

    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            self.shared().console.lock(|console| {
                let _ = writeln!(
                    console,
                    "[M4] shared region mapped, HSEM1 receive enabled, ready bit set (M7-T1)"
                );
            });
            loop {
                core::hint::spin_loop();
            }
        }
    }
}
