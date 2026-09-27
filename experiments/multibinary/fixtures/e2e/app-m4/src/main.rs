//! Receiver application of the end-to-end fixture (M4-T1).
//!
//! Global core 1 executes `EncryptTask` for spawns from the M7 binary (global
//! core 0). The cross-binary pass rewrites the receiver struct into a core
//! `#[task(.., task_trait = CrossBinTask)]` and generates one doorbell
//! dispatcher per `(source -> target, priority)` line, which drains the task
//! FIFO and calls `exec`.
#![no_main]

use xbin_mock_distro::app;

#[app(device = mock_pac, cores = 1, core_ids = [1], external_cores = [0])]
mod app {
    use xbin_mock_runtime::CrossBinTask;

    /// Receiver task: one spawn input per execution, `EncryptReq` from the
    /// shared IDL.
    #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
    pub struct EncryptTask;

    impl CrossBinTask for EncryptTask {
        type Input = ipc_types::EncryptReq;

        fn exec(&mut self, _input: Self::Input) {
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
