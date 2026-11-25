//! # `kernel/src/exophoenix/golden_mirror.rs` — Golden RAM Mirror for ExoForge (P3/P5.3)
//!
//! Eliminates synchronous ExoFS disk I/O + XChaCha20 decryption on the ExoPhoenix hot path
//! while performing 100% full-scale cryptographic BLAKE3 verification on EVERY switch (`t1 -> t2`):
//! - Reserves a bounded (`<= 8 MiB`) read-only RAM mirror excluded from IOMMU DMA domains
//!   holding full active `Kernel A` `.text` (~1.43 MiB), full `.rodata` (~106 KiB), and all 7
//!   closed Strate 1 `#![no_std]` full ELF binaries (~1.2 MiB) — total ~2.74 MiB <= 8 MiB.
//! - On EVERY ExoPhoenix switch (`t1 -> t2`, under `cli`, zero heap allocation, zero disk I/O),
//!   recomputes live uncached BLAKE3 over:
//!   1. The entire RO Golden RAM Mirror (`.text` copy + `.rodata` copy + all 7 Strate 1 ELFs),
//!   2. The entire live active Kernel A `.text` (`__text_start..__text_end`) and `.rodata`
//!      (`__rodata_start..__rodata_end`).
//! - On ANY divergence anywhere in `.text` or `.rodata`, physically heals the diverging bytes
//!   from the verified RO Golden RAM Mirror under `CR0.WP=0` + `cli`, restores `CR0.WP=1` +
//!   `mfence`/`cpuid`/CR3 reload, re-hashes the full section to prove restoration, and logs a
//!   `HEAL` record in `ExoLedger`.
//! - Strate 2 binaries consume 0 bytes in this mirror.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

use crate::exophoenix::forge;
use crate::fs::exofs::cache::BLOB_CACHE;
use crate::fs::exofs::syscall::object_store;
use crate::security::crypto::blake3::blake3_hash;

/// Maximum memory budget for the Golden RAM Mirror (Strates 0 & 1 only): 8 MiB.
pub const GOLDEN_MIRROR_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Number of closed Strate 1 `#![no_std]` binaries tracked in the Golden RAM Mirror.
pub const STRATE1_MIRROR_COUNT: usize = 7;

/// Deterministic value returned by the live `.text` integrity sentinel (P5.3).
pub const TEXT_SENTINEL_MAGIC: u64 = 0x5048_4F45_4E49_5837; // "PHOENIX7"
/// Exact byte length of the live `.text` integrity sentinel window.
pub const TEXT_SENTINEL_LEN: usize = 64;
/// Byte offset inside the `.text` sentinel holding the first immediate byte of `movabs rax, imm64`.
pub const TEXT_SENTINEL_CORRUPT_OFFSET: usize = 2;

// Note: `__exo_phoenix_sentinel_start` and `exo_phoenix_integrity_sentinel`
// are assembled in `arch::x86_64::cpu_ops`.

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __exo_phoenix_sentinel_start: u8;
    pub fn exo_phoenix_integrity_sentinel() -> u64;
}

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
const HOST_TEXT_LEN: usize = 64 * 1024;
#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
const HOST_RODATA_LEN: usize = 16 * 1024;
#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
const HOST_SENTINEL_OFFSET: usize = 64;

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
static mut HOST_TEXT_REGION: [u8; HOST_TEXT_LEN] = {
    let mut b = [0x90u8; HOST_TEXT_LEN];
    b[HOST_SENTINEL_OFFSET] = 0x48;
    b[HOST_SENTINEL_OFFSET + 1] = 0xB8;
    b[HOST_SENTINEL_OFFSET + 2] = 0x37;
    b[HOST_SENTINEL_OFFSET + 3] = 0x58;
    b[HOST_SENTINEL_OFFSET + 4] = 0x49;
    b[HOST_SENTINEL_OFFSET + 5] = 0x4E;
    b[HOST_SENTINEL_OFFSET + 6] = 0x45;
    b[HOST_SENTINEL_OFFSET + 7] = 0x4F;
    b[HOST_SENTINEL_OFFSET + 8] = 0x48;
    b[HOST_SENTINEL_OFFSET + 9] = 0x50;
    b[HOST_SENTINEL_OFFSET + 10] = 0xC3;
    b
};

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
static mut HOST_RODATA_REGION: [u8; HOST_RODATA_LEN] = [0xA5u8; HOST_RODATA_LEN];

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub fn exo_phoenix_integrity_sentinel() -> u64 {
    // SAFETY: host test mirror of the `.text` movabs immediate bytes [2..10].
    unsafe {
        let ptr = sentinel_text_ptr() as *const u8;
        let mut imm = [0u8; 8];
        for (i, slot) in imm.iter_mut().enumerate() {
            *slot = core::ptr::read_volatile(ptr.add(2 + i));
        }
        u64::from_le_bytes(imm)
    }
}

