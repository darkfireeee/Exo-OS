// kernel/src/memory/core/constants.rs
//
// Fundamental memory management constants — Exo-OS Layer 0
// No external dependencies. All values are statically verified
// compile-time constants.

// ─────────────────────────────────────────────────────────────────────────────
// PAGE SIZES
// ─────────────────────────────────────────────────────────────────────────────

/// Standard page size (4 KiB)
pub const PAGE_SIZE: usize = 4096;

/// Standard page shift
pub const PAGE_SHIFT: usize = 12;

/// Mask to extract offset within a page
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

/// Huge page size (2 MiB)
pub const HUGE_PAGE_SIZE: usize = 2 * 1024 * 1024;

/// Huge page shift (21 bits)
pub const HUGE_PAGE_SHIFT: usize = 21;

/// Mask to extract offset within a huge page
pub const HUGE_PAGE_MASK: usize = HUGE_PAGE_SIZE - 1;

/// Gigapage size (1 GiB)
pub const GIGA_PAGE_SIZE: usize = 1024 * 1024 * 1024;

/// Gigapage shift (30 bits)
pub const GIGA_PAGE_SHIFT: usize = 30;

// ─────────────────────────────────────────────────────────────────────────────
// CACHE LINE
// ─────────────────────────────────────────────────────────────────────────────

/// x86_64 cache line size (64 bytes)
pub const CACHE_LINE_SIZE: usize = 64;

/// Cache line shift
pub const CACHE_LINE_SHIFT: usize = 6;

/// Cache line alignment mask
pub const CACHE_LINE_MASK: usize = CACHE_LINE_SIZE - 1;

// ─────────────────────────────────────────────────────────────────────────────
// BUDDY ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum order of the buddy allocator (2^12 = 4096 pages = 16 MiB)
pub const BUDDY_MAX_ORDER: usize = 12;

/// Number of buddy allocator levels
pub const BUDDY_ORDER_COUNT: usize = BUDDY_MAX_ORDER + 1;

/// Maximum size allocated by buddy (in pages)
pub const BUDDY_MAX_PAGES: usize = 1 << BUDDY_MAX_ORDER;

/// Maximum size allocated by buddy (in bytes)
pub const BUDDY_MAX_BYTES: usize = BUDDY_MAX_PAGES * PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// SLAB ALLOCATOR
// ─────────────────────────────────────────────────────────────────────────────

/// Number of size classes in slab allocator
pub const SLAB_NR_SIZE_CLASSES: usize = 16;

/// Minimum slab object size (8-byte aligned)
pub const SLAB_MIN_OBJ_SIZE: usize = 8;

/// Maximum slab object size (2 KiB — beyond this -> buddy)
pub const SLAB_MAX_OBJ_SIZE: usize = 2048;

/// Maximum number of objects per slab page
pub const SLAB_MAX_OBJS_PER_PAGE: usize = PAGE_SIZE / SLAB_MIN_OBJ_SIZE;

/// Threshold: above this size, slab dispatches directly to buddy
pub const SLAB_LARGE_THRESHOLD: usize = SLAB_MAX_OBJ_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// PER-CPU POOLS
// ─────────────────────────────────────────────────────────────────────────────

/// Number of frames in each per-CPU pool
pub const PER_CPU_POOL_SIZE: usize = 512;

/// Refill threshold for per-CPU pool (25%)
pub const PER_CPU_REFILL_THRESHOLD: usize = PER_CPU_POOL_SIZE / 4;

/// Drain threshold for per-CPU pool (75%)
pub const PER_CPU_DRAIN_THRESHOLD: usize = (PER_CPU_POOL_SIZE * 3) / 4;

// ─────────────────────────────────────────────────────────────────────────────
// EMERGENCY POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Number of WaitNodes pre-allocated at boot in EmergencyPool
/// MUST be initialized BEFORE any other kernel module (RULE EMERGENCY-01)
/// RULE SCHED-POOL (V-12): >= 256 — otherwise trivial exhaustion DoS.
pub const EMERGENCY_POOL_SIZE: usize = 256;

// ─────────────────────────────────────────────────────────────────────────────
// TLB SHOOTDOWN
// ─────────────────────────────────────────────────────────────────────────────

/// TLB shootdown IPI timeout (cycles ~100µs at 3 GHz)
pub const TLB_SHOOTDOWN_TIMEOUT_CYCLES: u64 = 300_000;

/// Maximum number of supported CPUs
pub const MAX_CPUS: usize = 256;

/// Maximum number of supported NUMA nodes
pub const MAX_NUMA_NODES: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// MEMORY ZONES
// ─────────────────────────────────────────────────────────────────────────────

/// End of DMA zone (<16 MiB — legacy 32-bit devices)
pub const ZONE_DMA_END: usize = 16 * 1024 * 1024;

/// End of DMA32 zone (<4 GiB — 32-bit PCIe devices)
pub const ZONE_DMA32_END: usize = 4 * 1024 * 1024 * 1024;

/// Start of NORMAL zone (>= 4 GiB on 64-bit systems)
pub const ZONE_NORMAL_START: usize = ZONE_DMA32_END;

