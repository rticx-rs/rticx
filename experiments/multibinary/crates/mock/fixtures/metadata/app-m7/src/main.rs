//! Producer application of the metadata fixture.
//!
//! Spawns `EncryptTask` on the M4 core (global core id 1). The producer
//! declares nothing: the driver infers the producer from the
//! receiver's `spawn_by`, and the pass generates the sender stubs in phase 2.
//!
//! `main` calls a sender stub that only exists in phase 2. The metadata sync
//! succeeds because the pass terminates the compilation right after writing its
//! manifest, before the application is type-checked.
#![allow(dead_code)]

use metadata_macro::app;

#[app(device = fixture)]
mod app {}

fn main() {
    // `EncryptTask` is generated in phase 2 from the synced system view, so it
    // does not exist during `cargo xbin sync`. This call must not be resolved
    // in phase 1: if sync ever fails here, the metadata-mode halt regressed and
    // phase 1 is type-checking the application again.
    let _ = EncryptTask::cross_spawn(());
}