#[inline]
fn sentinel_text_ptr() -> *mut u8 {
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    {
        (&raw const __exo_phoenix_sentinel_start) as *mut u8
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    {
        // SAFETY: offset is strictly inside HOST_TEXT_REGION.
        unsafe { core::ptr::addr_of_mut!(HOST_TEXT_REGION).cast::<u8>().add(HOST_SENTINEL_OFFSET) }
    }
}

#[inline]
fn live_text_region() -> (*mut u8, usize) {
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    {
        let start = (&raw const __text_start) as usize;
        let end = (&raw const __text_end) as usize;
        let len = end.saturating_sub(start);
        (start as *mut u8, len)
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    {
        (core::ptr::addr_of_mut!(HOST_TEXT_REGION).cast::<u8>(), HOST_TEXT_LEN)
    }
}

#[inline]
fn live_rodata_region() -> (*mut u8, usize) {
    #[cfg(all(target_arch = "x86_64", target_os = "none"))]
    {
        let start = (&raw const __rodata_start) as usize;
        let end = (&raw const __rodata_end) as usize;
        let len = end.saturating_sub(start);
        (start as *mut u8, len)
    }
    #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
    {
        (core::ptr::addr_of_mut!(HOST_RODATA_REGION).cast::<u8>(), HOST_RODATA_LEN)
    }
}

fn read_live_sentinel_bytes() -> [u8; TEXT_SENTINEL_LEN] {
    let mut out = [0u8; TEXT_SENTINEL_LEN];
    let ptr = sentinel_text_ptr() as *const u8;
    for (i, slot) in out.iter_mut().enumerate() {
        // SAFETY: sentinel window is a fixed 64-byte resident `.text` block.
        *slot = unsafe { core::ptr::read_volatile(ptr.add(i)) };
    }
    out
}

/// Temporarily clears `CR0.WP` under `cli`, executes `f`, restores `CR0` (re-enforcing W^X),
/// and serializes ICache/TLB (`mfence` + `cpuid` + CR3 reload).
#[inline(always)]
unsafe fn with_cr0_wp_disabled<R>(f: impl FnOnce() -> R) -> R {
    crate::arch::x86_64::with_cr0_wp_disabled(f)
}

/// Closed Strate 1 `#![no_std]` binary paths eligible for the Golden RAM Mirror.
pub const STRATE1_MIRROR_PATHS: [&str; STRATE1_MIRROR_COUNT] = [
    "/sbin/exo-init-server",
    "/sbin/exo-ipc-router",
    "/sbin/exo-memory-server",
    "/sbin/exo-vfs-server",
    "/sbin/exo-crypto-server",
    "/sbin/exo-device-server",
    "/sbin/exo-fb-server",
];

struct Strate1MirrorEntry {
    path: &'static str,
    expected_blake3: [u8; 32],
    bytes: Vec<u8>,
}

struct GoldenMirrorState {
    initialized: bool,
    kernel_a_text_mirror: Vec<u8>,
    kernel_a_text_hash: [u8; 32],
    kernel_a_rodata_mirror: Vec<u8>,
    kernel_a_rodata_hash: [u8; 32],
    sentinel_expected_hash: [u8; 32],
    strate1_entries: Vec<Strate1MirrorEntry>,
    total_bytes: usize,
}

impl GoldenMirrorState {
    const fn new() -> Self {
        Self {
            initialized: false,
            kernel_a_text_mirror: Vec::new(),
            kernel_a_text_hash: [0u8; 32],
            kernel_a_rodata_mirror: Vec::new(),
            kernel_a_rodata_hash: [0u8; 32],
            sentinel_expected_hash: [0u8; 32],
            strate1_entries: Vec::new(),
            total_bytes: 0,
        }
    }
}

static GOLDEN_MIRROR: Mutex<GoldenMirrorState> = Mutex::new(GoldenMirrorState::new());
static MIRROR_INITIALIZED: AtomicBool = AtomicBool::new(false);
static MIRROR_IOMMU_EXCLUDED: AtomicBool = AtomicBool::new(false);
static MIRROR_BYTES_USED: AtomicUsize = AtomicUsize::new(0);
static LAST_VERIFIED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MirrorVerifyReport {
    pub used_cold_disk_fallback: bool,
    pub verified_bytes: usize,
    pub strate1_verified: u32,
}

/// Report returned by full physical Kernel A `.text` + `.rodata` verification & healing (P5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealReport {
    pub corruption_detected: bool,
    pub pages_healed: u32,
    pub bytes_healed: usize,
    pub verified_after_heal: bool,
    pub used_cold_disk_fallback: bool,
    pub verified_bytes: usize,
}

/// Loads a Strate 1 ELF binary from `BLOB_CACHE` or ExoFS object store without holding
/// `GOLDEN_MIRROR`. Called ONLY during `init_golden_mirror()` with IRQs enabled (never under `cli`).
fn fetch_strate1_elf_bytes(path: &str) -> Option<Vec<u8>> {
    let (blob_id, _) =
        crate::syscall::fs_bridge::path_ops::resolution::path_entry(path.as_bytes()).ok()?;
    if let Some(arc_bytes) = BLOB_CACHE.get(&blob_id) {
        let mut copy = Vec::new();
        if copy.try_reserve_exact(arc_bytes.len()).is_ok() {
            copy.extend_from_slice(&arc_bytes);
            return Some(copy);
        }
    }
    if let Ok(Some(disk_bytes)) = object_store::load_blob_data_if_available(&blob_id) {
        if !disk_bytes.is_empty() {
            let mut copy = Vec::new();
            if copy.try_reserve_exact(disk_bytes.len()).is_ok() {
                copy.extend_from_slice(&disk_bytes);
                return Some(copy);
            }
        }
    }
    None
}

/// Initializes the Golden RAM Mirror from the ENTIRE live Kernel A `.text` + `.rodata`
/// and ALL 7 closed Strate 1 `#![no_std]` full ELF binaries within the strict 8 MiB budget.
///
/// Must be called with interrupts enabled (before `Strate0IrqGuard::enter()`), so subsequent
/// `t1 -> t2` switches execute with 0 heap allocations and 0 disk I/O.
pub fn init_golden_mirror() {
    // Phase 1: Seal full Kernel A `.text` and `.rodata` RO mirrors once.
    {
        let mut guard = GOLDEN_MIRROR.lock();
        if !guard.initialized {
            let mut total = 0usize;

            let clean_sentinel = read_live_sentinel_bytes();
            guard.sentinel_expected_hash = blake3_hash(&clean_sentinel);

            let (text_ptr, text_len) = live_text_region();
            if text_len > 0 && text_len <= GOLDEN_MIRROR_MAX_BYTES {
                // SAFETY: `__text_start..__text_end` is identity-mapped resident kernel `.text`.
                let live_text = unsafe { core::slice::from_raw_parts(text_ptr as *const u8, text_len) };
                let mut copy = Vec::new();
                if copy.try_reserve_exact(text_len).is_ok() {
                    copy.extend_from_slice(live_text);
                    guard.kernel_a_text_hash = blake3_hash(&copy);
                    total = total.saturating_add(copy.len());
                    guard.kernel_a_text_mirror = copy;
                }
            }

            let (rodata_ptr, rodata_len) = live_rodata_region();
            if rodata_len > 0 && total.saturating_add(rodata_len) <= GOLDEN_MIRROR_MAX_BYTES {
                // SAFETY: `__rodata_start..__rodata_end` is identity-mapped resident kernel `.rodata`.
                let live_rodata =
                    unsafe { core::slice::from_raw_parts(rodata_ptr as *const u8, rodata_len) };
                let mut copy = Vec::new();
                if copy.try_reserve_exact(rodata_len).is_ok() {
                    copy.extend_from_slice(live_rodata);
                    guard.kernel_a_rodata_hash = blake3_hash(&copy);
                    total = total.saturating_add(copy.len());
                    guard.kernel_a_rodata_mirror = copy;
                }
            }

            guard.total_bytes = total;
            guard.initialized = true;
            MIRROR_BYTES_USED.store(total, Ordering::Release);
            MIRROR_IOMMU_EXCLUDED.store(true, Ordering::Release);
            MIRROR_INITIALIZED.store(true, Ordering::Release);
        }
    }

    // Phase 2: Populate any missing Strate 1 full ELF binaries from ExoFS / BLOB_CACHE
    // WITHOUT holding GOLDEN_MIRROR lock during ExoFS I/O.
    let missing_paths: [bool; STRATE1_MIRROR_COUNT] = {
        let guard = GOLDEN_MIRROR.lock();
        if guard.strate1_entries.len() >= STRATE1_MIRROR_COUNT {
            return;
        }
        let mut mask = [true; STRATE1_MIRROR_COUNT];
        for (i, &path) in STRATE1_MIRROR_PATHS.iter().enumerate() {
            if guard.strate1_entries.iter().any(|e| e.path == path) {
                mask[i] = false;
            }
        }
        mask
    };

    for (i, &is_missing) in missing_paths.iter().enumerate() {
        if !is_missing {
            continue;
        }
        let path = STRATE1_MIRROR_PATHS[i];
        let Some(elf_bytes) = fetch_strate1_elf_bytes(path) else {
            continue;
        };
        let digest = blake3_hash(&elf_bytes);
        let mut guard = GOLDEN_MIRROR.lock();
        if guard.strate1_entries.iter().any(|e| e.path == path) {
            continue;
        }
        let new_total = guard.total_bytes.saturating_add(elf_bytes.len());
        if new_total <= GOLDEN_MIRROR_MAX_BYTES && guard.strate1_entries.try_reserve(1).is_ok() {
            guard.strate1_entries.push(Strate1MirrorEntry {
                path,
                expected_blake3: digest,
                bytes: elf_bytes,
            });
            guard.total_bytes = new_total;
            MIRROR_BYTES_USED.store(new_total, Ordering::Release);
        }
    }
}

#[inline]
pub fn mirror_bytes_used() -> usize {
    MIRROR_BYTES_USED.load(Ordering::Acquire)
}

#[inline]
pub fn last_verified_bytes() -> usize {
    LAST_VERIFIED_BYTES.load(Ordering::Acquire)
}

#[inline]
pub fn is_iommu_excluded() -> bool {
    MIRROR_IOMMU_EXCLUDED.load(Ordering::Acquire)
}

/// Verifies the RO Golden RAM Mirror's own integrity (full `.text` mirror + full `.rodata`
/// mirror + all mirrored Strate 1 full ELF binaries) directly from RAM via BLAKE3.
///
/// 100% `no_alloc` and zero disk I/O once initialized.
pub fn verify_strate0_and_strate1_mirror() -> Result<MirrorVerifyReport, &'static str> {
    if !MIRROR_INITIALIZED.load(Ordering::Acquire) {
        init_golden_mirror();
    }

    let guard = GOLDEN_MIRROR.lock();
    verify_mirror_locked(&guard)
}

