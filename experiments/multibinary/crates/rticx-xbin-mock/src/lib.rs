//! Mock distribution/backend for the RTICX multi-binary extension host tests.
//!
//! [`MockSystem`] owns the in-process stand-in for a project's
//! shared-memory regions, doorbells and [`SharedState`]. Hand one
//! [`MockBackend`] per simulated core to the test threads:
//!
//! ```
//! use rticx_xbin_mock::MockSystem;
//! use rticx_xbin_rt::backend::CrossBinBackend;
//!
//! let mut system = MockSystem::new();
//! system.add_region(0, 1, 4096).unwrap();
//!
//! let sender = system.backend(0);
//! let receiver = system.backend(1);
//! sender.doorbell_setup(1, 0, 0).unwrap();
//!
//! assert_eq!(sender.ipc_region(0, 1).unwrap().size(), 4096);
//! assert_eq!(receiver.current_global_core_id(), 1);
//! ```
//!
//! A region is backed by an 8-byte-aligned, zeroed in-process array (the
//! "array" variant of the plan's `mmap`/array choice). The mock doorbell is a
//! pending flag with a condition variable: [`MockBackend::doorbell_ring`]
//! (the `CrossBinBackend` method) sets it and [`MockBackend::doorbell_wait`] /
//! [`MockBackend::doorbell_take`] let a test thread act as the target's
//! dispatcher ISR.
//!
//! This crate is host-only test support: it is not `no_std` and its
//! `MockBackend` implements the [`CrossBinBackend`] cache/MPU hooks with the
//! trait's no-op defaults, which is correct because host memory needs no
//! cache maintenance and is never Device/Strongly-ordered.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rticx_xbin_rt::backend::{CrossBinBackend, DoorbellError, IpcRegion};
use rticx_xbin_rt::{FIFO_ALIGN, SharedState};

const _: () = assert!(
    core::mem::align_of::<u64>() >= FIFO_ALIGN,
    "the region backing array must be FIFO_ALIGN-aligned"
);

/// A project-wide mock: the shared regions, doorbells and ready/epoch state
/// that every simulated core of one project sees in common.
///
/// Configure the regions with [`MockSystem::add_region`] *before* handing out
/// any [`MockBackend`] ([`MockSystem::backend`]), because the region table is
/// then frozen and shared. `MockSystem` is cheap to clone; every clone refers
/// to the same underlying memory.
pub struct MockSystem {
    inner: Arc<SystemInner>,
}

struct SystemInner {
    regions: BTreeMap<(u32, u32), RegionBacking>,
    doorbells: Mutex<BTreeMap<(u32, u32), Arc<Doorbell>>>,
    state: SharedState,
}

struct RegionBacking {
    /// Backing allocation; `base` points at its start and keeps it alive.
    _storage: Vec<u64>,
    base: usize,
    size: usize,
}

impl MockSystem {
    /// Creates an empty mock system: no regions, no doorbells, a fresh
    /// (initialized, empty) [`SharedState`].
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SystemInner {
                regions: BTreeMap::new(),
                doorbells: Mutex::new(BTreeMap::new()),
                state: SharedState::new(),
            }),
        }
    }

    /// Declares the `source -> target` IPC region with `size` bytes of
    /// zeroed, [`FIFO_ALIGN`]-aligned shared memory.
    ///
    /// Both cores see the region at the same base address (the mock has a
    /// single address space); the region must be configured before any
    /// backend handle is taken.
    pub fn add_region(&mut self, source: u32, target: u32, size: usize) -> Result<(), MockError> {
        if source == target {
            return Err(MockError::SelfRegion { core: source });
        }
        if size < FIFO_ALIGN {
            return Err(MockError::RegionTooSmall {
                size,
                minimum: FIFO_ALIGN,
            });
        }
        let inner = Arc::get_mut(&mut self.inner)
            .expect("MockSystem regions must be configured before any mock backend is handed out");
        if inner.regions.contains_key(&(source, target)) {
            return Err(MockError::DuplicateRegion { source, target });
        }

        let bytes = size.next_multiple_of(FIFO_ALIGN);
        let words = vec![0u64; bytes / core::mem::size_of::<u64>()];
        let base = words.as_ptr() as usize;
        debug_assert_eq!(base % FIFO_ALIGN, 0);
        inner.regions.insert(
            (source, target),
            RegionBacking {
                _storage: words,
                base,
                size,
            },
        );
        Ok(())
    }

    /// Returns the shared ready/epoch state.
    pub fn state(&self) -> &SharedState {
        &self.inner.state
    }

    /// Returns a backend handle for the simulated global core `global_core_id`.
    pub fn backend(&self, global_core_id: u32) -> MockBackend {
        MockBackend {
            system: self.clone(),
            core: global_core_id,
        }
    }

    fn doorbell(&self, target: u32, line: u32) -> Option<Arc<Doorbell>> {
        self.inner
            .doorbells
            .lock()
            .expect("mock doorbell table is never poisoned")
            .get(&(target, line))
            .cloned()
    }
}

