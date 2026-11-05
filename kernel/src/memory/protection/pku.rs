// kernel/src/memory/protection/pku.rs
//
// PKU — Protection Keys for Userspace (Intel MPX successor).
//
// The PKRU register (32 bits) associates 2 bits with each key (0..15):
//   bit[2k]   = AD (Access Disable)   — disables read + write
//   bit[2k+1] = WD (Write Disable)    — disables write only
//
// Pages carry a "pkey" (4 bits) in their PTEs (bits 62:59).
// CR4.PKE = 1 enables the mechanism in user-mode.
// CR4.PKS = 1 enables the mechanism in kernel-mode (new Intel feature).
//
// References:
//   Intel SDM Vol.3A § 4.6.2 — "Protection Keys"
//   AMD APM Vol.2 § 5.20 — "Page-Based Protection Keys"
//
// Layer 0 — no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Number of available protection keys.
pub const PKU_KEY_COUNT: usize = 16;
/// Mask allowing extraction of the pkey bits from a PTE (bits 62:59).
pub const PTE_PKEY_MASK: u64 = 0x7800_0000_0000_0000;
/// Shift of the pkey bits in a PTE.
pub const PTE_PKEY_SHIFT: u64 = 59;

/// CR4 bit 22 — PKE (Protection Keys Enable, user).
pub const CR4_PKE_BIT: u64 = 1 << 22;
/// CR4 bit 24 — PKS (Protection Key Supervisor, kernel).
pub const CR4_PKS_BIT: u64 = 1 << 24;

/// Bit AD in PKRU for key `k`: bit 2k.
#[inline(always)]
pub const fn pkru_ad_bit(k: u8) -> u32 {
    1u32 << (k as u32 * 2)
}

