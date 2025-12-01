//! Memory Management System Call Handlers
//!
//! Handles memory operations: mmap, munmap, mprotect, brk

use crate::memory::address::VirtualAddress;
use crate::memory::{MemoryError, MemoryResult};

/// Memory protection flags
#[derive(Debug, Clone, Copy)]
pub struct ProtFlags {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
}

impl ProtFlags {
    pub const NONE: Self = Self {
        read: false,
        write: false,
        exec: false,
    };
    pub const READ: Self = Self {
        read: true,
        write: false,
        exec: false,
    };
    pub const WRITE: Self = Self {
        read: false,
        write: true,
        exec: false,
    };
    pub const EXEC: Self = Self {
        read: false,
        write: false,
        exec: true,
    };
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        exec: false,
    };
    pub const READ_EXEC: Self = Self {
        read: true,
        write: false,
        exec: true,
    };
    pub const ALL: Self = Self {
        read: true,
        write: true,
        exec: true,
    };
}

/// Memory mapping flags
#[derive(Debug, Clone, Copy)]
pub struct MapFlags {
    pub shared: bool,
    pub private: bool,
    pub fixed: bool,
    pub anonymous: bool,
}

/// Map memory
pub fn sys_mmap(
    addr: VirtualAddress,
    length: usize,
    prot: ProtFlags,
    flags: MapFlags,
    fd: u64,
    offset: usize,
) -> MemoryResult<VirtualAddress> {
    log::debug!(
        "sys_mmap: addr={:?}, len={}, prot={:?}, flags={:?}, fd={}, offset={}",
        addr,
        length,
        prot,
        flags,
        fd,
        offset
    );

    // 1. Validate parameters
    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    if !flags.shared && !flags.private {
        return Err(MemoryError::InvalidParameter);
    }

    // 2. Convert to internal types
    let protection = if prot.read && prot.write && prot.exec {
        crate::memory::PageProtection::new()
            .read()
            .write()
            .execute()
    } else if prot.read && prot.write {
        crate::memory::PageProtection::new().read().write()
    } else if prot.read && prot.exec {
        crate::memory::PageProtection::new().read().execute()
    } else if prot.read {
        crate::memory::PageProtection::new().read()
    } else {
        crate::memory::PageProtection::new()
    };

    let mmap_flags = crate::memory::mmap::MmapFlags::new(
        (if flags.shared {
            crate::memory::mmap::MmapFlags::SHARED
        } else {
            0
        }) | (if flags.private {
            crate::memory::mmap::MmapFlags::PRIVATE
        } else {
            0
        }) | (if flags.fixed {
            crate::memory::mmap::MmapFlags::FIXED
        } else {
            0
        }) | (if flags.anonymous {
            crate::memory::mmap::MmapFlags::ANONYMOUS
        } else {
            0
        }),
    );

    // 3. Call mmap manager
    let fd_opt = if flags.anonymous {
        None
    } else {
        Some(fd as i32)
    };
    let addr_opt = if addr.value() != 0 { Some(addr) } else { None };

    let result =
        crate::memory::mmap::mmap(addr_opt, length, protection, mmap_flags, fd_opt, offset)?;

    log::info!(
        "mmap: allocated {:?}, size {}, prot={:?}",
        result,
        length,
        prot
    );
    Ok(result)
}

/// Unmap memory
pub fn sys_munmap(addr: VirtualAddress, length: usize) -> MemoryResult<()> {
    log::debug!("sys_munmap: addr={:?}, len={}", addr, length);

    // 1. Validate address range
    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    if addr.value() % 4096 != 0 {
        return Err(MemoryError::AlignmentError);
    }

    // 2. Call mmap manager to unmap
    crate::memory::mmap::munmap(addr, length)?;

    log::info!("munmap: unmapped {:?}, size {}", addr, length);
    Ok(())
}

