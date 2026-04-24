// kernel/src/fs/exofs/epoch/epoch_gc.rs
//
// =============================================================================
// Interface GC → Epoch : calcul de la fenêtre de collection + file différée
// Ring 0 · no_std · Exo-OS
// =============================================================================
//
// Ce module répond à la question clé du GC :
// "Jusqu'à quel epoch puis-je collecter en toute sécurité ?"
//
// RÈGLE DEAD-01 : Ce module NE JAMAIS acquiert EPOCH_COMMIT_LOCK.
//                 Il lit uniquement des atomiques et la table des pins.
// RÈGLE GC-04   : On ne collecte jamais un epoch épinglé par un snapshot.
// RÈGLE ARITH-02: checked_add/saturating_sub pour toute arithmétique.
// RÈGLE OOM-02  : try_reserve(1)? avant push().

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use alloc::vec::Vec;

use crate::fs::exofs::core::{DiskOffset, EpochId, ExofsError, ExofsResult, ObjectId};
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// =============================================================================
// Constantes
// =============================================================================

/// Nombre maximum d'entrées dans la DeferredDeleteQueue.
pub const GC_DEFERRED_QUEUE_MAX: usize = 4096;

/// Grace period par défaut (epochs à conserver avant collecte).
pub const GC_DEFAULT_GRACE_EPOCHS: u64 = 4;

// =============================================================================
// GcEpochWindow — fenêtre de collection sûre
// =============================================================================

/// Fenêtre d'epochs collectables calculée par le planificateur GC.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GcEpochWindow {
    /// Premier epoch collectable (inclus).
    pub from_epoch: EpochId,
    /// Dernier epoch collectable (exclus).
    pub until_epoch: EpochId,
    /// Nombre d'epochs dans la fenêtre.
    pub count: u64,
}

impl GcEpochWindow {
    /// Fenêtre vide (rien à collecter).
    pub const fn empty() -> Self {
        Self {
            from_epoch: EpochId(0),
            until_epoch: EpochId(0),
            count: 0,
        }
    }

    /// Vrai si la fenêtre est vide (count == 0).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Vrai si l'epoch donné est dans la fenêtre de collection.
    #[inline]
    pub fn contains(&self, epoch: EpochId) -> bool {
        !self.is_empty() && epoch.0 >= self.from_epoch.0 && epoch.0 < self.until_epoch.0
    }
}

impl fmt::Display for GcEpochWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcWindow[{}..{}) count={}",
            self.from_epoch.0, self.until_epoch.0, self.count
        )
    }
}

// =============================================================================
// compute_gc_window — calcul de la fenêtre sûre
// =============================================================================

/// Calcule la fenêtre d'epochs sûrs à collecter.
///
/// # Paramètres
/// - `current_epoch`   : epoch courant (sera EXCLU de la fenêtre).
/// - `oldest_pinned`   : epoch le plus ancien épinglé par un snapshot (si existant).
/// - `grace_epochs`    : nombre minimal d'epochs à conserver (tampon de sécurité).
///
/// # Algorithme
/// ```text
/// soft_limit = current_epoch - grace_epochs
/// hard_limit = oldest_pinned (si pin actif)
/// until_exclusive = min(soft_limit, hard_limit)
/// ```
///
/// RÈGLE DEAD-01 : Ne jamais acquérir EPOCH_COMMIT_LOCK ici.
/// RÈGLE ARITH-02: saturating_sub pour éviter underflow.
pub fn compute_gc_window(
    current_epoch: EpochId,
    oldest_pinned: Option<EpochId>,
    grace_epochs: u64,
) -> GcEpochWindow {
    let current_val = current_epoch.0;

    // Limite basse : grace period.
    let soft_limit = current_val.saturating_sub(grace_epochs);

    // Limite haute : contrainte de pin.
    let upper = match oldest_pinned {
        Some(pinned) if pinned.0 < soft_limit => pinned.0,
        _ => soft_limit,
    };

    if upper == 0 {
        return GcEpochWindow::empty();
    }

    GcEpochWindow {
        from_epoch: EpochId(0),
        until_epoch: EpochId(upper),
        count: upper,
    }
}