/// Bit WD in PKRU for key `k`: bit 2k+1.
#[inline(always)]
pub const fn pkru_wd_bit(k: u8) -> u32 {
    1u32 << (k as u32 * 2 + 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// Reserved key 0: default key, always accessible.
/// Key 0 is the "public" key — all pages without an explicit pkey use it.
pub const PKU_DEFAULT_KEY: u8 = 0;
/// Key 1 reserved for the kernel heap.
pub const PKU_KERNEL_HEAP_KEY: u8 = 1;
/// Key 2 reserved for guard pages (inaccessible).
pub const PKU_GUARD_KEY: u8 = 2;
/// Key 3 reserved for MMIO data.
pub const PKU_MMIO_KEY: u8 = 3;
// Keys 4..15 free for dynamic allocation by userland / driver.

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct PkuStats {
    pub enable_count: AtomicU64,
    pub alloc_count: AtomicU64,
    pub free_count: AtomicU64,
    pub pkru_write_count: AtomicU64,
    pub violation_count: AtomicU64,
    pub exhausted_count: AtomicU64,
}

impl Default for PkuStats {
    fn default() -> Self {
        Self::new()
    }
}

impl PkuStats {
    const fn new() -> Self {
        Self {
            enable_count: AtomicU64::new(0),
            alloc_count: AtomicU64::new(0),
            free_count: AtomicU64::new(0),
            pkru_write_count: AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            exhausted_count: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for PkuStats {}
pub static PKU_STATS: PkuStats = PkuStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// Key allocator
// ─────────────────────────────────────────────────────────────────────────────

/// Descriptor of an allocated PKU key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PkuKeyDesc {
    /// Key index (0..15).
    pub key: u8,
    /// Owner (TID, 0 = kernel).
    pub owner: u64,
    /// Currently allocated key.
    pub in_use: bool,
}

impl PkuKeyDesc {
    const fn free() -> Self {
        Self {
            key: 0,
            owner: 0,
            in_use: false,
        }
    }
}

struct PkuKeyAllocator {
    keys: [PkuKeyDesc; PKU_KEY_COUNT],
}

impl Default for PkuKeyAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl PkuKeyAllocator {
    const fn new() -> Self {
        // Keys 0..PKU_DEFAULT+3 are reserved for the kernel.
        let mut keys = [PkuKeyDesc::free(); PKU_KEY_COUNT];
        keys[PKU_DEFAULT_KEY as usize] = PkuKeyDesc {
            key: PKU_DEFAULT_KEY,
            owner: 0,
            in_use: true,
        };
        keys[PKU_KERNEL_HEAP_KEY as usize] = PkuKeyDesc {
            key: PKU_KERNEL_HEAP_KEY,
            owner: 0,
            in_use: true,
        };
        keys[PKU_GUARD_KEY as usize] = PkuKeyDesc {
            key: PKU_GUARD_KEY,
            owner: 0,
            in_use: true,
        };
        keys[PKU_MMIO_KEY as usize] = PkuKeyDesc {
            key: PKU_MMIO_KEY,
            owner: 0,
            in_use: true,
        };
        Self { keys }
    }

    /// Allocates the first free key (>=4). Returns `None` if exhausted.
    fn alloc(&mut self, owner: u64) -> Option<u8> {
        for k in 4..PKU_KEY_COUNT {
            if !self.keys[k].in_use {
                self.keys[k] = PkuKeyDesc {
                    key: k as u8,
                    owner,
                    in_use: true,
                };
                return Some(k as u8);
            }
        }
        None
    }

    /// Frees a key.
    fn free(&mut self, key: u8) -> bool {
        let k = key as usize;
        if !(4..PKU_KEY_COUNT).contains(&k) {
            return false; // reserved or invalid key
        }
        if !self.keys[k].in_use {
            return false;
        }
        self.keys[k].in_use = false;
        self.keys[k].owner = 0;
        true
    }
}

static PKU_KEY_ALLOC: Mutex<PkuKeyAllocator> = Mutex::new(PkuKeyAllocator::new());

// ─────────────────────────────────────────────────────────────────────────────
// Read / write PKRU via XSAVE instructions
// ─────────────────────────────────────────────────────────────────────────────

/// Reads the PKRU register on the current CPU.
///
/// # Safety
/// CPL 0, PKE must be active.
#[inline(always)]
pub unsafe fn rdpkru() -> u32 {
    let val: u32;
    core::arch::asm!(
        "xor ecx, ecx",
        "rdpkru",
        out("eax") val,
        lateout("edx") _,
        lateout("ecx") _,
        options(nostack, nomem),
    );
    val
}

/// Writes `val` into the PKRU register.
///
/// # Safety
/// CPL 0 or user (RDPKRU user-mode ok if CR4.PKE).
#[inline(always)]
pub unsafe fn wrpkru(val: u32) {
    core::arch::asm!(
        "xor ecx, ecx",
        "xor edx, edx",
        "wrpkru",
        in("eax") val,
        options(nostack, nomem),
    );
    PKU_STATS.pkru_write_count.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// CR4 read
// ─────────────────────────────────────────────────────────────────────────────

/// # Safety: CPL 0.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let v: u64;
    core::arch::asm!("mov {v}, cr4", v = out(reg) v, options(nostack, nomem, preserves_flags));
    v
}

/// # Safety: CPL 0.
#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {v}", v = in(reg) val, options(nostack, nomem));
}

// ─────────────────────────────────────────────────────────────────────────────
// CPUID detection
// ─────────────────────────────────────────────────────────────────────────────

/// PKU supported: CPUID.7.0:ECX bit 3.
#[inline]
pub fn pku_supported() -> bool {
    let ecx: u32;
    // SAFETY: CPUID available on x86_64; xchg preserves rbx reserved by LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            out("ecx") ecx,
            lateout("eax") _,
            lateout("edx") _,
            tmp = inout(reg) 0u64 => _,
            options(nostack, nomem),
        );
    }
    ecx & (1 << 3) != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// PKU enable
// ─────────────────────────────────────────────────────────────────────────────

/// Enables PKU (CR4.PKE = 1) and configures initial PKRU.
///
/// Initial config:
///   - Key 0 (default) : AD=0 WD=0 -> accessible in R/W.
///   - Key 1 (heap)    : AD=0 WD=0 -> accessible R/W (managed by key hypervisor).
///   - Key 2 (guard)   : AD=1 WD=0 -> inaccessible.
///   - Key 3 (MMIO)    : AD=0 WD=1 -> read-only (MMIO data protection).
///   - Keys 4..15      : AD=0 WD=0 -> accessible (allocated dynamically).
///
/// # Safety
/// CPL 0.
pub unsafe fn enable_pku() {
    if !pku_supported() {
        return;
    }
    let cr4 = read_cr4();
    write_cr4(cr4 | CR4_PKE_BIT);
    PKU_STATS.enable_count.fetch_add(1, Ordering::Relaxed);

    // Initial PKRU: key 2 (guard) inaccessible.
    let pkru: u32 = pkru_ad_bit(PKU_GUARD_KEY) // key 2 AD
                  | pkru_wd_bit(PKU_MMIO_KEY); // key 3 WD
    wrpkru(pkru);
}

// ─────────────────────────────────────────────────────────────────────────────
// Public keys API
// ─────────────────────────────────────────────────────────────────────────────

/// Allocates a PKU key for thread `owner_tid`.
/// Returns `Some(key)` or `None` if all exhausted.
pub fn pku_alloc_key(owner_tid: u64) -> Option<u8> {
    let key = PKU_KEY_ALLOC.lock().alloc(owner_tid);
    if key.is_some() {
        PKU_STATS.alloc_count.fetch_add(1, Ordering::Relaxed);
    } else {
        PKU_STATS.exhausted_count.fetch_add(1, Ordering::Relaxed);
    }
    key
}

/// Frees key `key`. Returns `true` if successful.
pub fn pku_free_key(key: u8) -> bool {
    let ok = PKU_KEY_ALLOC.lock().free(key);
    if ok {
        PKU_STATS.free_count.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Enables R/W access for key `key` in the current PKRU.
///
/// # Safety
/// CPL 0. Only if PKU is active.
pub unsafe fn pku_allow_key(key: u8) {
    if key >= PKU_KEY_COUNT as u8 {
        return;
    }
    let pkru = rdpkru();
    let mask = pkru_ad_bit(key) | pkru_wd_bit(key);
    wrpkru(pkru & !mask);
}

/// Disables all access for key `key` (AD = 1).
///
/// # Safety
/// CPL 0.
pub unsafe fn pku_deny_key(key: u8) {
    if key >= PKU_KEY_COUNT as u8 {
        return;
    }
    let pkru = rdpkru();
    wrpkru(pkru | pkru_ad_bit(key));
}

/// Enables read-only access for key `key` (WD = 1, AD = 0).
///
/// # Safety
/// CPL 0.
pub unsafe fn pku_readonly_key(key: u8) {
    if key >= PKU_KEY_COUNT as u8 {
        return;
    }
    let pkru = rdpkru();
    let new = (pkru & !pkru_ad_bit(key)) | pkru_wd_bit(key);
    wrpkru(new);
}

/// Encodes pkey into bits 62:59 of a PTE.
#[inline]
pub const fn pte_set_pkey(pte: u64, key: u8) -> u64 {
    (pte & !PTE_PKEY_MASK) | (((key as u64) & 0xF) << PTE_PKEY_SHIFT)
}

/// Extracts pkey from bits 62:59 of a PTE.
#[inline]
pub const fn pte_get_pkey(pte: u64) -> u8 {
    ((pte & PTE_PKEY_MASK) >> PTE_PKEY_SHIFT) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// RAII guard: temporary access to a key
// ─────────────────────────────────────────────────────────────────────────────

/// RAII guard that authorizes access to `key` during its lifetime,
/// then restores the previous PKRU state on drop.
pub struct PkuAccessGuard {
    #[allow(dead_code)]
    key: u8,
    saved_pkru: u32,
}

impl PkuAccessGuard {
    /// # Safety
    /// CPL 0, PKU active.
    pub unsafe fn new(key: u8) -> Self {
        let saved_pkru = rdpkru();
        pku_allow_key(key);
        PkuAccessGuard { key, saved_pkru }
    }
}

impl Drop for PkuAccessGuard {
    fn drop(&mut self) {
        // SAFETY: wrpkru restores the PKRU value saved in new(); CPL 0 required.
        unsafe { wrpkru(self.saved_pkru) };
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Violation handler
// ─────────────────────────────────────────────────────────────────────────────

/// Called during a PKU #PF. Returns `false` -> unrecoverable.
#[inline]
pub fn pku_handle_violation(fault_addr: u64, key: u8) -> bool {
    PKU_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = (fault_addr, key);
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes PKU. Must be called on BSP + each AP after page tables initialization.
///
/// # Safety
/// CPL 0.
pub unsafe fn init() {
    enable_pku();
}
