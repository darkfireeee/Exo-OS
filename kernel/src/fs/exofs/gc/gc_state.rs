// kernel/src/fs/exofs/gc/gc_state.rs
//
// ==============================================================================
// Machine d'etats du Garbage Collector ExoFS
// Ring 0 . no_std . Exo-OS
//
// Etats valides : Idle -> Scanning -> Marking -> Sweeping -> Finalizing -> Idle
//
// Conformite :
//   GC-05  : GC toujours en arriere-plan, jamais dans le chemin critique
//   GC-03  : grey_queue bornee a MAX_GC_GREY_QUEUE
//   DEAD-01: jamais EPOCH_COMMIT_LOCK depuis le GC
// ==============================================================================


use core::fmt;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::fs::exofs::core::{EpochId, ExofsError, ExofsResult};
use crate::scheduler::sync::spinlock::SpinLock;

// ==============================================================================
// Constantes
// ==============================================================================

/// Version du protocole GC — increment si la machine d'etats change.
pub const GC_STATE_VERSION: u32 = 1;

/// Timeout maximum d'une passe GC en nombre de "ticks" logiques.
/// Un tick = une iteration de la boucle principale du GC thread.
pub const GC_MAX_PASS_TICKS: u64 = 10_000_000;

/// Nombre maximum de passes GC consecutives sans retour a Idle.
pub const GC_MAX_CONSECUTIVE_PASSES: u32 = 8;

// ==============================================================================
// GcPhase — phase courante de la passe GC
// ==============================================================================

/// Phase courante du cycle GC.
///
/// Transitions valides :
///   Idle -> Scanning -> Marking -> Sweeping -> Finalizing -> Idle
///   n'importe quelle phase -> Aborted (sur erreur fatale)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GcPhase {
    /// Pas de passe active.
    Idle       = 0,
    /// Scan des EpochRoots pour construire la file grise initiale.
    Scanning   = 1,
    /// Phase de marquage tricolore (Blanc/Gris/Noir).
    Marking    = 2,
    /// Phase de balayage : collecte des blobs blancs.
    Sweeping   = 3,
    /// Phase finale : orphelins, deferred_delete, mise a jour metriques.
    Finalizing = 4,
    /// Passe interrompue (erreur ou shutdown).
    Aborted    = 5,
}

impl GcPhase {
    /// Convertit depuis u8 (pour la valeur atomique).
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => GcPhase::Idle,
            1 => GcPhase::Scanning,
            2 => GcPhase::Marking,
            3 => GcPhase::Sweeping,
            4 => GcPhase::Finalizing,
            5 => GcPhase::Aborted,
            _ => GcPhase::Aborted,
        }
    }

    /// Retourne true si une passe est active.
    pub fn is_active(self) -> bool {
        !matches!(self, GcPhase::Idle | GcPhase::Aborted)
    }

    /// Retourne le nom lisible de la phase.
    pub fn name(self) -> &'static str {
        match self {
            GcPhase::Idle       => "Idle",
            GcPhase::Scanning   => "Scanning",
            GcPhase::Marking    => "Marking",
            GcPhase::Sweeping   => "Sweeping",
            GcPhase::Finalizing => "Finalizing",
            GcPhase::Aborted    => "Aborted",
        }
    }

    /// Valide la transition vers la phase suivante.
    pub fn can_transition_to(self, next: GcPhase) -> bool {
        matches!(
            (self, next),
            (GcPhase::Idle,       GcPhase::Scanning)
            | (GcPhase::Scanning,   GcPhase::Marking)
            | (GcPhase::Marking,    GcPhase::Sweeping)
            | (GcPhase::Sweeping,   GcPhase::Finalizing)
            | (GcPhase::Finalizing, GcPhase::Idle)
            | (_, GcPhase::Aborted)
        )
    }
}

impl fmt::Display for GcPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ==============================================================================
// GcPassStats — statistiques d'une passe GC
// ==============================================================================

/// Statistiques collectees durant une passe GC.
#[derive(Debug, Default, Clone)]
pub struct GcPassStats {
    /// Epoch analysee.
    pub epoch:             u64,
    /// Blobs scannes lors de la phase Scanning.
    pub blobs_scanned:     u64,
    /// Blobs marques vivants (noir) a l'issue du marquage.
    pub blobs_marked_live: u64,
    /// Blobs collectes (blancs a la fin du marquage).
    pub blobs_swept:       u64,
    /// Octets liberes.
    pub bytes_freed:       u64,
    /// Orphelins collectes.
    pub orphans_collected: u64,
    /// Objets inline GC-es.
    pub inline_gc_count:   u64,
    /// Cycles detectes par le cycle_detector.
    pub cycles_detected:   u64,
    /// Tick de debut (logique, non arch::time).
    pub start_tick:        u64,
    /// Tick de fin.
    pub end_tick:          u64,
    /// true si la passe s'est terminee normalement.
    pub completed:         bool,
    /// Code d'abandon si completed=false.
    pub abort_reason:      Option<&'static str>,
}