/// Vrai si l'epoch donné est dans la fenêtre de collection.
#[inline]
pub fn epoch_is_collectable(epoch: EpochId, window: &GcEpochWindow) -> bool {
    window.contains(epoch)
}

/// Calcule le lag entre l'epoch courant et l'epoch le plus ancien non collecté.
///
/// Un lag élevé indique que le GC est en retard.
#[inline]
pub fn gc_epoch_lag(current_epoch: EpochId, oldest_uncollected: EpochId) -> u64 {
    current_epoch.0.saturating_sub(oldest_uncollected.0)
}

// =============================================================================
// DeferredDeleteEntry — une suppression différée
// =============================================================================

/// Raison pour laquelle la suppression a été différée.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeferReason {
    /// Epoch encore épinglé par un snapshot.
    PinnedSnapshot = 0,
    /// Epoch dans la grace period.
    GracePeriod = 1,
    /// File GC pleine (backpressure).
    QueueFull = 2,
    /// Epoch courant (jamais collectable).
    CurrentEpoch = 3,
}

/// Une entrée dans la file de suppressions différées.
#[derive(Copy, Clone, Debug)]
pub struct DeferredDeleteEntry {
    /// Identifiant de l'objet à supprimer.
    pub object_id: ObjectId,
    /// Epoch dans lequel la suppression a été enregistrée.
    pub del_epoch: EpochId,
    /// Offset disque du bloc de données à libérer.
    pub data_offset: DiskOffset,
    /// Taille du bloc en octets.
    pub size_bytes: u64,
    /// Raison du différé.
    pub reason: DeferReason,
}

impl DeferredDeleteEntry {
    /// Crée une entrée de suppression différée.
    pub fn new(
        object_id: ObjectId,
        del_epoch: EpochId,
        data_offset: DiskOffset,
        size_bytes: u64,
        reason: DeferReason,
    ) -> Self {
        Self {
            object_id,
            del_epoch,
            data_offset,
            size_bytes,
            reason,
        }
    }
}

// =============================================================================
// DeferredDeleteQueue — file d'attente des suppressions différées
// =============================================================================

/// File FIFO des suppressions différées par le GC.
///
/// RÈGLE DEAD-01 : Cette structure N'EST PAS protégée par EPOCH_COMMIT_LOCK.
///                 Elle a son propre SpinLock (intégré par l'appelant).
pub struct DeferredDeleteQueue {
    entries: Vec<DeferredDeleteEntry>,
    max_size: usize,
    total_pushed: u64,
    total_drained: u64,
}

impl DeferredDeleteQueue {
    /// Crée une file vide avec taille maximale.
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_size,
            total_pushed: 0,
            total_drained: 0,
        }
    }

    /// Retourne la taille maximale de la file.
    #[inline]
    pub fn max_size(&self) -> usize {
        self.max_size
    }

    /// Retourne le nombre d'entrées actuelles.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Vrai si la file est vide.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Vrai si la file est pleine.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.entries.len() >= self.max_size
    }

    /// Ajoute une entrée dans la file.
    ///
    /// RÈGLE OOM-02 : try_reserve(1)? avant push().
    /// Retourne Err(NoMemory) si OOM, Err(EpochFull) si backpressure.
    pub fn push(&mut self, entry: DeferredDeleteEntry) -> ExofsResult<()> {
        if self.entries.len() >= self.max_size {
            return Err(ExofsError::EpochFull);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.entries.push(entry);
        self.total_pushed = self.total_pushed.saturating_add(1);
        Ok(())
    }

    /// Draine toutes les entrées collectables selon la fenêtre.
    ///
    /// Retourne les entrées dont l'epoch est dans la fenêtre.
    /// Conserve les autres.
    ///
    /// RÈGLE RECUR-01 : itément itératif (pas de récursion).
    /// RÈGLE OOM-02 : try_reserve pour la Vec résultante.
    pub fn drain_collectable(
        &mut self,
        window: &GcEpochWindow,
    ) -> ExofsResult<Vec<DeferredDeleteEntry>> {
        if window.is_empty() {
            return Ok(Vec::new());
        }
        let mut collectables: Vec<DeferredDeleteEntry> = Vec::new();
        let mut remaining: Vec<DeferredDeleteEntry> = Vec::new();
        let n = self.entries.len();
        collectables
            .try_reserve(n)
            .map_err(|_| ExofsError::NoMemory)?;
        remaining.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        // RÈGLE RECUR-01 : boucle itérative.
        for entry in self.entries.drain(..) {
            if window.contains(entry.del_epoch) {
                collectables.push(entry);
            } else {
                remaining.push(entry);
            }
        }
        self.total_drained = self.total_drained.saturating_add(collectables.len() as u64);
        self.entries = remaining;
        Ok(collectables)
    }

    /// Snapshot des métriques de la file.
    pub fn stats_snapshot(&self) -> DeferredQueueStats {
        DeferredQueueStats {
            current_len: self.entries.len() as u64,
            max_size: self.max_size as u64,
            total_pushed: self.total_pushed,
            total_drained: self.total_drained,
        }
    }
}

