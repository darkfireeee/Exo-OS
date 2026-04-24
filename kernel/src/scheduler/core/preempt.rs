// kernel/src/scheduler/core/preempt.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// PRÉEMPTION RAII — PreemptGuard (Exo-OS Scheduler · Couche 1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE PREEMPT-01 (DOC3) : Ne jamais appeler preempt_disable/enable directement.
//   Toujours passer par PreemptGuard pour garantir une réactivation par Drop.
//   Raison : une exception entre disable et enable corromprait le compteur de
//   préemption et provoquerait un deadlock ou un crash du scheduler.
//
// RÈGLE ZONE NO-ALLOC : ce fichier ne doit jamais allouer de mémoire heap.
// RÈGLE UNSAFE : tout bloc unsafe documenté par // SAFETY:
// ═══════════════════════════════════════════════════════════════════════════════

use core::marker::PhantomData;
use core::sync::atomic::{AtomicI32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Compteur de préemption per-CPU
//
// Stocké ici en tant que static per-CPU simulé par un tableau aligné.
// En production sur un vrai noyau, ce serait un champ dans la per-CPU data
// structure, accédé via GS:offset. On utilise une approximation tableaux
// statiques pour un maximum de MAX_CPUS CPUs.
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximal de CPUs supportés.
/// CORR-27 : doit être ≥ SSR_MAX_CORES_LAYOUT (256) — Phase 0 obligatoire.
/// ÉTAIT : 64 — CORRIGÉ : 256
pub const MAX_CPUS: usize = 256;

/// Compteurs de préemption per-CPU — un compteur par CPU logique.
///
/// Valeur 0 : préemptible.
/// Valeur > 0 : non préemptible (profondeur d'imbrication).
/// Valeur < 0 : BUG — déséquilibre enable/disable (détecté en debug).
#[repr(C, align(64))]
struct PreemptCounter(AtomicI32, [u8; 60]); // padding cache line

static PREEMPT_COUNT: [PreemptCounter; MAX_CPUS] = {
    // SAFETY: zero-initialization correcte pour AtomicI32(0) + padding.
    const ZERO: PreemptCounter = PreemptCounter(AtomicI32::new(0), [0u8; 60]);
    [ZERO; MAX_CPUS]
};

/// Compteur global de désactivations de préemption (instrumentation).
static PREEMPT_DISABLE_TOTAL: AtomicI32 = AtomicI32::new(0);

/// Retourne l'ID du CPU courant.
/// Dans un vrai noyau x86_64, on lirait GS:percpu_cpu_id.
/// Ici on appelle une fonction externe fournie par arch/.
#[inline(always)]
fn current_cpu_id() -> usize {
    // SAFETY: lecture d'une variable per-CPU via l'ABI noyau.
    // La valeur est toujours dans [0, MAX_CPUS).
    #[cfg(target_arch = "x86_64")]
    {
        // Lecture du champ cpu_id dans le per-CPU data (GS:0).
        // En l'absence du mécanisme complet, on retourne 0.
        extern "C" {
            fn arch_current_cpu() -> u32;
        }
        // SAFETY: arch_current_cpu() est défini dans arch/x86_64 et retourne
        // toujours une valeur dans [0, MAX_CPUS).
        let id = unsafe { arch_current_cpu() } as usize;
        id.min(MAX_CPUS - 1)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// API interne de bas niveau (PRIVÉE)
// ─────────────────────────────────────────────────────────────────────────────

/// Désactive la préemption pour le CPU courant (incrémente compteur).
/// PRIVÉ — utiliser PreemptGuard.
#[inline(always)]
fn preempt_disable_raw() {
    let cpu = current_cpu_id();
    let prev = PREEMPT_COUNT[cpu].0.fetch_add(1, Ordering::Acquire);
    PREEMPT_DISABLE_TOTAL.fetch_add(1, Ordering::Relaxed);
    // Détection de débordement positif > 64 → signe d'un bug d'imbrication.
    debug_assert!(
        prev < 64,
        "preempt_disable: compteur > 64, bug d'imbrication probable"
    );
}

/// Réactive la préemption pour le CPU courant (décrémente compteur).
/// PRIVÉ — utiliser PreemptGuard::drop.
#[inline(always)]
fn preempt_enable_raw() {
    let cpu = current_cpu_id();
    let prev = PREEMPT_COUNT[cpu].0.fetch_sub(1, Ordering::Release);
    debug_assert!(
        prev > 0,
        "preempt_enable: compteur était <= 0 — déséquilibre disable/enable FATAL"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// PreemptGuard — RAII obligatoire
// ─────────────────────────────────────────────────────────────────────────────

/// Désactive la préemption pour la durée de vie de ce guard.
///
/// # Exemple
/// ```rust,ignore
/// {
///     let _guard = PreemptGuard::new();  // préemption désactivée
///     // ... section critique vis-à-vis du scheduler ...
///     // Drop automatique → préemption réactivée
/// }
/// ```
///
/// # Règles d'utilisation
/// - Ne JAMAIS stocker un `PreemptGuard` dans un champ de structure persistante.
/// - Ne JAMAIS transférer un `PreemptGuard` entre threads (non `Send`).
/// - Durée **minimale** : juste assez pour protéger la section critique.
/// - INTERDIT d'appeler des fonctions pouvant dormir à l'intérieur.
pub struct PreemptGuard {
    /// PhantomData *mut () rend ce type non-Send et non-Sync.
    _phantom: PhantomData<*mut ()>,
}

impl PreemptGuard {
    /// Désactive la préemption et retourne un guard RAII.
    #[inline(always)]
    #[must_use = "PreemptGuard doit être stocké dans une variable locale pour avoir un effet"]
    pub fn new() -> Self {
        preempt_disable_raw();
        Self {
            _phantom: PhantomData,
        }
    }

    /// Vérifie si la préemption est actuellement désactivée sur ce CPU.
    #[inline(always)]
    pub fn is_preempted_disabled() -> bool {
        let cpu = current_cpu_id();
        PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed) > 0
    }

    /// Retourne la profondeur d'imbrication actuelle (debug).
    #[inline(always)]
    pub fn depth() -> i32 {
        let cpu = current_cpu_id();
        PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed)
    }
}

impl Drop for PreemptGuard {
    #[inline(always)]
    fn drop(&mut self) {
        preempt_enable_raw();
    }
}

// PreemptGuard est !Send (PhantomData<*mut ()>), !Sync implicite.
// Pas de send/sync impl intentionnel.

// ─────────────────────────────────────────────────────────────────────────────
// IrqGuard — désactive les interruptions + préemption (sections IRQ-safe)
// ─────────────────────────────────────────────────────────────────────────────

/// Désactive les IRQ matérielles ET la préemption.
/// Utilisé pour les spinlocks IRQ-safe (scheduler tick, IPI handlers).
pub struct IrqGuard {
    /// Flags RFLAGS sauvegardés (IF bit).
    rflags: u64,
    _phantom: PhantomData<*mut ()>,
}

impl IrqGuard {
    /// Sauvegarde RFLAGS, coupe les IRQ, désactive la préemption.
    #[inline(always)]
    #[must_use]
    pub fn new() -> Self {
        let rflags: u64;
        #[cfg(target_arch = "x86_64")]
        // SAFETY: pushfq/popq lisent RFLAGS; cli coupe les IRQ; ordre correct (sauvegarder avant couper).
        unsafe {
            core::arch::asm!(
                "pushfq",
                "popq {flags}",
                "cli",
                flags = out(reg) rflags,
                options(nomem)
            );
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            rflags = 0;
        }

        preempt_disable_raw();
        Self {
            rflags,
            _phantom: PhantomData,
        }
    }

    /// Vrai si les IRQ étaient activées avant ce guard (bit IF = 9 de RFLAGS).
    #[inline(always)]
    pub fn irqs_were_enabled(&self) -> bool {
        self.rflags & (1 << 9) != 0
    }
}

impl Drop for IrqGuard {
    #[inline(always)]
    fn drop(&mut self) {
        preempt_enable_raw();
        if self.rflags & (1 << 9) != 0 {
            #[cfg(target_arch = "x86_64")]
            // SAFETY: sti restaure le bit IF; rflags provient de pushfq/popq dans new(); restauration atomique.
            unsafe {
                core::arch::asm!("sti", options(nomem, nostack));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialisation
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise les compteurs de préemption per-CPU pour tous les CPUs.
/// Appelé depuis la séquence d'initialisation du scheduler (step 1).
pub fn init() {
    for i in 0..MAX_CPUS {
        PREEMPT_COUNT[i].0.store(0, Ordering::SeqCst);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Assertions de cohérence (vérifiées en mode debug)
// ─────────────────────────────────────────────────────────────────────────────

/// Panique si la préemption n'est pas désactivée sur le CPU courant.
/// Utilisé pour vérifier les invariants dans le scheduler.
#[inline(always)]
pub fn assert_preempt_disabled() {
    assert!(
        PreemptGuard::is_preempted_disabled(),
        "Assertion préemption désactivée : ÉCHEC — appel hors section protégée"
    );
}

/// Panique si la préemption est désactivée (pour vérifier dormance autorisée).
#[inline(always)]
pub fn assert_preempt_enabled() {
    assert!(
        !PreemptGuard::is_preempted_disabled(),
        "Assertion préemption activée : ÉCHEC — can_sleep() hors section dormable"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Instrumentation
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le nombre total de fois que la préemption a été désactivée
/// depuis le boot (toutes CPUs confondues, instrumentation).
pub fn total_preempt_disable_count() -> i32 {
    PREEMPT_DISABLE_TOTAL.load(Ordering::Relaxed)
}
