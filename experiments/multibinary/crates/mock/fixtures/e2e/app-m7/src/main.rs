//! Producer application of the end-to-end fixture (M4-T1, M5.5).
//!
//! Global core 0 spawns `EncryptTask` on the M4 binary (global core 1) through
//! the cross-binary pass; the input type is the `EncryptReq` IDL message.
//!
//! The producer declares nothing: the pass generates the `EncryptTask`
//! sender stub and its `cross_spawn` from the synced system view (M5.5). The
//! mock distribution provides the `#[app]` macro and the in-process backend,
//! so the application builds and links on the host; M4-T2 drives the generated
//! API against the receiver.
#![no_main]

use xbin_mock_distro::app;

#[app(device = mock_pac, cores = 1, core_ids = [0], external_cores = [1])]
mod app {
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
