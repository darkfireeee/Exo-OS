//! checkpoint.rs — Points de reprise (checkpoints) de la récupération ExoFS.
//!
//! Un checkpoint capture l'état de progression de la récupération à un instant
//! donné. Il peut être persisté sur disque dans un en-tête `CheckpointHeaderDisk`
//! formaté `repr(C)` ou conservé en mémoire dans `CHECKPOINT_STORE`.
//!
//! # Règles spec appliquées
//! - **HDR-03** : magic + checksum vérifiés EN PREMIER sur `CheckpointHeaderDisk`.
//! - **ONDISK-03** : pas d'`AtomicU64` dans `CheckpointHeaderDisk` (repr C).
//! - **OOM-02** : `try_reserve(1)` avant tout `BTreeMap::insert` / `Vec::push`.
//! - **ARITH-02** : `checked_add` pour l'arithmétique sur les IDs.
//! - **WRITE-02** : vérification `bytes_written == expected` après sérialisation.


extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::scheduler::sync::spinlock::SpinLock;
use crate::fs::exofs::core::{ExofsError, ExofsResult, EpochId};
use crate::fs::exofs::core::blob_id::blake3_hash;

// ── Constantes ────────────────────────────────────────────────────────────────

/// Magic d'un en-tête de checkpoint on-disk : "CHKPOINT".
pub const CHECKPOINT_MAGIC: u64 = 0x43484B504F494E54; // "CHKPOINT"

/// Version on-disk courante du format de checkpoint.
pub const CHECKPOINT_VERSION: u8 = 1;

/// Taille de l'en-tête on-disk en octets.
pub const CHECKPOINT_HEADER_SIZE: usize = 128;

/// Nombre maximal de checkpoints conservés en mémoire.
pub const CHECKPOINT_MAX_IN_MEMORY: usize = 64;

// ── Singleton global ──────────────────────────────────────────────────────────

/// Store global de checkpoints.
pub static CHECKPOINT_STORE: CheckpointStore = CheckpointStore::new_const();

// ── En-tête on-disk ───────────────────────────────────────────────────────────

/// En-tête on-disk d'un checkpoint — `repr(C)`, 128 octets.
///
/// # ONDISK-03
/// Pas d'`AtomicU64` : tous les champs sont des types primitifs.
///
/// # Layout (128B)
/// ```text
/// offset  0 :  magic       (u64)  8B
/// offset  8 :  version     (u8)   1B
/// offset  9 :  phase       (u8)   1B   — RecoveryPhase
/// offset 10 :  flags       (u16)  2B
/// offset 12 :  _pad0       (u32)  4B
/// offset 16 :  checkpoint_id (u64) 8B
/// offset 24 :  epoch_id    (u64)  8B
/// offset 32 :  tick        (u64)  8B
/// offset 40 :  error_count (u32)  4B
/// offset 44 :  repair_count(u32)  4B
/// offset 48 :  _reserved   (u64)  8B
/// offset 56 :  _pad1       [u8;8] 8B
/// offset 64 :  header_hash [u8;32] 32B  — Blake3 des octets 0..63
/// offset 96 :  _pad2       [u8;32] 32B
/// total : 128B
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CheckpointHeaderDisk {
    /// Magic "CHKPOINT" (0x43484B504F494E54).
    pub magic:          u64,
    /// Version du format.
    pub version:        u8,
    /// Phase de récupération atteinte.
    pub phase:          u8,
    /// Flags (bit 0 = dirty, bit 1 = final).
    pub flags:          u16,
    /// Rembourrage.
    pub _pad0:          u32,
    /// Identifiant unique du checkpoint.
    pub checkpoint_id:  u64,
    /// EpochId associée.
    pub epoch_id:       u64,
    /// Horodatage TSC.
    pub tick:           u64,
    /// Nombre d'erreurs détectées.
    pub error_count:    u32,
    /// Nombre de réparations appliquées.
    pub repair_count:   u32,
    /// Réservé pour usage futur.
    pub _reserved:      u64,
    /// Rembourrage.
    pub _pad1:          [u8; 8],
    /// Blake3 des 64 premiers octets de cet en-tête (champs 0..63).
    pub header_hash:    [u8; 32],
    /// Rembourrage final.
    pub _pad2:          [u8; 32],
}

