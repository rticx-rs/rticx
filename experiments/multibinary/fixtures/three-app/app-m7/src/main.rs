//! First application of the three-application fixture (M6-T1).
//!
//! Global core 0 participates in both directions of its two pairs:
//!
//! - as a **producer** it spawns `EncryptTask` (priority 3) and `DigestTask`
//!   (priority 5) on `app-m4` (global core 1) through the `(0 -> 1)` region,
//!   and `ConfigTask` (priority 5) on `app-m5` (global core 2) through the
//!   `(0 -> 2)` region. The producer declares nothing: the pass generates the
//!   sender stubs and their `cross_spawn` from the synced system view (M5.5);
//! - as a **receiver** it executes `AckTask` (priority 3), spawned by global
//!   core 1 over `(1 -> 0)`, and `HeartbeatTask` (priority 6), spawned by
//!   global core 2 over `(2 -> 0)`.
//!
//! `ipc_dispatchers` lists one interrupt per cross line in ascending
//! `(source, priority)` order, so `XBIN_IPC_LINE_0` wakes the `(1, 3)`
//! dispatcher and `XBIN_IPC_LINE_1` the `(2, 6)` one.
#![no_main]

use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [0],
    external_cores = [1, 2],
    ipc_dispatchers = [XBIN_IPC_LINE_0, XBIN_IPC_LINE_1]
)]
mod app {
    /// Receiver of `AckTask`, spawned by global core 1.
    #[sw_task(priority = 3, capacity = 1, spawn_by = 1)]
    pub struct AckTask;

    impl RticSwTask for AckTask {
        type SpawnInput = ipc_types::AckMsg;

        fn exec(&mut self, _input: Self::SpawnInput) {
            // The in-tree runtime harness runs the generated dispatchers.
        }
    }

    /// Receiver of `HeartbeatTask`, spawned by global core 2.
    #[sw_task(priority = 6, capacity = 1, spawn_by = 2)]
    pub struct HeartbeatTask;

    impl RticSwTask for HeartbeatTask {
        type SpawnInput = ipc_types::HeartbeatMsg;

        fn exec(&mut self, _input: Self::SpawnInput) {}
    }

    #[idle]
    struct Idle;

    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            loop {
                core::hint::spin_loop();
            }
        }
    }

    #[init]
    fn init() -> TaskInits {
        TaskInits { idle: Idle }
    }
}