impl Clone for MockSystem {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

/// A per-core handle onto a [`MockSystem`], implementing [`CrossBinBackend`].
///
/// Cloning yields another handle for the same core; use
/// [`MockSystem::backend`] for another core.
#[derive(Clone)]
pub struct MockBackend {
    system: MockSystem,
    core: u32,
}

impl MockBackend {
    /// Returns the global core id this handle simulates.
    pub fn global_core_id(&self) -> u32 {
        self.core
    }

    /// Returns the [`MockSystem`] this handle belongs to.
    pub fn system(&self) -> &MockSystem {
        &self.system
    }

    /// Waits until doorbell `line` of `target` is pending, up to `timeout`.
    ///
    /// Returns `false` on timeout. Simulates the target core sleeping until
    /// its doorbell ISR fires; pair it with [`Self::doorbell_take`], which
    /// acts as the ISR clearing the pending flag.
    pub fn doorbell_wait(&self, target: u32, line: u32, timeout: Duration) -> bool {
        match self.system.doorbell(target, line) {
            Some(doorbell) => doorbell.wait_timeout(timeout),
            None => false,
        }
    }

    /// Returns whether doorbell `line` of `target` was pending, clearing it.
    ///
    /// Simulates the target's doorbell ISR observing and acknowledging the
    /// ring before pending the dispatcher.
    pub fn doorbell_take(&self, target: u32, line: u32) -> bool {
        match self.system.doorbell(target, line) {
            Some(doorbell) => doorbell.take(),
            None => false,
        }
    }
}

impl CrossBinBackend for MockBackend {
    fn current_global_core_id(&self) -> u32 {
        self.core
    }

    fn ipc_region(&self, source: u32, target: u32) -> Option<IpcRegion> {
        self.system
            .inner
            .regions
            .get(&(source, target))
            .map(|region| IpcRegion::new(region.base, region.base, region.size))
    }

    fn doorbell_setup(&self, target: u32, line: u32, _irq: u16) -> Result<(), DoorbellError> {
        self.system
            .inner
            .doorbells
            .lock()
            .expect("mock doorbell table is never poisoned")
            .entry((target, line))
            .or_insert_with(|| Arc::new(Doorbell::new()));
        Ok(())
    }

    fn doorbell_ring(&self, target: u32, line: u32) -> Result<(), DoorbellError> {
        let doorbell = self
            .system
            .doorbell(target, line)
            .ok_or(DoorbellError::UnknownLine)?;
        doorbell.ring();
        Ok(())
    }

    fn shared_state(&self) -> &SharedState {
        self.system.state()
    }
}

/// A pending flag plus a condition variable, the mock's doorbell line.
struct Doorbell {
    pending: AtomicBool,
    lock: Mutex<()>,
    ready: Condvar,
}

impl Doorbell {
    fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            lock: Mutex::new(()),
            ready: Condvar::new(),
        }
    }

    fn ring(&self) {
        self.pending.store(true, Ordering::Release);
        let _guard = self.lock.lock().expect("mock doorbell is never poisoned");
        self.ready.notify_all();
    }

    fn take(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }

    fn wait_timeout(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = self.lock.lock().expect("mock doorbell is never poisoned");
        while !self.pending.load(Ordering::Acquire) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .ready
                .wait_timeout(guard, remaining)
                .expect("mock doorbell is never poisoned");
            guard = next;
            if result.timed_out() && !self.pending.load(Ordering::Acquire) {
                return false;
            }
        }
        true
    }
}

/// Error returned by [`MockSystem::add_region`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    /// `source` and `target` are the same core.
    SelfRegion {
        /// The offending core id.
        core: u32,
    },
    /// The `(source, target)` region has already been declared.
    DuplicateRegion {
        /// The source (producer) core id.
        source: u32,
        /// The target (consumer) core id.
        target: u32,
    },
    /// The region is smaller than one aligned FIFO header.
    RegionTooSmall {
        /// The requested size in bytes.
        size: usize,
        /// The smallest accepted size in bytes.
        minimum: usize,
    },
}

impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MockError::SelfRegion { core } => {
                write!(f, "region connects core {core} to itself")
            }
            MockError::DuplicateRegion { source, target } => {
                write!(f, "region `{source}->{target}` is declared more than once")
            }
            MockError::RegionTooSmall { size, minimum } => write!(
                f,
                "region size {size} is below the minimum of {minimum} bytes"
            ),
        }
    }
}

impl std::error::Error for MockError {}
