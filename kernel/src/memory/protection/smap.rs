// kernel/src/memory/protection/smap.rs
//
// SMAP — Supervisor Mode Access Prevention.
//
// Lorsque CR4.SMAP = 1, tout accès *données* en espace user par le kernel
// génère une #PF — sauf si le kernel a préalablement exécuté `STAC` (set AC).
// `CLAC` (clear AC) ferme cette fenêtre.
//
// Références :
//   Intel SDM Vol.3A § 4.6.3 — "Supervisor-Mode Access Prevention"
//   AMD APM Vol.2 § 5.17 — "Supervisor Mode Access Prevention"
//
// Couche 0 : aucune dépendance scheduler/process/ipc/fs.


use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Bits CR4 / EFLAGS.AC
// ─────────────────────────────────────────────────────────────────────────────

/// CR4 bit 21 — SMAP.
pub const CR4_SMAP_BIT: u64 = 1 << 21;
/// EFLAGS / RFLAGS bit 18 — Alignment Check / Access Control (SMAP).
pub const RFLAGS_AC_BIT: u64 = 1 << 18;

// ─────────────────────────────────────────────────────────────────────────────
// Statistiques
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
pub struct SmapStats {
    pub enable_count:    AtomicU64,
    pub disable_count:   AtomicU64,
    pub stac_count:      AtomicU64,
    pub clac_count:      AtomicU64,
    pub violation_count: AtomicU64,
    pub redundant_enable: AtomicU64,
}

impl SmapStats {
    const fn new() -> Self {
        Self {
            enable_count:    AtomicU64::new(0),
            disable_count:   AtomicU64::new(0),
            stac_count:      AtomicU64::new(0),
            clac_count:      AtomicU64::new(0),
            violation_count: AtomicU64::new(0),
            redundant_enable: AtomicU64::new(0),
        }
    }
}

unsafe impl Sync for SmapStats {}
pub static SMAP_STATS: SmapStats = SmapStats::new();

static SMAP_ACTIVE: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// Helpers CR4
// ─────────────────────────────────────────────────────────────────────────────

/// # Safety : CPL 0.
#[inline(always)]
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!("mov {v}, cr4", v = out(reg) val, options(nostack, nomem, preserves_flags));
    val
}

/// # Safety : CPL 0.
#[inline(always)]
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {v}", v = in(reg) val, options(nostack, nomem));
}

// ─────────────────────────────────────────────────────────────────────────────
// Détection CPUID
// ─────────────────────────────────────────────────────────────────────────────

