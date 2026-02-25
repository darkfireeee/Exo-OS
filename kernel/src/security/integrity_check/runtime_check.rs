// kernel/src/security/integrity_check/runtime_check.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Runtime Integrity Check — Vérification périodique des sections kernel
// ═══════════════════════════════════════════════════════════════════════════════
//
// Architecture :
//   • Hash BLAKE3 des sections .text et .rodata calculé au boot
//   • Vérification périodique depuis un timer kernel (ou à la demande)
//   • En cas d'altération détectée → kernel panic immédiat
//   • Instrumentation : nombre de vérifications, timestamp dernière vérification
//
// RÈGLE RUNTIME-01 : Les hashes initiaux sont calculés AVANT d'activer les IRQ.
// RÈGLE RUNTIME-02 : Une altération détectée → kernel_panic() IMMÉDIAT, pas de retry.
// RÈGLE RUNTIME-03 : Les adresses .text/.rodata viennent du linker script (externs).
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use super::super::crypto::blake3::blake3_hash;

// ─────────────────────────────────────────────────────────────────────────────
// Symboles du linker script (kernel/linker.ld)
// ─────────────────────────────────────────────────────────────────────────────

// SAFETY: Ces symboles sont définis dans kernel/linker.ld et sont toujours
// présents dans un kernel correctement compilé. Les adresses sont dans la
// section de code en lecture seule — aucun accès concurrent n'est possible
// puisque .text/.rodata ne sont jamais écrits après le chargement.
extern "C" {
    static _text_start:   u8;
    static _text_end:     u8;
    static _rodata_start: u8;
    static _rodata_end:   u8;
}

// ─────────────────────────────────────────────────────────────────────────────
// Erreurs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum IntegrityError {
    /// Section .text altérée.
    TextSectionCorrupted,
    /// Section .rodata altérée.
    RodataSectionCorrupted,
    /// Section stack guard altérée.
    StackGuardCorrupted,
    /// Non initialisé.
    NotInitialized,
}

// ─────────────────────────────────────────────────────────────────────────────
// RuntimeIntegrityChecker — état des vérifications
// ─────────────────────────────────────────────────────────────────────────────

struct RuntimeIntegrityState {
    /// Hash de référence de la section .text.
    text_hash:   [u8; 32],
    /// Hash de référence de la section .rodata.
    rodata_hash: [u8; 32],
    /// Initialisé ?
    initialized: bool,
    /// Nombre de vérifications réussies.
    checks_ok:   u64,
    /// Dernier timestamp TSC de vérification réussie.
    last_ok_tsc: u64,
}

impl RuntimeIntegrityState {
    const fn new() -> Self {
        Self {
            text_hash:   [0u8; 32],
            rodata_hash: [0u8; 32],
            initialized: false,
            checks_ok:   0,
            last_ok_tsc: 0,
        }
    }
}

static INTEGRITY_STATE: spin::Mutex<RuntimeIntegrityState> =
    spin::Mutex::new(RuntimeIntegrityState::new());
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static CHECKS_PERFORMED: AtomicU64 = AtomicU64::new(0);
static VIOLATIONS_DETECTED: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires pour les sections kernel
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le slice de la section .text.
unsafe fn text_section() -> &'static [u8] {
    // SAFETY: Les symboles _text_start/_text_end sont définis par le linker script.
    // La section .text est valide et de taille correcte. On lit en lecture seule.
    let start = &_text_start as *const u8;
    let end   = &_text_end   as *const u8;
    let len   = end as usize - start as usize;
    core::slice::from_raw_parts(start, len)
}

/// Retourne le slice de la section .rodata.
unsafe fn rodata_section() -> &'static [u8] {
    // SAFETY: Identique à text_section() — symboles linker, lecture seule.
    let start = &_rodata_start as *const u8;
    let end   = &_rodata_end   as *const u8;
    let len   = end as usize - start as usize;
    core::slice::from_raw_parts(start, len)
}

/// Lit le TSC courant.
#[cfg(target_arch = "x86_64")]
fn read_tsc() -> u64 {
    // SAFETY: RDTSC est une instruction de lecture (aucun effet de bord sur la mémoire).
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
        ((hi as u64) << 32) | (lo as u64)
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn read_tsc() -> u64 { 0 }

// ─────────────────────────────────────────────────────────────────────────────
// API publique
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise les hashes de référence du kernel.
///
/// **DOIT être appelé avant d'activer les IRQ** (RÈGLE RUNTIME-01).
/// Doit être appelé une seule fois.
pub fn init_runtime_integrity() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return; // Double-init protégée
    }
    let mut state = INTEGRITY_STATE.lock();

    // SAFETY: Voir text_section() / rodata_section() — symboles linker valides.
    let text_hash   = unsafe { blake3_hash(text_section()) };
    let rodata_hash = unsafe { blake3_hash(rodata_section()) };

    state.text_hash   = text_hash;
    state.rodata_hash = rodata_hash;
    state.initialized = true;
    state.last_ok_tsc = read_tsc();
}

/// Effectue une vérification d'intégrité des sections kernel.
///
/// Retourne Err si une altération est détectée.
/// **En production : appeler kernel_panic() en cas d'Err.**
pub fn check_kernel_integrity() -> Result<(), IntegrityError> {
    let state = INTEGRITY_STATE.lock();
    if !state.initialized {
        return Err(IntegrityError::NotInitialized);
    }

    // Calculer les hashes actuels
    // SAFETY: Voir text_section() / rodata_section().
    let current_text   = unsafe { blake3_hash(text_section()) };
    let current_rodata = unsafe { blake3_hash(rodata_section()) };

    // Comparer en temps constant
    let mut text_diff   = 0u8;
    let mut rodata_diff = 0u8;
    for i in 0..32 {
        text_diff   |= current_text[i]   ^ state.text_hash[i];
        rodata_diff |= current_rodata[i] ^ state.rodata_hash[i];
    }

    CHECKS_PERFORMED.fetch_add(1, Ordering::Relaxed);
    drop(state);

    if text_diff != 0 {
        VIOLATIONS_DETECTED.fetch_add(1, Ordering::Relaxed);
        return Err(IntegrityError::TextSectionCorrupted);
    }
    if rodata_diff != 0 {
        VIOLATIONS_DETECTED.fetch_add(1, Ordering::Relaxed);
        return Err(IntegrityError::RodataSectionCorrupted);
    }

    // Mise à jour du timestamp de succès
    let mut state = INTEGRITY_STATE.lock();
    state.checks_ok   += 1;
    state.last_ok_tsc  = read_tsc();
    Ok(())
}

/// Vérifie l'intégrité et panics si compromise.
pub fn assert_kernel_integrity() {
    if let Err(e) = check_kernel_integrity() {
        // En no_std kernel : panic avec message d'erreur
        panic!("KERNEL INTEGRITY VIOLATION: {:?}", e);
    }
}

/// Retourne vrai si le système d'intégrité est initialisé.
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

#[derive(Debug, Clone, Copy)]
pub struct IntegrityStats {
    pub checks_performed:   u64,
    pub violations_detected: u64,
    pub last_ok_tsc:        u64,
}

pub fn integrity_stats() -> IntegrityStats {
    let state = INTEGRITY_STATE.lock();
    IntegrityStats {
        checks_performed:    CHECKS_PERFORMED.load(Ordering::Relaxed),
        violations_detected: VIOLATIONS_DETECTED.load(Ordering::Relaxed),
        last_ok_tsc:         state.last_ok_tsc,
    }
}