/// Métriques de la file différée (snapshot atomique).
#[derive(Copy, Clone, Debug)]
pub struct DeferredQueueStats {
    pub current_len: u64,
    pub max_size: u64,
    pub total_pushed: u64,
    pub total_drained: u64,
}

// =============================================================================
// GcSafetyCheck — validation avant collection

/// Jeton de validation sécurité avant déclenchement d'un cycle de GC.
/// Produit par `gc_safety_check()` — doit être passé à `run_gc_cycle()`.
pub struct GcSafetyCheck {
    /// true si le GC peut procéder sans risque d'écraser des données actives.
    pub safe: bool,
}
// =============================================================================

/// Résultat d'une vérification de sécurité GC.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GcCheckResult {
    /// Collection autorisée.
    Safe,
    /// Collection bloquée (raison fournie).
    Blocked(GcBlockReason),
}

/// Raison de blocage de la collection GC.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GcBlockReason {
    /// Commit en cours (RÈGLE DEAD-01 inverse — commit bloque GC).
    CommitInProgress,
    /// Fenêtre vide (rien à collecter).
    EmptyWindow,
    /// Epoch dans la grace period.
    GracePeriod,
    /// Epoch épinglé par un snapshot.
    PinnedBySnapshot,
}

/// Vérifie si la collection est sûre pour un epoch donné.
///
/// RÈGLE DEAD-01 : Ne PAS acquérir EPOCH_COMMIT_LOCK ici.
/// Cette fonction est non-bloquante.
pub fn gc_safety_check(
    epoch: EpochId,
    window: &GcEpochWindow,
    commit_in_prog: bool,
) -> GcCheckResult {
    if commit_in_prog {
        // Un commit est en cours : ne pas collecter pour éviter race.
        return GcCheckResult::Blocked(GcBlockReason::CommitInProgress);
    }
    if window.is_empty() {
        return GcCheckResult::Blocked(GcBlockReason::EmptyWindow);
    }
    if !window.contains(epoch) {
        return GcCheckResult::Blocked(GcBlockReason::GracePeriod);
    }
    GcCheckResult::Safe
}

// =============================================================================
// GcStats — compteurs atomiques du GC
// =============================================================================

/// Compteurs atomiques du garbage collector.
pub struct GcStats {
    /// Nombre de cycles GC déclenchés.
    pub cycles: AtomicU64,
    /// Epochs effectivement collectés.
    pub epochs_freed: AtomicU64,
    /// Objets / blocs effectivement supprimés.
    pub objects_freed: AtomicU64,
    /// Entrées différées drainées.
    pub entries_drained: AtomicU64,
    /// Cycles bloqués (commit en cours).
    pub cycles_blocked: AtomicU64,
    /// Total d'octets libérés.
    pub bytes_freed: AtomicU64,
}

impl GcStats {
    pub const fn new() -> Self {
        macro_rules! z {
            () => {
                AtomicU64::new(0)
            };
        }
        GcStats {
            cycles: z!(),
            epochs_freed: z!(),
            objects_freed: z!(),
            entries_drained: z!(),
            cycles_blocked: z!(),
            bytes_freed: z!(),
        }
    }

