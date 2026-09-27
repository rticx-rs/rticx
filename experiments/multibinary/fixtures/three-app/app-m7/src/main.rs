//! First producer application of the three-application fixture (M6-T1).
//!
//! Global core 0 spawns `EncryptTask` on the M4 binary (global core 1) at
//! priority 3, through the `(0 -> 1)` region. The producer declares nothing:
//! the pass generates the `EncryptTask` sender stub and its `cross_spawn` from
//! the synced system view (M5.5). The mock distribution provides the `#[app]`
//! macro and the in-process backend, so the application builds and links on
//! the host; the in-tree runtime harness drives the generated API.
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
