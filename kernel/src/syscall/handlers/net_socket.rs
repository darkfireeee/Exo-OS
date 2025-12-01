//! Unix Domain Sockets Implementation
//!
//! Handles local IPC using AF_UNIX sockets.

use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::RwLock;

use crate::fs::vfs::inode::{Inode, InodePermissions, InodeType};
use crate::fs::{FsError, FsResult};
use crate::ipc::fusion_ring::FusionRing;
use crate::posix_x::core::fd_table::GLOBAL_FD_TABLE;
use crate::posix_x::vfs_posix::path_resolver;
use crate::posix_x::vfs_posix::{OpenFlags, VfsHandle};

/// Socket domains
pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;

/// Socket types
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;

/// Socket state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketState {
    Unbound,
    Bound,
    Listening,
    Connected,
    Closed,
}

/// Unix Domain Socket
pub struct UnixSocket {
    state: RwLock<SocketState>,
    ring: Arc<FusionRing>,                       // For data transfer
    peer: RwLock<Option<Arc<UnixSocket>>>,       // Connected peer
    backlog: RwLock<Vec<Arc<UnixSocket>>>,       // Pending connections (for listening sockets)
    path: RwLock<Option<alloc::string::String>>, // Bound path
    ino: u64,
}

impl UnixSocket {
    pub fn new(_type_: i32, ino: u64) -> Self {
        Self {
            state: RwLock::new(SocketState::Unbound),
            ring: Arc::new(FusionRing::new(65536)), // Default 64KB buffer
            peer: RwLock::new(None),
            backlog: RwLock::new(Vec::new()),
            path: RwLock::new(None),
            ino,
        }
    }

    pub fn bind(&self, path: &str) -> FsResult<()> {
        let mut state = self.state.write();
        if *state != SocketState::Unbound {
            return Err(FsError::InvalidArgument); // Already bound or connected
        }

        // TODO: Create socket file in VFS
        // For now, just store the path
        *self.path.write() = Some(alloc::string::String::from(path));
        *state = SocketState::Bound;
        Ok(())
    }

    pub fn listen(&self, _backlog: usize) -> FsResult<()> {
        let mut state = self.state.write();
        if *state != SocketState::Bound {
            return Err(FsError::InvalidArgument);
        }
        *state = SocketState::Listening;
        Ok(())
    }

    pub fn connect_to(&self, me: Arc<UnixSocket>, peer: Arc<UnixSocket>) -> FsResult<()> {
        let mut state = self.state.write();
        if *state != SocketState::Unbound {
            return Err(FsError::InvalidArgument);
        }

        let peer_state = peer.state.read();
        if *peer_state != SocketState::Listening {
            return Err(FsError::ConnectionRefused);
        }
        drop(peer_state);

        // Add to peer's backlog
        peer.backlog.write().push(me);

        *self.peer.write() = Some(peer);
        *state = SocketState::Connected;
        Ok(())
    }

    pub fn accept(&self) -> FsResult<Arc<UnixSocket>> {
        let state = self.state.read();
        if *state != SocketState::Listening {
            return Err(FsError::InvalidArgument);
        }
        drop(state);

        let mut backlog = self.backlog.write();
        if let Some(client) = backlog.pop() {
            // Create new socket for this connection
            // Generate unique inode number
            static INODE_COUNTER: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(3000);
            let ino = INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

            let server_socket = Arc::new(UnixSocket::new(SOCK_STREAM, ino)); // Assume stream

            // Connect server_socket <-> client
            *server_socket.peer.write() = Some(client.clone());
            *server_socket.state.write() = SocketState::Connected;

            // Update client's peer to point to server_socket instead of listener
            *client.peer.write() = Some(server_socket.clone());

            // Register server_socket in global table
            GLOBAL_SOCKET_TABLE
                .write()
                .insert(ino, Arc::downgrade(&server_socket));

            Ok(server_socket)
        } else {
            Err(FsError::Again) // EAGAIN
        }
    }
}

/// Socket Inode wrapper
pub struct SocketInode {
    socket: Arc<UnixSocket>,
}

impl SocketInode {
    pub fn new(socket: Arc<UnixSocket>) -> Self {
        Self { socket }
    }
}

impl Inode for SocketInode {
    fn ino(&self) -> u64 {
        self.socket.ino
    }

