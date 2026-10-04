//! STM32H7 runtime half of the cross-binary distribution contract.
//!
//! [`Backend`] implements [`rticx_xbin_rt::CrossBinBackend`] for the single
//! `{Cortex-M7, Cortex-M4}` dual:
//!
//! - **Shared memory.** Both directions share the D2 SRAM3 pool whose geometry
//!   is reported by the compile-time capability binding
//!   (`rticx-stm32h7-bindings`). The M7 sees it at `0x3004_0000` and the M4
//!   through the hardware alias `0x1004_0000`; the FIFO area is at `+ 0x1000`,
//!   the ready/epoch state at `+ 0x0000` and the doorbell words at `+ 0x0200`.
//!   MPU region 15 (M7) / 7 (M4) maps the whole 32 KiB block Normal,
//!   Non-cacheable, Shareable, so the runtime's atomics are correct on silicon;
//!   Renode does not model the D-cache, so a green run validates the transport,
//!   not the cache policy. All HSEM/RCC register access goes through the
//!   `stm32h7` PAC (selected by the `cm7`/`cm4` feature), not raw addresses.
//! - **Boot release.** The M7 owns the shared state: [`CrossBinBackend::init_shared`]
//!   initializes it and *then* writes `RCC_GCR.BOOT_C2`, releasing the M4 from
//!   hold-boot. The M4 boots from flash bank 2 once released and only marks
//!   itself ready.
//! - **Doorbells.** The generated ring/read functions call [`doorbell_send`] /
//!   [`doorbell_take`]: a per-`(source, target)` shared atomic word carries the
//!   task id and the HSEM block raises the target's interrupt (`HSEM0` IRQ 125
//!   on the M7, `HSEM1` IRQ 126 on the M4). The M7 receives on HSEM semaphore 0
//!   and the M4 on semaphore 1; these two interrupts are reserved by the
//!   distribution and rejected for user tasks/dispatchers.
//!
//! The runtime trait carries no doorbell methods; the generated code is the
//! transport, and this module provides its H7 body.

use core::sync::atomic::Ordering;

use portable_atomic::AtomicU32;
use rticx_stm32h7_bindings as bindings;
use rticx_xbin_rt::SharedState;
use rticx_xbin_rt::backend::{CrossBinBackend, IpcRegion};

/// `stm32h7` PAC device module for this binary's core, selected by the distro
/// feature. All memory-mapped register access below goes through it.
#[cfg(feature = "cm4")]
use stm32h7::stm32h747cm4 as pac;
/// `stm32h7` PAC device module for this binary's core, selected by the distro
/// feature. All memory-mapped register access below goes through it.
#[cfg(feature = "cm7")]
use stm32h7::stm32h747cm7 as pac;

/// This binary's physical core id (global core id in the project view).
#[cfg(feature = "cm7")]
pub const CURRENT_CORE: u32 = bindings::PHYSICAL_CM7;
/// This binary's physical core id (global core id in the project view).
#[cfg(feature = "cm4")]
pub const CURRENT_CORE: u32 = bindings::PHYSICAL_CM4;

/// This core's view of the reserved D2 SRAM3 block.
#[cfg(feature = "cm7")]
const SRAM3_BASE: u32 = bindings::SRAM3_FROM_CM7;
/// This core's view of the reserved D2 SRAM3 block.
#[cfg(feature = "cm4")]
const SRAM3_BASE: u32 = bindings::SRAM3_FROM_CM4;

/// HSEM receive semaphore: the Cortex-M7 receives on semaphore 0 (M4 -> M7).
#[cfg(feature = "cm7")]
const RX_SEM: u32 = bindings::RX_SEM_CM7;
/// HSEM receive semaphore: the Cortex-M4 receives on semaphore 1 (M7 -> M4).
#[cfg(feature = "cm4")]
const RX_SEM: u32 = bindings::RX_SEM_CM4;

/// HSEM `COREID` value used in the lock/release protocol.
#[cfg(feature = "cm7")]
const HSEM_COREID: u32 = bindings::HSEM_COREID_CM7;
/// HSEM `COREID` value used in the lock/release protocol.
#[cfg(feature = "cm4")]
const HSEM_COREID: u32 = bindings::HSEM_COREID_CM4;

/// MPU region dedicated to the shared SRAM3 block (M7 has 16 regions).
#[cfg(feature = "cm7")]
const MPU_REGION: u32 = 15;
/// MPU region dedicated to the shared SRAM3 block (M4 has 8 regions).
#[cfg(feature = "cm4")]
const MPU_REGION: u32 = 7;

/// `RASR.SIZE` encoding of the 32 KiB SRAM3 block (`log2(32K) - 1`).
const MPU_RASR_SIZE: u32 = 14;

