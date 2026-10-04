//! Mock distribution/backend for the RTICX multi-binary extension host tests.
//!
//! [`MockSystem`] owns the in-process stand-in for a project's
//! shared-memory pools and doorbells. Hand one [`MockBackend`] per simulated
//! core to the test threads:
//!
//! ```
//! use rticx_xbin_mock::MockSystem;
//! use rticx_xbin_rt::backend::CrossBinBackend;
//!
//! let mut system = MockSystem::new();
//! system.add_pool(0, 1, 4096).unwrap();
//!
//! let sender = system.backend(0);
//! let receiver = system.backend(1);
//!
//! assert_eq!(sender.ipc_region(0, 1).unwrap().size(), 4096);
//! assert_eq!(receiver.current_global_core_id(), 1);
//! ```
//!
//! A pool is backed by an 8-byte-aligned, zeroed in-process array (the
//! "array" variant of the plan's `mmap`/array choice) and carries the FIFOs of
//! **both** directions of its dual: `ipc_region(a, b)` and `ipc_region(b, a)`
//! return the same pool (M6.9-T6). The doorbell transport
//! is the per-`(source -> target)` **pair message word** of the M6.5 router
//! ([`MockBackend::doorbell_send`], [`MockBackend::take_message`],
//! [`MockBackend::router_wait`]): it carries the task id a producer publishes
//! to the target's router, which then pends the line dispatcher. There are no
//! per-line doorbell methods (M6.5).
//!
//! This crate is host-only test support: it is not `no_std` and its
//! `MockBackend` implements the [`CrossBinBackend`] cache/MPU hooks with the
//! trait's no-op defaults, which is correct because host memory needs no
//! cache maintenance and is never Device/Strongly-ordered.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rticx_xbin_rt::FIFO_ALIGN;
use rticx_xbin_rt::backend::{CrossBinBackend, IpcRegion};

const _: () = assert!(
    core::mem::align_of::<u64>() >= FIFO_ALIGN,
    "the pool backing array must be FIFO_ALIGN-aligned"
);

/// A project-wide mock: the shared pools and doorbells that every simulated
/// core of one project sees in common.
///
/// Configure the pools with [`MockSystem::add_pool`] *before* handing out
/// any [`MockBackend`] ([`MockSystem::backend`]), because the pool table is
/// then frozen and shared. `MockSystem` is cheap to clone; every clone refers
/// to the same underlying memory.
pub struct MockSystem {
    inner: Arc<SystemInner>,
}

struct SystemInner {
    /// One backing per unordered core pair, keyed by `(core_a, core_b)` with
    /// `core_a < core_b`.
    pools: BTreeMap<(u32, u32), PoolBacking>,
    /// Per-`(source -> target)` pair message words of the M6.5 doorbell
    /// routers, keyed by `(source, target)`.
    pair_doorbells: Mutex<BTreeMap<(u32, u32), Arc<PairDoorbell>>>,
}

struct PoolBacking {
    /// Backing allocation; `base` points at its start and keeps it alive.
    _storage: Vec<u64>,
    base: usize,
    size: usize,
}

/// Orders a dual's cores as `(core_a, core_b)` with `core_a < core_b`.
fn ordered_pair(first: u32, second: u32) -> (u32, u32) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

impl MockSystem {
    /// Creates an empty mock system: no pools and no doorbells.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SystemInner {
                pools: BTreeMap::new(),
                pair_doorbells: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    /// Declares the shared pool of the `{core_a, core_b}` dual with `size`
    /// bytes of zeroed, [`FIFO_ALIGN`]-aligned shared memory.
    ///
    /// Both directions of the dual share this one block (M6.9-T6); the order
    /// of `core_a`/`core_b` is irrelevant. Both cores see the pool at the same
    /// base address (the mock has a single address space); the pool must be
    /// configured before any backend handle is taken.
    pub fn add_pool(&mut self, core_a: u32, core_b: u32, size: usize) -> Result<(), MockError> {
        if core_a == core_b {
            return Err(MockError::SelfPool { core: core_a });
        }
        if size < FIFO_ALIGN {
            return Err(MockError::PoolTooSmall {
                size,
                minimum: FIFO_ALIGN,
            });
        }
        let pair = ordered_pair(core_a, core_b);
        let inner = Arc::get_mut(&mut self.inner)
            .expect("MockSystem pools must be configured before any mock backend is handed out");
        if inner.pools.contains_key(&pair) {
            return Err(MockError::DuplicatePool {
                core_a: pair.0,
                core_b: pair.1,
            });
        }

        let bytes = size.next_multiple_of(FIFO_ALIGN);
        let words = vec![0u64; bytes / core::mem::size_of::<u64>()];
        let base = words.as_ptr() as usize;
        debug_assert_eq!(base % FIFO_ALIGN, 0);
        inner.pools.insert(
            pair,
            PoolBacking {
                _storage: words,
                base,
                size,
            },
        );
        Ok(())
    }

    /// Returns a backend handle for the simulated global core `global_core_id`.
    pub fn backend(&self, global_core_id: u32) -> MockBackend {
        MockBackend {
            system: self.clone(),
            core: global_core_id,
        }
    }