    fn inode_type(&self) -> InodeType {
        InodeType::Socket
    }

    fn size(&self) -> u64 {
        0
    }

    fn permissions(&self) -> InodePermissions {
        InodePermissions::new()
    }

    fn read_at(&self, _offset: u64, _buf: &mut [u8]) -> FsResult<usize> {
        // Sockets don't support pread/pwrite usually, but we can implement read as recv
        Err(FsError::NotSupported)
    }

    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }

    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }

    fn list(&self) -> FsResult<Vec<alloc::string::String>> {
        Err(FsError::NotDirectory)
    }

    fn lookup(&self, _name: &str) -> FsResult<u64> {
        Err(FsError::NotDirectory)
    }

    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::NotDirectory)
    }

    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotDirectory)
    }
}

// Syscall Handlers

// Global socket table (Inode -> Weak<UnixSocket>)
static GLOBAL_SOCKET_TABLE: RwLock<BTreeMap<u64, Weak<UnixSocket>>> = RwLock::new(BTreeMap::new());

pub fn sys_socket(domain: i32, type_: i32, _protocol: i32) -> i32 {
    if domain != AF_UNIX {
        return -97; // EAFNOSUPPORT
    }

    // Generate unique inode number
    static INODE_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(2000);
    let ino = INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let socket = Arc::new(UnixSocket::new(type_, ino));

    // Register in global table
    GLOBAL_SOCKET_TABLE
        .write()
        .insert(ino, Arc::downgrade(&socket));

    let inode = Arc::new(RwLock::new(SocketInode::new(socket)));

    let flags = OpenFlags {
        read: true,
        write: true,
        append: false,
        create: false,
        truncate: false,
        excl: false,
        nonblock: false, // TODO: Handle SOCK_NONBLOCK
        cloexec: false,  // TODO: Handle SOCK_CLOEXEC
    };

    let handle = VfsHandle::new(inode, flags, alloc::string::String::from("socket:[unix]"));

    let mut fd_table = GLOBAL_FD_TABLE.write();
    match fd_table.allocate(handle) {
        Ok(fd) => fd,
        Err(_) => -24, // EMFILE
    }
}

pub fn sys_bind(fd: i32, addr: *const u8, addrlen: usize) -> i32 {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    // Check if it's a socket
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK (Shouldn't happen if inode type is Socket)
    };
    drop(socket_table);

    // 2. Parse address
    if addrlen < 2 {
        return -22; // EINVAL
    }
    let family = unsafe { *(addr as *const u16) };
    if family as i32 != AF_UNIX {
        return -97; // EAFNOSUPPORT
    }

    // Read path (sun_path starts at offset 2)
    let path_ptr = unsafe { addr.add(2) };
    let path_len = addrlen - 2;

    let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
    // Find null terminator
    let len = path_slice.iter().position(|&c| c == 0).unwrap_or(path_len);
    let path_str = match core::str::from_utf8(&path_slice[..len]) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };

    // 3. Bind
    match socket.bind(path_str) {
        Ok(_) => 0,
        Err(e) => -e.to_errno(),
    }
}

pub fn sys_connect(fd: i32, addr: *const u8, addrlen: usize) -> i32 {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK
    };
    drop(socket_table);

    // 2. Parse address
    if addrlen < 2 {
        return -22; // EINVAL
    }
    let family = unsafe { *(addr as *const u16) };
    if family as i32 != AF_UNIX {
        return -97; // EAFNOSUPPORT
    }

    let path_ptr = unsafe { addr.add(2) };
    let path_len = addrlen - 2;
    let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
    let len = path_slice.iter().position(|&c| c == 0).unwrap_or(path_len);
    let path_str = match core::str::from_utf8(&path_slice[..len]) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };

    // 3. Resolve peer
    // TODO: Get CWD from process
    let cwd = None; // For now, assume absolute paths or no CWD

    let peer_inode: Arc<RwLock<dyn Inode>> = match path_resolver::resolve_path(path_str, cwd, true)
    {
        Ok(i) => i,
        Err(e) => return -e.to_errno(),
    };

    let peer_ino = peer_inode.read().ino();

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let peer_socket = match socket_table.get(&peer_ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -111, // ECONNREFUSED (Not a socket or not found)
    };
    drop(socket_table);

    // 4. Connect
    match socket.connect_to(socket.clone(), peer_socket) {
        Ok(_) => 0,
        Err(e) => -e.to_errno(),
    }
}

