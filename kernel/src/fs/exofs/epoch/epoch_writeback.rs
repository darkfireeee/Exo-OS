// kernel/src/fs/exofs/epoch/epoch_writeback.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Writeback périodique des epochs — flush automatique et group commit
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce module gère la politique de commit automatique :
//   - Commit si delta > EPOCH_MAX_OBJECTS / 2 (mode préemptif, EPOCH-05).
//   - Commit périodique toutes les N TSC-ticks (configurable).
//   - Commit forcé sur fsync() depuis posix_bridge.
//   - Group commit : coalescence des commits proches dans une fenêtre.
//   - Backpressure : ralentissement des writers si le delta est saturé.
//
// RÈGLE EPOCH-05 : commit anticipé si EpochRoot > 500 objets.
// RÈGLE EPOCH-03 : acquire EPOCH_COMMIT_LOCK avant chaque commit.
// RÈGLE DAG-01   : pas d'import storage/ ni core/config/.
// RÈGLE ARITH-02 : checked_add / saturating_* pour toute arithmétique.
// RÈGLE OOM-02   : try_reserve avant push.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::fs::exofs::core::{EpochId, ExofsResult, EPOCH_MAX_OBJECTS};
use crate::fs::exofs::epoch::epoch_delta::EpochDelta;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// =============================================================================
// Raisons de flush
// =============================================================================

/// Raison déclenchant un flush de l'epoch courant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlushReason {
    /// Flush périodique automatique (timer TSC écoulé).
    Periodic,
    /// Delta saturé (>= EPOCH_MAX_OBJECTS / 2).
    DeltaFull,
    /// fsync() explicite depuis userspace.
    Explicit,
    /// Démontage du volume (flush final).
    Umount,
    /// Mémoire sous pression — flush préventif.
    MemoryPressure,
    /// Forçage externe via `force_commit()`.
    Forced,
}

impl fmt::Display for FlushReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlushReason::Periodic => write!(f, "Periodic"),
            FlushReason::DeltaFull => write!(f, "DeltaFull"),
            FlushReason::Explicit => write!(f, "Explicit"),
            FlushReason::Umount => write!(f, "Umount"),
            FlushReason::MemoryPressure => write!(f, "MemoryPressure"),
            FlushReason::Forced => write!(f, "Forced"),
        }
    }
}

// =============================================================================
// Décision de writeback
// =============================================================================

/// Décision rendue par l'algorithme de scheduling du writeback.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WritebackDecision {
    /// Aucune action requise — attendre le prochain tick.
    Hold,
    /// Commit périodique requis (timer expiré).
    CommitPeriodic,
    /// Commit immédiat requis (delta saturé).
    CommitImmediate { reason: FlushReason },
    /// Commit forcé (fsync, umount, pression mémoire).
    CommitForced { reason: FlushReason },
}

impl WritebackDecision {
    /// Retourne `true` si la décision implique un commit.
    #[inline]
    pub fn requires_commit(&self) -> bool {
        !matches!(self, WritebackDecision::Hold)
    }

    /// Retourne la raison du flush si applicable.
    pub fn flush_reason(&self) -> Option<FlushReason> {
        match self {
            WritebackDecision::Hold => None,
            WritebackDecision::CommitPeriodic => Some(FlushReason::Periodic),
            WritebackDecision::CommitImmediate { reason } => Some(*reason),
            WritebackDecision::CommitForced { reason } => Some(*reason),
        }
    }
}

impl fmt::Display for WritebackDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WritebackDecision::Hold => write!(f, "Hold"),
            WritebackDecision::CommitPeriodic => write!(f, "CommitPeriodic"),
            WritebackDecision::CommitImmediate { reason } => {
                write!(f, "CommitImmediate({})", reason)
            }
            WritebackDecision::CommitForced { reason } => {
                write!(f, "CommitForced({})", reason)
            }
        }
    }
}

// =============================================================================
// Politique de backpressure
// =============================================================================

