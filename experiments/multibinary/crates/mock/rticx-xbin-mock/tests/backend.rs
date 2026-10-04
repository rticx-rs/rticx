//! Host tests for the mock cross-binary backend.
//!
//! Covers the pool table, the per-handle core identity, the pair doorbell
//! message word and the no-op cache hooks, plus the M2-T3 acceptance case:
//! two threads spawn and drain through a raw `Fifo` placed in a mock pool,
//! notified through the pair doorbell.

use std::mem::size_of;
use std::thread;
use std::time::Duration;

use rticx_xbin_mock::{MockError, MockSystem};
use rticx_xbin_rt::backend::CrossBinBackend;
use rticx_xbin_rt::{FIFO_ALIGN, FIFO_HEADER, Fifo};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Msg {
    seq: u32,
    payload: [u8; 8],
}

fn msg(seq: u32) -> Msg {
    Msg {
        seq,
        payload: [seq as u8; 8],
    }
}

const DEPTH: usize = 4;

#[cfg(miri)]
const MESSAGES: u32 = 64;
#[cfg(not(miri))]
const MESSAGES: u32 = 10_000;

#[test]
fn pools_are_aligned_and_addressable_from_both_ends() {
    let mut system = MockSystem::new();
    system.add_pool(0, 1, 256).unwrap();

    let sender = system.backend(0);
    let receiver = system.backend(1);
    let pool = sender.ipc_region(0, 1).expect("pool {0, 1} exists");

    assert_eq!(pool.base_from_source(), pool.base_from_target());
    assert_eq!(pool.base_from_source() % FIFO_ALIGN, 0);
    assert_eq!(pool.size(), 256);
    assert_eq!(pool.base_for(0, 0, 1), Some(pool.base_from_source()));
    assert_eq!(pool.base_for(1, 0, 1), Some(pool.base_from_target()));
    assert_eq!(pool.base_for(2, 0, 1), None);

    assert_eq!(receiver.ipc_region(0, 1), Some(pool));
    assert_eq!(sender.ipc_region(7, 8), None);
}

/// Both directions of a dual are the same shared pool: the backward view is
/// the forward one with the endpoint views swapped (M6.9-T6). In the mock's
/// single address space both base addresses coincide.
#[test]
fn both_directions_return_the_same_pool() {
    let mut system = MockSystem::new();
    system.add_pool(0, 1, 256).unwrap();

    let forward = system.backend(0).ipc_region(0, 1).expect("0 -> 1 pool");
    let backward = system.backend(0).ipc_region(1, 0).expect("1 -> 0 pool");

    assert_eq!(forward.size(), backward.size(), "one shared budget");
    assert_eq!(forward.base_from_source(), backward.base_from_target());
    assert_eq!(forward.base_from_target(), backward.base_from_source());
}

#[test]
fn pool_declaration_rejects_invalid_input() {
    let mut system = MockSystem::new();

    assert_eq!(
        system.add_pool(1, 1, 64),
        Err(MockError::SelfPool { core: 1 })
    );
    assert_eq!(
        system.add_pool(0, 1, 4),
        Err(MockError::PoolTooSmall {
            size: 4,
            minimum: FIFO_ALIGN
        })
    );

    system.add_pool(0, 1, 64).unwrap();
    assert_eq!(
        system.add_pool(0, 1, 64),
        Err(MockError::DuplicatePool {
            core_a: 0,
            core_b: 1
        })
    );
    // The dual is unordered: the reverse declaration is the same pool.
    assert_eq!(
        system.add_pool(1, 0, 64),
        Err(MockError::DuplicatePool {
            core_a: 0,
            core_b: 1
        })
    );
}

#[test]
fn current_global_core_id_comes_from_the_handle() {
    let system = MockSystem::new();
    assert_eq!(system.backend(0).current_global_core_id(), 0);
    assert_eq!(system.backend(7).current_global_core_id(), 7);
    assert_eq!(system.backend(7).global_core_id(), 7);
}

