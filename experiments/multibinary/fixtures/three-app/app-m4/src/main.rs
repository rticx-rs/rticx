//! Receiver application of the three-application fixture (M6-T1).
//!
//! Global core 1 executes two cross-binary receivers, each fed by a different
//! producer core through its own region, priority line and per-pair router:
//!
//! - `EncryptTask` (priority 3) is spawned by global core 0 over `(0 -> 1)`;
//! - `SensorTask` (priority 4) is spawned by global core 2 over `(2 -> 1)`.
//!
//! `ipc_dispatchers` lists one interrupt per cross line in ascending
//! `(source, priority)` order, so `XBIN_IPC_LINE_0` wakes the `(0, 3)`
//! dispatcher and `XBIN_IPC_LINE_1` the `(2, 4)` one (M6.5-T1). The lines are
//! disjoint, which the pass enforces at build (M6-T1).
#![no_main]

use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [1],
    external_cores = [0, 2],
    ipc_dispatchers = [XBIN_IPC_LINE_0, XBIN_IPC_LINE_1]
)]
mod app {
    /// Receiver of `EncryptTask`, spawned by global core 0.
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    pub struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = ipc_types::EncryptReq;

        fn exec(&mut self, _input: Self::SpawnInput) {
            // The in-tree runtime harness runs the generated dispatchers.
        }
    }

    /// Receiver of `SensorTask`, spawned by global core 2.
    #[sw_task(priority = 4, capacity = 1, spawn_by = 2)]
    pub struct SensorTask;

    impl RticSwTask for SensorTask {
        type SpawnInput = ipc_types::EncryptReq;

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
