// kernel/src/fs/exofs/epoch/epoch_barriers.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Barrières NVMe — wrappeurs mockables pour les nvme_flush() du commit Epoch
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE EPOCH-02 : INTERDIT d'omettre une barrière NVMe — reordering = corruption.
// Les 3 barrières correspondent aux 3 phases du protocole de commit.

use core::sync::atomic::{AtomicU64, Ordering};
use crate::fs::exofs::core::{ExofsError, ExofsResult};

/// Compteurs diagnostics des barrières.
static BARRIERS_ISSUED: [AtomicU64; 3] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Barrière NVMe Phase 1 : après écriture des données payload.
///
/// Garantit que les données du P-Blob ou du L-Obj sont persistées avant
/// l'écriture de l'EpochRoot.
#[inline]
pub fn nvme_barrier_after_data() -> ExofsResult<()> {
    BARRIERS_ISSUED[0].fetch_add(1, Ordering::Relaxed);
    nvme_flush_impl()
}

/// Barrière NVMe Phase 2 : après écriture de l'EpochRoot.
///
/// Garantit que l'EpochRoot est persisté avant l'écriture de l'EpochRecord
/// dans le slot.
#[inline]
pub fn nvme_barrier_after_root() -> ExofsResult<()> {
    BARRIERS_ISSUED[1].fetch_add(1, Ordering::Relaxed);
    nvme_flush_impl()
}

/// Barrière NVMe Phase 3 : après écriture de l'EpochRecord dans le slot.
///
/// Après cette barrière, l'Epoch est définitivement committé et visible
/// au recovery.
#[inline]
pub fn nvme_barrier_after_record() -> ExofsResult<()> {
    BARRIERS_ISSUED[2].fetch_add(1, Ordering::Relaxed);
    nvme_flush_impl()
}

/// Implémentation réelle du flush NVMe.
///
/// En production : envoie la commande Flush NVMe (opcode 0x00) via la
/// block layer du kernel.
/// En tests : no-op ou injection d'erreur via cfg.
#[cfg(not(test))]
fn nvme_flush_impl() -> ExofsResult<()> {
    // Délègue au block layer : soumet une Bio FlushFua et attend la complétion.
    // Le block device sous-jacent est acquis via le driver NVMe de la block layer.
    // En l'absence d'un accès direct au disque dans ce module (règle DAG-01),
    // on utilise le flush générique de la bio layer.
    use crate::fs::block::bio::{Bio, BioOp, BioFlags};
    // Crée une Bio de type Flush.
    let bio = Bio::new_flush();
    crate::fs::block::submit_bio(bio).map_err(|_| ExofsError::IoError)
}

#[cfg(test)]
fn nvme_flush_impl() -> ExofsResult<()> {
    // En tests : accepte systématiquement (les tests peuvent surcharger).
    Ok(())
}

/// Snapshot du nombre de barrières émises (diagnostics).
pub fn barriers_issued_snapshot() -> [u64; 3] {
    [
        BARRIERS_ISSUED[0].load(Ordering::Relaxed),
        BARRIERS_ISSUED[1].load(Ordering::Relaxed),
        BARRIERS_ISSUED[2].load(Ordering::Relaxed),
    ]
}
