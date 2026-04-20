//! # memory_dump — Memory region dump storage & checksum verification
//!
//! Captures and stores memory region dumps for forensic analysis.
//! The dump storage is a static 64 KB buffer that holds up to 16
//! dump regions. Each region is tracked with its address, size,
//! PID, and a CRC-32 checksum for integrity verification.
//!
//! ## Constraints
//! - `#![no_std]` compatible: only `core::sync::atomic` + `spin`
//! - Static 64 KB storage buffer — no heap allocation
//! - CRC-32 checksum for tamper detection
//! - Maximum 16 dump regions tracked simultaneously

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Total size of the dump storage buffer in bytes.
pub const DUMP_STORAGE_SIZE: usize = 65536;

/// Maximum number of dump region descriptors.
const MAX_DUMP_REGIONS: usize = 16;

/// Maximum size of a single dump region (4 KB).
const MAX_REGION_SIZE: usize = 4096;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Descriptor for a single memory dump region.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DumpRegion {
    /// PID of the process that owns the memory.
    pub pid: u32,
    /// Starting virtual address of the dump.
    pub addr: u64,
    /// Size of the dump in bytes.
    pub size: u32,
    /// Offset into the dump storage buffer.
    pub storage_offset: u32,
    /// CRC-32 checksum of the dump data.
    pub checksum: u32,
    /// Flags: bit 0 = valid, bit 1 = compressed (reserved).
    pub flags: u8,
    /// Dump reason: 0=manual, 1=overflow, 2=crash, 3=anomaly.
    pub reason: u8,
    /// Padding.
    pub _pad: [u8; 2],
    /// TSC timestamp of the dump.
    pub timestamp: u64,
}

impl Default for DumpRegion {
    fn default() -> Self {
        Self {
            pid: 0,
            addr: 0,
            size: 0,
            storage_offset: 0,
            checksum: 0,
            flags: 0,
            reason: 0,
            _pad: [0; 2],
            timestamp: 0,
        }
    }
}

/// Checksum verification result.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DumpChecksum {
    /// The stored checksum.
    pub stored: u32,
    /// The computed checksum.
    pub computed: u32,
    /// Whether the checksums match.
    pub valid: u8,
    /// Padding.
    pub _pad: [u8; 3],
}

impl Default for DumpChecksum {
    fn default() -> Self {
        Self {
            stored: 0,
            computed: 0,
            valid: 0,
            _pad: [0; 3],
        }
    }
}

/// Dump storage handle — provides typed access to the static buffer.
pub struct DumpStorage;

impl DumpStorage {
    /// Creates a new handle to the global dump storage.
    pub const fn new() -> Self {
        Self
    }

    /// Returns the total capacity of the dump storage in bytes.
    pub const fn capacity(&self) -> usize {
        DUMP_STORAGE_SIZE
    }

    /// Returns the number of bytes currently used.
    pub fn used(&self) -> usize {
        STORAGE_USED.load(Ordering::Acquire) as usize
    }

    /// Returns the number of bytes available.
    pub fn available(&self) -> usize {
        DUMP_STORAGE_SIZE.saturating_sub(self.used())
    }
}

// ── CRC-32 implementation ────────────────────────────────────────────────────

/// CRC-32 lookup table (IEEE 802.3 polynomial: 0xEDB88320).
static CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0usize;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Computes a CRC-32 checksum over the given data.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        let idx = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC_TABLE[idx];
    }
    !crc
}

// ── TSC read ──────────────────────────────────────────────────────────────────

#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ── Static storage ────────────────────────────────────────────────────────────

/// Dump storage buffer — 64 KB static.
static DUMP_BUFFER: Mutex<[u8; DUMP_STORAGE_SIZE]> = Mutex::new([0u8; DUMP_STORAGE_SIZE]);

/// Dump region descriptors.
static DUMP_REGIONS: Mutex<[DumpRegion; MAX_DUMP_REGIONS]> = Mutex::new(
    [DumpRegion::default(); MAX_DUMP_REGIONS],
);

/// Number of active dump regions.
static DUMP_COUNT: AtomicU32 = AtomicU32::new(0);

/// Number of bytes used in the dump buffer.
static STORAGE_USED: AtomicU32 = AtomicU32::new(0);

