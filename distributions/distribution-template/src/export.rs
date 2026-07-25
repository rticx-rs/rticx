// RTIC Distribution Template -- Runtime exports
//
// This module is re-exported as `rtic::export::*` in the user's crate.
// The macro-generated code (from `rtic-macro`) references items from
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
// `rtic_spsc::Queue` which is used by the generated dispatcher code.
//
// If you add more passes (e.g. a deadline pass), add their exports too.
pub use rtic_sw_pass::export::*;

// ===========================================================================
// Items expected by the macro-generated code
// ===========================================================================
//
// The `CorePassBackend` and `SwPassBackend` implementations in
// `rtic-macro/src/lib.rs` emit token streams that reference names
// under `rtic::export::*`.  You must define or re-export every name
// that appears in your backend's generated code.
//
// Below is a comprehensive list of what the four reference
// distributions export.  Not all of them are required for every
// port -- only the ones your backend actually emits.
//
// ---- Interrupt abstraction ----
//
//     pub use <your_pac>::InterruptNumber;
//
// A trait that abstracts the interrupt type so that pend/unpend/enable
// functions are generic.
//
// Reference: cortex-m-rtic re-exports `cortex_m::interrupt::InterruptNumber`.
//            atalanta-rtic re-exports `bsp::clic::InterruptNumber`.
//            rtic-hippo re-exports its own PAC's interrupt number trait.
//
// ---- Pend / unpend / enable helpers ----
//
//     pub fn pend<I: InterruptNumber>(i: I)   { /* ... */ }
//     pub fn unpend<I: InterruptNumber>(i: I) { /* ... */ }
//
// Called by generated software-task dispatchers.
// Reference: cortex-m-rtic wraps `NVIC::pend`.
//            atalanta-rtic wraps `Clic::ip(i).pend()`.
//
// ---- Lock primitive ----
//
//     pub unsafe fn lock<T, R>(
//         ptr: *mut T,
//         /* additional args depending on your lock strategy */
//         f: impl FnOnce(&mut T) -> R,
//     ) -> R { /* ... */ }
//
// The exact signature is YOUR choice.  The `generate_resource_proxy_lock_impl`
// method in the backend must emit a call to this function with matching
// arguments.
//
// Common signatures:
//
//   BASEPRI (Cortex-M armv7-m):
//     lock(ptr, ceiling: u8, nvic_prio_bits: u8, f)
//
//   Source-mask (Cortex-M armv6-m / RP2040):
//     lock(ptr, priority: u16, ceiling: u16, masks: &[Mask<M>; 3], f)
//
//   Threshold (RISC-V mintthresh):
//     lock(ptr, priority: u8, ceiling: u8, f)
//
// Reference: cortex-m-rtic/src/export.rs::basepri, ::source_mask.
//            rp2040-rtic/src/export.rs (source-mask).
//            rtic-hippo/src/export.rs (threshold).
//            atalanta-rtic/src/export.rs (threshold).
//
// ---- Task execution wrapper ----
//
//     pub fn run<F: FnOnce()>(priority: u8, f: F) { /* ... */ }
//
// Called by `wrap_task_execution` to save/restore state around task exec.
// For source-mask and threshold locks that handle everything inside `lock`,
// this can be a no-op.
//
// Reference: cortex-m-rtic/src/export.rs::basepri::run (saves/restores BASEPRI).
//            rp2040-rtic/src/export.rs::run (no-op for source-mask).
//
// ---- Interrupt-free (critical section) ----
//
//     The `__rtic_critical_section` function body is filled by
//     `generate_interrupt_free_fn`.  The function itself is generated
//     by the core pass -- you do NOT define it here.  But the body
//     must call into your target's disable/enable mechanism.
//
// ---- Priority conversion (Cortex-M specific) ----
//
//     pub const fn cortex_logical2hw(logical: u8, nvic_prio_bits: u8) -> u8 {
//         ((1 << nvic_prio_bits) - logical) << (8 - nvic_prio_bits)
//     }
//
// Only needed for Cortex-M targets.  Maps RTIC's logical priority
// (higher = more urgent) to the hardware encoding (lower = more urgent).
//
// ---- Source-mask types (armv6-m / RP2040) ----
//
//     pub struct Mask<const M: usize>([u32; M]);
//     pub const fn create_mask<const N: usize, const M: usize>(...) -> Mask<M>;
//     pub const fn compute_mask_chunks<const L: usize>(ids: [u32; L]) -> usize;
//
// Only needed for source-mask locking.
// Reference: cortex-m-rtic/src/export.rs::source_mask,
//            rp2040-rtic/src/export.rs.
//
// ---- Cross-core helpers (multicore only) ----
//
//     pub mod cross_core {
//         pub fn pend_irq(irq: u16) -> Result<(), ...> { /* ... */ }
//         pub fn get_pended_irq() -> Option<Interrupt> { /* ... */ }
//     }
//
// Only needed when `cores > 1`.  Implements the IPI mechanism
// (FIFO, mailbox, shared memory, etc.).
// Reference: rp2040-rtic/src/export.rs::cross_core.
//
// ---- Hardware-specific re-exports ----
//
//     pub use cortex_m::peripheral::{Peripherals, NVIC, SCB, ...};
//     pub use cortex_m::asm::{nop, wfi};
//     pub use cortex_m::interrupt;
//
//     pub use riscv::interrupt::machine::{disable, enable};
//     pub use some_hal::SomeType;
//
// Any type or function that your backend's generated code references
// must be re-exported here.