#[test]
fn pair_doorbell_carries_task_ids() {
    let system = MockSystem::new();
    let source = system.backend(0);
    let target = system.backend(1);

    // The pair word exists from the first ring, like a hardware doorbell.
    assert_eq!(target.take_message(0, 1), None, "no notification yet");
    assert!(!target.router_wait(0, 1, Duration::from_millis(10)));

    source.doorbell_send(0, 1, 3).unwrap();
    assert!(target.router_wait(0, 1, Duration::from_millis(10)));
    assert_eq!(target.take_message(0, 1), Some(3));
    assert_eq!(target.take_message(0, 1), None, "take clears the word");

    // Notifications coalesce: the latest task id replaces an unread one.
    source.doorbell_send(0, 1, 1).unwrap();
    source.doorbell_send(0, 1, 2).unwrap();
    assert_eq!(target.take_message(0, 1), Some(2));

    // Pairs are independent, and both endpoints see the same word.
    assert_eq!(target.take_message(1, 0), None);
    source.doorbell_send(0, 1, 9).unwrap();
    assert_eq!(source.take_message(0, 1), Some(9), "the word is shared");
}

#[test]
fn pair_doorbell_wakes_a_waiting_router() {
    let system = MockSystem::new();
    let target = system.backend(1);
    let source = system.backend(0);

    thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            assert!(target.router_wait(0, 1, Duration::from_secs(5)));
            assert_eq!(target.take_message(0, 1), Some(7));
        });
        source.doorbell_send(0, 1, 7).unwrap();
        waiter.join().unwrap();
    });
}

#[test]
fn cache_hooks_are_no_ops_on_host() {
    let mut system = MockSystem::new();
    system.add_pool(0, 1, 128).unwrap();
    let backend = system.backend(0);
    let region = backend.ipc_region(0, 1).unwrap();

    backend.configure_shared_memory();
    backend.clean_range(region.base_from_source(), region.size());
    backend.invalidate_range(region.base_from_source(), region.size());

    let fifo = unsafe { Fifo::<Msg, 2>::view_at(region.base_from_source()) };
    assert!(unsafe { (*fifo).enqueue(msg(1)) }.is_ok());
    assert_eq!(unsafe { (*fifo).dequeue() }, Some(msg(1)));
}

#[test]
fn two_threads_spawn_and_drain_via_raw_fifo() {
    let mut system = MockSystem::new();
    system.add_pool(0, 1, 4096).unwrap();

    let sender = system.backend(0);
    let receiver = system.backend(1);

    thread::scope(|scope| {
        let producer = scope.spawn(move || {
            let region = sender.ipc_region(0, 1).expect("region 0->1 exists");
            assert!(FIFO_HEADER + size_of::<Msg>() * DEPTH <= region.size());
            let fifo = unsafe { Fifo::<Msg, DEPTH>::view_at(region.base_from_source()) };

            for seq in 0..MESSAGES {
                let mut value = msg(seq);
                while let Err(returned) = unsafe { (*fifo).enqueue(value) } {
                    value = returned;
                    thread::yield_now();
                }
                // The task id is irrelevant here: the mock router word
                // coalesces and the consumer drains the FIFO until empty.
                sender
                    .doorbell_send(0, 1, 1)
                    .expect("the mock doorbell accepts the id");
            }
        });

        let consumer = scope.spawn(move || {
            let region = receiver.ipc_region(0, 1).expect("region 0->1 exists");
            let fifo = unsafe { Fifo::<Msg, DEPTH>::view_at(region.base_from_target()) };

            let mut expected = 0;
            while expected < MESSAGES {
                assert!(
                    receiver.router_wait(0, 1, Duration::from_secs(10)),
                    "timed out after {expected} of {MESSAGES} messages"
                );
                receiver.take_message(0, 1);
                while let Some(value) = unsafe { (*fifo).dequeue() } {
                    assert_eq!(value, msg(expected));
                    expected += 1;
                }
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    });
}

#[test]
fn mock_system_and_backend_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<MockSystem>();
    assert_send_sync::<rticx_xbin_mock::MockBackend>();
}
