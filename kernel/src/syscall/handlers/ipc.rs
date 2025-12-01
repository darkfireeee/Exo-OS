//! IPC System Call Handlers
//!
//! Handles inter-process communication syscalls

use crate::ipc::fusion_ring::FusionRing;
use crate::ipc::shared_memory::{ShmId, ShmPermissions};
use crate::memory::address::{PhysicalAddress, VirtualAddress};
use crate::memory::{MemoryError, MemoryResult};
use crate::scheduler::SCHEDULER;

/// IPC handle (channel/ring ID)
pub type IpcHandle = u64;

/// Create fusion ring channel
pub fn sys_channel_create() -> MemoryResult<(IpcHandle, IpcHandle)> {
    log::debug!("sys_channel_create");

    // 1. Allocate fusion ring
    const DEFAULT_CAPACITY: usize = 256;
    let ring = FusionRing::new(DEFAULT_CAPACITY);

    // 2. Create two handles (send/recv)
    // Stub: allocate_descriptor missing
    // use crate::ipc::descriptor::{allocate_descriptor, IpcDescriptor};
    // let send_handle = allocate_descriptor(ring, true, false)?;
    // let recv_handle = allocate_descriptor(ring, false, true)?;
    let send_handle = 100; // Stub
    let recv_handle = 101; // Stub

    // 3. Register in process IPC table (handled by allocate_descriptor)

    Ok((send_handle, recv_handle))
}

/// Send message through channel
pub fn sys_channel_send(handle: IpcHandle, data: &[u8], flags: u32) -> MemoryResult<usize> {
    log::debug!(
        "sys_channel_send: handle={}, len={}, flags={}",
        handle,
        data.len(),
        flags
    );

    // 1. Look up handle
    // use crate::ipc::descriptor::IpcDescriptor;
    // let descriptor = IpcDescriptor(handle);

    // 2. Validate permissions (send permission)
    // use crate::ipc::capability::{check_permission, CapabilityType};
    // if !check_permission(descriptor, CapabilityType::Send) {
    //     return Err(MemoryError::PermissionDenied);
    // }

    // 3. Send through fusion ring
    // TODO: Get ring from descriptor table
    // For now, stub implementation

    Ok(data.len())
}

/// Receive message from channel
pub fn sys_channel_recv(handle: IpcHandle, buffer: &mut [u8], flags: u32) -> MemoryResult<usize> {
    log::debug!(
        "sys_channel_recv: handle={}, len={}, flags={}",
        handle,
        buffer.len(),
        flags
    );

    // 1. Look up handle
    // use crate::ipc::descriptor::IpcDescriptor;
    // let descriptor = IpcDescriptor(handle);

    // 2. Validate permissions (receive permission)
    // use crate::ipc::capability::{check_permission, CapabilityType};
    // if !check_permission(descriptor, CapabilityType::Receive) {
    //     return Err(MemoryError::PermissionDenied);
    // }

    // 3. Receive from fusion ring
    // TODO: Get ring from descriptor table and recv
    // For now, stub implementation

    Ok(0)
}

/// Close IPC handle
pub fn sys_channel_close(handle: IpcHandle) -> MemoryResult<()> {
    log::debug!("sys_channel_close: handle={}", handle);

    // 1. Look up handle
    // use crate::ipc::descriptor::{close_descriptor, IpcDescriptor};
    // let descriptor = IpcDescriptor(handle);

    // 2-4. Remove from table, decrement ref count, free if needed
    // close_descriptor(descriptor)?;

    Ok(())
}

/// Create shared memory region
pub fn sys_shm_create(size: usize, perms: ShmPermissions) -> MemoryResult<ShmId> {
    log::debug!("sys_shm_create: size={}, perms={:?}", size, perms);

    // 1-3. Allocate physical memory, create descriptor, register in pool
    let creator_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
    crate::ipc::shared_memory::pool::allocate(size, perms, creator_pid as usize)
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
    // use crate::ipc::capability::{grant_capability, CapabilityFlags};
    let current_pid = SCHEDULER.with_current_thread(|t| t.id()).unwrap_or(0);
    // TODO: Get target PID from channel
    let target_pid = 0;
    // grant_capability(
    //     current_pid,
    //     target_pid,
    //     CapabilityFlags::READ_WRITE,
    //     Some("fd_transfer"),
    // )?;

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
    // use crate::ipc::capability::{check_permission, CapabilityType};
    // use crate::ipc::descriptor::IpcDescriptor;
    // if !check_permission(IpcDescriptor(handle), CapabilityType::Receive) {
    //     return Err(MemoryError::PermissionDenied);
    // }

    // 3. Install FD in current process FD table
    // TODO: Implement FD installation
    let new_fd = 10; // Stub

    // 4. Return new FD
    Ok(new_fd)
}

