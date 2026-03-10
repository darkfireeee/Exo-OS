//! # cpu/fpu.rs — Instructions ASM brutes XSAVE/XRSTOR/FXSAVE
//!
//! ⚠️ Ce module contient UNIQUEMENT les instructions matérielles FPU.
//!    La logique d'état (quand sauvegarder, flag lazy_fpu_used, etc.)
//!    est dans `scheduler/fpu/` — NE PAS dupliquer ici.
//!
//! ## Implémentation
//! - `fxsave` / `fxrstor` : fallback 512 bytes (FXSAVE area)
//! - `xsave`  / `xrstor`  : XSAVE complet (taille détectée via CPUID)
//! - `xsaveopt`           : optimisé (ne sauvegarde que les composants dirty)
//! - `xsavec`             : compacté (ne sauvegarde que les composants actifs)


use core::sync::atomic::{AtomicU32, Ordering};

// ── Constantes XSAVE ─────────────────────────────────────────────────────────

/// Composants XSAVE supportés (XCR0 bits)
pub const XSAVE_X87:       u64 = 1 << 0;   // x87 FPU
pub const XSAVE_SSE:       u64 = 1 << 1;   // XMM0–15
pub const XSAVE_AVX:       u64 = 1 << 2;   // YMM0–15 (upper 128)
pub const XSAVE_MPX_BNDREGS: u64 = 1 << 3; // BND0–3 (MPX)
pub const XSAVE_MPX_BNDCSR:  u64 = 1 << 4; // BNDCFGU / BNDSTATUS
pub const XSAVE_AVX512_K:     u64 = 1 << 5; // K0–7 opmask
pub const XSAVE_AVX512_ZMM_HI: u64 = 1 << 6; // ZMM0–15 upper 256
pub const XSAVE_AVX512_HI16:   u64 = 1 << 7; // ZMM16–31
pub const XSAVE_PT:           u64 = 1 << 8;  // Intel PT
pub const XSAVE_PKRU:         u64 = 1 << 9;  // Protection Keys

/// Masque XSAVE minimal — x87 + SSE + AVX (obligatoire pour Exo-OS)
pub const XSAVE_MASK_MINIMAL: u64 = XSAVE_X87 | XSAVE_SSE;

/// Masque XSAVE complet — tout ce que l'OS supporte
pub const XSAVE_MASK_ALL: u64 = 0xFFFF_FFFF_FFFF_FFFFu64;

/// Taille FXSAVE area fixe
pub const FXSAVE_AREA_SIZE: usize = 512;

/// Taille maximale XSAVE area (AVX-512 + PT + PKRU)
pub const XSAVE_AREA_MAX: usize = 2696;

/// Alignement requis pour FXSAVE/XSAVE area
pub const FPU_AREA_ALIGN: usize = 64;

// ── Taille XSAVE déterminée au runtime ───────────────────────────────────────

static XSAVE_AREA_SIZE_RUNTIME: AtomicU32 = AtomicU32::new(512);

/// Retourne la taille de l'XSAVE area détectée au boot
#[inline(always)]
pub fn xsave_area_size() -> usize {
    XSAVE_AREA_SIZE_RUNTIME.load(Ordering::Relaxed) as usize
}

/// Configure la taille XSAVE runtime (appelé depuis early_init)
pub fn set_xsave_area_size(size: u32) {
    XSAVE_AREA_SIZE_RUNTIME.store(size, Ordering::Release);
}

// ── Alignement statique pour FPU area ────────────────────────────────────────

/// Structure d'état FPU alignée 64 bytes (FXSAVE ou XSAVE area)
/// Le champ `data` est sur-dimensionné pour couvrir tous les cas.
/// L'allocateur kernel doit allouer avec alignement 64.
#[repr(C, align(64))]
pub struct FpuRawArea {
    pub data: [u8; XSAVE_AREA_MAX],
}

impl FpuRawArea {
    pub const fn zeroed() -> Self {
        Self { data: [0u8; XSAVE_AREA_MAX] }
    }
}

// ── FXSAVE / FXRSTOR ─────────────────────────────────────────────────────────

