//! Linker-script generation for the rticx-stm32h7 distribution.
//!
//! `cortex-m-rt`'s `link.x` does `INCLUDE memory.x`, which the linker resolves
//! through its search path. The distribution therefore ships the two
//! core-specific layouts and, for the selected core, writes the matching one as
//! `memory.x` into this crate's `OUT_DIR` and adds that directory to the link
//! search path. Applications depending on the distribution then get the right
//! flash/RAM map and the `h7-sram3` pool bound symbols without carrying a
//! `memory.x` of their own.
//!
//! Exactly one of the `cm7` / `cm4` features must be enabled; the script
//! encodes the core's flash bank, RAM alias and IPC pool view.

use std::env;
use std::fs;
use std::path::PathBuf;

/// Cortex-M7 layout and pool bounds (see `linker/memory-cm7.x`).
const MEMORY_CM7: &str = include_str!("linker/memory-cm7.x");
/// Cortex-M4 layout and pool bounds (see `linker/memory-cm4.x`).
const MEMORY_CM4: &str = include_str!("linker/memory-cm4.x");

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=linker/memory-cm7.x");
    println!("cargo:rerun-if-changed=linker/memory-cm4.x");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_CM7");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_CM4");

    let cm7 = env::var_os("CARGO_FEATURE_CM7").is_some();
    let cm4 = env::var_os("CARGO_FEATURE_CM4").is_some();

    let script = match (cm7, cm4) {
        (true, false) => MEMORY_CM7,
        (false, true) => MEMORY_CM4,
        (true, true) => {
            panic!("rticx-stm32h7: enable at most one of the `cm7` and `cm4` features")
        }
        (false, false) => {
            panic!("rticx-stm32h7: enable exactly one of the `cm7` and `cm4` features")
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo always sets OUT_DIR"));
    let linker_dir = out_dir.join("linker");
    fs::create_dir_all(&linker_dir).expect("failed to create the linker directory");
    fs::write(linker_dir.join("memory.x"), script).expect("failed to write memory.x");

    // `link.x` resolves `INCLUDE memory.x` through the link search path; this
    // propagates through the dependency graph to the application link.
    println!("cargo:rustc-link-search={}", linker_dir.display());
}
