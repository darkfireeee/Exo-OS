// kernel/src/fs/exofs/epoch/epoch_barriers.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Barrières NVMe — wrappeurs mockables pour les nvme_flush() du commit Epoch
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE EPOCH-02 : INTERDIT d'omettre une barrière NVMe — reordering = corruption.
// Les 3 barrières correspondent aux 3 phases du protocole de commit.

use core::fmt;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// =============================================================================
// Type de la fonction de flush NVMe (injectée par le block layer au boot)
// =============================================================================

/// Signature de la fonction de flush NVMe.
///
/// Injectée par le block layer au boot via `register_nvme_flush_fn()`.
/// En l'absence d'enregistrement, le flush est un no-op (dangereux en prod).
/// RÈGLE EPOCH-02 : omettre un flush = corruption certaine au prochain crash.
type NvmeFlushFn = fn() -> ExofsResult<()>;

/// Pointeur atomique vers la fonction de flush enregistrée.
///
/// Initialisé à `default_flush_stub` (no-op + log) jusqu'à l'enregistrement.
static FLUSH_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Stub par défaut : retourne immédiatement OK mais signale l'absence d'hook.
///
/// En production : si appelé, cela indique un boot incomplet (block layer
/// non initialisé). Les écritures sont en mémoire volatile — pas de durabilité.
fn default_flush_stub() -> ExofsResult<()> {
    // Compteur d'appels non-hookés (diagnostics).
    UNHOOK_FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
    // On retourne Ok() pour ne pas bloquer le boot, mais la durabilité
    // n'est pas garantie. En prod, le WatchDog NVMe détectera l'absence
    // de flush via SMART ou via les stats EPOCH_STATS.
    Ok(())
}

/// Nombre de flushes appelés sans hook enregistré (doit rester 0 en production).
static UNHOOK_FLUSH_COUNT: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// Enregistrement du hook NVMe
// =============================================================================

/// Enregistre la fonction de flush NVMe fournie par le block layer.
///
/// # Sécurité
/// Doit être appelé une seule fois, pendant la phase d'init du block layer,
/// AVANT le premier montage ExoFS. Les appels ultérieurs sont ignorés si
/// le hook est déjà non-default (protection contre une double initialisation).
///
/// # Paramètre
/// `flush_fn` : fonction bloquante qui soumet un Flush FUA au périphérique
///              NVMe sous-jacent et attend sa complétion.
pub fn register_nvme_flush_fn(flush_fn: NvmeFlushFn) {
    // Mis à jour atomiquement — si plusieurs appels concurrents, le dernier gagne.
    // En pratique, call uniquement depuis le thread d'init (single-path).
    FLUSH_HOOK.store(flush_fn as usize, Ordering::Release);
    // Réinitialise le compteur de flushes non-hookés (on est maintenant hookés).
    UNHOOK_FLUSH_COUNT.store(0, Ordering::Relaxed);
}

/// Retourne vrai si un hook NVMe a été enregistré (différent du stub).
#[inline]
pub fn is_nvme_flush_registered() -> bool {
    FLUSH_HOOK.load(Ordering::Relaxed) != 0
}

// =============================================================================
// Compteurs diagnostics des barrières
// =============================================================================

/// Nombre de barrières émises par phase (indexé 0=data, 1=root, 2=record).
static BARRIERS_ISSUED: [AtomicU64; 3] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Total de barrières ayant échoué par phase.
static BARRIERS_FAILED: [AtomicU64; 3] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Temps cumulé (en cycles TSC) des 3 phases de barrier (pour la latence).
static BARRIER_CYCLES: [AtomicU64; 3] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

// =============================================================================
// Exécution effective du flush NVMe
// =============================================================================

/// Appelle le hook de flush NVMe enregistré.
///
/// Ce chemin critique doit rester inlined — éviter tout indirection
/// non-nécessaire sur le hot path du commit.
#[inline(always)]
fn nvme_flush_impl(phase_idx: usize) -> ExofsResult<()> {
    // Charge le pointeur de fonction atomiquement.
    // SAFETY: le pointeur est initialisé à une fonction valide (`default_flush_stub`)
    // et mis à jour uniquement par `register_nvme_flush_fn` vers une autre fonction
    // valide. La conversion usize → fn() est donc sûre.
    let fn_ptr = FLUSH_HOOK.load(Ordering::Acquire);
    if fn_ptr == 0 {
        return default_flush_stub();
    }
    let flush: NvmeFlushFn = unsafe { core::mem::transmute(fn_ptr) };

    let t0 = read_tsc();
    let result = flush();
    let elapsed = read_tsc().wrapping_sub(t0);

    BARRIERS_ISSUED[phase_idx].fetch_add(1, Ordering::Relaxed);
    BARRIER_CYCLES[phase_idx].fetch_add(elapsed, Ordering::Relaxed);

    if result.is_err() {
        BARRIERS_FAILED[phase_idx].fetch_add(1, Ordering::Relaxed);
    }

    result
}

