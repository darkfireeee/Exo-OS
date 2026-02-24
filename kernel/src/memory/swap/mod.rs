// kernel/src/memory/swap/mod.rs
//
// Module swap — éviction de pages et gestion des dispositifs swap.

pub mod backend;
pub mod policy;
pub mod compress;
pub mod cluster;

pub use backend::{
    SwapSlot, SwapDevice, SwapError, SwapPte,
    SwapBackendRegistry, SWAP_BACKEND, MAX_SWAP_DEVICES,
};
pub use compress::{
    CompressBackend, Lz4Lite, ZswapPool, ZswapSlot,
    ZSWAP_POOL, ZswapStoreResult, ZswapLoadResult,
    ZSWAP_SLOT_SIZE, MAX_ZSWAP_SLOTS,
};
pub use cluster::{
    SwapCluster, ClusterEntry, ClusterManager, ClusterStats,
    CLUSTER_MANAGER, CLUSTER_SIZE, MAX_CLUSTER_QUEUE,
};
pub use policy::{
    EvictCandidate, ClockEvictList, EVICT_LIST,
    SwapWatermarks, SWAP_WATERMARKS, SWAP_POLICY_STATS,
    should_swap, is_critical,
};