/// Politique de backpressure appliquée aux writers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BackpressurePolicy {
    /// Aucune restriction — delta < 25% de EPOCH_MAX_OBJECTS.
    None,
    /// Avertissement — delta >= 25%.
    Warn,
    /// Ralentissement — delta >= 50%.
    Throttle,
    /// Blocage total — delta >= 90%.
    Block,
}

impl BackpressurePolicy {
    /// Détermine la politique à partir du nombre d'entrées dans le delta.
    pub fn from_delta_len(len: usize) -> Self {
        let cap = EPOCH_MAX_OBJECTS;
        // Utilise des divisions entières pour éviter fp arithmetic.
        if len >= cap.saturating_mul(9) / 10 {
            BackpressurePolicy::Block
        } else if len >= cap / 2 {
            BackpressurePolicy::Throttle
        } else if len >= cap / 4 {
            BackpressurePolicy::Warn
        } else {
            BackpressurePolicy::None
        }
    }

    /// Retourne `true` si les writers doivent être bloqués.
    #[inline]
    pub fn should_block(&self) -> bool {
        matches!(self, BackpressurePolicy::Block)
    }

    /// Retourne `true` si les writers doivent être ralentis.
    #[inline]
    pub fn should_throttle(&self) -> bool {
        matches!(
            self,
            BackpressurePolicy::Throttle | BackpressurePolicy::Block
        )
    }
}

impl fmt::Display for BackpressurePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackpressurePolicy::None => write!(f, "None"),
            BackpressurePolicy::Warn => write!(f, "Warn"),
            BackpressurePolicy::Throttle => write!(f, "Throttle"),
            BackpressurePolicy::Block => write!(f, "Block"),
        }
    }
}

// =============================================================================
// Résultat d'un cycle de writeback
// =============================================================================

/// Résultat d'un cycle de writeback exécuté.
#[derive(Copy, Clone, Debug)]
pub struct WritebackCycleResult {
    /// Epoch committé (None si aucun commit effectué).
    pub committed_epoch: Option<EpochId>,
    /// Nombre d'objets commités.
    pub object_count: u32,
    /// Durée du cycle en cycles TSC.
    pub duration_cycles: u64,
    /// Décision qui a déclenché ce cycle.
    pub decision: WritebackDecision,
    /// Politique de backpressure au moment du cycle.
    pub backpressure: BackpressurePolicy,
}

impl WritebackCycleResult {
    /// Résultat no-op (aucun commit).
    pub const fn noop() -> Self {
        Self {
            committed_epoch: None,
            object_count: 0,
            duration_cycles: 0,
            decision: WritebackDecision::Hold,
            backpressure: BackpressurePolicy::None,
        }
    }
}

// =============================================================================
// Planificateur de flush
// =============================================================================

/// Planificateur de flush — decide quand déclencher un commit.
pub struct FlushSchedule {
    /// Intervalle minimum entre deux commits périodiques (TSC ticks).
    /// Défaut : ~10ms à 1 GHz (10 000 000 ticks).
    min_interval_ticks: AtomicU64,
    /// Fenêtre de coalescence de commits (TSC ticks).
    /// Pendant cette fenêtre, un seul commit est déclenché.
    coalesce_window_ticks: AtomicU64,
    /// Seuil en % de EPOCH_MAX_OBJECTS déclenchant un commit immédiat.
    /// Stocké comme numérateur pour (num / 100) * EPOCH_MAX_OBJECTS.
    preempt_threshold_pct: AtomicU32,
}

impl FlushSchedule {
    /// Crée un planificateur avec les valeurs par défaut.
    pub const fn new() -> Self {
        Self {
            min_interval_ticks: AtomicU64::new(10_000_000),
            coalesce_window_ticks: AtomicU64::new(1_000_000),
            preempt_threshold_pct: AtomicU32::new(50),
        }
    }

