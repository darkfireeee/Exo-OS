// kernel/src/memory/swap/compress.rs
//
// Swap page compression/decompression — zswap light implementation.
//
// Provides an in-RAM cache of compressed pages (zswap pool) to delay
// I/O towards the swap device or avoid them entirely.
//
// Architecture:
//   • `CompressBackend` trait — compression/decompression interface.
//   • `Lz4Lite` — simplified LZ77 algorithm without external dependencies.
//   • `ZswapSlot` — pool entry (compressed data + metadata).
//   • `ZswapPool` — static table of slots, protected by Mutex.
//   • `ZSWAP_POOL` — unique global instance.
//
// The LZ77-lite compression format:
//   Token stream:
//     [0b0_LLLLLLL] literal  : L+1 literal bytes follow (1..128).
//     [0b1_LLLOOO | OO_OOOOOO | OOOOOOOO] match: offset 13 bits, len 3+L bytes (3..10).
//   If output exceeds ZSWAP_SLOT_SIZE -> failure (non-compressible page).
//
// Max size of a compressed slot: ZSWAP_SLOT_SIZE = 3072 bytes (75% of 4 KiB).
// Beyond that, the page is rejected towards the classic swap device.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use lz4_flex::block::{compress_into, decompress_into};

use crate::memory::core::constants::PAGE_SIZE;

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTS
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum accepted compressed buffer size (75% of PAGE_SIZE).
pub const ZSWAP_SLOT_SIZE: usize = 3072;

/// Number of slots in the zswap pool.
pub const MAX_ZSWAP_SLOTS: usize = 4096;

/// Validation hash size (CRC32-like XOR fold).
#[allow(dead_code)]
const CHECKSUM_SIZE: usize = 4;

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT COMPRESSBACKEND
// ─────────────────────────────────────────────────────────────────────────────

/// Compression/decompression interface.
pub trait CompressBackend {
    /// Compresses `src` (PAGE_SIZE bytes) into `dst`.
    /// Returns `Some(len)` if successful, `None` if the page is not compressible.
    fn compress(src: &[u8], dst: &mut [u8]) -> Option<usize>;

    /// Decompresses `src` (compressed data) into `dst` (PAGE_SIZE bytes).
    /// Returns `true` if successful.
    fn decompress(src: &[u8], dst: &mut [u8]) -> bool;
}

// ─────────────────────────────────────────────────────────────────────────────
// LZ4LITE — via crate lz4_flex (block mode, no_std)
// ─────────────────────────────────────────────────────────────────────────────
//
// CRYPTO-CRATES RULE: NEVER implement from scratch.
// Crate: lz4_flex v0.11.x, default-features = false
//   - LZ4 block format pure Rust, no_std + alloc
//   - Compliant with official LZ4 block format

pub struct Lz4Lite;

impl CompressBackend for Lz4Lite {
    /// Compresses `src` (PAGE_SIZE bytes) into `dst` via lz4_flex.
    /// Returns `Some(len)` if successful, `None` if the page is not compressible.
    fn compress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
        debug_assert_eq!(src.len(), PAGE_SIZE);
        compress_into(src, dst).ok()
    }

    /// Decompresses `src` towards `dst` (PAGE_SIZE bytes) via lz4_flex.
    /// Returns `true` if decompression filled exactly PAGE_SIZE.
    fn decompress(src: &[u8], dst: &mut [u8]) -> bool {
        debug_assert_eq!(dst.len(), PAGE_SIZE);
        match decompress_into(src, dst) {
            Ok(n) => n == PAGE_SIZE,
            Err(_) => false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ZSWAP SLOTS
// ─────────────────────────────────────────────────────────────────────────────

/// A slot in the zswap pool. Fixed size = ZSWAP_SLOT_SIZE + metadata.
pub struct ZswapSlot {
    /// Compressed data.
    data: [u8; ZSWAP_SLOT_SIZE],
    /// Actual length of compressed data (0 = free).
    compressed_len: u16,
    /// Origin PFN of the page (identifier).
    pub orig_pfn: u64,
    /// Is this slot occupied?
    pub valid: AtomicBool,
    /// XOR-fold CRC16 of decompressed data for validation.
    checksum: u16,
}

impl ZswapSlot {
    const fn new() -> Self {
        ZswapSlot {
            data: [0u8; ZSWAP_SLOT_SIZE],
            compressed_len: 0,
            orig_pfn: 0,
            valid: AtomicBool::new(false),
            checksum: 0,
        }
    }
}

/// Calculates a simple CRC16 XOR-fold on `data`.
fn xor_checksum(data: &[u8]) -> u16 {
    let mut acc = 0xFFFFu16;
    for &b in data {
        acc ^= b as u16;
        acc = acc.rotate_left(5);
    }
    acc
}

// ─────────────────────────────────────────────────────────────────────────────
// ZSWAP POOL
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a compression/storage attempt.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ZswapStoreResult {
    /// Stored successfully, slot index returned.
    Stored(usize),
    /// Non-compressible page (ratio > 75%).
    NotCompressible,
    /// Pool full.
    PoolFull,
}

/// Result of a retrieval from the zswap pool.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ZswapLoadResult {
    /// Decompressed successfully into the provided buffer.
    Ok,
    /// Slot not found for this PFN.
    NotFound,
    /// Corrupted data (checksum mismatch).
    Corrupt,
    /// Decompression error.
    DecompressError,
}

#[allow(dead_code)]
struct ZswapPoolInner {
    slots: &'static mut [ZswapSlot],
    count: usize,
}

/// Pool of compressed pages in RAM.
pub struct ZswapPool {
    inner: Mutex<()>,
    /// Statistics.
    pub stored: AtomicU64,
    pub evicted: AtomicU64,
    pub decomp_ok: AtomicU64,
    pub decomp_fail: AtomicU64,
    pub compress_fail: AtomicU64,
}

// Global static array of slots.
static mut ZSWAP_SLOTS: [ZswapSlot; MAX_ZSWAP_SLOTS] =
    [const { ZswapSlot::new() }; MAX_ZSWAP_SLOTS];

#[inline(always)]
fn zswap_slots_mut() -> &'static mut [ZswapSlot] {
    // SAFETY: all callers hold `ZswapPool::inner`, so mutable access is serialized.
    unsafe {
        core::slice::from_raw_parts_mut(
            core::ptr::addr_of_mut!(ZSWAP_SLOTS).cast::<ZswapSlot>(),
            MAX_ZSWAP_SLOTS,
        )
    }
}

