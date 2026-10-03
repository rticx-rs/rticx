//! Runtime support for the RTICX multi-binary extension.
//!
//! This crate is deliberately kept separate from `rticx-spsc`: the
//! single-binary software-task queue has frozen, non-atomic semantics, while
//! cross-core FIFOs must use atomics with `Release`/`Acquire` ordering to
//! publish data across cores.
//!
//! Contents:
//!
//! - [`CrossCoreMessage`] — marker trait implemented only by the generated
//!   `ipc-types`;
//! - [`Fifo`] — the atomic SPSC ring placed at fixed addresses inside the
//!   IPC pools, with its [`Producer`]/[`Consumer`] endpoints. See
//!   [`fifo`] for the in-region image and the memory/ordering rules;
//! - [`SharedState`] — the shared ready bitmap and epoch word used for boot
//!   coordination and peer-reset detection, plus [`ReadyCache`], the
//!   spawner-side cached-epoch check the generated `cross_spawn` gates on
//!   (M6-T2). See [`state`] for the protocol;
//! - [`Queue`] — the core-local SPSC queue behind the generated line ready
//!   queues: the target's router produces task notifications, its line
//!   dispatcher consumes them (M6.5-T3). It is the same queue type the
//!   software pass uses, re-exported so generated code reaches it through one
//!   path;
//! - [`backend`] — the [`CrossBinBackend`] contract a distribution
//!   implements for its IPC pools, core identity and cache/MPU policy; the
//!   in-tree `rticx-xbin-mock` implements it for host tests. The doorbell
//!   transport itself lives in the generated ring/read functions (M6.5), so
//!   the trait carries no per-line doorbell methods.

#![no_std]

pub mod backend;
pub mod fifo;
pub mod state;

pub use backend::{CrossBinBackend, IpcRegion};
pub use fifo::{Consumer, FIFO_ALIGN, FIFO_HEADER, FIFO_INDEX_STRIDE, Fifo, Producer};
pub use rticx_spsc::Queue;
pub use state::{MAX_CORES, ReadyCache, SharedState};

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