/// Change memory protection
pub fn sys_mprotect(addr: VirtualAddress, length: usize, prot: ProtFlags) -> MemoryResult<()> {
    log::debug!(
        "sys_mprotect: addr={:?}, len={}, prot={:?}",
        addr,
        length,
        prot
    );

    // 1. Validate address range
    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    if addr.value() % 4096 != 0 {
        return Err(MemoryError::AlignmentError);
    }

    // 2. Convert protection flags
    let protection = if prot.read && prot.write && prot.exec {
        crate::memory::PageProtection::new()
            .read()
            .write()
            .execute()
    } else if prot.read && prot.write {
        crate::memory::PageProtection::new().read().write()
    } else if prot.read && prot.exec {
        crate::memory::PageProtection::new().read().execute()
    } else if prot.read {
        crate::memory::PageProtection::new().read()
    } else {
        crate::memory::PageProtection::new()
    };

    // 3. Update page table entries and flush TLB
    crate::memory::mmap::mprotect(addr, length, protection)?;

    log::info!(
        "mprotect: changed {:?}, size {}, prot={:?}",
        addr,
        length,
        prot
    );
    Ok(())
}

/// Sync memory to disk (msync)
pub fn sys_msync(addr: VirtualAddress, length: usize, flags: u32) -> MemoryResult<()> {
    log::debug!(
        "sys_msync: addr={:?}, len={}, flags={}",
        addr,
        length,
        flags
    );

    const MS_ASYNC: u32 = 1;
    const MS_SYNC: u32 = 4;
    const MS_INVALIDATE: u32 = 2;

    // 1. Validate parameters
    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    // 2. Find mapped region (stub - would check mmap table)
    // 3. If file-backed, write dirty pages (stub - needs VFS)
    // For now, just acknowledge the sync request

    if flags & MS_SYNC != 0 {
        log::debug!("msync: synchronous sync requested for {:?}", addr);
        // Wait for completion (stub)
    } else if flags & MS_ASYNC != 0 {
        log::debug!("msync: asynchronous sync requested for {:?}", addr);
        // Queue for later (stub)
    }

    if flags & MS_INVALIDATE != 0 {
        log::debug!("msync: invalidate cache for {:?}", addr);
        // Invalidate cached pages (stub)
    }

    Ok(())
}

use core::sync::atomic::{AtomicUsize, Ordering};

// Process heap break (simplified - should be per-process)
static PROGRAM_BREAK: AtomicUsize = AtomicUsize::new(0x40000000); // Start at 1GB
const HEAP_START: usize = 0x40000000;
const HEAP_MAX: usize = 0x80000000; // Max 1GB heap

/// Program break (brk) - adjust heap size
pub fn sys_brk(addr: VirtualAddress) -> MemoryResult<VirtualAddress> {
    log::debug!("sys_brk: addr={:?}", addr);

    let current_break = PROGRAM_BREAK.load(Ordering::Relaxed);

    // 1. If addr is 0, return current break
    if addr.value() == 0 {
        return Ok(VirtualAddress::new(current_break));
    }

    // 2. Validate new break address
    let new_break = addr.value();

    if new_break < HEAP_START || new_break > HEAP_MAX {
        log::warn!("brk: invalid address {:?}", addr);
        return Ok(VirtualAddress::new(current_break)); // Return old break on error
    }

    // 3. Expand or shrink heap
    if new_break > current_break {
        // Expanding heap - allocate pages
        let size = new_break - current_break;
        let pages = (size + 4095) / 4096;

        log::debug!("brk: expanding heap by {} bytes ({} pages)", size, pages);

        // Map new pages (simplified - should use mmap internally)
        for i in 0..pages {
            let page_addr = current_break + (i * 4096);
            // Stub: actual page allocation would happen here
            let _ = page_addr;
        }
    } else if new_break < current_break {
        // Shrinking heap - free pages
        let size = current_break - new_break;
        log::debug!("brk: shrinking heap by {} bytes", size);
        // Stub: actual page deallocation would happen here
    }

    // 4. Update program break
    PROGRAM_BREAK.store(new_break, Ordering::Relaxed);

    Ok(VirtualAddress::new(new_break))
}

