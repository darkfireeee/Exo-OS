// kernel/src/memory/protection/smap.rs
//
// SMAP — Supervisor Mode Access Prevention.
//
// When CR4.SMAP = 1, any *data* access in user space by the kernel
// generates a #PF — unless the kernel has previously executed `STAC` (set AC).
// `CLAC` (clear AC) closes this window.
//
// References:
//   Intel SDM Vol.3A § 4.6.3 — "Supervisor-Mode Access Prevention"
//   AMD APM Vol.2 § 5.17 — "Supervisor Mode Access Prevention"
//
// Layer 0: no scheduler/process/ipc/fs dependencies.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// CR4 / EFLAGS.AC bits
// ─────────────────────────────────────────────────────────────────────────────

/// CR4 bit 21 — SMAP.
pub const CR4_SMAP_BIT: u64 = 1 << 21;
/// EFLAGS / RFLAGS bit 18 — Alignment Check / Access Control (SMAP).
pub const RFLAGS_AC_BIT: u64 = 1 << 18;

// ─────────────────────────────────────────────────────────────────────────────
// Statistics
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct SmapStats {
    pub enable_count: AtomicU64,
    pub disable_count: AtomicU64,
    pub stac_count: AtomicU64,
    pub clac_count: AtomicU64,
    pub violation_count: AtomicU64,
    pub redundant_enable: AtomicU64,
}

impl Default for SmapStats {
    fn default() -> Self {
        Self::new()
    }
}

impl SmapStats {
    const fn new() -> Self {
        Self {
            enable_count: AtomicU64::new(0),
            disable_count: AtomicU64::new(0),
            stac_count: AtomicU64::new(0),
            clac_count: AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            redundant_enable: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for SmapStats {}
pub static SMAP_STATS: SmapStats = SmapStats::new();

static SMAP_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// CR4 helpers
// ─────────────────────────────────────────────────────────────────────────────

/// # Safety: CPL 0.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!("mov {v}, cr4", v = out(reg) val, options(nostack, nomem, preserves_flags));
    val
}

/// # Safety: CPL 0.
#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {v}", v = in(reg) val, options(nostack, nomem));
}

// ─────────────────────────────────────────────────────────────────────────────
// CPUID detection
// ─────────────────────────────────────────────────────────────────────────────

/// Tests SMAP support: CPUID leaf 7, subleaf 0, EBX bit 20.
#[inline]
pub fn smap_supported() -> bool {
    let ebx_r: u64;
    // SAFETY: CPUID available on x86_64; xchg preserves rbx reserved by LLVM.
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            tmp = inout(reg) 0u64 => ebx_r,
            options(nostack, nomem),
        );
    }
    let ebx = ebx_r as u32;
    ebx & (1 << 20) != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Inline STAC / CLAC
// ─────────────────────────────────────────────────────────────────────────────

/// `STAC` — opens the SMAP access window (RFLAGS.AC = 1).
/// To be used right before copy_from_user / copy_to_user.
///
/// # Safety
/// CPL 0. The window must be closed by `clac()` as early as possible.
#[inline(always)]
pub unsafe fn stac() {
    core::arch::asm!("stac", options(nostack, nomem, preserves_flags));
    SMAP_STATS.stac_count.fetch_add(1, Ordering::Relaxed);
}

