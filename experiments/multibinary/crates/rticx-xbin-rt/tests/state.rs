//! Host tests for the shared ready/epoch state.
//!
//! Covers the ready bitmap, epoch bumps, raw-memory placement, cross-thread
//! visibility and the M2-T2 acceptance case: a peer reset (bit cleared, epoch
//! bumped) must invalidate the epoch a spawner cached earlier. The M6-T2
//! [`ReadyCache`] tests cover the spawn-side counterpart: not-ready targets
//! are reported as such, and a reset target is recovered once it re-marks
//! itself ready in the new epoch.

use std::mem::{MaybeUninit, align_of};
use std::thread;

use rticx_xbin_rt::{MAX_CORES, ReadyCache, SharedState};

#[test]
fn new_state_is_initialized_and_empty() {
    let state = SharedState::new();
    assert!(state.is_initialized());
    assert_eq!(state.epoch(), 0);
    assert_eq!(state.ready_mask(), 0);
    assert!(!state.is_ready(0));
}

#[test]
fn ready_bits_are_per_core() {
    let state = SharedState::new();

    state.mark_ready(0);
    state.mark_ready(MAX_CORES - 1);
    assert!(state.is_ready(0));
    assert!(state.is_ready(MAX_CORES - 1));
    assert!(!state.is_ready(1));
    assert_eq!(state.ready_mask(), 1 | (1 << (MAX_CORES - 1)));

    state.clear_ready(0);
    assert!(!state.is_ready(0));
    assert!(state.is_ready(MAX_CORES - 1));
    assert_eq!(state.ready_mask(), 1 << (MAX_CORES - 1));
}

#[test]
fn init_clears_bits_and_bumps_epoch() {
    let state = SharedState::new();
    assert_eq!(state.epoch(), 0);
    state.mark_ready(0);
    state.mark_ready(2);

    state.init();
    assert!(state.is_initialized());
    assert_eq!(state.epoch(), 1);
    assert_eq!(state.ready_mask(), 0);

    state.init();
    assert_eq!(state.epoch(), 2);
}

#[test]
fn bump_epoch_returns_the_new_epoch() {
    let state = SharedState::new();
    assert_eq!(state.bump_epoch(), 1);
    assert_eq!(state.bump_epoch(), 2);
    assert_eq!(state.epoch(), 2);
}

#[test]
fn stale_epoch_is_detected_after_peer_reset() {
    let state = SharedState::new();
    state.init();
    let boot_epoch = state.epoch();

    state.mark_ready(0);
    state.mark_ready(1);
    assert!(state.is_ready_at(1, boot_epoch));

    state.clear_ready(1);
    let reset_epoch = state.bump_epoch();
    assert_eq!(reset_epoch, boot_epoch + 1);
    assert!(!state.is_ready(1));
    assert!(
        !state.is_ready_at(1, boot_epoch),
        "the stale epoch must not validate readiness after a reset"
    );

    state.mark_ready(1);
    assert!(state.is_ready(1));
    assert!(
        !state.is_ready_at(1, boot_epoch),
        "the bit is set again, but the cached epoch predates the reset"
    );
    assert!(state.is_ready_at(1, reset_epoch));
    assert!(
        state.is_ready_at(0, reset_epoch),
        "the core that did not reset stays ready"
    );
}

#[test]
fn ready_cache_gates_on_the_cached_epoch() {
    let state = SharedState::new();
    let cache = ReadyCache::new();

    // Nothing is ready before the owner initializes and the target marks
    // itself ready.
    assert!(!cache.is_ready(&state, 1));

    state.init();
    assert!(
        !cache.is_ready(&state, 1),
        "the target has not marked itself ready"
    );

    state.mark_ready(1);
    assert!(
        cache.is_ready(&state, 1),
        "the check refreshes the epoch the first time"
    );
    assert!(cache.is_ready(&state, 1), "and reuses it afterwards");
}

#[test]
fn ready_cache_recovers_after_peer_reset() {
    let state = SharedState::new();
    let cache = ReadyCache::new();
    state.init();
    state.mark_ready(0);
    state.mark_ready(1);

    assert!(cache.is_ready(&state, 1));
    assert!(cache.is_ready(&state, 0), "an unrelated core stays ready");

    // Peer reset: clear the bit and bump the epoch before re-marking.
    state.clear_ready(1);
    let reset_epoch = state.bump_epoch();
    assert!(
        !cache.is_ready(&state, 1),
        "the stale epoch must not validate readiness after a reset"
    );
    assert!(
        cache.is_ready(&state, 0),
        "another target's readiness is unaffected"
    );

    state.mark_ready(1);
    assert!(
        cache.is_ready(&state, 1),
        "the refreshed epoch recovers readiness"
    );
    assert_eq!(
        state.epoch(),
        reset_epoch,
        "the check does not bump the epoch"
    );
}

#[test]
fn ready_cache_refreshes_on_reinitialization() {
    // `init_shared` (an owner re-boot) clears every ready bit and bumps the
    // epoch; a spawner must not trust its cached readiness across it.
    let state = SharedState::new();
    let cache = ReadyCache::new();
    state.init();
    state.mark_ready(1);
    assert!(cache.is_ready(&state, 1));

    state.init();
    assert!(
        !cache.is_ready(&state, 1),
        "reinitialization clears readiness until the target re-marks"
    );
    state.mark_ready(1);
    assert!(cache.is_ready(&state, 1));
}

#[test]
fn ready_cache_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ReadyCache>();
}

#[test]
fn view_at_reads_raw_shared_memory_and_init_seals_it() {
    let mut raw = MaybeUninit::<SharedState>::zeroed();
    let addr = raw.as_mut_ptr() as usize;
    assert_eq!(addr % align_of::<SharedState>(), 0);

    let state = unsafe { &*SharedState::view_at(addr) };
    assert!(!state.is_initialized(), "zeroed memory has no magic");
    assert_eq!(state.epoch(), 0);
    assert_eq!(state.ready_mask(), 0);

    state.init();
    assert!(state.is_initialized());
    assert_eq!(state.epoch(), 1);

    state.mark_ready(1);
    assert!(state.is_ready(1));
    assert!(state.is_ready_at(1, 1));
}

#[test]
fn ready_is_visible_across_threads() {
    let state = SharedState::new();
    let state = &state;

    thread::scope(|scope| {
        let waiter = scope.spawn(move || {
            while !state.is_ready(7) {
                thread::yield_now();
            }
            state.mark_ready(0);
        });

        state.mark_ready(7);
        while !state.is_ready(0) {
            thread::yield_now();
        }

        waiter.join().unwrap();
    });
}

#[test]
fn shared_state_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SharedState>();
}

#[test]
#[should_panic(expected = "exceeds the 32-core ready bitmap")]
fn out_of_range_core_id_panics() {
    SharedState::new().mark_ready(MAX_CORES);
}