    /// Returns the pair message word of `(source -> target)`, creating it on
    /// first use (like a hardware doorbell, which exists independently of the
    /// software that rings it).
    fn pair_doorbell(&self, source: u32, target: u32) -> Arc<PairDoorbell> {
        self.inner
            .pair_doorbells
            .lock()
            .expect("mock pair doorbell table is never poisoned")
            .entry((source, target))
            .or_insert_with(|| Arc::new(PairDoorbell::new()))
            .clone()
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

    /// Publishes `task_id` to the `(source -> target)` pair message word and
    /// triggers the target's router interrupt (M6.5-T2).
    ///
    /// This is what the generated `__rticx_xbin_ring_{source}_{target}`
    /// function calls: it is the mock's portable doorbell transport. An
    /// unread message is overwritten (the hardware doorbell coalesces
    /// notifications); the target's line dispatcher drains its FIFOs until
    /// empty, so repeated delivery of the same task id loses no spawn.
    ///
    /// Always succeeds: the mock doorbell exists from the first ring, like a
    /// hardware peripheral.
    #[allow(clippy::result_unit_err)] // mirrors the generated ring contract
    pub fn doorbell_send(&self, source: u32, target: u32, task_id: u32) -> Result<(), ()> {
        self.system.pair_doorbell(source, target).send(task_id);
        Ok(())
    }

    /// Takes the pending task id of the `(source -> target)` pair message
    /// word, or `None` when no notification is pending (M6.5-T3).
    ///
    /// This is what the generated `__rticx_xbin_read_{source}_{target}`
    /// function calls; the router drains the word in a loop.
    pub fn take_message(&self, source: u32, target: u32) -> Option<u32> {
        self.system.pair_doorbell(source, target).take()
    }

    /// Waits until a notification is pending on the `(source -> target)` pair
    /// message word, up to `timeout`.
    ///
    /// Returns `false` on timeout. Simulates the target core sleeping until
    /// its router interrupt fires; pair it with [`Self::take_message`], which
    /// acts as the router reading and clearing the word.
    pub fn router_wait(&self, source: u32, target: u32, timeout: Duration) -> bool {
        self.system
            .pair_doorbell(source, target)
            .wait_timeout(timeout)
    }
}

impl CrossBinBackend for MockBackend {
    fn current_global_core_id(&self) -> u32 {
        self.core
    }

    fn ipc_region(&self, source: u32, target: u32) -> Option<IpcRegion> {
        self.system
            .inner
            .pools
            .get(&ordered_pair(source, target))
            .map(|pool| IpcRegion::new(pool.base, pool.base, pool.size))
    }
}

/// The per-`(source -> target)` pair message word: the mock's doorbell
/// transport for task-id routing (M6.5-T2/T3).
///
/// `message` holds the latest published task id (`0` means empty; the driver
/// numbers tasks from 1), and the condition variable lets a test thread wait
/// for the router interrupt. This mirrors a single-word hardware doorbell:
/// notifications coalesce, and idempotence comes from the line dispatcher
/// draining its FIFOs until empty.
struct PairDoorbell {
    message: AtomicU32,
    lock: Mutex<()>,
    ready: Condvar,
}

impl PairDoorbell {
    fn new() -> Self {
        Self {
            message: AtomicU32::new(0),
            lock: Mutex::new(()),
            ready: Condvar::new(),
        }
    }

    fn send(&self, task_id: u32) {
        self.message.store(task_id, Ordering::Release);
        let _guard = self
            .lock
            .lock()
            .expect("the mock pair doorbell is never poisoned");
        self.ready.notify_all();
    }

    fn take(&self) -> Option<u32> {
        match self.message.swap(0, Ordering::AcqRel) {
            0 => None,
            task_id => Some(task_id),
        }
    }

    fn wait_timeout(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = self
            .lock
            .lock()
            .expect("the mock pair doorbell is never poisoned");
        while self.message.load(Ordering::Acquire) == 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .ready
                .wait_timeout(guard, remaining)
                .expect("the mock pair doorbell is never poisoned");
            guard = next;
            if result.timed_out() && self.message.load(Ordering::Acquire) == 0 {
                return false;
            }
        }
        true
    }
}

/// Error returned by [`MockSystem::add_pool`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockError {
    /// `core_a` and `core_b` are the same core.
    SelfPool {
        /// The offending core id.
        core: u32,
    },
    /// The dual's pool has already been declared.
    DuplicatePool {
        /// Lower core id of the dual.
        core_a: u32,
        /// Higher core id of the dual.
        core_b: u32,
    },
    /// The pool is smaller than one aligned FIFO header.
    PoolTooSmall {
        /// The requested size in bytes.
        size: usize,
        /// The smallest accepted size in bytes.
        minimum: usize,
    },
}

impl fmt::Display for MockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MockError::SelfPool { core } => {
                write!(f, "pool connects core {core} to itself")
            }
            MockError::DuplicatePool { core_a, core_b } => {
                write!(
                    f,
                    "the pool of the `{core_a}<->{core_b}` dual is declared more than once"
                )
            }
            MockError::PoolTooSmall { size, minimum } => write!(
                f,
                "pool size {size} is below the minimum of {minimum} bytes"
            ),
        }
    }
}

impl std::error::Error for MockError {}
