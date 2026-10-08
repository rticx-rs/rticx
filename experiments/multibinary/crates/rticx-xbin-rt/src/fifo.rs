//! Atomic single-producer single-consumer ring for cross-core shared memory.
//!
//! [`Fifo<T, DEPTH>`] is the runtime mirror of the canonical in-region image
//! documented in `rticx_xbin_proto::fifo` (crate `rticx-xbin-proto`):
//!
//! ```text
//! offset  0                            head index (`AtomicU32`, 4 bytes)
//! offset  4                            tail index (`AtomicU32`, 4 bytes)
//! offset  8                            first element
//! offset  8 + size_of::<T>() * DEPTH   end of the FIFO
//! ```
//!
//! `DEPTH` is `capacity + 1`: a ring buffer always leaves one slot empty to
//! tell a full FIFO from an empty one, and the dispatcher drains the FIFO
//! directly, so the effective capacity is exact. The two indices are
//! fixed-width 32-bit counters, so the header is 8 bytes on the target and on
//! the host where the canonical image is computed. [`FIFO_HEADER`] /
//! [`FIFO_INDEX_STRIDE`] / [`FIFO_ALIGN`] are pinned against `rticx-xbin-proto`
//! by tests in `tests/fifo.rs`.
//!
//! # Ordering and discipline
//!
//! This is the standard Vyukov bounded SPSC ring, with `head` as the
//! consumer-owned read index and `tail` as the producer-owned write index:
//!
//! - the producer writes the payload and then publishes `tail` with `Release`;
//! - the consumer loads `tail` with `Acquire` before reading the payload, and
//!   publishes `head` with `Release` after reading;
//! - the producer loads `head` with `Acquire` before reusing a slot.
//!
//! The type is deliberately **not `Sync`**: safe code must never share a FIFO
//! by reference. [`Fifo::split`] hands one [`Producer`] to the producing
//! core/thread and one [`Consumer`] to the consuming core/thread; fixed-address
//! placement inside the shared region goes through the unsafe
//! [`Fifo::view_at`].
//!
//! # Memory requirements
//!
//! The shared region holding the FIFO must be **effectively non-cacheable and
//! shared** on every core that touches it, by whatever mechanism the platform
//! provides. Atomics provide ordering, not cache flushing: a cacheable mapping
//! without transport-owned cache maintenance is unsupported in v1. Cores that
//! lack cross-core atomic primitives (for example ARMv6-M, which has no
//! `LDREX`/`STREX`) additionally require a distribution-provided cross-core
//! lock. See `docs/architecture.md` ("Cache policy and supported platforms").

use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, align_of, size_of};

use portable_atomic::{AtomicU32, Ordering};

/// Alignment of every FIFO start inside its region, in bytes.
///
/// Mirrors `rticx_xbin_proto::FIFO_ALIGN`.
pub const FIFO_ALIGN: usize = 8;

/// Stride of one ring index, in bytes (a 4-byte `AtomicU32`).
///
/// Mirrors `rticx_xbin_proto::FIFO_INDEX_STRIDE`.
pub const FIFO_INDEX_STRIDE: usize = 4;

/// Size of the FIFO header before the first element, in bytes: two
/// [`FIFO_INDEX_STRIDE`]-byte indices.
///
/// Mirrors `rticx_xbin_proto::FIFO_HEADER`.
pub const FIFO_HEADER: usize = 2 * FIFO_INDEX_STRIDE;

const _: () = {
    assert!(size_of::<AtomicU32>() == FIFO_INDEX_STRIDE);
    assert!(align_of::<AtomicU32>() <= FIFO_ALIGN);

    assert!(core::mem::offset_of!(Fifo<u32, 1>, head) == 0);
    assert!(core::mem::offset_of!(Fifo<u32, 1>, tail) == FIFO_INDEX_STRIDE);
    assert!(core::mem::offset_of!(Fifo<u32, 1>, buffer) == FIFO_HEADER);
};

/// Atomic SPSC ring over `DEPTH` in-place element slots.
///
/// See the [module documentation](self) for the memory image, ordering and
/// usage discipline. `DEPTH` must be at least 2 (`capacity + 1` as allocated
/// by `cargo xbin sync`); the ring can hold `DEPTH - 1` pending elements.
#[repr(C)]
pub struct Fifo<T, const DEPTH: usize> {
    head: AtomicU32,
    tail: AtomicU32,
    buffer: [UnsafeCell<MaybeUninit<T>>; DEPTH],
}