const MADV_NORMAL: i32 = 0;
const MADV_RANDOM: i32 = 1;
const MADV_SEQUENTIAL: i32 = 2;
const MADV_WILLNEED: i32 = 3;
const MADV_DONTNEED: i32 = 4;
const MADV_FREE: i32 = 8;

/// Advise kernel about memory usage (madvise)
pub fn sys_madvise(addr: VirtualAddress, length: usize, advice: i32) -> MemoryResult<()> {
    log::debug!(
        "sys_madvise: addr={:?}, len={}, advice={}",
        addr,
        length,
        advice
    );

    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    match advice {
        MADV_NORMAL => {
            log::debug!("madvise: normal access pattern for {:?}", addr);
            // Reset to default behavior
        }
        MADV_RANDOM => {
            log::debug!("madvise: random access pattern for {:?}", addr);
            // Disable readahead
        }
        MADV_SEQUENTIAL => {
            log::debug!("madvise: sequential access pattern for {:?}", addr);
            // Enable aggressive readahead
        }
        MADV_WILLNEED => {
            log::debug!("madvise: prefetch pages at {:?}", addr);
            // Prefetch pages into cache (stub)
        }
        MADV_DONTNEED => {
            log::debug!("madvise: can free pages at {:?}", addr);
            // Mark pages as freeable (stub)
        }
        MADV_FREE => {
            log::debug!("madvise: free pages at {:?} if needed", addr);
            // Mark pages for lazy freeing
        }
        _ => {
            log::warn!("madvise: unknown advice {}", advice);
            return Err(MemoryError::InvalidParameter);
        }
    }

    Ok(())
}

use alloc::collections::BTreeSet;
static LOCKED_PAGES: spin::Mutex<BTreeSet<usize>> = spin::Mutex::new(BTreeSet::new());

/// Lock pages in memory (mlock)
pub fn sys_mlock(addr: VirtualAddress, length: usize) -> MemoryResult<()> {
    log::debug!("sys_mlock: addr={:?}, len={}", addr, length);

    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    // 1. Calculate page range
    let start_page = addr.value() / 4096;
    let end_page = (addr.value() + length + 4095) / 4096;

    // 2. Check limits (stub - should check RLIMIT_MEMLOCK)
    let page_count = end_page - start_page;
    const MAX_LOCKED_PAGES: usize = 65536; // 256MB limit

    let mut locked = LOCKED_PAGES.lock();
    if locked.len() + page_count > MAX_LOCKED_PAGES {
        log::warn!(
            "mlock: would exceed limit ({} + {} > {})",
            locked.len(),
            page_count,
            MAX_LOCKED_PAGES
        );
        return Err(MemoryError::OutOfMemory);
    }

    // 3. Pin pages in physical memory
    for page in start_page..end_page {
        locked.insert(page);
    }

    log::info!("mlock: locked {} pages at {:?}", page_count, addr);
    Ok(())
}

/// Unlock pages (munlock)
pub fn sys_munlock(addr: VirtualAddress, length: usize) -> MemoryResult<()> {
    log::debug!("sys_munlock: addr={:?}, len={}", addr, length);

    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    // Calculate page range
    let start_page = addr.value() / 4096;
    let end_page = (addr.value() + length + 4095) / 4096;

    // Unpin pages
    let mut locked = LOCKED_PAGES.lock();
    let mut count = 0;
    for page in start_page..end_page {
        if locked.remove(&page) {
            count += 1;
        }
    }

    log::info!("munlock: unlocked {} pages at {:?}", count, addr);
    Ok(())
}

const MREMAP_MAYMOVE: u32 = 1;
const MREMAP_FIXED: u32 = 2;

