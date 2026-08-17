//! Shared infrastructure for the software-task compilation passes
//! (`rticx-sw-pass` and `rticx-async-pass`).
//!
//! `rticx-sw-pass` is the base crate for generic software-task machinery;
//! `rticx-async-pass` extends it and consumes these modules.  Everything here
//! is pass-internal plumbing, **not** a stable public API.

pub mod analyze;
pub mod codegen;
pub mod parse;