/// Teste le support SMAP : CPUID feuille 7, sous-feuille 0, EBX bit 20.
#[inline]
pub fn smap_supported() -> bool {
    let ebx_r: u64;
    // SAFETY: CPUID disponible sur x86_64; xchg préserve rbx réservé par LLVM.
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
// STAC / CLAC inline
// ─────────────────────────────────────────────────────────────────────────────

/// `STAC` — ouvre la fenêtre d'accès SMAP (RFLAGS.AC = 1).
/// À utiliser juste avant copy_from_user / copy_to_user.
///
/// # Safety
/// CPL 0. La fenêtre doit être refermée par `clac()` aussi tôt que possible.
#[inline(always)]
pub unsafe fn stac() {
    core::arch::asm!("stac", options(nostack, nomem, preserves_flags));
    SMAP_STATS.stac_count.fetch_add(1, Ordering::Relaxed);
}

/// `CLAC` — ferme la fenêtre d'accès SMAP (RFLAGS.AC = 0).
///
/// # Safety
/// CPL 0.
#[inline(always)]
pub unsafe fn clac() {
    core::arch::asm!("clac", options(nostack, nomem, preserves_flags));
    SMAP_STATS.clac_count.fetch_add(1, Ordering::Relaxed);
}

// ─────────────────────────────────────────────────────────────────────────────
// Activation / désactivation SMAP
// ─────────────────────────────────────────────────────────────────────────────

/// Active SMAP sur le CPU courant.
///
/// # Safety : CPL 0, interruptible safe.
pub unsafe fn enable_smap() {
    if !smap_supported() {
        return;
    }
    let cr4 = read_cr4();
    if cr4 & CR4_SMAP_BIT != 0 {
        SMAP_STATS.redundant_enable.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // S'assurer AC = 0 avant d'activer SMAP pour ne pas ouvrir de fenêtre.
    clac();
    write_cr4(cr4 | CR4_SMAP_BIT);
    SMAP_STATS.enable_count.fetch_add(1, Ordering::Relaxed);
    SMAP_ACTIVE.store(true, Ordering::Release);
}

/// Désactive SMAP temporairement — usage debug uniquement.
///
/// # Safety : CPL 0.
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

/// Retourne `true` si SMAP est actif sur ce CPU.
///
/// # Safety : CPL 0.
#[inline]
pub unsafe fn smap_active() -> bool {
    read_cr4() & CR4_SMAP_BIT != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Guard RAII copy user
// ─────────────────────────────────────────────────────────────────────────────

/// Guard RAII : exécute `stac()` à la construction et `clac()` au drop.
///
/// Usage :
/// ```ignore
/// let _guard = unsafe { SmapAccessGuard::new() };
/// // copier depuis / vers espace user
/// // guard dropped → clac automatique
/// ```
pub struct SmapAccessGuard {
    _private: (),
}

impl SmapAccessGuard {
    /// # Safety : CPL 0.  La fenêtre SMAP est ouverte tant que ce guard vit.
    #[inline]
    pub unsafe fn new() -> Self {
        stac();
        SmapAccessGuard { _private: () }
    }
}

impl Drop for SmapAccessGuard {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: clac() rétablit EFLAGS.AC à 0; protège SMAP après la section critic.
        unsafe { clac() };
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// copy_from_user / copy_to_user — wrappers sécurisés
// ─────────────────────────────────────────────────────────────────────────────

/// Copie `count` bytes depuis `user_src` vers `kernel_dst` en ouvrant
/// la fenêtre SMAP le temps de la copie.
///
/// # Safety
/// - `kernel_dst` doit être un pointeur kernel valide.
/// - `user_src` doit être une adresse user valide et accessible (≤ 47 bits).
/// - `count` doit être dans les limites des deux buffers.
pub unsafe fn copy_from_user(kernel_dst: *mut u8, user_src: *const u8, count: usize) {
    let _guard = SmapAccessGuard::new();
    core::ptr::copy_nonoverlapping(user_src, kernel_dst, count);
    // Guard dropped ici → clac automatique.
}

/// Copie `count` bytes depuis `kernel_src` vers `user_dst`.
///
/// # Safety : idem copy_from_user.
pub unsafe fn copy_to_user(user_dst: *mut u8, kernel_src: *const u8, count: usize) {
    let _guard = SmapAccessGuard::new();
    core::ptr::copy_nonoverlapping(kernel_src, user_dst, count);
}

/// Efface (zero) `count` bytes à `user_dst`.
///
/// # Safety : CPL 0, `user_dst` user-valid.
pub unsafe fn zero_user(user_dst: *mut u8, count: usize) {
    let _guard = SmapAccessGuard::new();
    core::ptr::write_bytes(user_dst, 0, count);
}

// ─────────────────────────────────────────────────────────────────────────────
// Violation handler
// ─────────────────────────────────────────────────────────────────────────────

/// Appelé par le fault handler lors d'une #PF SMAP (kernel accède user sans STAC).
/// Retourne `false` → non récupérable.
#[inline]
pub fn smap_handle_violation(fault_addr: u64, rip: u64) -> bool {
    SMAP_STATS.violation_count.fetch_add(1, Ordering::Relaxed);
    let _ = (fault_addr, rip);
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Init SMAP : doit être appelé sur BSP + chaque AP.
///
/// # Safety : CPL 0.
pub unsafe fn init() {
    enable_smap();
}
