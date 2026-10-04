//! Host tests for the atomic cross-core SPSC ring.
//!
//! Covers wrap-around, full/empty/backpressure, `Copy` payload round-trips,
//! the canonical in-region image (drift guard against `rticx-xbin-proto`) and
//! the `Release`/`Acquire` hand-off under real threads.

use std::mem::{align_of, size_of};
use std::thread;

use rticx_xbin_rt::{Consumer, FIFO_ALIGN, FIFO_HEADER, FIFO_INDEX_STRIDE, Fifo, Producer};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Msg {
    seq: u32,
    payload: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Word(u32);

fn msg(seq: u32) -> Msg {
    Msg {
        seq,
        payload: [seq as u8; 8],
    }
}

/// Canonical FIFO size rounded up to the Rust struct alignment, i.e. what
/// `size_of::<Fifo<_, DEPTH>>()` must be on the host.
fn host_fifo_size(elem_size: usize, depth: usize) -> usize {
    let raw = FIFO_HEADER + elem_size * depth;
    raw.next_multiple_of(align_of::<usize>())
}

#[test]
fn capacity_is_depth_minus_one() {
    assert_eq!(Fifo::<Msg, 2>::new().capacity(), 1);
    assert_eq!(Fifo::<Msg, 4>::new().capacity(), 3);
}

#[test]
fn empty_fifo_dequeues_none() {
    let fifo = Fifo::<Msg, 3>::new();
    assert!(fifo.is_empty());
    assert!(!fifo.is_full());
    assert_eq!(fifo.len(), 0);
    assert_eq!(fifo.dequeue(), None);
}

#[test]
fn fills_to_capacity_then_returns_the_value() {
    let fifo = Fifo::<Msg, 3>::new();
    assert!(fifo.is_empty());

    assert_eq!(fifo.enqueue(msg(1)), Ok(()));
    assert_eq!(fifo.enqueue(msg(2)), Ok(()));
    assert!(fifo.is_full());
    assert_eq!(fifo.len(), 2);

    assert_eq!(fifo.enqueue(msg(3)), Err(msg(3)));
    assert_eq!(fifo.len(), 2);

    assert_eq!(fifo.dequeue(), Some(msg(1)));
    assert!(!fifo.is_full());
    assert_eq!(fifo.enqueue(msg(3)), Ok(()));
    assert_eq!(fifo.dequeue(), Some(msg(2)));
    assert_eq!(fifo.dequeue(), Some(msg(3)));
    assert_eq!(fifo.dequeue(), None);
}

#[test]
fn wraps_around_and_preserves_order() {
    let fifo = Fifo::<Word, 3>::new();

    for round in 0..8u32 {
        assert_eq!(fifo.enqueue(Word(round * 2)), Ok(()));
        assert_eq!(fifo.len(), 1);
        assert_eq!(fifo.enqueue(Word(round * 2 + 1)), Ok(()));
        assert_eq!(fifo.len(), 2);
        assert!(fifo.is_full());
        assert_eq!(fifo.enqueue(Word(u32::MAX)), Err(Word(u32::MAX)));
        assert_eq!(fifo.dequeue(), Some(Word(round * 2)));
        assert_eq!(fifo.dequeue(), Some(Word(round * 2 + 1)));
        assert_eq!(fifo.len(), 0);
    }
}

#[test]
fn init_resets_the_ring() {
    let fifo = Fifo::<Msg, 4>::new();
    assert_eq!(fifo.enqueue(msg(7)), Ok(()));
    assert_eq!(fifo.dequeue(), Some(msg(7)));
    assert_eq!(fifo.enqueue(msg(8)), Ok(()));
    assert_eq!(fifo.enqueue(msg(9)), Ok(()));

    unsafe { fifo.init() };

    assert!(fifo.is_empty());
    assert_eq!(fifo.len(), 0);
    assert_eq!(fifo.dequeue(), None);
    assert_eq!(fifo.enqueue(msg(10)), Ok(()));
    assert_eq!(fifo.dequeue(), Some(msg(10)));
}

#[test]
fn layout_mirrors_the_canonical_image() {
    assert_eq!(FIFO_ALIGN, rticx_xbin_proto::FIFO_ALIGN as usize);
    assert_eq!(
        FIFO_INDEX_STRIDE,
        rticx_xbin_proto::FIFO_INDEX_STRIDE as usize
    );
    assert_eq!(FIFO_HEADER, rticx_xbin_proto::FIFO_HEADER as usize);

    let elem_size = size_of::<Msg>();
    let depth = 3;
    let canonical = rticx_xbin_proto::fifo_size(elem_size as u32, depth - 1).unwrap() as usize;
    assert_eq!(canonical, FIFO_HEADER + elem_size * depth);

    assert_eq!(size_of::<Fifo<Msg, 3>>(), host_fifo_size(elem_size, 3));
    assert_eq!(size_of::<Fifo<Word, 2>>(), host_fifo_size(4, 2));
    assert_eq!(align_of::<Fifo<Msg, 3>>(), align_of::<usize>());
}

#[test]
fn view_at_places_the_ring_at_a_fixed_address() {
    let boxed = Box::new(Fifo::<Msg, 4>::new());
    let addr = Box::into_raw(boxed) as usize;
    assert_eq!(addr % FIFO_ALIGN, 0);

    let view = unsafe { Fifo::<Msg, 4>::view_at(addr) };
    assert_eq!(view as usize, addr);

    unsafe {
        assert_eq!((*view).enqueue(msg(1)), Ok(()));
        assert_eq!((*view).enqueue(msg(2)), Ok(()));
        assert_eq!((*view).dequeue(), Some(msg(1)));
        assert_eq!((*view).dequeue(), Some(msg(2)));
    }

    drop(unsafe { Box::from_raw(view) });
}

#[test]
fn split_endpoints_share_one_ring() {
    fn assert_send<T: Send>() {}
    assert_send::<Producer<'static, Msg, 4>>();
    assert_send::<Consumer<'static, Msg, 4>>();

    let mut fifo = Fifo::<Msg, 4>::new();
    let (producer, consumer) = fifo.split();
    assert_eq!(producer.capacity(), 3);
    assert_eq!(consumer.capacity(), 3);

    assert_eq!(producer.enqueue(msg(1)), Ok(()));
    assert_eq!(producer.len(), 1);
    assert!(!consumer.is_empty());
    assert_eq!(consumer.dequeue(), Some(msg(1)));
    assert_eq!(consumer.dequeue(), None);
}

#[test]
fn producer_and_consumer_hand_off_under_threads() {
    const DEPTH: usize = 4;
    let mut fifo = Fifo::<Msg, DEPTH>::new();

    thread::scope(|scope| {
        let (producer, consumer) = fifo.split();

        let producer_thread = scope.spawn(move || {
            for seq in 0..MESSAGES {
                let mut value = msg(seq);
                while let Err(returned) = producer.enqueue(value) {
                    value = returned;
                    thread::yield_now();
                }
            }
        });

        let consumer_thread = scope.spawn(move || {
            for expected in 0..MESSAGES {
                loop {
                    if let Some(value) = consumer.dequeue() {
                        assert_eq!(value, msg(expected));
                        break;
                    }
                    thread::yield_now();
                }
            }
            assert!(consumer.is_empty());
        });

        producer_thread.join().unwrap();
        consumer_thread.join().unwrap();
    });
}

#[cfg(miri)]
const MESSAGES: u32 = 64;
#[cfg(not(miri))]
const MESSAGES: u32 = 100_000;
