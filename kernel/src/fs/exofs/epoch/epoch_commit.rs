// kernel/src/fs/exofs/epoch/epoch_commit.rs
//
// =============================================================================
// Protocole de commit Epoch — 3 barrières NVMe OBLIGATOIRES
// Ring 0 · no_std · Exo-OS
// =============================================================================
//
// RÈGLE EPOCH-01 (CRITIQUE) : ordre des écritures INVIOLABLE :
//   Phase 1 : write(payload) → nvme_flush()          ← BARRIÈRE 1
//   Phase 2 : write(EpochRoot) → nvme_flush()        ← BARRIÈRE 2
//   Phase 3 : write(EpochRecord→slot) → nvme_flush() ← BARRIÈRE 3
//
// Inverser cet ordre = corruption garantie au prochain reboot.
//
// RÈGLE DEAD-01  : EPOCH_COMMIT_LOCK jamais acquis par le GC.
// RÈGLE EPOCH-03 : EPOCH_COMMIT_LOCK — un seul commit à la fois.
// RÈGLE EPOCH-05 : commit anticipé si EpochRoot > 500 objets.
// RÈGLE DAG-01   : epoch/ ne doit PAS importer storage/ directement.
//                  Les dépendances envers le superblock sont injectées via callbacks.
// RÈGLE ARITH-02 : checked_add/saturating_* pour toute arithmétique.
// RÈGLE OOM-02   : try_reserve avant push.

use core::fmt;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset, ObjectId,
};
use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::epoch::epoch_barriers::{
    nvme_barrier_after_data, nvme_barrier_after_root, nvme_barrier_after_record,
};
use crate::fs::exofs::epoch::epoch_commit_lock::EPOCH_COMMIT_LOCK;
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::epoch::epoch_root::EpochRootInMemory;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// =============================================================================
// CommitCallbacks — injection de dépendances (pas d'import storage/)
// =============================================================================

/// Callbacks injectés par la couche storage pour le commit.
///
/// RÈGLE DAG-01 : epoch/ n'importe PAS storage/. Les dépendances sont
/// passées par pointeurs de fonctions en `const fn`-compatible.
pub struct CommitCallbacks<'a> {
    /// Lit l'EpochId courant du superblock.
    pub get_current_epoch: &'a dyn Fn() -> EpochId,
    /// Avance l'EpochId du superblock après commit réussi.
    pub advance_epoch:     &'a dyn Fn(EpochId) -> ExofsResult<()>,
    /// Horodatage TSC.
    pub get_tsc:           &'a dyn Fn() -> u64,
    /// Écrit `data` à l'`offset` disque ; retourne bytes_written.
    pub write_fn:          &'a dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
}

// =============================================================================
// CommitResult — résultat d'un commit Epoch réussi
// =============================================================================

/// Résultat d'un commit Epoch réussi.
#[derive(Debug)]
pub struct CommitResult {
    /// Identifiant du nouvel epoch committé.
    pub epoch_id:     EpochId,
    /// Offset disque du slot écrit.
    pub slot_offset:  DiskOffset,
    /// Nombre d'objets committés (total modified + deleted).
    pub object_count: u32,
    /// Durée du commit en cycles TSC.
    pub duration_cycles: u64,
}

impl fmt::Display for CommitResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CommitResult{{ epoch={} slot={} objects={} cycles={} }}",
            self.epoch_id.0, self.slot_offset.0, self.object_count, self.duration_cycles,
        )
    }
}

// =============================================================================
// CommitInput — paramètres d'un commit
// =============================================================================

/// Paramètres d'entrée pour le protocole de commit Epoch.
pub struct CommitInput<'a> {
    /// EpochRoot contenant les modifications.
    pub root:             &'a EpochRootInMemory,
    /// Callbacks vers la couche storage (RÈGLE DAG-01).
    pub callbacks:        CommitCallbacks<'a>,
    /// Offset disque où l'EpochRoot sérialisé a été écrit (après Phase 2).
    pub root_disk_offset: DiskOffset,
    /// Offset disque du slot cible pour l'EpochRecord (A, B ou C).
    pub slot_offset:      DiskOffset,
    /// Flags additionnels à fusionner dans l'EpochRecord.
    pub extra_flags:      EpochFlags,
}

// =============================================================================
// Protocole de commit — 3 phases + 3 barrières
// =============================================================================