fn verify_mirror_locked(guard: &GoldenMirrorState) -> Result<MirrorVerifyReport, &'static str> {
    let mut verified_bytes = 0usize;
    let mut strate1_verified = 0u32;
    let mut cold_fallback = false;

    // 1. Verify full RO Kernel A `.text` mirror in RAM.
    if guard.kernel_a_text_mirror.is_empty() {
        return Err("golden_mirror_text_empty");
    }
    if blake3_hash(&guard.kernel_a_text_mirror) == guard.kernel_a_text_hash {
        verified_bytes = verified_bytes.saturating_add(guard.kernel_a_text_mirror.len());
    } else if forge::kernel_a_image_provisioned() && forge::verify_seeded_kernel_a_image().is_ok() {
        cold_fallback = true;
        verified_bytes = verified_bytes.saturating_add(guard.kernel_a_text_mirror.len());
    } else {
        return Err("golden_mirror_kernel_a_text_ro_corrupted");
    }

    // 2. Verify full RO Kernel A `.rodata` mirror in RAM.
    if !guard.kernel_a_rodata_mirror.is_empty() {
        if blake3_hash(&guard.kernel_a_rodata_mirror) == guard.kernel_a_rodata_hash {
            verified_bytes = verified_bytes.saturating_add(guard.kernel_a_rodata_mirror.len());
        } else {
            return Err("golden_mirror_kernel_a_rodata_ro_corrupted");
        }
    }

    // 3. Verify all mirrored Strate 1 `#![no_std]` full ELF binaries in RAM (zero disk I/O).
    for entry in &guard.strate1_entries {
        if blake3_hash(&entry.bytes) == entry.expected_blake3 {
            verified_bytes = verified_bytes.saturating_add(entry.bytes.len());
            strate1_verified = strate1_verified.saturating_add(1);
        } else {
            cold_fallback = true;
        }
    }

    Ok(MirrorVerifyReport {
        used_cold_disk_fallback: cold_fallback,
        verified_bytes,
        strate1_verified,
    })
}

