//! Runtime support for the RTICX multi-binary extension.
//!
//! This crate is deliberately kept separate from [`rticx_spsc`]: the
//! single-binary software-task queue has frozen, non-atomic semantics, while
//! cross-core FIFOs must use atomics with `Release`/`Acquire` ordering to
//! publish data across cores.
//!
//! Milestone M0 only needs the [`CrossCoreMessage`] marker so that the
//! generated `ipc-types` crate can compile. The atomic SPSC ring, FIFO views
//! and ready/epoch helpers are implemented in M2.

#![no_std]

/// Marker trait for data types that may travel through cross-core shared
/// memory.
///
/// The generated `ipc-types` crate implements this trait for every IDL type
/// after asserting its canonical layout at compile time. It is intentionally
/// **not** blanket-implemented: types must be explicitly reviewed as
/// cross-core safe (plain data, no pointers/references, fixed layout).
///
/// # Safety
///
/// Implementors must:
///
/// - be `Copy` and have no interior mutability or destructor;
/// - have a stable, target-independent layout that matches the canonical
///   layout encoded by the generated `SIZE_*` / `ALIGN_*` / `OFF_*` constants;
/// - not contain pointers, references or other core-local state;
/// - not be larger than the FIFO element size they are stored in.
pub unsafe trait CrossCoreMessage: Copy + 'static {}