    /// Définit l'intervalle minimum entre commits périodiques.
    pub fn set_interval(&self, ticks: u64) {
        self.min_interval_ticks
            .store(ticks.max(1), Ordering::Relaxed);
    }

    /// Définit la fenêtre de coalescence.
    pub fn set_coalesce_window(&self, ticks: u64) {
        self.coalesce_window_ticks.store(ticks, Ordering::Relaxed);
    }

    /// Définit le seuil de préemption (0–100%).
    pub fn set_preempt_threshold_pct(&self, pct: u32) {
        let clamped = pct.min(100);
        self.preempt_threshold_pct.store(clamped, Ordering::Relaxed);
    }

    /// Calcule le seuil absolu de préemption (nombre d'entrées).
    pub fn preempt_threshold(&self) -> usize {
        let pct = self.preempt_threshold_pct.load(Ordering::Relaxed) as usize;
        EPOCH_MAX_OBJECTS.saturating_mul(pct) / 100
    }

    /// Retourne l'intervalle minimum en TSC ticks.
    pub fn interval_ticks(&self) -> u64 {
        self.min_interval_ticks.load(Ordering::Relaxed)
    }

    /// Retourne la fenêtre de coalescence en TSC ticks.
    pub fn coalesce_window_ticks(&self) -> u64 {
        self.coalesce_window_ticks.load(Ordering::Relaxed)
    }
}

/// Planificateur de flush global.
pub static FLUSH_SCHEDULE: FlushSchedule = FlushSchedule::new();

// =============================================================================
// Contrôleur du writeback
// =============================================================================

/// Contrôleur du thread de writeback — état atomique partagé.
pub struct WritebackController {
    /// Vrai si le thread de writeback est actif.
    running: AtomicBool,
    /// TSC du dernier commit effectué.
    last_commit_tsc: AtomicU64,
    /// Nombre de commits périodiques depuis le démarrage.
    periodic_commits: AtomicU64,
    /// Nombre de commits immédiats (delta saturé) depuis le démarrage.
    immediate_commits: AtomicU64,
    /// Nombre de commits forcés (fsync, umount) depuis le démarrage.
    forced_commits: AtomicU64,
    /// Nombre d'épisodes de backpressure (throttle ou block).
    backpressure_episodes: AtomicU64,
    /// Nombre total d'objets commités.
    total_objects_committed: AtomicU64,
    /// Vrai si un commit forcé est en attente.
    force_pending: AtomicBool,
    /// Raison du commit forcé en attente (encodée comme u32).
    force_reason_raw: AtomicU32,
}

