#![allow(dead_code)]
// syscall/net_bridge.rs -- Unified ExoNet BSD sockets <-> network_server bridge (V5)
//
// AF_INET sockets are installed as `PSEUDO_NET_SOCKET_TAG` blobs in `OBJECT_TABLE`
// and `pcb.files`, giving native support for `dup`/`dup2`/`dup3`, `fcntl`, `ioctl`,
// `FD_CLOEXEC`, `read`/`write`/`readv`/`writev`, `poll`/`select`/`epoll_wait`, and
// automatic teardown on process exit.
//
// Large payloads (> 128 B, up to 16 KiB per RPC) traverse `network_server`'s
// 4 KiB-aligned `NET_DATA_WINDOW` via Ring 0 page-table copies (`user_as.lock_pt`),
// lifting the 128-byte inline IPC bottleneck without exposing client memory to Ring 1.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::fs::exofs::core::BlobId;
use crate::fs::exofs::syscall::object_fd::{open_flags, OBJECT_TABLE};
use crate::ipc::core::types::{EndpointId, IpcError};
use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::errno::EMSGSIZE;
use crate::syscall::fs_bridge;
use crate::syscall::validation::{
    copy_from_user, copy_to_user, read_user_typed, write_user_typed, USER_ADDR_MAX,
};

pub const NET_OP_OPEN: u32 = 0x4E00;
pub const NET_OP_CONNECT: u32 = 0x4E01;
pub const NET_OP_BIND: u32 = 0x4E02;
pub const NET_OP_LISTEN: u32 = 0x4E03;
pub const NET_OP_ACCEPT: u32 = 0x4E04;
pub const NET_OP_SENDTO: u32 = 0x4E05;
pub const NET_OP_RECVFROM: u32 = 0x4E06;
pub const NET_OP_SENDMSG: u32 = 0x4E07;
pub const NET_OP_RECVMSG: u32 = 0x4E08;
pub const NET_OP_SHUTDOWN: u32 = 0x4E09;
pub const NET_OP_GETSOCKNAME: u32 = 0x4E0A;
pub const NET_OP_SOCKETPAIR: u32 = 0x4E0B;
pub const NET_OP_SETSOCKOPT: u32 = 0x4E0C;
pub const NET_OP_GETSOCKOPT: u32 = 0x4E0D;
pub const NET_OP_CLOSE: u32 = 0x4E0E;
pub const NET_OP_GETPEERNAME: u32 = 0x4E0F;
pub const NET_OP_HANDSHAKE: u32 = 0x4E10;
pub const NET_OP_POLL: u32 = 0x4E11;

const NET_FLAG_USE_DATA_WINDOW: u32 = 0x8000_0000;
const NET_DATA_WINDOW_MAX: usize = 16384;

const AF_UNIX: i32 = 1;
const AF_INET: u16 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCKADDR_IN_LEN: usize = 16;
const IOV_MAX: u64 = 1024;
const NET_HANDLE_TAG: u32 = 0x4000_0000;
const NET_HANDLE_TAG_MASK: u32 = 0xf000_0000;
const NET_INLINE_DATA_MAX: usize = 128;
const NET_REPLY_DATA_OFFSET: usize = core::mem::size_of::<NetReply>();

const SOL_SOCKET: i32 = 1;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;

const MSG_PEEK: u32 = 0x02;
const MSG_DONTWAIT: u32 = 0x40;
const MSG_NOSIGNAL: u32 = 0x4000;

const EINTR: i64 = -4;
const EBADF: i64 = -9;
const EAGAIN: i64 = -11;
const ENOMEM: i64 = -12;
const EFAULT: i64 = -14;
const EINVAL: i64 = -22;
const EPIPE: i64 = -32;
const ENOSYS: i64 = -38;
const ENOTSOCK: i64 = -88;
const ENOTSUP: i64 = -95;
const ENETDOWN: i64 = -100;
const ETIMEDOUT: i64 = -110;
const EINPROGRESS: i64 = -115;

const CONNECT_TIMEOUT_NS: u64 = 10_000_000_000;
const NET_WAIT_SLICE_NS: u64 = 2_000_000; // 2 ms slice while awaiting async Ring 1 network events

static NET_READY: AtomicBool = AtomicBool::new(false);
static NETWORK_ENDPOINT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
struct NetSocketMeta {
    blob_id: BlobId,
    net_handle: u32,
    owner_pid: u32,
    sock_type: i32,
    rcv_timeout_ns: u64,
    snd_timeout_ns: u64,
}

static NET_SOCKET_MAP: Mutex<Vec<NetSocketMeta>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct NetDataWindowInfo {
    pid: u32,
    va: u64,
    len: usize,
}

static NET_DATA_WINDOW_CACHE: Mutex<Option<NetDataWindowInfo>> = Mutex::new(None);
static NET_DATA_LOCKED: AtomicBool = AtomicBool::new(false);
static NET_DATA_GEN: AtomicU64 = AtomicU64::new(0);
static NET_DATA_WAITERS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

struct NetDataLockGuard;

impl Drop for NetDataLockGuard {
    fn drop(&mut self) {
        NET_DATA_LOCKED.store(false, Ordering::Release);
        NET_DATA_GEN.fetch_add(1, Ordering::AcqRel);
        let to_wake: Vec<u64> = {
            let mut waiters = NET_DATA_WAITERS.lock();
            let mut out = Vec::new();
            core::mem::swap(&mut out, &mut *waiters);
            out
        };
        for tcb_raw in to_wake {
            crate::scheduler::timer::sleep::wake_sleeping_tcb(tcb_raw);
        }
    }
}

/// Sleepable mutex serializing access to `network_server`'s 16 KiB `NET_DATA_WINDOW`.
/// Never holds a spinlock across `copy_from_user`/`copy_to_user` or blocking Ring 1 IPC.
fn lock_net_data_window() -> Result<NetDataLockGuard, i64> {
    loop {
        if NET_DATA_LOCKED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return Ok(NetDataLockGuard);
        }
        let observed = NET_DATA_GEN.load(Ordering::Acquire);
        let tcb = crate::scheduler::core::switch::current_thread_raw() as u64;
        if tcb == 0 {
            if !crate::scheduler::timer::sleep::sleep_ns(1_000_000) {
                return Err(EINTR);
            }
            continue;
        }
        {
            let mut waiters = NET_DATA_WAITERS.lock();
            if !NET_DATA_LOCKED.load(Ordering::Acquire)
                || NET_DATA_GEN.load(Ordering::Acquire) != observed
            {
                continue;
            }
            if !waiters.contains(&tcb) && waiters.try_reserve(1).is_ok() {
                waiters.push(tcb);
            }
        }
        let ok = crate::scheduler::timer::sleep::sleep_ns_if(10_000_000, || {
            NET_DATA_LOCKED.load(Ordering::Acquire)
                && NET_DATA_GEN.load(Ordering::Acquire) == observed
        });
        {
            let mut waiters = NET_DATA_WAITERS.lock();
            if let Some(pos) = waiters.iter().position(|&w| w == tcb) {
                waiters.swap_remove(pos);
            }
        }
        if !ok {
            return Err(EINTR);
        }
    }
}