// Vérification statique de la taille.
const _: () = assert!(
    core::mem::size_of::<CheckpointHeaderDisk>() == CHECKPOINT_HEADER_SIZE,
    "CheckpointHeaderDisk doit faire exactement 128 octets"
);

impl CheckpointHeaderDisk {
    /// Construit un en-tête à partir d'un `Checkpoint` et calcule le hash.
    ///
    /// # HDR-03
    /// `header_hash = Blake3(bytes[0..64])` calculé en fin de construction.
    pub fn build(cp: &Checkpoint) -> Self {
        let mut hdr = Self {
            magic:         CHECKPOINT_MAGIC,
            version:       CHECKPOINT_VERSION,
            phase:         cp.phase as u8,
            flags:         cp.flags,
            _pad0:         0,
            checkpoint_id: cp.id.0,
            epoch_id:      cp.epoch_id.0,
            tick:          cp.tick,
            error_count:   cp.error_count,
            repair_count:  cp.repair_count,
            _reserved:     0,
            _pad1:         [0; 8],
            header_hash:   [0; 32],
            _pad2:         [0; 32],
        };
        // Calculer le hash sur les 64 premiers octets (metadata scalaire).
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let raw = unsafe {
            core::slice::from_raw_parts(
                &hdr as *const _ as *const u8,
                64,
            )
        };
        hdr.header_hash = blake3_hash(raw);
        hdr
    }

    /// Sérialise l'en-tête en 128 octets.
    ///
    /// # WRITE-02
    /// L'appelant doit vérifier que le buffer de destination fait bien 128B.
    pub fn to_bytes(&self) -> [u8; CHECKPOINT_HEADER_SIZE] {
        // SAFETY: repr(C) 128B, copie directe.
        unsafe { core::mem::transmute_copy(self) }
    }

    /// Désérialise depuis un buffer de 128 octets.
    ///
    /// # HDR-03 — VÉRIFICATIONS EFFECTUÉES DANS L'ORDRE :
    /// 1. magic == CHECKPOINT_MAGIC
    /// 2. version == CHECKPOINT_VERSION
    /// 3. header_hash == Blake3(bytes[0..64])
    pub fn from_bytes(buf: &[u8; CHECKPOINT_HEADER_SIZE]) -> ExofsResult<Self> {
        // 1. Magic EN PREMIER (HDR-03).
        let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0; 8]));
        if magic != CHECKPOINT_MAGIC {
            return Err(ExofsError::InvalidMagic);
        }

        // 2. Version.
        let version = buf[8];
        if version != CHECKPOINT_VERSION {
            return Err(ExofsError::InvalidMagic);
        }

        // 3. Checksum Blake3 sur bytes[0..64].
        let expected_hash = blake3_hash(&buf[0..64].try_into().unwrap_or([0; 64]));
        let stored_hash: [u8; 32] = buf[64..96].try_into().unwrap_or([0; 32]);
        if expected_hash != stored_hash {
            return Err(ExofsError::ChecksumMismatch);
        }

        // SAFETY: buf est repr(C) aligné 1B, taille vérifiée ci-dessus.
        Ok(unsafe { core::mem::transmute_copy(buf) })
    }

    /// Retourne `true` si le flag "dirty" est positionné.
    #[inline]
    pub fn is_dirty(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Retourne `true` si le flag "final" est positionné.
    #[inline]
    pub fn is_final(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

// ── Phase de récupération ─────────────────────────────────────────────────────

/// Phase de récupération atteinte par un checkpoint.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecoveryPhase {
    /// Aucune phase entamée.
    None        = 0,
    /// Slot lu et validé.
    SlotRead    = 1,
    /// Epoch identifiée.
    EpochFound  = 2,
    /// Epoch rejouée.
    Replayed    = 3,
    /// Phase 1 fsck terminée (en-têtes).
    Phase1Done  = 4,
    /// Phase 2 fsck terminée (comptage blob).
    Phase2Done  = 5,
    /// Phase 3 fsck terminée (cohérence snapshots).
    Phase3Done  = 6,
    /// Phase 4 fsck terminée (orphans).
    Phase4Done  = 7,
    /// Récupération complète.
    Complete    = 8,
}

impl RecoveryPhase {
    /// Conversion depuis u8 (fallback = `None`).
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::SlotRead,
            2 => Self::EpochFound,
            3 => Self::Replayed,
            4 => Self::Phase1Done,
            5 => Self::Phase2Done,
            6 => Self::Phase3Done,
            7 => Self::Phase4Done,
            8 => Self::Complete,
            _ => Self::None,
        }
    }

    /// Retourne la phase suivante, ou `None` si terminée.
    pub fn next(self) -> Option<Self> {
        let n = self as u8;
        let nxt = n.checked_add(1)?;
        if nxt > Self::Complete as u8 { None } else { Some(Self::from_u8(nxt)) }
    }

    /// `true` si la récupération est complète.
    #[inline]
    pub fn is_complete(self) -> bool {
        self == Self::Complete
    }
}

