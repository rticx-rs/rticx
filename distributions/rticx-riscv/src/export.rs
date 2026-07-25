// Re-exports required by the core pass and generated code.
//
// Users of this distribution should add target-specific dependencies
// (e.g. `riscv`, `riscv-rt`, a PAC crate) and extend this module with
// the exports their target requires.

/// Distribution crate must re-export the `export` module from all the used compilation passes
pub use rticx_sw_pass::export::*;