impl GcPassStats {
    /// Duree en ticks logiques.
    pub fn duration_ticks(&self) -> u64 {
        self.end_tick.saturating_sub(self.start_tick)
    }

    /// Ratio de collecte en pourcent (blobs_swept / blobs_scanned * 100).
    pub fn collect_ratio_x100(&self) -> u64 {
        if self.blobs_scanned == 0 {
            return 0;
        }
        self.blobs_swept.saturating_mul(100) / self.blobs_scanned
    }
}

impl fmt::Display for GcPassStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcPassStats[epoch={} scan={} live={} swept={} freed={}B orphans={} \
             cycles={} ticks={} ok={}]",
            self.epoch,
            self.blobs_scanned,
            self.blobs_marked_live,
            self.blobs_swept,
            self.bytes_freed,
            self.orphans_collected,
            self.cycles_detected,
            self.duration_ticks(),
            self.completed,
        )
    }
}

// ==============================================================================
// GcStateInner — donnees protegees par SpinLock
// ==============================================================================

/// Donnees mutables de l'etat GC protegees par SpinLock.
struct GcStateInner {
    /// Epoch de la passe courante.
    current_epoch:      Option<EpochId>,
    /// Tick logique de debut de la passe courante.
    pass_start_tick:    u64,
    /// Statistiques de la derniere passe terminee.
    last_pass:          Option<GcPassStats>,
    /// Stats de la passe en cours (accumulees incrementalement).
    current_pass:       GcPassStats,
    /// Compteur de passes consecutives.
    consecutive_passes: u32,
    /// Tick logique global (incremente par gc_thread a chaque iteration).
    #[allow(dead_code)]
    logical_tick:       u64,
    /// Nombre total de passes effectuees depuis le boot.
    total_passes:       u64,
    /// Nombre total de passes abandonnees.
    total_aborts:       u64,
    /// Octets liberes en cumul.
    total_bytes_freed:  u64,
}

impl GcStateInner {
    const fn new() -> Self {
        Self {
            current_epoch:      None,
            pass_start_tick:    0,
            last_pass:          None,
            current_pass:       GcPassStats {
                epoch:             0,
                blobs_scanned:     0,
                blobs_marked_live: 0,
                blobs_swept:       0,
                bytes_freed:       0,
                orphans_collected: 0,
                inline_gc_count:   0,
                cycles_detected:   0,
                start_tick:        0,
                end_tick:          0,
                completed:         false,
                abort_reason:      None,
            },
            consecutive_passes: 0,
            logical_tick:       0,
            total_passes:       0,
            total_aborts:       0,
            total_bytes_freed:  0,
        }
    }
}

// ==============================================================================
// GcState — facade thread-safe
// ==============================================================================

/// Machine d'etats thread-safe du GC.
///
/// La phase est stockee dans un AtomicU8 pour lecture sans verrou.
/// Les donnees statistiques sont dans un SpinLock<GcStateInner>.
pub struct GcState {
    phase:  AtomicU8,
    inner:  SpinLock<GcStateInner>,
    /// Compteur de ticks logiques independant (incremente par le GC thread).
    tick:   AtomicU64,
}

impl GcState {
    pub const fn new() -> Self {
        Self {
            phase: AtomicU8::new(GcPhase::Idle as u8),
            inner: SpinLock::new(GcStateInner::new()),
            tick:  AtomicU64::new(0),
        }
    }

    // ── Lecture rapide sans verrou ───────────────────────────────────────────

    /// Phase courante (lecture atomique, pas de verrou).
    pub fn phase(&self) -> GcPhase {
        GcPhase::from_u8(self.phase.load(Ordering::Acquire))
    }

    /// Retourne true si une passe est active (pas Idle ni Aborted).
    pub fn is_active(&self) -> bool {
        self.phase().is_active()
    }

    /// Retourne true si le GC est en phase Idle.
    pub fn is_idle(&self) -> bool {
        matches!(self.phase(), GcPhase::Idle)
    }

    /// Tick logique courant.
    pub fn current_tick(&self) -> u64 {
        self.tick.load(Ordering::Acquire)
    }

    // ── Mutations controlees ─────────────────────────────────────────────────

