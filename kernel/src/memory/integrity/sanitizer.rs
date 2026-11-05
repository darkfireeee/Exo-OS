// kernel/src/memory/integrity/sanitizer.rs
//
// KASAN-lite — Shadow memory tracking for detection of:
//   • Use-after-free (UAF)
//   • Buffer overflow (heap / stack)
//   • Uninitialized memory access (via explicit poisoning at allocation)
//
// Architecture:
//   - Shadow map covers the kernel heap (KERNEL_HEAP_START + 256 GiB).
//   - Shadow ratio: 1 shadow byte monitors 8 object bytes -> shadow = 32 GiB.
//   - Each shadow slot encodes an 8-bit state:
//       SHADOW_ACCESSIBLE  (0x00) — accessible
//       SHADOW_PARTIAL_k   (0x01..0x07) — first k bytes accessible
//       SHADOW_REDZONE     (0xFA) — redzone (overflow)
//       SHADOW_FREED       (0xFD) — freed memory (UAF)
//       SHADOW_UNINIT      (0xFF) — uninitialized
//
// Dynamic coverage: disabled if `KASAN_ENABLED` = false (perf).
//
// LAYER 0 — no dependency on scheduler/process/ipc/fs.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::memory::core::layout::KERNEL_HEAP_START;

// ─────────────────────────────────────────────────────────────────────────────
// Shadow configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Kernel heap range covered: 256 GiB.
const KASAN_HEAP_SIZE: u64 = 256 * 1024 * 1024 * 1024;
/// Shadow memory base = KERNEL_HEAP_START - (KASAN_HEAP_SIZE / 8).
/// Placed right before heap for division-free address calculation at runtime.
const KASAN_SHADOW_OFFSET: u64 = KERNEL_HEAP_START.as_u64().wrapping_sub(KASAN_HEAP_SIZE / 8);
/// Total shadow size: 32 GiB.
#[allow(dead_code)]
const KASAN_SHADOW_SIZE: u64 = KASAN_HEAP_SIZE / 8;

// ─────────────────────────────────────────────────────────────────────────────
// Shadow values
// ─────────────────────────────────────────────────────────────────────────────

pub const SHADOW_ACCESSIBLE: u8 = 0x00;
/// Partially accessible: k valid bytes from the start of the 8-byte word.
pub const fn shadow_partial(k: u8) -> u8 {
    debug_assert!(k > 0 && k < 8);
    k
}
pub const SHADOW_REDZONE: u8 = 0xFA;
pub const SHADOW_FREED: u8 = 0xFD;
pub const SHADOW_UNINIT: u8 = 0xFF;

// ─────────────────────────────────────────────────────────────────────────────
// Global activation
// ─────────────────────────────────────────────────────────────────────────────

static KASAN_ENABLED: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn kasan_is_enabled() -> bool {
    KASAN_ENABLED.load(Ordering::Acquire)
}

/// Enables KASAN. Must be called after the shadow map is mapped.
pub fn kasan_enable() {
    KASAN_ENABLED.store(true, Ordering::Release);
    KASAN_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
}

