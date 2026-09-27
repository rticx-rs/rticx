//! Shared ready bitmap and epoch word for cross-binary boot and reset.
//!
//! [`SharedState`] is a small `#[repr(C)]` header placed in shared memory
//! (declared by the distribution, alongside the IPC regions) so that every
//! core of the project can observe which cores are ready and whether the
//! shared state has been (re)initialized since it last looked:
//!
//! ```text
//! offset 0   magic: 0x4E494258 ("XBIN" little-endian)
//! offset 4   epoch: bumped whenever the shared state is (re)initialized
//! offset 8   ready: bit `g` set while global core `g` is ready
//! ```
//!
//! `ready` is a bitmap over global core ids, so a project may have at most
//! [`MAX_CORES`] cores. `epoch` is a wrapping counter; a reset makes it
//! change, which is all [`SharedState::is_ready_at`] needs.
//!
//! # Ordering
//!
//! `magic` is published last on initialization (Release) and read with
//! Acquire; `mark_ready`/`clear_ready` publish with Release, `is_ready` and
//! `epoch` load with Acquire, and `bump_epoch` is an AcqRel read-modify-write.
//! The combined check [`SharedState::is_ready_at`] loads the ready bit first
//! and the epoch afterwards, so once a reset's epoch bump becomes visible a
//! cached epoch can no longer validate a ready bit: the check may report
//! `false` while a reset races it, which spawners turn into `Err(Some(input))`.
//!
//! # Boot protocol
//!
//! Owner core (the one that owns the shared region) during `init_shared`:
//!
//! 1. [`SharedState::init`] — clears every ready bit, bumps the epoch and
//!    publishes the magic;
//! 2. initialize the FIFO indices (`Fifo::init`) of every task;
//! 3. [`SharedState::mark_ready`]`(own_core)` last, at the end of
//!    `post_init` (the router IRQs are enabled by the core pass's used-IRQ
//!    machinery, not by a per-line arming step).
//!
//! Peer core boot (in particular after a peer reset):
//!
//! 1. [`SharedState::clear_ready`]`(own_core)` — a reset core may still have
//!    its old bit set in persistent shared memory;
//! 2. [`SharedState::bump_epoch`] — invalidates every cached epoch
//!    observation so spawners re-assess readiness instead of trusting stale
//!    state;
//! 3. re-establish its FIFO state, then
//!    [`SharedState::mark_ready`]`(own_core)`.
//!
//! A spawner caches the epoch it observed as valid; passing that epoch to
//! [`SharedState::is_ready_at`] after any reset returns `false` until the
//! spawner refreshes its observation (`epoch()` plus `is_ready(target)`).
//! [`ReadyCache`] implements exactly that refresh-on-stale check, and the
//! generated `cross_spawn` keeps one instance per task (M6-T2): a spawn aimed
//! at a not-ready or just-reset target returns `Err(Some(input))` without
//! enqueueing.
//!
//! # Target requirements
//!
//! The ready bitmap and the epoch counter use read-modify-write atomics
//! (`fetch_or`/`fetch_and`/`fetch_add`). Targets without native CAS (for
//! example `thumbv6m-none-eabi` or `riscv32imc-unknown-none-elf`) need the
//! crate's `atomic-critical-section` feature (enabled by default) and a
//! `critical-section` implementation supplied by the distribution, exactly
//! like `rticx-async`; `Fifo` itself only needs atomic loads/stores.

use core::mem::align_of;

use portable_atomic::{AtomicU32, Ordering};

/// Maximum number of cores trackable by the [`SharedState`] ready bitmap
/// (the width of its `AtomicU32`).
pub const MAX_CORES: u32 = u32::BITS;

/// Magic word marking an initialized [`SharedState`] ("XBIN" little-endian).
const MAGIC: u32 = u32::from_le_bytes(*b"XBIN");

/// Shared ready/epoch header, placed in Normal, Non-cacheable, Shareable
/// memory.
///
/// See the [module documentation](self) for the memory image, ordering and
/// the boot/reset protocol.
#[repr(C)]
pub struct SharedState {
    magic: AtomicU32,
    epoch: AtomicU32,
    ready: AtomicU32,
}

const _: () = {
    assert!(core::mem::size_of::<SharedState>() == 12);
    assert!(core::mem::offset_of!(SharedState, magic) == 0);
    assert!(core::mem::offset_of!(SharedState, epoch) == 4);
    assert!(core::mem::offset_of!(SharedState, ready) == 8);
};

