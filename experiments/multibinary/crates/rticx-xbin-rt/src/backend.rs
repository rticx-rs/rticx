//! Distribution backend contract for cross-binary projects.
//!
//! [`CrossBinBackend`] is the runtime half of the extension's
//! distribution/backend contract (see `multibinary-multicore-plan.md` §10).
//! A distribution implements it to supply the target-specific pieces of a
//! multi-binary project:
//!
//! - the shared-memory region of every `(source -> target)` direction
//!   ([`CrossBinBackend::ipc_region`]);
//! - the hardware doorbell that wakes a target's dispatcher
//!   ([`CrossBinBackend::doorbell_setup`] / [`CrossBinBackend::doorbell_ring`]);
//! - the identity of the running core
//!   ([`CrossBinBackend::current_global_core_id`]);
//! - the shared ready/epoch state ([`CrossBinBackend::shared_state`], which
//!   the ready/epoch helper methods delegate to by default);
//! - the shared-memory configuration and cache-maintenance hooks
//!   ([`CrossBinBackend::configure_shared_memory`],
//!   [`CrossBinBackend::clean_range`], [`CrossBinBackend::invalidate_range`]).
//!
//! The in-tree `rticx-xbin-mock` implements this trait for host tests; the
//! out-of-tree STM32H7 (M7+M4) distribution implements it on hardware.
//!
//! # Shared-memory requirements
//!
//! Every IPC region must be mapped **Normal, Non-cacheable, Shareable**
//! memory. Distributions configure that mapping themselves (MPU/MMU) in
//! [`CrossBinBackend::configure_shared_memory`]; the method's default
//! implementation is empty, which is correct only on hosts and targets
//! without a data cache over the region.
//!
//! **Device and Strongly-ordered memory is forbidden.** Atomics provide
//! ordering, not cache flushing: the `Release`/`Acquire` pairs of
//! [`crate::Fifo`] and [`crate::SharedState`] order accesses to the shared
//! region, but they do not clean or invalidate a data cache. On a core whose
//! data cache covers the region, correctness requires either the
//! non-cacheable mapping above or explicit maintenance through
//! [`CrossBinBackend::clean_range`] / [`CrossBinBackend::invalidate_range`]
//! around every shared access. Device/Strongly-ordered mappings break the
//! atomics themselves: the read-modify-write operations compile to exclusive
//! loads/stores (`ldrex`/`strex` on Cortex-M), which are not valid there.
//!
//! # Boot protocol
//!
//! The distribution owns the boot sequencing (the H7 release of the M4 core,
//! for example). The generated code calls, through the backend:
//!
//! 1. [`CrossBinBackend::configure_shared_memory`] on every core, at the
//!    start of its entry: MPU/MMU attributes are per-core, so each core maps
//!    its own view of the regions before any shared access;
//! 2. [`CrossBinBackend::init_shared`] on the owner core (the lowest global
//!    core id), which also zeroes the per-task FIFO indices, before any peer
//!    can observe the shared memory;
//! 3. on every core, [`CrossBinBackend::doorbell_setup`] for the doorbell
//!    lines targeting that core and then [`CrossBinBackend::mark_ready`] at
//!    the end of its `post_init`, so a ready bit always implies an armed
//!    doorbell;
//! 4. [`CrossBinBackend::is_ready`] (with the epoch from
//!    [`CrossBinBackend::epoch`]) in `cross_spawn`, which reports
//!    `Err(Some(input))` while the target core is not ready.

use crate::SharedState;

/// A shared-memory region carrying every FIFO of one `(source -> target)`
/// direction.
///
/// The two base addresses are the addresses through which each endpoint core
/// sees the same physical memory; aliases are allowed. Use
/// [`CrossBinBackend::ipc_region`] to obtain the region a distribution
/// declared in `rticx.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpcRegion {
    base_from_source: usize,
    base_from_target: usize,
    size: usize,
}

impl IpcRegion {
    /// Creates a region with the given per-core base views and size in bytes.
    pub const fn new(base_from_source: usize, base_from_target: usize, size: usize) -> Self {
        Self {
            base_from_source,
            base_from_target,
            size,
        }
    }

    /// Returns the region base address as seen by the source (producer) core.
    pub const fn base_from_source(&self) -> usize {
        self.base_from_source
    }

    /// Returns the region base address as seen by the target (consumer) core.
    pub const fn base_from_target(&self) -> usize {
        self.base_from_target
    }

    /// Returns the region size in bytes.
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Returns the base address as seen by `core` for the `source -> target`
    /// direction, or `None` when `core` is neither endpoint.
    pub const fn base_for(&self, core: u32, source: u32, target: u32) -> Option<usize> {
        if core == source {
            Some(self.base_from_source)
        } else if core == target {
            Some(self.base_from_target)
        } else {
            None
        }
    }
}

