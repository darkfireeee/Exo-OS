//! IPC (Inter-Process Communication) subsystem
//! 
//! Provides high-performance message passing between processes:
//! - Fusion Rings: Inline path (≤56B, ~400 cycles) + Zero-copy (>56B, ~900 cycles)
//! - Channels: Multi-producer, multi-consumer message queues
//! - Shared memory regions for bulk transfers

pub mod message;
pub mod channel;
pub mod fusion_ring;
pub mod shared_memory;

// Re-exports
pub use message::{Message, MessageHeader, MessageType, INLINE_THRESHOLD};
pub use fusion_ring::{FusionRing, Ring, Slot};

use core::sync::atomic::{AtomicU64, Ordering};

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
}

pub type IpcResult<T> = Result<T, IpcError>;

/// Initialize IPC subsystem
pub fn init() {
    // Initialize global channel ID counter
    NEXT_CHANNEL_ID.store(1, Ordering::Relaxed);
    
    // Initialize shared memory pool
    shared_memory::pool::init();
    
    log::info!("IPC subsystem initialized");
    log::debug!("  - Fusion rings: Inline (≤56B, ~400 cycles) + Zerocopy (>56B, ~900 cycles)");
    log::debug!("  - Channel ID counter: {}", NEXT_CHANNEL_ID.load(Ordering::Relaxed));
}