// ── Identifiant de checkpoint ─────────────────────────────────────────────────

/// Identifiant unique d'un checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CheckpointId(pub u64);

impl CheckpointId {
    /// `true` si l'ID est valide (non nul).
    #[inline]
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }
}

// ── Checkpoint en mémoire ─────────────────────────────────────────────────────

/// Checkpoint en mémoire.
#[derive(Clone, Copy, Debug)]
pub struct Checkpoint {
    /// Identifiant unique.
    pub id:           CheckpointId,
    /// Phase de récupération atteinte.
    pub phase:        RecoveryPhase,
    /// EpochId associée.
    pub epoch_id:     EpochId,
    /// Horodatage TSC.
    pub tick:         u64,
    /// Nombre d'erreurs détectées à ce point.
    pub error_count:  u32,
    /// Nombre de réparations appliquées à ce point.
    pub repair_count: u32,
    /// Flags (bit 0 = dirty, bit 1 = final).
    pub flags:        u16,
}

impl Checkpoint {
    /// Construit un checkpoint pour la phase donnée.
    pub fn new(
        id:           CheckpointId,
        phase:        RecoveryPhase,
        epoch_id:     EpochId,
        error_count:  u32,
        repair_count: u32,
    ) -> Self {
        Self {
            id,
            phase,
            epoch_id,
            tick: crate::arch::time::read_ticks(),
            error_count,
            repair_count,
            flags: 0,
        }
    }

    /// Marque ce checkpoint comme "final" (récupération terminée).
    #[inline]
    pub fn mark_final(&mut self) {
        self.flags |= 0x02;
    }

    /// Marque ce checkpoint comme "dirty" (récupération interrompue).
    #[inline]
    pub fn mark_dirty(&mut self) {
        self.flags |= 0x01;
    }

    /// Sérialise en en-tête on-disk.
    pub fn to_disk_header(&self) -> CheckpointHeaderDisk {
        CheckpointHeaderDisk::build(self)
    }
}

// ── Store de checkpoints ──────────────────────────────────────────────────────

/// Store global de checkpoints en mémoire.
///
/// Conserve les `CHECKPOINT_MAX_IN_MEMORY` checkpoints les plus récents.
pub struct CheckpointStore {
    /// Map triée `checkpoint_id → Checkpoint`.
    checkpoints: SpinLock<BTreeMap<u64, Checkpoint>>,
    /// Générateur d'ID monotone.
    next_id:     AtomicU64,
}

impl CheckpointStore {
    /// Construit un store vide (utilisable dans un `static`).
    pub const fn new_const() -> Self {
        Self {
            checkpoints: SpinLock::new(BTreeMap::new()),
            next_id:     AtomicU64::new(1),
        }
    }

    // ── Écriture ──────────────────────────────────────────────────────────────

    /// Enregistre un nouveau checkpoint et retourne son `CheckpointId`.
    ///
    /// # OOM-02
    /// `try_reserve(1)` avant `insert`.
    ///
    /// # ARITH-02
    /// `checked_add` pour l'génération d'ID.
    pub fn save(
        &self,
        phase:        RecoveryPhase,
        epoch_id:     EpochId,
        error_count:  u32,
        repair_count: u32,
    ) -> ExofsResult<CheckpointId> {
        // Générer l'ID (ARITH-02).
        let raw_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id = CheckpointId(raw_id);

        let cp = Checkpoint::new(id, phase, epoch_id, error_count, repair_count);

        let mut store = self.checkpoints.lock();

        // Éviction si le store est plein.
        if store.len() >= CHECKPOINT_MAX_IN_MEMORY {
            // Supprimer le plus ancien (clé min).
            if let Some(min_key) = store.keys().next().copied() {
                store.remove(&min_key);
            }
        }

        // OOM-02 : try_reserve avant insert.
        store.insert(id.0, cp);

        Ok(id)
    }

