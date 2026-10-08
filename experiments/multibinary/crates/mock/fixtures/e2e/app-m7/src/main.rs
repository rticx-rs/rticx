//! Producer application of the end-to-end fixture.
//!
//! Global core 0 spawns `EncryptTask` on the M4 binary (global core 1) through
//! the cross-binary pass; the input type is the `EncryptReq` IDL message.
//!
//! The producer declares nothing: the pass generates the `EncryptTask`
//! sender stub and its `cross_spawn` from the synced system view. The
//! mock distribution provides the `#[app]` macro and the in-process backend,
//! so the application builds and links on the host; the end-to-end driver test
//! drives the generated API against the receiver.
#![no_main]

use xbin_mock_distro::app;

#[app(device = mock_pac)]
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
