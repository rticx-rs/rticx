//! Second producer application of the three-application fixture (M6-T1).
//!
//! Global core 2 spawns `SensorTask` on the M4 binary (global core 1) at
//! priority 4, through its own `(2 -> 1)` region and doorbell router. Like the
//! other producer it declares nothing and only depends on the generated
//! `ipc-types` crate (M5.5); its priority line is disjoint from the first
//! producer's, so both can drive the receiver without sharing a dispatcher.
#![no_main]

use xbin_mock_distro::app;

#[app(device = mock_pac, cores = 1, core_ids = [2], external_cores = [1])]
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