/// Create pipe (for compatibility)
pub fn sys_pipe() -> MemoryResult<(i32, i32)> {
    log::debug!("sys_pipe");

    // 1. Create shared FusionRing
    use crate::ipc::fusion_ring::FusionRing;
    use alloc::sync::Arc;
    let ring = Arc::new(FusionRing::new(4096)); // 4KB buffer

    // 2. Create Inodes for read and write ends
    use crate::posix_x::vfs_posix::pipe::PipeInode;
    use spin::RwLock;

    // Generate unique inode numbers (TODO: global counter)
    let ino_read = 1000;
    let ino_write = 1001;

    let read_inode = Arc::new(RwLock::new(PipeInode::new(
        ino_read,
        Arc::clone(&ring),
        false,
    )));
    let write_inode = Arc::new(RwLock::new(PipeInode::new(ino_write, ring, true)));

    // 3. Create VfsHandles
    use crate::posix_x::vfs_posix::{OpenFlags, VfsHandle};

    let read_flags = OpenFlags {
        read: true,
        write: false,
        append: false,
        create: false,
        truncate: false,
        excl: false,
        nonblock: false,
        cloexec: false,
    };
    let write_flags = OpenFlags {
        read: false,
        write: true,
        append: false,
        create: false,
        truncate: false,
        excl: false,
        nonblock: false,
        cloexec: false,
    };

    let read_handle = VfsHandle::new(
        read_inode,
        read_flags,
        alloc::string::String::from("pipe:[read]"),
    );
    let write_handle = VfsHandle::new(
        write_inode,
        write_flags,
        alloc::string::String::from("pipe:[write]"),
    );

    // 4. Allocate FDs
    use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;
    let mut fd_table = GLOBAL_FD_TABLE.write();

    let fd_read = fd_table
        .allocate(read_handle)
        .map_err(|_| MemoryError::Mfile)?;
    let fd_write = fd_table
        .allocate(write_handle)
        .map_err(|_| MemoryError::Mfile)?;

    Ok((fd_read, fd_write))
}

/// Create pipe with flags
pub fn sys_pipe2(flags: i32) -> MemoryResult<(i32, i32)> {
    log::debug!("sys_pipe2: flags={:#x}", flags);

    // 1. Create shared FusionRing
    use crate::ipc::fusion_ring::FusionRing;
    use alloc::sync::Arc;
    let ring = Arc::new(FusionRing::new(4096));

    // 2. Create Inodes
    use crate::posix_x::vfs_posix::pipe::PipeInode;
    use spin::RwLock;

    let ino_read = 1002; // TODO: global counter
    let ino_write = 1003;

    let read_inode = Arc::new(RwLock::new(PipeInode::new(
        ino_read,
        Arc::clone(&ring),
        false,
    )));
    let write_inode = Arc::new(RwLock::new(PipeInode::new(ino_write, ring, true)));

    // 3. Create VfsHandles with flags
    use crate::posix_x::vfs_posix::{OpenFlags, VfsHandle};

    let mut read_flags = OpenFlags::from_posix(flags);
    read_flags.read = true;
    read_flags.write = false;

    let mut write_flags = OpenFlags::from_posix(flags);
    write_flags.read = false;
    write_flags.write = true;

    let read_handle = VfsHandle::new(
        read_inode,
        read_flags,
        alloc::string::String::from("pipe:[read]"),
    );
    let write_handle = VfsHandle::new(
        write_inode,
        write_flags,
        alloc::string::String::from("pipe:[write]"),
    );

    // 4. Allocate FDs
    use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;
    let mut fd_table = GLOBAL_FD_TABLE.write();

    let fd_read = fd_table
        .allocate(read_handle)
        .map_err(|_| MemoryError::Mfile)?;
    let fd_write = fd_table
        .allocate(write_handle)
        .map_err(|_| MemoryError::Mfile)?;

    Ok((fd_read, fd_write))
}

/// Create socketpair (for compatibility)
pub fn sys_socketpair() -> MemoryResult<(u64, u64)> {
    log::debug!("sys_socketpair");

    // Use bidirectional fusion rings
    let (handle1, handle2) = sys_channel_create()?;

    Ok((handle1, handle2))
}
