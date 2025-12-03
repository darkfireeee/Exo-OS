//! IPC Core - Ultra-High Performance Inter-Process Communication
//!
//! This is the beating heart of Exo-OS IPC, designed to crush Linux performance.
//!
//! ## Performance Targets (vs Linux pipes ~1200 cycles):
//! - **Inline path** (≤40B): ~80-100 cycles (12x faster!)
//! - **Zero-copy path**: ~200-300 cycles (4-6x faster)
//! - **Batch path**: ~25-35 cycles/msg amortized (35-50x faster!)
//! - **Multicast per receiver**: +40 cycles
//!
//! ## Key Innovations:
//! 1. **Lock-free MPMC**: Multiple producers/consumers without locks
//! 2. **Adaptive coalescing**: Dynamic batching based on load
//! 3. **Cache-line aligned slots**: Zero false sharing
//! 4. **Sequence-based ordering**: Wait-free progress guarantees
//! 5. **Integrated scheduler wake**: Direct thread wake without syscalls
//! 6. **Credit-based flow control**: Intelligent backpressure
//! 7. **Priority lanes**: 5-level priority with separate queues
//! 8. **Multicast/Anycast**: Efficient one-to-many patterns
//! 9. **Cache prefetching**: Predictive data loading
//! 10. **Timestamped latency tracking**: Built-in performance monitoring

pub mod sequence;
pub mod mpmc_ring;
pub mod slot_v2;
pub mod transfer;
pub mod wait_queue;
pub mod endpoint;
pub mod futex;
pub mod priority_queue;
pub mod benchmark;
pub mod advanced;
pub mod ultra_fast_ring;
pub mod advanced_channels;

pub use sequence::{Sequence, SequenceGroup, CacheLineCounter};
pub use mpmc_ring::{MpmcRing, RingConfig, ProducerToken, ConsumerToken};
pub use slot_v2::{SlotV2, SlotState, SlotHeader, SLOT_SIZE};
pub use transfer::{TransferMode, TransferDescriptor, TransferResult};
pub use wait_queue::{WaitQueue, WaitNode, WakeReason};
pub use endpoint::{Endpoint, EndpointId, EndpointFlags};
pub use futex::{FutexMutex, FutexCondvar, FutexSemaphore};
pub use priority_queue::{BoundedPriorityQueue, priority};

// Advanced IPC exports
pub use advanced::{
    CoalesceMode, CoalesceController, PriorityClass, LaneStats,
    CreditController, MulticastGroup, MulticastReceiver,
    AnycastGroup, AnycastPolicy, IpcPerfCounters, GLOBAL_PERF_COUNTERS,
    prefetch_read, prefetch_write, rdtsc, rdtscp,
    CACHE_LINE_SIZE, PREFETCH_STRIDE,
};
pub use ultra_fast_ring::{UltraFastRing, UltraFastRingStats, FAST_INLINE_MAX};
pub use advanced_channels::{
    PriorityChannel, PriorityChannelStats,
    MulticastChannel, MulticastReceiverState,
    AnycastChannel, AnycastReceiverState,
    RequestReplyChannel,
};

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// IPC channel handle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelHandle(pub u64);

/// Channel statistics for performance monitoring
#[derive(Debug, Default)]
pub struct ChannelStats {
    /// Messages sent via inline path
    pub inline_sends: AtomicU64,
    /// Messages sent via zero-copy path
    pub zerocopy_sends: AtomicU64,
    /// Messages sent via batch path
    pub batch_sends: AtomicU64,
    /// Total bytes transferred
    pub bytes_transferred: AtomicU64,
    /// Send operations that had to wait
    pub send_waits: AtomicU64,
    /// Receive operations that had to wait
    pub recv_waits: AtomicU64,
    /// Spurious wakeups (woke but nothing to do)
    pub spurious_wakes: AtomicU64,
}

impl ChannelStats {
    pub const fn new() -> Self {
        Self {
            inline_sends: AtomicU64::new(0),
            zerocopy_sends: AtomicU64::new(0),
            batch_sends: AtomicU64::new(0),
            bytes_transferred: AtomicU64::new(0),
            send_waits: AtomicU64::new(0),
            recv_waits: AtomicU64::new(0),
            spurious_wakes: AtomicU64::new(0),
        }
    }
    
    #[inline]
    pub fn record_inline_send(&self, bytes: usize) {
        self.inline_sends.fetch_add(1, Ordering::Relaxed);
        self.bytes_transferred.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    
    #[inline]
    pub fn record_zerocopy_send(&self, bytes: usize) {
        self.zerocopy_sends.fetch_add(1, Ordering::Relaxed);
        self.bytes_transferred.fetch_add(bytes as u64, Ordering::Relaxed);
    }
    
    #[inline]
    pub fn record_batch_send(&self, count: usize, bytes: usize) {
        self.batch_sends.fetch_add(count as u64, Ordering::Relaxed);
        self.bytes_transferred.fetch_add(bytes as u64, Ordering::Relaxed);
    }
}

/// IPC configuration constants
pub mod config {
    /// Default ring capacity (power of 2)
    pub const DEFAULT_RING_SIZE: usize = 1024;
    
    /// Maximum inline message size (fits in one cache line with header)
    pub const MAX_INLINE_SIZE: usize = 56;
    
    /// Large message threshold for zero-copy
    pub const ZEROCOPY_THRESHOLD: usize = 4096;
    
    /// Batch accumulation threshold
    pub const BATCH_THRESHOLD: usize = 16;
    
    /// Spin iterations before blocking
    pub const SPIN_ITERATIONS: u32 = 100;
    
    /// Maximum batch size
    pub const MAX_BATCH_SIZE: usize = 64;
    
    /// Number of priority levels
    pub const PRIORITY_LEVELS: usize = 8;
}