/// Statistics.
static TOTAL_DUMPS: AtomicU64 = AtomicU64::new(0);
static CHECKSUM_FAILURES: AtomicU64 = AtomicU64::new(0);
static DUMPS_EVICTED: AtomicU64 = AtomicU64::new(0);

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Find space in the dump buffer for a region of the given size.
/// Returns the offset if space is available, or attempts eviction.
fn find_storage_space(size: u32) -> Option<u32> {
    let used = STORAGE_USED.load(Ordering::Acquire);
    if used + size <= DUMP_STORAGE_SIZE as u32 {
        return Some(used);
    }

    // Not enough contiguous space — try to compact by evicting oldest regions
    let mut regions = DUMP_REGIONS.lock();
    let mut buffer = DUMP_BUFFER.lock();
    let count = DUMP_COUNT.load(Ordering::Acquire) as usize;

    // Find the oldest region to evict
    let mut oldest_idx = 0usize;
    let mut oldest_tsc = u64::MAX;
    for i in 0..count.min(MAX_DUMP_REGIONS) {
        if regions[i].flags & 1 != 0 && regions[i].timestamp < oldest_tsc {
            oldest_tsc = regions[i].timestamp;
            oldest_idx = i;
        }
    }

    if count > 0 && oldest_tsc < u64::MAX {
        // Evict the oldest region
        let evict_offset = regions[oldest_idx].storage_offset;
        let evict_size = regions[oldest_idx].size as usize;

        // Shift data after the evicted region backward
        let total_used = STORAGE_USED.load(Ordering::Acquire) as usize;
        if evict_offset as usize + evict_size < total_used {
            let src_start = evict_offset as usize + evict_size;
            let dst_start = evict_offset as usize;
            let move_len = total_used - src_start;
            // Copy within the same buffer — use temporary chunk copy
            let mut tmp = [0u8; 256];
            let mut copied = 0usize;
            while copied < move_len {
                let chunk = move_len.min(256).min(move_len - copied);
                let src = src_start + copied;
                tmp[..chunk].copy_from_slice(&buffer[src..src + chunk]);
                let dst = dst_start + copied;
                buffer[dst..dst + chunk].copy_from_slice(&tmp[..chunk]);
                copied += chunk;
            }
        }

        // Update storage used
        let new_used = total_used.saturating_sub(evict_size);
        STORAGE_USED.store(new_used as u32, Ordering::Release);
        DUMPS_EVICTED.fetch_add(1, Ordering::Relaxed);

        // Update offsets for regions after the evicted one
        for i in 0..count.min(MAX_DUMP_REGIONS) {
            if regions[i].flags & 1 != 0 && regions[i].storage_offset > evict_offset {
                regions[i].storage_offset -= evict_size as u32;
            }
        }

        // Mark evicted region as inactive
        regions[oldest_idx].flags = 0;

        // Compact the regions array
        let mut write = 0usize;
        for read in 0..count.min(MAX_DUMP_REGIONS) {
            if regions[read].flags & 1 != 0 {
                if write != read {
                    regions[write] = regions[read];
                    regions[read].flags = 0;
                }
                write += 1;
            }
        }
        DUMP_COUNT.store(write as u32, Ordering::Release);

        // Try again
        let used = STORAGE_USED.load(Ordering::Acquire);
        if used + size <= DUMP_STORAGE_SIZE as u32 {
            return Some(used);
        }
    }

    None
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Stores a memory dump into the forensic storage buffer.
///
/// Copies `data` into the static buffer and creates a region descriptor
/// with a CRC-32 checksum. If the buffer is full, the oldest dump is
/// evicted.
///
/// Returns the dump region index (0–15) on success, or `0xFF` on failure.
pub fn store_dump(pid: u32, addr: u64, data: &[u8], reason: u8) -> u8 {
    let size = data.len().min(MAX_REGION_SIZE) as u32;
    if size == 0 {
        return 0xFF;
    }

    // Find storage space
    let offset = match find_storage_space(size) {
        Some(off) => off,
        None => return 0xFF,
    };

    // Write data to storage buffer
    {
        let mut buffer = DUMP_BUFFER.lock();
        buffer[offset as usize..(offset as usize + size as usize)]
            .copy_from_slice(&data[..size as usize]);
    }

    // Compute checksum
    let checksum = crc32(&data[..size as usize]);

    let now = read_tsc();
    let region = DumpRegion {
        pid,
        addr,
        size,
        storage_offset: offset,
        checksum,
        flags: 1, // valid
        reason,
        _pad: [0; 2],
        timestamp: now,
    };

    // Store the region descriptor
    let idx = {
        let mut regions = DUMP_REGIONS.lock();
        let count = DUMP_COUNT.load(Ordering::Acquire) as usize;
        if count < MAX_DUMP_REGIONS {
            regions[count] = region;
            DUMP_COUNT.fetch_add(1, Ordering::Release);
            count as u8
        } else {
            // Find an invalid slot
            for i in 0..MAX_DUMP_REGIONS {
                if regions[i].flags & 1 == 0 {
                    regions[i] = region;
                    DUMP_COUNT.fetch_add(1, Ordering::Release);
                    return i as u8;
                }
            }
            0xFF
        }
    };

    if idx != 0xFF {
        STORAGE_USED.fetch_add(size, Ordering::Release);
        TOTAL_DUMPS.fetch_add(1, Ordering::Relaxed);
    }

    idx
}

/// Retrieves a stored dump by region index.
///
/// Copies the dump data into `out_buf` and returns the region descriptor.
/// Returns `None` if the index is invalid or the region is not active.
pub fn retrieve_dump(region_idx: u8, out_buf: &mut [u8]) -> Option<DumpRegion> {
    let idx = region_idx as usize;
    if idx >= MAX_DUMP_REGIONS {
        return None;
    }

    let regions = DUMP_REGIONS.lock();
    let region = regions[idx];

    if region.flags & 1 == 0 {
        return None;
    }

    let size = region.size as usize;
    let copy_len = size.min(out_buf.len());

    {
        let buffer = DUMP_BUFFER.lock();
        let src_start = region.storage_offset as usize;
        if src_start + size <= DUMP_STORAGE_SIZE {
            out_buf[..copy_len].copy_from_slice(
                &buffer[src_start..src_start + copy_len],
            );
        }
    }

    Some(region)
}

/// Verifies the CRC-32 checksum of a stored dump region.
///
/// Reads the data from storage, recomputes the checksum, and compares
/// it with the stored value. Returns a `DumpChecksum` with the result.
pub fn verify_dump_checksum(region_idx: u8) -> DumpChecksum {
    let idx = region_idx as usize;
    if idx >= MAX_DUMP_REGIONS {
        return DumpChecksum::default();
    }

    let regions = DUMP_REGIONS.lock();
    let region = regions[idx];

    if region.flags & 1 == 0 {
        return DumpChecksum::default();
    }

    let size = region.size as usize;
    let offset = region.storage_offset as usize;
    let stored_checksum = region.checksum;

    drop(regions);

    // Read data and compute checksum
    let computed = {
        let buffer = DUMP_BUFFER.lock();
        if offset + size <= DUMP_STORAGE_SIZE {
            crc32(&buffer[offset..offset + size])
        } else {
            0
        }
    };

    let valid = if stored_checksum == computed { 1 } else { 0 };
    if valid == 0 {
        CHECKSUM_FAILURES.fetch_add(1, Ordering::Relaxed);
    }

    DumpChecksum {
        stored: stored_checksum,
        computed,
        valid,
        _pad: [0; 3],
    }
}

/// Enumerates all active dump regions into a caller-provided buffer.
///
/// Returns the number of regions written.
pub fn enumerate_dumps(out: &mut [DumpRegion]) -> usize {
    let regions = DUMP_REGIONS.lock();
    let count = DUMP_COUNT.load(Ordering::Acquire) as usize;
    let mut written = 0usize;

    for i in 0..count.min(MAX_DUMP_REGIONS) {
        if regions[i].flags & 1 != 0 && written < out.len() {
            out[written] = regions[i];
            written += 1;
        }
    }
    written
}

/// Deletes a dump region by index.
///
/// Returns `true` if the region was found and deleted.
pub fn delete_dump(region_idx: u8) -> bool {
    let idx = region_idx as usize;
    if idx >= MAX_DUMP_REGIONS {
        return false;
    }

    let mut regions = DUMP_REGIONS.lock();
    if regions[idx].flags & 1 == 0 {
        return false;
    }

    let freed_size = regions[idx].size;
    regions[idx].flags = 0;
    DUMP_COUNT.fetch_sub(1, Ordering::Release);
    STORAGE_USED.fetch_sub(freed_size, Ordering::Release);
    true
}

/// Verifies all stored dump regions and returns the number of checksum failures.
pub fn verify_all_dumps() -> u32 {
    let regions = DUMP_REGIONS.lock();
    let count = DUMP_COUNT.load(Ordering::Acquire) as usize;
    let mut failures = 0u32;

    for i in 0..count.min(MAX_DUMP_REGIONS) {
        if regions[i].flags & 1 != 0 {
            let offset = regions[i].storage_offset as usize;
            let size = regions[i].size as usize;
            let stored = regions[i].checksum;

            let computed = {
                let buffer = DUMP_BUFFER.lock();
                if offset + size <= DUMP_STORAGE_SIZE {
                    crc32(&buffer[offset..offset + size])
                } else {
                    continue;
                }
            };

            if stored != computed {
                failures += 1;
            }
        }
    }
    failures
}

/// Dump subsystem statistics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DumpStats {
    pub total_dumps: u64,
    pub checksum_failures: u64,
    pub dumps_evicted: u64,
    pub active_regions: u32,
    pub storage_used: u32,
    pub storage_total: u32,
}

/// Collects dump statistics.
pub fn get_dump_stats() -> DumpStats {
    DumpStats {
        total_dumps: TOTAL_DUMPS.load(Ordering::Relaxed),
        checksum_failures: CHECKSUM_FAILURES.load(Ordering::Relaxed),
        dumps_evicted: DUMPS_EVICTED.load(Ordering::Relaxed),
        active_regions: DUMP_COUNT.load(Ordering::Relaxed),
        storage_used: STORAGE_USED.load(Ordering::Relaxed),
        storage_total: DUMP_STORAGE_SIZE as u32,
    }
}

/// Resets the memory dump subsystem.
pub fn memory_dump_init() {
    DUMP_COUNT.store(0, Ordering::Release);
    STORAGE_USED.store(0, Ordering::Release);
    TOTAL_DUMPS.store(0, Ordering::Release);
    CHECKSUM_FAILURES.store(0, Ordering::Release);
    DUMPS_EVICTED.store(0, Ordering::Release);
}
