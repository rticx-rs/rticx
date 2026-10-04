//! Second application of the three-application fixture (M6-T1).
//!
//! Global core 2 participates in both directions of its two pairs:
//!
//! - as a **producer** it spawns `SensorTask` (priority 4) and `TelemetryTask`
//!   (priority 6) on `app-m4` (global core 1) through the `(2 -> 1)` region,
//!   and `HeartbeatTask` (priority 6) on `app-m7` (global core 0) through the
//!   `(2 -> 0)` region. Like the other producer it declares nothing and only
//!   depends on the generated `ipc-types` crate (M5.5);
//! - as a **receiver** it executes `ConfigTask` (priority 5), spawned by global
//!   core 0 over `(0 -> 2)`, and `StatusTask` (priority 3), spawned by global
//!   core 1 over `(1 -> 2)`.
//!
//! `ipc_dispatchers` lists one interrupt per cross line in ascending
//! `(source, priority)` order, so `XBIN_IPC_LINE_0` wakes the `(0, 5)`
//! dispatcher and `XBIN_IPC_LINE_1` the `(1, 3)` one (M6.5-T1).
#![no_main]

use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [2],
    external_cores = [0, 1],
    ipc_dispatchers = [XBIN_IPC_LINE_0, XBIN_IPC_LINE_1]
)]
mod app {
    /// Receiver of `ConfigTask`, spawned by global core 0.
    #[sw_task(priority = 5, capacity = 1, spawn_by = 0)]
    pub struct ConfigTask;

    impl RticSwTask for ConfigTask {
        type SpawnInput = ipc_types::ConfigMsg;

        fn exec(&mut self, _input: Self::SpawnInput) {
            // The in-tree runtime harness runs the generated dispatchers.
        }
    }

    /// Receiver of `StatusTask`, spawned by global core 1.
    #[sw_task(priority = 3, capacity = 1, spawn_by = 1)]
    pub struct StatusTask;

    impl RticSwTask for StatusTask {
        type SpawnInput = ipc_types::StatusMsg;

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