impl<T, const DEPTH: usize> Fifo<T, DEPTH> {
    /// Fails to compile when `DEPTH` does not fit the `u32` indices.
    ///
    /// The comparison is done in `u64` so the bound is valid on 32-bit hosts
    /// too (where `u32::MAX + 1` overflows a `u32`). A depth of
    /// `u32::MAX + 1` is the largest ring the counters can address.
    const _DEPTH_FITS_U32: usize = {
        assert!(DEPTH as u64 <= (u32::MAX as u64) + 1);
        DEPTH
    };

    /// Creates a zeroed, empty FIFO.
    ///
    /// If the FIFO lives in shared memory, prefer [`Self::view_at`] over
    /// moving a value (the peer's view must alias the same bytes).
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        let _ = Self::_DEPTH_FITS_U32;
        let buffer = [const { UnsafeCell::new(MaybeUninit::uninit()) }; DEPTH];
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            buffer,
        }
    }
}

impl<T: Copy + 'static, const DEPTH: usize> Fifo<T, DEPTH> {
    /// Number of pending elements the FIFO can hold (`DEPTH - 1`).
    pub const fn capacity(&self) -> usize {
        DEPTH.saturating_sub(1)
    }

    /// Views a FIFO placed at the fixed address `addr` inside a shared region.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that:
    ///
    /// - `addr` is non-null and [`FIFO_ALIGN`]-aligned, and at least
    ///   `size_of::<Fifo<T, DEPTH>>()` bytes starting there are valid for
    ///   reads and writes for as long as the view is used (typically for the
    ///   whole program run);
    /// - the memory is shared with the peer core using the access pattern
    ///   described in the [module documentation](self);
    /// - at most one producer core calls [`Self::enqueue`] and at most one
    ///   consumer core calls [`Self::dequeue`] on the resulting view, through
    ///   the generated doorbell/dispatcher protocol, not ad hoc;
    /// - the FIFO has been initialized ([`Self::init`] or zeroed memory)
    ///   before any peer can observe it.
    pub unsafe fn view_at(addr: usize) -> *mut Self {
        let _ = Self::_DEPTH_FITS_U32;
        debug_assert_ne!(addr, 0, "FIFO address must not be null");
        debug_assert_eq!(
            addr % FIFO_ALIGN,
            0,
            "FIFO address must be {FIFO_ALIGN}-byte aligned"
        );
        addr as *mut Self
    }

    /// Resets the ring to empty by zeroing both indices.
    ///
    /// # Safety
    ///
    /// Must not run concurrently with a producer or consumer: the peer may
    /// otherwise observe an index pair that points at an uninitialized slot.
    /// Call it on the producer core before any spawn.
    pub unsafe fn init(&self) {
        store_index(&self.head, 0, Ordering::Relaxed);
        store_index(&self.tail, 0, Ordering::Relaxed);
    }

    /// Returns whether the FIFO holds no pending elements.
    pub fn is_empty(&self) -> bool {
        load_index(&self.head, Ordering::Acquire) == load_index(&self.tail, Ordering::Acquire)
    }

    /// Returns whether the FIFO is full (no element can be enqueued).
    pub fn is_full(&self) -> bool {
        let head = load_index(&self.head, Ordering::Acquire);
        let tail = load_index(&self.tail, Ordering::Acquire);
        next_index::<DEPTH>(tail) == head
    }

    /// Returns the number of pending elements.
    pub fn len(&self) -> usize {
        let head = load_index(&self.head, Ordering::Acquire);
        let tail = load_index(&self.tail, Ordering::Acquire);
        if tail >= head {
            tail - head
        } else {
            tail + DEPTH - head
        }
    }

    /// Enqueues `value`, returning it back when the FIFO is full.
    ///
    /// Must be called from the single producer core only.
    pub fn enqueue(&self, value: T) -> Result<(), T> {
        assert!(DEPTH >= 2, "FIFO depth must be at least 2");
        let head = load_index(&self.head, Ordering::Acquire);
        let tail = load_index(&self.tail, Ordering::Relaxed);
        let next = next_index::<DEPTH>(tail);
        if next == head {
            return Err(value);
        }
        unsafe { (*self.buffer[tail].get()).write(value) };
        store_index(&self.tail, next, Ordering::Release);
        Ok(())
    }

    /// Dequeues the oldest element, or returns `None` when empty.
    ///
    /// Must be called from the single consumer core only.
    pub fn dequeue(&self) -> Option<T> {
        assert!(DEPTH >= 2, "FIFO depth must be at least 2");
        let head = load_index(&self.head, Ordering::Relaxed);
        let tail = load_index(&self.tail, Ordering::Acquire);
        if head == tail {
            return None;
        }
        let value = unsafe { self.buffer[head].get().read().assume_init() };
        store_index(&self.head, next_index::<DEPTH>(head), Ordering::Release);
        Some(value)
    }

    /// Splits the FIFO into its single producer and single consumer endpoint.
    ///
    /// The endpoints may be sent to different threads or used on different
    /// cores; each endpoint still upholds the single-producer /
    /// single-consumer rule.
    pub fn split(&mut self) -> (Producer<'_, T, DEPTH>, Consumer<'_, T, DEPTH>) {
        let fifo: &Fifo<T, DEPTH> = self;
        (Producer { fifo }, Consumer { fifo })
    }
}

