//! Fixture IPC capability table of the mock distributions (M6.9-T9).
//!
//! The `xbin-mock-distro` proc-macro half and the metadata-only
//! `metadata-macro` fixture stand-in both need the same capability binding, so
//! the mock vocabulary lives here in a normal library the two proc-macro
//! crates can share. It is the compile-time counterpart of the runtime pools
//! declared by `xbin-mock-runtime::system` (M6.9-T6).
//!
//! Physical cores 0, 1 and 2 are fully connected through the three duals
//! `{0, 1}`, `{1, 2}` and `{0, 2}` (M6.9-T1). The mock has a single address
//! space, so both endpoints of a dual see its base at the same address; the
//! budget matches the runtime's 4096-byte pools.

use rticx_xbin_pass::{CachePolicy, IpcPool, PhysicalCore, PoolId};

/// Bytes reserved for both directions of one dual.
pub const BUDGET: u32 = 4096;

/// Every IPC pool the mock physical core `local` can reach (M6.9-T1).
///
/// The entries are ordered by ascending peer for determinism; a physical core
/// outside the fixture topology reaches nothing.
pub fn fixture_pools(local: PhysicalCore) -> Vec<IpcPool> {
    let dual = |id: &str, peer: u32, base: u32| IpcPool {
        id: PoolId::new(id),
        peer: PhysicalCore(peer),
        base_local: base,
        base_peer: base,
        budget: BUDGET,
        policy: CachePolicy::NormalNonCacheableShareable,
    };

    match local.0 {
        0 => vec![
            dual("mock-0-1", 1, 0x3000_0000),
            dual("mock-0-2", 2, 0x3000_2000),
        ],
        1 => vec![
            dual("mock-0-1", 0, 0x3000_0000),
            dual("mock-1-2", 2, 0x3000_1000),
        ],
        2 => vec![
            dual("mock-0-2", 0, 0x3000_2000),
            dual("mock-1-2", 1, 0x3000_1000),
        ],
        _ => Vec::new(),
    }
}