impl SharedState {
    /// Creates an initialized, empty state (magic set, epoch 0, no ready
    /// cores).
    ///
    /// For memory shared between cores, place the state with
    /// [`Self::view_at`] and initialize it with [`Self::init`] instead.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            magic: AtomicU32::new(MAGIC),
            epoch: AtomicU32::new(0),
            ready: AtomicU32::new(0),
        }
    }

    /// Views the shared state placed at the fixed address `addr`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that:
    ///
    /// - `addr` is non-null and aligned for [`SharedState`], and at least
    ///   `size_of::<SharedState>()` bytes starting there are valid for reads
    ///   and writes for as long as the view is used (typically for the whole
    ///   program run);
    /// - the memory is shared with the peer cores and configured Normal,
    ///   Non-cacheable, Shareable (MPU), or explicit cache maintenance is
    ///   performed;
    /// - the boot protocol in the [module documentation](self) is followed,
    ///   so peers never rely on a partially initialized state.
    pub unsafe fn view_at(addr: usize) -> *mut Self {
        debug_assert_ne!(addr, 0, "shared state address must not be null");
        debug_assert_eq!(
            addr % align_of::<Self>(),
            0,
            "shared state address must be {}-byte aligned",
            align_of::<Self>()
        );
        addr as *mut Self
    }

    /// Re-initializes the state: clears every ready bit, bumps the epoch and
    /// publishes the magic.
    ///
    /// Called by the owner core as the first step of `init_shared`, before
    /// any peer can rely on the state. The epoch is *bumped*, not set to a
    /// constant, so a reset of the owner is detectable through the epoch even
    /// though the ready bits live in persistent memory.
    pub fn init(&self) {
        self.ready.store(0, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.magic.store(MAGIC, Ordering::Release);
    }

    /// Returns whether the magic word marks an initialized state.
    ///
    /// Only meaningful once a peer can observe the shared region.
    pub fn is_initialized(&self) -> bool {
        self.magic.load(Ordering::Acquire) == MAGIC
    }

    /// Returns the current epoch.
    ///
    /// Cache it next to a successful readiness check and pass it to
    /// [`Self::is_ready_at`]; any (re)initialization or peer reset makes it
    /// change.
    pub fn epoch(&self) -> u32 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Invalidates every cached epoch observation and returns the new epoch.
    ///
    /// Called at the start of a peer's boot sequence (after clearing its own
    /// ready bit), and internally by [`Self::init`].
    pub fn bump_epoch(&self) -> u32 {
        self.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    /// Returns the ready bitmap over global core ids.
    pub fn ready_mask(&self) -> u32 {
        self.ready.load(Ordering::Acquire)
    }

    /// Returns whether global core `core` is ready.
    ///
    /// Prefer [`Self::is_ready_at`] in spawn paths: a ready bit alone does
    /// not tell whether it belongs to the current epoch.
    ///
    /// # Panics
    ///
    /// Panics when `core >= MAX_CORES`.
    pub fn is_ready(&self, core: u32) -> bool {
        self.ready.load(Ordering::Acquire) & core_bit(core) != 0
    }

    /// Returns whether global core `core` is ready *at `epoch`*.
    ///
    /// A spawner passes the epoch it observed as valid; the result is
    /// conservative during a concurrent reset (it may report `false`) and
    /// discards stale readiness after a reset (or any reinitialization),
    /// which the spawn path reports as `Err(Some(input))`.
    ///
    /// # Panics
    ///
    /// Panics when `core >= MAX_CORES`.
    pub fn is_ready_at(&self, core: u32, epoch: u32) -> bool {
        self.is_ready(core) && self.epoch() == epoch
    }

    /// Marks global core `core` as ready and publishes its FIFO state
    /// (Release).
    ///
    /// # Panics
    ///
    /// Panics when `core >= MAX_CORES`.
    pub fn mark_ready(&self, core: u32) {
        self.ready.fetch_or(core_bit(core), Ordering::Release);
    }

    /// Marks global core `core` as not ready.
    ///
    /// Called at the start of a boot sequence, because a reset core may still
    /// have its bit set in persistent shared memory.
    ///
    /// # Panics
    ///
    /// Panics when `core >= MAX_CORES`.
    pub fn clear_ready(&self, core: u32) {
        self.ready.fetch_and(!core_bit(core), Ordering::Release);
    }
}

/// Spawner-local epoch cache for the target-ready check (M6-T2).
///
/// Generated spawners declare one `ReadyCache` per cross-binary task and call
/// [`Self::is_ready`] before enqueueing. The cache holds the epoch the
/// spawner last observed as valid: while it still matches the shared state,
/// readiness is a single ready-bit load; after any reinitialization or peer
/// reset the epoch changed, so the cache refreshes it on the next spawn
/// attempt and reports the target as not ready until the peer marks itself
/// ready in the new epoch.
///
/// The cache is core-local state (a `static` inside the generated
/// `cross_spawn`) touched only by the task's single producer core, so
/// `Relaxed` loads/stores suffice; the `Release`/`Acquire` ordering that
/// publishes a peer's FIFO state lives in [`SharedState::mark_ready`] and
/// [`SharedState::is_ready_at`].
#[derive(Debug)]
pub struct ReadyCache {
    epoch: AtomicU32,
}

impl ReadyCache {
    /// Creates a cache with an unknown epoch (zero).
    ///
    /// The first [`Self::is_ready`] call after boot refreshes it.
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            epoch: AtomicU32::new(0),
        }
    }

    /// Returns whether global core `core` is ready at the cached epoch,
    /// refreshing the cache once when the epoch changed.
    ///
    /// A `false` result means the target is not ready (it has not marked
    /// itself ready yet, or it reset and its epoch moved on without
    /// re-marking); the caller reports `Err(Some(input))` without enqueueing.
    /// The result is conservative while a reset races the check: it may
    /// report `false`, never a stale `true`.
    ///
    /// # Panics
    ///
    /// Panics when `core >= MAX_CORES` (through [`SharedState::is_ready_at`]).
    pub fn is_ready(&self, state: &SharedState, core: u32) -> bool {
        let cached = self.epoch.load(Ordering::Relaxed);
        if state.is_ready_at(core, cached) {
            return true;
        }
        let current = state.epoch();
        if state.is_ready_at(core, current) {
            self.epoch.store(current, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

/// Returns the ready bitmap bit of global core id `core`.
#[inline]
fn core_bit(core: u32) -> u32 {
    assert!(
        core < MAX_CORES,
        "global core id {core} exceeds the {MAX_CORES}-core ready bitmap"
    );
    1 << core
}
