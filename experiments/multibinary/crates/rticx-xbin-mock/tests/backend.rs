//! Host tests for the mock cross-binary backend.
//!
//! Covers the region table, the per-handle core identity, the pair doorbell
//! message word, the ready/epoch defaults and the no-op cache hooks, plus the
//! M2-T3 acceptance case: two threads spawn and drain through a raw `Fifo`
//! placed in a mock region, notified through the pair doorbell, and the M6-T2
//! peer-reset recovery path (`ReadyCache` against a reset target).

use std::mem::size_of;
use std::thread;
use std::time::Duration;

use rticx_xbin_mock::{MockError, MockSystem};
use rticx_xbin_rt::backend::CrossBinBackend;
use rticx_xbin_rt::{CrossCoreMessage, FIFO_ALIGN, FIFO_HEADER, Fifo, ReadyCache};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Msg {
    seq: u32,
    payload: [u8; 8],
}

unsafe impl CrossCoreMessage for Msg {}

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
fn regions_are_aligned_and_addressable_from_both_ends() {
    let mut system = MockSystem::new();
    system.add_region(0, 1, 256).unwrap();

    let sender = system.backend(0);
    let receiver = system.backend(1);
    let region = sender.ipc_region(0, 1).expect("region 0->1 exists");

    assert_eq!(region.base_from_source(), region.base_from_target());
    assert_eq!(region.base_from_source() % FIFO_ALIGN, 0);
    assert_eq!(region.size(), 256);
    assert_eq!(region.base_for(0, 0, 1), Some(region.base_from_source()));
    assert_eq!(region.base_for(1, 0, 1), Some(region.base_from_target()));
    assert_eq!(region.base_for(2, 0, 1), None);

    assert_eq!(receiver.ipc_region(0, 1), Some(region));
    assert_eq!(receiver.ipc_region(1, 0), None);
    assert_eq!(sender.ipc_region(7, 8), None);
}

#[test]
fn region_declaration_rejects_invalid_input() {
    let mut system = MockSystem::new();

    assert_eq!(
        system.add_region(1, 1, 64),
        Err(MockError::SelfRegion { core: 1 })
    );
    assert_eq!(
        system.add_region(0, 1, 4),
        Err(MockError::RegionTooSmall {
            size: 4,
            minimum: FIFO_ALIGN
        })
    );

    system.add_region(0, 1, 64).unwrap();
    assert_eq!(
        system.add_region(0, 1, 64),
        Err(MockError::DuplicateRegion {
            source: 0,
            target: 1
        })
    );
    assert!(system.add_region(1, 0, 64).is_ok());
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
fn ready_and_epoch_delegate_to_the_shared_state() {
    let system = MockSystem::new();
    let core0 = system.backend(0);
    let core1 = system.backend(1);

    core0.init_shared();
    assert_eq!(core0.epoch(), 1);
    assert!(!core0.is_ready(1));

    core1.mark_ready(1);
    assert!(core0.is_ready(1));
    assert_eq!(system.state().ready_mask(), 1 << 1);
    assert!(system.state().is_ready_at(1, core0.epoch()));

    core1.init_shared();
    assert_eq!(core1.epoch(), 2);
    assert_eq!(system.state().ready_mask(), 0);
}

/// The M6-T2 peer-reset recovery path through the mock: the spawner's cached
/// epoch gate rejects spawn attempts while the peer is reset and recovers
/// once the peer re-marks itself ready in the new epoch.
#[test]
fn ready_cache_gates_spawns_across_a_peer_reset() {
    let system = MockSystem::new();
    let producer = system.backend(0);
    let consumer = system.backend(1);
    let ready = ReadyCache::new();

    // Boot: the owner publishes the shared state, then the target marks
    // itself ready. The spawner's first check adopts the boot epoch.
    producer.init_shared();
    assert!(!ready.is_ready(producer.shared_state(), 1));
    consumer.mark_ready(1);
    assert!(ready.is_ready(producer.shared_state(), 1));

    // Peer reset: the target clears its own bit and bumps the epoch before
    // re-initializing, which makes the spawner's cached epoch stale.
    let state = producer.shared_state();
    state.clear_ready(1);
    state.bump_epoch();
    assert!(
        !ready.is_ready(state, 1),
        "the reset target rejects spawns until it re-marks ready"
    );

    // Recovery: the target re-marks ready and the next spawn attempt
    // refreshes the cached epoch.
    state.mark_ready(1);
    assert!(
        ready.is_ready(state, 1),
        "the refresh recovers readiness after the reset"
    );
}

#[test]
fn cache_hooks_are_no_ops_on_host() {
    let mut system = MockSystem::new();
    system.add_region(0, 1, 128).unwrap();
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
    system.add_region(0, 1, 4096).unwrap();

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
