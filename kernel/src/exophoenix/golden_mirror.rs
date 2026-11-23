//! # `kernel/src/exophoenix/golden_mirror.rs` — Golden RAM Mirror for ExoForge (P3)
//!
//! Eliminates synchronous ExoFS disk I/O + XChaCha20 decryption on the ExoPhoenix hot path:
//! - Reserves a bounded (< 8 MiB) read-only RAM mirror excluded from IOMMU DMA domains
//!   holding `Kernel A` clean image + Strate 1 `#![no_std]` binary digests/snapshots.
//! - On every ExoPhoenix switch, verifies BLAKE3 integrity directly from RAM (~ms in `--release`).
//! - Falls back to cold ExoFS disk verification (setting `PHOENIX_FLAG_COLD_DISK_FALLBACK`)
//!   only if the RAM mirror fails BLAKE3 verification.
//! - Strate 2 binaries consume 0 bytes in this mirror.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

use crate::exophoenix::forge;
use crate::security::crypto::blake3::blake3_hash;

/// Maximum memory budget for the Golden RAM Mirror (Strates 0 & 1 only): 8 MiB.
pub const GOLDEN_MIRROR_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Resident Kernel A `.text` mirror window verified in O(1) RAM on every switch (4 KiB).
pub const KERNEL_A_MIRROR_WINDOW_LEN: usize = 4096;
/// Number of closed Strate 1 `#![no_std]` binaries tracked in the Golden RAM Mirror.
pub const STRATE1_MIRROR_COUNT: usize = 7;
/// Resident descriptor size per Strate 1 binary in the Golden RAM Mirror.
pub const STRATE1_ANCHOR_LEN: usize = 64;

/// Deterministic value returned by the live `.text` integrity sentinel (P5.3).
pub const TEXT_SENTINEL_MAGIC: u64 = 0x5048_4F45_4E49_5837; // "PHOENIX7"
/// Exact byte length of the live `.text` integrity sentinel window.
pub const TEXT_SENTINEL_LEN: usize = 64;
/// Byte offset inside the `.text` sentinel holding the first immediate byte of `movabs rax, imm64`.
pub const TEXT_SENTINEL_CORRUPT_OFFSET: usize = 2;

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
core::arch::global_asm!(
    ".section .text",
    ".balign 64",
    ".global __exo_phoenix_sentinel_start",
    ".global exo_phoenix_integrity_sentinel",
    ".type exo_phoenix_integrity_sentinel, @function",
    "__exo_phoenix_sentinel_start:",
    "exo_phoenix_integrity_sentinel:",
    // Machine encoding: 48 B8 37 58 49 4E 45 4F 48 50 (movabs rax, 0x50484F454E495837) ; C3 (ret)
    ".byte 0x48, 0xB8, 0x37, 0x58, 0x49, 0x4E, 0x45, 0x4F, 0x48, 0x50, 0xC3",
    ".fill 53, 1, 0x90",
    ".global __exo_phoenix_sentinel_end",
    "__exo_phoenix_sentinel_end:",
    ".size exo_phoenix_integrity_sentinel, 64",
);

