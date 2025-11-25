//! IPC System Call Handlers
//!
//! Handles inter-process communication syscalls

use crate::ipc::fusion_ring::FusionRing;
use crate::ipc::shared_memory::{ShmId, ShmPermissions};
use crate::memory::{MemoryResult, MemoryError};
use crate::memory::address::{PhysicalAddress, VirtualAddress};

/// IPC handle (channel/ring ID)
pub type IpcHandle = u64;

/// Create fusion ring channel
pub fn sys_channel_create() -> MemoryResult<(IpcHandle, IpcHandle)> {
    log::debug!("sys_channel_create");
    
    // 1. Allocate fusion ring
    const DEFAULT_CAPACITY: usize = 256;
    let ring = FusionRing::new(DEFAULT_CAPACITY);
    
    // 2. Create two handles (send/recv)
    use crate::ipc::descriptor::{allocate_descriptor, IpcDescriptor};
    let send_handle = allocate_descriptor(ring, true, false)?;
    let recv_handle = allocate_descriptor(ring, false, true)?;
    
    // 3. Register in process IPC table (handled by allocate_descriptor)
    
    Ok((send_handle.0, recv_handle.0))
}

/// Send message through channel
pub fn sys_channel_send(handle: IpcHandle, data: &[u8], flags: u32) -> MemoryResult<usize> {
    log::debug!("sys_channel_send: handle={}, len={}, flags={}", handle, data.len(), flags);
    
    // 1. Look up handle
    use crate::ipc::descriptor::IpcDescriptor;
    let descriptor = IpcDescriptor(handle);
    
    // 2. Validate permissions (send permission)
    use crate::ipc::capability::{check_permission, CapabilityType};
    if !check_permission(descriptor, CapabilityType::Send) {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 3. Send through fusion ring
    // TODO: Get ring from descriptor table
    // For now, stub implementation
    
    Ok(data.len())
}

/// Receive message from channel
pub fn sys_channel_recv(handle: IpcHandle, buffer: &mut [u8], flags: u32) -> MemoryResult<usize> {
    log::debug!("sys_channel_recv: handle={}, len={}, flags={}", handle, buffer.len(), flags);
    
    // 1. Look up handle
    use crate::ipc::descriptor::IpcDescriptor;
    let descriptor = IpcDescriptor(handle);
    
    // 2. Validate permissions (receive permission)
    use crate::ipc::capability::{check_permission, CapabilityType};
    if !check_permission(descriptor, CapabilityType::Receive) {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 3. Receive from fusion ring
    // TODO: Get ring from descriptor table and recv
    // For now, stub implementation
    
    Ok(0)
}

/// Close IPC handle
pub fn sys_channel_close(handle: IpcHandle) -> MemoryResult<()> {
    log::debug!("sys_channel_close: handle={}", handle);
    
    // 1. Look up handle
    use crate::ipc::descriptor::{IpcDescriptor, close_descriptor};
    let descriptor = IpcDescriptor(handle);
    
    // 2-4. Remove from table, decrement ref count, free if needed
    close_descriptor(descriptor)?;
    
    Ok(())
}

/// Create shared memory region
pub fn sys_shm_create(size: usize, perms: ShmPermissions) -> MemoryResult<ShmId> {
    log::debug!("sys_shm_create: size={}, perms={:?}", size, perms);
    
    // 1-3. Allocate physical memory, create descriptor, register in pool
    use crate::task;
    let creator_pid = task::current().pid() as u64;
    crate::ipc::shared_memory::pool::allocate(size, perms, creator_pid)
}

/// Open named shared memory
pub fn sys_shm_open(name: &str) -> MemoryResult<ShmId> {
    log::debug!("sys_shm_open: name={}", name);
    
    crate::ipc::shared_memory::pool::open_named(name)
}

/// Attach shared memory to process
pub fn sys_shm_attach(id: ShmId, addr: VirtualAddress) -> MemoryResult<VirtualAddress> {
    log::debug!("sys_shm_attach: id={:?}, addr={:?}", id, addr);
    
    // 1. Look up shared memory and get physical address
    let phys_addr = crate::ipc::shared_memory::pool::attach(id)?;
    
    // 2. Map to process address space
    use crate::ipc::shared_memory::mapping::{map_shared, MappingFlags};
    let mapping = map_shared(phys_addr, 4096, addr, MappingFlags::READ_WRITE)?;
    
    // 3. Ref count increment is handled by attach()
    
    Ok(mapping.virt_addr())
}

/// Detach shared memory from process
pub fn sys_shm_detach(addr: VirtualAddress) -> MemoryResult<()> {
    log::debug!("sys_shm_detach: addr={:?}", addr);
    
    // 1-2. Find and unmap (mapping Drop handles unmapping)
    // TODO: Find mapping by virtual address in process table
    
    // 3-4. Decrement ref count, free if needed
    use crate::ipc::shared_memory::pool;
    // TODO: Get ShmId from address mapping
    // pool::detach(shm_id)?;
    
    Ok(())
}

/// Send file descriptor through IPC
pub fn sys_send_fd(handle: IpcHandle, fd: u64) -> MemoryResult<()> {
    log::debug!("sys_send_fd: handle={}, fd={}", handle, fd);
    
    // 1. Look up FD in current process
    // TODO: Implement FD table lookup
    
    // 2. Create capability for FD
    use crate::ipc::capability::{grant_capability, CapabilityFlags};
    use crate::task;
    let current_pid = task::current().pid() as u64;
    // TODO: Get target PID from channel
    let target_pid = 0;
    grant_capability(current_pid, target_pid, CapabilityFlags::READ_WRITE, Some("fd_transfer"))?;
    
    // 3. Send through channel with FD metadata
    // TODO: Implement FD sending protocol
    
    // 4. Increment ref count (handled by capability system)
    
    Ok(())
}

/// Receive file descriptor through IPC
pub fn sys_recv_fd(handle: IpcHandle) -> MemoryResult<u64> {
    log::debug!("sys_recv_fd: handle={}", handle);
    
    // 1. Receive from channel
    // TODO: Implement FD receiving protocol
    
    // 2. Validate capability
    use crate::ipc::capability::{check_permission, CapabilityType};
    use crate::ipc::descriptor::IpcDescriptor;
    if !check_permission(IpcDescriptor(handle), CapabilityType::Receive) {
        return Err(MemoryError::PermissionDenied);
    }
    
    // 3. Install FD in current process FD table
    // TODO: Implement FD installation
    let new_fd = 10; // Stub
    
    // 4. Return new FD
    Ok(new_fd)
}

/// Create pipe (for compatibility)
pub fn sys_pipe() -> MemoryResult<(u64, u64)> {
    log::debug!("sys_pipe");
    
    // Use fusion ring as pipe implementation
    let (send_handle, recv_handle) = sys_channel_create()?;
    
    Ok((send_handle, recv_handle))
}

/// Create socketpair (for compatibility)
pub fn sys_socketpair() -> MemoryResult<(u64, u64)> {
    log::debug!("sys_socketpair");
    
    // Use bidirectional fusion rings
    let (handle1, handle2) = sys_channel_create()?;
    
    Ok((handle1, handle2))
}