/// Sauvegarde l'état FPU/SSE via FXSAVE (512 bytes)
///
/// # SAFETY
/// - `dst` doit être aligné sur 16 bytes minimum (64 recommandé)
/// - `dst` doit pointer vers au moins 512 bytes valides et accessibles
/// - Le CPU doit supporter FXSR (CPUID.EDX[24]) — vérifier avant appel
#[inline(always)]
pub unsafe fn fxsave(dst: *mut u8) {
    // SAFETY: délégué à l'appelant — dst aligné et valide
    unsafe {
        core::arch::asm!(
            "fxsave64 [{dst}]",
            dst = in(reg) dst,
            options(nostack)
        );
    }
}

/// Restaure l'état FPU/SSE depuis une FXSAVE area
///
/// # SAFETY
/// - `src` doit être une FXSAVE area valide préalablement sauvegardée
/// - Alignement 16 bytes minimum requis
#[inline(always)]
pub unsafe fn fxrstor(src: *const u8) {
    // SAFETY: délégué à l'appelant — src est une zone fxsave valide
    unsafe {
        core::arch::asm!(
            "fxrstor64 [{src}]",
            src = in(reg) src,
            options(nostack)
        );
    }
}

// ── XSAVE / XRSTOR ───────────────────────────────────────────────────────────

/// Sauvegarde l'état FPU étendu via XSAVE
///
/// Sauvegarde tous les composants dont le bit est 1 dans `rfbm` (Request Feature BitMap).
/// Utiliser `XSAVE_MASK_ALL` pour tout sauvegarder.
///
/// # SAFETY
/// - `dst` doit être aligné sur 64 bytes
/// - `dst` doit pointer vers au moins `xsave_area_size()` bytes valides
/// - Le CPU doit supporter XSAVE (CPUID.ECX[26])
/// - OSXSAVE doit être activé dans CR4 (CR4.OSXSAVE = 1)
#[inline(always)]
pub unsafe fn xsave(dst: *mut u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "xsave64 [{dst}]",
            dst  = in(reg) dst,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

/// Restaure l'état FPU étendu via XRSTOR
///
/// # SAFETY
/// - `src` doit être une XSAVE area valide sauvegardée par `xsave()`
/// - Alignement 64 bytes requis
/// - Le composant header (offset 512) doit être cohérent
#[inline(always)]
pub unsafe fn xrstor(src: *const u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "xrstor64 [{src}]",
            src  = in(reg) src,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

/// Sauvegarde via XSAVEOPT (ne sauvegarde que les composants modifiés)
///
/// Plus rapide que XSAVE si peu de composants ont été modifiés depuis la dernière restore.
///
/// # SAFETY
/// Mêmes garanties que `xsave()`. De plus, le CPU doit supporter XSAVEOPT
/// (CPUID leaf 0xD subleaf 1, EAX[0]).
#[inline(always)]
pub unsafe fn xsaveopt(dst: *mut u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "xsaveopt64 [{dst}]",
            dst  = in(reg) dst,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

/// Sauvegarde via XSAVEC (format compact — ne sauvegarde que les composants actifs)
///
/// # SAFETY
/// Idem `xsaveopt`. CPU doit supporter XSAVEC (CPUID leaf 0xD subleaf 1, EAX[1]).
#[inline(always)]
pub unsafe fn xsavec(dst: *mut u8, rfbm: u64) {
    let rfbm_lo = rfbm as u32;
    let rfbm_hi = (rfbm >> 32) as u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "xsavec64 [{dst}]",
            dst  = in(reg) dst,
            in("eax") rfbm_lo,
            in("edx") rfbm_hi,
            options(nostack)
        );
    }
}

// ── XCR0 lecture/écriture ─────────────────────────────────────────────────────

/// Lit XCR0 (XSAVE Extended Control Register 0)
///
/// # SAFETY
/// Nécessite OSXSAVE actif dans CR4. Exécutable uniquement depuis Ring 0.
#[inline(always)]
pub unsafe fn read_xcr0() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Écrit XCR0 (active les composants XSAVE à sauvegarder)
///
/// # SAFETY
/// - Nécessite OSXSAVE dans CR4
/// - Bits non supportés dans xcr0 provoqueront #GP
/// - Ne pas désactiver X87 (bit 0) ni SSE (bit 1) une fois activés
#[inline(always)]
pub unsafe fn write_xcr0(val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "xsetbv",
            in("ecx") 0u32,
            in("eax") lo,
            in("edx") hi,
            options(nostack, nomem)
        );
    }
}

