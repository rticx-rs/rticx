//! Sender application of the metadata fixture.
//!
//! Spawns `EncryptTask` on the M4 core (global core id 1).
#![allow(dead_code)]

use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [0], external_cores = [1])]
mod app {
    #[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
    struct EncryptTask;
}

fn main() {}