// ─────────────────────────────────────────────────────────────────────────────
// DMA
// ─────────────────────────────────────────────────────────────────────────────

/// Size of DMA ring (512 descriptors, power of two mandatory)
pub const DMA_RING_SIZE: usize = 512;

/// DMA ring mask
pub const DMA_RING_MASK: usize = DMA_RING_SIZE - 1;

/// DMA completion timeout (cycles ~500ns at 3 GHz)
pub const DMA_COMPLETION_TIMEOUT_CYCLES: u64 = 1_500;

/// Maximum size of continuous DMA transfer
pub const DMA_MAX_TRANSFER_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

// ─────────────────────────────────────────────────────────────────────────────
// IPC RING (shared with ipc/ via memory/utils/)
// ─────────────────────────────────────────────────────────────────────────────

/// IPC ring size (power of two mandatory)
pub const IPC_RING_SIZE: usize = 4096;

// ─────────────────────────────────────────────────────────────────────────────
// KERNEL HEAP
// ─────────────────────────────────────────────────────────────────────────────

/// Kernel heap TLS cache size (hot path <25 cycles)
pub const HEAP_TLS_CACHE_SIZE: usize = 64;

/// Magazine size (batch alloc/free)
pub const HEAP_MAGAZINE_SIZE: usize = 128;

/// TLS cache drain threshold to global pool
pub const HEAP_TLS_DRAIN_COUNT: usize = HEAP_TLS_CACHE_SIZE / 2;

// ─────────────────────────────────────────────────────────────────────────────
// STACK CANARY
// ─────────────────────────────────────────────────────────────────────────────

/// Initial value of kernel stack canary (compiled random pattern)
/// Replaced at boot by a value read from RDRAND or RDSEED
pub const STACK_CANARY_INITIAL: u64 = 0x_DEAD_BEEF_CAFE_BABE;

// ─────────────────────────────────────────────────────────────────────────────
// FUTEX
// ─────────────────────────────────────────────────────────────────────────────

/// Number of buckets in futex hash table (must be power of two).
/// RULE MEM-FUTEX (V-34): >= 4096 + SipHash-keyed — anti-DoS via collision.
pub const FUTEX_HASH_BUCKETS: usize = 4096;

/// Mask for futex table hash
pub const FUTEX_HASH_MASK: usize = FUTEX_HASH_BUCKETS - 1;

// ─────────────────────────────────────────────────────────────────────────────
// STATIC CHECKS
// ─────────────────────────────────────────────────────────────────────────────

const _: () = assert!(
    PAGE_SIZE.is_power_of_two(),
    "PAGE_SIZE doit être une puissance de 2"
);
const _: () = assert!(
    HUGE_PAGE_SIZE.is_power_of_two(),
    "HUGE_PAGE_SIZE doit être une puissance de 2"
);
const _: () = assert!(
    GIGA_PAGE_SIZE.is_power_of_two(),
    "GIGA_PAGE_SIZE doit être une puissance de 2"
);
const _: () = assert!(
    CACHE_LINE_SIZE.is_power_of_two(),
    "CACHE_LINE_SIZE doit être une puissance de 2"
);
const _: () = assert!(
    DMA_RING_SIZE.is_power_of_two(),
    "DMA_RING_SIZE doit être une puissance de 2"
);
const _: () = assert!(
    IPC_RING_SIZE.is_power_of_two(),
    "IPC_RING_SIZE doit être une puissance de 2"
);
const _: () = assert!(
    FUTEX_HASH_BUCKETS.is_power_of_two(),
    "FUTEX_HASH_BUCKETS doit être une puissance de 2"
);
const _: () = assert!(PAGE_SHIFT == 12, "PAGE_SHIFT doit être 12 pour x86_64");
const _: () = assert!(HUGE_PAGE_SHIFT == 21, "HUGE_PAGE_SHIFT doit être 21 (2MiB)");
const _: () = assert!(
    EMERGENCY_POOL_SIZE >= 256,
    "EmergencyPool trop petit (< 256) — risque DoS par épuisement (SCHED-POOL)"
);
const _: () = assert!(BUDDY_MAX_ORDER <= 20, "BUDDY_MAX_ORDER excessif");
const _: () = assert!(
    SLAB_LARGE_THRESHOLD <= SLAB_MAX_OBJ_SIZE,
    "SLAB_LARGE_THRESHOLD ne peut pas depasser SLAB_MAX_OBJ_SIZE"
);
const _: () = assert!(MAX_CPUS <= 4096, "MAX_CPUS dépasse la limite x2APIC");
const _: () = assert!(
    ZONE_DMA_END == 16 * 1024 * 1024,
    "Zone DMA doit se terminer à 16MiB"
);
const _: () = assert!(
    1 << PAGE_SHIFT == PAGE_SIZE,
    "PAGE_SHIFT incohérent avec PAGE_SIZE"
);
const _: () = assert!(
    1 << HUGE_PAGE_SHIFT == HUGE_PAGE_SIZE,
    "HUGE_PAGE_SHIFT incohérent"
);
