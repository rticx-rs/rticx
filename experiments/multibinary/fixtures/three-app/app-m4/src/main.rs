//! Third application of the three-application fixture (M6-T1).
//!
//! Global core 1 is the busiest core: it receives from both other producers and
//! produces back to both of them.
//!
//! Receivers (this application executes them), two per source core on disjoint
//! priority lines:
//!
//! - from global core 0 over `(0 -> 1)`: `EncryptTask` (priority 3, input
//!   `EncryptReq`) and `DigestTask` (priority 5, input `DigestReq`);
//! - from global core 2 over `(2 -> 1)`: `SensorTask` (priority 4, input
//!   `SensorSample`) and `TelemetryTask` (priority 6, input `Telemetry`).
//!
//! Producers (generated stubs):
//!
//! - `AckTask` (priority 3) on global core 0 through `(1 -> 0)`;
//! - `StatusTask` (priority 3) on global core 2 through `(1 -> 2)`.
//!
//! `ipc_dispatchers` lists one interrupt per cross line in ascending
//! `(source, priority)` order, so `XBIN_IPC_LINE_0` wakes the `(0, 3)`
//! dispatcher, `XBIN_IPC_LINE_1` the `(0, 5)` one, `XBIN_IPC_LINE_2` the
//! `(2, 4)` one and `XBIN_IPC_LINE_3` the `(2, 6)` one (M6.5-T1). The lines are
//! disjoint across sources, which the pass enforces at build (M6-T1).
#![no_main]

use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [1],
    external_cores = [0, 2],
    ipc_dispatchers = [
        XBIN_IPC_LINE_0,
        XBIN_IPC_LINE_1,
        XBIN_IPC_LINE_2,
        XBIN_IPC_LINE_3
    ]
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

    /// Receiver of `DigestTask`, spawned by global core 0.
    #[sw_task(priority = 5, capacity = 1, spawn_by = 0)]
    pub struct DigestTask;

    impl RticSwTask for DigestTask {
        type SpawnInput = ipc_types::DigestReq;

        fn exec(&mut self, _input: Self::SpawnInput) {}
    }

    /// Receiver of `SensorTask`, spawned by global core 2.
    #[sw_task(priority = 4, capacity = 1, spawn_by = 2)]
    pub struct SensorTask;

    impl RticSwTask for SensorTask {
        type SpawnInput = ipc_types::SensorSample;

        fn exec(&mut self, _input: Self::SpawnInput) {}
    }

    /// Receiver of `TelemetryTask`, spawned by global core 2.
    #[sw_task(priority = 6, capacity = 2, spawn_by = 2)]
    pub struct TelemetryTask;

    impl RticSwTask for TelemetryTask {
        type SpawnInput = ipc_types::Telemetry;

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
