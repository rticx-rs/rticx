#![no_std]

// Distribution library crate.
//
// Every RTIC distribution follows this minimal structure:
//   * `export` -- runtime helpers re-exported as `<your-crate>::export::*`.
//   * `app`    -- the proc-macro attribute re-exported from the inner
//                 proc-macro crate.
//
// When copying this template, replace `distribution-template` and
// `distribution-template-macro` with your own names everywhere.

pub mod export;

pub use distribution_template_macro::app;
