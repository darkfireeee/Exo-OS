// kernel/src/memory/physical/frame/mod.rs

pub mod descriptor;
pub mod emergency_pool;
pub mod pool;
pub mod reclaim;
pub mod ref_count;

pub use descriptor::{
    FrameDesc, FrameDescEntry, FrameDescriptorTable, FrameFlags, FRAME_DESCRIPTORS, MAX_PHYS_FRAMES,
};
pub use emergency_pool::{
    acquire as acquire_wait_node, init as init_emergency_pool, release as release_wait_node,
    stats as emergency_pool_stats, EmergencyPool, EmergencyPoolStats, WaitNode, EMERGENCY_POOL,
};
pub use pool::{init_cpu_pool, PerCpuFramePool, PerCpuPoolStats, PerCpuPoolTable, PER_CPU_POOLS};
pub use reclaim::{
    demote_to_inactive, enter_memalloc, in_memalloc, kswapd_reclaim, leave_memalloc, lru_add_new,
    lru_counts, lru_remove, promote_to_active, ReclaimResult, ReclaimStats, HIGH_WATER_ACTIVE,
    LRU_LIST_SIZE, RECLAIM_STATS,
};
pub use ref_count::{cow_can_promote, AtomicRefCount, CowBreakResult, RefCountDecResult};
