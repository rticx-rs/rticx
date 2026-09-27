//! Sender application of the end-to-end fixture (M4-T1).
//!
//! Global core 0 spawns `EncryptTask` on the M4 binary (global core 1) through
//! the cross-binary pass; the input type is the `EncryptReq` IDL message.
//!
//! The mock distribution provides the `#[app]` macro and the in-process
//! backend, so the application builds and links on the host. The generated
//! `EncryptTask::cross_spawn` lands in the app module; M4-T2 drives it against
//! the receiver.
#![no_main]

use xbin_mock_distro::app;

#[app(device = mock_pac, cores = 1, core_ids = [0], external_cores = [1])]
mod app {
    /// Sender stub for the receiver declared by `app-m4`. The input type comes
    /// from `ipc-types` through the cross-binary pass.
    #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
    pub struct EncryptTask;

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
