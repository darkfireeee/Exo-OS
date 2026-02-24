// kernel/src/scheduler/fpu/state.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// FPU STATE — Zone mémoire XSAVE/FXSAVE par thread (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE FPU-01 (DOC3) :
//   Ce module gère LA LOGIQUE d'état FPU (quand, pour quel thread).
//   Les instructions ASM brutes (xsave/xrstor) sont dans arch/x86_64/cpu/fpu.rs.
//   NE PAS dupliquer les instructions ASM ici.
//
// Tailles de zone selon le niveau d'extension FPU :
//   • FXSAVE seulement       : 512 bytes (minimum x86_64)
//   • XSAVE + AVX            : 832 bytes
//   • XSAVE + AVX + AVX-512  : 2688 bytes
//
// Alignement obligatoire : 64 bytes (requis par XSAVE instruction).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicUsize, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Détection de la taille XSAVE au boot
// ─────────────────────────────────────────────────────────────────────────────

/// Taille de la zone XSAVE courante — détectée au boot via CPUID.
/// Initialisée à 512 (FXSAVE incompressible), mise à jour par detect_xsave_size().
pub static XSAVE_AREA_SIZE: AtomicUsize = AtomicUsize::new(512);

/// Taille minimale (FXSAVE, toujours disponible sur x86_64).
pub const FXSAVE_SIZE: usize = 512;
/// Taille avec XSAVE + AVX.
pub const XSAVE_AVX_SIZE: usize = 832;
/// Taille avec XSAVE + AVX-512.
pub const XSAVE_AVX512_SIZE: usize = 2688;
/// Taille maximale supportée — doit correspondre à FpuState::MAX_SIZE.
pub const FPU_STATE_MAX_SIZE: usize = 2688;

/// Détecte la taille de la zone XSAVE via CPUID et initialise XSAVE_AREA_SIZE.
/// Appelé depuis scheduler::init(), step 3.
pub fn detect_xsave_size() {
    #[cfg(target_arch = "x86_64")]
    {
        // CPUID.(EAX=0Dh, ECX=0):EBX retourne la taille de la zone XSAVE.
        let size: usize;
        // SAFETY: CPUID est disponible sur tout x86_64, aucun effet de bord mémoire.
        // rbx ne peut pas être utilisé directement comme opérande inline asm dans LLVM,
        // on le sauvegarde/restaure manuellement autour de CPUID.
        unsafe {
            let cpuid_eax: u32;
            let cpuid_ebx: u32;
            core::arch::asm!(
                "push rbx",
                "cpuid",
                "mov {out_ebx:e}, ebx",
                "pop rbx",
                inout("eax") 0x0Du32 => cpuid_eax,
                inout("ecx") 0u32 => _,
                out("edx") _,
                out_ebx = lateout(reg) cpuid_ebx,
            );
            let _ = cpuid_eax; // EAX ignoré, seul EBX nous intéresse.
            // ebx = taille minimale de la zone XCSR pour sauvegarder tous les composants actifs.
            if cpuid_ebx >= 512 {
                size = (cpuid_ebx as usize).min(FPU_STATE_MAX_SIZE);
            } else {
                // CPUID non supporté ou résultat invalide → FXSAVE par défaut.
                size = FXSAVE_SIZE;
            }
        }
        XSAVE_AREA_SIZE.store(size, Ordering::Release);
    }
    // Sur architecture non-x86_64 : taille fixe 512.
}

// ─────────────────────────────────────────────────────────────────────────────
// FpuState — zone alignée pour XSAVE/FXSAVE
// ─────────────────────────────────────────────────────────────────────────────

/// Zone de sauvegarde FPU/SIMD par thread — 2688 bytes, alignée 64 bytes.
///
/// La taille effective utilisée est dans XSAVE_AREA_SIZE (512 à 2688).
/// Le reste est du padding non utilisé (zone inerte).
///
/// RÈGLE : allouée en dehors du TCB (TCB = 128 bytes fixe).
///         Le TCB stocke uniquement `fpu_state_ptr: *mut u8`.
#[repr(C, align(64))]
pub struct FpuState {
    /// Données brutes FXSAVE/XSAVE — format dicté par le CPU.
    /// Les 512 premiers bytes sont toujours FXSAVE-compatible.
    data: [u8; FPU_STATE_MAX_SIZE],
    /// Taille réellement utilisée (copie de XSAVE_AREA_SIZE au moment de l'alloc).
    pub active_size: usize,
    /// Numéro de génération pour détecter les sauvegardes obsolètes (debug).
    pub generation:  u64,
}

impl FpuState {
    /// Crée une zone FPU vierge (état initial du processeur x86_64 après FINIT).
    pub const fn new() -> Self {
        let mut data = [0u8; FPU_STATE_MAX_SIZE];
        // Initialisation FXSAVE par défaut :
        // FCW = 0x037F (masque toutes les exceptions FP sauf invalide)
        // MXCSR = 0x1F80 (mode arrondi = round-to-nearest, flush-to-zero off)
        data[0] = 0x7F;  // FCW low
        data[1] = 0x03;  // FCW high
        data[24] = 0x80; // MXCSR low  : 0x1F80
        data[25] = 0x1F; // MXCSR high
        Self {
            data,
            active_size: FXSAVE_SIZE,
            generation:  0,
        }
    }

    /// Retourne un pointeur brut vers la zone de données pour XSAVE/XRSTOR.
    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.data.as_mut_ptr()
    }

    /// Retourne un pointeur const pour XRSTOR.
    #[inline(always)]
    pub fn as_ptr(&self) -> *const u8 {
        self.data.as_ptr()
    }

    /// Met à jour la taille active depuis XSAVE_AREA_SIZE (appelé au premier save).
    #[inline(always)]
    pub fn refresh_size(&mut self) {
        self.active_size = XSAVE_AREA_SIZE.load(Ordering::Relaxed);
    }

    /// Retourne la taille active en bytes.
    #[inline(always)]
    pub fn size(&self) -> usize {
        self.active_size
    }
}

impl Default for FpuState {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: FpuState est une structure purement de données, sans pointeurs internes.
// Elle peut être transférée entre CPUs via le scheduler (un seul CPU à la fois l'utilise).
unsafe impl Send for FpuState {}
