//! Receiver application of the metadata fixture (M5.5).
//!
//! Executes `EncryptTask` for the M7 core (global core id 0) through the
//! native `#[sw_task]` syntax. The local `RticSwTask` trait stands in for the
//! one `rticx-sw-pass` provides in a real distribution.
#![allow(dead_code)]

use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [1], external_cores = [0])]
mod app {
    trait RticSwTask {
        type SpawnInput;
        fn exec(&mut self, input: Self::SpawnInput);
    }

    /// Stand-in for the generated `ipc_types::EncryptReq` (IDL: `ipc-types.toml`).
    struct EncryptReq;

    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = EncryptReq;
        fn exec(&mut self, _input: Self::SpawnInput) {}
    }
}

fn main() {}