pub fn sys_listen(fd: i32, backlog: i32) -> i32 {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK
    };
    drop(socket_table);

    match socket.listen(backlog as usize) {
        Ok(_) => 0,
        Err(e) => -e.to_errno(),
    }
}

pub fn sys_accept(fd: i32, addr: *mut u8, addrlen: *mut usize) -> i32 {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK
    };
    drop(socket_table);

    // 2. Accept
    let new_socket = match socket.accept() {
        Ok(s) => s,
        Err(e) => return -e.to_errno(),
    };

    // 3. Create new FD
    let inode = Arc::new(RwLock::new(SocketInode::new(new_socket.clone())));
    let flags = OpenFlags {
        read: true,
        write: true,
        append: false,
        create: false,
        truncate: false,
        excl: false,
        nonblock: false,
        cloexec: false,
    };
    let handle = VfsHandle::new(inode, flags, alloc::string::String::from("socket:[unix]"));

    let mut fd_table = GLOBAL_FD_TABLE.write();
    let new_fd = match fd_table.allocate(handle) {
        Ok(fd) => fd,
        Err(_) => return -24, // EMFILE
    };

    // 4. Write address if requested
    if !addr.is_null() && !addrlen.is_null() {
        let max_len = unsafe { *addrlen };
        let client_path = new_socket.path.read();

        // Construct sockaddr_un
        // family (2 bytes) + path
        if max_len >= 2 {
            unsafe { *(addr as *mut u16) = AF_UNIX as u16 };
            let mut len = 2;

            if let Some(path) = &*client_path {
                let path_bytes = path.as_bytes();
                let copy_len = core::cmp::min(path_bytes.len(), max_len - 2);
                unsafe {
                    core::ptr::copy_nonoverlapping(path_bytes.as_ptr(), addr.add(2), copy_len);
                    // Null terminate if space
                    if 2 + copy_len < max_len {
                        *addr.add(2 + copy_len) = 0;
                        len += copy_len + 1;
                    } else {
                        len += copy_len;
                    }
                }
            }
            unsafe { *addrlen = len };
        }
    }

    new_fd
}

pub fn sys_sendto(
    fd: i32,
    buf: *const u8,
    len: usize,
    _flags: i32,
    _dest_addr: *const u8,
    _addrlen: usize,
) -> isize {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK
    };
    drop(socket_table);

    // 2. Get peer
    // TODO: Handle dest_addr for DGRAM/unconnected
    let peer = match socket.peer.read().as_ref() {
        Some(p) => p.clone(),
        None => return -107, // ENOTCONN
    };

    // 3. Send data
    let data = unsafe { core::slice::from_raw_parts(buf, len) };
    match peer.ring.send_blocking(data) {
        Ok(_) => len as isize,
        Err(e) => -FsError::from(e).to_errno() as isize,
    }
}

pub fn sys_recvfrom(
    fd: i32,
    buf: *mut u8,
    len: usize,
    _flags: i32,
    src_addr: *mut u8,
    addrlen: *mut usize,
) -> isize {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK
    };
    drop(socket_table);

    // 2. Receive data
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf, len) };
    let n = match socket.ring.recv_blocking(buffer) {
        Ok(n) => n,
        Err(e) => return -FsError::from(e).to_errno() as isize,
    };

    // 3. Fill src_addr if requested
    if !src_addr.is_null() && !addrlen.is_null() {
        let max_len = unsafe { *addrlen };
        // For STREAM, use connected peer
        if let Some(peer) = socket.peer.read().as_ref() {
            let peer_path = peer.path.read();

            if max_len >= 2 {
                unsafe { *(src_addr as *mut u16) = AF_UNIX as u16 };
                let mut addr_len = 2;

                if let Some(path) = &*peer_path {
                    let path_bytes = path.as_bytes();
                    let copy_len = core::cmp::min(path_bytes.len(), max_len - 2);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            path_bytes.as_ptr(),
                            src_addr.add(2),
                            copy_len,
                        );
                        if 2 + copy_len < max_len {
                            *src_addr.add(2 + copy_len) = 0;
                            addr_len += copy_len + 1;
                        } else {
                            addr_len += copy_len;
                        }
                    }
                }
                unsafe { *addrlen = addr_len };
            }
        }
    }

    n as isize
}