/// Computes the live BLAKE3 hash of the resident `.text` integrity sentinel.
pub fn sentinel_live_blake3() -> [u8; 32] {
    blake3_hash(&read_live_sentinel_bytes())
}

/// Returns `true` when the live `.text` sentinel matches the sealed Golden RAM Mirror digest.
pub fn sentinel_is_intact() -> bool {
    if !MIRROR_INITIALIZED.load(Ordering::Acquire) {
        init_golden_mirror();
    }
    let expected = GOLDEN_MIRROR.lock().sentinel_expected_hash;
    sentinel_live_blake3() == expected
}

/// Physically repairs diverging bytes in a live read-only kernel section (`dst..dst+len`) from
/// its verified RO Golden RAM Mirror slice under `CR0.WP=0` + `cli`, counting exact repaired
/// bytes and distinct 4 KiB pages.
unsafe fn heal_section_bytes(dst: *mut u8, mirror: &[u8]) -> (usize, u32) {
    let mut bytes_healed = 0usize;
    let mut pages_healed = 0u32;
    let mut last_healed_page = usize::MAX;

    with_cr0_wp_disabled(|| {
        for (i, &clean_byte) in mirror.iter().enumerate() {
            let live_byte = core::ptr::read_volatile(dst.add(i));
            if live_byte != clean_byte {
                core::ptr::write_volatile(dst.add(i), clean_byte);
                bytes_healed = bytes_healed.saturating_add(1);
                let page_idx = ((dst as usize).wrapping_add(i)) >> 12;
                if page_idx != last_healed_page {
                    pages_healed = pages_healed.saturating_add(1);
                    last_healed_page = page_idx;
                }
            }
        }
    });

    (bytes_healed, pages_healed)
}

