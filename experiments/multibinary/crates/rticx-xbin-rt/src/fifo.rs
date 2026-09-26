//! Atomic single-producer single-consumer ring for cross-core shared memory.
//!
//! [`Fifo<T, DEPTH>`] is the runtime mirror of the canonical in-region image
//! documented in `rticx_xbin_proto::fifo` (crate `rticx-xbin-proto`):
//!
//! ```text
//! offset 0                            head index (one 32-byte slot)
//! offset 32                           tail index (one 32-byte slot)
//! offset 64                           first element
//! offset 64 + size_of::<T>() * DEPTH  end of the FIFO
//! ```
//!
//! `DEPTH` is `capacity + 1`: a ring buffer always leaves one slot empty to
//! tell a full FIFO from an empty one, and the dispatcher drains the FIFO
//! directly, so the effective capacity is exact. The two indices live in
//! separate cache-line slots, so the producer and the consumer never share a
//! cache line. [`FIFO_HEADER`] / [`FIFO_INDEX_STRIDE`] / [`FIFO_ALIGN`] are
//! pinned against `rticx-xbin-proto` by tests in `tests/fifo.rs`.
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
//! The shared region holding the FIFO must be Normal, Non-cacheable,
//! Shareable (MPU), or the distribution must provide explicit cache
//! maintenance. Device/Strongly-ordered memory is forbidden: `ldrex`/`strex`
//! are not valid there. Atomics provide ordering, not cache flushing.

use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, align_of, size_of};

use portable_atomic::{AtomicUsize, Ordering};

/// Alignment of every FIFO start inside its region, in bytes.
///
/// Mirrors `rticx_xbin_proto::FIFO_ALIGN`.
pub const FIFO_ALIGN: usize = 8;

/// Stride of one ring index, in bytes (a 32-byte cache-line slot).
///
/// Mirrors `rticx_xbin_proto::FIFO_INDEX_STRIDE`.
pub const FIFO_INDEX_STRIDE: usize = 32;

/// Size of the FIFO header before the first element, in bytes: two
/// [`FIFO_INDEX_STRIDE`]-byte slots.
///
/// Mirrors `rticx_xbin_proto::FIFO_HEADER`.
pub const FIFO_HEADER: usize = 2 * FIFO_INDEX_STRIDE;

const INDEX_PAD: usize = FIFO_INDEX_STRIDE - size_of::<AtomicUsize>();

const _: () = {
    assert!(size_of::<AtomicUsize>() == size_of::<usize>());
    assert!(align_of::<AtomicUsize>() <= FIFO_ALIGN);
    assert!(FIFO_HEADER == 64);
};

#[repr(C)]
struct IndexSlot {
    value: AtomicUsize,
    _pad: [u8; INDEX_PAD],
}

impl IndexSlot {
    const fn new() -> Self {
        Self {
            value: AtomicUsize::new(0),
            _pad: [0; INDEX_PAD],
        }
    }
}

const _: () = {
    assert!(size_of::<IndexSlot>() == FIFO_INDEX_STRIDE);
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
    head: IndexSlot,
    tail: IndexSlot,
    buffer: [UnsafeCell<MaybeUninit<T>>; DEPTH],
}

impl<T, const DEPTH: usize> Fifo<T, DEPTH> {
    /// Creates a zeroed, empty FIFO.
    ///
    /// If the FIFO lives in shared memory, prefer [`Self::view_at`] over
    /// moving a value (the peer's view must alias the same bytes).
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        let buffer = [const { UnsafeCell::new(MaybeUninit::uninit()) }; DEPTH];
        Self {
            head: IndexSlot::new(),
            tail: IndexSlot::new(),
            buffer,
        }
    }
}

impl<T: crate::CrossCoreMessage, const DEPTH: usize> Fifo<T, DEPTH> {
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
    ///   the protocol in the plan (doorbells/dispatchers), not ad hoc;
    /// - the FIFO has been initialized ([`Self::init`] or zeroed memory)
    ///   before any peer can observe it.
    pub unsafe fn view_at(addr: usize) -> *mut Self {
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
    /// Call it on the owner core during `init_shared`, before publishing
    /// ready.
    pub unsafe fn init(&self) {
        self.head.value.store(0, Ordering::Relaxed);
        self.tail.value.store(0, Ordering::Relaxed);
    }

    /// Returns whether the FIFO holds no pending elements.
    pub fn is_empty(&self) -> bool {
        self.head.value.load(Ordering::Acquire) == self.tail.value.load(Ordering::Acquire)
    }

    /// Returns whether the FIFO is full (no element can be enqueued).
    pub fn is_full(&self) -> bool {
        let head = self.head.value.load(Ordering::Acquire);
        let tail = self.tail.value.load(Ordering::Acquire);
        next_index::<DEPTH>(tail) == head
    }

    /// Returns the number of pending elements.
    pub fn len(&self) -> usize {
        let head = self.head.value.load(Ordering::Acquire);
        let tail = self.tail.value.load(Ordering::Acquire);
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
        let head = self.head.value.load(Ordering::Acquire);
        let tail = self.tail.value.load(Ordering::Relaxed);
        let next = next_index::<DEPTH>(tail);
        if next == head {
            return Err(value);
        }
        unsafe { (*self.buffer[tail].get()).write(value) };
        self.tail.value.store(next, Ordering::Release);
        Ok(())
    }

    /// Dequeues the oldest element, or returns `None` when empty.
    ///
    /// Must be called from the single consumer core only.
    pub fn dequeue(&self) -> Option<T> {
        assert!(DEPTH >= 2, "FIFO depth must be at least 2");
        let head = self.head.value.load(Ordering::Relaxed);
        let tail = self.tail.value.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let value = unsafe { self.buffer[head].get().read().assume_init() };
        self.head
            .value
            .store(next_index::<DEPTH>(head), Ordering::Release);
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

impl<T: crate::CrossCoreMessage, const DEPTH: usize> Producer<'_, T, DEPTH> {
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

impl<T: crate::CrossCoreMessage, const DEPTH: usize> Consumer<'_, T, DEPTH> {
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
unsafe impl<T: crate::CrossCoreMessage + Send, const DEPTH: usize> Send for Producer<'_, T, DEPTH> {}
unsafe impl<T: crate::CrossCoreMessage + Send, const DEPTH: usize> Send for Consumer<'_, T, DEPTH> {}