/// Error returned when a doorbell operation cannot be performed.
///
/// A doorbell ring that fails is reported by `cross_spawn` as `Err(None)`:
/// the input is already in the target's FIFO and the spawner must not retry
/// the enqueue, only the notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DoorbellError {
    /// The `(target, line)` pair has not been set up (or is out of range).
    UnknownLine,
}

/// Runtime backend contract of the cross-binary extension.
///
/// See the [module documentation](self) for the shared-memory requirements and
/// the boot protocol. The methods with default implementations are hooks a
/// backend overrides only when its target needs them; their defaults are the
/// host/no-op behaviour.
pub trait CrossBinBackend {
    /// Returns the global core id of the core this backend instance runs on.
    fn current_global_core_id(&self) -> u32;

    /// Returns the IPC region declared for the `source -> target` direction,
    /// or `None` when the distribution provides none.
    ///
    /// The region is Normal, Non-cacheable, Shareable memory carrying the
    /// per-task FIFOs of that direction (see the [module
    /// documentation](self)).
    fn ipc_region(&self, source: u32, target: u32) -> Option<IpcRegion>;

    /// Sets up doorbell `line` of `target` so that
    /// [`Self::doorbell_ring`] pends `irq` on the target.
    ///
    /// Called once per `(target, line)` during boot, before
    /// [`Self::mark_ready`]. Calling it again (for example after a peer
    /// reset) must be idempotent.
    fn doorbell_setup(&self, target: u32, line: u32, irq: u16) -> Result<(), DoorbellError>;

    /// Rings doorbell `line` of `target`, waking its dispatcher.
    ///
    /// Called by the generated spawn after the input has been published to
    /// the target's FIFO. An error means the notification could not be
    /// delivered; the input is *already enqueued*, and `cross_spawn` reports
    /// `Err(None)` accordingly.
    fn doorbell_ring(&self, target: u32, line: u32) -> Result<(), DoorbellError>;

    /// Returns the shared ready/epoch state.
    ///
    /// The default implementations of [`Self::init_shared`],
    /// [`Self::mark_ready`], [`Self::is_ready`] and [`Self::epoch`] delegate
    /// here.
    fn shared_state(&self) -> &SharedState;

    /// Initializes the shared ready/epoch state on the owner core, before any
    /// peer can observe the shared memory.
    ///
    /// The default implementation calls [`SharedState::init`]: clear every
    /// ready bit, bump the epoch and publish the magic. Region contents
    /// (FIFO indices) are initialized separately by the generated code, which
    /// is the only side that knows the per-task FIFO offsets.
    fn init_shared(&self) {
        self.shared_state().init();
    }

    /// Marks global core `core` ready and publishes its FIFO/doorbell state.
    ///
    /// Called at the end of the core's `post_init`, after its doorbells are
    /// armed. The default implementation delegates to
    /// [`SharedState::mark_ready`].
    fn mark_ready(&self, core: u32) {
        self.shared_state().mark_ready(core);
    }

    /// Returns whether global core `core` is ready.
    ///
    /// The default implementation delegates to [`SharedState::is_ready`];
    /// spawn paths should use [`SharedState::is_ready_at`] with the cached
    /// epoch instead (M5).
    fn is_ready(&self, core: u32) -> bool {
        self.shared_state().is_ready(core)
    }

    /// Returns the current shared epoch.
    ///
    /// The default implementation delegates to [`SharedState::epoch`].
    fn epoch(&self) -> u32 {
        self.shared_state().epoch()
    }

    /// Configures every IPC region as Normal, Non-cacheable, Shareable
    /// memory.
    ///
    /// The default implementation is empty (no-op), which is correct on hosts
    /// and on targets whose data cache does not cover the region, or when the
    /// cache hooks below are overridden.
    ///
    /// **Device and Strongly-ordered memory is forbidden.** Exclusive
    /// accesses (`ldrex`/`strex` on Cortex-M) are not valid there, so the
    /// region's atomics would fault or lose atomicity.
    fn configure_shared_memory(&self) {}

    /// Cleans the data cache for `[addr, addr + len)`: the fallback for a
    /// cacheable region.
    ///
    /// The default implementation is a no-op, matching v1's non-cacheable
    /// requirement. A backend that maps a region cacheable must override this
    /// *and* [`Self::invalidate_range`] to maintain the caches explicitly
    /// around every shared access; see the [module documentation](self).
    fn clean_range(&self, addr: usize, len: usize) {
        let _ = (addr, len);
    }

    /// Invalidates the data cache for `[addr, addr + len)`: the fallback for
    /// a cacheable region.
    ///
    /// The default implementation is a no-op; see [`Self::clean_range`].
    fn invalidate_range(&self, addr: usize, len: usize) {
        let _ = (addr, len);
    }
}