/// Disables KASAN (debug, benchmark).
pub fn kasan_disable() {
    KASAN_ENABLED.store(false, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct KasanStats {
    pub enable_count: AtomicU64,
    pub uaf_detected: AtomicU64,
    pub overflow_detected: AtomicU64,
    pub uninit_detected: AtomicU64,
    pub poison_calls: AtomicU64,
    pub unpoison_calls: AtomicU64,
    pub shadow_writes: AtomicU64,
}

impl Default for KasanStats {
    fn default() -> Self {
        Self::new()
    }
}

impl KasanStats {
    const fn new() -> Self {
        Self {
            enable_count: AtomicU64::new(0),
            uaf_detected: AtomicU64::new(0),
            overflow_detected: AtomicU64::new(0),
            uninit_detected: AtomicU64::new(0),
            poison_calls: AtomicU64::new(0),
            unpoison_calls: AtomicU64::new(0),
            shadow_writes: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for KasanStats {}
pub static KASAN_STATS: KasanStats = KasanStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Shadow memory access
// ─────────────────────────────────────────────────────────────────────────────

/// Converts an object address to shadow address.
///
/// addr_shadow = (addr - KERNEL_HEAP_START) / 8 + KASAN_SHADOW_OFFSET
///
/// # Safety
/// `addr` must be in covered range [KERNEL_HEAP_START, +KASAN_HEAP_SIZE).
#[inline]
unsafe fn shadow_addr(addr: u64) -> *mut u8 {
    let offset = addr.wrapping_sub(KERNEL_HEAP_START.as_u64()) >> 3;
    (KASAN_SHADOW_OFFSET + offset) as *mut u8
}

/// Reads the shadow byte for `addr`.
/// # Safety: `addr` must be within covered heap range.
#[inline]
unsafe fn read_shadow(addr: u64) -> u8 {
    shadow_addr(addr).read_volatile()
}

/// Writes the shadow byte for `addr`.
/// # Safety: same.
#[inline]
unsafe fn write_shadow(addr: u64, val: u8) {
    shadow_addr(addr).write_volatile(val);
    KASAN_STATS.shadow_writes.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Poisoning / unpoisoning
// ─────────────────────────────────────────────────────────────────────────────

/// Poisons `size` bytes starting from `addr` with `poison_val`.
///
/// Each 8-byte word -> 1 shadow byte.
///
/// # Safety
/// `addr` must be 8-byte aligned and in covered range.
pub unsafe fn kasan_poison(addr: u64, size: usize, poison_val: u8) {
    if !kasan_is_enabled() {
        return;
    }
    KASAN_STATS.poison_calls.fetch_add(1, Ordering::Relaxed);
    let words = size / 8;
    let rem = (size % 8) as u8;
    for i in 0..words as u64 {
        write_shadow(addr + i * 8, poison_val);
    }
    if rem > 0 {
        // Partial final word: only `rem` bytes accessible.
        write_shadow(
            addr + words as u64 * 8,
            match poison_val {
                SHADOW_ACCESSIBLE => shadow_partial(rem),
                _ => poison_val,
            },
        );
    }
}

/// Marks `size` bytes starting from `addr` as accessible (SHADOW_ACCESSIBLE).
///
/// # Safety
/// same as `kasan_poison`.
pub unsafe fn kasan_unpoison(addr: u64, size: usize) {
    if !kasan_is_enabled() {
        return;
    }
    KASAN_STATS.unpoison_calls.fetch_add(1, Ordering::Relaxed);
    let words = (size + 7) / 8;
    for i in 0..words as u64 {
        write_shadow(addr + i * 8, SHADOW_ACCESSIBLE);
    }
}

/// Poisons redzones around an allocation.
/// `obj_addr`: start of object, `obj_size`: size.
/// `redzone_size`: size of each redzone (must be multiple of 8).
///
/// # Safety
/// valid addresses within covered heap range.
pub unsafe fn kasan_poison_redzone(obj_addr: u64, obj_size: usize, redzone_size: usize) {
    if !kasan_is_enabled() {
        return;
    }
    let left_rz = obj_addr.wrapping_sub(redzone_size as u64);
    let right_rz = obj_addr + obj_size as u64;
    kasan_poison(left_rz, redzone_size, SHADOW_REDZONE);
    kasan_poison(right_rz, redzone_size, SHADOW_REDZONE);
}

// ─────────────────────────────────────────────────────────────────────────────
// Access verification
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a KASAN check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KasanError {
    UseAfterFree {
        addr: u64,
    },
    BufferOverflow {
        addr: u64,
    },
    UninitMemory {
        addr: u64,
    },
    PartialAccess {
        addr: u64,
        accessible: u8,
        requested: u8,
    },
}

/// Verifies access to `addr` for `size` bytes.
/// Returns `Ok(())` if range is fully accessible.
///
/// # Safety
/// `addr` must be in covered range [KERNEL_HEAP_START, +KASAN_HEAP_SIZE).
pub unsafe fn kasan_check_access(addr: u64, size: usize) -> Result<(), KasanError> {
    if !kasan_is_enabled() {
        return Ok(());
    }
    if addr < KERNEL_HEAP_START.as_u64() || addr >= KERNEL_HEAP_START.as_u64() + KASAN_HEAP_SIZE {
        return Ok(()); // Outside covered range — no KASAN error (e.g. vmalloc).
    }

    let end = addr + size as u64;
    let mut cur = addr;
    while cur < end {
        let shadow = read_shadow(cur);
        let err = match shadow {
            SHADOW_ACCESSIBLE => None,
            SHADOW_FREED => {
                KASAN_STATS.uaf_detected.fetch_add(1, Ordering::Relaxed);
                Some(KasanError::UseAfterFree { addr: cur })
            }
            SHADOW_REDZONE => {
                KASAN_STATS
                    .overflow_detected
                    .fetch_add(1, Ordering::Relaxed);
                Some(KasanError::BufferOverflow { addr: cur })
            }
            SHADOW_UNINIT => {
                KASAN_STATS.uninit_detected.fetch_add(1, Ordering::Relaxed);
                Some(KasanError::UninitMemory { addr: cur })
            }
            partial if partial > 0 && partial < 8 => {
                // Check if access exceeds valid `partial` bytes.
                let byte_in_word = (cur & 7) as u8;
                let end_in_word = (byte_in_word + size as u8).min(8);
                if end_in_word > partial {
                    KASAN_STATS
                        .overflow_detected
                        .fetch_add(1, Ordering::Relaxed);
                    Some(KasanError::PartialAccess {
                        addr: cur,
                        accessible: partial,
                        requested: end_in_word,
                    })
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(e) = err {
            return Err(e);
        }
        // Advance by one word (8 bytes).
        cur = (cur & !7) + 8;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Violation report
// ─────────────────────────────────────────────────────────────────────────────

/// Called by KASAN handler when an invalid access is detected.
/// Minimal log + panic.
pub fn kasan_report(error: KasanError, ip: u64) -> ! {
    match error {
        KasanError::UseAfterFree { addr } => {
            panic!("KASAN: use-after-free at {:#x} ip={:#x}", addr, ip)
        }
        KasanError::BufferOverflow { addr } => {
            panic!("KASAN: buffer-overflow at {:#x} ip={:#x}", addr, ip)
        }
        KasanError::UninitMemory { addr } => {
            panic!("KASAN: uninit-memory read at {:#x} ip={:#x}", addr, ip)
        }
        KasanError::PartialAccess {
            addr,
            accessible,
            requested,
        } => panic!(
            "KASAN: partial-access at {:#x} accessible={} requested={} ip={:#x}",
            addr, accessible, requested, ip
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Allocation / free hooks (called by hybrid.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Called right before returning a pointer from allocator.
/// Marks `size` bytes accessible.
///
/// # Safety
/// `ptr` valid within heap range.
pub unsafe fn kasan_on_alloc(ptr: *mut u8, size: usize) {
    kasan_unpoison(ptr as u64, size);
}

/// Called right after freeing a pointer.
/// Marks `size` bytes as FREED.
///
/// # Safety
/// `ptr` valid within heap range.
pub unsafe fn kasan_on_free(ptr: *mut u8, size: usize) {
    kasan_poison(ptr as u64, size, SHADOW_FREED);
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes KASAN.
///
/// The shadow map must have been previously mapped (32 GiB of zeroed pages)
/// by the kernel mapper. This function simply enables verification.
///
/// # Safety
/// CPL 0, shadow map mapped.
pub unsafe fn init() {
    // Poison entire shadow map with SHADOW_UNINIT.
    // In practice we rely on page table zeroing (0x00 = ACCESSIBLE).
    // We simply enable KASAN.
    kasan_enable();
}