/// The HSEM register block (doorbell semaphores).
#[inline(always)]
fn hsem() -> &'static pac::hsem::RegisterBlock {
    // SAFETY: HSEM is memory-mapped and the distribution reserves the doorbell
    // semaphores for the cross-binary transport.
    unsafe { &*pac::HSEM::ptr() }
}

/// Clears this core's pending HSEM interrupt-status bit for semaphore `sem`.
///
/// Each core owns a separate interrupt bank (`C1ICR` on the M7, `C2ICR` on the
/// M4); clearing the local copy is what deasserts this core's HSEM line.
#[inline(always)]
fn hsem_clear_status(sem: u32) {
    let hsem = hsem();
    #[cfg(feature = "cm7")]
    hsem.c1icr().write(|w| w.isc(sem as u8).set_bit());
    #[cfg(feature = "cm4")]
    hsem.c2icr().write(|w| w.isc(sem as u8).set_bit());
}

/// The HSEM receive semaphore of the physical core `target`.
#[inline]
const fn rx_sem_of(target: u32) -> u32 {
    if target == bindings::PHYSICAL_CM7 {
        bindings::RX_SEM_CM7
    } else {
        bindings::RX_SEM_CM4
    }
}

/// Returns the shared atomic word of the `(source -> target)` doorbell.
#[inline]
fn doorbell_word(source: u32, target: u32) -> *const AtomicU32 {
    debug_assert!(
        (source as usize) < bindings::MAX_PHYSICAL_CORES
            && (target as usize) < bindings::MAX_PHYSICAL_CORES
    );
    let base = (SRAM3_BASE + bindings::DOORBELL_OFFSET) as *const AtomicU32;
    // SAFETY: `source`/`target` are validated physical-core ids and the
    // `MAX_PHYSICAL_CORES^2` words live in the reserved SRAM3 control area.
    unsafe { base.add((source as usize) * bindings::MAX_PHYSICAL_CORES + target as usize) }
}

/// Publishes `task_id` to the `(source -> target)` doorbell and signals the
/// target through HSEM.
///
/// Called by the generated `__rticx_xbin_ring_{source}_{target}` on the source
/// core. The target's router reads the word with [`doorbell_take`]. The word
/// carries a single id, so a producer that rings twice before the target runs
/// coalesces them; the target's line dispatcher drains its FIFOs until empty,
/// which makes a lost duplicate id harmless (M6.5-T3).
pub fn doorbell_send(source: u32, target: u32, task_id: u32) {
    let word = doorbell_word(source, target);
    // SAFETY: this core is the single producer for the pair's word.
    unsafe { (*word).store(task_id, Ordering::Release) };
    hsem_signal(rx_sem_of(target));
}

/// Takes the next pending task id of the `(source -> target)` doorbell, or
/// `None` when it is empty.
///
/// Called by the generated `__rticx_xbin_read_{source}_{target}` on the target
/// core; also clears this core's HSEM interrupt status bit so the doorbell line
/// deasserts once the router drained it.
pub fn doorbell_take(source: u32, target: u32) -> Option<u32> {
    // Clear the HSEM status *before* reading the word: a signal that arrives
    // after the clear re-raises the interrupt, so the router re-enters and sees
    // the new id instead of a lost wakeup. Reading first and clearing after
    // would drop a signal that lands between the two.
    hsem_ack();
    let word = doorbell_word(source, target);
    // SAFETY: this core is the single consumer for the pair's word.
    let task_id = unsafe { (*word).swap(0, Ordering::AcqRel) };
    (task_id != 0).then_some(task_id)
}

/// Takes and releases HSEM semaphore `sem`, raising the receiving core's
/// interrupt.
///
/// The take-then-release sequence is the one ST firmware uses to release the
/// Cortex-M4 at boot (see the upstream ping-pong demo): the release sets the
/// semaphore's interrupt status bit in both core banks, and the releasing core
/// clears its own copy so only the peer's bank stays pending.
fn hsem_signal(sem: u32) {
    let hsem = hsem();
    let coreid = HSEM_COREID as u8;
    // 2-step take: LOCK bit set, ours if it was free.
    hsem.r(sem as usize)
        .write(|w| unsafe { w.coreid().bits(coreid).lock().set_bit() });
    while hsem.r(sem as usize).read().coreid().bits() != coreid {
        core::hint::spin_loop();
    }
    // Release: clears the lock and raises the interrupt status bits.
    hsem.r(sem as usize)
        .write(|w| unsafe { w.coreid().bits(coreid).lock().clear_bit() });
    // Drop our own copy of the status bit: only the peer should wake.
    hsem_clear_status(sem);
}

/// Clears this core's pending HSEM doorbell status bit.
fn hsem_ack() {
    hsem_clear_status(RX_SEM);
}

/// Enables this core's HSEM interrupt for its receive semaphore.
fn hsem_enable_rx() {
    let hsem = hsem();
    #[cfg(feature = "cm7")]
    hsem.c1ier().modify(|_, w| w.ise(RX_SEM as u8).set_bit());
    #[cfg(feature = "cm4")]
    hsem.c2ier().modify(|_, w| w.ise(RX_SEM as u8).set_bit());
    hsem_ack();
}