/// Exécute le protocole de commit Epoch complet avec les 3 barrières NVMe.
///
/// # Invariant de sécurité
/// Cette fonction est la SEULE à pouvoir avancer l'EpochId du superblock.
/// Elle est protégée par EPOCH_COMMIT_LOCK (règle EPOCH-03).
///
/// # Ordre des opérations (INVIOLABLE — règle EPOCH-01)
/// 1. Écriture payload (déjà faite par l'appelant) + BARRIÈRE 1.
/// 2. Écriture EpochRoot (déjà faite par l'appelant) + BARRIÈRE 2.
/// 3. Écriture EpochRecord dans slot + BARRIÈRE 3.
///
/// # Retour
/// - `Ok(CommitResult)` : epoch committé, superblock mis à jour.
/// - `Err(ExofsError::CommitInProgress)` : lock déjà tenu.
/// - Autres erreurs : I/O failure, overflow, OOM, etc.
pub fn commit_epoch(input: CommitInput<'_>) -> ExofsResult<CommitResult> {
    let t_start = (input.callbacks.get_tsc)();

    // ── Acquisition de EPOCH_COMMIT_LOCK (règle EPOCH-03) ──────────────────
    let mut lock = EPOCH_COMMIT_LOCK.lock();
    lock.commit_seq = lock.commit_seq.wrapping_add(1);

    // Calcul du prochain EpochId.
    let current_epoch = (input.callbacks.get_current_epoch)();
    let next_epoch = current_epoch.next().ok_or_else(|| {
        lock.aborted_commits = lock.aborted_commits.saturating_add(1);
        EPOCH_STATS.inc_commits_aborted();
        ExofsError::OffsetOverflow
    })?;

    // ── Phase 1 : BARRIÈRE après payload ───────────────────────────────────
    // Le payload a été écrit par l'appelant AVANT cet appel.
    // RÈGLE EPOCH-01 Phase 1 : nvme_flush() ← OBLIGATOIRE.
    if let Err(e) = nvme_barrier_after_data() {
        lock.aborted_commits = lock.aborted_commits.saturating_add(1);
        EPOCH_STATS.inc_commits_aborted();
        EPOCH_STATS.inc_barrier_failures();
        return Err(e);
    }
    EPOCH_STATS.inc_barriers_data();

    // ── Phase 2 : BARRIÈRE après EpochRoot ─────────────────────────────────
    // L'EpochRoot a été sérialisé et écrit AVANT cet appel.
    // RÈGLE EPOCH-01 Phase 2 : nvme_flush() ← OBLIGATOIRE.
    if let Err(e) = nvme_barrier_after_root() {
        lock.aborted_commits = lock.aborted_commits.saturating_add(1);
        EPOCH_STATS.inc_commits_aborted();
        EPOCH_STATS.inc_barrier_failures();
        return Err(e);
    }
    EPOCH_STATS.inc_barriers_root();

    // ── Phase 3 : Écriture de l'EpochRecord dans le slot ──────────────────
    let object_count = input.root.total_entries() as u32;
    let mut flags = input.root.flags;
    flags.set(EpochFlags::COMMITTED);
    flags.merge(input.extra_flags);

    // Sélectionne un ObjectId représentatif (premier objet modifié, sinon zéro).
    let root_oid = input.root.modified_objects.first()
        .map(|e| ObjectId(e.object_id))
        .unwrap_or(ObjectId([0u8; 32]));

    // Création de l'EpochRecord (104 octets, sans object_count).
    let tsc_now = (input.callbacks.get_tsc)();
    let record = EpochRecord::new(
        next_epoch,
        flags,
        tsc_now,
        root_oid,
        input.root_disk_offset,
        DiskOffset(0), // prev_slot : rempli par le slot selector avant cet appel
    );

    // Sérialise l'EpochRecord en tableau de 104 octets.
    // SAFETY: EpochRecord est #[repr(C, packed)], plain types, taille 104.
    let record_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(&record as *const EpochRecord as *const u8, 104)
    };

    // Écriture physique dans le slot.
    let bytes_written = (input.callbacks.write_fn)(record_bytes, input.slot_offset)
        .map_err(|e| {
            lock.aborted_commits = lock.aborted_commits.saturating_add(1);
            EPOCH_STATS.inc_commits_aborted();
            e
        })?;

    // Vérification bytes_written (RÈGLE WRITE-01).
    if bytes_written != 104 {
        lock.aborted_commits = lock.aborted_commits.saturating_add(1);
        EPOCH_STATS.inc_commits_aborted();
        EPOCH_STATS.inc_partial_commits();
        return Err(ExofsError::PartialWrite);
    }

    // RÈGLE EPOCH-01 Phase 3 : barrière finale OBLIGATOIRE.
    if let Err(e) = nvme_barrier_after_record() {
        // Le record est écrit mais la barrière a échoué : état ambigu.
        // Le commit est considéré comme avorté.
        lock.aborted_commits = lock.aborted_commits.saturating_add(1);
        EPOCH_STATS.inc_commits_aborted();
        EPOCH_STATS.inc_barrier_failures();
        return Err(e);
    }
    EPOCH_STATS.inc_barriers_record();

    // ── Avancement du superblock ────────────────────────────────────────────
    // Injecté via callback — RÈGLE DAG-01.
    (input.callbacks.advance_epoch)(next_epoch).map_err(|e| {
        lock.aborted_commits = lock.aborted_commits.saturating_add(1);
        EPOCH_STATS.inc_commits_aborted();
        e
    })?;

    // ── Statistiques ────────────────────────────────────────────────────────
    let t_end = (input.callbacks.get_tsc)();
    let duration = t_end.saturating_sub(t_start);
    EPOCH_STATS.inc_commits_ok();
    EPOCH_STATS.add_objects_committed(object_count as u64);
    EPOCH_STATS.record_commit_cycles(duration);
    if input.root.flags.contains(EpochFlags::HAS_DELETIONS) {
        EPOCH_STATS.inc_roots_with_deletions();
    }
    lock.total_commits = lock.total_commits.saturating_add(1);

    drop(lock);

    Ok(CommitResult {
        epoch_id:        next_epoch,
        slot_offset:     input.slot_offset,
        object_count,
        duration_cycles: duration,
    })
}

