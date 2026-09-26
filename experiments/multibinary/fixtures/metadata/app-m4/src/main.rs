//! Receiver application of the metadata fixture.
//!
//! Executes `EncryptTask` for the M7 core (global core id 0). The local
//! `CrossBinTask` trait stands in for the one the runtime crate will provide.
#![allow(dead_code)]

use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [1], external_cores = [0])]
mod app {
    trait CrossBinTask {
        type Input;
        fn exec(&mut self, input: Self::Input);
    }

    /// Stand-in for the generated `ipc_types::EncryptReq` (IDL: `ipc-types.toml`).
    struct EncryptReq;

    #[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
    struct EncryptTask;

    impl CrossBinTask for EncryptTask {
        type Input = EncryptReq;
        fn exec(&mut self, _input: Self::Input) {}
    }
}

fn main() {}
