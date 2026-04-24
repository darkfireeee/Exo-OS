use exo_syscall_abi as syscall;

use crate::allocator::{QuotaSnapshot, QuotaTable};
use crate::ipc_bridge::{payload_u32, payload_u64, MemoryReply};

const PAGE_SIZE: u64 = 4_096;
const MAX_REGIONS: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Private,
    Shared,
}

#[derive(Clone, Copy)]
struct RegionEntry {
    active: bool,
    kind: RegionKind,
    handle: u64,
    owner_pid: u32,
    base_addr: u64,
    length: u64,
    prot: u32,
    flags: u32,
    share_count: u16,
}

impl RegionEntry {
    const fn empty() -> Self {
        Self {
            active: false,
            kind: RegionKind::Private,
            handle: 0,
            owner_pid: 0,
            base_addr: 0,
            length: 0,
            prot: 0,
            flags: 0,
            share_count: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct RegionSnapshot {
    pub handle: u64,
    pub length: u64,
    pub prot: u32,
    pub flags: u32,
    pub share_count: u16,
}

pub struct MemoryService {
    quotas: QuotaTable,
    next_handle: u64,
    regions: [RegionEntry; MAX_REGIONS],
}

impl MemoryService {
    pub const fn new() -> Self {
        Self {
            quotas: QuotaTable::new(),
            next_handle: 1,
            regions: [RegionEntry::empty(); MAX_REGIONS],
        }
    }

    fn align_len(length: u64) -> Result<u64, i64> {
        if length == 0 {
            return Err(syscall::EINVAL);
        }
        length
            .checked_add(PAGE_SIZE - 1)
            .map(|value| value & !(PAGE_SIZE - 1))
            .ok_or(syscall::ENOMEM)
    }

    fn region_index(&self, handle: u64) -> Option<usize> {
        self.regions
            .iter()
            .position(|region| region.active && region.handle == handle)
    }

    fn allocate_region(
        &mut self,
        owner_pid: u32,
        kind: RegionKind,
        requested_len: u64,
        prot: u32,
        extra_flags: u32,
    ) -> Result<(RegionSnapshot, QuotaSnapshot), i64> {
        let length = Self::align_len(requested_len)?;
        let quota = self.quotas.reserve(owner_pid, length)?;
        let Some(idx) = self.regions.iter().position(|region| !region.active) else {
            let _ = self.quotas.release(owner_pid, length);
            return Err(syscall::ENOSPC);
        };

        let map_flags = match kind {
            RegionKind::Private => syscall::MAP_PRIVATE | syscall::MAP_ANONYMOUS,
            RegionKind::Shared => syscall::MAP_SHARED | syscall::MAP_ANONYMOUS,
        } | extra_flags as u64;

        // SAFETY: allocation locale de backing store dans le serveur mémoire.
        let base = unsafe {
            syscall::syscall6(
                syscall::SYS_MMAP,
                0,
                length,
                prot as u64,
                map_flags,
                u64::MAX,
                0,
            )
        };

        if base < 0 {
            let _ = self.quotas.release(owner_pid, length);
            return Err(base);
        }

        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1).max(1);

        self.regions[idx] = RegionEntry {
            active: true,
            kind,
            handle,
            owner_pid,
            base_addr: base as u64,
            length,
            prot,
            flags: extra_flags,
            share_count: 0,
        };

        Ok((
            RegionSnapshot {
                handle,
                length,
                prot,
                flags: extra_flags,
                share_count: 0,
            },
            quota,
        ))
    }

    fn region_snapshot(&self, idx: usize) -> RegionSnapshot {
        let region = self.regions[idx];
        RegionSnapshot {
            handle: region.handle,
            length: region.length,
            prot: region.prot,
            flags: region.flags,
            share_count: region.share_count,
        }
    }

    fn reply_with_quota(snapshot: RegionSnapshot, quota: QuotaSnapshot) -> MemoryReply {
        let flags = ((snapshot.prot as u64) & 0xFFFF)
            | (((snapshot.flags as u64) & 0xFFFF) << 16)
            | ((snapshot.share_count as u64) << 32);
        MemoryReply::ok(
            snapshot.handle,
            snapshot.length,
            quota.used_bytes,
            flags as u32,
        )
    }

    pub fn handle_alloc(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let size = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let prot = match payload_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let flags = match payload_u32(payload, 12) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };

        match self.allocate_region(sender_pid, RegionKind::Private, size, prot, flags) {
            Ok((snapshot, quota)) => Self::reply_with_quota(snapshot, quota),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn handle_free(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let handle = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };

        let Some(idx) = self.region_index(handle) else {
            return MemoryReply::error(syscall::ENOENT);
        };
        let region = self.regions[idx];

        if region.owner_pid != sender_pid && sender_pid != 1 {
            return MemoryReply::error(syscall::EACCES);
        }
        if region.kind == RegionKind::Shared && region.share_count != 0 {
            return MemoryReply::error(syscall::EBUSY);
        }

        // SAFETY: unmap du backing store alloué plus haut pour cette région.
        let rc = unsafe { syscall::syscall2(syscall::SYS_MUNMAP, region.base_addr, region.length) };
        if rc < 0 {
            return MemoryReply::error(rc);
        }

        self.regions[idx] = RegionEntry::empty();
        match self.quotas.release(region.owner_pid, region.length) {
            Ok(quota) => MemoryReply::ok(0, 0, quota.used_bytes, 0),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn handle_protect(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let handle = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let prot = match payload_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };

        let Some(idx) = self.region_index(handle) else {
            return MemoryReply::error(syscall::ENOENT);
        };
        let region = &mut self.regions[idx];
        if region.owner_pid != sender_pid && sender_pid != 1 {
            return MemoryReply::error(syscall::EACCES);
        }

        // SAFETY: `mprotect` agit sur le mapping local détenu par le serveur.
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_MPROTECT,
                region.base_addr,
                region.length,
                prot as u64,
            )
        };
        if rc < 0 {
            return MemoryReply::error(rc);
        }

        region.prot = prot;
        match self.quotas.snapshot(region.owner_pid) {
            Ok(quota) => Self::reply_with_quota(self.region_snapshot(idx), quota),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn handle_query(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let handle = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };

        let Some(idx) = self.region_index(handle) else {
            return MemoryReply::error(syscall::ENOENT);
        };
        let region = self.regions[idx];
        if region.owner_pid != sender_pid && sender_pid != 1 {
            return MemoryReply::error(syscall::EACCES);
        }

        match self.quotas.snapshot(region.owner_pid) {
            Ok(quota) => Self::reply_with_quota(self.region_snapshot(idx), quota),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn handle_quota_set(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        if sender_pid != 1 {
            return MemoryReply::error(syscall::EPERM);
        }

        let pid = match payload_u32(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let limit = match payload_u64(payload, 8) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };

        match self.quotas.set_limit(pid, limit) {
            Ok(snapshot) => {
                MemoryReply::ok(pid as u64, snapshot.used_bytes, snapshot.limit_bytes, 0)
            }
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn handle_quota_query(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let requested_pid = match payload_u32(payload, 0) {
            Ok(value) if value != 0 => value,
            Ok(_) => sender_pid,
            Err(err) => return MemoryReply::error(err),
        };

        if requested_pid != sender_pid && sender_pid != 1 {
            return MemoryReply::error(syscall::EPERM);
        }

        match self.quotas.snapshot(requested_pid) {
            Ok(snapshot) => MemoryReply::ok(
                requested_pid as u64,
                snapshot.used_bytes,
                snapshot.limit_bytes,
                snapshot.peak_bytes.min(u32::MAX as u64) as u32,
            ),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn create_shared_region(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let size = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let prot = match payload_u32(payload, 8) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let flags = match payload_u32(payload, 12) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };

        match self.allocate_region(sender_pid, RegionKind::Shared, size, prot, flags) {
            Ok((snapshot, quota)) => Self::reply_with_quota(snapshot, quota),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn attach_shared_region(&mut self, _sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let handle = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let Some(idx) = self.region_index(handle) else {
            return MemoryReply::error(syscall::ENOENT);
        };
        let region = &mut self.regions[idx];
        if region.kind != RegionKind::Shared {
            return MemoryReply::error(syscall::EINVAL);
        }

        region.share_count = region.share_count.saturating_add(1);
        match self.quotas.snapshot(region.owner_pid) {
            Ok(quota) => Self::reply_with_quota(self.region_snapshot(idx), quota),
            Err(err) => MemoryReply::error(err),
        }
    }

    pub fn destroy_shared_region(&mut self, sender_pid: u32, payload: &[u8]) -> MemoryReply {
        let handle = match payload_u64(payload, 0) {
            Ok(value) => value,
            Err(err) => return MemoryReply::error(err),
        };
        let Some(idx) = self.region_index(handle) else {
            return MemoryReply::error(syscall::ENOENT);
        };
        let region = self.regions[idx];
        if region.kind != RegionKind::Shared {
            return MemoryReply::error(syscall::EINVAL);
        }
        if sender_pid != region.owner_pid && sender_pid != 1 {
            if region.share_count == 0 {
                return MemoryReply::error(syscall::EACCES);
            }
            self.regions[idx].share_count = region.share_count.saturating_sub(1);
            match self.quotas.snapshot(region.owner_pid) {
                Ok(quota) => Self::reply_with_quota(self.region_snapshot(idx), quota),
                Err(err) => MemoryReply::error(err),
            }
        } else if region.share_count > 0 {
            self.regions[idx].share_count = region.share_count - 1;
            match self.quotas.snapshot(region.owner_pid) {
                Ok(quota) => Self::reply_with_quota(self.region_snapshot(idx), quota),
                Err(err) => MemoryReply::error(err),
            }
        } else {
            self.handle_free(sender_pid, payload)
        }
    }
}