pub fn sys_send(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize {
    sys_sendto(fd, buf, len, flags, core::ptr::null(), 0)
}

pub fn sys_recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize {
    sys_recvfrom(
        fd,
        buf,
        len,
        flags,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    )
}

pub fn sys_socketpair(domain: i32, type_: i32, protocol: i32, sv: *mut i32) -> i32 {
    if domain != AF_UNIX {
        return -97; // EAFNOSUPPORT
    }

    // Create two sockets
    static INODE_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(4000);
    let ino1 = INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let ino2 = INODE_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let socket1 = Arc::new(UnixSocket::new(type_, ino1));
    let socket2 = Arc::new(UnixSocket::new(type_, ino2));

    // Connect them
    *socket1.peer.write() = Some(socket2.clone());
    *socket1.state.write() = SocketState::Connected;

    *socket2.peer.write() = Some(socket1.clone());
    *socket2.state.write() = SocketState::Connected;

    // Register in global table
    GLOBAL_SOCKET_TABLE
        .write()
        .insert(ino1, Arc::downgrade(&socket1));
    GLOBAL_SOCKET_TABLE
        .write()
        .insert(ino2, Arc::downgrade(&socket2));

    // Create FDs
    let inode1 = Arc::new(RwLock::new(SocketInode::new(socket1)));
    let inode2 = Arc::new(RwLock::new(SocketInode::new(socket2)));

    let flags = OpenFlags {
        read: true,
        write: true,
        append: false,
        create: false,
        truncate: false,
        excl: false,
        nonblock: false,
        cloexec: false,
    };

    let handle1 = VfsHandle::new(inode1, flags, alloc::string::String::from("socket:[unix]"));
    let handle2 = VfsHandle::new(inode2, flags, alloc::string::String::from("socket:[unix]"));

    let mut fd_table = GLOBAL_FD_TABLE.write();
    let fd1 = match fd_table.allocate(handle1) {
        Ok(fd) => fd,
        Err(_) => return -24, // EMFILE
    };
    let fd2 = match fd_table.allocate(handle2) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = fd_table.close(fd1);
            return -24; // EMFILE
        }
    };

    unsafe {
        *sv = fd1;
        *sv.add(1) = fd2;
    }

    0
}

/// I/O Vector structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Iovec {
    pub base: *mut u8,
    pub len: usize,
}

/// Message header structure for sendmsg/recvmsg
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Msghdr {
    pub msg_name: *mut u8,
    pub msg_namelen: u32,
    pub msg_iov: *mut Iovec,
    pub msg_iovlen: usize,
    pub msg_control: *mut u8,
    pub msg_controllen: usize,
    pub msg_flags: i32,
}

pub fn sys_shutdown(fd: i32, how: i32) -> i32 {
    // 1. Get socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    let ino = inode.ino();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    let socket_table = GLOBAL_SOCKET_TABLE.read();
    let socket = match socket_table.get(&ino).and_then(|w| w.upgrade()) {
        Some(s) => s,
        None => return -88, // ENOTSOCK
    };
    drop(socket_table);

    // 2. Shutdown
    // TODO: Implement actual shutdown logic (SHUT_RD, SHUT_WR, SHUT_RDWR)
    // For now, just return success as we don't fully support half-closed states yet
    log::debug!("sys_shutdown: fd={}, how={}", fd, how);

    // Stub: Mark state as closed if SHUT_RDWR
    if how == 2 {
        *socket.state.write() = SocketState::Closed;
    }

    0
}

pub fn sys_sendmsg(fd: i32, msg: *const Msghdr, flags: i32) -> isize {
    // 1. Validate pointer
    if msg.is_null() {
        return -14; // EFAULT
    }
    let msg = unsafe { &*msg };

    // 2. Gather data from iov
    if msg.msg_iov.is_null() || msg.msg_iovlen == 0 {
        return 0;
    }

    let iovs = unsafe { core::slice::from_raw_parts(msg.msg_iov, msg.msg_iovlen) };
    let mut total_len = 0;
    for iov in iovs {
        total_len += iov.len;
    }

    // Allocate temporary buffer to gather data
    // TODO: Optimize to avoid allocation (direct copy to ring buffer)
    let mut buffer = Vec::with_capacity(total_len);
    for iov in iovs {
        let slice = unsafe { core::slice::from_raw_parts(iov.base, iov.len) };
        buffer.extend_from_slice(slice);
    }

    // 3. Send using sys_sendto logic
    // If msg_name is present, use it as dest_addr
    let (addr, addrlen) = if !msg.msg_name.is_null() {
        (msg.msg_name as *const u8, msg.msg_namelen as usize)
    } else {
        (core::ptr::null(), 0)
    };

    sys_sendto(fd, buffer.as_ptr(), buffer.len(), flags, addr, addrlen)
}

