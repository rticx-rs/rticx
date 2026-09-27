//! Producer application of the metadata fixture (M5.5).
//!
//! Spawns `EncryptTask` on the M4 core (global core id 1). Since M5.5 the
//! producer declares nothing: the driver infers the producer from the
//! receiver's `spawn_by`, and the pass generates the sender stubs in phase 2.
#![allow(dead_code)]

use metadata_macro::app;

#[app(device = fixture, cores = 1, core_ids = [0], external_cores = [1])]
mod app {}

fn main() {}