// ── Initialisation FPU au boot ────────────────────────────────────────────────

/// Initialise la FPU sur le CPU courant
///
/// - Active CR0.MP, efface CR0.EM
/// - Active CR4.OSFXSR + CR4.OSXMMEXCPT
/// - Active CR4.OSXSAVE si XSAVE disponible
/// - Configure XCR0 avec x87 + SSE (+ AVX si disponible)
/// - Exécute FINIT pour nettoyer l'état x87
pub fn init_fpu_for_cpu() {
    

    // Lecture CR0 courant
    let cr0 = super::super::read_cr4();
    let _ = cr0; // utilisation future

    let mut cr0_raw: u64;
    // SAFETY: lecture CR0 — aucun effet de bord
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0_raw, options(nostack, nomem));
    }

    // CR0.MP = 1 (Monitor co-processor)
    // CR0.EM = 0 (pas d'émulation FPU)
    // CR0.NE = 1 (native FP exceptions)
    // CR0.TS = 0 (Task Switched — lazy FPU le mettra à 1 après switch)
    cr0_raw |= (1 << 1) | (1 << 5);  // MP, NE
    cr0_raw &= !((1 << 2) | (1 << 3)); // clear EM, TS
    // SAFETY: modification cohérente de CR0 avec état FPU valide
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0_raw, options(nostack, nomem));
    }

    // CR4 : activer OSFXSR et OSXMMEXCPT
    let mut cr4 = super::super::read_cr4();
    cr4 |= (1 << 9)  // OSFXSR
         | (1 << 10); // OSXMMEXCPT

    let features = &super::features::CPU_FEATURES;

    if features.has_xsave() {
        cr4 |= 1 << 18; // OSXSAVE
    }

    // SAFETY: activation de OSFXSR/OSXMMEXCPT/OSXSAVE — requis pour SSE/XSAVE
    unsafe { super::super::write_cr4(cr4); }

    // Configurer XCR0 si XSAVE disponible
    if features.has_xsave() {
        let mut xcr0 = XSAVE_X87 | XSAVE_SSE;

        if features.has_avx() {
            xcr0 |= XSAVE_AVX;
        }
        if features.has_avx512f() {
            xcr0 |= XSAVE_AVX512_K | XSAVE_AVX512_ZMM_HI | XSAVE_AVX512_HI16;
        }
        if features.has_pku() {
            xcr0 |= XSAVE_PKRU;
        }

        // SAFETY: xcr0 ne contient que des bits supportés par ce CPU
        unsafe { write_xcr0(xcr0); }

        // Mettre à jour la taille XSAVE détectée
        let size = features.xsave_size();
        set_xsave_area_size(size);
    }

    // FINIT : nettoie l'état x87 (tag=empty, PC=80bit, RC=round-nearest)
    // SAFETY: FINIT est toujours valide en Ring 0 avec FPU activée
    unsafe {
        core::arch::asm!("finit", options(nostack));
    }

    // MXCSR par défaut : masquer toutes les exceptions SSE non-critiques
    // Bit 7–12 = exception masks ON, bits 13–14 = RC=round-to-nearest
    const MXCSR_DEFAULT: u32 = 0x1F80;
    // SAFETY: ldmxcsr charge un MXCSR valide
    unsafe {
        core::arch::asm!(
            "ldmxcsr [{mxcsr}]",
            mxcsr = in(reg) &MXCSR_DEFAULT as *const u32,
            options(nostack)
        );
    }
}

// ── CLFLUSH / CLFLUSHOPT / CLWB ──────────────────────────────────────────────