/// Remap pages (mremap) - Linux-specific
pub fn sys_mremap(
    old_addr: VirtualAddress,
    old_size: usize,
    new_size: usize,
    flags: u32,
    new_addr: VirtualAddress,
) -> MemoryResult<VirtualAddress> {
    log::debug!(
        "sys_mremap: old_addr={:?}, old_size={}, new_size={}, flags={}, new_addr={:?}",
        old_addr,
        old_size,
        new_size,
        flags,
        new_addr
    );

    // 1. Validate parameters
    if old_size == 0 || new_size == 0 {
        return Err(MemoryError::InvalidSize);
    }

    // 2. If shrinking, unmap excess pages
    if new_size < old_size {
        let excess_start = VirtualAddress::new(old_addr.value() + new_size);
        let excess_size = old_size - new_size;
        sys_munmap(excess_start, excess_size)?;
        log::info!(
            "mremap: shrunk mapping from {} to {} bytes",
            old_size,
            new_size
        );
        return Ok(old_addr);
    }

    // 3. If growing, try to expand in place
    if new_size > old_size {
        let additional = new_size - old_size;

        // Try to expand in place first
        // (stub - would check if adjacent space is free)
        let can_expand_in_place = false;

        if can_expand_in_place {
            log::info!("mremap: expanded in place to {} bytes", new_size);
            return Ok(old_addr);
        }

        // 4. If MREMAP_MAYMOVE, allocate new location and copy
        if flags & MREMAP_MAYMOVE != 0 {
            let target_addr = if flags & MREMAP_FIXED != 0 {
                Some(new_addr)
            } else {
                None
            };

            // Allocate new mapping
            let result = sys_mmap(
                target_addr.unwrap_or(VirtualAddress::new(0)),
                new_size,
                ProtFlags::READ_WRITE,
                MapFlags {
                    shared: false,
                    private: true,
                    fixed: flags & MREMAP_FIXED != 0,
                    anonymous: true,
                },
                0,
                0,
            )?;

            // Copy old data (stub - would use memcpy)
            log::info!(
                "mremap: moved from {:?} to {:?}, size {}",
                old_addr,
                result,
                new_size
            );

            // Unmap old region
            sys_munmap(old_addr, old_size)?;

            return Ok(result);
        }
    }

    log::warn!("mremap: cannot expand and MREMAP_MAYMOVE not set");
    Err(MemoryError::OutOfMemory)
}

/// Get memory information
pub fn sys_meminfo() -> MemoryResult<MemInfo> {
    log::debug!("sys_meminfo");

    // Query buddy allocator for stats (stub - would use actual allocator API)
    let total = 512 * 1024 * 1024; // 512MB
    let used = 128 * 1024 * 1024; // 128MB used
    let free = total - used;
    let cached = 32 * 1024 * 1024; // 32MB cache
    let buffers = 16 * 1024 * 1024; // 16MB buffers

    Ok(MemInfo {
        total,
        free,
        used,
        cached,
        buffers,
    })
}

/// Memory information structure
#[derive(Debug, Clone, Copy)]
pub struct MemInfo {
    pub total: usize,
    pub free: usize,
    pub used: usize,
    pub cached: usize,
    pub buffers: usize,
}
/// Check if pages are in memory (mincore)
pub fn sys_mincore(addr: VirtualAddress, length: usize, vec: *mut u8) -> MemoryResult<()> {
    log::debug!(
        "sys_mincore: addr={:?}, len={}, vec={:?}",
        addr,
        length,
        vec
    );

    if length == 0 {
        return Err(MemoryError::InvalidSize);
    }

    if addr.value() % 4096 != 0 {
        return Err(MemoryError::AlignmentError);
    }

    // Calculate number of pages
    let page_count = (length + 4095) / 4096;

    // Validate output buffer
    if vec.is_null() {
        return Err(MemoryError::InvalidAddress);
    }

    // In our current implementation without swap, all valid mappings are resident.
    // We should check if the pages are actually mapped.
    // Stub: assume all pages in range are resident if mapped.

    // TODO: Verify mappings exist for the range

    // Fill vector with 1s (resident)
    // Safety: we trust the user provided a valid buffer of sufficient size (page_count bytes)
    // In a real kernel we'd use copy_to_user
    unsafe {
        for i in 0..page_count {
            *vec.add(i) = 1; // 1 = resident
        }
    }

    Ok(())
}