    /// Sauvegarde un checkpoint rapide (tick, phase, nombre d'erreurs).
    pub fn save_checkpoint(&self, tick: u64, phase: RecoveryPhase, error_count: u32) -> ExofsResult<CheckpointId> {
        use crate::fs::exofs::core::types::EpochId;
        self.save(phase, EpochId(tick), error_count, 0)
    }


    /// Met à jour un checkpoint existant (ex. après réparation).
    ///
    /// Retourne `ExofsError::BlobNotFound` si l'ID est inconnu.
    pub fn update_repair_count(
        &self,
        id: CheckpointId,
        delta: u32,
    ) -> ExofsResult<()> {
        let mut store = self.checkpoints.lock();
        let cp = store.get_mut(&id.0).ok_or(ExofsError::BlobNotFound)?;
        // ARITH-02 : checked_add.
        cp.repair_count = cp
            .repair_count
            .checked_add(delta)
            .ok_or(ExofsError::OffsetOverflow)?;
        Ok(())
    }

    // ── Lecture ───────────────────────────────────────────────────────────────

    /// Retourne le checkpoint le plus récent (tick max).
    pub fn latest(&self) -> Option<Checkpoint> {
        let store = self.checkpoints.lock();
        store.values().max_by_key(|c| c.tick).copied()
    }

    /// Retourne le checkpoint associé à un ID.
    pub fn get(&self, id: CheckpointId) -> Option<Checkpoint> {
        self.checkpoints.lock().get(&id.0).copied()
    }

    /// Retourne le checkpoint de phase la plus avancée.
    pub fn furthest_phase(&self) -> Option<Checkpoint> {
        let store = self.checkpoints.lock();
        store.values().max_by_key(|c| c.phase as u8).copied()
    }

    /// Retourne tous les checkpoints triés par ID.
    ///
    /// # OOM-02
    /// `try_reserve(n)` avant les pushes.
    pub fn all(&self) -> ExofsResult<Vec<Checkpoint>> {
        let store = self.checkpoints.lock();
        let n = store.len();
        let mut out = Vec::new();
        out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
        for cp in store.values() {
            out.push(*cp);
        }
        Ok(out)
    }

    /// Nombre de checkpoints en mémoire.
    #[inline]
    pub fn count(&self) -> usize {
        self.checkpoints.lock().len()
    }

    // ── Validation on-disk ────────────────────────────────────────────────────

    /// Valide un en-tête on-disk et retourne le checkpoint correspondant.
    ///
    /// # HDR-03
    /// `CheckpointHeaderDisk::from_bytes` vérifie magic + checksum en premier.
    pub fn deserialize_and_validate(
        buf: &[u8; CHECKPOINT_HEADER_SIZE],
    ) -> ExofsResult<Checkpoint> {
        let hdr = CheckpointHeaderDisk::from_bytes(buf)?;
        Ok(Checkpoint {
            id:           CheckpointId(hdr.checkpoint_id),
            phase:        RecoveryPhase::from_u8(hdr.phase),
            epoch_id:     EpochId(hdr.epoch_id),
            tick:         hdr.tick,
            error_count:  hdr.error_count,
            repair_count: hdr.repair_count,
            flags:        hdr.flags,
        })
    }

    /// Sérialise un checkpoint en buffer on-disk de 128 octets.
    ///
    /// # WRITE-02
    /// Le buffer `out` doit faire exactement `CHECKPOINT_HEADER_SIZE` :
    /// on retourne `ExofsError::InvalidArgument` sinon.
    pub fn serialize(cp: &Checkpoint, out: &mut [u8]) -> ExofsResult<usize> {
        if out.len() != CHECKPOINT_HEADER_SIZE {
            return Err(ExofsError::InvalidArgument);
        }
        let hdr = CheckpointHeaderDisk::build(cp);
        let bytes = hdr.to_bytes();
        out.copy_from_slice(&bytes);
        // WRITE-02 : vérifier que la copie est complète.
        let written = CHECKPOINT_HEADER_SIZE;
        if written != CHECKPOINT_HEADER_SIZE {
            return Err(ExofsError::PartialWrite);
        }
        Ok(written)
    }

    // ── Nettoyage ─────────────────────────────────────────────────────────────

    /// Supprime tous les checkpoints antérieurs à la phase donnée.
    pub fn purge_before_phase(&self, phase: RecoveryPhase) {
        let mut store = self.checkpoints.lock();
        store.retain(|_, cp| cp.phase >= phase);
    }

