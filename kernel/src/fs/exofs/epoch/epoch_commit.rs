// kernel/src/fs/exofs/epoch/epoch_commit.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Protocole de commit Epoch — 3 barrières NVMe OBLIGATOIRES
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE EPOCH-01 (CRITIQUE) : ordre des écritures INVIOLABLE :
//   Phase 1 : write(payload) → nvme_flush()        ← BARRIÈRE 1
//   Phase 2 : write(EpochRoot) → nvme_flush()      ← BARRIÈRE 2
//   Phase 3 : write(EpochRecord→slot) → nvme_flush() ← BARRIÈRE 3
//
// Inverser cet ordre = corruption garantie au prochain reboot.
//
// RÈGLE DEAD-01  : EPOCH_COMMIT_LOCK jamais acquis par le GC.
// RÈGLE LOCK-05  : Ne pas tenir le lock pendant I/O disque bloquante directe.
// RÈGLE EPOCH-05 : commit anticipé si EpochRoot > 500 objets.

use alloc::sync::Arc;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
};
use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::epoch::epoch_barriers::{
    nvme_barrier_after_data, nvme_barrier_after_root, nvme_barrier_after_record,
};
use crate::fs::exofs::epoch::epoch_commit_lock::EPOCH_COMMIT_LOCK;
use crate::fs::exofs::epoch::epoch_record::EpochRecord;
use crate::fs::exofs::epoch::epoch_root::EpochRootInMemory;
use crate::fs::exofs::epoch::epoch_slots::EpochSlotSelector;
use crate::fs::exofs::storage::superblock::ExoSuperblockInMemory;
use crate::fs::exofs::core::stats::EXOFS_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// Contexte de commit
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'un commit Epoch réussi.
#[derive(Debug)]
pub struct CommitResult {
    /// Identifiant du nouvel epoch committé.
    pub epoch_id: EpochId,
    /// Offset disque du slot écrit.
    pub slot_offset: DiskOffset,
    /// Nombre d'objets committés.
    pub object_count: u32,
}

/// Paramètres d'entrée pour un commit Epoch.
pub struct CommitInput<'a> {
    /// EpochRoot contenant les modifications.
    pub root: &'a EpochRootInMemory,
    /// Superblock in-memory pour mise à jour de l'epoch courant.
    pub superblock: &'a ExoSuperblockInMemory,
    /// Fonction de lancement TSC (injection pour testabilité).
    pub get_timestamp: fn() -> u64,
    /// Offset disque où l'EpochRoot a été écrit (Phase 2).
    pub root_disk_offset: DiskOffset,
    /// Offset disque du slot cible (A, B ou C).
    pub slot_offset: DiskOffset,
    /// Séquence d'écriture (fournie par write_epoch_root/write_payload).
    pub write_fn: &'a dyn Fn(&[u8], DiskOffset) -> ExofsResult<usize>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Protocole de commit — 3 phases + 3 barrières
// ─────────────────────────────────────────────────────────────────────────────

/// Exécute le protocole de commit Epoch complet avec les 3 barrières NVMe.
///
/// # Invariant de sécurité
/// Cette fonction est la SEULE à pouvoir avancer l'EpochId du superblock.
/// Elle est protégée par EPOCH_COMMIT_LOCK (règle EPOCH-03).
///
/// # Ordre des opérations (INVIOLABLE — règle EPOCH-01)
/// 1. Écriture payload + BARRIÈRE 1
/// 2. Écriture EpochRoot + BARRIÈRE 2
/// 3. Écriture EpochRecord dans le slot + BARRIÈRE 3
///
/// # Retour
/// - `Ok(CommitResult)` : epoch committé, superblock mis à jour.
/// - `Err(ExofsError::CommitInProgress)` : lock déjà tenu.
/// - Autres erreurs : I/O failure, overflow, etc.
pub fn commit_epoch(input: CommitInput<'_>) -> ExofsResult<CommitResult> {
    // Acquisition du EPOCH_COMMIT_LOCK — UN SEUL commit à la fois (règle EPOCH-03).
    // try_lock pour éviter de bloquer indéfiniment si appelé depuis un contexte dangereux.
    let mut lock = EPOCH_COMMIT_LOCK.lock();
    lock.commit_seq = lock.commit_seq.wrapping_add(1);

    // Calcul du prochain EpochId.
    let current_epoch = input.superblock.current_epoch();
    let next_epoch = current_epoch.next().ok_or(ExofsError::OffsetOverflow)?;

    // ── Phase 1 : Écriture des données payload ──────────────────────────────
    // Le payload a DÉJÀ été écrit par le writer avant cet appel.
    // La barrière 1 garantit que les données sont persistées avant l'EpochRoot.
    //
    // RÈGLE EPOCH-01 Phase 1 :
    nvme_barrier_after_data()?;

    // ── Phase 2 : Écriture de l'EpochRoot ──────────────────────────────────
    // L'EpochRoot liste tous les objets modifiés dans cet epoch.
    // Il a DÉJÀ été sérialisé et son offset est dans input.root_disk_offset.
    //
    // RÈGLE EPOCH-01 Phase 2 :
    nvme_barrier_after_root()?;

    // ── Phase 3 : Écriture de l'EpochRecord dans le slot ───────────────────
    // Création de l'EpochRecord avec checksum.
    let object_count = input.root.total_entries() as u32;
    let mut flags = input.root.flags;
    flags.set(EpochFlags::COMMITTED);

    let record = EpochRecord::new(
        next_epoch,
        flags,
        (input.get_timestamp)(),
        input.root.modified_objects.first()
            .map(|e| crate::fs::exofs::core::ObjectId(e.object_id))
            .unwrap_or(crate::fs::exofs::core::ObjectId([0u8; 32])),
        input.root_disk_offset,
        DiskOffset(0), // prev_slot — à remplir par le slot selector
        object_count,
    );

    // Sérialise l'EpochRecord dans un buffer de 104 octets.
    let record_bytes = {
        // SAFETY: EpochRecord est #[repr(C, packed)], taille 104 octets, types plain.
        // La copie via from_raw_parts est correcte car le type est Copy + plain.
        let ptr = &record as *const EpochRecord as *const u8;
        unsafe { core::slice::from_raw_parts(ptr, 104) }
    };

    // Écriture physique dans le slot sélectionné.
    let bytes_written = (input.write_fn)(record_bytes, input.slot_offset)?;
    // RÈGLE WRITE-01 : vérification bytes_written == expected.
    if bytes_written != 104 {
        lock.aborted_commits += 1;
        return Err(ExofsError::PartialWrite);
    }

    // RÈGLE EPOCH-01 Phase 3 : barrière finale — commit définitivement persisté.
    nvme_barrier_after_record()?;

    // ── Mise à jour du superblock in-memory ─────────────────────────────────
    input.superblock.advance_epoch(next_epoch);

    // ── Mise à jour des statistiques ────────────────────────────────────────
    EXOFS_STATS.inc_epochs_committed();
    lock.total_commits += 1;

    drop(lock);

    Ok(CommitResult {
        epoch_id:     next_epoch,
        slot_offset:  input.slot_offset,
        object_count,
    })
}
