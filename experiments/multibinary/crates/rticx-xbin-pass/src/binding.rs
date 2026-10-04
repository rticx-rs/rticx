//! Distribution capability binding of the cross-binary code-generation
//! contract (M6.9-T1).
//!
//! Starting with M6.9 the distribution is the single owner of IPC memory: it
//! reports, per physical core, which other physical cores it can reach and
//! through which pool. The project (`rticx.toml`) carries no IPC address at
//! all. The driver matches the two endpoints of every dual and allocates both
//! directions inside one shared budget (M6.9-T4); the distribution ships the
//! linker reservations that keep the pools out of every binary's
//! `.data`/`.bss` (M6.9-T7).
//!
//! A distribution implements the two queries on
//! [`XbinPassBackend`](crate::XbinPassBackend) —
//! [`physical_core`](crate::XbinPassBackend::physical_core) and
//! [`ipc_pools`](crate::XbinPassBackend::ipc_pools) — with its own vocabulary:
//!
//! ```ignore
//! fn physical_core(&self, local_core: u32) -> PhysicalCore {
//!     PhysicalCore(self.core_ids[local_core as usize])
//! }
//!
//! fn ipc_pools(&self, local_core: u32) -> Vec<IpcPool> {
//!     match self.physical_core(local_core) {
//!         PhysicalCore(0) => vec![IpcPool {
//!             id: PoolId::new("sram3"),
//!             peer: PhysicalCore(1),
//!             base_local: 0x3004_0000,
//!             base_peer: 0x1004_0000,
//!             budget: 4096,
//!             policy: CachePolicy::NormalNonCacheableShareable,
//!         }],
//!         _ => Vec::new(),
//!     }
//! }
//! ```
//!
//! The adjacency is exactly the set of pools: an edge `{A, B}` exists iff `A`
//! reports a pool whose peer is `B` **and** `B` reports the matching pool (the
//! same [`PoolId`] with the opposite physical core). The two endpoints of a
//! dual see the same pool from their own side: each reports its own view as
//! `base_local` and the peer's view as `base_peer`.

/// Stable, distro-defined identifier of a physical core.
///
/// The id is a *distro* vocabulary, not a project one: for example `0`/`1` on
/// the mock, or `"cm7"`/`"cm4"` on the STM32H7. The two applications that run
/// on the endpoints of a dual agree on it even though their local core indexes
/// may differ, so the driver can match the two sides of a pool across
/// application manifests (M6.9-T4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysicalCore(pub u32);

impl PhysicalCore {
    /// Creates a physical-core id.
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the numeric id.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for PhysicalCore {
    fn from(id: u32) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for PhysicalCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Symbolic, distro-defined identifier of an IPC pool.
///
/// The id names a pool in the distribution's vocabulary (`"sram3"`,
/// `"axi"`, …). It is the key the driver uses to match the two endpoints'
/// capability entries: the same `PoolId` with opposite physical cores is one
/// dual (M6.9-T4).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PoolId(pub String);

impl PoolId {
    /// Creates a pool id from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for PoolId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

impl From<String> for PoolId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl std::fmt::Display for PoolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Cache/MPU policy of an IPC pool.
///
/// v1 supports only [`CachePolicy::NormalNonCacheableShareable`]: atomics
/// provide ordering, not cache maintenance, so a cacheable pool needs explicit
/// clean/invalidate around every shared access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CachePolicy {
    /// Normal, Non-cacheable, Shareable — the supported v1 policy.
    NormalNonCacheableShareable,
    /// Normal, cacheable, Shareable: unsupported in v1. A cacheable pool would
    /// need explicit clean/invalidate around every shared access; the
    /// distribution's generated doorbell/read bodies would own that
    /// maintenance (D2).
    NormalCacheableShareable,
}

impl CachePolicy {
    /// Returns whether the policy maps the pool cacheable and therefore needs
    /// explicit cache maintenance.
    pub const fn is_cacheable(self) -> bool {
        matches!(self, Self::NormalCacheableShareable)
    }
}

/// One shared IPC pool of an unordered core pair (a *dual*), as seen from one
/// endpoint's side.
///
/// Both directions of a dual (`A -> B` and `B -> A`) allocate inside the same
/// physical block under [`IpcPool::budget`], so the two endpoints must report
/// the same [`IpcPool::id`], budget and policy, with the physical cores and
/// the two base views swapped (M6.9-T4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcPool {
    /// Symbolic, distro-defined pool id (the dual's matching key).
    pub id: PoolId,
    /// The other endpoint of the dual, in the distribution's physical-core
    /// vocabulary.
    pub peer: PhysicalCore,
    /// This core's view (base address) of the pool.
    pub base_local: u32,
    /// The peer's view (base address) of the pool; aliases are allowed.
    pub base_peer: u32,
    /// Bytes the distribution reserves in the pool for IPC, shared by both
    /// directions of the dual.
    pub budget: u32,
    /// Cache/MPU policy of the pool.
    pub policy: CachePolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binding types carry the documented fields: the pool names its peer
    /// and both per-core views of one shared budget (M6.9-T1).
    #[test]
    fn ipc_pool_carries_both_views_and_the_shared_budget() {
        let pool = IpcPool {
            id: PoolId::new("sram3"),
            peer: PhysicalCore::new(1),
            base_local: 0x3004_0000,
            base_peer: 0x1004_0000,
            budget: 4096,
            policy: CachePolicy::NormalNonCacheableShareable,
        };

        assert_eq!(pool.id.as_str(), "sram3");
        assert_eq!(pool.peer.get(), 1);
        assert_eq!(pool.base_local, 0x3004_0000);
        assert_eq!(pool.base_peer, 0x1004_0000);
        assert_eq!(pool.budget, 4096);
        assert!(!pool.policy.is_cacheable());
    }

    /// A `PoolId` is a value key: equal strings compare equal, so the driver
    /// can match the two endpoints' entries (M6.9-T4).
    #[test]
    fn pool_id_is_a_value_key() {
        assert_eq!(PoolId::from("axi"), PoolId::new("axi"));
        assert_ne!(PoolId::new("axi"), PoolId::new("sram3"));
    }

    /// `CachePolicy` distinguishes the supported v1 policy from the cacheable
    /// fallback.
    #[test]
    fn cache_policy_marks_the_cacheable_fallback() {
        assert!(!CachePolicy::NormalNonCacheableShareable.is_cacheable());
        assert!(CachePolicy::NormalCacheableShareable.is_cacheable());
    }

    /// Physical-core ids order deterministically, so allocation and rendering
    /// over pools are stable.
    #[test]
    fn physical_core_orders_by_id() {
        let mut cores = [PhysicalCore(2), PhysicalCore(0), PhysicalCore(1)];
        cores.sort_unstable();
        assert_eq!(cores, [PhysicalCore(0), PhysicalCore(1), PhysicalCore(2)]);
    }
}