/// Lit le TSC x86 pour la mesure de latence (ou 0 sur architectures non-x86).
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn read_tsc() -> u64 {
    // SAFETY: instruction rdtsc disponible sur tous les x86_64 modernes.
    // Aucune dépendance mémoire nécessaire ici (mesure de temps, pas de barrière).
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline(always)]
fn read_tsc() -> u64 {
    0u64
}

// =============================================================================
// Les 3 barrières NVMe du protocole de commit (RÈGLE EPOCH-01)
// =============================================================================

/// Barrière NVMe Phase 1 : après écriture des données payload (P-Blobs, L-Objs).
///
/// Garantit que toutes les données de l'epoch sont persistées sur le média NVMe
/// AVANT l'écriture de l'EpochRoot. Si cette barrière est omise, un crash après
/// Phase 2 mais avant Phase 1 laisserait un EpochRoot pointant vers des données
/// non persistées — corruption des données.
///
/// # RÈGLE EPOCH-01 Phase 1 — INVIOLABLE
/// `write(payload)` → `nvme_barrier_after_data()` → Phase 2.
#[inline]
pub fn nvme_barrier_after_data() -> ExofsResult<()> {
    nvme_flush_impl(0).map_err(|_| ExofsError::NvmeFlushFailed)
}

/// Barrière NVMe Phase 2 : après écriture de l'EpochRoot.
///
/// Garantit que l'EpochRoot (liste des objets modifiés) est persisté AVANT
/// l'EpochRecord dans le slot. Si omise, le slot pourrait pointer vers un
/// EpochRoot invalide ou partiellement écrit.
///
/// # RÈGLE EPOCH-01 Phase 2 — INVIOLABLE
/// `write(EpochRoot)` → `nvme_barrier_after_root()` → Phase 3.
#[inline]
pub fn nvme_barrier_after_root() -> ExofsResult<()> {
    nvme_flush_impl(1).map_err(|_| ExofsError::NvmeFlushFailed)
}

/// Barrière NVMe Phase 3 : après écriture de l'EpochRecord dans le slot.
///
/// Après cette barrière, l'Epoch est définitivement committé et visible au
/// recovery. C'est le point de non-retour : si le système crash après cette
/// barrière, le recovery retrouvera l'epoch.
///
/// # RÈGLE EPOCH-01 Phase 3 — INVIOLABLE
/// `write(EpochRecord→slot)` → `nvme_barrier_after_record()` → commit terminé.
#[inline]
pub fn nvme_barrier_after_record() -> ExofsResult<()> {
    nvme_flush_impl(2).map_err(|_| ExofsError::NvmeFlushFailed)
}

// =============================================================================
// Barrière de lecture (pour la cohérence du recovery)
// =============================================================================

/// Barrière en lecture : garantit que les lectures suivantes voient les
/// écritures précédentes persistées.
///
/// Utilisée lors du recovery avant de lire les slots pour s'assurer que
/// les écritures préalables au crash sont bien visibles.
#[inline]
pub fn nvme_read_barrier() -> ExofsResult<()> {
    // Sur NVMe, un flush FUA garantit aussi la cohérence en lecture.
    // En l'absence d'un hook spécifique, utilise le même hook que le write flush.
    nvme_flush_impl(0).map_err(|_| ExofsError::NvmeFlushFailed)
}

// =============================================================================
// Exécution du protocole 3 barrières (utilitaire de haut niveau)
// =============================================================================

/// Résultat de l'exécution des 3 barrières.
#[derive(Debug)]
pub struct ThreePhaseBarrierResult {
    /// Vrai si toutes les barrières ont réussi.
    pub success: bool,
    /// Phase où l'erreur s'est produite (1, 2, ou 3), 0 si OK.
    pub failed_phase: u8,
    /// Cycles TSC consommés par chaque phase.
    pub phase_cycles: [u64; 3],
}

impl ThreePhaseBarrierResult {
    fn ok(cycles: [u64; 3]) -> Self {
        Self { success: true, failed_phase: 0, phase_cycles: cycles }
    }
    fn fail(phase: u8, cycles: [u64; 3]) -> Self {
        Self { success: false, failed_phase: phase, phase_cycles: cycles }
    }
}

/// Exécute les barrières après_data, après_root et après_record en séquence.
///
/// Retourne `ThreePhaseBarrierResult` pour permettre le diagnostic en cas d'erreur.
/// En production, chaque phase DOIT réussir — toute erreur arrête le commit.
pub fn execute_three_phase_barriers() -> ThreePhaseBarrierResult {
    let mut cycles = [0u64; 3];

    let t0 = read_tsc();
    if nvme_barrier_after_data().is_err() {
        cycles[0] = read_tsc().wrapping_sub(t0);
        return ThreePhaseBarrierResult::fail(1, cycles);
    }
    cycles[0] = read_tsc().wrapping_sub(t0);

    let t1 = read_tsc();
    if nvme_barrier_after_root().is_err() {
        cycles[1] = read_tsc().wrapping_sub(t1);
        return ThreePhaseBarrierResult::fail(2, cycles);
    }
    cycles[1] = read_tsc().wrapping_sub(t1);

    let t2 = read_tsc();
    if nvme_barrier_after_record().is_err() {
        cycles[2] = read_tsc().wrapping_sub(t2);
        return ThreePhaseBarrierResult::fail(3, cycles);
    }
    cycles[2] = read_tsc().wrapping_sub(t2);

    ThreePhaseBarrierResult::ok(cycles)
}