pub fn sys_recvmsg(fd: i32, msg: *mut Msghdr, flags: i32) -> isize {
    // 1. Validate pointer
    if msg.is_null() {
        return -14; // EFAULT
    }
    let msg = unsafe { &mut *msg };

    // 2. Calculate total buffer size
    if msg.msg_iov.is_null() || msg.msg_iovlen == 0 {
        return 0;
    }

    let iovs = unsafe { core::slice::from_raw_parts_mut(msg.msg_iov, msg.msg_iovlen) };
    let mut total_len = 0;
    for iov in iovs.iter() {
        total_len += iov.len;
    }

    // 3. Receive data into temporary buffer
    let mut buffer = Vec::with_capacity(total_len);
    unsafe { buffer.set_len(total_len) };

    // Use sys_recvfrom logic
    let mut addrlen = if !msg.msg_name.is_null() {
        msg.msg_namelen as usize
    } else {
        0
    };

    let n = sys_recvfrom(
        fd,
        buffer.as_mut_ptr(),
        total_len,
        flags,
        msg.msg_name,
        &mut addrlen,
    );

    if n < 0 {
        return n;
    }

    let received = n as usize;
    if !msg.msg_name.is_null() {
        msg.msg_namelen = addrlen as u32;
    }

    // 4. Scatter data into iovs
    let mut copied = 0;
    for iov in iovs.iter() {
        if copied >= received {
            break;
        }
        let to_copy = core::cmp::min(iov.len, received - copied);
        unsafe {
            core::ptr::copy_nonoverlapping(buffer.as_ptr().add(copied), iov.base, to_copy);
        }
        copied += to_copy;
    }

    // Handle control messages (msg_control) - Stub for now
    msg.msg_controllen = 0;
    msg.msg_flags = 0;

    n
}

pub fn sys_getsockopt(
    fd: i32,
    level: i32,
    optname: i32,
    optval: *mut u8,
    optlen: *mut usize,
) -> i32 {
    // 1. Validate socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    if optval.is_null() || optlen.is_null() {
        return -14; // EFAULT
    }

    let max_len = unsafe { *optlen };

    // Stub implementation for common options
    // SOL_SOCKET = 1
    if level == 1 {
        match optname {
            3 => {
                // SO_TYPE
                if max_len < 4 {
                    return -22;
                }
                unsafe { *(optval as *mut i32) = SOCK_STREAM }; // Assume stream for now
                unsafe { *optlen = 4 };
                return 0;
            }
            7 => {
                // SO_SNDBUF
                if max_len < 4 {
                    return -22;
                }
                unsafe { *(optval as *mut i32) = 65536 };
                unsafe { *optlen = 4 };
                return 0;
            }
            8 => {
                // SO_RCVBUF
                if max_len < 4 {
                    return -22;
                }
                unsafe { *(optval as *mut i32) = 65536 };
                unsafe { *optlen = 4 };
                return 0;
            }
            _ => {}
        }
    }

    0 // Success (default)
}

pub fn sys_setsockopt(fd: i32, level: i32, optname: i32, optval: *const u8, optlen: usize) -> i32 {
    // 1. Validate socket
    let fd_table = GLOBAL_FD_TABLE.read();
    let handle = match fd_table.get(fd) {
        Some(h) => h,
        None => return -9, // EBADF
    };
    let inode_lock = handle.read().inode();
    let inode = inode_lock.read();
    if inode.inode_type() != InodeType::Socket {
        return -88; // ENOTSOCK
    }
    drop(inode);
    drop(fd_table);

    if optval.is_null() {
        return -14; // EFAULT
    }

    // Stub implementation
    log::debug!(
        "sys_setsockopt: fd={}, level={}, optname={}, len={}",
        fd,
        level,
        optname,
        optlen
    );

    0 // Success (stub)
}
