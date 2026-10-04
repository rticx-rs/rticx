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
//!   path.
//!
//! This crate is a pure shared-memory data-structure runtime: the FIFO image
//! and the ready-queue type. The distribution contract that used to live here
//! (pools, core identity and cache/MPU policy) is a **compile-time** concern,
//! carried by `rticx_xbin_pass::XbinPassBackend`. That binding bakes the
//! `system.json` addresses and the executing core id into the generated code,
//! so there is no runtime backend value to thread through. Host/mock backends
//! whose pools are not at the synced addresses resolve one at runtime through
//! `XbinPassBackend::ipc_base_override` instead.
//!
//! The doorbell transport is likewise generated: the
//! `__rticx_xbin_ring_{source}_{target}` / `__rticx_xbin_read_{source}_{target}`
//! bodies are emitted by the pass from the `XbinPassBackend` bindings (M6.5),
//! so this crate carries no doorbell methods.
//!
//! Boot coordination between cores is **distribution-owned**: the runtime
//! defines no ready/epoch handshake. Each distribution guarantees that its
//! peers are booted before interrupts/IPC are used, for example through
//! `CorePassBackend::post_init` or its own pass injection points.

#![no_std]

pub mod fifo;

pub use fifo::{Consumer, FIFO_ALIGN, FIFO_HEADER, FIFO_INDEX_STRIDE, Fifo, Producer};
pub use rticx_spsc::Queue;