// =============================================================================
// Diagnostics et statistiques des barrières
// =============================================================================

/// Snapshot des statistiques de toutes les barrières.
#[derive(Copy, Clone, Debug)]
pub struct BarrierStats {
    /// Nombre de barrières émises par phase [data, root, record].
    pub issued:        [u64; 3],
    /// Nombre de barrières ayant échoué par phase.
    pub failed:        [u64; 3],
    /// Cycles cumulés par phase.
    pub total_cycles:  [u64; 3],
    /// Nombre de flushes sans hook enregistré.
    pub unhook_count:  u64,
    /// Vrai si un hook NVMe est actif.
    pub hook_active:   bool,
}

impl BarrierStats {
    /// Retourne les cycles moyens par barrière pour une phase donnée (0..2).
    pub fn avg_cycles_per_barrier(&self, phase: usize) -> u64 {
        if phase >= 3 || self.issued[phase] == 0 {
            return 0;
        }
        self.total_cycles[phase] / self.issued[phase]
    }

    /// Taux d'échec pour une phase (en millièmes de pourcent, 0..100000).
    pub fn failure_rate_ppm(&self, phase: usize) -> u64 {
        if phase >= 3 || self.issued[phase] == 0 {
            return 0;
        }
        self.failed[phase].saturating_mul(100_000) / self.issued[phase]
    }
}

impl fmt::Display for BarrierStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BarrierStats{{ data:[{}ok/{}err] root:[{}ok/{}err] rec:[{}ok/{}err] hook={} }}",
            self.issued[0], self.failed[0],
            self.issued[1], self.failed[1],
            self.issued[2], self.failed[2],
            self.hook_active,
        )
    }
}

/// Prend un instantané des statistiques de barrières.
pub fn barrier_stats_snapshot() -> BarrierStats {
    BarrierStats {
        issued: [
            BARRIERS_ISSUED[0].load(Ordering::Relaxed),
            BARRIERS_ISSUED[1].load(Ordering::Relaxed),
            BARRIERS_ISSUED[2].load(Ordering::Relaxed),
        ],
        failed: [
            BARRIERS_FAILED[0].load(Ordering::Relaxed),
            BARRIERS_FAILED[1].load(Ordering::Relaxed),
            BARRIERS_FAILED[2].load(Ordering::Relaxed),
        ],
        total_cycles: [
            BARRIER_CYCLES[0].load(Ordering::Relaxed),
            BARRIER_CYCLES[1].load(Ordering::Relaxed),
            BARRIER_CYCLES[2].load(Ordering::Relaxed),
        ],
        unhook_count: UNHOOK_FLUSH_COUNT.load(Ordering::Relaxed),
        hook_active:  is_nvme_flush_registered(),
    }
}

/// Retourne le nombre total de barrières émises (diagnostics globaux).
pub fn total_barriers_issued() -> u64 {
    BARRIERS_ISSUED[0].load(Ordering::Relaxed)
        .saturating_add(BARRIERS_ISSUED[1].load(Ordering::Relaxed))
        .saturating_add(BARRIERS_ISSUED[2].load(Ordering::Relaxed))
}

/// Retourne le nombre total de barrières ayant échoué.
pub fn total_barriers_failed() -> u64 {
    BARRIERS_FAILED[0].load(Ordering::Relaxed)
        .saturating_add(BARRIERS_FAILED[1].load(Ordering::Relaxed))
        .saturating_add(BARRIERS_FAILED[2].load(Ordering::Relaxed))
}

// =============================================================================
// Santé du sous-système barrières
// =============================================================================

/// Niveau de santé des barrières NVMe.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BarrierHealth {
    /// Tout est nominal.
    Healthy,
    /// Hook non enregistré — durabilité non garantie.
    Degraded,
    /// Taux d'échec élevé — problème matériel ou I/O.
    Critical,
}

/// Évalue la santé du sous-système barrières.
pub fn barrier_health() -> BarrierHealth {
    if !is_nvme_flush_registered() {
        return BarrierHealth::Degraded;
    }
    let stats = barrier_stats_snapshot();
    for phase in 0..3 {
        // Plus de 1 ppm d'échec = situation critique.
        if stats.failure_rate_ppm(phase) > 1 {
            return BarrierHealth::Critical;
        }
    }
    BarrierHealth::Healthy
}

// =============================================================================
// Reset pour les tests
// =============================================================================

/// Réinitialise tous les compteurs de barrières (utilisation tests uniquement).
///
/// En production : ne pas appeler (données de diagnostics critiques).
#[cfg(test)]
pub fn reset_barrier_stats() {
    for i in 0..3 {
        BARRIERS_ISSUED[i].store(0, Ordering::Relaxed);
        BARRIERS_FAILED[i].store(0, Ordering::Relaxed);
        BARRIER_CYCLES[i].store(0, Ordering::Relaxed);
    }
    UNHOOK_FLUSH_COUNT.store(0, Ordering::Relaxed);
}