/// Flush une cache line (CLFLUSH — invalide dans tous les niveaux cache)
///
/// # SAFETY
/// `addr` doit être mappé et accessible.
#[inline(always)]
pub unsafe fn clflush(addr: *const u8) {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "clflush [{addr}]",
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

/// Flush optimisé (CLFLUSHOPT — non-sérialisante, plus rapide que CLFLUSH)
///
/// # SAFETY
/// Idem `clflush`. Nécessite CLFLUSHOPT (CPUID leaf 7, EBX[23]).
#[inline(always)]
pub unsafe fn clflushopt(addr: *const u8) {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "clflushopt [{addr}]",
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

/// Cache line write-back (CLWB — writeback sans invalider)
///
/// # SAFETY
/// Idem `clflush`. Nécessite CLWB (CPUID leaf 7, EBX[24]).
#[inline(always)]
pub unsafe fn clwb(addr: *const u8) {
    // SAFETY: délégué à l'appelant
    unsafe {
        core::arch::asm!(
            "clwb [{addr}]",
            addr = in(reg) addr,
            options(nostack)
        );
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// C ABI EXPORTS — scheduler/fpu/save_restore.rs interface
// ═════════════════════════════════════════════════════════════════════════════
//
// Ces fonctions #[no_mangle] extern "C" sont le pont FFI entre
// scheduler/fpu/save_restore.rs (RÈGLE FPU-01 DOC3) et ce module.
//
// scheduler/ est Couche 1 et ne peut pas importer directement arch/.
// La séparation logique/instructions est respectée :
//   • scheduler/fpu/save_restore.rs = LOGIQUE (quand sauvegarder, pour qui)
//   • arch/x86_64/cpu/fpu.rs        = INSTRUCTIONS ASM brutes (xsave/xrstor)
//
// SYNCHRONISATION : toute modification de signature ici DOIT être mise à jour
// dans scheduler/fpu/save_restore.rs extern "C" block simultanément.
// ═════════════════════════════════════════════════════════════════════════════

/// Sauvegarde l'état FPU étendu via XSAVE (C ABI export).
///
/// Appelé depuis `scheduler::fpu::save_restore::xsave_current()`.
///
/// # Safety
/// - `area` doit être aligné 64 bytes, taille ≥ `xsave_area_size()`.
/// - OSXSAVE doit être activé dans CR4.
/// - XSAVE doit être supporté (vérifier `arch_has_xsave()` avant).
#[no_mangle]
pub unsafe extern "C" fn arch_xsave64(area: *mut u8, rfbm: u64) {
    // SAFETY: délégué à l'appelant (scheduler vérifie arch_has_xsave et l'alignement).
    xsave(area, rfbm);
}

/// Restaure l'état FPU étendu via XRSTOR (C ABI export).
///
/// Appelé depuis `scheduler::fpu::save_restore::xrstor_for()`.
///
/// # Safety
/// - `area` doit être une XSAVE area valide, alignée 64 bytes.
/// - Le composant header (offset 512) doit être cohérent avec XCR0.
#[no_mangle]
pub unsafe extern "C" fn arch_xrstor64(area: *const u8, rfbm: u64) {
    // SAFETY: délégué à l'appelant.
    xrstor(area, rfbm);
}

/// Sauvegarde l'état FPU/SSE via FXSAVE (C ABI export, fallback sans XSAVE).
///
/// Appelé depuis `scheduler::fpu::save_restore::xsave_current()` si !arch_has_xsave().
///
/// # Safety
/// - `area` doit être aligné 16 bytes minimum (64 recommandé), taille ≥ 512.
#[no_mangle]
pub unsafe extern "C" fn arch_fxsave64(area: *mut u8) {
    // SAFETY: délégué à l'appelant.
    fxsave(area);
}

/// Restaure l'état FPU/SSE via FXRSTOR (C ABI export, fallback sans XSAVE).
///
/// Appelé depuis `scheduler::fpu::save_restore::xrstor_for()` si !arch_has_xsave().
///
/// # Safety
/// - `area` doit être une FXSAVE area valide, alignée 16 bytes.
#[no_mangle]
pub unsafe extern "C" fn arch_fxrstor64(area: *const u8) {
    // SAFETY: délégué à l'appelant.
    fxrstor(area);
}

/// Retourne 1 si le CPU supporte XSAVE, 0 sinon (C ABI export).
///
/// Appelé depuis `scheduler::fpu::save_restore::init()` au boot.
/// Lit uniquement les feature flags, aucun effet de bord.
#[no_mangle]
pub extern "C" fn arch_has_xsave() -> u8 {
    super::features::CPU_FEATURES.has_xsave() as u8
}

/// Retourne 1 si le CPU supporte AVX (YMM registers), 0 sinon (C ABI export).
///
/// Appelé depuis `scheduler::fpu::save_restore` pour dimensionner le buffer.
/// Lit uniquement les feature flags, aucun effet de bord.
#[no_mangle]
pub extern "C" fn arch_has_avx() -> u8 {
    super::features::CPU_FEATURES.has_avx() as u8
}