impl Default for ZswapPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ZswapPool {
    const fn new() -> Self {
        ZswapPool {
            inner: Mutex::new(()),
            stored: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            decomp_ok: AtomicU64::new(0),
            decomp_fail: AtomicU64::new(0),
            compress_fail: AtomicU64::new(0),
        }
    }

    /// Compresses and stores the page identified by `pfn`.
    /// `page_data` must be exactly PAGE_SIZE bytes.
    pub fn store(&self, pfn: u64, page_data: &[u8]) -> ZswapStoreResult {
        debug_assert_eq!(page_data.len(), PAGE_SIZE);

        // Compress into a temporary buffer on the stack.
        let mut tmp = [0u8; ZSWAP_SLOT_SIZE];
        let compressed_len = match Lz4Lite::compress(page_data, &mut tmp) {
            Some(n) => n,
            None => {
                self.compress_fail.fetch_add(1, Ordering::Relaxed);
                return ZswapStoreResult::NotCompressible;
            }
        };

        if compressed_len > ZSWAP_SLOT_SIZE {
            self.compress_fail.fetch_add(1, Ordering::Relaxed);
            return ZswapStoreResult::NotCompressible;
        }

        let csum = xor_checksum(page_data);
        let _guard = self.inner.lock();

        // Find a free slot (linear scan).
        let slots = zswap_slots_mut();
        for (idx, slot) in slots.iter_mut().enumerate() {
            if !slot.valid.load(Ordering::Acquire) {
                slot.data[..compressed_len].copy_from_slice(&tmp[..compressed_len]);
                slot.compressed_len = compressed_len as u16;
                slot.orig_pfn = pfn;
                slot.checksum = csum;
                slot.valid.store(true, Ordering::Release);
                self.stored.fetch_add(1, Ordering::Relaxed);
                return ZswapStoreResult::Stored(idx);
            }
        }

        ZswapStoreResult::PoolFull
    }

    /// Decompresses page `pfn` from the pool into `out` (PAGE_SIZE bytes).
    /// Frees the slot after retrieval.
    pub fn load(&self, pfn: u64, out: &mut [u8]) -> ZswapLoadResult {
        debug_assert_eq!(out.len(), PAGE_SIZE);
        let _guard = self.inner.lock();
        let slots = zswap_slots_mut();

        for slot in slots.iter_mut() {
            if slot.valid.load(Ordering::Acquire) && slot.orig_pfn == pfn {
                let clen = slot.compressed_len as usize;
                let ok = Lz4Lite::decompress(&slot.data[..clen], out);
                if !ok {
                    self.decomp_fail.fetch_add(1, Ordering::Relaxed);
                    return ZswapLoadResult::DecompressError;
                }
                let csum = xor_checksum(out);
                if csum != slot.checksum {
                    self.decomp_fail.fetch_add(1, Ordering::Relaxed);
                    return ZswapLoadResult::Corrupt;
                }
                // Free the slot.
                slot.valid.store(false, Ordering::Release);
                slot.compressed_len = 0;
                self.decomp_ok.fetch_add(1, Ordering::Relaxed);
                self.evicted.fetch_add(1, Ordering::Relaxed);
                return ZswapLoadResult::Ok;
            }
        }
        ZswapLoadResult::NotFound
    }

    /// Invalidates (evicts) the slot associated with `pfn` without decompressing.
    pub fn evict(&self, pfn: u64) -> bool {
        let _guard = self.inner.lock();
        let slots = zswap_slots_mut();
        for slot in slots.iter_mut() {
            if slot.valid.load(Ordering::Acquire) && slot.orig_pfn == pfn {
                slot.valid.store(false, Ordering::Release);
                slot.compressed_len = 0;
                self.evicted.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Number of occupied slots.
    pub fn occupancy(&self) -> usize {
        let _guard = self.inner.lock();
        let slots = zswap_slots_mut();
        slots
            .iter()
            .filter(|s| s.valid.load(Ordering::Relaxed))
            .count()
    }
}

/// Global instance of the zswap pool.
pub static ZSWAP_POOL: ZswapPool = ZswapPool::new();
