// kernel/src/memory/physical/frame/mod.rs

pub mod descriptor;
pub mod ref_count;
pub mod pool;
pub mod emergency_pool;
pub mod reclaim;

pub use descriptor::{
    FrameDesc, FrameFlags,
    FrameDescEntry, FrameDescriptorTable, FRAME_DESCRIPTORS, MAX_PHYS_FRAMES,
};
pub use ref_count::{AtomicRefCount, RefCountDecResult, CowBreakResult, cow_can_promote};
pub use pool::{PerCpuFramePool, PerCpuPoolStats, PerCpuPoolTable, PER_CPU_POOLS,
               init_cpu_pool};
pub use emergency_pool::{EmergencyPool, EmergencyPoolStats, WaitNode, EMERGENCY_POOL,
                         init as init_emergency_pool, acquire as acquire_wait_node,
                         release as release_wait_node, stats as emergency_pool_stats};
pub use reclaim::{
    ReclaimResult, ReclaimStats, RECLAIM_STATS,
    kswapd_reclaim, lru_add_new, lru_remove, promote_to_active, demote_to_inactive,
    enter_memalloc, leave_memalloc, in_memalloc, lru_counts,
    LRU_LIST_SIZE, HIGH_WATER_ACTIVE,
};