impl WritebackController {
    /// Crée un contrôleur initial.
    pub const fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            last_commit_tsc: AtomicU64::new(0),
            periodic_commits: AtomicU64::new(0),
            immediate_commits: AtomicU64::new(0),
            forced_commits: AtomicU64::new(0),
            backpressure_episodes: AtomicU64::new(0),
            total_objects_committed: AtomicU64::new(0),
            force_pending: AtomicBool::new(false),
            force_reason_raw: AtomicU32::new(0),
        }
    }

    /// Lance le thread de writeback (appel unique au montage).
    pub fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
    }

    /// Arrête le thread de writeback (appel au démontage).
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Retourne `true` si le thread est actif.
    #[inline]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Enregistre un commit effectué (met à jour les compteurs et TSC).
    pub fn record_commit(&self, reason: FlushReason, tsc_now: u64, object_count: u32) {
        self.last_commit_tsc.store(tsc_now, Ordering::Relaxed);
        self.total_objects_committed
            .fetch_add(object_count as u64, Ordering::Relaxed);

        match reason {
            FlushReason::Periodic => {
                self.periodic_commits.fetch_add(1, Ordering::Relaxed);
            }
            FlushReason::DeltaFull | FlushReason::MemoryPressure => {
                self.immediate_commits.fetch_add(1, Ordering::Relaxed);
                EPOCH_STATS.inc_forced_commits();
            }
            FlushReason::Explicit | FlushReason::Umount | FlushReason::Forced => {
                self.forced_commits.fetch_add(1, Ordering::Relaxed);
            }
        }

        EPOCH_STATS.add_objects_committed(object_count as u64);

        // Annule le force_pending si c'était un commit forcé.
        if matches!(
            reason,
            FlushReason::Forced | FlushReason::Explicit | FlushReason::Umount
        ) {
            self.force_pending.store(false, Ordering::Relaxed);
        }
    }

    /// Retourne `true` si un flush périodique est dû (TSC dépassé).
    #[inline]
    pub fn needs_periodic_flush(&self, tsc_now: u64) -> bool {
        let last = self.last_commit_tsc.load(Ordering::Relaxed);
        let interval = FLUSH_SCHEDULE.interval_ticks();
        tsc_now.saturating_sub(last) >= interval
    }

    /// Force un commit au prochain tick du writeback.
    pub fn request_force_commit(&self, reason: FlushReason) {
        let raw = match reason {
            FlushReason::Explicit => 1u32,
            FlushReason::Umount => 2u32,
            FlushReason::MemoryPressure => 3u32,
            FlushReason::Forced => 4u32,
            _ => 4u32,
        };
        self.force_reason_raw.store(raw, Ordering::Relaxed);
        self.force_pending.store(true, Ordering::Release);
    }

    /// Retourne le commit forcé en attente (s'il y en a un).
    pub fn take_force_pending(&self) -> Option<FlushReason> {
        if !self.force_pending.load(Ordering::Acquire) {
            return None;
        }
        let raw = self.force_reason_raw.load(Ordering::Relaxed);
        let reason = match raw {
            1 => FlushReason::Explicit,
            2 => FlushReason::Umount,
            3 => FlushReason::MemoryPressure,
            _ => FlushReason::Forced,
        };
        Some(reason)
    }

    /// Enregistre un épisode de backpressure.
    #[inline]
    pub fn record_backpressure(&self) {
        self.backpressure_episodes.fetch_add(1, Ordering::Relaxed);
    }

    /// Collecte un snapshot des statistiques du contrôleur.
    pub fn stats(&self) -> WritebackStats {
        WritebackStats {
            running: self.running.load(Ordering::Relaxed),
            periodic_commits: self.periodic_commits.load(Ordering::Relaxed),
            immediate_commits: self.immediate_commits.load(Ordering::Relaxed),
            forced_commits: self.forced_commits.load(Ordering::Relaxed),
            backpressure_episodes: self.backpressure_episodes.load(Ordering::Relaxed),
            total_objects_committed: self.total_objects_committed.load(Ordering::Relaxed),
            last_commit_tsc: self.last_commit_tsc.load(Ordering::Relaxed),
        }
    }
}

/// Contrôleur global du writeback.
pub static WRITEBACK_CTL: WritebackController = WritebackController::new();

// =============================================================================
// Statistiques de writeback
// =============================================================================

/// Snapshot immutable des statistiques du writeback.
#[derive(Debug, Copy, Clone)]
pub struct WritebackStats {
    /// Vrai si le thread de writeback est actif.
    pub running: bool,
    /// Commits périodiques effectués.
    pub periodic_commits: u64,
    /// Commits immédiats (delta saturé).
    pub immediate_commits: u64,
    /// Commits forcés (fsync, umount, etc.).
    pub forced_commits: u64,
    /// Épisodes de backpressure (throttle + block).
    pub backpressure_episodes: u64,
    /// Total des objets commités.
    pub total_objects_committed: u64,
    /// TSC du dernier commit.
    pub last_commit_tsc: u64,
}

impl WritebackStats {
    /// Nombre total de commits (tous types confondus).
    #[inline]
    pub fn total_commits(&self) -> u64 {
        self.periodic_commits
            .saturating_add(self.immediate_commits)
            .saturating_add(self.forced_commits)
    }
}

