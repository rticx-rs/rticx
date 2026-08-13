#![no_std]
#![allow(dead_code)]

//! Async runtime primitives for RTICX: executor slots, channels, wakers,
//! and wait queues used by `rticx-async-pass`.
//!
//! # Atomics and critical sections
//!
//! This crate is **not** single-core-only: on multicore targets a future's
//! `set_pending()` may be called from a different core than the one that
//! polls it. It therefore relies on two synchronization primitives:
//!
//! - **Atomics** via [`portable_atomic`] — the `ExecSlot` `running`/`pending`
//!   flags and `ExecSlotPtr`.
//! - **Critical sections** via [`critical_section`] — channel queues, wait
//!   queues, waker registration, and the `make_channel!` one-shot guard.
//!
//! ## Targets without native atomics
//!
//! On targets without native atomic instructions (Cortex-M0/M0+ / ARMv6-M,
//! RISC-V without the "A" extension) the distribution must enable the
//! `atomic-critical-section` feature, which routes every atomic operation
//! through a critical section:
//!
//! ```toml
//! [features]
//! async = ["rticx-async/atomic-critical-section"]
//! ```
//!
//! ## Critical-section backend
//!
//! `critical-section` requires exactly one backend linked into the final
//! binary; the distribution (or its HAL) must provide it:
//!
//! - **Single-core**: interrupt-disable is sufficient (e.g.
//!   `cortex-m/critical-section-single-core`,
//!   `riscv/critical-section-single-hart`).
//! - **Multicore**: interrupt-disable is **not** enough. Use a multicore-aware
//!   backend such as `rp2040-hal/critical-section-impl` (hardware spinlocks).
//!
//! A multicore distribution must enable *both* the atomic fallback and a
//! spinlock backend:
//!
//! ```toml
//! [features]
//! async = ["rticx-async/atomic-critical-section", "rp2040-hal/critical-section-impl"]
//! ```

pub mod channel;
mod dropper;
pub mod executor;
mod wait_queue;
mod waker_registration;

pub use portable_atomic;

#[macro_export]
macro_rules! make_channel {
    ($type:ty, $size:expr) => {{
        static mut CHANNEL: $crate::channel::Channel<$type, $size> =
            $crate::channel::Channel::new();

        static CHECK: $crate::portable_atomic::AtomicU8 = $crate::portable_atomic::AtomicU8::new(0);

        $crate::channel::critical_section::with(|_| {
            if CHECK.load($crate::portable_atomic::Ordering::Relaxed) != 0 {
                ::core::panic!("call to the same `make_channel` instance twice");
            }
            CHECK.store(1, $crate::portable_atomic::Ordering::Relaxed);
        });

        #[allow(static_mut_refs)]
        unsafe {
            CHANNEL.split()
        }
    }};
}