/// Maps the reserved SRAM3 block Normal, Non-cacheable, Shareable through the
/// MPU.
///
/// Exclusive accesses (`ldrex`/`strex`) are only valid on Normal memory, and
/// the runtime's atomics provide ordering, not cache maintenance, so a
/// cacheable mapping would need explicit clean/invalidate around every shared
/// access. `PRIVDEFENA` keeps the default memory map for everything else
/// (flash, private RAM), so enabling the MPU does not disturb the rest of the
/// application.
fn configure_mpu() {
    // SAFETY: writing the MPU registers from privileged code; region
    // `MPU_REGION` is reserved by the distribution for the IPC pool.
    unsafe {
        let mpu = cortex_m::peripheral::MPU::PTR;
        // RBAR: aligned base, REGION field, VALID bit.
        let rbar = (SRAM3_BASE & !0x1F) | MPU_REGION | (1 << 4);
        // RASR: ENABLE | SIZE | S(shareable, bit 18) | TEX=001 (bit 19) |
        // AP=011 full access (bits 24..26).
        let rasr = 1 | (MPU_RASR_SIZE << 1) | (1 << 18) | (1 << 19) | (3 << 24);
        (*mpu).rnr.write(MPU_REGION);
        (*mpu).rbar.write(rbar);
        (*mpu).rasr.write(rasr);
        // ENABLE | PRIVDEFENA.
        (*mpu).ctrl.write((1 << 0) | (1 << 2));
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
}

/// Reinitializes the doorbell words (owner core only; shared state is reset
/// separately).
fn clear_doorbells() {
    for source in 0..bindings::MAX_PHYSICAL_CORES as u32 {
        for target in 0..bindings::MAX_PHYSICAL_CORES as u32 {
            let word = doorbell_word(source, target);
            // SAFETY: the owner core runs before any peer can ring.
            unsafe { (*word).store(0, Ordering::Relaxed) };
        }
    }
}

/// Releases the Cortex-M4 from hold-boot by setting `RCC_GCR.BOOT_C2`.
///
/// Only the Cortex-M7 (the project owner) has anything to release; the call is
/// compiled out of the M4 binary.
#[cfg(feature = "cm7")]
fn release_secondary_core() {
    // SAFETY: RCC is memory-mapped; setting BOOT_C2 is the documented handshake
    // that releases the Cortex-M4 from hold-boot.
    let rcc = unsafe { &*pac::RCC::ptr() };
    rcc.gcr().modify(|_, w| w.boot_c2().set_bit());
}

/// The Cortex-M4 is the released secondary core: it never releases a peer.
#[cfg(feature = "cm4")]
fn release_secondary_core() {}

/// Zero-sized runtime backend of the STM32H7 distribution.
///
/// The generated code constructs it with `rticx_stm32h7::xbin::Backend` (see
/// the macro's `XbinPassBackend::backend`).
pub struct Backend;

impl CrossBinBackend for Backend {
    fn current_global_core_id(&self) -> u32 {
        CURRENT_CORE
    }

    fn ipc_region(&self, source: u32, target: u32) -> Option<IpcRegion> {
        let cm7_to_cm4 = source == bindings::PHYSICAL_CM7 && target == bindings::PHYSICAL_CM4;
        let cm4_to_cm7 = source == bindings::PHYSICAL_CM4 && target == bindings::PHYSICAL_CM7;
        if !cm7_to_cm4 && !cm4_to_cm7 {
            return None;
        }

        let view = |core: u32| {
            if core == bindings::PHYSICAL_CM7 {
                bindings::SRAM3_FROM_CM7
            } else {
                bindings::SRAM3_FROM_CM4
            }
        };
        Some(IpcRegion::new(
            (view(source) + bindings::POOL_OFFSET) as usize,
            (view(target) + bindings::POOL_OFFSET) as usize,
            bindings::POOL_BUDGET as usize,
        ))
    }

    fn shared_state(&self) -> &SharedState {
        // SAFETY: the ready/epoch state lives in the reserved SRAM3 control
        // area, is 4-byte aligned, is never overlapped by an allocated section
        // (the linker scripts keep SRAM3 out of MEMORY) and is shared between
        // the two cores through their respective views of SRAM3.
        unsafe { &*((SRAM3_BASE + bindings::SHARED_STATE_OFFSET) as *const SharedState) }
    }

    fn init_shared(&self) {
        // Owner boot sequence: publish the ready/epoch state, clear the
        // doorbell words, *then* release the peer so it can never observe a
        // half-initialized control area.
        self.shared_state().init();
        clear_doorbells();
        release_secondary_core();
    }

    fn configure_shared_memory(&self) {
        configure_mpu();
        hsem_enable_rx();
    }
}