impl fmt::Display for WritebackStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WritebackStats {{ running={}, total_commits={}, \
             periodic={}, immediate={}, forced={}, \
             backpressure={}, objects={}, last_tsc={} }}",
            self.running,
            self.total_commits(),
            self.periodic_commits,
            self.immediate_commits,
            self.forced_commits,
            self.backpressure_episodes,
            self.total_objects_committed,
            self.last_commit_tsc,
        )
    }
}

// =============================================================================
// Algorithmes de décision
// =============================================================================

/// Détermine si un flush doit être déclenché à l'instant `tsc_now`.
///
/// # Priorités (ordre décroissant)
/// 1. Commit forcé en attente (force_pending).
/// 2. Delta >= threshold (commit immédiat).
/// 3. Timer périodique expiré.
///
/// RÈGLE EPOCH-05 : flush si delta.len() >= EPOCH_MAX_OBJECTS / 2.
pub fn should_flush_now(delta: &EpochDelta, tsc_now: u64) -> WritebackDecision {
    // 1. Commit forcé en attente ?
    if let Some(reason) = WRITEBACK_CTL.take_force_pending() {
        return WritebackDecision::CommitForced { reason };
    }

    // 2. Delta saturé ?
    let threshold = FLUSH_SCHEDULE.preempt_threshold();
    if delta.len() >= threshold {
        let bp = BackpressurePolicy::from_delta_len(delta.len());
        if bp.should_throttle() {
            WRITEBACK_CTL.record_backpressure();
        }
        return WritebackDecision::CommitImmediate {
            reason: FlushReason::DeltaFull,
        };
    }

    // 3. Timer périodique ?
    if WRITEBACK_CTL.needs_periodic_flush(tsc_now) {
        return WritebackDecision::CommitPeriodic;
    }

    WritebackDecision::Hold
}

/// Décision simplifiée retournant `Option<FlushReason>` (compat. legacy).
///
/// Retourne `Some(FlushReason)` si un flush est requis, `None` sinon.
#[inline]
pub fn should_flush_now_simple(delta: &EpochDelta, tsc_now: u64) -> Option<FlushReason> {
    should_flush_now(delta, tsc_now).flush_reason()
}

/// Enregistre le résultat d'un flush et met à jour les statistiques.
pub fn record_flush(reason: FlushReason, tsc_now: u64, object_count: u32) {
    WRITEBACK_CTL.record_commit(reason, tsc_now, object_count);
}

/// Retourne la politique de backpressure courante pour un delta donné.
#[inline]
pub fn current_backpressure(delta: &EpochDelta) -> BackpressurePolicy {
    BackpressurePolicy::from_delta_len(delta.len())
}

/// Demande un commit forcé au prochain tick du writeback.
///
/// Thread-safe — peut être appelé depuis n'importe quel contexte.
#[inline]
pub fn request_force_commit(reason: FlushReason) {
    WRITEBACK_CTL.request_force_commit(reason);
}

/// Collecte les statistiques globales du writeback.
#[inline]
pub fn writeback_stats() -> WritebackStats {
    WRITEBACK_CTL.stats()
}

// =============================================================================
// Buffer de group commit
// =============================================================================

/// Entrée dans le buffer de group commit.
#[derive(Copy, Clone, Debug)]
pub struct GroupCommitEntry {
    /// TSC du moment où l'écriture a été soumise.
    pub submitted_at: u64,
    /// Nombre d'objets dans cette écriture.
    pub object_count: u32,
    /// Vrai si c'est un fsync (commit forcé).
    pub is_sync: bool,
}

/// Capacité maximale du buffer de group commit.
pub const GROUP_COMMIT_CAPACITY: usize = 32;

/// Buffer de group commit — coalescence des commits proches.
///
/// Protégé par le EPOCH_COMMIT_LOCK externe, donc pas de SpinLock interne.
pub struct GroupCommitBuffer {
    /// Entrées en attente de commit.
    entries: [GroupCommitEntry; GROUP_COMMIT_CAPACITY],
    /// Nombre d'entrées valides.
    count: usize,
    /// TSC de la première entrée ajoutée dans la fenêtre courante.
    window_start_tsc: u64,
    /// Nombre total de flushes effectués.
    total_flushes: u64,
    /// Nombre total d'entrées coalesçées.
    total_coalesced: u64,
}

