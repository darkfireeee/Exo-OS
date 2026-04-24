// kernel/src/memory/swap/mod.rs
//
// Module swap — éviction de pages et gestion des dispositifs swap.

pub mod backend;
pub mod cluster;
pub mod compress;
pub mod policy;

pub use backend::{
    SwapBackendRegistry, SwapDevice, SwapError, SwapPte, SwapSlot, MAX_SWAP_DEVICES, SWAP_BACKEND,
};
pub use cluster::{
    ClusterEntry, ClusterManager, ClusterStats, SwapCluster, CLUSTER_MANAGER, CLUSTER_SIZE,
    MAX_CLUSTER_QUEUE,
};
pub use compress::{
    CompressBackend, Lz4Lite, ZswapLoadResult, ZswapPool, ZswapSlot, ZswapStoreResult,
    MAX_ZSWAP_SLOTS, ZSWAP_POOL, ZSWAP_SLOT_SIZE,
};
pub use policy::{
    is_critical, should_swap, ClockEvictList, EvictCandidate, SwapWatermarks, EVICT_LIST,
    SWAP_POLICY_STATS, SWAP_WATERMARKS,
};