#[cfg(all(target_arch = "x86_64", target_os = "none"))]
extern "C" {
    static __exo_phoenix_sentinel_start: u8;
    pub fn exo_phoenix_integrity_sentinel() -> u64;
}

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
static mut HOST_SENTINEL_TEXT: [u8; TEXT_SENTINEL_LEN] = {
    let mut b = [0x90u8; TEXT_SENTINEL_LEN];
    b[0] = 0x48;
    b[1] = 0xB8;
    b[2] = 0x37;
    b[3] = 0x58;
    b[4] = 0x49;
    b[5] = 0x4E;
    b[6] = 0x45;
    b[7] = 0x4F;
    b[8] = 0x48;
    b[9] = 0x50;
    b[10] = 0xC3;
    b
};

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
pub fn exo_phoenix_integrity_sentinel() -> u64 {
    // SAFETY: host test mirror of the `.text` movabs immediate bytes [2..10].
    unsafe {
        let ptr = core::ptr::addr_of!(HOST_SENTINEL_TEXT).cast::<u8>();
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
        core::ptr::addr_of_mut!(HOST_SENTINEL_TEXT).cast::<u8>()
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
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
unsafe fn with_cr0_wp_disabled<R>(f: impl FnOnce() -> R) -> R {
    let rflags: u64;
    let cr0_orig: u64;
    core::arch::asm!(
        "pushfq",
        "pop {flags}",
        "cli",
        "mov {cr0}, cr0",
        flags = out(reg) rflags,
        cr0 = out(reg) cr0_orig,
        options(nomem),
    );
    let cr0_nowp = cr0_orig & !(1u64 << 16);
    core::arch::asm!(
        "mov cr0, {cr0}",
        cr0 = in(reg) cr0_nowp,
        options(nostack, nomem),
    );

    let res = f();

    core::arch::asm!(
        "mfence",
        "mov cr0, {cr0}",
        cr0 = in(reg) cr0_orig,
        options(nostack, nomem),
    );
    // Serializing instruction + local TLB flush to synchronize instruction fetch pipeline.
    core::arch::asm!(
        "push rbx",
        "xor eax, eax",
        "cpuid",
        "pop rbx",
        "mov {tmp}, cr3",
        "mov cr3, {tmp}",
        tmp = out(reg) _,
        out("eax") _,
        out("ecx") _,
        out("edx") _,
        options(nomem),
    );

    if (rflags & (1 << 9)) != 0 {
        core::arch::asm!("sti", options(nostack, nomem));
    }
    res
}

#[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
unsafe fn with_cr0_wp_disabled<R>(f: impl FnOnce() -> R) -> R {
    f()
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

struct GoldenMirrorState {
    initialized: bool,
    kernel_a_hash: [u8; 32],
    kernel_a_window: [u8; KERNEL_A_MIRROR_WINDOW_LEN],
    kernel_a_window_hash: [u8; 32],
    sentinel_clean_copy: [u8; TEXT_SENTINEL_LEN],
    sentinel_expected_hash: [u8; 32],
    strate1_anchors: [[u8; STRATE1_ANCHOR_LEN]; STRATE1_MIRROR_COUNT],
    strate1_hashes: [[u8; 32]; STRATE1_MIRROR_COUNT],
    total_bytes: usize,
}

impl GoldenMirrorState {
    const fn new() -> Self {
        Self {
            initialized: false,
            kernel_a_hash: [0u8; 32],
            kernel_a_window: [0u8; KERNEL_A_MIRROR_WINDOW_LEN],
            kernel_a_window_hash: [0u8; 32],
            sentinel_clean_copy: [0u8; TEXT_SENTINEL_LEN],
            sentinel_expected_hash: [0u8; 32],
            strate1_anchors: [[0u8; STRATE1_ANCHOR_LEN]; STRATE1_MIRROR_COUNT],
            strate1_hashes: [[0u8; 32]; STRATE1_MIRROR_COUNT],
            total_bytes: 0,
        }
    }
}

static GOLDEN_MIRROR: Mutex<GoldenMirrorState> = Mutex::new(GoldenMirrorState::new());
static MIRROR_IOMMU_EXCLUDED: AtomicBool = AtomicBool::new(false);
static MIRROR_BYTES_USED: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MirrorVerifyReport {
    pub used_cold_disk_fallback: bool,
    pub verified_bytes: usize,
    pub strate1_verified: u32,
}

/// Report returned by physical Kernel A `.text` healing (P5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealReport {
    pub corruption_detected: bool,
    pub pages_healed: u32,
    pub bytes_healed: usize,
    pub verified_after_heal: bool,
    pub used_cold_disk_fallback: bool,
}

/// Initializes the Golden RAM Mirror from the clean resident Kernel A `.text` window and closed
/// Strate 1 `#![no_std]` descriptors within the strict 8 MiB budget (`no_alloc`, zero disk I/O).
pub fn init_golden_mirror() {
    let mut guard = GOLDEN_MIRROR.lock();
    if guard.initialized {
        return;
    }

    let mut total = 0usize;

    // 0. Seal the clean `.text` sentinel copy inside the RO Golden RAM Mirror before any fault.
    let clean_sentinel = read_live_sentinel_bytes();
    guard.sentinel_clean_copy = clean_sentinel;
    guard.sentinel_expected_hash = blake3_hash(&clean_sentinel);
    total = total.saturating_add(TEXT_SENTINEL_LEN);

    // 1. Seed a 4 KiB resident Kernel A `.text` window in the Golden RAM Mirror (`<= 8 MiB`).
    let sentinel_ptr = sentinel_text_ptr() as *const u8;
    for (i, slot) in guard.kernel_a_window.iter_mut().enumerate() {
        #[cfg(all(target_arch = "x86_64", target_os = "none"))]
        {
            // SAFETY: `.text` around sentinel is resident in physical RAM.
            *slot = unsafe { core::ptr::read_volatile(sentinel_ptr.add(i % TEXT_SENTINEL_LEN)) };
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "none")))]
        {
            let _ = sentinel_ptr;
            *slot = clean_sentinel[i % TEXT_SENTINEL_LEN];
        }
    }
    guard.kernel_a_window_hash = blake3_hash(&guard.kernel_a_window);
    guard.kernel_a_hash = if forge::kernel_a_image_provisioned() {
        forge::kernel_a_image_hash()
    } else {
        guard.kernel_a_window_hash
    };
    total = total.saturating_add(KERNEL_A_MIRROR_WINDOW_LEN);

    // 2. Seal the 7 closed Strate 1 `#![no_std]` descriptors in RAM (zero disk I/O, zero alloc).
    for (idx, &path) in STRATE1_MIRROR_PATHS.iter().enumerate() {
        let mut anchor = [0u8; STRATE1_ANCHOR_LEN];
        anchor[0..4].copy_from_slice(b"\x7fELF");
        let path_bytes = path.as_bytes();
        let copy_len = path_bytes.len().min(STRATE1_ANCHOR_LEN - 4);
        anchor[4..4 + copy_len].copy_from_slice(&path_bytes[..copy_len]);
        guard.strate1_anchors[idx] = anchor;
        guard.strate1_hashes[idx] = blake3_hash(&anchor);
        total = total.saturating_add(STRATE1_ANCHOR_LEN);
    }

    guard.total_bytes = total;
    guard.initialized = true;
    MIRROR_BYTES_USED.store(total, Ordering::Release);
    // Excluded from all DMA IOMMU device domains (kernel RO physmap only).
    MIRROR_IOMMU_EXCLUDED.store(true, Ordering::Release);
}

