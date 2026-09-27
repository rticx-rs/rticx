//! Receiver application of the end-to-end fixture (M4-T1, M5.5).
//!
//! Global core 1 executes `EncryptTask` for spawns from the M7 binary (global
//! core 0). The receiver is a native `#[sw_task]` plus `impl RticSwTask`; the
//! cross-binary pass rewrites the struct into a core
//! `#[task(.., task_trait = RticSwTask)]` and generates one doorbell
//! dispatcher per `(source -> target, priority)` line, which drains the task
//! FIFO and calls `exec`.
#![no_main]

use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [1],
    external_cores = [0],
    ipc_dispatchers = [XBIN_IPC_LINE_0]
)]
mod app {
    use xbin_mock_runtime::RticSwTask;

    /// Receiver task: one spawn input per execution, `EncryptReq` from the
    /// shared IDL.
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    pub struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = ipc_types::EncryptReq;

        fn exec(&mut self, _input: Self::SpawnInput) {
            // M4-T2 runs this through the generated doorbell dispatcher.
        }
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
