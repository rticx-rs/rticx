// SLIC-based RISC-V distribution: rely on the `riscv-slic` crate for the
// runtime. The upstream exports re-export the SLIC primitives verbatim,
// and define a small `interrupt` shim mirroring the cortex-m layout.
//
// Upstream: `upstream/exports/slic.rs`.
pub use riscv_slic::{self, codegen, lock, pend, run, set_priority, InterruptNumber};

pub mod interrupt {
    #[inline]
    pub fn disable() {
        riscv_slic::disable();
    }

    /// # Safety
    ///
    /// Caller must ensure the SLIC interrupt controller is initialized.
    #[inline]
    pub unsafe fn enable() {
        unsafe { riscv_slic::enable() };
    }
}
