#![no_std]

// Distribution library crate.
//
// Every RTIC distribution follows this minimal structure:
//   * `export` -- runtime helpers re-exported as `rtic::export::*`.
//   * `app`    -- the proc-macro attribute re-exported from the inner
//                 proc-macro crate.
//
// See README.md for the porting checklist.

pub mod export;

pub use rtic_macro::app;