/// `CLAC` — closes the SMAP access window (RFLAGS.AC = 0).
///
/// # Safety
/// CPL 0.
#[inline(always)]
pub unsafe fn clac() {
    core::arch::asm!("clac", options(nostack, nomem, preserves_flags));
    SMAP_STATS.clac_count.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// SMAP enable / disable
// ─────────────────────────────────────────────────────────────────────────────

/// Enables SMAP on the current CPU.
///
/// # Safety
/// CPL 0, interruptible safe.
pub unsafe fn enable_smap() {
    if !smap_supported() {
        return;
    }
    let cr4 = read_cr4();
    if cr4 & CR4_SMAP_BIT != 0 {
        SMAP_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // Ensure AC = 0 before enabling SMAP to avoid opening a window.
    clac();
    write_cr4(cr4 | CR4_SMAP_BIT);
    SMAP_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
    SMAP_ACTIVE.store(true, Ordering::Release);
}

/// Temporarily disables SMAP — debug usage only.
///
/// # Safety
/// CPL 0.
pub unsafe fn disable_smap() -> bool {
    let cr4 = read_cr4();
    if cr4 & CR4_SMAP_BIT == 0 {
        return false;
    }
    write_cr4(cr4 & !CR4_SMAP_BIT);
    SMAP_STATS.disable_count.fetch_add(1, Ordering::Relaxed);
    SMAP_ACTIVE.store(false, Ordering::Release);
    true
}

/// Returns `true` if SMAP is active on this CPU.
///
/// # Safety
/// CPL 0.
#[inline]
pub unsafe fn smap_active() -> bool {
    read_cr4() & CR4_SMAP_BIT != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// User copy RAII guard
// ─────────────────────────────────────────────────────────────────────────────

/// RAII guard: executes `stac()` upon construction and `clac()` on drop.
///
/// Usage:
/// ```ignore
/// let _guard = unsafe { SmapAccessGuard::new() };
/// // copy from / to user space
/// // guard dropped -> automatic clac
/// ```
pub struct SmapAccessGuard {
    _private: (),
}

impl SmapAccessGuard {
    /// # Safety
    /// CPL 0. The SMAP window is open as long as this guard lives.
    #[inline]
    pub unsafe fn new() -> Self {
        stac();
        SmapAccessGuard { _private: () }
    }
}

impl Drop for SmapAccessGuard {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: clac() restores EFLAGS.AC to 0; protects SMAP after the critical section.
        unsafe { clac() };
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// copy_from_user / copy_to_user — safe wrappers
// ─────────────────────────────────────────────────────────────────────────────

/// Copies `count` bytes from `user_src` to `kernel_dst` while opening
/// the SMAP window for the duration of the copy.
///
/// # Safety
/// - `kernel_dst` must be a valid kernel pointer.
/// - `user_src` must be a valid and accessible user address (<= 47 bits).
/// - `count` must be within the bounds of both buffers.
pub unsafe fn copy_from_user(kernel_dst: *mut u8, user_src: *const u8, count: usize) {
    let _guard = SmapAccessGuard::new();
    core::ptr::copy_nonoverlapping(user_src, kernel_dst, count);
    // Guard dropped here -> automatic clac.
}

/// Copies `count` bytes from `kernel_src` to `user_dst`.
///
/// # Safety
/// Same as copy_from_user.
pub unsafe fn copy_to_user(user_dst: *mut u8, kernel_src: *const u8, count: usize) {
    let _guard = SmapAccessGuard::new();
    core::ptr::copy_nonoverlapping(kernel_src, user_dst, count);
}

/// Zeroes `count` bytes at `user_dst`.
///
/// # Safety
/// CPL 0, `user_dst` user-valid.
pub unsafe fn zero_user(user_dst: *mut u8, count: usize) {
    let _guard = SmapAccessGuard::new();
    core::ptr::write_bytes(user_dst, 0, count);
}

// ─────────────────────────────────────────────────────────────────────────────
// Violation handler
// ─────────────────────────────────────────────────────────────────────────────

/// Called by the fault handler during an SMAP #PF (kernel accesses user without STAC).
/// Returns `false` -> unrecoverable.
#[inline]
pub fn smap_handle_violation(fault_addr: u64, rip: u64) -> bool {
    SMAP_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = (fault_addr, rip);
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

/// Initializes SMAP: must be called on BSP + each AP.
///
/// # Safety
/// CPL 0.
pub unsafe fn init() {
    enable_smap();
}
