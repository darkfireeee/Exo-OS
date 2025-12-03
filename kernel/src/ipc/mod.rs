//! IPC (Inter-Process Communication) subsystem
//!
//! Ultra-high performance IPC designed to crush Linux performance.
//!
//! ## Architecture:
//! - **Core**: Lock-free MPMC rings, futex, priority queues
//! - **Fusion Rings**: Adaptive inline/zerocopy/batch paths
//! - **Named Channels**: System-wide named pipes with permissions
//! - **Shared Memory**: Zero-copy large transfers
//! - **Advanced**: Multicast, anycast, priority lanes, request-reply
//!
//! ## Performance Targets (vs Linux):
//! - Inline send (≤40B): ~80-100 cycles (Linux pipes: ~1200) = 12-15x faster
//! - Zero-copy: ~200-300 cycles = 4-6x faster
//! - Batch: ~25-35 cycles/msg amortized = 35-50x faster
//! - Futex uncontended: ~20 cycles (Linux: ~50) = 2.5x faster
//! - Multicast per-receiver: +40 cycles additional

pub mod core;
pub mod message;
pub mod channel;
pub mod fusion_ring;
pub mod shared_memory;
pub mod named;
pub mod capability;
pub mod descriptor;

// Core re-exports
pub use self::core::{
    MpmcRing, RingConfig, Sequence, SequenceGroup,
    SlotV2, SlotState, SLOT_SIZE,
    TransferMode, TransferDescriptor,
    Endpoint, EndpointId, EndpointFlags,
    WaitQueue, WaitNode, WakeReason,
    FutexMutex, FutexCondvar, FutexSemaphore,
    BoundedPriorityQueue, priority,
    ChannelHandle, ChannelStats,
    // Advanced IPC
    CoalesceMode, CoalesceController, PriorityClass, LaneStats,
    CreditController, AnycastPolicy, IpcPerfCounters, GLOBAL_PERF_COUNTERS,
    UltraFastRing, UltraFastRingStats, FAST_INLINE_MAX,
    PriorityChannel, PriorityChannelStats,
    MulticastChannel, MulticastReceiverState,
    AnycastChannel, AnycastReceiverState,
    RequestReplyChannel,
    prefetch_read, prefetch_write, rdtsc, rdtscp,
};

// Message re-exports
pub use message::{Message, MessageHeader, MessageType, INLINE_THRESHOLD};

// Fusion ring re-exports
pub use fusion_ring::{FusionRing, Ring, Slot};

// Named channel re-exports
pub use named::{
    ChannelNamespace, NamedChannelHandle, ChannelInfo,
    ChannelType, ChannelPermissions, ChannelFlags,
    create_channel, open_channel, unlink_channel,
    list_channels, stat_channel, pipe, mkfifo,
};

use ::core::sync::atomic::{AtomicU64, Ordering};

/// Global channel ID counter
static NEXT_CHANNEL_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new unique channel ID
pub fn alloc_channel_id() -> u64 {
    NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed)
}

/// IPC error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Channel not found
    NotFound,
    /// Channel is full
    Full,
    /// Channel is empty
    Empty,
    /// Permission denied
    PermissionDenied,
    /// Invalid message size
    InvalidSize,
    /// Ring buffer overflow
    Overflow,
    /// Timeout
    Timeout,
    /// Resource temporarily unavailable
    WouldBlock,
    /// Channel already exists
    AlreadyExists,
    /// Invalid channel name
    InvalidName,
    /// Channel is closed
    ChannelClosed,
    /// Channel is busy
    Busy,
    /// Interrupted
    Interrupted,
}

pub type IpcResult<T> = Result<T, IpcError>;

/// Initialize IPC subsystem
pub fn init() {
    // Initialize global channel ID counter
    NEXT_CHANNEL_ID.store(1, Ordering::Relaxed);
    
    // Initialize shared memory pool
    shared_memory::pool::init();
    
    log::info!("IPC subsystem initialized - Linux Crusher Edition");
    log::info!("  Performance targets vs Linux pipes (~1200 cycles):");
    log::info!("    - Inline (≤40B): ~80-100 cycles (12-15x faster)");
    log::info!("    - Zero-copy: ~200-300 cycles (4-6x faster)");
    log::info!("    - Batch: ~25-35 cycles/msg (35-50x faster)");
    log::info!("    - Futex: ~20 cycles uncontended (2.5x faster)");
    log::info!("    - Multicast: +40 cycles/receiver");
    log::info!("  Advanced features: Priority lanes, Anycast, Request-Reply");
}