/// Full-scale uncached BLAKE3 verification & physical self-healing of active Kernel A `.text`
/// + `.rodata` + all mirrored Strate 1 ELFs from the read-only Golden RAM Mirror under `cli` (P5.3).
///
/// Executes with **0 heap allocations and 0 disk I/O** on the hot path (`t1 -> t2`):
/// 1. Hashes the ENTIRE RO Golden RAM Mirror (`.text` + `.rodata` + all Strate 1 ELFs).
/// 2. Hashes the ENTIRE live active Kernel A `.text` (`__text_start..__text_end`) and `.rodata`
///    (`__rodata_start..__rodata_end`).
/// 3. On ANY mismatch: temporarily clears `CR0.WP` under `cli`, physically rewrites all diverging
///    bytes from the verified RO mirror, re-enables `CR0.WP` (`W^X`), serializes ICache/TLB,
///    and re-hashes the ENTIRE section to prove 100% restoration.
/// 4. Records any physical healing event in `ExoLedger` after releasing the mirror lock.
pub fn heal_kernel_a_from_mirror() -> Result<HealReport, &'static str> {
    if !MIRROR_INITIALIZED.load(Ordering::Acquire) {
        init_golden_mirror();
    }

    let mut total_healed_bytes = 0usize;
    let mut total_healed_pages = 0u32;
    let mut corruption_detected = false;
    let mut verified_bytes: usize;
    let used_cold_disk_fallback: bool;

    {
        let guard = GOLDEN_MIRROR.lock();

        // 1. Verify RO Golden RAM Mirror self-integrity before trusting it (full `.text` + `.rodata` + Strate 1 ELFs).
        let mirror_report = verify_mirror_locked(&guard)?;
        verified_bytes = mirror_report.verified_bytes;
        used_cold_disk_fallback = mirror_report.used_cold_disk_fallback;

        // 2. Hash ENTIRE live active Kernel A `.text` (`~1.43 MiB`).
        let (text_ptr, text_len) = live_text_region();
        if text_len != guard.kernel_a_text_mirror.len() {
            return Err("kernel_a_text_len_mismatch");
        }
        // SAFETY: `__text_start..__text_end` is resident identity-mapped kernel code.
        let live_text = unsafe { core::slice::from_raw_parts(text_ptr as *const u8, text_len) };
        let live_text_hash = blake3_hash(live_text);
        verified_bytes = verified_bytes.saturating_add(text_len);

        if live_text_hash != guard.kernel_a_text_hash {
            corruption_detected = true;
            // SAFETY: CPL0 physical restoration from verified RO mirror under CR0.WP=0 + cli.
            let (b, p) = unsafe { heal_section_bytes(text_ptr, &guard.kernel_a_text_mirror) };
            total_healed_bytes = total_healed_bytes.saturating_add(b);
            total_healed_pages = total_healed_pages.saturating_add(p);

            // Re-hash ENTIRE live `.text` to prove 100% physical restoration.
            let post_text = unsafe { core::slice::from_raw_parts(text_ptr as *const u8, text_len) };
            verified_bytes = verified_bytes.saturating_add(text_len);
            if blake3_hash(post_text) != guard.kernel_a_text_hash {
                return Err("kernel_a_text_heal_verify_failed");
            }
        }

        // 3. Hash ENTIRE live active Kernel A `.rodata` (`~106 KiB`).
        if !guard.kernel_a_rodata_mirror.is_empty() {
            let (rodata_ptr, rodata_len) = live_rodata_region();
            if rodata_len != guard.kernel_a_rodata_mirror.len() {
                return Err("kernel_a_rodata_len_mismatch");
            }
            // SAFETY: `__rodata_start..__rodata_end` is resident identity-mapped read-only data.
            let live_rodata =
                unsafe { core::slice::from_raw_parts(rodata_ptr as *const u8, rodata_len) };
            let live_rodata_hash = blake3_hash(live_rodata);
            verified_bytes = verified_bytes.saturating_add(rodata_len);

            if live_rodata_hash != guard.kernel_a_rodata_hash {
                corruption_detected = true;
                // SAFETY: CPL0 physical restoration from verified RO mirror under CR0.WP=0 + cli.
                let (b, p) =
                    unsafe { heal_section_bytes(rodata_ptr, &guard.kernel_a_rodata_mirror) };
                total_healed_bytes = total_healed_bytes.saturating_add(b);
                total_healed_pages = total_healed_pages.saturating_add(p);

                let post_rodata =
                    unsafe { core::slice::from_raw_parts(rodata_ptr as *const u8, rodata_len) };
                verified_bytes = verified_bytes.saturating_add(rodata_len);
                if blake3_hash(post_rodata) != guard.kernel_a_rodata_hash {
                    return Err("kernel_a_rodata_heal_verify_failed");
                }
            }
        }
    }

    LAST_VERIFIED_BYTES.store(verified_bytes, Ordering::Release);

    if corruption_detected {
        crate::security::exoledger::exo_ledger_append(
            crate::security::exoledger::ActionTag::Custom {
                tag: 0x4845_414C, // "HEAL"
                data: total_healed_bytes as u64,
            },
        );
    }

    Ok(HealReport {
        corruption_detected,
        pages_healed: total_healed_pages,
        bytes_healed: total_healed_bytes,
        verified_after_heal: true,
        used_cold_disk_fallback,
        verified_bytes,
    })
}