    /// Incremente le tick logique. Appele par gc_thread a chaque iteration.
    pub fn advance_tick(&self) -> u64 {
        self.tick.fetch_add(1, Ordering::AcqRel).saturating_add(1)
    }

    /// Demarre une nouvelle passe GC sur `epoch`.
    ///
    /// Retourne `ExofsError::Concurrency` si une passe est deja active.
    pub fn begin_pass(&self, epoch: EpochId) -> ExofsResult<()> {
        // CAS atomique : Idle → Scanning. Echoue si pas Idle.
        let idle = GcPhase::Idle as u8;
        let scanning = GcPhase::Scanning as u8;
        self.phase
            .compare_exchange(idle, scanning, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| ExofsError::Concurrency)?;

        let tick = self.tick.load(Ordering::Acquire);
        let mut g = self.inner.lock();
        g.current_epoch = Some(epoch);
        g.pass_start_tick = tick;
        g.current_pass = GcPassStats {
            epoch:      epoch.0,
            start_tick: tick,
            ..GcPassStats::default()
        };
        g.consecutive_passes = g.consecutive_passes.saturating_add(1);
        Ok(())
    }

    /// Fait avancer la phase (Scanning -> Marking -> Sweeping -> Finalizing).
    ///
    /// Retourne `ExofsError::Logic` si la transition n'est pas valide.
    pub fn set_phase(&self, next: GcPhase) -> ExofsResult<()> {
        let current = self.phase();
        if !current.can_transition_to(next) {
            return Err(ExofsError::Logic);
        }
        self.phase.store(next as u8, Ordering::Release);
        Ok(())
    }

    /// Termine la passe en cours normalement. Retour vers Idle.
    pub fn end_pass(&self, blobs_swept: u64, bytes_freed: u64) {
        let tick = self.tick.load(Ordering::Acquire);
        {
            let mut g = self.inner.lock();
            g.current_pass.blobs_swept  = blobs_swept;
            g.current_pass.bytes_freed  = bytes_freed;
            g.current_pass.end_tick     = tick;
            g.current_pass.completed    = true;
            let stats = g.current_pass.clone();
            g.last_pass = Some(stats);
            g.total_passes = g.total_passes.saturating_add(1);
            g.total_bytes_freed = g.total_bytes_freed.saturating_add(bytes_freed);
            g.current_epoch = None;
        }
        // Retour a Idle — toujours valide depuis Finalizing ou Aborted.
        self.phase.store(GcPhase::Idle as u8, Ordering::Release);
    }

    /// Abandonne la passe en cours avec une raison textuelle.
    pub fn abort_pass(&self, reason: &'static str) {
        let tick = self.tick.load(Ordering::Acquire);
        {
            let mut g = self.inner.lock();
            g.current_pass.end_tick     = tick;
            g.current_pass.completed    = false;
            g.current_pass.abort_reason = Some(reason);
            let stats = g.current_pass.clone();
            g.last_pass = Some(stats);
            g.total_aborts = g.total_aborts.saturating_add(1);
            g.consecutive_passes = 0;
            g.current_epoch = None;
        }
        self.phase.store(GcPhase::Idle as u8, Ordering::Release);
    }

    // ── Mise a jour incrementale des stats de la passe courante ─────────────

    /// Enregistre le nombre de blobs scannes.
    pub fn record_scanned(&self, count: u64) {
        let mut g = self.inner.lock();
        g.current_pass.blobs_scanned = g.current_pass.blobs_scanned.saturating_add(count);
    }

    /// Enregistre les blobs marques vivants.
    pub fn record_marked(&self, count: u64) {
        let mut g = self.inner.lock();
        g.current_pass.blobs_marked_live = g.current_pass.blobs_marked_live.saturating_add(count);
    }

    /// Enregistre les orphelins collectes.
    pub fn record_orphans(&self, count: u64) {
        let mut g = self.inner.lock();
        g.current_pass.orphans_collected = g.current_pass.orphans_collected.saturating_add(count);
    }

    /// Enregistre les objets inline GC-es.
    pub fn record_inline_gc(&self, count: u64) {
        let mut g = self.inner.lock();
        g.current_pass.inline_gc_count = g.current_pass.inline_gc_count.saturating_add(count);
    }

    /// Enregistre les cycles detectes.
    pub fn record_cycles(&self, count: u64) {
        let mut g = self.inner.lock();
        g.current_pass.cycles_detected = g.current_pass.cycles_detected.saturating_add(count);
    }

    // ── Lecture des statistiques ─────────────────────────────────────────────

    /// Retourne les statistiques de la derniere passe terminee (copie).
    pub fn last_pass_stats(&self) -> Option<GcPassStats> {
        self.inner.lock().last_pass.clone()
    }

