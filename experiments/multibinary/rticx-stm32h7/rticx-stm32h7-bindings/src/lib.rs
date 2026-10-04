//! Shared STM32H7 dual-core geometry for the `rticx-stm32h7` distribution.
//!
//! The distribution and its proc-macro crate must agree, to the byte, on where
//! the cross-binary IPC pool, the shared ready/epoch state and the doorbell
//! words live. The distribution (`rticx-stm32h7`) implements the runtime half
//! (`rticx_xbin_rt::CrossBinBackend`); the proc-macro crate
//! (`rticx-stm32h7-macro`) reports the same pool through the compile-time
//! capability binding (`XbinPassBackend::ipc_pools`). Neither can depend on the
//! other — the distribution depends on the macro, and a proc-macro crate cannot
//! export normal items — so the distro-specific IPC geometry lives here, in a
//! small `no_std` crate both sides link.
//!
//! Only the pool geometry and the IPC protocol vocabulary (physical core ids,
//! HSEM semaphore indices and `COREID`s) live here. All memory-mapped register
//! access (HSEM, RCC, USART) goes through the `stm32h7` PAC, so this crate
//! holds no hardware addresses.
//!
//! The layout of the reserved D2 SRAM3 block (32 KiB, `0x3004_0000` from the
//! Cortex-M7 and its `0x1004_0000` alias from the Cortex-M4):
//!
//! ```text
//! SRAM3 + 0x0000   shared ready/epoch state (`rticx_xbin_rt::SharedState`)
//! SRAM3 + 0x0200   doorbell words, one per `(source, target)` direction
//! SRAM3 + 0x1000   IPC pool, both directions, `POOL_BUDGET` bytes
//! ```
//!
//! The pool occupies `[SRAM3 + 0x1000, SRAM3 + 0x1000 + POOL_BUDGET)`; the
//! linker scripts reserve it and export the `__rticx_xbin_pool_<id>_start/_end`
//! symbols the `cargo xbin` ELF verifier checks (M6.9-T7).

#![no_std]

/// Distro physical-core id of the Cortex-M7 (the boot core / project owner).
pub const PHYSICAL_CM7: u32 = 0;
/// Distro physical-core id of the Cortex-M4.
pub const PHYSICAL_CM4: u32 = 1;

/// Distro-defined symbolic id of the single `{M7, M4}` IPC pool (the dual).
pub const POOL_ID: &str = "h7-sram3";

/// Bytes reserved for both directions of the dual.
pub const POOL_BUDGET: u32 = 4096;

/// D2 SRAM3 as seen from the Cortex-M7.
pub const SRAM3_FROM_CM7: u32 = 0x3004_0000;
/// D2 SRAM3 as seen from the Cortex-M4 (hardware alias of the same memory).
pub const SRAM3_FROM_CM4: u32 = 0x1004_0000;

/// Offset of the shared ready/epoch state inside SRAM3.
pub const SHARED_STATE_OFFSET: u32 = 0x0000;
/// Offset of the doorbell words inside SRAM3.
pub const DOORBELL_OFFSET: u32 = 0x0200;
/// Offset of the IPC pool inside SRAM3.
pub const POOL_OFFSET: u32 = 0x1000;

/// HSEM `COREID` of the Cortex-M7 (matches RM0399 and the Renode model).
pub const HSEM_COREID_CM7: u32 = 0x3;
/// HSEM `COREID` of the Cortex-M4 (matches RM0399 and the Renode model).
pub const HSEM_COREID_CM4: u32 = 0x1;

/// HSEM semaphore the Cortex-M7 receives its doorbell on (M4 -> M7).
pub const RX_SEM_CM7: u32 = 0;
/// HSEM semaphore the Cortex-M4 receives its doorbell on (M7 -> M4).
pub const RX_SEM_CM4: u32 = 1;

/// Number of physical cores in the dual (used to index the doorbell words).
pub const MAX_PHYSICAL_CORES: usize = 2;

/// The IPC pool of one endpoint of the dual, as plain geometry.
///
/// The proc-macro crate converts this into the richer
/// `rticx_xbin_pass::IpcPool` binding; the runtime uses the same numbers
/// directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolGeometry {
    /// Symbolic pool id (the dual's matching key).
    pub id: &'static str,
    /// The other endpoint of the dual.
    pub peer: u32,
    /// This core's base view of the pool (FIFO area).
    pub base_local: u32,
    /// The peer's base view of the pool.
    pub base_peer: u32,
    /// Bytes shared by both directions of the dual.
    pub budget: u32,
}

/// Returns the dual's pool geometry as seen from `physical`.
///
/// # Panics
///
/// Panics when `physical` is not one of [`PHYSICAL_CM7`] / [`PHYSICAL_CM4`].
pub const fn pool_for(physical: u32) -> PoolGeometry {
    match physical {
        PHYSICAL_CM7 => PoolGeometry {
            id: POOL_ID,
            peer: PHYSICAL_CM4,
            base_local: SRAM3_FROM_CM7 + POOL_OFFSET,
            base_peer: SRAM3_FROM_CM4 + POOL_OFFSET,
            budget: POOL_BUDGET,
        },
        PHYSICAL_CM4 => PoolGeometry {
            id: POOL_ID,
            peer: PHYSICAL_CM7,
            base_local: SRAM3_FROM_CM4 + POOL_OFFSET,
            base_peer: SRAM3_FROM_CM7 + POOL_OFFSET,
            budget: POOL_BUDGET,
        },
        _ => panic!("unknown STM32H7 physical core"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two endpoints report the same pool from their own side: the same
    /// id and budget, opposite peers and swapped base views.
    #[test]
    fn pool_is_symmetric_across_endpoints() {
        let cm7 = pool_for(PHYSICAL_CM7);
        let cm4 = pool_for(PHYSICAL_CM4);

        assert_eq!(cm7.id, POOL_ID);
        assert_eq!(cm4.id, POOL_ID);
        assert_eq!(cm7.peer, PHYSICAL_CM4);
        assert_eq!(cm4.peer, PHYSICAL_CM7);
        assert_eq!(cm7.base_local, cm4.base_peer);
        assert_eq!(cm7.base_peer, cm4.base_local);
        assert_eq!(cm7.budget, POOL_BUDGET);
        assert_eq!(cm4.budget, POOL_BUDGET);
    }

    /// The reserved control area never overlaps the pool.
    #[test]
    fn control_area_precedes_the_pool() {
        assert!(DOORBELL_OFFSET + MAX_PHYSICAL_CORES as u32 * 4 <= POOL_OFFSET);
        assert!(SHARED_STATE_OFFSET + 12 <= DOORBELL_OFFSET);
    }

    /// The pool fits the reserved SRAM3 block (32 KiB).
    #[test]
    fn pool_fits_sram3() {
        assert!(POOL_OFFSET + POOL_BUDGET <= 0x8000);
    }
}
