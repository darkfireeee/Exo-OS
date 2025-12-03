//! IPC Channel Syscall Handlers
//!
//! System call interface for high-performance IPC channels.
//! Provides userspace access to the Fusion Ring IPC system.
//!
//! ## Syscall Numbers:
//! - `SYS_CHANNEL_CREATE`: Create a new IPC channel
//! - `SYS_CHANNEL_SEND`: Send message to channel
//! - `SYS_CHANNEL_RECV`: Receive message from channel
//! - `SYS_CHANNEL_CLOSE`: Close channel handle
//! - `SYS_CHANNEL_STAT`: Get channel statistics

pub mod typed;
pub mod broadcast;

// Re-exports
pub use typed::{SyscallTypedChannel, IpcMessage, create_typed_pair};
pub use broadcast::{SyscallBroadcastSender, SyscallBroadcastReceiver, create_broadcast_channel};

use crate::ipc::{FusionRing, IpcError, IpcResult};
use crate::ipc::descriptor::{IpcDescriptor, DescriptorType, DescriptorObject, allocate_descriptor, close_descriptor};
use crate::memory::{MemoryResult, MemoryError};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Channel table for process
static CHANNEL_TABLE: Mutex<BTreeMap<u64, Arc<FusionRing>>> = Mutex::new(BTreeMap::new());
static NEXT_CHANNEL_ID: AtomicU64 = AtomicU64::new(1);

/// Syscall: Create new IPC channel
/// 
/// # Arguments
/// * `capacity` - Ring buffer capacity (power of 2, default 256)
/// * `flags` - Channel flags (reserved)
/// 
/// # Returns
/// * Channel descriptor ID on success
/// * -ENOMEM if out of memory
pub fn sys_channel_create(pid: u64, capacity: usize, flags: u32) -> SyscallResult {
    let cap = if capacity == 0 { 256 } else { capacity.next_power_of_two() };
    
    // Create fusion ring
    let ring = Arc::new(FusionRing::new(cap));
    
    // Generate channel ID
    let channel_id = NEXT_CHANNEL_ID.fetch_add(1, Ordering::Relaxed);
    
    // Store in global table
    CHANNEL_TABLE.lock().insert(channel_id, ring.clone());
    
    // Create descriptor for process
    match allocate_descriptor(pid, DescriptorType::Channel, flags, DescriptorObject::Channel(ring)) {
        Ok(desc) => Ok(desc.0 as i64),
        Err(_) => Err(crate::syscall::SyscallError::NoMemory),
    }
}

/// Syscall: Send message to channel
/// 
/// # Arguments
/// * `desc` - Channel descriptor
/// * `data` - Pointer to message data
/// * `len` - Message length
/// * `flags` - Send flags (0 = blocking, 1 = non-blocking)
/// 
/// # Returns
/// * 0 on success
/// * -EAGAIN if non-blocking and would block
/// * -EINVAL if invalid descriptor
pub fn sys_channel_send(pid: u64, desc: u64, data: *const u8, len: usize, flags: u32) -> SyscallResult {
    // Get channel from descriptor
    let ring = get_channel_from_desc(pid, desc)?;
    
    // Read data from user space
    let data_slice = unsafe { core::slice::from_raw_parts(data, len) };
    
    // Send based on flags
    let result = if flags & 1 != 0 {
        // Non-blocking
        ring.try_send(data_slice)
    } else {
        // Blocking
        ring.send_blocking(data_slice)
    };
    
    match result {
        Ok(()) => Ok(0),
        Err(MemoryError::WouldBlock) => Err(crate::syscall::SyscallError::WouldBlock),
        Err(MemoryError::QueueFull) => Err(crate::syscall::SyscallError::WouldBlock),
        Err(_) => Err(crate::syscall::SyscallError::InvalidArg),
    }
}

/// Syscall: Receive message from channel
/// 
/// # Arguments
/// * `desc` - Channel descriptor
/// * `buffer` - Pointer to receive buffer
/// * `max_len` - Maximum message length
/// * `flags` - Receive flags (0 = blocking, 1 = non-blocking)
/// 
/// # Returns
/// * Message length on success
/// * -EAGAIN if non-blocking and no message
/// * -EINVAL if invalid descriptor
pub fn sys_channel_recv(pid: u64, desc: u64, buffer: *mut u8, max_len: usize, flags: u32) -> SyscallResult {
    let ring = get_channel_from_desc(pid, desc)?;
    
    // Get mutable buffer
    let buffer_slice = unsafe { core::slice::from_raw_parts_mut(buffer, max_len) };
    
    // Receive based on flags
    let result = if flags & 1 != 0 {
        // Non-blocking
        ring.try_recv(buffer_slice)
    } else {
        // Blocking
        ring.recv_blocking(buffer_slice)
    };
    
    match result {
        Ok(len) => Ok(len as i64),
        Err(MemoryError::WouldBlock) => Err(crate::syscall::SyscallError::WouldBlock),
        Err(MemoryError::NotFound) => Err(crate::syscall::SyscallError::WouldBlock),
        Err(_) => Err(crate::syscall::SyscallError::InvalidArg),
    }
}

/// Syscall: Close channel
/// 
/// # Arguments
/// * `desc` - Channel descriptor
/// 
/// # Returns
/// * 0 on success
pub fn sys_channel_close(pid: u64, desc: u64) -> SyscallResult {
    match close_descriptor(pid, IpcDescriptor(desc)) {
        Ok(()) => Ok(0),
        Err(_) => Err(crate::syscall::SyscallError::InvalidArg),
    }
}

/// Syscall: Get channel statistics
/// 
/// # Arguments
/// * `desc` - Channel descriptor
/// * `stats_out` - Pointer to stats structure
/// 
/// # Returns
/// * 0 on success
pub fn sys_channel_stat(pid: u64, desc: u64, stats_out: *mut ChannelStatsSyscall) -> SyscallResult {
    let ring = get_channel_from_desc(pid, desc)?;
    let stats = ring.stats();
    
    unsafe {
        (*stats_out).capacity = stats.capacity;
        (*stats_out).length = stats.length;
        (*stats_out).is_empty = stats.is_empty as u8;
        (*stats_out).is_full = stats.is_full as u8;
        (*stats_out).total_enqueued = stats.total_enqueued;
        (*stats_out).total_dequeued = stats.total_dequeued;
    }
    
    Ok(0)
}

/// Channel statistics for syscall
#[repr(C)]
pub struct ChannelStatsSyscall {
    pub capacity: usize,
    pub length: usize,
    pub is_empty: u8,
    pub is_full: u8,
    pub _reserved: [u8; 6],
    pub total_enqueued: u64,
    pub total_dequeued: u64,
}

/// Helper: Get channel from descriptor
fn get_channel_from_desc(pid: u64, desc: u64) -> Result<Arc<FusionRing>, crate::syscall::SyscallError> {
    // For now, use global table (proper implementation would use per-process descriptor table)
    CHANNEL_TABLE.lock()
        .get(&desc)
        .cloned()
        .ok_or(crate::syscall::SyscallError::InvalidArg)
}