    /// Epoch de la passe courante.
    pub fn current_epoch(&self) -> Option<EpochId> {
        self.inner.lock().current_epoch
    }

    /// Nombre total de passes depuis le boot.
    pub fn total_passes(&self) -> u64 {
        self.inner.lock().total_passes
    }

    /// Nombre total de passes abandonnees.
    pub fn total_aborts(&self) -> u64 {
        self.inner.lock().total_aborts
    }

    /// Total des octets liberes depuis le boot.
    pub fn total_bytes_freed(&self) -> u64 {
        self.inner.lock().total_bytes_freed
    }

    /// Retourne un snapshot complet de l'etat courant.
    pub fn snapshot(&self) -> GcStateSnapshot {
        let g = self.inner.lock();
        GcStateSnapshot {
            phase:              self.phase(),
            logical_tick:       self.tick.load(Ordering::Acquire),
            current_epoch:      g.current_epoch,
            total_passes:       g.total_passes,
            total_aborts:       g.total_aborts,
            total_bytes_freed:  g.total_bytes_freed,
            consecutive_passes: g.consecutive_passes,
            is_running:         self.phase().is_active(),
        }
    }
}

// ==============================================================================
// GcStateSnapshot — vue instantanee pour metriques / debug
// ==============================================================================

/// Vue instantanee de l'etat GC (pas de verrou apres construction).
#[derive(Debug, Clone)]
pub struct GcStateSnapshot {
    pub phase:              GcPhase,
    pub logical_tick:       u64,
    pub current_epoch:      Option<EpochId>,
    pub total_passes:       u64,
    pub total_aborts:       u64,
    pub total_bytes_freed:  u64,
    pub consecutive_passes: u32,
    /// `true` si le GC est en cours d'exécution (non Idle, non Aborted).
    pub is_running:         bool,
}

impl fmt::Display for GcStateSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GC[phase={} tick={} passes={} aborts={} freed={}B]",
            self.phase,
            self.logical_tick,
            self.total_passes,
            self.total_aborts,
            self.total_bytes_freed,
        )
    }
}

// ==============================================================================
// Instance globale
// ==============================================================================

/// Instance globale de la machine d'etats GC.
///
/// DEAD-01 : toutes les methodes ici n'acquierent JAMAIS EPOCH_COMMIT_LOCK.
pub static GC_STATE: GcState = GcState::new();

// ==============================================================================
// Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_transitions_valid() {
        assert!(GcPhase::Idle.can_transition_to(GcPhase::Scanning));
        assert!(GcPhase::Scanning.can_transition_to(GcPhase::Marking));
        assert!(GcPhase::Marking.can_transition_to(GcPhase::Sweeping));
        assert!(GcPhase::Sweeping.can_transition_to(GcPhase::Finalizing));
        assert!(GcPhase::Finalizing.can_transition_to(GcPhase::Idle));
    }

    #[test]
    fn test_phase_transitions_invalid() {
        assert!(!GcPhase::Idle.can_transition_to(GcPhase::Marking));
        assert!(!GcPhase::Marking.can_transition_to(GcPhase::Scanning));
        assert!(!GcPhase::Idle.can_transition_to(GcPhase::Idle));
    }

    #[test]
    fn test_phase_aborted_always_valid() {
        assert!(GcPhase::Idle.can_transition_to(GcPhase::Aborted));
        assert!(GcPhase::Marking.can_transition_to(GcPhase::Aborted));
        assert!(GcPhase::Sweeping.can_transition_to(GcPhase::Aborted));
    }

    #[test]
    fn test_is_active() {
        assert!(!GcPhase::Idle.is_active());
        assert!(!GcPhase::Aborted.is_active());
        assert!(GcPhase::Scanning.is_active());
        assert!(GcPhase::Marking.is_active());
        assert!(GcPhase::Sweeping.is_active());
        assert!(GcPhase::Finalizing.is_active());
    }

    #[test]
    fn test_pass_stats_collect_ratio() {
        let mut s = GcPassStats::default();
        s.blobs_scanned = 1000;
        s.blobs_swept = 250;
        assert_eq!(s.collect_ratio_x100(), 25);
    }

    #[test]
    fn test_pass_stats_zero_scanned() {
        let s = GcPassStats::default();
        assert_eq!(s.collect_ratio_x100(), 0);
    }

    #[test]
    fn test_pass_stats_duration() {
        let mut s = GcPassStats::default();
        s.start_tick = 100;
        s.end_tick   = 250;
        assert_eq!(s.duration_ticks(), 150);
    }
}