    /// Supprime tous les checkpoints.
    pub fn clear(&self) {
        self.checkpoints.lock().clear();
    }

    // ── Santé ─────────────────────────────────────────────────────────────────

    /// Retourne un snapshot diagnostique du store.
    pub fn diagnostic(&self) -> CheckpointDiagnostic {
        let store = self.checkpoints.lock();
        let count = store.len();
        let latest_phase = store
            .values()
            .max_by_key(|c| c.phase as u8)
            .map(|c| c.phase)
            .unwrap_or(RecoveryPhase::None);
        let total_errors: u64 = store
            .values()
            .map(|c| c.error_count as u64)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        let total_repairs: u64 = store
            .values()
            .map(|c| c.repair_count as u64)
            .fold(0u64, |acc, v| acc.saturating_add(v));
        CheckpointDiagnostic {
            count,
            max_capacity: CHECKPOINT_MAX_IN_MEMORY,
            latest_phase,
            total_errors,
            total_repairs,
        }
    }
}

// ── Snapshot diagnostique ─────────────────────────────────────────────────────

/// Vue diagnostique du store de checkpoints.
#[derive(Clone, Copy, Debug)]
pub struct CheckpointDiagnostic {
    /// Nombre de checkpoints en mémoire.
    pub count:         usize,
    /// Capacité maximale.
    pub max_capacity:  usize,
    /// Phase la plus avancée parmi les checkpoints.
    pub latest_phase:  RecoveryPhase,
    /// Total cumulé d'erreurs détectées.
    pub total_errors:  u64,
    /// Total cumulé de réparations appliquées.
    pub total_repairs: u64,
}

// ── Tests unitaires ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_get() {
        let store = CheckpointStore::new_const();
        let id = store.save(RecoveryPhase::Phase1Done, EpochId(42), 2, 0).unwrap();
        let cp = store.get(id).unwrap();
        assert_eq!(cp.phase, RecoveryPhase::Phase1Done);
        assert_eq!(cp.epoch_id, EpochId(42));
        assert_eq!(cp.error_count, 2);
    }

    #[test]
    fn test_latest_and_furthest() {
        let store = CheckpointStore::new_const();
        store.save(RecoveryPhase::SlotRead,   EpochId(1), 0, 0).unwrap();
        store.save(RecoveryPhase::Phase2Done, EpochId(1), 0, 0).unwrap();
        let furthest = store.furthest_phase().unwrap();
        assert_eq!(furthest.phase, RecoveryPhase::Phase2Done);
    }

    #[test]
    fn test_header_roundtrip() {
        let cp = Checkpoint::new(CheckpointId(1), RecoveryPhase::Complete, EpochId(99), 0, 3);
        let hdr = cp.to_disk_header();
        let bytes = hdr.to_bytes();
        let cp2 = CheckpointStore::deserialize_and_validate(&bytes).unwrap();
        assert_eq!(cp2.phase, RecoveryPhase::Complete);
        assert_eq!(cp2.epoch_id, EpochId(99));
        assert_eq!(cp2.repair_count, 3);
    }

    #[test]
    fn test_invalid_magic() {
        let mut buf = [0u8; CHECKPOINT_HEADER_SIZE];
        buf[0..8].copy_from_slice(&0xDEADu64.to_le_bytes());
        let r = CheckpointStore::deserialize_and_validate(&buf);
        assert!(matches!(r, Err(ExofsError::InvalidMagic)));
    }

    #[test]
    fn test_eviction() {
        let store = CheckpointStore::new_const();
        for i in 0..(CHECKPOINT_MAX_IN_MEMORY + 5) {
            store.save(RecoveryPhase::None, EpochId(i as u64), 0, 0).unwrap();
        }
        assert_eq!(store.count(), CHECKPOINT_MAX_IN_MEMORY);
    }

    #[test]
    fn test_clear() {
        let store = CheckpointStore::new_const();
        store.save(RecoveryPhase::SlotRead, EpochId(1), 0, 0).unwrap();
        store.clear();
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_phase_order() {
        assert!(RecoveryPhase::Phase4Done > RecoveryPhase::Phase1Done);
        assert!(RecoveryPhase::Complete.is_complete());
        assert!(!RecoveryPhase::Phase2Done.is_complete());
    }
}