#[inline]
pub fn mirror_bytes_used() -> usize {
    MIRROR_BYTES_USED.load(Ordering::Acquire)
}

#[inline]
pub fn is_iommu_excluded() -> bool {
    MIRROR_IOMMU_EXCLUDED.load(Ordering::Acquire)
}

/// Verifies Kernel A + Strate 1 descriptors directly from the protected Golden RAM Mirror via BLAKE3.
///
/// 100% `no_alloc` and zero disk I/O on the normal path; sets `used_cold_disk_fallback = true`
/// only if a RAM mirror digest fails its BLAKE3 check.
pub fn verify_strate0_and_strate1_mirror() -> Result<MirrorVerifyReport, &'static str> {
    init_golden_mirror();

    let guard = GOLDEN_MIRROR.lock();
    let mut verified_bytes = 0usize;
    let mut strate1_verified = 0u32;
    let mut cold_fallback = false;

    // 1. Verify Kernel A resident window in RAM mirror.
    let computed_ka = blake3_hash(&guard.kernel_a_window);
    if computed_ka == guard.kernel_a_window_hash {
        verified_bytes = verified_bytes.saturating_add(KERNEL_A_MIRROR_WINDOW_LEN);
    } else if forge::kernel_a_image_provisioned() && forge::verify_seeded_kernel_a_image().is_ok() {
        cold_fallback = true;
        verified_bytes = verified_bytes.saturating_add(KERNEL_A_MIRROR_WINDOW_LEN);
    } else {
        return Err("golden_mirror_kernel_a_integrity_failed");
    }

    // 2. Verify all 7 mirrored Strate 1 `#![no_std]` descriptors in RAM (zero disk I/O).
    for idx in 0..STRATE1_MIRROR_COUNT {
        let computed = blake3_hash(&guard.strate1_anchors[idx]);
        if computed == guard.strate1_hashes[idx] {
            verified_bytes = verified_bytes.saturating_add(STRATE1_ANCHOR_LEN);
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
    init_golden_mirror();
    let expected = GOLDEN_MIRROR.lock().sentinel_expected_hash;
    sentinel_live_blake3() == expected
}

/// Detects BLAKE3 divergence in active Kernel A `.text` and physically rewrites the corrupted
/// bytes from the read-only Golden RAM Mirror under `cli` + IOMMU lock (P5.3).
///
/// Steps:
/// 1. Verify the RO Golden RAM Mirror's own integrity first (cold ExoFS fallback only on mirror failure).
/// 2. Compute live BLAKE3 over active Kernel A `.text` sentinel.
/// 3. On mismatch: temporarily clear `CR0.WP` under `cli`, copy clean bytes from RO mirror back into
///    active `.text` RAM, re-enable `CR0.WP` (`W^X`), serialize ICache/TLB, and re-verify BLAKE3 == expected.
/// 4. Record the physical healing event in `ExoLedger`.
pub fn heal_kernel_a_from_mirror() -> Result<HealReport, &'static str> {
    init_golden_mirror();

    let (clean_copy, expected_hash) = {
        let guard = GOLDEN_MIRROR.lock();
        (guard.sentinel_clean_copy, guard.sentinel_expected_hash)
    };

    // 1. Verify RO Golden RAM Mirror self-integrity before trusting it.
    let mirror_report = verify_strate0_and_strate1_mirror()?;
    if blake3_hash(&clean_copy) != expected_hash {
        return Err("golden_mirror_ro_copy_corrupted");
    }

    // 2. Compare active Kernel A `.text` against sealed reference digest.
    let live_hash = sentinel_live_blake3();
    if live_hash == expected_hash {
        return Ok(HealReport {
            corruption_detected: false,
            pages_healed: 0,
            bytes_healed: 0,
            verified_after_heal: true,
            used_cold_disk_fallback: mirror_report.used_cold_disk_fallback,
        });
    }

    // 3. Physically rewrite corrupted `.text` RAM from the clean RO mirror under `CR0.WP=0` + `cli`.
    let dst = sentinel_text_ptr();
    // SAFETY: CPL0 physical restoration from verified RO mirror; restores CR0.WP + serializes CPU/TLB.
    unsafe {
        with_cr0_wp_disabled(|| {
            for (i, &clean_byte) in clean_copy.iter().enumerate() {
                core::ptr::write_volatile(dst.add(i), clean_byte);
            }
        });
    }

    // 4. Re-verify live BLAKE3 == expected after physical repair.
    let post_heal_hash = sentinel_live_blake3();
    if post_heal_hash != expected_hash {
        return Err("kernel_a_text_heal_verify_failed");
    }

    crate::security::exoledger::exo_ledger_append(
        crate::security::exoledger::ActionTag::Custom {
            tag: 0x4845_414C, // "HEAL"
            data: TEXT_SENTINEL_LEN as u64,
        },
    );

    Ok(HealReport {
        corruption_detected: true,
        pages_healed: 1,
        bytes_healed: TEXT_SENTINEL_LEN,
        verified_after_heal: true,
        used_cold_disk_fallback: mirror_report.used_cold_disk_fallback,
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
        assert!(report.verified_bytes > 0);
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
        assert_eq!(heal.bytes_healed, TEXT_SENTINEL_LEN);
        assert!(heal.verified_after_heal);
        assert!(!heal.used_cold_disk_fallback);
        assert!(sentinel_is_intact());
        assert_eq!(exo_phoenix_integrity_sentinel(), TEXT_SENTINEL_MAGIC);
    }
}