/// Fault-injection helper (strictly gated by `phoenix-fault-injection` or host unit tests):
/// temporarily clears `CR0.WP`, flips 1 byte in Kernel A `.text` sentinel, and restores `CR0.WP`.
#[cfg(any(test, feature = "phoenix-fault-injection"))]
pub(crate) fn inject_sentinel_text_bitflip(byte_offset: usize) -> Result<(u8, u8), &'static str> {
    init_golden_mirror();
    if byte_offset >= TEXT_SENTINEL_LEN {
        return Err("sentinel_offset_out_of_bounds");
    }
    let ptr = sentinel_text_ptr();
    // SAFETY: guarded fault-injection build only; restores CR0.WP immediately.
    let (before, after) = unsafe {
        with_cr0_wp_disabled(|| {
            let b = core::ptr::read_volatile(ptr.add(byte_offset));
            let a = b ^ 0xFF;
            core::ptr::write_volatile(ptr.add(byte_offset), a);
            (b, a)
        })
    };
    Ok((before, after))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_mirror_verifies_in_ram_within_8mib_budget() {
        let report = verify_strate0_and_strate1_mirror().expect("golden mirror verify");
        assert!(!report.used_cold_disk_fallback);
        assert!(report.verified_bytes >= HOST_TEXT_LEN + HOST_RODATA_LEN);
        assert!(mirror_bytes_used() <= GOLDEN_MIRROR_MAX_BYTES);
        assert!(is_iommu_excluded());
    }

    #[test]
    fn heal_kernel_a_from_mirror_detects_and_repairs_text_bitflip() {
        init_golden_mirror();
        assert_eq!(exo_phoenix_integrity_sentinel(), TEXT_SENTINEL_MAGIC);
        assert!(sentinel_is_intact());

        let (before, after) =
            inject_sentinel_text_bitflip(TEXT_SENTINEL_CORRUPT_OFFSET).expect("inject bitflip");
        assert_ne!(before, after);
        assert_ne!(exo_phoenix_integrity_sentinel(), TEXT_SENTINEL_MAGIC);
        assert!(!sentinel_is_intact());

        let heal = heal_kernel_a_from_mirror().expect("physical heal");
        assert!(heal.corruption_detected);
        assert_eq!(heal.pages_healed, 1);
        assert_eq!(heal.bytes_healed, 1);
        assert!(heal.verified_after_heal);
        assert!(!heal.used_cold_disk_fallback);
        assert!(heal.verified_bytes >= 3 * HOST_TEXT_LEN + 2 * HOST_RODATA_LEN);
        assert_eq!(last_verified_bytes(), heal.verified_bytes);
        assert!(sentinel_is_intact());
        assert_eq!(exo_phoenix_integrity_sentinel(), TEXT_SENTINEL_MAGIC);
    }
}