/// Wraps an index around the ring.
#[inline]
fn next_index<const DEPTH: usize>(index: usize) -> usize {
    if index + 1 == DEPTH { 0 } else { index + 1 }
}

/// Reads one ring index as a host `usize`.
#[inline]
fn load_index(index: &AtomicU32, ordering: Ordering) -> usize {
    index.load(ordering) as usize
}

/// Publishes one ring index.
///
/// The cast is lossless: `_DEPTH_FITS_U32` const-asserts `DEPTH` fits the
/// `u32` counter and [`next_index`] always yields a value below `DEPTH`.
#[inline]
fn store_index(index: &AtomicU32, value: usize, ordering: Ordering) {
    index.store(value as u32, ordering);
}

/// The single producer endpoint of a [`Fifo`].
///
/// `Send` (one endpoint may move to the producing thread/core) but not
/// `Sync`: do not share it.
pub struct Producer<'a, T, const DEPTH: usize> {
    fifo: &'a Fifo<T, DEPTH>,
}

/// The single consumer endpoint of a [`Fifo`].
///
/// `Send` (one endpoint may move to the consuming thread/core) but not
/// `Sync`: do not share it.
pub struct Consumer<'a, T, const DEPTH: usize> {
    fifo: &'a Fifo<T, DEPTH>,
}

impl<T: Copy + 'static, const DEPTH: usize> Producer<'_, T, DEPTH> {
    /// Enqueues `value`, returning it back when the FIFO is full.
    pub fn enqueue(&self, value: T) -> Result<(), T> {
        self.fifo.enqueue(value)
    }

    /// Number of pending elements the FIFO can hold.
    pub const fn capacity(&self) -> usize {
        self.fifo.capacity()
    }

    /// Returns whether the FIFO is full.
    pub fn is_full(&self) -> bool {
        self.fifo.is_full()
    }

    /// Returns whether the FIFO is empty.
    pub fn is_empty(&self) -> bool {
        self.fifo.is_empty()
    }

    /// Returns the number of pending elements.
    pub fn len(&self) -> usize {
        self.fifo.len()
    }
}

impl<T: Copy + 'static, const DEPTH: usize> Consumer<'_, T, DEPTH> {
    /// Dequeues the oldest element, or returns `None` when empty.
    pub fn dequeue(&self) -> Option<T> {
        self.fifo.dequeue()
    }

    /// Number of pending elements the FIFO can hold.
    pub const fn capacity(&self) -> usize {
        self.fifo.capacity()
    }

    /// Returns whether the FIFO is empty.
    pub fn is_empty(&self) -> bool {
        self.fifo.is_empty()
    }

    /// Returns whether the FIFO is full.
    pub fn is_full(&self) -> bool {
        self.fifo.is_full()
    }

    /// Returns the number of pending elements.
    pub fn len(&self) -> usize {
        self.fifo.len()
    }
}

// Safety: the endpoints alias the same FIFO, but `enqueue` and `dequeue`
// only touch the payload slots the SPSC discipline assigns to their side and
// communicate through atomics. The caller of `split` (or `view_at`) promises
// exactly one producer and one consumer. `T` must itself be `Send` for the
// payload hand-off to be sound.
unsafe impl<T: Copy + Send + 'static, const DEPTH: usize> Send for Producer<'_, T, DEPTH> {}
unsafe impl<T: Copy + Send + 'static, const DEPTH: usize> Send for Consumer<'_, T, DEPTH> {}
