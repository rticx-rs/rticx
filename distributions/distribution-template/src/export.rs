// RTICX Distribution Template -- Runtime exports
//
// This module is re-exported as `<your-crate>::export::*` in the user's crate.
// The macro-generated code (from your proc-macro crate) references items from
// this module.  You MUST export every item your backend codegen emits.
//
// Currently only the software-tasks pass has runtime items that must
// be re-exported.  All other items are target-specific and will be
// added by you during the port.

#![allow(clippy::inline_always)]

// ===========================================================================
// Compilation pass re-exports (mandatory)
// ===========================================================================
//
// Every compilation pass your distribution enables MUST have its
// runtime items re-exported here.  The software-tasks pass provides
// `rticx_spsc::Queue` which is used by the generated dispatcher code.
//
// If you add more passes (e.g. a deadline pass), add their exports too.
pub use rticx_sw_pass::export::*;

// ===========================================================================
// Items expected by the macro-generated code
// ===========================================================================
//
// The `CorePassBackend` and `SwPassBackend` implementations in
// `<your-distro>-macro/src/lib.rs` emit token streams that reference names
// under `<your-crate>::export::*`.  You must define or re-export every name
// that appears in your backend's generated code.
