// kernel/src/memory/protection/umip.rs
//
// UMIP - User-Mode Instruction Prevention.
//
// CR4.UMIP makes SGDT, SIDT, SLDT, SMSW, and STR fault in CPL > 0.  The
// protection is per-CPU and must be enabled alongside the other CR4 hardening
// bits during BSP/AP protection init.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// CR4 bit 11 - UMIP.
pub const CR4_UMIP_BIT: u64 = 1 << 11;

#[repr(C)]
pub struct UmipStats {
    pub enable_count: AtomicU64,
    pub redundant_enable: AtomicU64,
}

impl UmipStats {
    const fn new() -> Self {
        Self {
            enable_count: AtomicU64::new(0),
            redundant_enable: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for UmipStats {}
pub static UMIP_STATS: UmipStats = UmipStats::new();

static UMIP_ACTIVE: AtomicBool = AtomicBool::new(false);

#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {v}, cr4",
        v = out(reg) val,
        options(nostack, nomem, preserves_flags),
    );
    val
}

#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {v}", v = in(reg) val, options(nostack, nomem));
}

#[inline]
pub fn umip_supported() -> bool {
    crate::arch::x86_64::cpu::features::cpu_features_or_none()
        .is_some_and(|features| features.has_umip())
}

/// Enables UMIP on the current CPU when the feature is present.
///
/// # Safety
/// Must run at CPL 0 on the target CPU.
pub unsafe fn enable_umip() {
    if !umip_supported() {
        return;
    }

    let cr4 = read_cr4();
    if cr4 & CR4_UMIP_BIT != 0 {
        UMIP_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        UMIP_ACTIVE.store(true, Ordering::Release);
        return;
    }

    write_cr4(cr4 | CR4_UMIP_BIT);
    UMIP_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
    UMIP_ACTIVE.store(true, Ordering::Release);
}

/// Returns true when CR4.UMIP is active on the current CPU.
///
/// # Safety
/// Must run at CPL 0.
#[inline]
pub unsafe fn umip_active() -> bool {
    read_cr4() & CR4_UMIP_BIT != 0
}

/// Initializes UMIP on the current CPU.
///
/// # Safety
/// Must run at CPL 0.
pub unsafe fn init() {
    enable_umip();
}
