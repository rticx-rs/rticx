//! Runtime support for the RTICX multi-binary extension.
//!
//! Contents:
//!
//! - [`Fifo`] — the atomic SPSC ring placed at fixed addresses inside the
//!   IPC pools, with its [`Producer`]/[`Consumer`] endpoints. See
//!   [`fifo`] for the in-region image and the memory/ordering rules;
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
//!
//! Boot coordination between cores is **distribution-owned**: the runtime
//! defines no ready/epoch handshake. Each distribution guarantees that its
//! peers are booted before interrupts/IPC are used, for example through
//! `CorePassBackend::post_init` or its own pass injection points.

#![no_std]

pub mod backend;
pub mod fifo;

pub use backend::{CrossBinBackend, IpcRegion};
pub use fifo::{Consumer, FIFO_ALIGN, FIFO_HEADER, FIFO_INDEX_STRIDE, Fifo, Producer};
pub use rticx_spsc::Queue;