    #[inline]
    pub fn inc_cycles(&self) {
        self.cycles.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_epochs_freed(&self, n: u64) {
        self.epochs_freed.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_objects_freed(&self, n: u64) {
        self.objects_freed.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_entries_drained(&self, n: u64) {
        self.entries_drained.fetch_add(n, Ordering::Relaxed);
    }
    #[inline]
    pub fn inc_cycles_blocked(&self) {
        self.cycles_blocked.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn add_bytes_freed(&self, n: u64) {
        self.bytes_freed.fetch_add(n, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> GcStatsSnapshot {
        GcStatsSnapshot {
            cycles: self.cycles.load(Ordering::Relaxed),
            epochs_freed: self.epochs_freed.load(Ordering::Relaxed),
            objects_freed: self.objects_freed.load(Ordering::Relaxed),
            entries_drained: self.entries_drained.load(Ordering::Relaxed),
            cycles_blocked: self.cycles_blocked.load(Ordering::Relaxed),
            bytes_freed: self.bytes_freed.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot non-atomique des compteurs GC.
#[derive(Copy, Clone, Debug)]
pub struct GcStatsSnapshot {
    pub cycles: u64,
    pub epochs_freed: u64,
    pub objects_freed: u64,
    pub entries_drained: u64,
    pub cycles_blocked: u64,
    pub bytes_freed: u64,
}

impl fmt::Display for GcStatsSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcStats{{ cycles={} freed=[epochs={} objs={} bytes={}] blocked={} }}",
            self.cycles,
            self.epochs_freed,
            self.objects_freed,
            self.bytes_freed,
            self.cycles_blocked,
        )
    }
}

/// Singleton global des statistiques GC.
pub static GC_STATS: GcStats = GcStats::new();

// =============================================================================
// GcCollectionResult — résultat d'un cycle de collection
// =============================================================================

/// Résultat d'un cycle GC.
#[derive(Debug)]
pub struct GcCollectionResult {
    /// Nombre d'entrées traitées.
    pub processed: u64,
    /// Nombre d'entrées effectivement supprimées.
    pub freed: u64,
    /// Nombre d'octets libérés.
    pub bytes_freed: u64,
    /// Fenêtre utilisée.
    pub window_used: GcEpochWindow,
}

/// Exécute un cycle GC sur la file différée.
///
/// # Paramètres
/// - `queue`          : file de suppressions différées.
/// - `window`         : fenêtre de collecte calculée par `compute_gc_window`.
/// - `commit_in_prog` : vrai si un commit est en cours.
/// - `free_fn`        : fonction de libération physique de blocs.
///
/// RÈGLE DEAD-01 : Cette fonction NE TIENT JAMAIS EPOCH_COMMIT_LOCK.
/// RÈGLE RECUR-01: itération strictement itérative.
pub fn run_gc_cycle(
    queue: &mut DeferredDeleteQueue,
    window: &GcEpochWindow,
    commit_in_prog: bool,
    free_fn: &dyn Fn(DiskOffset, u64) -> ExofsResult<()>,
) -> ExofsResult<GcCollectionResult> {
    GC_STATS.inc_cycles();
    EPOCH_STATS.inc_gc_cycles();

    if commit_in_prog {
        GC_STATS.inc_cycles_blocked();
        return Ok(GcCollectionResult {
            processed: 0,
            freed: 0,
            bytes_freed: 0,
            window_used: *window,
        });
    }

    let collectables = queue.drain_collectable(window)?;
    let total = collectables.len() as u64;
    let mut freed = 0u64;
    let mut bytes = 0u64;

    // RÈGLE RECUR-01 : boucle itérative.
    for entry in &collectables {
        match free_fn(entry.data_offset, entry.size_bytes) {
            Ok(()) => {
                freed = freed.saturating_add(1);
                bytes = bytes.saturating_add(entry.size_bytes);
            }
            Err(_) => {
                // Échec de libération : pas fatal, on continue.
            }
        }
    }

    GC_STATS.add_epochs_freed(freed);
    GC_STATS.add_objects_freed(freed);
    GC_STATS.add_bytes_freed(bytes);
    GC_STATS.add_entries_drained(total);
    EPOCH_STATS.add_gc_objects_freed(freed);

    Ok(GcCollectionResult {
        processed: total,
        freed,
        bytes_freed: bytes,
        window_used: *window,
    })
}