// =============================================================================
// CommitPhase — phase actuelle du protocole (pour diagnostics)
// =============================================================================

/// Phase du protocole de commit (pour les logs d'abort).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CommitPhase {
    /// Pas encore commencé.
    Idle           = 0,
    /// Phase 1 : écriture payload.
    Payload        = 1,
    /// Barrière 1 en cours.
    Barrier1       = 2,
    /// Phase 2 : écriture EpochRoot.
    EpochRoot      = 3,
    /// Barrière 2 en cours.
    Barrier2       = 4,
    /// Phase 3 : écriture EpochRecord.
    EpochRecord    = 5,
    /// Barrière 3 en cours.
    Barrier3       = 6,
    /// Avancement du superblock.
    AdvanceSuperblock = 7,
    /// Commit terminé avec succès.
    Done           = 8,
    /// Commit avorté.
    Aborted        = 255,
}

impl fmt::Display for CommitPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle              => write!(f, "IDLE"),
            Self::Payload           => write!(f, "PAYLOAD"),
            Self::Barrier1          => write!(f, "BARRIER_1"),
            Self::EpochRoot         => write!(f, "EPOCH_ROOT"),
            Self::Barrier2          => write!(f, "BARRIER_2"),
            Self::EpochRecord       => write!(f, "EPOCH_RECORD"),
            Self::Barrier3          => write!(f, "BARRIER_3"),
            Self::AdvanceSuperblock => write!(f, "ADVANCE_SB"),
            Self::Done              => write!(f, "DONE"),
            Self::Aborted           => write!(f, "ABORTED"),
        }
    }
}

// =============================================================================
// AbortReason — raison d'un abort de commit
// =============================================================================

/// Raison de l'abandon d'un commit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AbortReason {
    /// Barrière NVMe Phase 1 échouée.
    Barrier1Failed,
    /// Barrière NVMe Phase 2 échouée.
    Barrier2Failed,
    /// Barrière NVMe Phase 3 échouée.
    Barrier3Failed,
    /// EpochId overflow.
    EpochIdOverflow,
    /// Écriture partielle du slot EpochRecord.
    PartialWrite,
    /// Superblock advance a échoué.
    SuperblockAdvanceFailed,
    /// Lock déjà tenu (commit concurrent).
    LockContention,
}

impl fmt::Display for AbortReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Barrier1Failed          => write!(f, "barrier_1_failed"),
            Self::Barrier2Failed          => write!(f, "barrier_2_failed"),
            Self::Barrier3Failed          => write!(f, "barrier_3_failed"),
            Self::EpochIdOverflow         => write!(f, "epoch_id_overflow"),
            Self::PartialWrite            => write!(f, "partial_write"),
            Self::SuperblockAdvanceFailed => write!(f, "superblock_advance_failed"),
            Self::LockContention          => write!(f, "lock_contention"),
        }
    }
}

// =============================================================================
// Force-commit helper — commit forcé pour pression capacité
// =============================================================================

/// Vérifie si un commit forcé est nécessaire (règle EPOCH-05).
///
/// Retourne `true` si le delta est plein (>= EPOCH_MAX_OBJECTS) ou si
/// l'EpochRoot est plein.
#[inline]
pub fn should_force_commit(root: &EpochRootInMemory) -> bool {
    root.is_full()
}

/// Construit les flags appropriés pour un commit forcé.
pub fn forced_commit_flags(base: EpochFlags) -> EpochFlags {
    let mut f = base;
    f.set(EpochFlags::FORCE_COMMITTED);
    f
}

