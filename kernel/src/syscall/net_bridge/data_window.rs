//! # syscall/net_bridge/data_window.rs — Ring 0 <-> `network_server` 16 KiB `NET_DATA_WINDOW`

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::{phys_to_virt, PhysAddr, VirtAddr};
use crate::process::core::pid::Pid;
use crate::process::core::registry::PROCESS_REGISTRY;
use crate::syscall::errno::EMSGSIZE;
use crate::syscall::validation::{copy_from_user, copy_to_user, USER_ADDR_MAX};
use exo_abi::errno::{EFAULT, EINTR, ENETDOWN};

use super::{
    call_network, current_pid, make_msg_for, reply_u32, reply_u64, NET_DATA_WINDOW_MAX,
    NET_OP_HANDSHAKE,
};

#[derive(Clone, Copy)]
pub(super) struct NetDataWindowInfo {
    pub(super) pid: u32,
    pub(super) va: u64,
    pub(super) len: usize,
}

pub(super) static NET_DATA_WINDOW_CACHE: Mutex<Option<NetDataWindowInfo>> = Mutex::new(None);
static NET_DATA_LOCKED: AtomicBool = AtomicBool::new(false);
static NET_DATA_GEN: AtomicU64 = AtomicU64::new(0);
static NET_DATA_WAITERS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

pub(super) struct NetDataLockGuard;

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
pub(super) fn lock_net_data_window() -> Result<NetDataLockGuard, i64> {
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
    Some((
        entry.phys_addr().as_u64() + offset as u64,
        page_size - offset,
    ))
}

pub(super) fn ensure_net_data_window() -> Option<NetDataWindowInfo> {
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

pub(super) fn copy_caller_user_to_net_window(
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

pub(super) fn copy_net_window_to_caller_user(
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