impl GroupCommitBuffer {
    /// Crée un buffer vide.
    pub const fn new() -> Self {
        const EMPTY_ENTRY: GroupCommitEntry = GroupCommitEntry {
            submitted_at: 0,
            object_count: 0,
            is_sync: false,
        };
        Self {
            entries: [EMPTY_ENTRY; GROUP_COMMIT_CAPACITY],
            count: 0,
            window_start_tsc: 0,
            total_flushes: 0,
            total_coalesced: 0,
        }
    }

    /// Ajoute une entrée au buffer.
    ///
    /// Retourne `Err(ExofsError::QuotaExceeded)` si le buffer est plein.
    pub fn push(&mut self, entry: GroupCommitEntry) -> ExofsResult<()> {
        use crate::fs::exofs::core::ExofsError;
        if self.count >= GROUP_COMMIT_CAPACITY {
            return Err(ExofsError::QuotaExceeded);
        }
        if self.count == 0 {
            self.window_start_tsc = entry.submitted_at;
        }
        self.entries[self.count] = entry;
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    /// Vide le buffer et retourne le nombre d'objets total.
    ///
    /// Appelé après chaque commit réussi.
    pub fn drain(&mut self) -> GroupCommitSummary {
        let mut total_objects: u32 = 0;
        let mut has_sync = false;
        for i in 0..self.count {
            total_objects = total_objects.saturating_add(self.entries[i].object_count);
            if self.entries[i].is_sync {
                has_sync = true;
            }
        }
        let coalesced = self.count as u64;
        self.total_flushes = self.total_flushes.saturating_add(1);
        self.total_coalesced = self.total_coalesced.saturating_add(coalesced);
        self.count = 0;
        self.window_start_tsc = 0;
        GroupCommitSummary {
            entries_coalesced: coalesced as u32,
            total_objects,
            has_sync,
        }
    }

    /// Retourne `true` si la fenêtre de coalescence est expirée.
    pub fn is_window_expired(&self, tsc_now: u64) -> bool {
        if self.count == 0 {
            return false;
        }
        let window = FLUSH_SCHEDULE.coalesce_window_ticks();
        tsc_now.saturating_sub(self.window_start_tsc) >= window
    }

    /// Retourne le nombre d'entrées en attente.
    #[inline]
    pub fn pending_count(&self) -> usize {
        self.count
    }

    /// Retourne `true` si le buffer est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Retourne `true` si le buffer est plein.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.count >= GROUP_COMMIT_CAPACITY
    }

    /// Statistiques du buffer de group commit.
    pub fn stats(&self) -> GroupCommitStats {
        GroupCommitStats {
            pending: self.count as u32,
            total_flushes: self.total_flushes,
            total_coalesced: self.total_coalesced,
        }
    }
}

/// Résumé d'un group commit drainé.
#[derive(Debug, Copy, Clone)]
pub struct GroupCommitSummary {
    /// Nombre d'entrées coalesçées dans ce flush.
    pub entries_coalesced: u32,
    /// Nombre total d'objets.
    pub total_objects: u32,
    /// Vrai si au moins un fsync était en attente.
    pub has_sync: bool,
}

/// Statistiques du GroupCommitBuffer.
#[derive(Debug, Copy, Clone)]
pub struct GroupCommitStats {
    /// Entrées actuellement en attente.
    pub pending: u32,
    /// Nombre de flushes effectués.
    pub total_flushes: u64,
    /// Nombre total d'entrées coalesçées.
    pub total_coalesced: u64,
}

impl fmt::Display for GroupCommitStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GroupCommit {{ pending={}, flushes={}, coalesced={} }}",
            self.pending, self.total_flushes, self.total_coalesced,
        )
    }
}
