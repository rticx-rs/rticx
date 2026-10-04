//! Cortex-M7 side of the rticx-stm32h7 demo (M7-T1 + M7-T2).
//!
//! Global core 0. The rticx-stm32h7 `#[app]` macro runs the core pass, the
//! cross-binary pass and the software-tasks pass; the distribution's runtime
//! backend maps the shared D2 SRAM3 region Non-cacheable through the MPU, and
//! its `post_init` hook (distribution-owned boot sequencing) clears the shared
//! control area, releases the Cortex-M4 by setting `RCC_GCR.BOOT_C2` and then
//! signals this core up. The register access goes through the `stm32h7` PAC;
//! the console is a `stm32-hal2` `Usart`.
//!
//! The application declares a cross-binary receiver (`PongTask`, spawned by the
//! M4 on global core 1). The pass therefore generates the complete transport
//! here: the `PingTask` sender stub, the `0 -> 1` doorbell ring function, the
//! `1 -> 0` read function and doorbell router bound to `HSEM0` (IRQ 125), and
//! the USART3 line dispatcher.
//!
//! `idle` waits until the M4 is up (the distribution's peer-up flag) and then
//! starts the cross-binary ping-pong by calling the generated
//! `PingTask::cross_spawn` (M7-T2). Each handler logs its hop and spawns the
//! next one through the other core, so a single `renode/run.sh` boot shows both
//! directions executing through the generated dispatch path.
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

    /// Cross-binary hops the demo runs before it stops; the ping-pong
    /// alternates M7 -> M4 -> M7 ..., so four hops show both directions twice.
    const PING_PONG_LIMIT: u32 = 4;

    /// Console shared (under the SRP lock) by `init`, `PongTask` and `Idle`.
    #[shared]
    struct Shared {
        console: Usart<pac::USART1>,
    }

    /// Cross-binary receiver: the M4 (global core 1) spawns a pong here. The
    /// M7 source never declares the `PingTask` sender stub; the cross-binary
    /// pass generates it from the synced system view.
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

            if input.seq >= PING_PONG_LIMIT {
                self.shared().console.lock(|console| {
                    let _ = writeln!(
                        console,
                        "[M7] cross-binary ping-pong complete after {PING_PONG_LIMIT} hops \
                         (M7-T2)"
                    );
                });
                return;
            }

            // Answer the M4 through the generated `PingTask` sender stub: the
            // input is enqueued in the `(0 -> 1)` FIFO and the M4's HSEM1 router
            // is rung (M7-T2).
            let _ = PingTask::cross_spawn(ipc_types::PingMsg {
                seq: input.seq + 1,
                value: input.value.wrapping_add(1),
            });
        }
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let dp = pac::Peripherals::take().unwrap();
        let clocks = Clocks::default();
        let mut console = Usart::new(dp.USART1, 115_200, Default::default(), &clocks).unwrap();
        let _ = writeln!(console, "[M7] boot; initializing shared IPC region");
        (Shared { console }, TaskInits { idle: Idle })
    }

    #[idle(shared = [console])]
    struct Idle;

    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            self.shared().console.lock(|console| {
                let _ = writeln!(
                    console,
                    "[M7] shared region ready; waiting for the M4 to come up"
                );
            });

            // The M4 signals that it is up at the end of its `post_init`; the
            // distribution owns this peer-up handshake.
            while !rticx_stm32h7::xbin::peer_is_up() {
                core::hint::spin_loop();
            }
            self.shared().console.lock(|console| {
                let _ = writeln!(
                    console,
                    "[M7] Cortex-M4 up; starting the M7 -> M4 ping (M7-T2)"
                );
            });

            // Start the ping-pong; the M4's `PingTask::exec` answers with a pong
            // and the two handlers keep alternating until `PING_PONG_LIMIT`.
            let _ = PingTask::cross_spawn(ipc_types::PingMsg {
                seq: 1,
                // `0x4D37` is "M7" in ASCII: the marker of the initiating core.
                value: 0x4D37,
            });

            loop {
                core::hint::spin_loop();
            }
        }
    }
}