#[derive(Clone, Copy)]
struct ResolvedNetSocket {
    blob_id: Option<BlobId>,
    net_handle: u32,
    owner_pid: u32,
    nonblock: bool,
    sock_type: i32,
    rcv_timeout_ns: u64,
    snd_timeout_ns: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetMsg {
    pub opcode: u32,
    pub sender_pid: u32,
    pub fd: u32,
    pub _pad0: u32,
    pub arg1: u64,
    pub arg2: u64,
    pub arg3: u32,
    pub arg4: u32,
    pub _reserved: [u8; 8],
}

const _: () = assert!(core::mem::size_of::<NetMsg>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetReply {
    pub status: i64,
    pub payload: [u8; 40],
}

const _: () = assert!(core::mem::size_of::<NetReply>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxIovec {
    iov_base: u64,
    iov_len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxMsghdr {
    msg_name: u64,
    msg_namelen: u32,
    _pad0: u32,
    msg_iov: u64,
    msg_iovlen: u64,
    msg_control: u64,
    msg_controllen: u64,
    msg_flags: i32,
    _pad1: u32,
}

const _: () = assert!(core::mem::size_of::<LinuxMsghdr>() == 56);

/// Activates the network bridge after kernel base initialization.
///
/// # Safety
/// Call once from the BSP boot path, after IPC initialization.
pub unsafe fn net_bridge_preinit() {
    NET_READY.store(true, Ordering::Release);
}

#[inline]
pub fn bridge_result(result: Result<i64, i64>) -> i64 {
    match result {
        Ok(value) => value,
        Err(errno) => errno,
    }
}

fn current_pid() -> u32 {
    crate::syscall::fast_path::syscall_current_pid()
}

fn network_endpoint() -> Result<EndpointId, i64> {
    if !NET_READY.load(Ordering::Acquire) {
        return Err(ENOSYS);
    }

    let cached = NETWORK_ENDPOINT.load(Ordering::Acquire);
    if cached != 0 {
        return EndpointId::new(cached).ok_or(ENETDOWN);
    }

    let endpoint = crate::ipc::endpoint::lookup_endpoint(b"network_server").ok_or(ENETDOWN)?;
    NETWORK_ENDPOINT.store(endpoint.get(), Ordering::Release);
    Ok(endpoint)
}

fn ipc_errno(err: IpcError) -> i64 {
    match err {
        IpcError::MessageTooLarge => EMSGSIZE,
        IpcError::WouldBlock | IpcError::QueueEmpty | IpcError::QueueFull | IpcError::Full => -11,
        IpcError::OutOfResources | IpcError::ResourceExhausted | IpcError::ShmPoolFull => -12,
        IpcError::NotFound
        | IpcError::EndpointNotFound
        | IpcError::ChannelClosed
        | IpcError::Closed
        | IpcError::ConnRefused => {
            NETWORK_ENDPOINT.store(0, Ordering::Release);
            *NET_DATA_WINDOW_CACHE.lock() = None;
            ENETDOWN
        }
        IpcError::Timeout => ETIMEDOUT,
        IpcError::NullEndpoint
        | IpcError::InvalidEndpoint
        | IpcError::InvalidParam
        | IpcError::Invalid
        | IpcError::InvalidArgument
        | IpcError::InvalidHandle
        | IpcError::ProtocolError
        | IpcError::HandshakeFailed
        | IpcError::OutOfOrder => EINVAL,
        IpcError::PermissionDenied => -13,
        _ => ENETDOWN,
    }
}

fn make_msg_for(
    sender_pid: u32,
    opcode: u32,
    fd: u32,
    arg1: u64,
    arg2: u64,
    arg3: u32,
    arg4: u32,
) -> NetMsg {
    NetMsg {
        opcode,
        sender_pid,
        fd,
        _pad0: 0,
        arg1,
        arg2,
        arg3,
        arg4,
        _reserved: [0; 8],
    }
}

fn call_network(msg: &NetMsg) -> Result<NetReply, i64> {
    let mut reply_buf = [0u8; core::mem::size_of::<NetReply>()];
    let n = call_network_raw(msg, &[], &mut reply_buf)?;
    if n < core::mem::size_of::<NetReply>() {
        return Err(EINVAL);
    }
    Ok(unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const NetReply) })
}

fn call_network_raw(msg: &NetMsg, data: &[u8], reply_buf: &mut [u8]) -> Result<usize, i64> {
    if data.len() > NET_INLINE_DATA_MAX {
        return Err(EMSGSIZE);
    }
    let endpoint = network_endpoint()?;
    let mut request = [0u8; core::mem::size_of::<NetMsg>() + NET_INLINE_DATA_MAX];
    request[..core::mem::size_of::<NetMsg>()].copy_from_slice(unsafe {
        core::slice::from_raw_parts(
            msg as *const NetMsg as *const u8,
            core::mem::size_of::<NetMsg>(),
        )
    });
    request[core::mem::size_of::<NetMsg>()..core::mem::size_of::<NetMsg>() + data.len()]
        .copy_from_slice(data);
    match crate::ipc::rpc::call_raw(
        endpoint,
        &request[..core::mem::size_of::<NetMsg>() + data.len()],
        reply_buf,
    ) {
        Ok(n) if n >= core::mem::size_of::<NetReply>() => Ok(n),
        Ok(_) => Err(EINVAL),
        Err(err) => Err(ipc_errno(err)),
    }
}

fn dispatch_for(
    sender_pid: u32,
    opcode: u32,
    fd: u32,
    arg1: u64,
    arg2: u64,
    arg3: u32,
    arg4: u32,
) -> Result<NetReply, i64> {
    let msg = make_msg_for(sender_pid, opcode, fd, arg1, arg2, arg3, arg4);
    let reply = call_network(&msg)?;
    if reply.status < 0 {
        Err(reply.status)
    } else {
        Ok(reply)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ring 0 <-> network_server 16 KiB NET_DATA_WINDOW helpers
// ─────────────────────────────────────────────────────────────────────────────

struct PinnedUserAs {
    ptr: *const crate::memory::virt::UserAddressSpace,
}

impl core::ops::Deref for PinnedUserAs {
    type Target = crate::memory::virt::UserAddressSpace;
    fn deref(&self) -> &Self::Target {
        // SAFETY: pinned via `pin_remote()` until `Drop`.
        unsafe { &*self.ptr }
    }
}

impl Drop for PinnedUserAs {
    fn drop(&mut self) {
        // SAFETY: paired with `pin_remote()`.
        unsafe { &*self.ptr }.unpin_remote();
    }
}

fn pin_process_as(pid: u32) -> Option<PinnedUserAs> {
    if pid == 0 {
        return None;
    }
    PROCESS_REGISTRY
        .with_pinned_pid(Pid(pid), |pcb| {
            let _as_guard = pcb.as_lock.lock();
            let ptr = pcb.address_space_ptr() as *const crate::memory::virt::UserAddressSpace;
            if ptr.is_null() {
                return None;
            }
            // SAFETY: PCB pinned and `as_lock` held while taking remote pin.
            unsafe { &*ptr }.pin_remote();
            Some(PinnedUserAs { ptr })
        })
        .flatten()
}

fn resolve_remote_phys(
    user_as: &crate::memory::virt::UserAddressSpace,
    addr: u64,
    write: bool,
) -> Option<(u64, usize)> {
    use crate::memory::virt::page_table::{PageTableWalker, WalkResult};
    let walker = PageTableWalker::new(user_as.pml4_phys());
    let (entry, level) = match walker.walk_read(VirtAddr::new(addr)) {
        WalkResult::Leaf { entry, level } | WalkResult::HugePage { entry, level } => (entry, level),
        _ => return None,
    };
    if !entry.is_user() || (write && (!entry.is_writable() || entry.is_cow())) {
        return None;
    }
    let page_size = level.page_size();
    let offset = (addr as usize) & (page_size - 1);
    Some((entry.phys_addr().as_u64() + offset as u64, page_size - offset))
}

fn ensure_net_data_window() -> Option<NetDataWindowInfo> {
    if let Some(info) = *NET_DATA_WINDOW_CACHE.lock() {
        return Some(info);
    }
    let msg = make_msg_for(current_pid(), NET_OP_HANDSHAKE, 0, 0, 0, 0, 0);
    let reply = call_network(&msg).ok()?;
    if reply.status < 0 {
        return None;
    }
    let pid = reply_u32(&reply, 0);
    let va = reply_u64(&reply, 8);
    let len = reply_u32(&reply, 16) as usize;
    if pid == 0 || va < 0x1000 || len < 4096 || len > NET_DATA_WINDOW_MAX {
        return None;
    }
    let end = va.checked_add(len as u64)?;
    if end > USER_ADDR_MAX {
        return None;
    }
    let info = NetDataWindowInfo { pid, va, len };
    *NET_DATA_WINDOW_CACHE.lock() = Some(info);
    Some(info)
}

fn copy_caller_user_to_net_window(
    info: &NetDataWindowInfo,
    src_user_ptr: u64,
    len: usize,
) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if len > info.len {
        return Err(EMSGSIZE);
    }
    let Some(net_as) = pin_process_as(info.pid) else {
        *NET_DATA_WINDOW_CACHE.lock() = None;
        return Err(ENETDOWN);
    };
    let mut copied = 0usize;
    let mut scratch = [0u8; 512];
    while copied < len {
        let dst_va = info.va + copied as u64;
        let page_rem = 4096 - ((dst_va as usize) & 4095);
        let chunk = (len - copied).min(scratch.len()).min(page_rem);
        let src_va = src_user_ptr.checked_add(copied as u64).ok_or(EFAULT)?;
        if copy_from_user(scratch.as_mut_ptr(), src_va as *const u8, chunk).is_err() {
            return Err(EFAULT);
        }
        {
            let _pt_guard = net_as.lock_pt();
            let Some((phys, avail)) = resolve_remote_phys(&net_as, dst_va, true) else {
                *NET_DATA_WINDOW_CACHE.lock() = None;
                return Err(ENETDOWN);
            };
            let n = chunk.min(avail);
            let dst_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *mut u8;
            // SAFETY: `dst_ptr` maps a present writable user page of `network_server`
            // locked against concurrent unmap via `net_as.lock_pt()`.
            unsafe {
                core::ptr::copy_nonoverlapping(scratch.as_ptr(), dst_ptr, n);
            }
            copied += n;
        }
    }
    Ok(())
}

fn copy_net_window_to_caller_user(
    info: &NetDataWindowInfo,
    dst_user_ptr: u64,
    len: usize,
) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    if len > info.len {
        return Err(EMSGSIZE);
    }
    let Some(net_as) = pin_process_as(info.pid) else {
        *NET_DATA_WINDOW_CACHE.lock() = None;
        return Err(ENETDOWN);
    };
    let mut copied = 0usize;
    let mut scratch = [0u8; 512];
    while copied < len {
        let src_va = info.va + copied as u64;
        let n = {
            let _pt_guard = net_as.lock_pt();
            let Some((phys, avail)) = resolve_remote_phys(&net_as, src_va, false) else {
                *NET_DATA_WINDOW_CACHE.lock() = None;
                return Err(ENETDOWN);
            };
            let n = (len - copied).min(scratch.len()).min(avail);
            let src_ptr = phys_to_virt(PhysAddr::new(phys)).as_u64() as *const u8;
            // SAFETY: `src_ptr` maps a present user page of `network_server` under `net_as.lock_pt()`.
            unsafe {
                core::ptr::copy_nonoverlapping(src_ptr, scratch.as_mut_ptr(), n);
            }
            n
        };
        let dst_va = dst_user_ptr.checked_add(copied as u64).ok_or(EFAULT)?;
        if copy_to_user(dst_va as *mut u8, scratch.as_ptr(), n).is_err() {
            return Err(EFAULT);
        }
        copied += n;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Unified OBJECT_TABLE / pcb.files integration (`PSEUDO_NET_SOCKET_TAG`)
// ─────────────────────────────────────────────────────────────────────────────

fn install_net_socket_fd(
    net_handle: u32,
    owner_pid: u32,
    sock_type: i32,
    open_flags_raw: u32,
) -> Result<i64, i64> {
    if !fs_bridge::is_fs_ready() {
        return Ok(net_handle as i64);
    }
    let blob_id = fs_bridge::next_pseudo_blob(fs_bridge::PSEUDO_NET_SOCKET_TAG);
    if fs_bridge::ensure_blob_exists(blob_id).is_err() {
        let _ = dispatch_for(owner_pid, NET_OP_CLOSE, net_handle, 0, 0, 0, 0);
        return Err(ENOMEM);
    }
    let obj_flags = open_flags::O_RDWR | (open_flags_raw & fs_bridge::O_NONBLOCK);
    let obj_fd = match OBJECT_TABLE.open(blob_id, obj_flags, 0, 0, owner_pid as u64) {
        Ok(fd) => fd,
        Err(_) => {
            let _ = dispatch_for(owner_pid, NET_OP_CLOSE, net_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
    };
    {
        let mut map = NET_SOCKET_MAP.lock();
        if map.try_reserve(1).is_err() {
            drop(map);
            let _ = OBJECT_TABLE.close(obj_fd);
            let _ = dispatch_for(owner_pid, NET_OP_CLOSE, net_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
        map.push(NetSocketMeta {
            blob_id,
            net_handle,
            owner_pid,
            sock_type,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
        });
    }
    if fs_bridge::process_has_fd_table(owner_pid) {
        let fd_flags = fs_bridge::fd_table_flags(open_flags_raw, open_flags::O_RDWR);
        if let Some(logical_fd) = fs_bridge::install_process_fd(owner_pid, obj_fd as u64, fd_flags)
        {
            return Ok(logical_fd as i64);
        }
        let _ = fs_bridge::close_object_handle_with_cleanup(obj_fd);
        return Err(ENOMEM);
    }
    Ok(obj_fd as i64)
}

fn lookup_net_meta_by_blob(blob_id: BlobId) -> Option<NetSocketMeta> {
    NET_SOCKET_MAP
        .lock()
        .iter()
        .find(|m| m.blob_id == blob_id)
        .copied()
}

fn resolve_net_fd(fd: i32, pid: u32) -> Result<ResolvedNetSocket, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if fs_bridge::is_fs_ready() {
        if let Ok(res) = fs_bridge::resolve_fd(pid, fd as u32) {
            if !fs_bridge::is_tty_handle(res.handle) {
                if let Ok(entry) = OBJECT_TABLE.get(res.handle) {
                    if fs_bridge::is_pseudo_blob(&entry.blob_id, fs_bridge::PSEUDO_NET_SOCKET_TAG) {
                        let meta = lookup_net_meta_by_blob(entry.blob_id).ok_or(EBADF)?;
                        let nonblock =
                            ((res.flags | entry.flags) & fs_bridge::O_NONBLOCK) != 0;
                        return Ok(ResolvedNetSocket {
                            blob_id: Some(entry.blob_id),
                            net_handle: meta.net_handle,
                            owner_pid: meta.owner_pid,
                            nonblock,
                            sock_type: meta.sock_type,
                            rcv_timeout_ns: meta.rcv_timeout_ns,
                            snd_timeout_ns: meta.snd_timeout_ns,
                        });
                    }
                    return Err(ENOTSOCK);
                }
            }
            return Err(ENOTSOCK);
        }
    }
    if let Some(raw_handle) = socket_handle_from_raw(fd as u64) {
        return Ok(ResolvedNetSocket {
            blob_id: None,
            net_handle: raw_handle as u32,
            owner_pid: pid,
            nonblock: false,
            sock_type: SOCK_STREAM,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
        });
    }
    Err(EBADF)
}

/// Called by `fs_bridge::cleanup_closed_blob` when the final `OBJECT_TABLE` reference
/// to a `PSEUDO_NET_SOCKET_TAG` blob is closed (explicit close, dup2, CLOEXEC, or exit).
pub fn on_net_socket_blob_closed(blob_id: BlobId) {
    let removed = {
        let mut map = NET_SOCKET_MAP.lock();
        map.iter()
            .position(|m| m.blob_id == blob_id)
            .map(|pos| map.swap_remove(pos))
    };
    if let Some(meta) = removed {
        let _ = dispatch_for(meta.owner_pid, NET_OP_CLOSE, meta.net_handle, 0, 0, 0, 0);
    }
    fs_bridge::notify_io_event(Some(blob_id));
}

/// Returns real Linux `poll`/`epoll` readiness bitmask from `network_server`.
pub fn net_poll_mask_for_blob(blob_id: BlobId) -> u32 {
    let Some(meta) = lookup_net_meta_by_blob(blob_id) else {
        return fs_bridge::EPOLLHUP;
    };
    match dispatch_for(meta.owner_pid, NET_OP_POLL, meta.net_handle, 0, 0, 0, 0) {
        Ok(reply) => reply_u32(&reply, 0),
        Err(_) => fs_bridge::EPOLLERR | fs_bridge::EPOLLHUP,
    }
}

/// Returns available RX byte count for `ioctl(FIONREAD)`.
pub fn net_available_bytes_for_blob(blob_id: BlobId) -> usize {
    let Some(meta) = lookup_net_meta_by_blob(blob_id) else {
        return 0;
    };
    match dispatch_for(meta.owner_pid, NET_OP_POLL, meta.net_handle, 0, 0, 0, 0) {
        Ok(reply) => reply_u32(&reply, 4) as usize,
        Err(_) => 0,
    }
}

pub fn net_sendto_blob(
    blob_id: BlobId,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
) -> Result<i64, i64> {
    let meta = lookup_net_meta_by_blob(blob_id).ok_or(EBADF)?;
    let sock = ResolvedNetSocket {
        blob_id: Some(blob_id),
        net_handle: meta.net_handle,
        owner_pid: meta.owner_pid,
        nonblock: (flags & MSG_DONTWAIT) != 0,
        sock_type: meta.sock_type,
        rcv_timeout_ns: meta.rcv_timeout_ns,
        snd_timeout_ns: meta.snd_timeout_ns,
    };
    net_sendto_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len)
}

pub fn net_recvfrom_blob(
    blob_id: BlobId,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, i64> {
    let meta = lookup_net_meta_by_blob(blob_id).ok_or(EBADF)?;
    let sock = ResolvedNetSocket {
        blob_id: Some(blob_id),
        net_handle: meta.net_handle,
        owner_pid: meta.owner_pid,
        nonblock: (flags & MSG_DONTWAIT) != 0,
        sock_type: meta.sock_type,
        rcv_timeout_ns: meta.rcv_timeout_ns,
        snd_timeout_ns: meta.snd_timeout_ns,
    };
    net_recvfrom_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len_ptr)
}

// ─────────────────────────────────────────────────────────────────────────────
// Address & helper serialization
// ─────────────────────────────────────────────────────────────────────────────

fn read_sockaddr_in(addr_ptr: u64, addr_len: u64) -> Result<(u32, u16), i64> {
    if addr_ptr == 0 || addr_len < SOCKADDR_IN_LEN as u64 {
        return Err(EINVAL);
    }
    let mut raw = [0u8; SOCKADDR_IN_LEN];
    if copy_from_user(raw.as_mut_ptr(), addr_ptr as *const u8, raw.len()).is_err() {
        return Err(EFAULT);
    }
    let family = u16::from_ne_bytes([raw[0], raw[1]]);
    if family != AF_INET {
        return Err(ENOTSUP);
    }
    let port = u16::from_be_bytes([raw[2], raw[3]]);
    let addr = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
    Ok((addr, port))
}

fn write_sockaddr_in(addr_ptr: u64, addr_len_ptr: u64, addr: u32, port: u16) -> Result<(), i64> {
    if addr_ptr != 0 {
        let mut raw = [0u8; SOCKADDR_IN_LEN];
        raw[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
        raw[2..4].copy_from_slice(&port.to_be_bytes());
        raw[4..8].copy_from_slice(&addr.to_be_bytes());
        if copy_to_user(addr_ptr as *mut u8, raw.as_ptr(), raw.len()).is_err() {
            return Err(EFAULT);
        }
    }
    if addr_len_ptr != 0 {
        let len = SOCKADDR_IN_LEN as u32;
        if copy_to_user(
            addr_len_ptr as *mut u8,
            &len as *const u32 as *const u8,
            core::mem::size_of::<u32>(),
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    Ok(())
}

fn reply_u64(reply: &NetReply, offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&reply.payload[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

fn reply_u32(reply: &NetReply, offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&reply.payload[offset..offset + 4]);
    u32::from_le_bytes(bytes)
}

fn reply_u16(reply: &NetReply, offset: usize) -> u16 {
    let mut bytes = [0u8; 2];
    bytes.copy_from_slice(&reply.payload[offset..offset + 2]);
    u16::from_le_bytes(bytes)
}

fn msghdr_total_iov_len(msg: &LinuxMsghdr) -> Result<usize, i64> {
    if msg.msg_iovlen > IOV_MAX {
        return Err(EINVAL);
    }
    if msg.msg_iovlen != 0 && msg.msg_iov == 0 {
        return Err(EFAULT);
    }

    let mut total = 0usize;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(EFAULT)?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| EFAULT)?;
        if iov.iov_len > isize::MAX as u64 {
            return Err(EINVAL);
        }
        if iov.iov_len != 0 && iov.iov_base == 0 {
            return Err(EFAULT);
        }
        total = total.checked_add(iov.iov_len as usize).ok_or(EINVAL)?;
        idx += 1;
    }
    Ok(total)
}

#[inline]
fn check_net_permission(syscall_no: u64) -> Result<(), i64> {
    let caller_pid = current_pid();
    if caller_pid != 0 {
        let ctx = crate::security::zero_trust::context_for_caller(caller_pid, 0);
        if crate::security::zero_trust::verify_syscall(&ctx, syscall_no).is_err() {
            return Err(-1); // EPERM
        }
    }
    Ok(())
}

fn wait_net_slice(deadline_ns: Option<u64>) -> Result<bool, i64> {
    let gen = fs_bridge::io_wait_gen();
    fs_bridge::wait_for_io_event(gen, deadline_ns, NET_WAIT_SLICE_NS).map_err(|e| e.to_errno())
}

// ─────────────────────────────────────────────────────────────────────────────
// Public BSD Socket Syscall Bridge
// ─────────────────────────────────────────────────────────────────────────────

pub fn net_socket(domain: i32, ty: i32, protocol: i32) -> Result<i64, i64> {
    if domain == AF_UNIX {
        return fs_bridge::fs_unix_socket(domain, ty, protocol, current_pid())
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_SOCKET)?;
    let pid = current_pid();
    let clean_ty = ty & !((fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC) as i32);
    let open_flags_raw = (ty as u32) & (fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC);
    let reply = dispatch_for(
        pid,
        NET_OP_OPEN,
        0,
        domain as u32 as u64,
        clean_ty as u32 as u64,
        protocol as u32,
        0,
    )?;
    let net_handle = reply_u64(&reply, 0) as u32;
    install_net_socket_fd(net_handle, pid, clean_ty, open_flags_raw)
}

pub fn socket_handle_from_raw(raw: u64) -> Option<i32> {
    if raw > i32::MAX as u64 {
        return None;
    }
    let fd = raw as u32;
    (fd & NET_HANDLE_TAG_MASK == NET_HANDLE_TAG).then_some(fd as i32)
}

pub fn net_close(fd: i32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    dispatch_for(current_pid(), NET_OP_CLOSE, fd as u32, 0, 0, 0, 0)?;
    Ok(0)
}

pub fn net_bind(fd: i32, addr_ptr: u64, addr_len: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_bind(fd as u32, addr_ptr, addr_len, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_BIND)?;
    let sock = resolve_net_fd(fd, pid)?;
    let (addr, port) = read_sockaddr_in(addr_ptr, addr_len)?;
    dispatch_for(
        sock.owner_pid,
        NET_OP_BIND,
        sock.net_handle,
        addr as u64,
        port as u64,
        addr_len as u32,
        0,
    )?;
    Ok(0)
}

pub fn net_connect(fd: i32, addr_ptr: u64, addr_len: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_connect(fd as u32, addr_ptr, addr_len, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_CONNECT)?;
    let sock = resolve_net_fd(fd, pid)?;
    let (addr, port) = read_sockaddr_in(addr_ptr, addr_len)?;
    let nb_flag = u32::from(sock.nonblock);
    let deadline =
        crate::scheduler::timer::clock::monotonic_ns().saturating_add(CONNECT_TIMEOUT_NS);
    loop {
        match dispatch_for(
            sock.owner_pid,
            NET_OP_CONNECT,
            sock.net_handle,
            addr as u64,
            port as u64,
            addr_len as u32,
            nb_flag,
        ) {
            Ok(_) => {
                fs_bridge::notify_io_event(sock.blob_id);
                return Ok(0);
            }
            Err(EINPROGRESS) if sock.nonblock => return Err(EINPROGRESS),
            Err(EAGAIN) if !sock.nonblock => {
                if !wait_net_slice(Some(deadline))? {
                    return Err(ETIMEDOUT);
                }
            }
            Err(err) => {
                fs_bridge::notify_io_event(sock.blob_id);
                return Err(err);
            }
        }
    }
}

pub fn net_listen(fd: i32, backlog: i32) -> Result<i64, i64> {
    if fd < 0 || backlog < 0 {
        return Err(EINVAL);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_listen(fd as u32, backlog, pid).map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_LISTEN)?;
    let sock = resolve_net_fd(fd, pid)?;
    dispatch_for(
        sock.owner_pid,
        NET_OP_LISTEN,
        sock.net_handle,
        backlog as u64,
        0,
        0,
        0,
    )?;
    Ok(0)
}

pub fn net_accept(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_accept(fd as u32, addr_ptr, addr_len_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_ACCEPT)?;
    let sock = resolve_net_fd(fd, pid)?;
    let reply = loop {
        match dispatch_for(sock.owner_pid, NET_OP_ACCEPT, sock.net_handle, 0, 0, 0, 0) {
            Ok(r) => break r,
            Err(EAGAIN) if !sock.nonblock => {
                let _ = wait_net_slice(None)?;
            }
            Err(err) => return Err(err),
        }
    };
    let child_handle = reply_u64(&reply, 0) as u32;
    let peer_addr = reply_u32(&reply, 8);
    let peer_port = reply_u16(&reply, 12);
    if let Err(err) = write_sockaddr_in(addr_ptr, addr_len_ptr, peer_addr, peer_port) {
        let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
        return Err(err);
    }
    // Note: child socket was opened under `sock.owner_pid` on network_server; install into caller `pid`
    // while recording `sock.owner_pid` as the server-side handle owner.
    if !fs_bridge::is_fs_ready() {
        return Ok(child_handle as i64);
    }
    let blob_id = fs_bridge::next_pseudo_blob(fs_bridge::PSEUDO_NET_SOCKET_TAG);
    if fs_bridge::ensure_blob_exists(blob_id).is_err() {
        let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
        return Err(ENOMEM);
    }
    let obj_fd = match OBJECT_TABLE.open(blob_id, open_flags::O_RDWR, 0, 0, pid as u64) {
        Ok(f) => f,
        Err(_) => {
            let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
    };
    {
        let mut map = NET_SOCKET_MAP.lock();
        if map.try_reserve(1).is_err() {
            drop(map);
            let _ = OBJECT_TABLE.close(obj_fd);
            let _ = dispatch_for(sock.owner_pid, NET_OP_CLOSE, child_handle, 0, 0, 0, 0);
            return Err(ENOMEM);
        }
        map.push(NetSocketMeta {
            blob_id,
            net_handle: child_handle,
            owner_pid: sock.owner_pid,
            sock_type: SOCK_STREAM,
            rcv_timeout_ns: 0,
            snd_timeout_ns: 0,
        });
    }
    if fs_bridge::process_has_fd_table(pid) {
        let fd_flags = fs_bridge::fd_table_flags(0, open_flags::O_RDWR);
        if let Some(logical_fd) = fs_bridge::install_process_fd(pid, obj_fd as u64, fd_flags) {
            return Ok(logical_fd as i64);
        }
        let _ = fs_bridge::close_object_handle_with_cleanup(obj_fd);
        return Err(ENOMEM);
    }
    Ok(obj_fd as i64)
}

pub fn net_sendto(
    fd: i32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_sendto(fd as u32, buf_ptr, len, flags, addr_ptr, addr_len, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_SENDTO)?;
    let sock = resolve_net_fd(fd, pid)?;
    net_sendto_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len)
}

fn net_sendto_resolved(
    sock: ResolvedNetSocket,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len: u64,
) -> Result<i64, i64> {
    if len == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(EFAULT);
    }
    let (addr, port) = if addr_ptr != 0 {
        read_sockaddr_in(addr_ptr, addr_len)?
    } else {
        (0, 0)
    };
    let nonblock = sock.nonblock || (flags & MSG_DONTWAIT) != 0;
    let is_dgram = sock.sock_type != SOCK_STREAM;
    let deadline = if sock.snd_timeout_ns != 0 {
        Some(
            crate::scheduler::timer::clock::monotonic_ns()
                .saturating_add(sock.snd_timeout_ns),
        )
    } else {
        None
    };

    let window_opt = ensure_net_data_window();
    let max_chunk = window_opt
        .as_ref()
        .map(|w| w.len)
        .unwrap_or(NET_INLINE_DATA_MAX);
    if is_dgram && len > max_chunk {
        return Err(EMSGSIZE);
    }

    let mut total_sent: usize = 0;
    while total_sent < len {
        let chunk_len = (len - total_sent).min(max_chunk);
        let curr_buf = match buf_ptr.checked_add(total_sent as u64) {
            Some(p) => p,
            None => {
                return if total_sent > 0 {
                    Ok(total_sent as i64)
                } else {
                    Err(EFAULT)
                };
            }
        };

        let reply_status = if let Some(ref win) = window_opt {
            if chunk_len > NET_INLINE_DATA_MAX || is_dgram {
                let _lock = match lock_net_data_window() {
                    Ok(g) => g,
                    Err(e) => {
                        return if total_sent > 0 {
                            Ok(total_sent as i64)
                        } else {
                            Err(e)
                        };
                    }
                };
                if let Err(e) = copy_caller_user_to_net_window(win, curr_buf, chunk_len) {
                    return if total_sent > 0 {
                        Ok(total_sent as i64)
                    } else {
                        Err(e)
                    };
                }
                let msg = make_msg_for(
                    sock.owner_pid,
                    NET_OP_SENDTO,
                    sock.net_handle,
                    chunk_len as u64,
                    addr as u64,
                    port as u32,
                    (flags & !NET_FLAG_USE_DATA_WINDOW) | NET_FLAG_USE_DATA_WINDOW,
                );
                let mut reply_raw = [0u8; core::mem::size_of::<NetReply>()];
                match call_network_raw(&msg, &[], &mut reply_raw) {
                    Ok(_) => {
                        let r = unsafe {
                            core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply)
                        };
                        r.status
                    }
                    Err(e) => {
                        return if total_sent > 0 {
                            Ok(total_sent as i64)
                        } else {
                            Err(e)
                        };
                    }
                }
            } else {
                send_inline_chunk(&sock, curr_buf, chunk_len, addr, port, flags)?
            }
        } else {
            send_inline_chunk(&sock, curr_buf, chunk_len, addr, port, flags)?
        };

        if reply_status == EAGAIN {
            if total_sent > 0 {
                return Ok(total_sent as i64);
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if !wait_net_slice(deadline)? {
                return Err(EAGAIN);
            }
            continue;
        }
        if reply_status == EPIPE && (flags & MSG_NOSIGNAL) == 0 {
            let _ = crate::process::signal::delivery::send_signal_to_pid(
                Pid(current_pid()),
                crate::process::signal::default::Signal::SIGPIPE,
            );
        }
        if reply_status < 0 {
            return if total_sent > 0 {
                Ok(total_sent as i64)
            } else {
                Err(reply_status)
            };
        }
        let sent_this_chunk = (reply_status as usize).min(chunk_len);
        total_sent += sent_this_chunk;
        fs_bridge::notify_io_event(sock.blob_id);
        if is_dgram || sent_this_chunk < chunk_len {
            break;
        }
    }
    Ok(total_sent as i64)
}

fn send_inline_chunk(
    sock: &ResolvedNetSocket,
    curr_buf: u64,
    chunk_len: usize,
    addr: u32,
    port: u16,
    flags: u32,
) -> Result<i64, i64> {
    let mut data = [0u8; NET_INLINE_DATA_MAX];
    if copy_from_user(data.as_mut_ptr(), curr_buf as *const u8, chunk_len).is_err() {
        return Err(EFAULT);
    }
    let msg = make_msg_for(
        sock.owner_pid,
        NET_OP_SENDTO,
        sock.net_handle,
        chunk_len as u64,
        addr as u64,
        port as u32,
        flags & !NET_FLAG_USE_DATA_WINDOW,
    );
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let _ = call_network_raw(&msg, &data[..chunk_len], &mut reply_raw)?;
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    Ok(reply.status)
}

pub fn net_recvfrom(
    fd: i32,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_recvfrom(
            fd as u32,
            buf_ptr,
            len,
            flags,
            addr_ptr,
            addr_len_ptr,
            pid,
        )
        .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_RECVFROM)?;
    let sock = resolve_net_fd(fd, pid)?;
    net_recvfrom_resolved(sock, buf_ptr, len, flags, addr_ptr, addr_len_ptr)
}

fn net_recvfrom_resolved(
    sock: ResolvedNetSocket,
    buf_ptr: u64,
    len: usize,
    flags: u32,
    addr_ptr: u64,
    addr_len_ptr: u64,
) -> Result<i64, i64> {
    if len == 0 {
        return Ok(0);
    }
    if buf_ptr == 0 {
        return Err(EFAULT);
    }
    let nonblock = sock.nonblock || (flags & MSG_DONTWAIT) != 0;
    let is_dgram = sock.sock_type != SOCK_STREAM;
    let deadline = if sock.rcv_timeout_ns != 0 {
        Some(
            crate::scheduler::timer::clock::monotonic_ns()
                .saturating_add(sock.rcv_timeout_ns),
        )
    } else {
        None
    };

    let window_opt = ensure_net_data_window();
    let max_chunk = window_opt
        .as_ref()
        .map(|w| w.len)
        .unwrap_or(NET_INLINE_DATA_MAX);

    let mut total_recv: usize = 0;
    let mut peer_addr_recorded = false;

    while total_recv < len {
        let chunk_len = (len - total_recv).min(max_chunk);
        let chunk_flags = if total_recv == 0 {
            flags & !NET_FLAG_USE_DATA_WINDOW
        } else {
            (flags & !NET_FLAG_USE_DATA_WINDOW) | MSG_DONTWAIT
        };
        let curr_buf = match buf_ptr.checked_add(total_recv as u64) {
            Some(p) => p,
            None => {
                return if total_recv > 0 {
                    Ok(total_recv as i64)
                } else {
                    Err(EFAULT)
                };
            }
        };

        let (reply, copied_len) = if let Some(ref win) = window_opt {
            if chunk_len > NET_INLINE_DATA_MAX || is_dgram {
                let _lock = match lock_net_data_window() {
                    Ok(g) => g,
                    Err(e) => {
                        return if total_recv > 0 {
                            Ok(total_recv as i64)
                        } else {
                            Err(e)
                        };
                    }
                };
                let msg = make_msg_for(
                    sock.owner_pid,
                    NET_OP_RECVFROM,
                    sock.net_handle,
                    chunk_len as u64,
                    0,
                    chunk_flags | NET_FLAG_USE_DATA_WINDOW,
                    0,
                );
                let mut reply_raw = [0u8; core::mem::size_of::<NetReply>()];
                if let Err(e) = call_network_raw(&msg, &[], &mut reply_raw) {
                    return if total_recv > 0 {
                        Ok(total_recv as i64)
                    } else {
                        Err(e)
                    };
                }
                let r = unsafe {
                    core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply)
                };
                if r.status > 0 {
                    let n = (r.status as usize).min(chunk_len);
                    if let Err(e) = copy_net_window_to_caller_user(win, curr_buf, n) {
                        return if total_recv > 0 {
                            Ok(total_recv as i64)
                        } else {
                            Err(e)
                        };
                    }
                    (r, n)
                } else {
                    (r, 0)
                }
            } else {
                recv_inline_chunk(&sock, curr_buf, chunk_len, chunk_flags, total_recv)?
            }
        } else {
            recv_inline_chunk(&sock, curr_buf, chunk_len, chunk_flags, total_recv)?
        };

        if reply.status == EAGAIN {
            if total_recv == 0 && !nonblock {
                if !wait_net_slice(deadline)? {
                    return Err(EAGAIN);
                }
                continue;
            }
            return if total_recv > 0 {
                Ok(total_recv as i64)
            } else {
                Err(EAGAIN)
            };
        }
        if reply.status < 0 {
            return if total_recv > 0 {
                Ok(total_recv as i64)
            } else {
                Err(reply.status)
            };
        }
        if !peer_addr_recorded {
            let peer_addr = reply_u32(&reply, 8);
            let peer_port = reply_u16(&reply, 12);
            if peer_addr != 0 || addr_ptr != 0 {
                write_sockaddr_in(addr_ptr, addr_len_ptr, peer_addr, peer_port)?;
            }
            peer_addr_recorded = true;
        }
        total_recv += copied_len;
        if copied_len > 0 {
            fs_bridge::notify_io_event(sock.blob_id);
        }
        // Stop after one datagram, EOF (0 bytes), partial TCP read, or MSG_PEEK.
        if is_dgram || copied_len == 0 || copied_len < chunk_len || (flags & MSG_PEEK) != 0 {
            break;
        }
    }
    Ok(total_recv as i64)
}

fn recv_inline_chunk(
    sock: &ResolvedNetSocket,
    curr_buf: u64,
    chunk_len: usize,
    chunk_flags: u32,
    total_recv: usize,
) -> Result<(NetReply, usize), i64> {
    let msg = make_msg_for(
        sock.owner_pid,
        NET_OP_RECVFROM,
        sock.net_handle,
        chunk_len as u64,
        0,
        chunk_flags & !NET_FLAG_USE_DATA_WINDOW,
        0,
    );
    let mut reply_raw = [0u8; core::mem::size_of::<NetReply>() + NET_INLINE_DATA_MAX];
    let n_raw = match call_network_raw(&msg, &[], &mut reply_raw) {
        Ok(n) => n,
        Err(e) => {
            return Err(if total_recv > 0 { EAGAIN } else { e });
        }
    };
    let reply = unsafe { core::ptr::read_unaligned(reply_raw.as_ptr() as *const NetReply) };
    if reply.status <= 0 {
        return Ok((reply, 0));
    }
    let data_len = (reply.status as usize)
        .min(chunk_len)
        .min(n_raw.saturating_sub(NET_REPLY_DATA_OFFSET));
    if data_len != 0
        && copy_to_user(
            curr_buf as *mut u8,
            reply_raw[NET_REPLY_DATA_OFFSET..].as_ptr(),
            data_len,
        )
        .is_err()
    {
        return Err(EFAULT);
    }
    Ok((reply, data_len))
}

pub fn net_sendmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if msg_ptr == 0 {
        return Err(EFAULT);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_sendmsg(fd as u32, msg_ptr, flags, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_SENDMSG)?;
    let msg = read_user_typed::<LinuxMsghdr>(msg_ptr).map_err(|_| EFAULT)?;
    let _total_len = msghdr_total_iov_len(&msg)?;

    let mut total_sent: usize = 0;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(if total_sent > 0 {
                total_sent as i64
            } else {
                EFAULT
            })?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| {
            if total_sent > 0 {
                total_sent as i64
            } else {
                EFAULT
            }
        })?;
        let chunk_len = iov.iov_len as usize;
        if chunk_len != 0 {
            if iov.iov_base == 0 {
                return if total_sent > 0 {
                    Ok(total_sent as i64)
                } else {
                    Err(EFAULT)
                };
            }
            match net_sendto(
                fd,
                iov.iov_base,
                chunk_len,
                flags,
                msg.msg_name,
                msg.msg_namelen as u64,
            ) {
                Ok(n) => {
                    let sent = n.max(0) as usize;
                    total_sent += sent;
                    if sent < chunk_len {
                        break;
                    }
                }
                Err(err) => {
                    return if total_sent > 0 {
                        Ok(total_sent as i64)
                    } else {
                        Err(err)
                    };
                }
            }
        }
        idx += 1;
    }
    Ok(total_sent as i64)
}

pub fn net_recvmsg(fd: i32, msg_ptr: u64, flags: u32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    if msg_ptr == 0 {
        return Err(EFAULT);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_recvmsg(fd as u32, msg_ptr, flags, pid)
            .map_err(|e| e.to_errno());
    }
    check_net_permission(crate::syscall::numbers::SYS_RECVMSG)?;
    let mut msg = read_user_typed::<LinuxMsghdr>(msg_ptr).map_err(|_| EFAULT)?;
    let _total_len = msghdr_total_iov_len(&msg)?;

    let mut total_recv: usize = 0;
    let mut idx = 0u64;
    while idx < msg.msg_iovlen {
        let ptr = msg
            .msg_iov
            .checked_add(idx.saturating_mul(core::mem::size_of::<LinuxIovec>() as u64))
            .ok_or(if total_recv > 0 {
                total_recv as i64
            } else {
                EFAULT
            })?;
        let iov = read_user_typed::<LinuxIovec>(ptr).map_err(|_| {
            if total_recv > 0 {
                total_recv as i64
            } else {
                EFAULT
            }
        })?;
        let chunk_len = iov.iov_len as usize;
        if chunk_len != 0 {
            if iov.iov_base == 0 {
                return if total_recv > 0 {
                    Ok(total_recv as i64)
                } else {
                    Err(EFAULT)
                };
            }
            let effective_flags = if total_recv == 0 {
                flags
            } else {
                flags | MSG_DONTWAIT
            };
            let addr_out = if total_recv == 0 { msg.msg_name } else { 0 };
            match net_recvfrom(fd, iov.iov_base, chunk_len, effective_flags, addr_out, 0) {
                Ok(n) => {
                    let recvd = n.max(0) as usize;
                    if total_recv == 0 && msg.msg_name != 0 {
                        msg.msg_namelen = SOCKADDR_IN_LEN as u32;
                    }
                    total_recv += recvd;
                    if recvd < chunk_len || (flags & MSG_PEEK) != 0 {
                        break;
                    }
                }
                Err(err) => {
                    return if total_recv > 0 {
                        msg.msg_flags = 0;
                        let _ = write_user_typed::<LinuxMsghdr>(msg_ptr, msg);
                        Ok(total_recv as i64)
                    } else {
                        Err(err)
                    };
                }
            }
        }
        idx += 1;
    }
    msg.msg_flags = 0;
    write_user_typed::<LinuxMsghdr>(msg_ptr, msg).map_err(|_| EFAULT)?;
    Ok(total_recv as i64)
}

pub fn net_shutdown(fd: i32, how: i32) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_shutdown(fd as u32, how, pid).map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    dispatch_for(
        sock.owner_pid,
        NET_OP_SHUTDOWN,
        sock.net_handle,
        how as u32 as u64,
        0,
        0,
        0,
    )?;
    fs_bridge::notify_io_event(sock.blob_id);
    Ok(0)
}

pub fn net_getsockname(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_getsockname(fd as u32, addr_ptr, addr_len_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    let reply = dispatch_for(sock.owner_pid, NET_OP_GETSOCKNAME, sock.net_handle, 0, 0, 0, 0)?;
    write_sockaddr_in(
        addr_ptr,
        addr_len_ptr,
        reply_u32(&reply, 8),
        reply_u16(&reply, 12),
    )?;
    Ok(0)
}

pub fn net_getpeername(fd: i32, addr_ptr: u64, addr_len_ptr: u64) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_getpeername(fd as u32, addr_ptr, addr_len_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    let reply = dispatch_for(sock.owner_pid, NET_OP_GETPEERNAME, sock.net_handle, 0, 0, 0, 0)?;
    write_sockaddr_in(
        addr_ptr,
        addr_len_ptr,
        reply_u32(&reply, 8),
        reply_u16(&reply, 12),
    )?;
    Ok(0)
}

pub fn net_socketpair(
    domain: i32,
    ty: i32,
    protocol: i32,
    sv_ptr: u64,
    pid: u32,
) -> Result<i64, i64> {
    if domain == AF_UNIX {
        return fs_bridge::fs_socketpair(domain, ty, protocol, sv_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    if sv_ptr == 0 {
        return Err(EFAULT);
    }
    let reply = dispatch_for(
        pid,
        NET_OP_SOCKETPAIR,
        0,
        domain as u32 as u64,
        ty as u32 as u64,
        protocol as u32,
        0,
    )?;
    let a_h = reply_u32(&reply, 0);
    let b_h = reply_u32(&reply, 4);
    let clean_ty = ty & !((fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC) as i32);
    let open_flags_raw = (ty as u32) & (fs_bridge::O_NONBLOCK | fs_bridge::O_CLOEXEC);
    let fd_a = install_net_socket_fd(a_h, pid, clean_ty, open_flags_raw)? as i32;
    let fd_b = match install_net_socket_fd(b_h, pid, clean_ty, open_flags_raw) {
        Ok(f) => f as i32,
        Err(e) => {
            let _ = fs_bridge::fs_close(fd_a as u32, pid);
            return Err(e);
        }
    };
    let fds = [fd_a, fd_b];
    if copy_to_user(
        sv_ptr as *mut u8,
        fds.as_ptr() as *const u8,
        core::mem::size_of_val(&fds),
    )
    .is_err()
    {
        let _ = fs_bridge::fs_close(fd_a as u32, pid);
        let _ = fs_bridge::fs_close(fd_b as u32, pid);
        return Err(EFAULT);
    }
    Ok(0)
}

pub fn net_setsockopt(
    fd: i32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen: u32,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_setsockopt(fd as u32, level, optname, optval, optlen, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;
    if optlen != 0 && optval == 0 {
        return Err(EFAULT);
    }

    if level == SOL_SOCKET && (optname == SO_RCVTIMEO || optname == SO_SNDTIMEO) {
        if optlen < core::mem::size_of::<fs_bridge::LinuxTimeval>() as u32 {
            return Err(EINVAL);
        }
        let tv = read_user_typed::<fs_bridge::LinuxTimeval>(optval).map_err(|_| EFAULT)?;
        if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
            return Err(EINVAL);
        }
        let ns = (tv.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add((tv.tv_usec as u64).saturating_mul(1_000));
        if let Some(blob_id) = sock.blob_id {
            let mut map = NET_SOCKET_MAP.lock();
            if let Some(m) = map.iter_mut().find(|m| m.blob_id == blob_id) {
                if optname == SO_RCVTIMEO {
                    m.rcv_timeout_ns = ns;
                } else {
                    m.snd_timeout_ns = ns;
                }
            }
        }
        return Ok(0);
    }

    let copy_len = (optlen as usize).min(NET_INLINE_DATA_MAX);
    let mut inline = [0u8; NET_INLINE_DATA_MAX];
    if copy_len != 0
        && copy_from_user(inline.as_mut_ptr(), optval as *const u8, copy_len).is_err()
    {
        return Err(EFAULT);
    }
    let msg = make_msg_for(
        sock.owner_pid,
        NET_OP_SETSOCKOPT,
        sock.net_handle,
        level as u32 as u64,
        optname as u32 as u64,
        optlen,
        0,
    );
    let mut reply_buf = [0u8; core::mem::size_of::<NetReply>()];
    let n = call_network_raw(&msg, &inline[..copy_len], &mut reply_buf)?;
    if n < core::mem::size_of::<NetReply>() {
        return Err(EINVAL);
    }
    let reply = unsafe { core::ptr::read_unaligned(reply_buf.as_ptr() as *const NetReply) };
    if reply.status < 0 {
        return Err(reply.status);
    }
    Ok(0)
}

pub fn net_getsockopt(
    fd: i32,
    level: i32,
    optname: i32,
    optval: u64,
    optlen_ptr: u64,
) -> Result<i64, i64> {
    if fd < 0 {
        return Err(EBADF);
    }
    let pid = current_pid();
    if fs_bridge::fs_is_unix_socket_fd(fd as u32, pid) {
        return fs_bridge::fs_unix_getsockopt(fd as u32, level, optname, optval, optlen_ptr, pid)
            .map_err(|e| e.to_errno());
    }
    let sock = resolve_net_fd(fd, pid)?;

    if level == SOL_SOCKET && (optname == SO_RCVTIMEO || optname == SO_SNDTIMEO) {
        if optval == 0 || optlen_ptr == 0 {
            return Err(EFAULT);
        }
        let user_cap = read_user_typed::<u32>(optlen_ptr).map_err(|_| EFAULT)? as usize;
        if user_cap < core::mem::size_of::<fs_bridge::LinuxTimeval>() {
            return Err(EINVAL);
        }
        let ns = if optname == SO_RCVTIMEO {
            sock.rcv_timeout_ns
        } else {
            sock.snd_timeout_ns
        };
        let tv = fs_bridge::LinuxTimeval {
            tv_sec: (ns / 1_000_000_000) as i64,
            tv_usec: ((ns % 1_000_000_000) / 1_000) as i64,
        };
        write_user_typed(optval, tv).map_err(|_| EFAULT)?;
        write_user_typed(optlen_ptr, core::mem::size_of::<fs_bridge::LinuxTimeval>() as u32)
            .map_err(|_| EFAULT)?;
        return Ok(0);
    }

    let reply = dispatch_for(
        sock.owner_pid,
        NET_OP_GETSOCKOPT,
        sock.net_handle,
        level as u32 as u64,
        optname as u32 as u64,
        0,
        0,
    )?;
    let val_len = (reply_u32(&reply, 24) as usize).clamp(4, 8);
    if optval != 0 {
        let user_cap = if optlen_ptr != 0 {
            read_user_typed::<u32>(optlen_ptr).map_err(|_| EFAULT)? as usize
        } else {
            val_len
        };
        let to_copy = val_len.min(user_cap);
        if to_copy != 0
            && copy_to_user(optval as *mut u8, reply.payload[16..].as_ptr(), to_copy).is_err()
        {
            return Err(EFAULT);
        }
    }
    if optlen_ptr != 0 {
        let out_len = val_len as u32;
        if copy_to_user(
            optlen_ptr as *mut u8,
            &out_len as *const u32 as *const u8,
            core::mem::size_of::<u32>(),
        )
        .is_err()
        {
            return Err(EFAULT);
        }
    }
    Ok(0)
}
