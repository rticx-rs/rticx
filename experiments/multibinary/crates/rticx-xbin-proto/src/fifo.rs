//! Canonical in-region FIFO image and sizing.
//!
//! Every cross-binary task gets a fixed-size FIFO inside the shared-memory
//! region of its `(source -> target)` direction. The driver computes sizes and
//! offsets on the host, so the image below is a canonical, target-independent
//! contract that the runtime type of `rticx-xbin-rt` mirrors exactly:
//!
//! ```text
//! offset 0                              head index, one 32-byte slot
//! offset 32                             tail index, one 32-byte slot
//! offset 64                             first element
//! offset 64 + elem_size * depth         end of the FIFO
//! ```
//!
//! The two ring indices sit in separate 32-byte cache-line slots so the
//! producer and consumer indexes do not share a cache line. The element stride
//! is the canonical size of the message type (already a multiple of its
//! alignment, which is capped at 4). [`FIFO_ALIGN`] is the alignment the
//! allocator guarantees for every FIFO start; it keeps the header front
//! naturally aligned on 64-bit hosts as well.
//!
//! The ring depth is `capacity + 1`: a ring buffer leaves one slot empty to
//! distinguish a full FIFO from an empty one, and the dispatcher drains the
//! FIFO directly, so the effective capacity is exact.
//!
//! The runtime mirror of this image is `rticx_xbin_rt::Fifo<T, DEPTH>` (crate
//! `rticx-xbin-rt`, `src/fifo.rs`). Its integration tests pin
//! [`FIFO_HEADER`] / [`FIFO_INDEX_STRIDE`] / [`FIFO_ALIGN`] against the
//! constants here, so the two definitions must be changed together.

/// Alignment of every FIFO inside its region, in bytes.
pub const FIFO_ALIGN: u64 = 8;

/// Stride of one ring index, in bytes (a 32-byte cache-line slot).
pub const FIFO_INDEX_STRIDE: u64 = 32;

/// Size of the FIFO header before the first element, in bytes.
///
/// Two [`FIFO_INDEX_STRIDE`]-byte slots: the head index starts at offset 0,
/// the tail index at offset 32.
pub const FIFO_HEADER: u64 = 2 * FIFO_INDEX_STRIDE;

/// Returns the ring depth of a FIFO with `capacity` pending elements.
///
/// The depth is `capacity + 1` because one ring slot stays empty. Returns
/// `None` when `capacity` does not fit in `u64`.
pub fn fifo_depth(capacity: usize) -> Option<u64> {
    u64::try_from(capacity).ok()?.checked_add(1)
}

/// Returns the total size of one FIFO in bytes.
///
/// `elem_size` is the canonical size of the input message; the FIFO stores
/// [`fifo_depth`] elements after the [`FIFO_HEADER`]. Returns `None` when the
/// arithmetic overflows `u64`.
pub fn fifo_size(elem_size: u32, capacity: usize) -> Option<u64> {
    let depth = fifo_depth(capacity)?;
    FIFO_HEADER.checked_add(u64::from(elem_size).checked_mul(depth)?)
}

/// Aligns a byte offset up to the next [`FIFO_ALIGN`] boundary.
///
/// Returns `None` on overflow.
pub fn align_up(offset: u64) -> Option<u64> {
    offset
        .checked_add(FIFO_ALIGN - 1)
        .map(|value| value & !(FIFO_ALIGN - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_wastes_one_slot() {
        assert_eq!(fifo_depth(0), Some(1));
        assert_eq!(fifo_depth(1), Some(2));
        assert_eq!(fifo_depth(2), Some(3));
    }

    #[test]
    fn size_is_header_plus_depth_elements() {
        assert_eq!(fifo_size(12, 2), Some(FIFO_HEADER + 3 * 12));
        assert_eq!(fifo_size(4, 1), Some(FIFO_HEADER + 2 * 4));
        assert_eq!(fifo_size(1, 0), Some(FIFO_HEADER + 1));
    }

    #[test]
    fn alignment_rounds_up_to_fifo_align() {
        assert_eq!(align_up(0), Some(0));
        assert_eq!(align_up(1), Some(FIFO_ALIGN));
        assert_eq!(align_up(FIFO_ALIGN), Some(FIFO_ALIGN));
        assert_eq!(align_up(FIFO_ALIGN + 1), Some(2 * FIFO_ALIGN));
        assert_eq!(align_up(u64::MAX), None);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn absurd_capacity_overflows() {
        assert_eq!(fifo_depth(usize::MAX), None);
        assert_eq!(fifo_size(4, usize::MAX), None);
    }
}
